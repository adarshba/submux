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

use axum::http::{HeaderValue, StatusCode, header};
use axum::response::IntoResponse;

/// `GET /metrics` — renders the OTel Prometheus reader as 0.0.4 exposition text.
pub async fn metrics_handler() -> impl IntoResponse {
    let body = crate::telemetry::metrics::render_prometheus_text();
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
    async fn metrics_handler_returns_prometheus_content_type() {
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

        // Body is empty until metrics::init runs at startup; just ensure the
        // handler renders without error here.
        let _ = to_bytes(resp.into_body(), 1024 * 1024)
            .await
            .expect("read body");
    }
}
