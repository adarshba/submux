//! Translator wrapper that bridges [`AnthropicToOpenAiTranslator`] into the
//! generic [`crate::streaming::relay`] driver.
//!
//! The inner translator takes a typed `AnthropicEvent` and emits typed
//! `OpenAiChatChunk` values; this wrapper handles SSE-data parsing on the
//! way in and chunk-to-bytes encoding on the way out, so the relay loop can
//! work in pure `Bytes`.

use bytes::Bytes;

use crate::protocols::openai::emit::chunk_to_sse;
use crate::streaming::anthropic_events::AnthropicEvent;
use crate::streaming::relay::SseTranslator;
use crate::streaming::sse_parser::SseEvent;
use crate::streaming::translate::AnthropicToOpenAiTranslator;

/// Bridge between Anthropic upstream SSE bytes and OpenAI chunk SSE bytes.
pub struct AnthropicToOpenAiRelay {
    inner: AnthropicToOpenAiTranslator,
}

impl AnthropicToOpenAiRelay {
    /// Wrap a fresh inner translator with the given OpenAI `model` override.
    pub fn new(model_override: Option<String>) -> Self {
        Self {
            inner: AnthropicToOpenAiTranslator::new(model_override),
        }
    }
}

impl SseTranslator for AnthropicToOpenAiRelay {
    fn ingest(&mut self, event: SseEvent) -> Vec<Bytes> {
        if event.data.is_empty() {
            return Vec::new();
        }
        let parsed = match AnthropicEvent::from_sse_data(&event.data) {
            Ok(p) => p,
            Err(err) => {
                tracing::warn!(
                    error = %err,
                    event = ?event.event,
                    "failed to parse anthropic SSE event payload"
                );
                return Vec::new();
            }
        };
        self.inner
            .ingest(parsed)
            .into_iter()
            .filter_map(|chunk| match chunk_to_sse(&chunk) {
                Ok(b) => Some(b),
                Err(err) => {
                    tracing::error!(error = %err, "failed to encode chat completion chunk");
                    None
                }
            })
            .collect()
    }

    fn finalize(&mut self) -> Vec<Bytes> {
        self.inner
            .finalize()
            .into_iter()
            .filter_map(|chunk| chunk_to_sse(&chunk).ok())
            .collect()
    }
}
