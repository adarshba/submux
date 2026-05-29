//! Stream translation: OpenAI Responses SSE → Anthropic Messages SSE.
//!
//! Event map:
//! - `response.created` → `message_start`.
//! - `response.output_item.added`:
//!     - `message` items defer `content_block_start` until the first text
//!       delta — Responses sometimes opens empty message items.
//!     - `function_call` items emit `content_block_start` (tool_use) eagerly.
//!     - other item types (`reasoning`, …) are tracked and dropped.
//! - `response.output_text.delta` → lazy text `content_block_start`, then
//!   `content_block_delta` with `text_delta`.
//! - `response.function_call_arguments.delta` → `content_block_delta` with
//!   `input_json_delta`.
//! - `*.done` family → `content_block_stop` for that item.
//! - `response.completed` → `message_delta` (stop_reason + usage) +
//!   `message_stop`.
//! - `response.failed` / `response.error` → Anthropic `error` (terminal).
//!
//! Not an SSE codec; callers push parsed [`SseEvent`]s and forward the
//! returned [`Bytes`].

use std::collections::HashMap;

use serde_json::{json, Value};

use crate::streaming::sse_emitter::encode_event;
use crate::streaming::sse_parser::SseEvent;
use bytes::Bytes;

/// Fallback model id when `response.created` omits one.
const DEFAULT_MODEL: &str = "gpt-5-codex";

#[derive(Debug)]
pub struct ResponsesToAnthropicTranslator {
    message_id: String,
    model: String,
    started: bool,
    finished: bool,
    /// Drives the `stop_reason` (`tool_use` vs `end_turn`) sent in
    /// `message_delta`.
    saw_tool_use: bool,
    blocks: HashMap<u32, BlockState>,
    /// Indices are minted eagerly per `output_item.added` so they stay in
    /// upstream order even when `content_block_start` is deferred.
    next_block_index: u32,
}

#[derive(Debug)]
struct BlockState {
    index: u32,
    kind: BlockKind,
    started: bool,
    stopped: bool,
}

#[derive(Debug, Clone, Copy)]
enum BlockKind {
    Text,
    ToolUse,
    Ignored,
}

fn output_index_of(payload: &Value) -> Option<u32> {
    payload
        .get("output_index")
        .and_then(Value::as_u64)
        .and_then(|i| u32::try_from(i).ok())
}

impl ResponsesToAnthropicTranslator {
    pub fn new() -> Self {
        Self {
            message_id: String::new(),
            model: DEFAULT_MODEL.to_owned(),
            started: false,
            finished: false,
            saw_tool_use: false,
            blocks: HashMap::new(),
            next_block_index: 0,
        }
    }

    /// Ingest one parsed SSE frame. The JSON `type` is preferred over the
    /// SSE `event:` line because Codex variants don't always populate the
    /// latter consistently.
    pub fn ingest(&mut self, frame: SseEvent) -> Vec<Bytes> {
        if frame.data.is_empty() {
            return Vec::new();
        }
        let payload: Value = match serde_json::from_str(&frame.data) {
            Ok(v) => v,
            Err(err) => {
                tracing::warn!(
                    error = %err,
                    event = ?frame.event,
                    "failed to parse responses SSE payload"
                );
                return Vec::new();
            }
        };

        let kind = payload
            .get("type")
            .and_then(Value::as_str)
            .or(frame.event.as_deref())
            .unwrap_or("")
            .to_owned();

        self.dispatch(&kind, &payload)
    }

    /// Synthesize a well-formed close when the upstream ends without
    /// `response.completed`.
    pub fn finalize(&mut self) -> Vec<Bytes> {
        if self.finished {
            return Vec::new();
        }
        let mut out = Vec::new();
        self.close_open_blocks(&mut out);
        out.extend(self.emit_message_delta("end_turn", 0, 0));
        out.push(self.emit_message_stop());
        self.finished = true;
        out
    }

