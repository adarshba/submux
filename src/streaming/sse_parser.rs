//! Incremental Server-Sent Events parser.
//!
//! Per the HTML SSE spec, a stream is a sequence of UTF-8 lines terminated by
//! `\n`, `\r\n`, or `\r`. A blank line dispatches whatever fields have been
//! accumulated. Field syntax is `name: value`. We recognise three fields:
//! `event`, `data` (repeatable, joined with `\n`), and `id`. Lines starting
//! with `:` are comments and are silently discarded.
//!
//! This parser is byte-oriented and handles chunk boundaries that fall in the
//! middle of a line. Callers feed [`SseStreamParser::push`] arbitrary byte
//! slices and consume the [`SseEvent`]s it returns.

/// A fully parsed SSE event.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SseEvent {
    pub event: Option<String>,
    pub data: String,
    pub id: Option<String>,
}

/// Incremental parser state.
#[derive(Debug, Default)]
pub struct SseStreamParser {
    /// Carry-over bytes that did not yet end with a newline.
    line_buf: Vec<u8>,
    /// Fields accumulated for the current (not yet dispatched) event.
    cur_event: Option<String>,
    cur_data: Vec<String>,
    cur_id: Option<String>,
    /// True once we have observed any field for the in-progress event.
    has_fields: bool,
}

impl SseStreamParser {
    pub fn new() -> Self {
        Self::default()
    }

    /// Feed raw bytes from the upstream. Returns zero or more fully-parsed events.
    pub fn push(&mut self, chunk: &[u8]) -> Vec<SseEvent> {
        let mut out = Vec::new();
        self.line_buf.extend_from_slice(chunk);

        loop {
            let nl_pos = self.line_buf.iter().position(|b| *b == b'\n');
            let Some(pos) = nl_pos else {
                break;
            };

            let mut end = pos;
            if end > 0 && self.line_buf[end - 1] == b'\r' {
                end -= 1;
            }
            let line_bytes = self.line_buf[..end].to_vec();
            self.line_buf.drain(..=pos);

            self.process_line(&line_bytes, &mut out);
        }

        out
    }

    /// Drain on stream end. Returns any remaining buffered event.
    pub fn flush(&mut self) -> Vec<SseEvent> {
        let mut out = Vec::new();
        if !self.line_buf.is_empty() {
            let line = std::mem::take(&mut self.line_buf);
            let trimmed: &[u8] = if line.last() == Some(&b'\r') {
                &line[..line.len() - 1]
            } else {
                &line[..]
            };
            self.process_line(trimmed, &mut out);
        }
        if self.has_fields {
            self.dispatch(&mut out);
        }
        out
    }

    fn process_line(&mut self, line: &[u8], out: &mut Vec<SseEvent>) {
        if line.is_empty() {
            if self.has_fields {
                self.dispatch(out);
            }
            return;
        }
        if line[0] == b':' {
            return;
        }

        let (field_bytes, value_bytes) = match line.iter().position(|b| *b == b':') {
            Some(idx) => {
                let mut v_start = idx + 1;
                if v_start < line.len() && line[v_start] == b' ' {
                    v_start += 1;
                }
                (&line[..idx], &line[v_start..])
            }
            None => (line, &[][..]),
        };

        let Ok(field) = std::str::from_utf8(field_bytes) else {
            return;
        };
        let Ok(value) = std::str::from_utf8(value_bytes) else {
            return;
        };

        match field {
            "event" => {
                self.has_fields = true;
                self.cur_event = Some(value.to_owned());
            }
            "data" => {
                self.has_fields = true;
                self.cur_data.push(value.to_owned());
            }
            "id" => {
                self.has_fields = true;
                if !value.contains('\0') {
                    self.cur_id = Some(value.to_owned());
                }
            }
            _ => {}
        }
    }

    fn dispatch(&mut self, out: &mut Vec<SseEvent>) {
        let event = self.cur_event.take();
        let id = self.cur_id.take();
        let data = if self.cur_data.is_empty() {
            String::new()
        } else {
            self.cur_data.join("\n")
        };
        self.cur_data.clear();
        self.has_fields = false;
        out.push(SseEvent { event, data, id });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn simple_data_only() {
        let mut p = SseStreamParser::new();
        let evs = p.push(b"data: hello\n\n");
        assert_eq!(evs.len(), 1);
        assert_eq!(evs[0].data, "hello");
        assert!(evs[0].event.is_none());
        assert!(evs[0].id.is_none());
    }

    #[test]
    fn event_and_data() {
        let mut p = SseStreamParser::new();
        let evs = p.push(b"event: message_start\ndata: {\"x\":1}\n\n");
        assert_eq!(evs.len(), 1);
        assert_eq!(evs[0].event.as_deref(), Some("message_start"));
        assert_eq!(evs[0].data, "{\"x\":1}");
    }

    #[test]
    fn multi_line_data() {
        let mut p = SseStreamParser::new();
        let evs = p.push(b"data: one\ndata: two\ndata: three\n\n");
        assert_eq!(evs.len(), 1);
        assert_eq!(evs[0].data, "one\ntwo\nthree");
    }

    #[test]
    fn partial_chunks_split_midline() {
        let mut p = SseStreamParser::new();
        assert!(p.push(b"data: hel").is_empty());
        assert!(p.push(b"lo\n").is_empty());
        let evs = p.push(b"\n");
        assert_eq!(evs.len(), 1);
        assert_eq!(evs[0].data, "hello");
    }

    #[test]
    fn comments_are_ignored() {
        let mut p = SseStreamParser::new();
        let evs = p.push(b": keepalive\ndata: ok\n\n");
        assert_eq!(evs.len(), 1);
        assert_eq!(evs[0].data, "ok");
    }

    #[test]
    fn crlf_line_endings() {
        let mut p = SseStreamParser::new();
        let evs = p.push(b"event: ping\r\ndata: {}\r\n\r\n");
        assert_eq!(evs.len(), 1);
        assert_eq!(evs[0].event.as_deref(), Some("ping"));
        assert_eq!(evs[0].data, "{}");
    }

    #[test]
    fn multiple_events_in_one_chunk() {
        let mut p = SseStreamParser::new();
        let evs = p.push(b"data: a\n\ndata: b\n\ndata: c\n\n");
        assert_eq!(evs.len(), 3);
        assert_eq!(evs[0].data, "a");
        assert_eq!(evs[1].data, "b");
        assert_eq!(evs[2].data, "c");
    }

    #[test]
    fn flush_yields_buffered_event() {
        let mut p = SseStreamParser::new();
        let evs = p.push(b"data: tail\n");
        assert!(evs.is_empty());
        let evs = p.flush();
        assert_eq!(evs.len(), 1);
        assert_eq!(evs[0].data, "tail");
    }

    #[test]
    fn id_field_parses() {
        let mut p = SseStreamParser::new();
        let evs = p.push(b"id: 42\ndata: x\n\n");
        assert_eq!(evs.len(), 1);
        assert_eq!(evs[0].id.as_deref(), Some("42"));
    }

    #[test]
    fn field_without_colon_treated_as_empty_value() {
        let mut p = SseStreamParser::new();
        let evs = p.push(b"data\n\n");
        assert_eq!(evs.len(), 1);
        assert_eq!(evs[0].data, "");
    }
}
