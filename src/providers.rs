//! Outbound provider implementations.
//!
//! Each provider owns its own auth scheme, header cloaking, and request
//! shape; routes only know the inbound URL → provider mapping.

pub mod anthropic;
pub mod codex;