    fn dispatch(&mut self, kind: &str, payload: &Value) -> Vec<Bytes> {
        match kind {
            "response.created" => self.on_response_created(payload),
            "response.in_progress" => Vec::new(),
            "response.output_item.added" => self.on_output_item_added(payload),
            "response.output_item.done" => self.on_output_item_done(payload),
            "response.content_part.added" => Vec::new(),
            "response.content_part.done" => Vec::new(),
            "response.output_text.delta" => self.on_output_text_delta(payload),
            "response.output_text.done" => self.on_text_finish(payload),
            "response.function_call_arguments.delta" => self.on_fc_args_delta(payload),
            "response.function_call_arguments.done" => self.on_fc_args_done(payload),
            "response.completed" => self.on_response_completed(payload),
            "response.failed" | "response.error" | "error" => self.on_response_failed(payload),
            // reasoning_summary_text.*, reasoning.*, refusal.*, unknown — drop
            _ => Vec::new(),
        }
    }

    fn on_response_created(&mut self, payload: &Value) -> Vec<Bytes> {
        if self.started {
            return Vec::new();
        }
        let response = payload.get("response").unwrap_or(payload);
        if let Some(id) = response.get("id").and_then(Value::as_str) {
            self.message_id = id.to_owned();
        } else {
            self.message_id = format!("msg_{}", ulid::Ulid::new());
        }
        if let Some(model) = response.get("model").and_then(Value::as_str) {
            self.model = model.to_owned();
        }
        self.started = true;
        let data = json!({
            "type": "message_start",
            "message": {
                "id": self.message_id,
                "type": "message",
                "role": "assistant",
                "model": self.model,
                "content": [],
                "stop_reason": null,
                "stop_sequence": null,
                "usage": {"input_tokens": 0, "output_tokens": 0}
            }
        });
        vec![sse("message_start", &data)]
    }

    fn on_output_item_added(&mut self, payload: &Value) -> Vec<Bytes> {
        let Some(output_index) = output_index_of(payload) else {
            return Vec::new();
        };
        let item = match payload.get("item") {
            Some(v) => v,
            None => return Vec::new(),
        };
        let item_type = item.get("type").and_then(Value::as_str).unwrap_or("");

        match item_type {
            "message" => {
                self.allocate_block(output_index, BlockKind::Text);
                Vec::new()
            }
            "function_call" => {
                let block_index = self.allocate_block(output_index, BlockKind::ToolUse);
                self.saw_tool_use = true;
                let call_id = item
                    .get("call_id")
                    .or_else(|| item.get("id"))
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_owned();
                let name = item
                    .get("name")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_owned();
                if let Some(state) = self.blocks.get_mut(&output_index) {
                    state.started = true;
                }
                let data = json!({
                    "type": "content_block_start",
                    "index": block_index,
                    "content_block": {
                        "type": "tool_use",
                        "id": call_id,
                        "name": name,
                        "input": {}
                    }
                });
                vec![sse("content_block_start", &data)]
            }
            _ => {
                self.allocate_block(output_index, BlockKind::Ignored);
                Vec::new()
            }
        }
    }

    fn on_output_item_done(&mut self, payload: &Value) -> Vec<Bytes> {
        let Some(output_index) = output_index_of(payload) else {
            return Vec::new();
        };
        self.close_block(output_index)
    }

    fn on_output_text_delta(&mut self, payload: &Value) -> Vec<Bytes> {
        let Some(output_index) = output_index_of(payload) else {
            return Vec::new();
        };
        let delta = payload
            .get("delta")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned();
        if delta.is_empty() {
            return Vec::new();
        }

        let mut out = Vec::new();
        let block_index = self.allocate_block(output_index, BlockKind::Text);
        if let Some(start) = self.maybe_open_text_block(output_index, block_index) {
            out.push(start);
        }
        let data = json!({
            "type": "content_block_delta",
            "index": block_index,
            "delta": {"type": "text_delta", "text": delta}
        });
        out.push(sse("content_block_delta", &data));
        out
    }

    fn on_text_finish(&mut self, payload: &Value) -> Vec<Bytes> {
        let Some(output_index) = output_index_of(payload) else {
            return Vec::new();
        };
        self.close_block(output_index)
    }

