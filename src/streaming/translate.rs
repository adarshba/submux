//! Streaming protocol translation as a typed state machine.
//!
//! The translator consumes Anthropic Messages SSE events and emits OpenAI Chat
//! Completions chunks. State is intentionally tiny — we only need:
//!   * the chat completion id (assigned at `MessageStart`),
//!   * the model name (echoed back in every chunk),
//!   * which Anthropic content block indices map to which OpenAI tool-call
//!     indices (Anthropic indices include the text block; OpenAI's
//!     `tool_calls[*].index` is a separate counter starting at 0).
//!
//! The translator is intentionally **not** an SSE codec. The caller is
//! responsible for parsing inbound frames and emitting outbound frames
//! (including the OpenAI `[DONE]` sentinel).

use std::collections::HashMap;

use chrono::Utc;
use ulid::Ulid;

use crate::streaming::anthropic_events::{AnthropicBlockStart, AnthropicDelta, AnthropicEvent};
use crate::streaming::openai_chunks::{
    OpenAiChatChunk, OpenAiChoice, OpenAiDelta, OpenAiFunctionDelta, OpenAiToolCallDelta,
    chat_completion_chunk,
};

#[derive(Debug)]
pub struct AnthropicToOpenAiTranslator {
    /// OpenAI chunk id — generated once at `MessageStart`.
    request_id: String,
    /// Unix seconds, captured at construction.
    created: i64,
    /// Model name. Populated from `MessageStart`; may be overridden by the
    /// constructor when the caller already knows the user-facing model.
    model: String,
    model_override: Option<String>,
    /// Bookkeeping for currently-open text block (we don't need much; we
    /// just track presence so finalize() knows if anything was streamed).
    open_text_block_idx: Option<u32>,
    /// Map Anthropic content_block index -> OpenAI tool-call state.
    tool_call_state: HashMap<u32, ToolCallState>,
    /// Monotonic OpenAI tool_calls index counter.
    tool_call_index_counter: u32,
    /// True once we have emitted a chunk with `finish_reason != None`.
    finished: bool,
}

#[derive(Debug)]
struct ToolCallState {
    openai_index: u32,
    #[allow(dead_code)]
    id: String,
    #[allow(dead_code)]
    name_emitted: bool,
}

impl AnthropicToOpenAiTranslator {
    pub fn new(model_override: Option<String>) -> Self {
        let id = format!("chatcmpl-{}", Ulid::new());
        Self {
            request_id: id,
            created: Utc::now().timestamp(),
            model: model_override.clone().unwrap_or_default(),
            model_override,
            open_text_block_idx: None,
            tool_call_state: HashMap::new(),
            tool_call_index_counter: 0,
            finished: false,
        }
    }

    /// Build a base chunk skeleton with the current id/model/created.
    fn base_chunk(&self) -> OpenAiChatChunk {
        OpenAiChatChunk {
            id: self.request_id.clone(),
            object: chat_completion_chunk(),
            created: self.created,
            model: self.model.clone(),
            choices: vec![],
        }
    }

    fn role_chunk(&self) -> OpenAiChatChunk {
        let mut chunk = self.base_chunk();
        chunk.choices.push(OpenAiChoice {
            index: 0,
            delta: OpenAiDelta {
                role: Some("assistant".to_string()),
                ..Default::default()
            },
            finish_reason: None,
        });
        chunk
    }

    fn content_chunk(&self, text: String) -> OpenAiChatChunk {
        let mut chunk = self.base_chunk();
        chunk.choices.push(OpenAiChoice {
            index: 0,
            delta: OpenAiDelta {
                content: Some(text),
                ..Default::default()
            },
            finish_reason: None,
        });
        chunk
    }

    fn finish_chunk(&self, reason: &str) -> OpenAiChatChunk {
        let mut chunk = self.base_chunk();
        chunk.choices.push(OpenAiChoice {
            index: 0,
            delta: OpenAiDelta::default(),
            finish_reason: Some(reason.to_string()),
        });
        chunk
    }

