use axum::{
    body::Body,
    extract::State,
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::post,
    Json, Router,
};
use bytes::Bytes;
use futures::StreamExt;
use serde_json::{json, Value};

use crate::core::{NormalizedRequest, ProviderKind};
use crate::protocols::openai::collapse::chunks_to_completion;
use crate::protocols::openai::emit::done_marker;
use crate::protocols::openai::parse::parse_chat_body;
use crate::protocols::openai::translate_in::openai_to_normalized;
use crate::providers::anthropic::PassthroughResponse;
use crate::server::app::AppState;
use crate::server::responses::openai_error_response;
use crate::streaming::anthropic_events::AnthropicEvent;
use crate::streaming::openai_chunks::OpenAiChatChunk;
use crate::streaming::sse_parser::SseStreamParser;
use crate::streaming::translate::AnthropicToOpenAiTranslator;
use crate::streaming::{relay, AnthropicToOpenAiRelay};

pub fn router() -> Router<AppState> {
    Router::new().route("/v1/chat/completions", post(handle_chat))
}

async fn handle_chat(State(state): State<AppState>, headers: HeaderMap, body: Bytes) -> Response {
    let parsed = match parse_chat_body(&body) {
        Ok(p) => p,
        Err(err) => {
            tracing::warn!(error = %err, "openai chat parse failure");
            return openai_error_response(
                StatusCode::BAD_REQUEST,
                &format!("invalid OpenAI Chat Completions body: {err}"),
            );
        }
    };

    let want_stream = parsed.stream;
    let client_model = parsed.model.clone();

    let normalized = openai_to_normalized(parsed);
    let anthropic_body = match build_anthropic_body(&normalized) {
        Ok(bytes) => bytes,
        Err(err) => {
            tracing::error!(error = %err, "failed to serialize anthropic body");
            return openai_error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "failed to build upstream request body",
            );
        }
    };

    let accounts = state.pool.by_provider(ProviderKind::AnthropicSubscription);
    let Some(account) = accounts.into_iter().next() else {
        return openai_error_response(
            StatusCode::SERVICE_UNAVAILABLE,
            "no Anthropic OAuth account registered — set SUBMUX_ANTHROPIC_OAUTH_TOKEN at startup",
        );
    };
    let Some(token) = account.anthropic_oauth_token() else {
        return openai_error_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            "account is not an Anthropic OAuth account",
        );
    };

    let forward_headers = filter_client_headers(&headers);

    let upstream = match state
        .anthropic
        .passthrough(&forward_headers, anthropic_body, &token)
        .await
    {
        Ok(p) => p,
        Err(err) => {
            tracing::warn!(error = %err, "anthropic passthrough failed");
            return openai_error_response(
                StatusCode::BAD_GATEWAY,
                &format!("upstream error: {err}"),
            );
        }
    };

    if !upstream.status.is_success() {
        tracing::warn!(
            status = %upstream.status,
            "anthropic upstream returned non-success; forwarding body as-is"
        );
        return forward_passthrough_as_is(upstream);
    }

    if !want_stream {
        return buffer_to_completion(upstream, client_model).await;
    }

    let translated = relay::drive(
        upstream.stream,
        AnthropicToOpenAiRelay::new(Some(client_model)),
        Some(done_marker()),
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
            tracing::error!(error = %e, "failed to build chat response body");
            openai_error_response(StatusCode::INTERNAL_SERVER_ERROR, "response build failed")
        })
}

