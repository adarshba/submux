use crate::core::{NormalizedRequest, ProtocolKind, SubmuxError};
use serde_json::Value;

pub fn parse_body(_body: &[u8]) -> Result<NormalizedRequest, SubmuxError> {
    Err(SubmuxError::ProtocolParse(
        "anthropic parser not yet implemented".into(),
    ))
}

pub fn peek_model(body: &[u8]) -> Result<Option<String>, SubmuxError> {
    let v: Value =
        serde_json::from_slice(body).map_err(|e| SubmuxError::ProtocolParse(e.to_string()))?;
    Ok(v.get("model")
        .and_then(|m| m.as_str())
        .map(|s| s.to_owned()))
}

pub const PROTOCOL: ProtocolKind = ProtocolKind::Anthropic;