    /// Ingest one Anthropic event, return zero or more OpenAI chunks to forward.
    pub fn ingest(&mut self, event: AnthropicEvent) -> Vec<OpenAiChatChunk> {
        if self.finished {
            if let AnthropicEvent::Error { error } = event {
                tracing::warn!(
                    error.kind = %error.kind,
                    error.message = %error.message,
                    "anthropic error after stream finished"
                );
            }
            return Vec::new();
        }

        match event {
            AnthropicEvent::MessageStart { message } => {
                if self.model_override.is_none() {
                    self.model = message.model;
                }
                vec![self.role_chunk()]
            }
            AnthropicEvent::ContentBlockStart {
                index,
                content_block,
            } => match content_block {
                AnthropicBlockStart::Text { .. } => {
                    self.open_text_block_idx = Some(index);
                    Vec::new()
                }
                AnthropicBlockStart::ToolUse { id, name, .. } => {
                    let openai_index = self.tool_call_index_counter;
                    self.tool_call_index_counter += 1;
                    self.tool_call_state.insert(
                        index,
                        ToolCallState {
                            openai_index,
                            id: id.clone(),
                            name_emitted: true,
                        },
                    );
                    let mut chunk = self.base_chunk();
                    chunk.choices.push(OpenAiChoice {
                        index: 0,
                        delta: OpenAiDelta {
                            tool_calls: vec![OpenAiToolCallDelta {
                                index: openai_index,
                                id: Some(id),
                                kind: "function".to_string(),
                                function: OpenAiFunctionDelta {
                                    name: Some(name),
                                    arguments: None,
                                },
                            }],
                            ..Default::default()
                        },
                        finish_reason: None,
                    });
                    vec![chunk]
                }
                AnthropicBlockStart::Thinking { .. } => Vec::new(),
            },
            AnthropicEvent::ContentBlockDelta { index, delta } => match delta {
                AnthropicDelta::TextDelta { text } => vec![self.content_chunk(text)],
                AnthropicDelta::InputJsonDelta { partial_json } => {
                    let Some(state) = self.tool_call_state.get(&index) else {
                        tracing::warn!(
                            anthropic_index = index,
                            "input_json_delta without matching tool_use block"
                        );
                        return Vec::new();
                    };
                    let openai_index = state.openai_index;
                    let mut chunk = self.base_chunk();
                    chunk.choices.push(OpenAiChoice {
                        index: 0,
                        delta: OpenAiDelta {
                            tool_calls: vec![OpenAiToolCallDelta {
                                index: openai_index,
                                id: None,
                                kind: "function".to_string(),
                                function: OpenAiFunctionDelta {
                                    name: None,
                                    arguments: Some(partial_json),
                                },
                            }],
                            ..Default::default()
                        },
                        finish_reason: None,
                    });
                    vec![chunk]
                }
                AnthropicDelta::ThinkingDelta { .. } | AnthropicDelta::SignatureDelta { .. } => {
                    Vec::new()
                }
            },
            AnthropicEvent::ContentBlockStop { index } => {
                if self.open_text_block_idx == Some(index) {
                    self.open_text_block_idx = None;
                }
                Vec::new()
            }
            AnthropicEvent::MessageDelta { delta, .. } => {
                let reason = map_stop_reason(delta.stop_reason.as_deref());
                self.finished = true;
                vec![self.finish_chunk(reason)]
            }
            AnthropicEvent::MessageStop => {
                if self.finished {
                    Vec::new()
                } else {
                    self.finished = true;
                    vec![self.finish_chunk("stop")]
                }
            }
            AnthropicEvent::Ping => Vec::new(),
            AnthropicEvent::Error { error } => {
                tracing::warn!(
                    error.kind = %error.kind,
                    error.message = %error.message,
                    "anthropic error in stream — emitting truncation"
                );
                self.finished = true;
                vec![self.finish_chunk("stop")]
            }
        }
    }

    /// Called when the upstream stream ends without `MessageStop`. Idempotent
    /// with respect to a stream that already finished cleanly.
    pub fn finalize(&mut self) -> Vec<OpenAiChatChunk> {
        if self.finished {
            Vec::new()
        } else {
            self.finished = true;
            vec![self.finish_chunk("stop")]
        }
    }

    /// Inspect the chat completion id (useful for logging/tee).
    pub fn request_id(&self) -> &str {
        &self.request_id
    }
}

