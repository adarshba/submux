//! Streaming primitives: SSE parse/emit, typed protocol event/chunk shapes,
//! the Anthropic↔OpenAI translator, and a shared relay state machine.

pub mod anthropic_events;
pub mod chat_relay;
pub mod checkpoint;
pub mod openai_chunks;
pub mod relay;
pub mod sse_emitter;
pub mod sse_parser;
pub mod tee;
pub mod translate;
pub mod translate_responses_to_anthropic;

pub use chat_relay::AnthropicToOpenAiRelay;
pub use relay::{drive, SseTranslator};
