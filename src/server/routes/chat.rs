use axum::{
    body::Body,
    extract::State,
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::post,
    Json, Router,
};
use bytes::Bytes;
use futures::{Stream, StreamExt};
use serde_json::{json, Map, Value};
use std::collections::VecDeque;

use crate::core::{NormalizedRequest, ProviderKind};
use crate::protocols::openai::emit::{chunk_to_sse, done_marker};
use crate::protocols::openai::parse::parse_chat_body;
use crate::protocols::openai::translate_in::openai_to_normalized;
use crate::providers::anthropic::adapter::PassthroughResponse;
use crate::server::app::AppState;
use crate::server::responses::openai_error_response;
use crate::streaming::anthropic_events::AnthropicEvent;
use crate::streaming::openai_chunks::OpenAiChatChunk;
use crate::streaming::sse_parser::SseStreamParser;
use crate::streaming::translate::AnthropicToOpenAiTranslator;

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

    let translated = translate_stream(upstream.stream, Some(client_model));

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

/// Translation streaming pipeline.
///
/// Given an Anthropic SSE byte stream, return a stream of OpenAI SSE byte
/// chunks (ending with `data: [DONE]\n\n`). Implemented as a state machine
/// driven by `futures::stream::unfold` because the crate does not depend on
/// `async-stream`.
///
/// State variants:
///   * `Running` — drain upstream, parse SSE, ingest events, yield translated
///     chunks one at a time from an internal queue.
///   * `Draining` — upstream ended; emit translator.finalize() chunks, then
///     `[DONE]`.
///   * `Done` — terminal; the unfold returns `None`.
fn translate_stream(
    upstream: crate::core::ResponseStream,
    model_override: Option<String>,
) -> impl Stream<Item = Result<Bytes, std::io::Error>> + Send + 'static {
    struct State {
        upstream: crate::core::ResponseStream,
        parser: SseStreamParser,
        translator: AnthropicToOpenAiTranslator,
        /// Translated frames waiting to be yielded.
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
        translator: AnthropicToOpenAiTranslator::new(model_override),
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
                    for chunk in st.translator.finalize() {
                        match chunk_to_sse(&chunk) {
                            Ok(b) => st.pending.push_back(b),
                            Err(err) => {
                                tracing::error!(error = %err, "failed to encode finalize chunk");
                            }
                        }
                    }
                    st.pending.push_back(done_marker());
                    st.phase = Phase::Done;
                }
                Phase::Running => match st.upstream.next().await {
                    Some(Ok(bytes)) => {
                        let events = st.parser.push(&bytes);
                        ingest_events(&mut st.translator, events, &mut st.pending);
                    }
                    Some(Err(err)) => {
                        tracing::warn!(error = %err, "anthropic upstream stream error");
                        let flushed = st.parser.flush();
                        ingest_events(&mut st.translator, flushed, &mut st.pending);
                        st.phase = Phase::Draining;
                    }
                    None => {
                        let flushed = st.parser.flush();
                        ingest_events(&mut st.translator, flushed, &mut st.pending);
                        st.phase = Phase::Draining;
                    }
                },
            }
        }
    })
}

/// Helper: feed a batch of SSE events through the translator and push every
/// resulting OpenAI chunk into the pending queue.
fn ingest_events(
    translator: &mut AnthropicToOpenAiTranslator,
    events: Vec<crate::streaming::sse_parser::SseEvent>,
    pending: &mut VecDeque<Bytes>,
) {
    for ev in events {
        if ev.data.is_empty() {
            continue;
        }
        let parsed = match AnthropicEvent::from_sse_data(&ev.data) {
            Ok(p) => p,
            Err(err) => {
                tracing::warn!(
                    error = %err,
                    event = ?ev.event,
                    "failed to parse anthropic SSE event payload"
                );
                continue;
            }
        };
        for chunk in translator.ingest(parsed) {
            match chunk_to_sse(&chunk) {
                Ok(b) => pending.push_back(b),
                Err(err) => {
                    tracing::error!(error = %err, "failed to encode chat completion chunk");
                }
            }
        }
    }
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
    let response = collapse_chunks_to_completion(all_chunks, &model, &request_id);
    Json(response).into_response()
}

