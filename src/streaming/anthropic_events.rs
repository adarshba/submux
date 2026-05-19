//! Typed Anthropic Messages streaming events.
//!
//! Mirrors the Anthropic Messages SSE event types so the streaming
//! translator can pattern-match instead of poking at `serde_json::Value`.
//! See: <https://docs.anthropic.com/en/api/messages-streaming>.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum AnthropicEvent {
    MessageStart {
        message: AnthropicMessageMeta,
    },
    ContentBlockStart {
        index: u32,
        content_block: AnthropicBlockStart,
    },
    ContentBlockDelta {
        index: u32,
        delta: AnthropicDelta,
    },
    ContentBlockStop {
        index: u32,
    },
    MessageDelta {
        delta: AnthropicMessageDelta,
        #[serde(default)]
        usage: AnthropicUsage,
    },
    MessageStop,
    Ping,
    Error {
        error: AnthropicErrorBody,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AnthropicMessageMeta {
    pub id: String,
    pub model: String,
    pub role: String,
    #[serde(default)]
    pub usage: AnthropicUsage,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum AnthropicBlockStart {
    Text {
        #[serde(default)]
        text: String,
    },
    ToolUse {
        id: String,
        name: String,
        #[serde(default)]
        input: serde_json::Value,
    },
    Thinking {
        #[serde(default)]
        thinking: String,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum AnthropicDelta {
    TextDelta { text: String },
    InputJsonDelta { partial_json: String },
    ThinkingDelta { thinking: String },
    SignatureDelta { signature: String },
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct AnthropicUsage {
    #[serde(default)]
    pub input_tokens: u32,
    #[serde(default)]
    pub output_tokens: u32,
    #[serde(default)]
    pub cache_creation_input_tokens: Option<u32>,
    #[serde(default)]
    pub cache_read_input_tokens: Option<u32>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AnthropicMessageDelta {
    pub stop_reason: Option<String>,
    pub stop_sequence: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AnthropicErrorBody {
    #[serde(rename = "type")]
    pub kind: String,
    pub message: String,
}

impl AnthropicEvent {
    /// Parse an SSE `data:` payload into a typed event.
    pub fn from_sse_data(data: &str) -> serde_json::Result<Self> {
        serde_json::from_str(data)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_message_start() {
        let json = r#"{"type":"message_start","message":{"id":"msg_1","model":"claude-3-5-sonnet","role":"assistant","usage":{"input_tokens":10,"output_tokens":0}}}"#;
        let ev = AnthropicEvent::from_sse_data(json).unwrap();
        match ev {
            AnthropicEvent::MessageStart { message } => {
                assert_eq!(message.id, "msg_1");
                assert_eq!(message.model, "claude-3-5-sonnet");
                assert_eq!(message.usage.input_tokens, 10);
            }
            _ => panic!("wrong variant"),
        }
    }

    #[test]
    fn parse_content_block_start_text() {
        let json =
            r#"{"type":"content_block_start","index":0,"content_block":{"type":"text","text":""}}"#;
        let ev = AnthropicEvent::from_sse_data(json).unwrap();
        match ev {
            AnthropicEvent::ContentBlockStart {
                index,
                content_block: AnthropicBlockStart::Text { text },
            } => {
                assert_eq!(index, 0);
                assert_eq!(text, "");
            }
            _ => panic!("wrong variant"),
        }
    }

    #[test]
    fn parse_content_block_start_tool_use() {
        let json = r#"{"type":"content_block_start","index":1,"content_block":{"type":"tool_use","id":"toolu_1","name":"get_weather","input":{}}}"#;
        let ev = AnthropicEvent::from_sse_data(json).unwrap();
        match ev {
            AnthropicEvent::ContentBlockStart {
                index: 1,
                content_block: AnthropicBlockStart::ToolUse { id, name, .. },
            } => {
                assert_eq!(id, "toolu_1");
                assert_eq!(name, "get_weather");
            }
            _ => panic!("wrong variant"),
        }
    }

    #[test]
    fn parse_text_delta() {
        let json =
            r#"{"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"hi"}}"#;
        let ev = AnthropicEvent::from_sse_data(json).unwrap();
        match ev {
            AnthropicEvent::ContentBlockDelta {
                index: 0,
                delta: AnthropicDelta::TextDelta { text },
            } => assert_eq!(text, "hi"),
            _ => panic!("wrong variant"),
        }
    }

    #[test]
    fn parse_input_json_delta() {
        let json = r#"{"type":"content_block_delta","index":1,"delta":{"type":"input_json_delta","partial_json":"{\"a\":"}}"#;
        let ev = AnthropicEvent::from_sse_data(json).unwrap();
        match ev {
            AnthropicEvent::ContentBlockDelta {
                index: 1,
                delta: AnthropicDelta::InputJsonDelta { partial_json },
            } => assert_eq!(partial_json, "{\"a\":"),
            _ => panic!("wrong variant"),
        }
    }

    #[test]
    fn parse_message_delta() {
        let json = r#"{"type":"message_delta","delta":{"stop_reason":"end_turn","stop_sequence":null},"usage":{"input_tokens":10,"output_tokens":5}}"#;
        let ev = AnthropicEvent::from_sse_data(json).unwrap();
        match ev {
            AnthropicEvent::MessageDelta { delta, usage } => {
                assert_eq!(delta.stop_reason.as_deref(), Some("end_turn"));
                assert_eq!(usage.output_tokens, 5);
            }
            _ => panic!("wrong variant"),
        }
    }

    #[test]
    fn parse_ping_and_stop() {
        assert!(matches!(
            AnthropicEvent::from_sse_data(r#"{"type":"ping"}"#).unwrap(),
            AnthropicEvent::Ping
        ));
        assert!(matches!(
            AnthropicEvent::from_sse_data(r#"{"type":"message_stop"}"#).unwrap(),
            AnthropicEvent::MessageStop
        ));
    }

    #[test]
    fn parse_error() {
        let json = r#"{"type":"error","error":{"type":"overloaded_error","message":"try later"}}"#;
        let ev = AnthropicEvent::from_sse_data(json).unwrap();
        match ev {
            AnthropicEvent::Error { error } => {
                assert_eq!(error.kind, "overloaded_error");
                assert_eq!(error.message, "try later");
            }
            _ => panic!("wrong variant"),
        }
    }
}
