//! Outbound OpenAI wire-format helpers.
//!
//! These thin functions sit between the translator's typed
//! [`OpenAiChatChunk`]s and the raw bytes that go out on the SSE stream.
//! Non-streaming render (full `chat.completion` object) lives elsewhere; this
//! module covers the streaming surface only.

use bytes::Bytes;

use crate::streaming::openai_chunks::OpenAiChatChunk;
use crate::streaming::sse_emitter;

/// Encode a translator-produced chunk as an SSE frame.
///
/// Returns `Err` only when serialization fails — which would indicate a bug
/// in this crate (the chunk types are owned and fully serializable). Callers
/// at the protocol boundary should treat any error as a 5xx.
pub fn chunk_to_sse(chunk: &OpenAiChatChunk) -> Result<Bytes, serde_json::Error> {
    let data = serde_json::to_string(chunk)?;
    Ok(sse_emitter::encode_event(None, &data, None))
}

/// OpenAI's `data: [DONE]\n\n` end-of-stream sentinel.
pub fn done_marker() -> Bytes {
    sse_emitter::encode_done()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::streaming::openai_chunks::{chat_completion_chunk, OpenAiChoice, OpenAiDelta};

    #[test]
    fn encodes_chunk_as_sse_frame() {
        let chunk = OpenAiChatChunk {
            id: "chatcmpl-x".into(),
            object: chat_completion_chunk(),
            created: 1,
            model: "gpt-4o".into(),
            choices: vec![OpenAiChoice {
                index: 0,
                delta: OpenAiDelta {
                    content: Some("hi".into()),
                    ..Default::default()
                },
                finish_reason: None,
            }],
        };
        let bytes = chunk_to_sse(&chunk).unwrap();
        let s = std::str::from_utf8(&bytes).unwrap();
        assert!(s.starts_with("data: {"));
        assert!(s.ends_with("\n\n"));
        assert!(s.contains("\"content\":\"hi\""));
    }

    #[test]
    fn done_marker_exact_bytes() {
        assert_eq!(&done_marker()[..], b"data: [DONE]\n\n");
    }
}
