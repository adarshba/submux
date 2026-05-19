use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::core::{ContentBlock, Message, ProtocolKind};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NormalizedRequest {
    pub protocol: ProtocolKind,
    #[serde(default)]
    pub system: Vec<ContentBlock>,
    pub messages: Vec<Message>,
    #[serde(default)]
    pub tools: Vec<Tool>,
    #[serde(default)]
    pub stream: bool,
    pub model_hint: Option<String>,
    pub max_tokens: Option<u32>,
    pub temperature: Option<f32>,
    pub top_p: Option<f32>,
    pub top_k: Option<u32>,
    #[serde(default)]
    pub passthrough: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Tool {
    pub name: String,
    pub description: Option<String>,
    pub input_schema: Value,
}
