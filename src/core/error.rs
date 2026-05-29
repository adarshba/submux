use bytes::Bytes;
use chrono::{DateTime, Utc};
use std::time::Duration;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum SubmuxError {
    #[error("protocol parse error: {0}")]
    ProtocolParse(String),

    #[error("internal error: {0}")]
    Internal(String),
}

#[derive(Debug, Error)]
pub enum AdapterError {
    #[error("token invalid for account (needs_reauth={needs_reauth})")]
    TokenInvalid { needs_reauth: bool },

    #[error("rate limited (retry_after={retry_after:?}, scope={scope:?})")]
    RateLimited {
        retry_after: Option<Duration>,
        scope: RlScope,
    },

    #[error("subscription quota exhausted until {reset_at}")]
    QuotaExhausted { reset_at: DateTime<Utc> },

    #[error("interactive challenge required: {kind:?}")]
    InteractiveChallenge { kind: ChallengeKind },

    #[error("context window exceeded: used {tokens_used}, limit {limit}")]
    ContextWindowExceeded { tokens_used: u32, limit: u32 },

    #[error("content policy violation: {provider_code}")]
    ContentPolicy { provider_code: String },

    #[error("bad request: {provider_msg}")]
    BadRequest { provider_msg: String },

    #[error("transient error: {cause:?}")]
    Transient { cause: TransientKind },

    #[error("upstream error: status={status}")]
    Upstream { status: u16, body: Bytes },

    #[error("internal: {0}")]
    Internal(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RlScope {
    Account,
    Model,
    Org,
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChallengeKind {
    Captcha,
    HumanVerify,
    EmailVerify,
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TransientKind {
    Connect,
    ReadTimeout,
    WriteTimeout,
    Dns,
    Tls,
    Reset,
    Other,
}