/// Map Anthropic stop reasons to OpenAI finish reasons.
fn map_stop_reason(reason: Option<&str>) -> &'static str {
    match reason {
        Some("end_turn") => "stop",
        Some("max_tokens") => "length",
        Some("tool_use") => "tool_calls",
        Some("stop_sequence") => "stop",
        _ => "stop",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::streaming::anthropic_events::{
        AnthropicMessageDelta, AnthropicMessageMeta, AnthropicUsage,
    };

    fn msg_start(model: &str) -> AnthropicEvent {
        AnthropicEvent::MessageStart {
            message: AnthropicMessageMeta {
                id: "msg_1".to_string(),
                model: model.to_string(),
                role: "assistant".to_string(),
                usage: AnthropicUsage::default(),
            },
        }
    }

    #[test]
    fn text_stream_flow() {
        let mut t = AnthropicToOpenAiTranslator::new(None);
        let mut out: Vec<OpenAiChatChunk> = Vec::new();

        out.extend(t.ingest(msg_start("claude-3-5-sonnet")));
        out.extend(t.ingest(AnthropicEvent::ContentBlockStart {
            index: 0,
            content_block: AnthropicBlockStart::Text {
                text: String::new(),
            },
        }));
        for piece in ["Hel", "lo,", " world"] {
            out.extend(t.ingest(AnthropicEvent::ContentBlockDelta {
                index: 0,
                delta: AnthropicDelta::TextDelta {
                    text: piece.to_string(),
                },
            }));
        }
        out.extend(t.ingest(AnthropicEvent::ContentBlockStop { index: 0 }));
        out.extend(t.ingest(AnthropicEvent::MessageDelta {
            delta: AnthropicMessageDelta {
                stop_reason: Some("end_turn".into()),
                stop_sequence: None,
            },
            usage: AnthropicUsage::default(),
        }));
        out.extend(t.ingest(AnthropicEvent::MessageStop));

        assert_eq!(out.len(), 5, "got {:?}", out);
        assert_eq!(out[0].choices[0].delta.role.as_deref(), Some("assistant"));
        assert!(out[0].choices[0].delta.content.is_none());
        assert!(out[0].choices[0].finish_reason.is_none());
        assert_eq!(out[0].model, "claude-3-5-sonnet");
        assert_eq!(out[1].choices[0].delta.content.as_deref(), Some("Hel"));
        assert_eq!(out[2].choices[0].delta.content.as_deref(), Some("lo,"));
        assert_eq!(out[3].choices[0].delta.content.as_deref(), Some(" world"));
        assert_eq!(out[4].choices[0].finish_reason.as_deref(), Some("stop"));
        let id = &out[0].id;
        assert!(id.starts_with("chatcmpl-"));
        for c in &out {
            assert_eq!(&c.id, id);
            assert_eq!(c.created, out[0].created);
            assert_eq!(c.object, "chat.completion.chunk");
        }
    }

    #[test]
    fn tool_use_stream_flow() {
        let mut t = AnthropicToOpenAiTranslator::new(Some("gpt-4o".into()));
        let mut out: Vec<OpenAiChatChunk> = Vec::new();

        out.extend(t.ingest(msg_start("claude-3-5-sonnet")));
        out.extend(t.ingest(AnthropicEvent::ContentBlockStart {
            index: 0,
            content_block: AnthropicBlockStart::ToolUse {
                id: "toolu_1".into(),
                name: "get_weather".into(),
                input: serde_json::json!({}),
            },
        }));
        out.extend(t.ingest(AnthropicEvent::ContentBlockDelta {
            index: 0,
            delta: AnthropicDelta::InputJsonDelta {
                partial_json: "{\"city\":\"".into(),
            },
        }));
        out.extend(t.ingest(AnthropicEvent::ContentBlockDelta {
            index: 0,
            delta: AnthropicDelta::InputJsonDelta {
                partial_json: "SF\"}".into(),
            },
        }));
        out.extend(t.ingest(AnthropicEvent::ContentBlockStop { index: 0 }));
        out.extend(t.ingest(AnthropicEvent::MessageDelta {
            delta: AnthropicMessageDelta {
                stop_reason: Some("tool_use".into()),
                stop_sequence: None,
            },
            usage: AnthropicUsage::default(),
        }));
        out.extend(t.ingest(AnthropicEvent::MessageStop));

        assert_eq!(out.len(), 5, "got {:?}", out);
        assert_eq!(out[0].model, "gpt-4o");
        let tc = &out[1].choices[0].delta.tool_calls;
        assert_eq!(tc.len(), 1);
        assert_eq!(tc[0].index, 0);
        assert_eq!(tc[0].id.as_deref(), Some("toolu_1"));
        assert_eq!(tc[0].kind, "function");
        assert_eq!(tc[0].function.name.as_deref(), Some("get_weather"));
        assert!(tc[0].function.arguments.is_none());
        let args: String = out[2..4]
            .iter()
            .flat_map(|c| c.choices[0].delta.tool_calls.iter())
            .filter_map(|tc| tc.function.arguments.clone())
            .collect();
        assert_eq!(args, "{\"city\":\"SF\"}");
        for c in &out[2..4] {
            let tc = &c.choices[0].delta.tool_calls[0];
            assert!(tc.id.is_none());
            assert!(tc.function.name.is_none());
        }
        assert_eq!(
            out[4].choices[0].finish_reason.as_deref(),
            Some("tool_calls")
        );
    }

    #[test]
    fn finalize_emits_stop_when_stream_truncated() {
        let mut t = AnthropicToOpenAiTranslator::new(None);
        let _ = t.ingest(msg_start("claude-3-5-sonnet"));
        let _ = t.ingest(AnthropicEvent::ContentBlockStart {
            index: 0,
            content_block: AnthropicBlockStart::Text {
                text: String::new(),
            },
        });
        let _ = t.ingest(AnthropicEvent::ContentBlockDelta {
            index: 0,
            delta: AnthropicDelta::TextDelta { text: "hi".into() },
        });
        let final_chunks = t.finalize();
        assert_eq!(final_chunks.len(), 1);
        assert_eq!(
            final_chunks[0].choices[0].finish_reason.as_deref(),
            Some("stop")
        );
        assert!(t.finalize().is_empty());
    }

    #[test]
    fn message_stop_after_message_delta_is_noop() {
        let mut t = AnthropicToOpenAiTranslator::new(None);
        let _ = t.ingest(msg_start("m"));
        let one = t.ingest(AnthropicEvent::MessageDelta {
            delta: AnthropicMessageDelta {
                stop_reason: Some("end_turn".into()),
                stop_sequence: None,
            },
            usage: AnthropicUsage::default(),
        });
        assert_eq!(one.len(), 1);
        assert_eq!(one[0].choices[0].finish_reason.as_deref(), Some("stop"));
        let two = t.ingest(AnthropicEvent::MessageStop);
        assert!(two.is_empty());
    }

    #[test]
    fn ping_is_silent() {
        let mut t = AnthropicToOpenAiTranslator::new(None);
        let _ = t.ingest(msg_start("m"));
        assert!(t.ingest(AnthropicEvent::Ping).is_empty());
    }

    #[test]
    fn thinking_is_dropped() {
        let mut t = AnthropicToOpenAiTranslator::new(None);
        let _ = t.ingest(msg_start("m"));
        assert!(
            t.ingest(AnthropicEvent::ContentBlockStart {
                index: 1,
                content_block: AnthropicBlockStart::Thinking {
                    thinking: "internal".into()
                }
            })
            .is_empty()
        );
        assert!(
            t.ingest(AnthropicEvent::ContentBlockDelta {
                index: 1,
                delta: AnthropicDelta::ThinkingDelta {
                    thinking: "more".into()
                }
            })
            .is_empty()
        );
        assert!(
            t.ingest(AnthropicEvent::ContentBlockDelta {
                index: 1,
                delta: AnthropicDelta::SignatureDelta {
                    signature: "sig".into()
                }
            })
            .is_empty()
        );
    }

    #[test]
    fn stop_reason_mapping() {
        assert_eq!(map_stop_reason(Some("end_turn")), "stop");
        assert_eq!(map_stop_reason(Some("max_tokens")), "length");
        assert_eq!(map_stop_reason(Some("tool_use")), "tool_calls");
        assert_eq!(map_stop_reason(Some("stop_sequence")), "stop");
        assert_eq!(map_stop_reason(Some("weird_thing")), "stop");
        assert_eq!(map_stop_reason(None), "stop");
    }
}
