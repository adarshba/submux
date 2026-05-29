//! Pass-through taps that account token usage from a provider SSE stream.
//!
//! Each tap forwards every byte unchanged to the client while parsing events to
//! read `usage`, recording `submux_tokens_total` once when the stream ends.
//! Anthropic reports input on `message_start` and final output on
//! `message_delta`; Codex reports both on `response.completed`.

use futures::StreamExt;

use crate::core::{ConsumerId, ResponseStream};
use crate::streaming::anthropic_events::AnthropicEvent;
use crate::streaming::sse_parser::SseStreamParser;
use crate::telemetry::metrics;

/// Extracts cumulative `(input, output)` token counts from one SSE `data`
/// payload, updating the running totals in place.
type UsageExtract = fn(&str, &mut u64, &mut u64);

struct TapState {
    upstream: ResponseStream,
    parser: SseStreamParser,
    consumer: ConsumerId,
    protocol: &'static str,
    model: String,
    input_tokens: u64,
    output_tokens: u64,
    extract: UsageExtract,
    done: bool,
}

/// Meter Anthropic Messages token usage off `upstream`.
pub fn meter_anthropic_tokens(
    upstream: ResponseStream,
    consumer: ConsumerId,
    model: String,
) -> ResponseStream {
    meter_tokens(upstream, consumer, "anthropic", model, account_anthropic)
}

/// Meter Codex Responses token usage off `upstream`.
pub fn meter_codex_tokens(
    upstream: ResponseStream,
    consumer: ConsumerId,
    model: String,
) -> ResponseStream {
    meter_tokens(upstream, consumer, "openai", model, account_codex)
}

fn meter_tokens(
    upstream: ResponseStream,
    consumer: ConsumerId,
    protocol: &'static str,
    model: String,
    extract: UsageExtract,
) -> ResponseStream {
    let state = TapState {
        upstream,
        parser: SseStreamParser::new(),
        consumer,
        protocol,
        model,
        input_tokens: 0,
        output_tokens: 0,
        extract,
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
    let extract = st.extract;
    extract(data, &mut st.input_tokens, &mut st.output_tokens);
}

fn account_anthropic(data: &str, input: &mut u64, output: &mut u64) {
    match AnthropicEvent::from_sse_data(data) {
        Ok(AnthropicEvent::MessageStart { message }) => {
            *input = u64::from(message.usage.input_tokens);
        }
        Ok(AnthropicEvent::MessageDelta { usage, .. }) => {
            *output = u64::from(usage.output_tokens);
        }
        _ => {}
    }
}

fn account_codex(data: &str, input: &mut u64, output: &mut u64) {
    let Ok(value) = serde_json::from_str::<serde_json::Value>(data) else {
        return;
    };
    if value.get("type").and_then(serde_json::Value::as_str) != Some("response.completed") {
        return;
    }
    let Some(usage) = value.get("response").and_then(|r| r.get("usage")) else {
        return;
    };
    if let Some(i) = usage
        .get("input_tokens")
        .and_then(serde_json::Value::as_u64)
    {
        *input = i;
    }
    if let Some(o) = usage
        .get("output_tokens")
        .and_then(serde_json::Value::as_u64)
    {
        *output = o;
    }
}

fn record(st: &TapState) {
    if st.input_tokens > 0 {
        metrics::add_tokens(
            st.consumer.as_str(),
            st.protocol,
            "input",
            &st.model,
            st.input_tokens,
        );
    }
    if st.output_tokens > 0 {
        metrics::add_tokens(
            st.consumer.as_str(),
            st.protocol,
            "output",
            &st.model,
            st.output_tokens,
        );
    }
}
