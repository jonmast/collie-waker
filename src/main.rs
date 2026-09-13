use std::net::SocketAddr;
use std::sync::Arc;

use collie_waker::{
    app::{AppState, build_router, health_only_router},
    config::AppConfig,
    poll::HttpColliePoller,
    tunnel,
    wake::Waker,
    wol::{RateLimitedWol, UdpMagicPacketSender},
};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .with_target(false)
        .compact()
        .init();

    let config = AppConfig::from_env()?;

    // The tunnel is the Traefik *primary* upstream, so it is maintained for the
    // lifetime of the pod, independently of any wake request.
    if let Some(ssh) = config.ssh.clone() {
        tokio::spawn(tunnel::supervise(ssh));
    }

    // Both roles serve HTTP: the waker serves the wake handler, and the tunnel
    // serves /healthz alone so the kubelet has something to probe.
    let app = match config.waker.clone() {
        Some(waker_config) => {
            let wol = config
                .wol
                .clone()
                .expect("wol config is present whenever waker config is");

            let waker = Waker::new(
                RateLimitedWol::new(
                    UdpMagicPacketSender::new(wol.mac, wol.broadcast, wol.port),
                    wol.rate_limit,
                ),
                HttpColliePoller::new(
                    waker_config.poll_url.clone(),
                    waker_config.public_host.clone(),
                    waker_config.poll_interval,
                ),
                waker_config.timings(),
            );

            build_router(AppState {
                waker: Arc::new(waker),
                public_host: waker_config.public_host,
                retry_after: waker_config.health_interval,
            })
        }
        None => health_only_router(),
    };

    let addr = SocketAddr::from(([0, 0, 0, 0], config.listen_port));
    let listener = tokio::net::TcpListener::bind(addr).await?;

    tracing::info!(address = %addr, role = ?config.role, "listening");
    axum::serve(listener, app).await?;

    Ok(())
}
