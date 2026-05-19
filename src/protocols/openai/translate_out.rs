//! Outbound OpenAI translation.
//!
//! The streaming Anthropic→OpenAI state machine lives in
//! [`crate::streaming::translate`]. This module is a re-export shim so the
//! protocol surface stays internally consistent (`protocols::openai::*` for
//! parsing/emit, `streaming::*` for stream codecs).

pub use crate::streaming::translate::AnthropicToOpenAiTranslator;