    fn on_fc_args_delta(&mut self, payload: &Value) -> Vec<Bytes> {
        let Some(output_index) = output_index_of(payload) else {
            return Vec::new();
        };
        let Some(state) = self.blocks.get(&output_index) else {
            return Vec::new();
        };
        if !matches!(state.kind, BlockKind::ToolUse) {
            return Vec::new();
        }
        let block_index = state.index;
        let delta = payload
            .get("delta")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned();
        if delta.is_empty() {
            return Vec::new();
        }
        let data = json!({
            "type": "content_block_delta",
            "index": block_index,
            "delta": {"type": "input_json_delta", "partial_json": delta}
        });
        vec![sse("content_block_delta", &data)]
    }

    fn on_fc_args_done(&mut self, payload: &Value) -> Vec<Bytes> {
        let Some(output_index) = output_index_of(payload) else {
            return Vec::new();
        };
        self.close_block(output_index)
    }

    fn on_response_completed(&mut self, payload: &Value) -> Vec<Bytes> {
        if self.finished {
            return Vec::new();
        }
        let response = payload.get("response").unwrap_or(payload);
        let usage = response.get("usage");
        let input_tokens = usage
            .and_then(|u| u.get("input_tokens"))
            .and_then(Value::as_u64)
            .unwrap_or(0);
        let output_tokens = usage
            .and_then(|u| u.get("output_tokens"))
            .and_then(Value::as_u64)
            .unwrap_or(0);

        let mut out = Vec::new();
        self.close_open_blocks(&mut out);
        let stop_reason = if self.saw_tool_use {
            "tool_use"
        } else {
            "end_turn"
        };
        out.extend(self.emit_message_delta(stop_reason, input_tokens, output_tokens));
        out.push(self.emit_message_stop());
        self.finished = true;
        out
    }

    fn on_response_failed(&mut self, payload: &Value) -> Vec<Bytes> {
        if self.finished {
            return Vec::new();
        }
        let err = payload
            .get("error")
            .or_else(|| payload.get("response").and_then(|r| r.get("error")));
        let kind = err
            .and_then(|e| e.get("type"))
            .and_then(Value::as_str)
            .unwrap_or("api_error")
            .to_owned();
        let message = err
            .and_then(|e| e.get("message"))
            .and_then(Value::as_str)
            .unwrap_or("upstream responses stream failed")
            .to_owned();
        self.finished = true;
        let data = json!({
            "type": "error",
            "error": {"type": kind, "message": message}
        });
        vec![sse("error", &data)]
    }

    /// Allocate (or reuse) the Anthropic block index for a given Responses
    /// `output_index`. Idempotent — repeated calls return the same index.
    fn allocate_block(&mut self, output_index: u32, kind: BlockKind) -> u32 {
        if let Some(existing) = self.blocks.get(&output_index) {
            return existing.index;
        }
        let index = self.next_block_index;
        self.next_block_index += 1;
        self.blocks.insert(
            output_index,
            BlockState {
                index,
                kind,
                started: false,
                stopped: false,
            },
        );
        index
    }

    /// Emit `content_block_start` for a text block if we haven't yet. Called
    /// lazily on the first text delta because Responses sometimes opens a
    /// `message` output item without ever producing text.
    fn maybe_open_text_block(&mut self, output_index: u32, block_index: u32) -> Option<Bytes> {
        let state = self.blocks.get_mut(&output_index)?;
        if state.started {
            return None;
        }
        state.started = true;
        let data = json!({
            "type": "content_block_start",
            "index": block_index,
            "content_block": {"type": "text", "text": ""}
        });
        Some(sse("content_block_start", &data))
    }

    fn close_block(&mut self, output_index: u32) -> Vec<Bytes> {
        let Some(state) = self.blocks.get_mut(&output_index) else {
            return Vec::new();
        };
        if state.stopped || !state.started {
            state.stopped = true;
            return Vec::new();
        }
        state.stopped = true;
        let index = state.index;
        let data = json!({"type": "content_block_stop", "index": index});
        vec![sse("content_block_stop", &data)]
    }

