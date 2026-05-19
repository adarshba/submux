use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum AnthropicEvent {
    MessageStart { message: Value },
    Ping,
    ContentBlockStart { index: u32, content_block: Value },
    ContentBlockDelta { index: u32, delta: Value },
    ContentBlockStop { index: u32 },
    MessageDelta { delta: Value, usage: Option<Value> },
    MessageStop,
    Error { error: Value },
}
