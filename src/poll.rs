use std::future::Future;
use std::pin::Pin;
use std::time::Duration;

pub type PollFuture<'a> = Pin<Box<dyn Future<Output = bool> + Send + 'a>>;

/// Probes Collie through the SSH tunnel. Any answer at all means the dev box is
/// awake — an auth/authorization rejection still proves Collie is serving.
pub trait ColliePoller: Send + Sync {
    fn is_awake(&self) -> PollFuture<'_>;
}

pub struct HttpColliePoller {
    client: reqwest::Client,
    poll_url: String,
    public_host: String,
}

impl HttpColliePoller {
    pub fn new(poll_url: String, public_host: String, request_timeout: Duration) -> Self {
        let client = reqwest::Client::builder()
            .timeout(request_timeout)
            .build()
            .expect("reqwest client builds with default TLS backend");

        Self {
            client,
            poll_url,
            public_host,
        }
    }
}

impl ColliePoller for HttpColliePoller {
    fn is_awake(&self) -> PollFuture<'_> {
        Box::pin(async move {
            // Collie fails closed on an unknown Host, so send the public host it
            // has in COLLIE_PUBLIC_HOSTS rather than the tunnel's 127.0.0.1.
            let result = self
                .client
                .get(&self.poll_url)
                .header(reqwest::header::HOST, &self.public_host)
                .send()
                .await;

            match result {
                Ok(response) => {
                    tracing::debug!(status = %response.status(), "collie answered");
                    true
                }
                Err(error) => {
                    tracing::debug!(%error, "collie not answering yet");
                    false
                }
            }
        })
    }
}
