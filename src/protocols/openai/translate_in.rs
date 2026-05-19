//! Translate an inbound OpenAI Chat Completions request into the canonical
//! Anthropic-shaped [`NormalizedRequest`].
//!
//! Mapping rules:
//!   * `system` role messages are lifted out into [`NormalizedRequest::system`].
//!   * `user`/`assistant`/`tool` map onto [`Role`]; `tool` role becomes an
//!     `assistant` containing a [`ContentBlock::ToolResult`] keyed off
//!     `tool_call_id`. (Anthropic always wraps tool_result inside a user
//!     message, not assistant — we follow that convention.)
//!   * string content → single [`ContentBlock::Text`].
//!   * array content → per-part conversion supporting text and image_url.
//!   * assistant `tool_calls` become [`ContentBlock::ToolUse`] blocks following
//!     any text content in the same message.
//!   * `tools` map directly into [`Tool`].
//!   * `passthrough` retains any fields we did not lift, so providers can
//!     forward unknown options verbatim.

use serde_json::{Map, Value};

use crate::core::{
    ContentBlock, ImageSource, Message, NormalizedRequest, ProtocolKind, Role, Tool, ToolUse,
};
use crate::protocols::openai::parse::{
    OpenAiChatRequest, OpenAiFunctionDef, OpenAiMessage, OpenAiTool,
};

/// Translate an OpenAI Chat Completions request into the canonical shape.
pub fn openai_to_normalized(req: OpenAiChatRequest) -> NormalizedRequest {
    let OpenAiChatRequest {
        model,
        messages,
        stream,
        max_tokens,
        temperature,
        top_p,
        tools,
        tool_choice,
    } = req;

    let mut system: Vec<ContentBlock> = Vec::new();
    let mut out_messages: Vec<Message> = Vec::new();

    for msg in messages {
        match msg.role.as_str() {
            "system" => {
                for block in message_content_to_blocks(&msg.content) {
                    system.push(block);
                }
            }
            "user" => {
                let blocks = message_content_to_blocks(&msg.content);
                out_messages.push(Message {
                    role: Role::User,
                    content: blocks,
                });
            }
            "assistant" => {
                let mut blocks = message_content_to_blocks(&msg.content);
                for tc in &msg.tool_calls {
                    if let Some(tu) = openai_tool_call_to_tool_use(tc) {
                        blocks.push(ContentBlock::ToolUse(tu));
                    }
                }
                if !blocks.is_empty() {
                    out_messages.push(Message {
                        role: Role::Assistant,
                        content: blocks,
                    });
                }
            }
            "tool" => {
                let OpenAiMessage {
                    content,
                    tool_call_id,
                    ..
                } = msg;
                let id = tool_call_id.unwrap_or_default();
                let result_content = message_content_to_blocks(&content);
                let block = ContentBlock::ToolResult {
                    tool_use_id: id,
                    content: result_content,
                    is_error: false,
                };
                if let Some(last) = out_messages.last_mut() {
                    if matches!(last.role, Role::User) {
                        last.content.push(block);
                        continue;
                    }
                }
                out_messages.push(Message {
                    role: Role::User,
                    content: vec![block],
                });
            }
            _ => {
                out_messages.push(Message {
                    role: Role::User,
                    content: message_content_to_blocks(&msg.content),
                });
            }
        }
    }

    let normalized_tools: Vec<Tool> = tools.into_iter().map(openai_tool_to_tool).collect();

    let mut pass = Map::new();
    if let Some(tc) = tool_choice {
        pass.insert("tool_choice".to_string(), tc);
    }

    NormalizedRequest {
        protocol: ProtocolKind::OpenAI,
        system,
        messages: out_messages,
        tools: normalized_tools,
        stream,
        model_hint: Some(model),
        max_tokens,
        temperature,
        top_p,
        top_k: None,
        passthrough: Value::Object(pass),
    }
}

fn message_content_to_blocks(content: &Value) -> Vec<ContentBlock> {
    match content {
        Value::Null => Vec::new(),
        Value::String(s) => {
            if s.is_empty() {
                Vec::new()
            } else {
                vec![ContentBlock::Text { text: s.clone() }]
            }
        }
        Value::Array(parts) => parts.iter().filter_map(part_to_block).collect(),
        other => vec![ContentBlock::Text {
            text: other.to_string(),
        }],
    }
}

fn part_to_block(part: &Value) -> Option<ContentBlock> {
    let kind = part.get("type").and_then(|v| v.as_str()).unwrap_or("");
    match kind {
        "text" => {
            let text = part
                .get("text")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            Some(ContentBlock::Text { text })
        }
        "image_url" => {
            let url = match part.get("image_url") {
                Some(Value::String(s)) => Some(s.clone()),
                Some(Value::Object(o)) => o.get("url").and_then(|v| v.as_str()).map(String::from),
                _ => None,
            };
            url.map(|u| ContentBlock::Image {
                source: ImageSource::Url { url: u },
            })
        }
        _ => None,
    }
}

fn openai_tool_call_to_tool_use(call: &Value) -> Option<ToolUse> {
    let id = call.get("id")?.as_str()?.to_string();
    let function = call.get("function")?;
    let name = function.get("name")?.as_str()?.to_string();
    let input: Value = match function.get("arguments") {
        Some(Value::String(s)) => serde_json::from_str(s).unwrap_or(Value::String(s.clone())),
        Some(other) => other.clone(),
        None => Value::Object(Map::new()),
    };
    Some(ToolUse { id, name, input })
}

