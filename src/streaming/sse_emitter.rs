//! Server-Sent Events encoder.
//!
//! Produces wire bytes for outbound SSE frames. Both the OpenAI and Anthropic
//! protocols share the same frame format; the only OpenAI-specific quirk is
//! the terminal `data: [DONE]\n\n` sentinel.

use bytes::{BufMut, Bytes, BytesMut};

/// Encode a single SSE event. `data` may contain newlines — each line is
/// emitted as its own `data:` field so the consumer can reconstruct the value
/// per the SSE spec.
pub fn encode_event(event_name: Option<&str>, data: &str, id: Option<&str>) -> Bytes {
    let mut buf = BytesMut::with_capacity(data.len() + 64);

    if let Some(name) = event_name {
        buf.put_slice(b"event: ");
        buf.put_slice(name.as_bytes());
        buf.put_u8(b'\n');
    }
    if let Some(id) = id {
        buf.put_slice(b"id: ");
        buf.put_slice(id.as_bytes());
        buf.put_u8(b'\n');
    }
    if data.is_empty() {
        buf.put_slice(b"data: \n");
    } else {
        for line in data.split('\n') {
            buf.put_slice(b"data: ");
            buf.put_slice(line.as_bytes());
            buf.put_u8(b'\n');
        }
    }
    buf.put_u8(b'\n');
    buf.freeze()
}

/// Encode an SSE comment line (`: text\n\n`). Useful for keepalives.
pub fn encode_comment(text: &str) -> Bytes {
    let mut buf = BytesMut::with_capacity(text.len() + 8);
    buf.put_slice(b": ");
    buf.put_slice(text.as_bytes());
    buf.put_slice(b"\n\n");
    buf.freeze()
}

/// OpenAI's terminator sentinel. Not part of the SSE spec — it's an OpenAI
/// convention that signals end-of-stream on chat completion endpoints.
pub fn encode_done() -> Bytes {
    Bytes::from_static(b"data: [DONE]\n\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn data_only_round_trip() {
        let frame = encode_event(None, "hello", None);
        assert_eq!(&frame[..], b"data: hello\n\n");
    }

    #[test]
    fn event_id_data() {
        let frame = encode_event(Some("ping"), "{}", Some("42"));
        assert_eq!(&frame[..], b"event: ping\nid: 42\ndata: {}\n\n");
    }

    #[test]
    fn multiline_data_splits_into_multiple_data_fields() {
        let frame = encode_event(None, "line1\nline2", None);
        assert_eq!(&frame[..], b"data: line1\ndata: line2\n\n");
    }

    #[test]
    fn comment_format() {
        let frame = encode_comment("keepalive");
        assert_eq!(&frame[..], b": keepalive\n\n");
    }

    #[test]
    fn done_marker() {
        assert_eq!(&encode_done()[..], b"data: [DONE]\n\n");
    }

    #[test]
    fn empty_data_remains_valid() {
        let frame = encode_event(None, "", None);
        assert_eq!(&frame[..], b"data: \n\n");
    }
}
