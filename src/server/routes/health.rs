//! Open, unauthenticated probes: `/` status, `/health`, `/ready`.

use axum::{Json, Router, routing::get};
use serde_json::json;

use crate::server::app::AppState;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/", get(root))
        .route("/health", get(|| async { "ok" }))
        .route("/ready", get(|| async { "ok" }))
}

/// Service identity + endpoint listing. Returned for `GET /` (and `HEAD /`,
/// which axum routes to the same handler). Useful for liveness probes and for
/// anyone who opens the gateway URL directly.
async fn root() -> Json<serde_json::Value> {
    Json(json!({
        "service": "submux",
        "version": env!("CARGO_PKG_VERSION"),
        "status": "ok",
        "endpoints": [
            "/v1/messages",
            "/v1/models",
            "/v1/chat/completions",
            "/codex/responses",
            "/codex/v1/messages",
            "/health",
            "/ready",
            "/metrics",
        ],
    }))
}