    fn close_open_blocks(&mut self, out: &mut Vec<Bytes>) {
        let mut indices: Vec<u32> = self.blocks.keys().copied().collect();
        indices.sort_unstable();
        for idx in indices {
            out.extend(self.close_block(idx));
        }
    }

    fn emit_message_delta(
        &self,
        stop_reason: &str,
        input_tokens: u64,
        output_tokens: u64,
    ) -> Vec<Bytes> {
        let data = json!({
            "type": "message_delta",
            "delta": {"stop_reason": stop_reason, "stop_sequence": null},
            "usage": {"input_tokens": input_tokens, "output_tokens": output_tokens}
        });
        vec![sse("message_delta", &data)]
    }

    fn emit_message_stop(&self) -> Bytes {
        let data = json!({"type": "message_stop"});
        sse("message_stop", &data)
    }
}

impl crate::streaming::relay::SseTranslator for ResponsesToAnthropicTranslator {
    fn ingest(&mut self, event: SseEvent) -> Vec<Bytes> {
        ResponsesToAnthropicTranslator::ingest(self, event)
    }

    fn finalize(&mut self) -> Vec<Bytes> {
        ResponsesToAnthropicTranslator::finalize(self)
    }
}

impl Default for ResponsesToAnthropicTranslator {
    fn default() -> Self {
        Self::new()
    }
}