fn openai_tool_to_tool(t: OpenAiTool) -> Tool {
    let OpenAiFunctionDef {
        name,
        description,
        parameters,
    } = t.function;
    Tool {
        name,
        description,
        input_schema: parameters,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocols::openai::parse::parse_chat_body;

    #[test]
    fn system_message_is_lifted() {
        let body = br#"{"model":"gpt-4o","messages":[
            {"role":"system","content":"be helpful"},
            {"role":"user","content":"hi"}
        ]}"#;
        let req = parse_chat_body(body).unwrap();
        let n = openai_to_normalized(req);
        assert_eq!(n.system.len(), 1);
        assert!(matches!(
            &n.system[0],
            ContentBlock::Text { text } if text == "be helpful"
        ));
        assert_eq!(n.messages.len(), 1);
        assert!(matches!(n.messages[0].role, Role::User));
    }

    #[test]
    fn array_content_with_image_url() {
        let body = br#"{"model":"gpt-4o","messages":[
            {"role":"user","content":[
                {"type":"text","text":"caption this"},
                {"type":"image_url","image_url":{"url":"https://x/a.png"}}
            ]}
        ]}"#;
        let req = parse_chat_body(body).unwrap();
        let n = openai_to_normalized(req);
        assert_eq!(n.messages.len(), 1);
        let blocks = &n.messages[0].content;
        assert_eq!(blocks.len(), 2);
        assert!(matches!(&blocks[0], ContentBlock::Text { text } if text == "caption this"));
        assert!(matches!(
            &blocks[1],
            ContentBlock::Image { source: ImageSource::Url { url } } if url == "https://x/a.png"
        ));
    }

    #[test]
    fn assistant_tool_calls_become_tool_use_blocks() {
        let body = br#"{"model":"gpt-4o","messages":[
            {"role":"assistant","content":null,"tool_calls":[
                {"id":"call_1","type":"function","function":{"name":"get_weather","arguments":"{\"city\":\"SF\"}"}}
            ]}
        ]}"#;
        let req = parse_chat_body(body).unwrap();
        let n = openai_to_normalized(req);
        assert_eq!(n.messages.len(), 1);
        assert!(matches!(n.messages[0].role, Role::Assistant));
        let block = &n.messages[0].content[0];
        match block {
            ContentBlock::ToolUse(tu) => {
                assert_eq!(tu.id, "call_1");
                assert_eq!(tu.name, "get_weather");
                assert_eq!(tu.input["city"], "SF");
            }
            _ => panic!("expected ToolUse"),
        }
    }

    #[test]
    fn tool_role_becomes_user_tool_result() {
        let body = br#"{"model":"gpt-4o","messages":[
            {"role":"user","content":"please"},
            {"role":"assistant","content":null,"tool_calls":[
                {"id":"call_1","type":"function","function":{"name":"f","arguments":"{}"}}
            ]},
            {"role":"tool","tool_call_id":"call_1","content":"42"}
        ]}"#;
        let req = parse_chat_body(body).unwrap();
        let n = openai_to_normalized(req);
        assert_eq!(n.messages.len(), 3);
        assert!(matches!(n.messages[2].role, Role::User));
        match &n.messages[2].content[0] {
            ContentBlock::ToolResult {
                tool_use_id,
                content,
                is_error,
            } => {
                assert_eq!(tool_use_id, "call_1");
                assert!(!is_error);
                assert!(
                    matches!(&content[0], ContentBlock::Text { text } if text == "42"),
                    "got {:?}",
                    content
                );
            }
            _ => panic!("expected ToolResult"),
        }
    }

    #[test]
    fn tools_definitions_round_trip() {
        let body = br#"{"model":"gpt-4o","messages":[],"tools":[
            {"type":"function","function":{"name":"f","description":"d","parameters":{"type":"object","properties":{}}}}
        ]}"#;
        let req = parse_chat_body(body).unwrap();
        let n = openai_to_normalized(req);
        assert_eq!(n.tools.len(), 1);
        assert_eq!(n.tools[0].name, "f");
        assert_eq!(n.tools[0].description.as_deref(), Some("d"));
        assert_eq!(n.tools[0].input_schema["type"], "object");
    }

    #[test]
    fn stream_and_sampling_params_propagate() {
        let body = br#"{"model":"gpt-4o","messages":[],"stream":true,"max_tokens":256,"temperature":0.4,"top_p":0.9}"#;
        let req = parse_chat_body(body).unwrap();
        let n = openai_to_normalized(req);
        assert!(n.stream);
        assert_eq!(n.max_tokens, Some(256));
        assert_eq!(n.temperature, Some(0.4));
        assert_eq!(n.top_p, Some(0.9));
        assert_eq!(n.model_hint.as_deref(), Some("gpt-4o"));
        assert_eq!(n.protocol, ProtocolKind::OpenAI);
    }

    #[test]
    fn tool_choice_lands_in_passthrough() {
        let body = br#"{"model":"gpt-4o","messages":[],"tool_choice":"auto"}"#;
        let req = parse_chat_body(body).unwrap();
        let n = openai_to_normalized(req);
        assert_eq!(n.passthrough["tool_choice"], "auto");
    }
}
