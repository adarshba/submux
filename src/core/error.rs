use bytes::Bytes;
use chrono::{DateTime, Utc};
use std::time::Duration;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum SubmuxError {
    #[error("no healthy account available for model_group `{model_group}`")]
    NoHealthyAccount { model_group: String },

    #[error("account `{account_id}` requires re-authentication")]
    NeedsReauth { account_id: String },

    #[error("retry budget exhausted after {attempts} attempt(s)")]
    RetryExhausted { attempts: u32 },

    #[error("protocol parse error: {0}")]
    ProtocolParse(String),

    #[error("translation error: {0}")]
    Translation(String),

    #[error("storage error: {0}")]
    Storage(String),

    #[error("config error: {0}")]
    Config(String),

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
