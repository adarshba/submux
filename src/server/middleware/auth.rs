//! Inbound shared-secret auth gate.
//!
//! When an [`ApiKey`] is wired into the layer, every request must carry the
//! same value via `Authorization: Bearer <key>` or `x-api-key`. Otherwise the
//! request is rejected with a 401 in the meridian-shaped envelope so that
//! Anthropic and OpenAI SDK clients render the error consistently.
//!
//! Apply this layer only to proxy routes. Health, readiness, and metrics
//! intentionally stay open so deployments and Prometheus keep working.

use axum::{
    extract::Request,
    middleware::Next,
    response::{IntoResponse, Response},
};
use http::{HeaderMap, StatusCode};
use std::sync::Arc;

use crate::constants::http_headers::X_API_KEY;
use crate::core::ApiKey;

const BEARER_PREFIX: &str = "Bearer ";

/// Axum middleware that rejects unauthenticated requests when a key is set.
pub async fn require_api_key(
    axum::extract::State(key): axum::extract::State<Arc<ApiKey>>,
    req: Request,
    next: Next,
) -> Response {
    match extract_candidate(req.headers()) {
        Some(candidate) if key.verify(candidate.as_bytes()) => next.run(req).await,
        _ => unauthorized_response(),
    }
}

fn extract_candidate(headers: &HeaderMap) -> Option<&str> {
    if let Some(value) = headers.get(&X_API_KEY).and_then(|v| v.to_str().ok()) {
        let trimmed = value.trim();
        if !trimmed.is_empty() {
            return Some(trimmed);
        }
    }
    let auth = headers
        .get(http::header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())?;
    auth.strip_prefix(BEARER_PREFIX)
        .map(str::trim)
        .filter(|s| !s.is_empty())
}

fn unauthorized_response() -> Response {
    let body = serde_json::json!({
        "type": "error",
        "error": {
            "type": "authentication_error",
            "message": "Invalid or missing API key",
        }
    })
    .to_string();
    let mut resp = (StatusCode::UNAUTHORIZED, body).into_response();
    resp.headers_mut().insert(
        http::header::CONTENT_TYPE,
        http::HeaderValue::from_static("application/json"),
    );
    resp
}

#[cfg(test)]
mod tests {
    use super::*;
    use http::HeaderValue;

    fn headers_with(name: http::HeaderName, value: &str) -> HeaderMap {
        let mut h = HeaderMap::new();
        h.insert(name, HeaderValue::from_str(value).expect("valid value"));
        h
    }

    #[test]
    fn extract_prefers_x_api_key() {
        let h = headers_with(X_API_KEY, "smx_live_aaaa");
        assert_eq!(extract_candidate(&h), Some("smx_live_aaaa"));
    }

    #[test]
    fn extract_falls_back_to_bearer() {
        let h = headers_with(http::header::AUTHORIZATION, "Bearer smx_live_bbbb");
        assert_eq!(extract_candidate(&h), Some("smx_live_bbbb"));
    }

    #[test]
    fn extract_ignores_empty_values() {
        let h = headers_with(X_API_KEY, "   ");
        assert_eq!(extract_candidate(&h), None);
    }

    #[test]
    fn extract_ignores_non_bearer_auth() {
        let h = headers_with(http::header::AUTHORIZATION, "Basic dXNlcjpwYXNz");
        assert_eq!(extract_candidate(&h), None);
    }

    #[test]
    fn extract_returns_none_when_absent() {
        assert_eq!(extract_candidate(&HeaderMap::new()), None);
    }
}