fn sse(event_name: &str, data: &Value) -> Bytes {
    let payload = data.to_string();
    encode_event(Some(event_name), &payload, None)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frame(event: &str, data: Value) -> SseEvent {
        SseEvent {
            event: Some(event.to_owned()),
            data: data.to_string(),
            id: None,
        }
    }

    fn drain(translator: &mut ResponsesToAnthropicTranslator, frames: Vec<SseEvent>) -> String {
        let mut out = String::new();
        for f in frames {
            for chunk in translator.ingest(f) {
                out.push_str(std::str::from_utf8(&chunk).unwrap());
            }
        }
        for chunk in translator.finalize() {
            out.push_str(std::str::from_utf8(&chunk).unwrap());
        }
        out
    }

    #[test]
    fn text_only_stream() {
        let mut t = ResponsesToAnthropicTranslator::new();
        let frames = vec![
            frame(
                "response.created",
                json!({"type":"response.created","response":{"id":"resp_1","model":"gpt-5-codex"}}),
            ),
            frame(
                "response.output_item.added",
                json!({"type":"response.output_item.added","output_index":0,"item":{"type":"message","role":"assistant"}}),
            ),
            frame(
                "response.output_text.delta",
                json!({"type":"response.output_text.delta","output_index":0,"delta":"Hel"}),
            ),
            frame(
                "response.output_text.delta",
                json!({"type":"response.output_text.delta","output_index":0,"delta":"lo"}),
            ),
            frame(
                "response.output_text.done",
                json!({"type":"response.output_text.done","output_index":0,"text":"Hello"}),
            ),
            frame(
                "response.completed",
                json!({"type":"response.completed","response":{"usage":{"input_tokens":12,"output_tokens":7}}}),
            ),
        ];
        let wire = drain(&mut t, frames);
        assert!(wire.contains("event: message_start"));
        assert!(wire.contains("\"id\":\"resp_1\""));
        assert!(wire.contains("\"model\":\"gpt-5-codex\""));
        assert!(wire.contains("event: content_block_start"));
        assert!(wire.contains("\"type\":\"text\""));
        assert!(wire.contains("event: content_block_delta"));
        assert!(wire.contains("\"text\":\"Hel\""));
        assert!(wire.contains("\"text\":\"lo\""));
        assert!(wire.contains("event: content_block_stop"));
        assert!(wire.contains("event: message_delta"));
        assert!(wire.contains("\"stop_reason\":\"end_turn\""));
        assert!(wire.contains("\"input_tokens\":12"));
        assert!(wire.contains("\"output_tokens\":7"));
        assert!(wire.contains("event: message_stop"));
    }

    #[test]
    fn tool_call_stream_sets_stop_reason_tool_use() {
        let mut t = ResponsesToAnthropicTranslator::new();
        let frames = vec![
            frame(
                "response.created",
                json!({"type":"response.created","response":{"id":"resp_2","model":"gpt-5-codex"}}),
            ),
            frame(
                "response.output_item.added",
                json!({
                    "type":"response.output_item.added",
                    "output_index":0,
                    "item":{"type":"function_call","call_id":"call_42","name":"get_weather"}
                }),
            ),
            frame(
                "response.function_call_arguments.delta",
                json!({"type":"response.function_call_arguments.delta","output_index":0,"delta":"{\"city\":"}),
            ),
            frame(
                "response.function_call_arguments.delta",
                json!({"type":"response.function_call_arguments.delta","output_index":0,"delta":"\"SF\"}"}),
            ),
            frame(
                "response.function_call_arguments.done",
                json!({"type":"response.function_call_arguments.done","output_index":0,"arguments":"{\"city\":\"SF\"}"}),
            ),
            frame(
                "response.completed",
                json!({"type":"response.completed","response":{"usage":{"input_tokens":5,"output_tokens":9}}}),
            ),
        ];
        let wire = drain(&mut t, frames);
        assert!(wire.contains("\"type\":\"tool_use\""));
        assert!(wire.contains("\"id\":\"call_42\""));
        assert!(wire.contains("\"name\":\"get_weather\""));
        assert!(wire.contains("\"type\":\"input_json_delta\""));
        assert!(wire.contains("\"partial_json\":\"{\\\"city\\\":\""));
        assert!(wire.contains("\"partial_json\":\"\\\"SF\\\"}\""));
        assert!(wire.contains("\"stop_reason\":\"tool_use\""));
        assert!(wire.contains("event: message_stop"));
    }

    #[test]
    fn finalize_closes_blocks_when_upstream_truncates() {
        let mut t = ResponsesToAnthropicTranslator::new();
        let frames = vec![
            frame(
                "response.created",
                json!({"type":"response.created","response":{"id":"resp_3","model":"gpt-5-codex"}}),
            ),
            frame(
                "response.output_item.added",
                json!({"type":"response.output_item.added","output_index":0,"item":{"type":"message"}}),
            ),
            frame(
                "response.output_text.delta",
                json!({"type":"response.output_text.delta","output_index":0,"delta":"partial"}),
            ),
        ];
        let wire = drain(&mut t, frames);
        assert!(wire.contains("event: content_block_start"));
        assert!(wire.contains("event: content_block_stop"));
        assert!(wire.contains("event: message_delta"));
        assert!(wire.contains("event: message_stop"));
    }

    #[test]
    fn response_failed_emits_error() {
        let mut t = ResponsesToAnthropicTranslator::new();
        let frames = vec![
            frame(
                "response.created",
                json!({"type":"response.created","response":{"id":"resp_4","model":"gpt-5-codex"}}),
            ),
            frame(
                "response.failed",
                json!({"type":"response.failed","response":{"error":{"type":"server_error","message":"oops"}}}),
            ),
        ];
        let mut wire = String::new();
        for f in frames {
            for chunk in t.ingest(f) {
                wire.push_str(std::str::from_utf8(&chunk).unwrap());
            }
        }
        assert!(wire.contains("event: error"));
        assert!(wire.contains("\"type\":\"server_error\""));
        assert!(wire.contains("\"message\":\"oops\""));
        // Anthropic treats `error` as terminal — finalize is a no-op.
        assert!(t.finalize().is_empty());
    }

    #[test]
    fn unknown_event_kind_is_dropped() {
        let mut t = ResponsesToAnthropicTranslator::new();
        let chunks = t.ingest(frame(
            "response.reasoning_summary_text.delta",
            json!({"type":"response.reasoning_summary_text.delta","delta":"thinking..."}),
        ));
        assert!(chunks.is_empty());
    }

    #[test]
    fn allocate_block_is_idempotent() {
        let mut t = ResponsesToAnthropicTranslator::new();
        let a = t.allocate_block(0, BlockKind::Text);
        let b = t.allocate_block(0, BlockKind::Text);
        assert_eq!(a, b);
        let c = t.allocate_block(1, BlockKind::ToolUse);
        assert_ne!(a, c);
    }
}
