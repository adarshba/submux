//! Pass-through tap that accounts Anthropic token usage from an SSE stream.
//!
//! Wraps a [`ResponseStream`], forwarding every byte unchanged to the client
//! while parsing the Anthropic Messages events to read `usage`. Input tokens
//! arrive on `message_start`, the final output count on `message_delta`; both
//! are recorded once when the stream ends.

use futures::StreamExt;

use crate::core::{ConsumerId, ResponseStream};
use crate::streaming::anthropic_events::AnthropicEvent;
use crate::streaming::sse_parser::SseStreamParser;
use crate::telemetry::metrics;

struct TapState {
    upstream: ResponseStream,
    parser: SseStreamParser,
    consumer: ConsumerId,
    model: String,
    input_tokens: u64,
    output_tokens: u64,
    done: bool,
}

/// Forward `upstream` unchanged while metering `submux_tokens_total` for
/// `consumer` against `model` when the stream completes.
pub fn meter_anthropic_tokens(
    upstream: ResponseStream,
    consumer: ConsumerId,
    model: String,
) -> ResponseStream {
    let state = TapState {
        upstream,
        parser: SseStreamParser::new(),
        consumer,
        model,
        input_tokens: 0,
        output_tokens: 0,
        done: false,
    };

    futures::stream::unfold(state, |mut st| async move {
        if st.done {
            return None;
        }
        match st.upstream.next().await {
            Some(Ok(bytes)) => {
                for ev in st.parser.push(&bytes) {
                    account(&mut st, &ev.data);
                }
                Some((Ok(bytes), st))
            }
            Some(Err(err)) => {
                for ev in st.parser.flush() {
                    account(&mut st, &ev.data);
                }
                record(&st);
                st.done = true;
                Some((Err(err), st))
            }
            None => {
                for ev in st.parser.flush() {
                    account(&mut st, &ev.data);
                }
                record(&st);
                None
            }
        }
    })
    .boxed()
}

fn account(st: &mut TapState, data: &str) {
    if data.is_empty() {
        return;
    }
    match AnthropicEvent::from_sse_data(data) {
        Ok(AnthropicEvent::MessageStart { message }) => {
            st.input_tokens = u64::from(message.usage.input_tokens);
            if st.model.is_empty() {
                st.model = message.model;
            }
        }
        Ok(AnthropicEvent::MessageDelta { usage, .. }) => {
            st.output_tokens = u64::from(usage.output_tokens);
        }
        _ => {}
    }
}

fn record(st: &TapState) {
    if st.input_tokens > 0 {
        metrics::add_tokens(st.consumer.as_str(), "input", &st.model, st.input_tokens);
    }
    if st.output_tokens > 0 {
        metrics::add_tokens(st.consumer.as_str(), "output", &st.model, st.output_tokens);
    }
}
