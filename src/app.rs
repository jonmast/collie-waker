use std::sync::Arc;
use std::time::Duration;

use axum::{
    Router,
    body::Body,
    extract::State,
    http::{StatusCode, Uri, header},
    response::{IntoResponse, Response},
    routing::get,
};

use crate::wake::{WakeOutcome, WakeService};

#[derive(Clone)]
pub struct AppState {
    pub waker: Arc<dyn WakeService>,
    /// Host the redirect points back at, i.e. the Traefik front door.
    pub public_host: String,
    /// How long the 503 page waits before reloading itself.
    pub retry_after: Duration,
}

/// Router for a process that serves no wake handler — the tunnel role. It
/// exists only so the kubelet has a liveness endpoint; everything else 404s
/// rather than silently behaving like the fallback.
pub fn health_only_router() -> Router {
    Router::new().route("/healthz", get(healthz))
}

pub fn build_router(state: AppState) -> Router {
    Router::new()
        // Reserved: the kubelet liveness probe. Collie is not reachable at this
        // path while the waker is serving, so nothing is shadowed in practice.
        .route("/healthz", get(healthz))
        .fallback(wake_handler)
        .with_state(state)
}

async fn healthz() -> &'static str {
    "ok"
}

async fn wake_handler(State(state): State<AppState>, uri: Uri) -> Response {
    match state.waker.wake().await {
        WakeOutcome::Awake => redirect(&state.public_host, &uri),
        WakeOutcome::TimedOut => timeout_page(state.retry_after),
    }
}

/// Send the client back to the same URL on the public host. Traefik's failover
/// should now pick the primary, so the retry reaches Collie itself.
fn redirect(public_host: &str, uri: &Uri) -> Response {
    let location = redirect_target(public_host, uri);
    tracing::info!(%location, "redirecting to woken collie");

    (
        StatusCode::TEMPORARY_REDIRECT,
        [
            (header::LOCATION, location),
            (header::CACHE_CONTROL, "no-store".to_string()),
        ],
    )
        .into_response()
}

pub fn redirect_target(public_host: &str, uri: &Uri) -> String {
    let path_and_query = uri.path_and_query().map(|p| p.as_str()).unwrap_or("/");
    format!("https://{public_host}{path_and_query}")
}

fn timeout_page(retry_after: Duration) -> Response {
    let seconds = retry_after.as_secs().max(1);

    Response::builder()
        .status(StatusCode::SERVICE_UNAVAILABLE)
        .header(header::CONTENT_TYPE, "text/html; charset=utf-8")
        .header(header::CACHE_CONTROL, "no-store")
        .header(header::RETRY_AFTER, seconds.to_string())
        .body(Body::from(timeout_html(seconds)))
        .expect("static response builds")
}

fn timeout_html(seconds: u64) -> String {
    format!(
        r#"<!DOCTYPE html>
<html lang="en">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<meta http-equiv="refresh" content="{seconds}">
<title>Waking Collie…</title>
<style>
  body {{ font-family: system-ui, sans-serif; margin: 0; min-height: 100vh;
         display: grid; place-items: center; background: #11131a; color: #e6e8ef; }}
  main {{ text-align: center; padding: 2rem; }}
  h1 {{ font-size: 1.25rem; font-weight: 600; }}
  p {{ color: #9aa0b4; }}
</style>
</head>
<body>
<main>
  <h1>Waking the dev box…</h1>
  <p>It did not answer in time. Retrying in {seconds} seconds.</p>
</main>
</body>
</html>
"#
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::wake::WakeFuture;
    use axum::http::Request;
    use http_body_util::BodyExt;
    use tower::ServiceExt;

    struct StubWaker(WakeOutcome);

    impl WakeService for StubWaker {
        fn wake(&self) -> WakeFuture<'_> {
            let outcome = self.0;
            Box::pin(async move { outcome })
        }
    }

    fn router(outcome: WakeOutcome) -> Router {
        build_router(AppState {
            waker: Arc::new(StubWaker(outcome)),
            public_host: "collie.example.com".to_string(),
            retry_after: Duration::from_secs(5),
        })
    }

    fn uri(value: &str) -> Uri {
        value.parse().unwrap()
    }

    #[test]
    fn redirect_target_preserves_path_and_query() {
        assert_eq!(
            redirect_target("collie.example.com", &uri("/api/snapshot?since=7")),
            "https://collie.example.com/api/snapshot?since=7"
        );
    }

    #[test]
    fn redirect_target_defaults_to_root() {
        assert_eq!(
            redirect_target("collie.example.com", &uri("/")),
            "https://collie.example.com/"
        );
    }

    #[tokio::test]
    async fn woken_request_gets_a_307_back_to_the_public_host() {
        let response = router(WakeOutcome::Awake)
            .oneshot(
                Request::builder()
                    .uri("/api/snapshot?since=7")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::TEMPORARY_REDIRECT);
        assert_eq!(
            response.headers()[header::LOCATION],
            "https://collie.example.com/api/snapshot?since=7"
        );
    }

    #[tokio::test]
    async fn timed_out_request_gets_the_503_auto_reload_page() {
        let response = router(WakeOutcome::TimedOut)
            .oneshot(Request::builder().uri("/").body(Body::empty()).unwrap())
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(response.headers()[header::RETRY_AFTER], "5");

        let body = response.into_body().collect().await.unwrap().to_bytes();
        let body = String::from_utf8(body.to_vec()).unwrap();
        assert!(body.contains(r#"http-equiv="refresh" content="5""#));
    }

    #[tokio::test]
    async fn healthz_does_not_trigger_a_wake() {
        let response = router(WakeOutcome::TimedOut)
            .oneshot(
                Request::builder()
                    .uri("/healthz")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
    }
}
