//! OpenAI Chat Completions protocol — inbound parser, outbound emitter, and
//! translation into the canonical Anthropic-shaped `NormalizedRequest`.

pub mod emit;
pub mod parse;
pub mod translate_in;
pub mod translate_out;

pub use parse::{parse_chat_body, OpenAiChatRequest, OpenAiFunctionDef, OpenAiMessage, OpenAiTool};
pub use translate_in::openai_to_normalized;
pub use translate_out::AnthropicToOpenAiTranslator;
