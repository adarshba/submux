//! Request-id middleware: extracts or generates `X-Request-Id`, stores it as
//! a `RequestId` extension, and echoes it on the response.

use axum::{extract::Request, http::HeaderValue, middleware::Next, response::Response};

use crate::constants::http_headers::X_REQUEST_ID;
use crate::telemetry::tracer::new_request_id;

/// Per-request id (`req_<ULID>`) carried as an axum extension.
#[derive(Debug, Clone)]
pub struct RequestId(pub String);

/// Ensure every request has an id, surface it via extensions, and stamp the
/// response with the same `X-Request-Id` header.
pub async fn request_id_middleware(mut req: Request, next: Next) -> Response {
    let id: String = req
        .headers()
        .get(&X_REQUEST_ID)
        .and_then(|v| v.to_str().ok())
        .map(|s| s.to_owned())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(new_request_id);

    req.extensions_mut().insert(RequestId(id.clone()));

    let mut response = next.run(req).await;

    if let Ok(value) = HeaderValue::from_str(&id) {
        response.headers_mut().insert(X_REQUEST_ID, value);
    }

    response
}
