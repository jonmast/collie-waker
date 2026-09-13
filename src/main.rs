use std::net::SocketAddr;
use std::sync::Arc;

use collie_waker::{
    app::{AppState, build_router},
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
    tokio::spawn(tunnel::supervise(config.ssh.clone()));

    let waker = Waker::new(
        RateLimitedWol::new(
            UdpMagicPacketSender::new(
                config.wol.mac,
                config.wol.broadcast.clone(),
                config.wol.port,
            ),
            config.wol.rate_limit,
        ),
        HttpColliePoller::new(
            config.waker.poll_url.clone(),
            config.waker.public_host.clone(),
            config.waker.poll_interval,
        ),
        config.waker.timings(),
    );

    let app = build_router(AppState {
        waker: Arc::new(waker),
        public_host: config.waker.public_host.clone(),
        retry_after: config.waker.health_interval,
    });

    let addr = SocketAddr::from(([0, 0, 0, 0], config.listen_port));
    let listener = tokio::net::TcpListener::bind(addr).await?;

    tracing::info!(address = %addr, public_host = %config.waker.public_host, "listening");
    axum::serve(listener, app).await?;

    Ok(())
}
