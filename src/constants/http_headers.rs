//! HTTP header constants shared across adapters and middleware.

use http::HeaderName;

pub const X_REQUEST_ID: HeaderName = HeaderName::from_static("x-request-id");

/// Alternate carrier for the inbound API key. Equivalent to
/// `Authorization: Bearer <key>`; checked by the inbound auth middleware.
pub const X_API_KEY: HeaderName = HeaderName::from_static("x-api-key");

/// Inbound consumer identity (`usr_<id>`) used for per-consumer metric labels.
pub const X_PROXY_USER_ID: HeaderName = HeaderName::from_static("x-proxy-user-id");

/// Header name strings that are stripped from inbound client requests before
/// proxying upstream. Authorization is rewritten by the adapter; the rest are
/// either hop-by-hop or fingerprint headers we re-emit ourselves.
pub const HOP_BY_HOP_BASE: &[&str] = &[
    "host",
    "content-length",
    "connection",
    "transfer-encoding",
    "accept-encoding",
];

/// Hop-by-hop + auth/fingerprint headers the Anthropic adapter strips.
pub const HOP_BY_HOP_ANTHROPIC: &[&str] = &[
    "host",
    "content-length",
    "connection",
    "transfer-encoding",
    "authorization",
    "x-api-key",
    "accept-encoding",
    "user-agent",
    "anthropic-dangerous-direct-browser-access",
];

/// Hop-by-hop + auth/cookie/fingerprint headers the Codex adapter strips.
pub const HOP_BY_HOP_CODEX: &[&str] = &[
    "host",
    "content-length",
    "connection",
    "transfer-encoding",
    "authorization",
    "cookie",
    "user-agent",
    "accept-encoding",
];

/// Stainless-prefix headers are stripped wholesale and re-emitted by the adapter.
pub const STAINLESS_PREFIX: &str = "x-stainless-";

/// Anthropic OAuth beta tag required when authenticating via Claude Code OAuth.
pub const ANTHROPIC_OAUTH_BETA: &str = "oauth-2025-04-20";

/// Anthropic ratelimit header names.
pub const H_RL_5H_UTIL: &str = "anthropic-ratelimit-unified-5h-utilization";
pub const H_RL_7D_UTIL: &str = "anthropic-ratelimit-unified-7d-utilization";
pub const H_RL_5H_RESET: &str = "anthropic-ratelimit-unified-5h-reset-at";
pub const H_RL_7D_RESET: &str = "anthropic-ratelimit-unified-7d-reset-at";
pub const H_RL_FALLBACK: &str = "anthropic-ratelimit-unified-fallback-percentage";
pub const H_RL_STATUS: &str = "anthropic-ratelimit-unified-status";
