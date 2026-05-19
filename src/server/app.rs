use axum::Router as AxumRouter;
use http::StatusCode;
use std::sync::Arc;

use crate::accounts::{AccountPool, RefreshManager};
use crate::constants::limits::{REQUEST_BODY_LIMIT_BYTES, REQUEST_TIMEOUT};
use crate::providers::anthropic::adapter::AnthropicOAuthAdapter;
use crate::router::cooldown::CooldownCache;
use crate::router::Router;
use crate::telemetry::events::EventBus;

#[derive(Clone)]
pub struct AppState {
    pub pool: Arc<AccountPool>,
    pub router: Arc<Router>,
    pub anthropic: Arc<AnthropicOAuthAdapter>,
    pub refresh: Arc<RefreshManager>,
    pub cooldown: Arc<CooldownCache>,
    pub events: Arc<EventBus>,
    pub http: Arc<reqwest::Client>,
}

pub fn build_app(state: AppState) -> AxumRouter {
    AxumRouter::new()
        .merge(crate::server::routes::health::router())
        .merge(crate::server::routes::messages::router())
        .merge(crate::server::routes::chat::router())
        .merge(crate::server::routes::codex::router())
        .merge(crate::server::routes::codex_messages::router())
        .merge(crate::server::routes::admin::router())
        .merge(crate::server::routes::metrics::router())
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
