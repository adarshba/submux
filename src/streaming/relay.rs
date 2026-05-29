//! Generic upstream-SSE → translated-SSE relay state machine.
//!
//! Both the `/v1/chat/completions` (Anthropic → OpenAI) and
//! `/codex/v1/messages` (Codex Responses → Anthropic) routes drive the same
//! unfold loop: parse incoming bytes into SSE frames, feed them through a
//! translator, queue the translated frames, and flush on upstream close.
//!
//! Implementations of [`SseTranslator`] live alongside the concrete
//! translator types they wrap. Routes call [`drive`] with the upstream byte
//! stream, a translator instance, and an optional trailer (e.g. the OpenAI
//! `data: [DONE]\n\n` marker).

use bytes::Bytes;
use futures::{Stream, StreamExt};
use std::collections::VecDeque;

use crate::core::ResponseStream;
use crate::streaming::sse_parser::{SseEvent, SseStreamParser};

/// Consume parsed SSE frames from an upstream and emit translated bytes
/// ready to forward to the client.
pub trait SseTranslator: Send + 'static {
    /// Translate one parsed SSE event into zero or more output frames.
    fn ingest(&mut self, event: SseEvent) -> Vec<Bytes>;

    /// Flush any internal buffers when the upstream stream ends.
    fn finalize(&mut self) -> Vec<Bytes>;
}

enum Phase {
    Running,
    Draining,
    Done,
}

struct RelayState<T: SseTranslator> {
    upstream: ResponseStream,
    parser: SseStreamParser,
    translator: T,
    pending: VecDeque<Bytes>,
    phase: Phase,
    trailer: Option<Bytes>,
}

/// Build a streaming body that drives `translator` over `upstream`.
///
/// After the upstream closes (or errors), [`SseTranslator::finalize`] is
/// drained into the output, then `trailer` (if any) is appended, then the
/// stream ends.
pub fn drive<T: SseTranslator>(
    upstream: ResponseStream,
    translator: T,
    trailer: Option<Bytes>,
) -> impl Stream<Item = Result<Bytes, std::io::Error>> + Send + 'static {
    let state = RelayState {
        upstream,
        parser: SseStreamParser::new(),
        translator,
        pending: VecDeque::new(),
        phase: Phase::Running,
        trailer,
    };

    futures::stream::unfold(state, |mut st| async move {
        loop {
            if let Some(frame) = st.pending.pop_front() {
                return Some((Ok(frame), st));
            }
            match st.phase {
                Phase::Done => return None,
                Phase::Draining => {
                    for b in st.translator.finalize() {
                        st.pending.push_back(b);
                    }
                    if let Some(t) = st.trailer.take() {
                        st.pending.push_back(t);
                    }
                    st.phase = Phase::Done;
                }
                Phase::Running => match st.upstream.next().await {
                    Some(Ok(bytes)) => {
                        for ev in st.parser.push(&bytes) {
                            for b in st.translator.ingest(ev) {
                                st.pending.push_back(b);
                            }
                        }
                    }
                    Some(Err(err)) => {
                        tracing::warn!(error = %err, "upstream stream error");
                        for ev in st.parser.flush() {
                            for b in st.translator.ingest(ev) {
                                st.pending.push_back(b);
                            }
                        }
                        st.phase = Phase::Draining;
                    }
                    None => {
                        for ev in st.parser.flush() {
                            for b in st.translator.ingest(ev) {
                                st.pending.push_back(b);
                            }
                        }
                        st.phase = Phase::Draining;
                    }
                },
            }
        }
    })
}
