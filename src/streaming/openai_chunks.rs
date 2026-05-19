//! Typed OpenAI Chat Completions streaming chunks.
//!
//! These are emitted by the [`AnthropicToOpenAiTranslator`][crate::streaming::translate::AnthropicToOpenAiTranslator]
//! and serialized into SSE `data:` payloads on the wire.

use serde::{Deserialize, Serialize};

/// Stable string constant matching OpenAI's chunk object discriminator.
pub fn chat_completion_chunk() -> String {
    "chat.completion.chunk".to_string()
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OpenAiChatChunk {
    pub id: String,
    /// Always "chat.completion.chunk" in OpenAI's wire format. We use `String`
    /// so the struct is owned and serde round-trips cleanly.
    pub object: String,
    pub created: i64,
    pub model: String,
    pub choices: Vec<OpenAiChoice>,
}

impl OpenAiChatChunk {
    /// Serialize to a JSON string suitable for the `data:` field of an SSE
    /// frame. Failures here would indicate a bug in this crate, not a runtime
    /// condition, so we surface them via `serde_json::Error` rather than
    /// panicking.
    pub fn to_sse_data(&self) -> serde_json::Result<String> {
        serde_json::to_string(self)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OpenAiChoice {
    pub index: u32,
    pub delta: OpenAiDelta,
    pub finish_reason: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct OpenAiDelta {
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub role: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub content: Option<String>,
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub tool_calls: Vec<OpenAiToolCallDelta>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OpenAiToolCallDelta {
    pub index: u32,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub id: Option<String>,
    /// Always "function" in current OpenAI schemas.
    #[serde(rename = "type")]
    pub kind: String,
    pub function: OpenAiFunctionDelta,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct OpenAiFunctionDelta {
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub arguments: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn serializes_minimal_chunk() {
        let chunk = OpenAiChatChunk {
            id: "chatcmpl-1".into(),
            object: chat_completion_chunk(),
            created: 100,
            model: "gpt-4o".into(),
            choices: vec![OpenAiChoice {
                index: 0,
                delta: OpenAiDelta {
                    role: Some("assistant".into()),
                    ..Default::default()
                },
                finish_reason: None,
            }],
        };
        let s = chunk.to_sse_data().unwrap();
        assert!(s.contains("\"object\":\"chat.completion.chunk\""));
        assert!(s.contains("\"role\":\"assistant\""));
        assert!(!s.contains("\"content\""));
        assert!(!s.contains("\"tool_calls\""));
    }

    #[test]
    fn tool_call_delta_emits_function_payload() {
        let chunk = OpenAiChatChunk {
            id: "chatcmpl-2".into(),
            object: chat_completion_chunk(),
            created: 200,
            model: "gpt-4o".into(),
            choices: vec![OpenAiChoice {
                index: 0,
                delta: OpenAiDelta {
                    tool_calls: vec![OpenAiToolCallDelta {
                        index: 0,
                        id: Some("call_1".into()),
                        kind: "function".into(),
                        function: OpenAiFunctionDelta {
                            name: Some("get_weather".into()),
                            arguments: None,
                        },
                    }],
                    ..Default::default()
                },
                finish_reason: None,
            }],
        };
        let s = chunk.to_sse_data().unwrap();
        assert!(s.contains("\"tool_calls\":[{"));
        assert!(s.contains("\"id\":\"call_1\""));
        assert!(s.contains("\"name\":\"get_weather\""));
        assert!(!s.contains("\"arguments\""));
    }
}
