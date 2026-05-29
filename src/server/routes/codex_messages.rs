//! `POST /codex/v1/messages` — speak Anthropic Messages to a Codex backend.
//!
//! Translates the request body Anthropic → Responses, drives the Codex
//! passthrough, and translates the SSE stream back Responses → Anthropic so
//! stock Claude Code clients (`ANTHROPIC_BASE_URL=http://submux/codex`) can
//! drive a ChatGPT Plus account. Non-2xx upstream replies are wrapped in an
//! Anthropic error envelope — the client only speaks Anthropic.

use axum::{
    Router,
    body::Body,
    extract::State,
    http::{HeaderMap, StatusCode},
    response::Response,
    routing::post,
};
use bytes::Bytes;
use chrono::Utc;
use std::time::Instant;

use crate::constants::upstream_paths::CODEX_REFRESH_ENDPOINT;
use crate::core::{Credentials, ProviderKind};
use crate::protocols::anthropic::translate_to_responses::anthropic_to_responses_body;
use crate::providers::codex::current as current_codex;
use crate::server::app::AppState;
use crate::server::responses::{
    anthropic_error_response, consumer_from_headers, record_terminal_metric, sniff_model_hint,
};
use crate::streaming::relay;
use crate::streaming::translate_responses_to_anthropic::ResponsesToAnthropicTranslator;
use crate::telemetry::tracer;

const PROVIDER: &str = "openai";

pub fn router() -> Router<AppState> {
    Router::new().route("/codex/v1/messages", post(handle_codex_messages))
}

async fn handle_codex_messages(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let started = Instant::now();
    let request_id = tracer::new_request_id();
    let consumer = consumer_from_headers(&headers);
    let model_group = sniff_model_hint(&body).unwrap_or_else(|| "unknown".to_owned());

    let translated_body = match anthropic_to_responses_body(&body) {
        Ok(b) => b,
        Err(err) => {
            tracing::warn!(error = %err, "anthropic→responses translate failed");
            record_terminal_metric(PROVIDER, &model_group, "bad_request", &consumer, started);
            return anthropic_error_response(
                StatusCode::BAD_REQUEST,
                &format!("invalid Anthropic Messages body: {err}"),
            );
        }
    };

    let Some(proxy) = current_codex() else {
        record_terminal_metric(PROVIDER, &model_group, "no_proxy", &consumer, started);
        return anthropic_error_response(
            StatusCode::SERVICE_UNAVAILABLE,
            "codex proxy not installed",
        );
    };

    let accounts = state.pool.by_provider(ProviderKind::OpenAiSubscription);
    let Some(account) = accounts.into_iter().next() else {
        record_terminal_metric(PROVIDER, &model_group, "no_account", &consumer, started);
        return anthropic_error_response(
            StatusCode::SERVICE_UNAVAILABLE,
            "no OpenAI subscription account registered — set SUBMUX_OPENAI_ACCESS_TOKEN at startup",
        );
    };

    let Some((access_token, cookies, device_id)) = account.openai_credentials() else {
        record_terminal_metric(
            PROVIDER,
            &model_group,
            "wrong_credential_kind",
            &consumer,
            started,
        );
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

    let passthrough = match proxy
        .codex
        .passthrough(&headers, translated_body, &creds)
        .await
    {
        Ok(p) => p,
        Err(err) => {
            tracing::warn!(request_id = %request_id, error = %err, "codex passthrough failed");
            record_terminal_metric(PROVIDER, &model_group, "upstream_error", &consumer, started);
            return anthropic_error_response(
                StatusCode::BAD_GATEWAY,
                &format!("upstream error: {err}"),
            );
        }
    };

    let status = passthrough.status;
    record_terminal_metric(
        PROVIDER,
        &model_group,
        &status.as_u16().to_string(),
        &consumer,
        started,
    );

    if !status.is_success() {
        tracing::warn!(
            status = %status,
            "codex upstream returned non-success for /codex/v1/messages"
        );
        return anthropic_error_response(
            status,
            &format!("codex upstream returned status {}", status.as_u16()),
        );
    }

    let translated = relay::drive(
        passthrough.stream,
        ResponsesToAnthropicTranslator::new(),
        None,
    );

    let mut builder = Response::builder().status(StatusCode::OK);
    if let Some(h) = builder.headers_mut() {
        h.insert(
            http::header::CONTENT_TYPE,
            http::HeaderValue::from_static("text/event-stream"),
        );
        h.insert(
            http::header::CACHE_CONTROL,
            http::HeaderValue::from_static("no-cache"),
        );
        h.insert(
            http::header::CONNECTION,
            http::HeaderValue::from_static("keep-alive"),
        );
    }
    builder
        .body(Body::from_stream(translated))
        .unwrap_or_else(|e| {
            tracing::error!(error = %e, "failed to build codex messages response body");
            anthropic_error_response(StatusCode::INTERNAL_SERVER_ERROR, "response build failed")
        })
}
