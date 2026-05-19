//! OAuth body cloaking.
//!
//! Anthropic OAuth tokens are issued for Claude Code; sending a raw Messages
//! body without Claude Code's billing-invariant system preamble trips the
//! OAuth body inspector. The cloak block is appended to the last `system`
//! entry (or pushed as a new last entry if absent) — prepending would
//! invalidate the prompt cache on every request.

use serde_json::{json, Value};

const CLOAK_TEXT: &str = "You are Claude Code, Anthropic's official CLI for Claude.";

/// Append the Claude Code system preamble to the request body. Idempotent —
/// the function detects an existing cloak block and skips it.
pub fn apply_body_cloak(body: &mut Value) {
    let Some(obj) = body.as_object_mut() else {
        return;
    };

    let cloak_block = json!({
        "type": "text",
        "text": CLOAK_TEXT,
    });

    match obj.get_mut("system") {
        Some(Value::String(s)) => {
            let preserved = json!({ "type": "text", "text": s.clone() });
            obj.insert("system".into(), Value::Array(vec![preserved, cloak_block]));
        }
        Some(Value::Array(arr)) => {
            if !arr.iter().any(is_cloak_block) {
                arr.push(cloak_block);
            }
        }
        _ => {
            obj.insert("system".into(), Value::Array(vec![cloak_block]));
        }
    }
}

fn is_cloak_block(v: &Value) -> bool {
    v.get("type").and_then(Value::as_str) == Some("text")
        && v.get("text").and_then(Value::as_str) == Some(CLOAK_TEXT)
}

/// Cloak a JSON body — accepts raw bytes, returns re-serialized bytes.
/// Returns `None` when the body isn't valid JSON; callers should forward the
/// bytes untouched in that case.
pub fn cloak_bytes(body: &[u8]) -> Option<Vec<u8>> {
    let mut value: Value = serde_json::from_slice(body).ok()?;
    apply_body_cloak(&mut value);
    serde_json::to_vec(&value).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cloak_appends_to_string_system() {
        let mut body = json!({
            "system": "You are a helpful coding assistant.",
            "messages": [{"role":"user","content":"hi"}],
        });
        apply_body_cloak(&mut body);
        let system = body["system"].as_array().expect("system became array");
        assert_eq!(system.len(), 2);
        assert_eq!(system[1]["text"], CLOAK_TEXT);
    }

    #[test]
    fn cloak_appends_to_array_system() {
        let mut body = json!({
            "system": [{"type":"text","text":"original"}],
            "messages": [],
        });
        apply_body_cloak(&mut body);
        let arr = body["system"].as_array().unwrap();
        assert_eq!(arr.len(), 2);
        assert_eq!(arr[1]["text"], CLOAK_TEXT);
    }

    #[test]
    fn cloak_inserts_when_missing() {
        let mut body = json!({ "messages": [] });
        apply_body_cloak(&mut body);
        let arr = body["system"].as_array().unwrap();
        assert_eq!(arr.len(), 1);
        assert_eq!(arr[0]["text"], CLOAK_TEXT);
    }

    #[test]
    fn cloak_is_idempotent_on_array() {
        let mut body = json!({"messages": []});
        apply_body_cloak(&mut body);
        apply_body_cloak(&mut body);
        let arr = body["system"].as_array().unwrap();
        assert_eq!(arr.len(), 1, "second cloak should not duplicate the block");
    }

    #[test]
    fn cloak_bytes_roundtrip() {
        let original = br#"{"messages":[]}"#;
        let cloaked = cloak_bytes(original).expect("valid json");
        let parsed: Value = serde_json::from_slice(&cloaked).unwrap();
        assert_eq!(parsed["system"][0]["text"].as_str().unwrap(), CLOAK_TEXT);
    }
}
