//! Buffer a stream of OpenAI chat-completion *chunks* into the single
//! non-streaming `chat.completion` JSON value.
//!
//! Used by `/v1/chat/completions` when the client sent `stream: false`:
//! the upstream call still returns SSE chunks, so we accumulate them and
//! collapse into the canonical non-streaming response shape.

use serde_json::{Map, Value, json};
use std::collections::BTreeMap;

use crate::streaming::openai_chunks::OpenAiChatChunk;

/// Accumulator for one tool call across many delta chunks. Fields are filled
/// as the corresponding delta fragments arrive on the wire.
#[derive(Debug, Default)]
struct ToolCallAccumulator {
    id: Option<String>,
    kind: Option<String>,
    name: Option<String>,
    arguments: String,
}

impl ToolCallAccumulator {
    fn into_value(self) -> Value {
        let mut obj = Map::new();
        obj.insert("id".to_string(), Value::String(self.id.unwrap_or_default()));
        obj.insert(
            "type".to_string(),
            Value::String(self.kind.unwrap_or_else(|| "function".to_string())),
        );
        let mut func = Map::new();
        func.insert(
            "name".to_string(),
            Value::String(self.name.unwrap_or_default()),
        );
        func.insert("arguments".to_string(), Value::String(self.arguments));
        obj.insert("function".to_string(), Value::Object(func));
        Value::Object(obj)
    }
}

/// Fold a sequence of streaming chunks into the non-streaming
/// `chat.completion` JSON shape.
///
/// * Text deltas on choice index 0 are concatenated into `message.content`.
/// * Tool-call deltas are accumulated by their `tool_calls[*].index`.
/// * `finish_reason` is taken from the last chunk that supplied one;
///   defaults to `"stop"` if the upstream never sent one.
pub fn chunks_to_completion(chunks: &[OpenAiChatChunk], model: &str, request_id: &str) -> Value {
    let mut content = String::new();
    let mut tool_calls: BTreeMap<u32, ToolCallAccumulator> = BTreeMap::new();
    let mut finish_reason: Option<String> = None;
    let mut created: i64 = 0;

    for chunk in chunks {
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
            .map(ToolCallAccumulator::into_value)
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
