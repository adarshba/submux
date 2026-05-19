//! Cloak fingerprint strings for the official upstream clients we impersonate.
//!
//! These are *the* contract that gets the gateway accepted by the upstream
//! provider; bumping a version here is intentional and visible in `git log`.

pub const CLAUDE_CODE_VERSION: &str = "2.1.87";
pub const CLAUDE_CODE_USER_AGENT: &str = "claude-cli/2.1.87 (external, cli)";

pub const CODEX_CLI_VERSION: &str = "0.30.0";
pub const CODEX_CLI_USER_AGENT: &str = "codex-cli/0.30.0 (Mac OS X; arm64)";
