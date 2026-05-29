//! Shared route helpers: error envelopes, model sniffing, terminal metrics.
//!
//! Each upstream surface emits errors in its own native shape:
//! - Anthropic `/v1/messages` and the Codex `/codex/responses` proxy use the
//!   Anthropic envelope `{"type":"error","error":{...}}`.
//! - OpenAI `/v1/chat/completions` uses `{"error":{...}}` without the outer
//!   `type` discriminator.

use axum::body::Body;
use axum::response::{IntoResponse, Response};
use bytes::Bytes;
use http::{HeaderMap, StatusCode};
use std::time::Instant;

use crate::constants::http_headers::X_PROXY_USER_ID;
use crate::core::ConsumerId;
use crate::providers::anthropic::PassthroughResponse;
use crate::telemetry::metrics;

/// Resolve the inbound [`ConsumerId`] from the `X-Proxy-User-Id` header sent by
/// the calling client, folding a missing or malformed value into `unknown`.
pub fn consumer_from_headers(headers: &HeaderMap) -> ConsumerId {
    ConsumerId::parse_or_unknown(headers.get(&X_PROXY_USER_ID).and_then(|v| v.to_str().ok()))
}

/// Stream a provider [`PassthroughResponse`] back to the client, preserving the
/// upstream status and (hop-by-hop-stripped) headers. Shared by every Anthropic
/// passthrough route so upstream errors and SSE bodies are forwarded verbatim.
pub fn forward_passthrough(passthrough: PassthroughResponse) -> Response {
    let mut builder = Response::builder().status(passthrough.status);
    if let Some(response_headers) = builder.headers_mut() {
        for (name, value) in passthrough.headers.iter() {
            response_headers.append(name.clone(), value.clone());
        }
    }
    builder
        .body(Body::from_stream(passthrough.stream))
        .unwrap_or_else(|e| {
            tracing::error!(error = %e, "failed to build response body");
            anthropic_error_response(StatusCode::INTERNAL_SERVER_ERROR, "response build failed")
        })
}

/// Cheap probe — read the `model` field of the JSON body without a full
/// parse. Used only for metric labels, so a `None` on parse failure is fine.
pub fn sniff_model_hint(body: &Bytes) -> Option<String> {
    let value: serde_json::Value = serde_json::from_slice(body).ok()?;
    value
        .get("model")
        .and_then(|v| v.as_str())
        .map(str::to_owned)
}

/// Stamp a terminal-status metric for a completed request. `provider` is the
/// upstream family (`anthropic`/`openai`), `model_group` the bucket from
/// [`sniff_model_hint`], `status` a short tag or HTTP code, and `consumer` the
/// inbound identity from [`consumer_from_headers`].
pub fn record_terminal_metric(
    provider: &str,
    model_group: &str,
    status: &str,
    consumer: &ConsumerId,
    started: Instant,
) {
    metrics::record_request(
        provider,
        model_group,
        status,
        consumer.as_str(),
        started.elapsed().as_secs_f64(),
    );
}

/// Build a JSON error response in the Anthropic envelope shape. Used by
/// `/v1/messages` and the Codex proxy (which mirrors the Anthropic surface
/// for callers).
pub fn anthropic_error_response(status: StatusCode, body: &str) -> Response {
    let json = serde_json::json!({
        "type": "error",
        "error": {
            "type": "submux_gateway_error",
            "message": body,
        }
    });
    json_response(status, json)
}

/// Build a JSON error response in the OpenAI envelope shape, used by
/// `/v1/chat/completions`.
pub fn openai_error_response(status: StatusCode, body: &str) -> Response {
    let json = serde_json::json!({
        "error": {
            "type": "submux_gateway_error",
            "message": body,
        }
    });
    json_response(status, json)
}

fn json_response(status: StatusCode, json: serde_json::Value) -> Response {
    let mut resp = (status, json.to_string()).into_response();
    resp.headers_mut().insert(
        http::header::CONTENT_TYPE,
        http::HeaderValue::from_static("application/json"),
    );
    resp
}
