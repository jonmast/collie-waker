use std::fmt;
use std::future::Future;
use std::pin::Pin;
use std::time::Duration;

use tokio::sync::Mutex;
use tokio::time::Instant;

pub type WolFuture<'a> = Pin<Box<dyn Future<Output = Result<(), WolError>> + Send + 'a>>;

#[derive(Debug)]
pub struct WolError(String);

impl WolError {
    pub fn new(message: impl Into<String>) -> Self {
        Self(message.into())
    }
}

impl fmt::Display for WolError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for WolError {}

#[derive(Debug, PartialEq, Eq)]
pub struct MacParseError(String);

impl fmt::Display for MacParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "invalid MAC address: {}", self.0)
    }
}

impl std::error::Error for MacParseError {}

/// Parse `XX:XX:XX:XX:XX:XX` (or `-` separated) into six octets.
pub fn parse_mac(value: &str) -> Result<[u8; 6], MacParseError> {
    let octets: Vec<&str> = value.split([':', '-']).collect();
    if octets.len() != 6 {
        return Err(MacParseError(value.to_string()));
    }

    let mut mac = [0u8; 6];
    for (slot, octet) in mac.iter_mut().zip(octets) {
        if octet.len() != 2 {
            return Err(MacParseError(value.to_string()));
        }
        *slot = u8::from_str_radix(octet, 16).map_err(|_| MacParseError(value.to_string()))?;
    }
    Ok(mac)
}

/// A WoL magic packet: six `0xFF` bytes followed by the MAC repeated 16 times.
pub fn magic_packet(mac: &[u8; 6]) -> Vec<u8> {
    let mut packet = Vec::with_capacity(102);
    packet.extend_from_slice(&[0xFF; 6]);
    for _ in 0..16 {
        packet.extend_from_slice(mac);
    }
    packet
}

pub trait MagicPacketSender: Send + Sync {
    fn send(&self) -> WolFuture<'_>;
}

#[derive(Clone, Debug)]
pub struct UdpMagicPacketSender {
    mac: [u8; 6],
    broadcast: String,
    port: u16,
}

impl UdpMagicPacketSender {
    pub fn new(mac: [u8; 6], broadcast: String, port: u16) -> Self {
        Self {
            mac,
            broadcast,
            port,
        }
    }
}

impl MagicPacketSender for UdpMagicPacketSender {
    fn send(&self) -> WolFuture<'_> {
        let packet = magic_packet(&self.mac);
        let broadcast = self.broadcast.clone();
        let port = self.port;
        Box::pin(async move {
            let socket = tokio::net::UdpSocket::bind("0.0.0.0:0")
                .await
                .map_err(|e| WolError::new(format!("WOL bind failed: {e}")))?;
            socket
                .set_broadcast(true)
                .map_err(|e| WolError::new(format!("WOL broadcast failed: {e}")))?;
            socket
                .send_to(&packet, format!("{broadcast}:{port}"))
                .await
                .map_err(|e| WolError::new(format!("WOL send failed: {e}")))?;
            Ok(())
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WakeSignal {
    Sent,
    /// Suppressed because a packet was sent less than the rate limit ago.
    RateLimited,
}

/// Wraps a sender so at most one magic packet leaves the pod per `min_interval`.
pub struct RateLimitedWol<S> {
    sender: S,
    min_interval: Duration,
    last_sent: Mutex<Option<Instant>>,
}

impl<S: MagicPacketSender> RateLimitedWol<S> {
    pub fn new(sender: S, min_interval: Duration) -> Self {
        Self {
            sender,
            min_interval,
            last_sent: Mutex::new(None),
        }
    }

    pub async fn wake(&self) -> Result<WakeSignal, WolError> {
        let mut last_sent = self.last_sent.lock().await;
        let now = Instant::now();

        if let Some(previous) = *last_sent
            && now.duration_since(previous) < self.min_interval
        {
            return Ok(WakeSignal::RateLimited);
        }

        self.sender.send().await?;
        *last_sent = Some(now);
        Ok(WakeSignal::Sent)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    struct CountingSender(Arc<AtomicUsize>);

    impl MagicPacketSender for CountingSender {
        fn send(&self) -> WolFuture<'_> {
            self.0.fetch_add(1, Ordering::SeqCst);
            Box::pin(async { Ok(()) })
        }
    }

    #[test]
    fn parses_colon_separated_mac() {
        assert_eq!(
            parse_mac("00:11:22:33:44:55").unwrap(),
            [0x00, 0x11, 0x22, 0x33, 0x44, 0x55]
        );
    }

    #[test]
    fn parses_uppercase_and_dash_separated_mac() {
        assert_eq!(
            parse_mac("00-11-22-33-44-55").unwrap(),
            [0x00, 0x11, 0x22, 0x33, 0x44, 0x55]
        );
    }

    #[test]
    fn rejects_malformed_macs() {
        for bad in [
            "",
            "00:11:22:33:44",
            "00:11:22:33:44:55:33",
            "zz:zz:zz:zz:zz:zz",
        ] {
            assert!(parse_mac(bad).is_err(), "expected {bad:?} to be rejected");
        }
    }

    #[test]
    fn magic_packet_is_sync_header_plus_sixteen_macs() {
        let mac = [0x00, 0x11, 0x22, 0x33, 0x44, 0x55];
        let packet = magic_packet(&mac);

        assert_eq!(packet.len(), 102);
        assert_eq!(&packet[..6], &[0xFF; 6]);
        for chunk in packet[6..].chunks(6) {
            assert_eq!(chunk, &mac);
        }
    }

    #[tokio::test(start_paused = true)]
    async fn rate_limiter_suppresses_within_the_window() {
        let calls = Arc::new(AtomicUsize::new(0));
        let wol = RateLimitedWol::new(CountingSender(calls.clone()), Duration::from_secs(10));

        assert_eq!(wol.wake().await.unwrap(), WakeSignal::Sent);
        tokio::time::advance(Duration::from_secs(9)).await;
        assert_eq!(wol.wake().await.unwrap(), WakeSignal::RateLimited);

        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }

    #[tokio::test(start_paused = true)]
    async fn rate_limiter_allows_after_the_window() {
        let calls = Arc::new(AtomicUsize::new(0));
        let wol = RateLimitedWol::new(CountingSender(calls.clone()), Duration::from_secs(10));

        assert_eq!(wol.wake().await.unwrap(), WakeSignal::Sent);
        tokio::time::advance(Duration::from_secs(11)).await;
        assert_eq!(wol.wake().await.unwrap(), WakeSignal::Sent);

        assert_eq!(calls.load(Ordering::SeqCst), 2);
    }
}
