use std::future::Future;
use std::pin::Pin;
use std::time::Duration;

use tokio::time::Instant;

use crate::poll::ColliePoller;
use crate::wol::{MagicPacketSender, RateLimitedWol, WakeSignal};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WakeOutcome {
    /// Collie answered and the failover has had a health-check interval to flip
    /// back to the primary. Safe to redirect.
    Awake,
    /// The budget expired before Collie answered.
    TimedOut,
}

pub type WakeFuture<'a> = Pin<Box<dyn Future<Output = WakeOutcome> + Send + 'a>>;

pub trait WakeService: Send + Sync {
    fn wake(&self) -> WakeFuture<'_>;
}

#[derive(Clone, Copy, Debug)]
pub struct WakeTimings {
    /// Give up after this long and serve the 503 auto-reload page.
    pub timeout: Duration,
    /// One Traefik health-check interval, waited after Collie answers.
    pub health_interval: Duration,
    /// Delay between polls of Collie.
    pub poll_interval: Duration,
}

pub struct Waker<S, P> {
    wol: RateLimitedWol<S>,
    poller: P,
    timings: WakeTimings,
}

impl<S: MagicPacketSender, P: ColliePoller> Waker<S, P> {
    pub fn new(wol: RateLimitedWol<S>, poller: P, timings: WakeTimings) -> Self {
        Self {
            wol,
            poller,
            timings,
        }
    }

    async fn run(&self) -> WakeOutcome {
        let deadline = Instant::now() + self.timings.timeout;

        loop {
            match self.wol.wake().await {
                Ok(WakeSignal::Sent) => tracing::info!("sent Wake-on-LAN magic packet"),
                Ok(WakeSignal::RateLimited) => tracing::debug!("magic packet rate-limited"),
                Err(error) => tracing::warn!(%error, "failed to send magic packet"),
            }

            if self.poller.is_awake().await {
                // Collie is up, but Traefik is still routing here. Give the
                // failover one health-check interval to notice the primary
                // recovered, otherwise the redirect lands back on the waker.
                tracing::info!("collie is awake, waiting for failover to flip back");
                tokio::time::sleep(self.timings.health_interval).await;
                return WakeOutcome::Awake;
            }

            if Instant::now() + self.timings.poll_interval >= deadline {
                tracing::warn!("gave up waiting for collie");
                return WakeOutcome::TimedOut;
            }

            tokio::time::sleep(self.timings.poll_interval).await;
        }
    }
}

impl<S: MagicPacketSender, P: ColliePoller> WakeService for Waker<S, P> {
    fn wake(&self) -> WakeFuture<'_> {
        Box::pin(self.run())
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::poll::PollFuture;
    use crate::wol::{WolError, WolFuture};
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    pub(crate) struct CountingSender(pub Arc<AtomicUsize>);

    impl MagicPacketSender for CountingSender {
        fn send(&self) -> WolFuture<'_> {
            self.0.fetch_add(1, Ordering::SeqCst);
            Box::pin(async { Ok(()) })
        }
    }

    pub(crate) struct FailingSender;

    impl MagicPacketSender for FailingSender {
        fn send(&self) -> WolFuture<'_> {
            Box::pin(async { Err(WolError::new("no network")) })
        }
    }

    /// Reports "asleep" for the first `awake_after` polls, then "awake".
    pub(crate) struct ScriptedPoller {
        awake_after: usize,
        polls: AtomicUsize,
    }

    impl ScriptedPoller {
        pub(crate) fn new(awake_after: usize) -> Self {
            Self {
                awake_after,
                polls: AtomicUsize::new(0),
            }
        }
    }

    impl ColliePoller for ScriptedPoller {
        fn is_awake(&self) -> PollFuture<'_> {
            let seen = self.polls.fetch_add(1, Ordering::SeqCst);
            Box::pin(async move { seen >= self.awake_after })
        }
    }

    pub(crate) fn timings() -> WakeTimings {
        WakeTimings {
            timeout: Duration::from_secs(45),
            health_interval: Duration::from_secs(5),
            poll_interval: Duration::from_secs(1),
        }
    }

    fn waker<S: MagicPacketSender, P: ColliePoller>(sender: S, poller: P) -> Waker<S, P> {
        Waker::new(
            RateLimitedWol::new(sender, Duration::from_secs(10)),
            poller,
            timings(),
        )
    }

    #[tokio::test(start_paused = true)]
    async fn returns_awake_once_collie_answers() {
        let calls = Arc::new(AtomicUsize::new(0));
        let waker = waker(CountingSender(calls.clone()), ScriptedPoller::new(0));

        assert_eq!(waker.run().await, WakeOutcome::Awake);
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }

    #[tokio::test(start_paused = true)]
    async fn waits_one_health_interval_before_reporting_awake() {
        let calls = Arc::new(AtomicUsize::new(0));
        let waker = waker(CountingSender(calls), ScriptedPoller::new(0));

        let started = Instant::now();
        assert_eq!(waker.run().await, WakeOutcome::Awake);

        assert!(started.elapsed() >= timings().health_interval);
    }

    #[tokio::test(start_paused = true)]
    async fn polls_until_collie_wakes_up() {
        let calls = Arc::new(AtomicUsize::new(0));
        let waker = waker(CountingSender(calls.clone()), ScriptedPoller::new(12));

        assert_eq!(waker.run().await, WakeOutcome::Awake);
        // 13 poll rounds over ~12s, but the 10s rate limit caps the packets.
        assert_eq!(calls.load(Ordering::SeqCst), 2);
    }

    #[tokio::test(start_paused = true)]
    async fn times_out_when_collie_never_answers() {
        let calls = Arc::new(AtomicUsize::new(0));
        let waker = waker(CountingSender(calls), ScriptedPoller::new(usize::MAX));

        let started = Instant::now();
        assert_eq!(waker.run().await, WakeOutcome::TimedOut);

        assert!(started.elapsed() <= timings().timeout);
    }

    #[tokio::test(start_paused = true)]
    async fn keeps_polling_when_the_magic_packet_fails() {
        let waker = waker(FailingSender, ScriptedPoller::new(3));

        assert_eq!(waker.run().await, WakeOutcome::Awake);
    }
}