/// Build the Anthropic Messages JSON request body from the canonical request.
///
/// `ContentBlock`, `Message` and `Tool` already serialize to the Anthropic
/// wire shape, so we just need to project `NormalizedRequest` onto the small
/// set of fields Anthropic accepts and force `stream: true`.
fn build_anthropic_body(req: &NormalizedRequest) -> Result<Bytes, serde_json::Error> {
    let model = req
        .model_hint
        .clone()
        .unwrap_or_else(|| "claude-3-5-sonnet-latest".to_owned());
    let max_tokens = req.max_tokens.unwrap_or(4096);

    let mut obj = serde_json::Map::new();
    obj.insert("model".to_string(), Value::String(model));
    obj.insert("max_tokens".to_string(), json!(max_tokens));
    obj.insert("stream".to_string(), Value::Bool(true));
    obj.insert("messages".to_string(), serde_json::to_value(&req.messages)?);
    if !req.system.is_empty() {
        obj.insert("system".to_string(), serde_json::to_value(&req.system)?);
    }
    if !req.tools.is_empty() {
        obj.insert("tools".to_string(), serde_json::to_value(&req.tools)?);
    }
    if let Some(t) = req.temperature {
        obj.insert("temperature".to_string(), json!(t));
    }
    if let Some(p) = req.top_p {
        obj.insert("top_p".to_string(), json!(p));
    }
    if let Some(k) = req.top_k {
        obj.insert("top_k".to_string(), json!(k));
    }
    if let Value::Object(extras) = &req.passthrough {
        for (k, v) in extras {
            obj.entry(k.clone()).or_insert_with(|| v.clone());
        }
    }

    let bytes = serde_json::to_vec(&Value::Object(obj))?;
    Ok(Bytes::from(bytes))
}

/// Strip headers that no longer match the rewritten request body.
///
/// The adapter already drops auth, fingerprint, and hop-by-hop headers; we
/// just need to make sure we don't lie about the body shape.
fn filter_client_headers(headers: &HeaderMap) -> HeaderMap {
    let mut out = HeaderMap::with_capacity(headers.len());
    for (name, value) in headers.iter() {
        let n = name.as_str();
        if n.eq_ignore_ascii_case("content-type") || n.eq_ignore_ascii_case("content-length") {
            continue;
        }
        out.append(name.clone(), value.clone());
    }
    out
}

/// Forward a non-2xx upstream response verbatim, preserving status & headers.
fn forward_passthrough_as_is(passthrough: PassthroughResponse) -> Response {
    let mut builder = Response::builder().status(passthrough.status);
    if let Some(response_headers) = builder.headers_mut() {
        for (name, value) in passthrough.headers.iter() {
            response_headers.append(name.clone(), value.clone());
        }
    }
    builder
        .body(Body::from_stream(passthrough.stream))
        .unwrap_or_else(|e| {
            tracing::error!(error = %e, "failed to build passthrough error response");
            openai_error_response(StatusCode::INTERNAL_SERVER_ERROR, "response build failed")
        })
}

