//! Axum handler for the Prometheus scrape endpoint.
//!
//! Mount with:
//!
//! ```ignore
//! use axum::{routing::get, Router};
//! use submux::telemetry::exporters::prometheus::metrics_handler;
//!
//! let app: Router = Router::new().route("/metrics", get(metrics_handler));
//! ```

use axum::http::{header, HeaderValue, StatusCode};
use axum::response::IntoResponse;

/// `GET /metrics` — renders the process-wide registry as Prometheus 0.0.4
/// exposition-format text.
pub async fn metrics_handler() -> impl IntoResponse {
    let body = crate::telemetry::metrics::registry().render_prometheus();
    (
        StatusCode::OK,
        [(
            header::CONTENT_TYPE,
            HeaderValue::from_static("text/plain; version=0.0.4"),
        )],
        body,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::to_bytes;

    #[tokio::test]
    async fn metrics_handler_returns_prometheus_body() {
        crate::telemetry::metrics::record_request("anthropic", "claude-3-5-sonnet", "ok", 0.1);

        let resp = metrics_handler().await.into_response();
        assert_eq!(resp.status(), StatusCode::OK);
        let ctype = resp
            .headers()
            .get(header::CONTENT_TYPE)
            .expect("content-type")
            .to_str()
            .unwrap()
            .to_string();
        assert!(ctype.starts_with("text/plain"), "got: {ctype}");

        let body = to_bytes(resp.into_body(), 1024 * 1024)
            .await
            .expect("read body");
        let s = std::str::from_utf8(&body).expect("utf8");
        assert!(s.contains("submux_requests_total"));
    }
}
