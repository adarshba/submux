//! Translate an Anthropic Messages request body into an OpenAI Responses API
//! request body.
//!
//! Mapping contract:
//! - `messages` flattens into the Responses `input` array. Each text/image
//!   message becomes one `message` item; `tool_use` blocks are hoisted to
//!   top-level `function_call` siblings and `tool_result` blocks to
//!   `function_call_output` siblings, because Responses does not nest tool
//!   calls inside messages.
//! - `system` (string or array of text blocks) joins into `instructions`.
//! - `tools` map onto the Responses `function` shape with `strict: false`.
//! - `model` is replaced with `DEFAULT_CODEX_MODEL` unless it already looks
//!   like a Codex model id.
//! - `stream` is forced to `true`; Codex only speaks SSE.
//!
//! TODO: image blocks → `input_image`, assistant `thinking` → `reasoning`,
//! `temperature` / `top_p` passthrough.

use bytes::Bytes;
use serde_json::{json, Map, Value};

/// Default Codex model when the client doesn't supply one or supplies an
/// Anthropic model id. Matches the latest Codex CLI default.
pub const DEFAULT_CODEX_MODEL: &str = "gpt-5-codex";

#[derive(Debug, thiserror::Error)]
pub enum TranslateError {
    #[error("body is not valid JSON: {0}")]
    Json(#[from] serde_json::Error),
    #[error("body is not a JSON object")]
    NotObject,
    #[error("messages field is missing or not an array")]
    BadMessages,
}

/// Translate raw Anthropic Messages request bytes into a Codex Responses
/// JSON body, ready to POST to `chatgpt.com/backend-api/codex/responses`.
pub fn anthropic_to_responses_body(body: &[u8]) -> Result<Bytes, TranslateError> {
    let value: Value = serde_json::from_slice(body)?;
    let obj = value.as_object().ok_or(TranslateError::NotObject)?;

    let messages = obj
        .get("messages")
        .and_then(Value::as_array)
        .ok_or(TranslateError::BadMessages)?;

    let mut input: Vec<Value> = Vec::with_capacity(messages.len());
    for msg in messages {
        translate_message_into(msg, &mut input);
    }

    let mut out = Map::new();
    out.insert("model".into(), Value::String(pick_model(obj)));
    out.insert("input".into(), Value::Array(input));
    if let Some(instructions) = translate_system(obj.get("system")) {
        out.insert("instructions".into(), Value::String(instructions));
    }
    if let Some(tools) = translate_tools(obj.get("tools")) {
        out.insert("tools".into(), Value::Array(tools));
    }
    out.insert("stream".into(), Value::Bool(true));
    out.insert("store".into(), Value::Bool(false));
    out.insert("parallel_tool_calls".into(), Value::Bool(false));

    let bytes = serde_json::to_vec(&Value::Object(out))?;
    Ok(Bytes::from(bytes))
}

fn pick_model(obj: &Map<String, Value>) -> String {
    let from_client = obj.get("model").and_then(Value::as_str).unwrap_or("");
    if is_codex_model(from_client) {
        from_client.to_owned()
    } else {
        DEFAULT_CODEX_MODEL.to_owned()
    }
}

fn is_codex_model(model: &str) -> bool {
    let m = model.to_ascii_lowercase();
    m.starts_with("gpt-") || m.starts_with("o1") || m.starts_with("o3") || m.starts_with("codex")
}

fn translate_system(system: Option<&Value>) -> Option<String> {
    match system? {
        Value::String(s) if !s.is_empty() => Some(s.clone()),
        Value::Array(arr) => {
            let mut parts = Vec::new();
            for block in arr {
                if let Some(text) = block
                    .get("type")
                    .and_then(Value::as_str)
                    .filter(|t| *t == "text")
                    .and(block.get("text").and_then(Value::as_str))
                {
                    parts.push(text.to_owned());
                }
            }
            if parts.is_empty() {
                None
            } else {
                Some(parts.join("\n\n"))
            }
        }
        _ => None,
    }
}

fn translate_tools(tools: Option<&Value>) -> Option<Vec<Value>> {
    let arr = tools?.as_array()?;
    let mut out = Vec::with_capacity(arr.len());
    for tool in arr {
        let Some(name) = tool.get("name").and_then(Value::as_str) else {
            continue;
        };
        let mut entry = Map::new();
        entry.insert("type".into(), Value::String("function".into()));
        entry.insert("name".into(), Value::String(name.to_owned()));
        if let Some(desc) = tool.get("description").and_then(Value::as_str) {
            entry.insert("description".into(), Value::String(desc.to_owned()));
        }
        if let Some(schema) = tool.get("input_schema") {
            entry.insert("parameters".into(), schema.clone());
        }
        entry.insert("strict".into(), Value::Bool(false));
        out.push(Value::Object(entry));
    }
    if out.is_empty() {
        None
    } else {
        Some(out)
    }
}

fn translate_message_into(msg: &Value, out: &mut Vec<Value>) {
    let role = msg.get("role").and_then(Value::as_str).unwrap_or("user");
    let content = match msg.get("content") {
        Some(Value::String(s)) => {
            out.push(simple_message_item(role, s));
            return;
        }
        Some(Value::Array(blocks)) => blocks,
        _ => return,
    };

    let mut message_content: Vec<Value> = Vec::new();
    for block in content {
        let kind = block.get("type").and_then(Value::as_str).unwrap_or("");
        match kind {
            "text" => {
                let text = block.get("text").and_then(Value::as_str).unwrap_or("");
                message_content.push(json!({
                    "type": content_text_type(role),
                    "text": text,
                }));
            }
            "tool_use" => {
                if !message_content.is_empty() {
                    out.push(make_message_item(
                        role,
                        std::mem::take(&mut message_content),
                    ));
                }
                out.push(make_function_call(block));
            }
            "tool_result" => {
                if !message_content.is_empty() {
                    out.push(make_message_item(
                        role,
                        std::mem::take(&mut message_content),
                    ));
                }
                out.push(make_function_call_output(block));
            }
            // TODO: images / documents / thinking blocks.
            _ => {}
        }
    }
    if !message_content.is_empty() {
        out.push(make_message_item(role, message_content));
    }
}

fn simple_message_item(role: &str, text: &str) -> Value {
    json!({
        "type": "message",
        "role": role,
        "content": [{
            "type": content_text_type(role),
            "text": text,
        }],
    })
}

fn make_message_item(role: &str, content: Vec<Value>) -> Value {
    json!({
        "type": "message",
        "role": role,
        "content": content,
    })
}

fn content_text_type(role: &str) -> &'static str {
    match role {
        "assistant" => "output_text",
        _ => "input_text",
    }
}

