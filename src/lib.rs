//! Submux — subscription-native LLM gateway.
//!
//! See [`docs/architecture.md`](https://github.com/example/submux/blob/main/docs/architecture.md)
//! for the full design. The module tree:
//!
//! - [`core`] — canonical types + traits (no I/O).
//! - [`protocols`] — inbound parsers + outbound emitters (Anthropic, OpenAI).
//! - [`providers`] — outbound adapters (Anthropic OAuth, ChatGPT session).
//! - [`router`] — scheduler, strategies, cooldown, retry, failover.
//! - [`accounts`] — pool, refresh manager, cookie jars, quota, health.
//! - [`streaming`] — SSE parse/emit, tee, translation, checkpoints.
//! - [`telemetry`] — typed event channel, named tracers, metrics.
//! - [`storage`] — account store + secret-at-rest sealing.
//! - [`config`] — figment-based config schema.
//! - [`server`] — axum app + routes + middleware.

pub mod accounts;
pub mod config;
pub mod constants;
pub mod coordination;
pub mod core;
pub mod protocols;
pub mod providers;
pub mod router;
pub mod server;
pub mod storage;
pub mod streaming;
pub mod telemetry;

pub use crate::core::{
    AccountHandle, AccountId, AdapterError, ChallengeKind, ContentBlock, Credentials, Delta,
    FinishReason, Message, NormalizedRequest, NormalizedResponse, ProtocolKind, ProviderAdapter,
    ProviderHealth, ProviderKind, ResponseStream, RlScope, Role, Session, SubmuxError, Tool,
    ToolUse, TransientKind, Usage,
};
