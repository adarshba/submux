//! Submux — subscription-native LLM gateway.
//!
//! Proxies Anthropic Claude Max (OAuth) and ChatGPT Codex (subscription) to
//! standard SDK clients. One axum binary, one config file. Modules:
//!
//! - [`core`] — canonical types shared across modules (no I/O).
//! - [`config`] — TOML file + env resolver → [`config::Settings`].
//! - [`accounts`] — account pool, refresh singleflight, cooldown cache.
//! - [`providers`] — outbound adapters (Anthropic OAuth, Codex session).
//! - [`protocols`] — Anthropic ↔ OpenAI request/response translation.
//! - [`streaming`] — SSE parser, emitter, and stream translators.
//! - [`telemetry`] — metrics + tracer (Prometheus exporter + tracing).
//! - [`server`] — axum app, middleware, routes.
//! - [`cli`] — clap entry surface.

pub mod accounts;
pub mod cli;
pub mod config;
pub mod constants;
pub mod core;
pub mod protocols;
pub mod providers;
pub mod server;
pub mod streaming;
pub mod telemetry;

pub use crate::core::{
    AccountHandle, AccountId, AdapterError, ApiKey, ApiKeyError, ChallengeKind, ContentBlock,
    Credentials, Delta, FinishReason, Message, NormalizedRequest, NormalizedResponse, ProtocolKind,
    ProviderKind, ResponseStream, RlScope, Role, Session, SubmuxError, Tool, ToolUse,
    TransientKind, Usage,
};
