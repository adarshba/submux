use axum::{
    body::Body,
    extract::State,
    http::{HeaderMap, StatusCode},
    response::Response,
    routing::post,
    Router,
};
use bytes::Bytes;
use chrono::Utc;
use std::time::Instant;

use crate::constants::upstream_paths::CODEX_REFRESH_ENDPOINT;
use crate::core::{Credentials, ProviderKind};
use crate::providers::openai::{openai_adapter, CodexPassthroughResponse};
use crate::server::app::AppState;
use crate::server::responses::{
    anthropic_error_response, record_terminal_metric, sniff_model_hint,
};
use crate::telemetry::events::RequestEvent;
use crate::telemetry::tracer;

const PROVIDER: &str = "openai";

pub fn router() -> Router<AppState> {
    Router::new().route("/codex/responses", post(handle_codex_responses))
}

async fn handle_codex_responses(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let started = Instant::now();
    let request_id = tracer::new_request_id();
    let model_hint = sniff_model_hint(&body);
    let model_group = model_hint.clone().unwrap_or_else(|| "unknown".to_owned());

    state.events.publish(RequestEvent::Accepted {
        request_id: request_id.clone(),
        protocol: PROVIDER.to_owned(),
        model_hint: model_hint.clone(),
        timestamp: Utc::now(),
    });

    let Some(adapter) = openai_adapter() else {
        record_terminal_metric(PROVIDER, &model_group, "no_adapter", started);
        return anthropic_error_response(
            StatusCode::SERVICE_UNAVAILABLE,
            "openai adapter not installed",
        );
    };

    let accounts = state.pool.by_provider(ProviderKind::OpenAiSubscription);
    let Some(account) = accounts.into_iter().next() else {
        record_terminal_metric(PROVIDER, &model_group, "no_account", started);
        return anthropic_error_response(
            StatusCode::SERVICE_UNAVAILABLE,
            "no OpenAI subscription account registered — set SUBMUX_OPENAI_ACCESS_TOKEN at startup",
        );
    };

    let Some((access_token, cookies, device_id)) = account.openai_credentials() else {
        record_terminal_metric(PROVIDER, &model_group, "wrong_credential_kind", started);
        return anthropic_error_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            "account is not an OpenAI subscription account",
        );
    };

    let refresh_endpoint = CODEX_REFRESH_ENDPOINT
        .parse()
        .expect("static codex refresh endpoint is a valid URL");
    let creds = Credentials::OpenAiChatGptSession {
        access_token,
        cookies,
        device_id,
        expires_at: Utc::now() + chrono::Duration::days(30),
        refresh_endpoint,
    };

    state.events.publish(RequestEvent::AccountPicked {
        request_id: request_id.clone(),
        account_id: account.id,
        provider: PROVIDER.to_owned(),
        timestamp: Utc::now(),
    });
    state.events.publish(RequestEvent::UpstreamStart {
        request_id: request_id.clone(),
        account_id: account.id,
        timestamp: Utc::now(),
    });

    let passthrough = match adapter.codex.passthrough(&headers, body, &creds).await {
        Ok(p) => p,
        Err(err) => {
            state.events.publish(RequestEvent::UpstreamError {
                request_id,
                account_id: Some(account.id),
                error: err.to_string(),
                timestamp: Utc::now(),
            });
            tracing::warn!(error = %err, "codex passthrough failed");
            record_terminal_metric(PROVIDER, &model_group, "upstream_error", started);
            return anthropic_error_response(
                StatusCode::BAD_GATEWAY,
                &format!("upstream error: {err}"),
            );
        }
    };

    let status = passthrough.status;
    state.events.publish(RequestEvent::UpstreamComplete {
        request_id,
        account_id: account.id,
        status: status.as_u16(),
        latency_ms: started.elapsed().as_millis() as u64,
        input_tokens: None,
        output_tokens: None,
        timestamp: Utc::now(),
    });
    record_terminal_metric(
        PROVIDER,
        &model_group,
        &status.as_u16().to_string(),
        started,
    );

    forward_response(passthrough)
}

fn forward_response(passthrough: CodexPassthroughResponse) -> Response {
    let mut builder = Response::builder().status(passthrough.status);
    if let Some(response_headers) = builder.headers_mut() {
        for (name, value) in passthrough.headers.iter() {
            response_headers.append(name.clone(), value.clone());
        }
    }
    builder
        .body(Body::from_stream(passthrough.stream))
        .unwrap_or_else(|e| {
            tracing::error!(error = %e, "failed to build codex response body");
            anthropic_error_response(StatusCode::INTERNAL_SERVER_ERROR, "response build failed")
        })
}
