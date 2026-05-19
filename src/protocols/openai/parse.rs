//! OpenAI Chat Completions inbound request parser.
//!
//! Decodes the wire JSON into strongly-typed Rust structures. Translation
//! into the canonical [`NormalizedRequest`][crate::core::NormalizedRequest]
//! shape lives in [`super::translate_in`].

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct OpenAiChatRequest {
    pub model: String,
    pub messages: Vec<OpenAiMessage>,
    #[serde(default)]
    pub stream: bool,
    #[serde(default)]
    pub max_tokens: Option<u32>,
    #[serde(default)]
    pub temperature: Option<f32>,
    #[serde(default)]
    pub top_p: Option<f32>,
    #[serde(default)]
    pub tools: Vec<OpenAiTool>,
    #[serde(default)]
    pub tool_choice: Option<serde_json::Value>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct OpenAiMessage {
    pub role: String,
    /// Either a plain string, an array of content parts, or absent (`null`).
    #[serde(default)]
    pub content: serde_json::Value,
    #[serde(default)]
    pub tool_calls: Vec<serde_json::Value>,
    #[serde(default)]
    pub tool_call_id: Option<String>,
    #[serde(default)]
    pub name: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct OpenAiTool {
    #[serde(rename = "type")]
    pub kind: String,
    pub function: OpenAiFunctionDef,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct OpenAiFunctionDef {
    pub name: String,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub parameters: serde_json::Value,
}

/// Strict parse of an inbound OpenAI Chat Completions body.
pub fn parse_chat_body(bytes: &[u8]) -> Result<OpenAiChatRequest, serde_json::Error> {
    serde_json::from_slice(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_minimal() {
        let body = br#"{"model":"gpt-4o","messages":[{"role":"user","content":"hi"}]}"#;
        let req = parse_chat_body(body).unwrap();
        assert_eq!(req.model, "gpt-4o");
        assert_eq!(req.messages.len(), 1);
        assert_eq!(req.messages[0].role, "user");
        assert_eq!(req.messages[0].content.as_str(), Some("hi"));
        assert!(!req.stream);
    }

    #[test]
    fn parses_array_content() {
        let body = br#"{"model":"gpt-4o","messages":[{"role":"user","content":[{"type":"text","text":"hello"}]}]}"#;
        let req = parse_chat_body(body).unwrap();
        assert!(req.messages[0].content.is_array());
    }

    #[test]
    fn parses_tools() {
        let body = br#"{"model":"gpt-4o","messages":[],"tools":[{"type":"function","function":{"name":"f","parameters":{"type":"object"}}}]}"#;
        let req = parse_chat_body(body).unwrap();
        assert_eq!(req.tools.len(), 1);
        assert_eq!(req.tools[0].function.name, "f");
    }

    #[test]
    fn missing_content_is_null_value() {
        let body = br#"{"model":"gpt-4o","messages":[{"role":"assistant","tool_calls":[]}]}"#;
        let req = parse_chat_body(body).unwrap();
        assert!(req.messages[0].content.is_null());
    }
}
