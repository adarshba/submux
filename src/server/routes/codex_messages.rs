//! `POST /codex/v1/messages` — speak Anthropic Messages to a Codex backend.
//!
//! Translates the request body Anthropic → Responses, drives the Codex
//! passthrough, and translates the SSE stream back Responses → Anthropic so
//! stock Claude Code clients (`ANTHROPIC_BASE_URL=http://submux/codex`) can
//! drive a ChatGPT Plus account. Non-2xx upstream replies are wrapped in an
//! Anthropic error envelope — the client only speaks Anthropic.

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
use futures::{Stream, StreamExt};
use std::collections::VecDeque;
use std::time::Instant;

use crate::constants::upstream_paths::CODEX_REFRESH_ENDPOINT;
use crate::core::{Credentials, ProviderKind};
use crate::protocols::anthropic::translate_to_responses::anthropic_to_responses_body;
use crate::providers::openai::openai_adapter;
use crate::server::app::AppState;
use crate::server::responses::{
    anthropic_error_response, record_terminal_metric, sniff_model_hint,
};
use crate::streaming::sse_parser::SseStreamParser;
use crate::streaming::translate_responses_to_anthropic::ResponsesToAnthropicTranslator;
use crate::telemetry::events::RequestEvent;
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
    let model_hint = sniff_model_hint(&body);
    let model_group = model_hint.clone().unwrap_or_else(|| "unknown".to_owned());

    state.events.publish(RequestEvent::Accepted {
        request_id: request_id.clone(),
        protocol: "anthropic".to_owned(),
        model_hint: model_hint.clone(),
        timestamp: Utc::now(),
    });

    let translated_body = match anthropic_to_responses_body(&body) {
        Ok(b) => b,
        Err(err) => {
            tracing::warn!(error = %err, "anthropic→responses translate failed");
            record_terminal_metric(PROVIDER, &model_group, "bad_request", started);
            return anthropic_error_response(
                StatusCode::BAD_REQUEST,
                &format!("invalid Anthropic Messages body: {err}"),
            );
        }
    };

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

    let passthrough = match adapter
        .codex
        .passthrough(&headers, translated_body, &creds)
        .await
    {
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

    let translated = translate_stream(passthrough.stream);

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

/// Drive the Responses SSE → Anthropic SSE state machine over an upstream
/// byte stream.
fn translate_stream(
    upstream: crate::core::ResponseStream,
) -> impl Stream<Item = Result<Bytes, std::io::Error>> + Send + 'static {
    struct State {
        upstream: crate::core::ResponseStream,
        parser: SseStreamParser,
        translator: ResponsesToAnthropicTranslator,
        pending: VecDeque<Bytes>,
        phase: Phase,
    }
    enum Phase {
        Running,
        Draining,
        Done,
    }

    let state = State {
        upstream,
        parser: SseStreamParser::new(),
        translator: ResponsesToAnthropicTranslator::new(),
        pending: VecDeque::new(),
        phase: Phase::Running,
    };

    futures::stream::unfold(state, |mut st| async move {
        loop {
            if let Some(frame) = st.pending.pop_front() {
                return Some((Ok(frame), st));
            }
            match st.phase {
                Phase::Done => return None,
                Phase::Draining => {
                    for b in st.translator.finalize() {
                        st.pending.push_back(b);
                    }
                    st.phase = Phase::Done;
                }
                Phase::Running => match st.upstream.next().await {
                    Some(Ok(bytes)) => {
                        let events = st.parser.push(&bytes);
                        for ev in events {
                            for b in st.translator.ingest(ev) {
                                st.pending.push_back(b);
                            }
                        }
                    }
                    Some(Err(err)) => {
                        tracing::warn!(error = %err, "codex upstream stream error");
                        for ev in st.parser.flush() {
                            for b in st.translator.ingest(ev) {
                                st.pending.push_back(b);
                            }
                        }
                        st.phase = Phase::Draining;
                    }
                    None => {
                        for ev in st.parser.flush() {
                            for b in st.translator.ingest(ev) {
                                st.pending.push_back(b);
                            }
                        }
                        st.phase = Phase::Draining;
                    }
                },
            }
        }
    })
}