/// Drain a successful Anthropic SSE upstream into memory, translate each
/// event, and collapse the resulting OpenAI streaming chunks into a single
/// non-streaming ChatCompletion JSON response.
async fn buffer_to_completion(upstream: PassthroughResponse, model: String) -> Response {
    let mut parser = SseStreamParser::new();
    let mut translator = AnthropicToOpenAiTranslator::new(Some(model.clone()));
    let mut all_chunks: Vec<OpenAiChatChunk> = Vec::new();

    let mut stream = upstream.stream;
    while let Some(item) = stream.next().await {
        match item {
            Ok(bytes) => {
                for ev in parser.push(&bytes) {
                    if ev.data.is_empty() {
                        continue;
                    }
                    match AnthropicEvent::from_sse_data(&ev.data) {
                        Ok(ae) => all_chunks.extend(translator.ingest(ae)),
                        Err(err) => {
                            tracing::warn!(
                                error = %err,
                                event = ?ev.event,
                                "failed to parse anthropic SSE event during buffer"
                            );
                        }
                    }
                }
            }
            Err(err) => {
                tracing::warn!(error = %err, "upstream error during non-streaming buffer");
                return openai_error_response(
                    StatusCode::BAD_GATEWAY,
                    &format!("upstream error: {err}"),
                );
            }
        }
    }
    for ev in parser.flush() {
        if ev.data.is_empty() {
            continue;
        }
        if let Ok(ae) = AnthropicEvent::from_sse_data(&ev.data) {
            all_chunks.extend(translator.ingest(ae));
        }
    }
    all_chunks.extend(translator.finalize());

    let request_id = translator.request_id().to_string();
    let response = chunks_to_completion(&all_chunks, &model, &request_id);
    Json(response).into_response()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::streaming::openai_chunks::{
        chat_completion_chunk, OpenAiChatChunk, OpenAiChoice, OpenAiDelta, OpenAiFunctionDelta,
        OpenAiToolCallDelta,
    };

    fn base(created: i64) -> OpenAiChatChunk {
        OpenAiChatChunk {
            id: "chatcmpl-test".to_string(),
            object: chat_completion_chunk(),
            created,
            model: "gpt-4o".to_string(),
            choices: vec![],
        }
    }

    fn content_chunk(text: &str) -> OpenAiChatChunk {
        let mut c = base(1000);
        c.choices.push(OpenAiChoice {
            index: 0,
            delta: OpenAiDelta {
                content: Some(text.to_string()),
                ..Default::default()
            },
            finish_reason: None,
        });
        c
    }

    fn finish_chunk(reason: &str) -> OpenAiChatChunk {
        let mut c = base(1000);
        c.choices.push(OpenAiChoice {
            index: 0,
            delta: OpenAiDelta::default(),
            finish_reason: Some(reason.to_string()),
        });
        c
    }

    #[test]
    fn collapse_concatenates_content_and_picks_finish_reason() {
        let chunks = vec![
            content_chunk("Hello"),
            content_chunk(", "),
            content_chunk("world!"),
            finish_chunk("stop"),
        ];
        let v = chunks_to_completion(&chunks, "gpt-4o", "chatcmpl-abc");
        assert_eq!(v["object"], "chat.completion");
        assert_eq!(v["id"], "chatcmpl-abc");
        assert_eq!(v["model"], "gpt-4o");
        assert_eq!(v["created"], 1000);
        let choice = &v["choices"][0];
        assert_eq!(choice["index"], 0);
        assert_eq!(choice["finish_reason"], "stop");
        assert_eq!(choice["message"]["role"], "assistant");
        assert_eq!(choice["message"]["content"], "Hello, world!");
        assert!(choice["message"].get("tool_calls").is_none());
        assert!(v["usage"].is_null());
    }

    #[test]
    fn collapse_accumulates_tool_call_argument_fragments() {
        let mut start = base(2000);
        start.choices.push(OpenAiChoice {
            index: 0,
            delta: OpenAiDelta {
                tool_calls: vec![OpenAiToolCallDelta {
                    index: 0,
                    id: Some("call_1".to_string()),
                    kind: "function".to_string(),
                    function: OpenAiFunctionDelta {
                        name: Some("get_weather".to_string()),
                        arguments: None,
                    },
                }],
                ..Default::default()
            },
            finish_reason: None,
        });
        let mut frag1 = base(2000);
        frag1.choices.push(OpenAiChoice {
            index: 0,
            delta: OpenAiDelta {
                tool_calls: vec![OpenAiToolCallDelta {
                    index: 0,
                    id: None,
                    kind: "function".to_string(),
                    function: OpenAiFunctionDelta {
                        name: None,
                        arguments: Some("{\"city\":".to_string()),
                    },
                }],
                ..Default::default()
            },
            finish_reason: None,
        });
        let mut frag2 = base(2000);
        frag2.choices.push(OpenAiChoice {
            index: 0,
            delta: OpenAiDelta {
                tool_calls: vec![OpenAiToolCallDelta {
                    index: 0,
                    id: None,
                    kind: "function".to_string(),
                    function: OpenAiFunctionDelta {
                        name: None,
                        arguments: Some("\"Paris\"}".to_string()),
                    },
                }],
                ..Default::default()
            },
            finish_reason: None,
        });
        let chunks = vec![start, frag1, frag2, finish_chunk("tool_calls")];
        let v = chunks_to_completion(&chunks, "gpt-4o", "chatcmpl-xyz");
        let choice = &v["choices"][0];
        assert_eq!(choice["finish_reason"], "tool_calls");
        assert_eq!(choice["message"]["content"], "");
        let tool_calls = choice["message"]["tool_calls"].as_array().unwrap();
        assert_eq!(tool_calls.len(), 1);
        assert_eq!(tool_calls[0]["id"], "call_1");
        assert_eq!(tool_calls[0]["type"], "function");
        assert_eq!(tool_calls[0]["function"]["name"], "get_weather");
        assert_eq!(
            tool_calls[0]["function"]["arguments"],
            "{\"city\":\"Paris\"}"
        );
    }

    #[test]
    fn collapse_defaults_finish_reason_to_stop_when_missing() {
        let chunks = vec![content_chunk("hi")];
        let v = chunks_to_completion(&chunks, "gpt-4o", "chatcmpl-d");
        assert_eq!(v["choices"][0]["finish_reason"], "stop");
        assert_eq!(v["choices"][0]["message"]["content"], "hi");
    }
}
