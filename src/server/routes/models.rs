//! `GET /v1/models` + `GET /v1/models/:model` — Anthropic model-metadata
//! passthrough.
//!
//! Claude Code probes these endpoints at startup to validate the selected
//! model. Without them the gateway 404s the probe and the client reports the
//! model as unavailable, even though `/v1/messages` works. We proxy upstream
//! through the same OAuth cloak as the messages path so the list always
//! reflects what the subscription can actually reach.

use axum::{
    Router,
    extract::{Path, State},
    http::{HeaderMap, StatusCode},
    response::Response,
    routing::get,
};
use std::time::Instant;

use crate::constants::upstream_paths::ANTHROPIC_MODELS_PATH;
use crate::core::ProviderKind;
use crate::server::app::AppState;
use crate::server::oauth_refresh::refresh_anthropic_account;
use crate::server::responses::{
    anthropic_error_response, consumer_from_headers, forward_passthrough, record_terminal_metric,
};
use crate::telemetry::tracer;

const PROVIDER: &str = "anthropic";

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/v1/models", get(handle_list))
        .route("/v1/models/:model", get(handle_get))
}

async fn handle_list(State(state): State<AppState>, headers: HeaderMap) -> Response {
    proxy_models(&state, &headers, ANTHROPIC_MODELS_PATH.to_owned(), "models").await
}

async fn handle_get(
    State(state): State<AppState>,
    Path(model): Path<String>,
    headers: HeaderMap,
) -> Response {
    let path = format!("{ANTHROPIC_MODELS_PATH}/{model}");
    proxy_models(&state, &headers, path, &model).await
}

/// Resolve the Anthropic OAuth account, GET `upstream_path` with the cloak
/// headers, and forward the response — refreshing the token once on a 401.
async fn proxy_models(
    state: &AppState,
    headers: &HeaderMap,
    upstream_path: String,
    metric_label: &str,
) -> Response {
    let started = Instant::now();
    let request_id = tracer::new_request_id();
    let consumer = consumer_from_headers(headers);

    let accounts = state.pool.by_provider(ProviderKind::AnthropicSubscription);
    let Some(account) = accounts.into_iter().next() else {
        record_terminal_metric(PROVIDER, metric_label, "no_account", &consumer, started);
        return anthropic_error_response(
            StatusCode::SERVICE_UNAVAILABLE,
            "no Anthropic OAuth account registered — set SUBMUX_ANTHROPIC_OAUTH_TOKEN at startup",
        );
    };
    let Some(mut token) = account.anthropic_oauth_token() else {
        record_terminal_metric(
            PROVIDER,
            metric_label,
            "wrong_credential_kind",
            &consumer,
            started,
        );
        return anthropic_error_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            "account is not an Anthropic OAuth account",
        );
    };

    let mut passthrough = match state
        .anthropic
        .passthrough_get(headers, &upstream_path, &token)
        .await
    {
        Ok(p) => p,
        Err(err) => {
            tracing::warn!(request_id = %request_id, error = %err, "anthropic models passthrough failed");
            record_terminal_metric(PROVIDER, metric_label, "upstream_error", &consumer, started);
            return anthropic_error_response(
                StatusCode::BAD_GATEWAY,
                &format!("upstream error: {err}"),
            );
        }
    };

    if passthrough.status == StatusCode::UNAUTHORIZED && account.anthropic_refresh_token().is_some()
    {
        match refresh_anthropic_account(state, &account).await {
            Ok(new_token) => {
                token = new_token;
                match state
                    .anthropic
                    .passthrough_get(headers, &upstream_path, &token)
                    .await
                {
                    Ok(retry) => passthrough = retry,
                    Err(err) => {
                        record_terminal_metric(
                            PROVIDER,
                            metric_label,
                            "upstream_error",
                            &consumer,
                            started,
                        );
                        return anthropic_error_response(
                            StatusCode::BAD_GATEWAY,
                            &format!("retry after refresh failed: {err}"),
                        );
                    }
                }
            }
            Err(err) => {
                tracing::warn!(account = %account.id, error = %err, "refresh-on-401 failed");
            }
        }
    }

    let status = passthrough.status;
    record_terminal_metric(
        PROVIDER,
        metric_label,
        &status.as_u16().to_string(),
        &consumer,
        started,
    );

    forward_passthrough(passthrough)
}
