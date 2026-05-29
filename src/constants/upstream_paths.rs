//! Upstream URL paths and endpoint constants for the providers we proxy.

pub const ANTHROPIC_MESSAGES_PATH: &str = "/v1/messages";
pub const ANTHROPIC_MODELS_PATH: &str = "/v1/models";
pub const ANTHROPIC_DEFAULT_UPSTREAM: &str = "https://api.anthropic.com";

pub const CODEX_RESPONSES_PATH: &str = "/backend-api/codex/responses";
pub const CODEX_DEFAULT_UPSTREAM: &str = "https://chatgpt.com";
pub const CODEX_REFRESH_ENDPOINT: &str = "https://chatgpt.com/backend-api/codex/auth/refresh";

pub const ANTHROPIC_OAUTH_TOKEN_ENDPOINTS: &[&str] = &[
    "https://api.anthropic.com/v1/oauth/token",
    "https://console.anthropic.com/v1/oauth/token",
];

pub const CLAUDE_CODE_OAUTH_CLIENT_ID: &str = "9d1c250a-e61b-44d9-88ed-5944d1962f5e";
