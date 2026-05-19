use serde::{Deserialize, Serialize};

use crate::core::ContentBlock;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NormalizedResponse {
    pub id: String,
    pub model: String,
    pub content: Vec<ContentBlock>,
    pub stop_reason: Option<FinishReason>,
    pub usage: Usage,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FinishReason {
    EndTurn,
    MaxTokens,
    StopSequence,
    ToolUse,
    ContentFiltered,
    Refusal,
    Error,
}

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize)]
pub struct Usage {
    pub input_tokens: u32,
    pub output_tokens: u32,
    pub cache_creation_input_tokens: u32,
    pub cache_read_input_tokens: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Delta {
    MessageStart {
        id: String,
        model: String,
    },
    Ping,
    ContentBlockStart {
        index: u32,
        block: ContentBlock,
    },
    TextDelta {
        index: u32,
        text: String,
    },
    InputJsonDelta {
        index: u32,
        partial_json: String,
    },
    ThinkingDelta {
        index: u32,
        thinking: String,
    },
    SignatureDelta {
        index: u32,
        signature: String,
    },
    ContentBlockStop {
        index: u32,
    },
    MessageDelta {
        stop_reason: Option<FinishReason>,
        usage: Usage,
    },
    MessageStop,
    Error {
        message: String,
    },
}