/// Fold a sequence of OpenAI streaming chunks into the non-streaming
/// `chat.completion` JSON shape:
///
/// * Text deltas (`choice.delta.content`) on choice index 0 are concatenated
///   into `message.content`.
/// * Tool-call deltas are accumulated by their `tool_calls[*].index`:
///   * `id` and `function.name` are captured the first time they appear.
///   * `function.arguments` fragments are concatenated into a single string.
/// * `finish_reason` is taken from the last chunk that supplied one;
///   defaults to `"stop"` if the upstream never sent one.
fn collapse_chunks_to_completion(
    chunks: Vec<OpenAiChatChunk>,
    model: &str,
    request_id: &str,
) -> Value {
    /// Accumulator for one tool call across many delta chunks.
    #[derive(Default)]
    struct AccTool {
        id: Option<String>,
        kind: Option<String>,
        name: Option<String>,
        arguments: String,
    }

    let mut content = String::new();
    let mut tool_calls: std::collections::BTreeMap<u32, AccTool> =
        std::collections::BTreeMap::new();
    let mut finish_reason: Option<String> = None;
    let mut created: i64 = 0;

    for chunk in &chunks {
        if chunk.created != 0 {
            created = chunk.created;
        }
        for choice in &chunk.choices {
            if choice.index != 0 {
                continue;
            }
            if let Some(text) = &choice.delta.content {
                content.push_str(text);
            }
            for tc in &choice.delta.tool_calls {
                let entry = tool_calls.entry(tc.index).or_default();
                if entry.id.is_none() {
                    if let Some(id) = &tc.id {
                        entry.id = Some(id.clone());
                    }
                }
                if entry.kind.is_none() {
                    entry.kind = Some(tc.kind.clone());
                }
                if entry.name.is_none() {
                    if let Some(name) = &tc.function.name {
                        entry.name = Some(name.clone());
                    }
                }
                if let Some(args) = &tc.function.arguments {
                    entry.arguments.push_str(args);
                }
            }
            if let Some(reason) = &choice.finish_reason {
                finish_reason = Some(reason.clone());
            }
        }
    }

    let mut message = Map::new();
    message.insert("role".to_string(), Value::String("assistant".to_string()));
    message.insert("content".to_string(), Value::String(content));
    if !tool_calls.is_empty() {
        let arr: Vec<Value> = tool_calls
            .into_values()
            .map(|tc| {
                let mut obj = Map::new();
                obj.insert("id".to_string(), Value::String(tc.id.unwrap_or_default()));
                obj.insert(
                    "type".to_string(),
                    Value::String(tc.kind.unwrap_or_else(|| "function".to_string())),
                );
                let mut func = Map::new();
                func.insert(
                    "name".to_string(),
                    Value::String(tc.name.unwrap_or_default()),
                );
                func.insert("arguments".to_string(), Value::String(tc.arguments));
                obj.insert("function".to_string(), Value::Object(func));
                Value::Object(obj)
            })
            .collect();
        message.insert("tool_calls".to_string(), Value::Array(arr));
    }

    let finish_reason = finish_reason.unwrap_or_else(|| "stop".to_string());

    json!({
        "id": request_id,
        "object": "chat.completion",
        "created": created,
        "model": model,
        "choices": [{
            "index": 0,
            "message": Value::Object(message),
            "finish_reason": finish_reason,
        }],
        "usage": Value::Null,
    })
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
        let v = collapse_chunks_to_completion(chunks, "gpt-4o", "chatcmpl-abc");
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
        let v = collapse_chunks_to_completion(chunks, "gpt-4o", "chatcmpl-xyz");
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
        let v = collapse_chunks_to_completion(chunks, "gpt-4o", "chatcmpl-d");
        assert_eq!(v["choices"][0]["finish_reason"], "stop");
        assert_eq!(v["choices"][0]["message"]["content"], "hi");
    }
}
