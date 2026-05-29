//! Axum app construction: state aggregation, route grouping, layer order.
//!
//! Routes are split into two groups so the inbound auth gate applies only
//! where it should:
//!  - **Open**: `/health`, `/ready`, `/metrics` — health probes and
//!    Prometheus scrapes work even when an api key is configured.
//!  - **Gated**: `/v1/messages`, `/v1/chat/completions`, `/codex/*` — the
//!    auth middleware is applied iff [`AppState::api_key`] is `Some`.

use axum::Router as AxumRouter;
use http::StatusCode;
use std::sync::Arc;

use crate::accounts::{AccountPool, CooldownCache, RefreshManager};
use crate::constants::limits::{REQUEST_BODY_LIMIT_BYTES, REQUEST_TIMEOUT};
use crate::core::ApiKey;
use crate::providers::anthropic::AnthropicProxy;
use crate::server::middleware::auth::require_api_key;

/// Singletons shared by every request handler.
#[derive(Clone)]
pub struct AppState {
    pub pool: Arc<AccountPool>,
    pub anthropic: Arc<AnthropicProxy>,
    pub refresh: Arc<RefreshManager>,
    pub cooldown: Arc<CooldownCache>,
    pub http: Arc<reqwest::Client>,
    pub api_key: Option<Arc<ApiKey>>,
}

/// Build the full axum router with all middleware applied.
pub fn build_app(state: AppState) -> AxumRouter {
    let open = AxumRouter::new()
        .merge(crate::server::routes::health::router())
        .merge(crate::server::routes::metrics::router());

    let mut gated = AxumRouter::new()
        .merge(crate::server::routes::messages::router())
        .merge(crate::server::routes::chat::router())
        .merge(crate::server::routes::codex::router())
        .merge(crate::server::routes::codex_messages::router());

    if let Some(key) = state.api_key.clone() {
        gated = gated.layer(axum::middleware::from_fn_with_state(key, require_api_key));
    }

    open.merge(gated)
        .layer(axum::middleware::from_fn(
            crate::server::middleware::request_id::request_id_middleware,
        ))
        .layer(tower_http::trace::TraceLayer::new_for_http())
        .layer(tower_http::limit::RequestBodyLimitLayer::new(
            REQUEST_BODY_LIMIT_BYTES,
        ))
        .layer(tower_http::timeout::TimeoutLayer::with_status_code(
            StatusCode::GATEWAY_TIMEOUT,
            REQUEST_TIMEOUT,
        ))
        .layer(crate::server::middleware::panic_catch::layer())
        .with_state(state)
}
