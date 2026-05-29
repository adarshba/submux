//! Auto-discovery of provider credentials from the official CLIs.
//!
//! `claude login` and `codex login` already store OAuth credentials on
//! disk in well-known locations. Submux reads from those same locations
//! at startup so the user never has to copy-paste a token.
//!
//! Resolution order owned by [`crate::config::Settings`]:
//! `env > config file > auto-discovery > none`.

pub mod anthropic;
pub mod codex;

pub use anthropic::{discover as discover_anthropic, DiscoveredAnthropic};
pub use codex::{discover as discover_codex, DiscoveredCodex};

/// Concrete origin for an auto-discovered credential. The label is what
/// the startup banner renders so users can see exactly which CLI's
/// credentials were picked up.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiscoverySource {
    /// macOS keychain item `Claude Code-credentials`.
    ClaudeKeychain,
    /// `~/.claude/.credentials.json` (Linux + macOS fallback).
    ClaudeFile,
    /// `~/.codex/auth.json` written by the official Codex CLI.
    CodexAuthFile,
}