fn make_function_call(block: &Value) -> Value {
    let id = block
        .get("id")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_owned();
    let name = block
        .get("name")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_owned();
    let arguments = block
        .get("input")
        .map(|v| serde_json::to_string(v).unwrap_or_else(|_| "{}".into()))
        .unwrap_or_else(|| "{}".into());
    json!({
        "type": "function_call",
        "call_id": id,
        "name": name,
        "arguments": arguments,
    })
}

fn make_function_call_output(block: &Value) -> Value {
    let call_id = block
        .get("tool_use_id")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_owned();
    let output = match block.get("content") {
        Some(Value::String(s)) => s.clone(),
        Some(other) => serde_json::to_string(other).unwrap_or_default(),
        None => String::new(),
    };
    json!({
        "type": "function_call_output",
        "call_id": call_id,
        "output": output,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn translate(body: &str) -> Value {
        let bytes = anthropic_to_responses_body(body.as_bytes()).expect("translate ok");
        serde_json::from_slice(&bytes).unwrap()
    }

    #[test]
    fn simple_string_user_message() {
        let v = translate(
            r#"{"model":"claude-sonnet-4.5","messages":[{"role":"user","content":"hi"}]}"#,
        );
        assert_eq!(v["model"], DEFAULT_CODEX_MODEL);
        assert_eq!(v["stream"], true);
        assert_eq!(v["input"][0]["type"], "message");
        assert_eq!(v["input"][0]["role"], "user");
        assert_eq!(v["input"][0]["content"][0]["type"], "input_text");
        assert_eq!(v["input"][0]["content"][0]["text"], "hi");
    }

    #[test]
    fn preserves_codex_model_id() {
        let v = translate(r#"{"model":"gpt-5-codex","messages":[{"role":"user","content":"x"}]}"#);
        assert_eq!(v["model"], "gpt-5-codex");
    }

    #[test]
    fn system_string_becomes_instructions() {
        let v = translate(
            r#"{"model":"x","system":"be helpful","messages":[{"role":"user","content":"hi"}]}"#,
        );
        assert_eq!(v["instructions"], "be helpful");
    }

    #[test]
    fn system_array_joins_text_blocks() {
        let body = r#"{"model":"x","system":[{"type":"text","text":"A"},{"type":"text","text":"B"}],"messages":[{"role":"user","content":"hi"}]}"#;
        let v = translate(body);
        assert_eq!(v["instructions"], "A\n\nB");
    }

    #[test]
    fn assistant_text_uses_output_text_type() {
        let body = r#"{"model":"x","messages":[{"role":"assistant","content":[{"type":"text","text":"hi"}]}]}"#;
        let v = translate(body);
        assert_eq!(v["input"][0]["content"][0]["type"], "output_text");
    }

    #[test]
    fn assistant_tool_use_becomes_function_call_item() {
        let body = r#"{
            "model":"x",
            "messages":[{"role":"assistant","content":[
                {"type":"text","text":"calling"},
                {"type":"tool_use","id":"toolu_1","name":"get_weather","input":{"city":"SF"}}
            ]}]
        }"#;
        let v = translate(body);
        let input = v["input"].as_array().unwrap();
        assert_eq!(input.len(), 2);
        assert_eq!(input[0]["type"], "message");
        assert_eq!(input[1]["type"], "function_call");
        assert_eq!(input[1]["call_id"], "toolu_1");
        assert_eq!(input[1]["name"], "get_weather");
        assert_eq!(input[1]["arguments"], r#"{"city":"SF"}"#);
    }

    #[test]
    fn user_tool_result_becomes_function_call_output() {
        let body = r#"{
            "model":"x",
            "messages":[{"role":"user","content":[
                {"type":"tool_result","tool_use_id":"toolu_1","content":"sunny"}
            ]}]
        }"#;
        let v = translate(body);
        let item = &v["input"][0];
        assert_eq!(item["type"], "function_call_output");
        assert_eq!(item["call_id"], "toolu_1");
        assert_eq!(item["output"], "sunny");
    }

    #[test]
    fn tools_get_responses_shape() {
        let body = r#"{
            "model":"x",
            "messages":[{"role":"user","content":"hi"}],
            "tools":[{"name":"f","description":"d","input_schema":{"type":"object","properties":{}}}]
        }"#;
        let v = translate(body);
        let tool = &v["tools"][0];
        assert_eq!(tool["type"], "function");
        assert_eq!(tool["name"], "f");
        assert_eq!(tool["description"], "d");
        assert_eq!(tool["parameters"]["type"], "object");
        assert_eq!(tool["strict"], false);
    }
}
