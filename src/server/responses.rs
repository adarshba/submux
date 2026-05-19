//! Shared route helpers: error envelopes, model sniffing, terminal metrics.
//!
//! Each upstream surface emits errors in its own native shape:
//! - Anthropic `/v1/messages` and the Codex `/codex/responses` proxy use the
//!   Anthropic envelope `{"type":"error","error":{...}}`.
//! - OpenAI `/v1/chat/completions` uses `{"error":{...}}` without the outer
//!   `type` discriminator.

use axum::response::{IntoResponse, Response};
use bytes::Bytes;
use http::StatusCode;
use std::time::Instant;

use crate::telemetry::metrics;

/// Cheap probe — read the `model` field of the JSON body without a full
/// parse. Used only for metric labels, so a `None` on parse failure is fine.
pub fn sniff_model_hint(body: &Bytes) -> Option<String> {
    let value: serde_json::Value = serde_json::from_slice(body).ok()?;
    value
        .get("model")
        .and_then(|v| v.as_str())
        .map(str::to_owned)
}

/// Stamp a terminal-status metric for a request that completed (success or
/// failure). `provider` is the upstream family (`anthropic` / `openai`),
/// `model_group` is the coarse bucket from [`sniff_model_hint`], and
/// `status` is a short tag like `"ok"` / `"upstream_error"` / `"no_account"`.
pub fn record_terminal_metric(provider: &str, model_group: &str, status: &str, started: Instant) {
    metrics::record_request(
        provider,
        model_group,
        status,
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
