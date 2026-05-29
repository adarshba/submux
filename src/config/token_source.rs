//! Origin of a provider's credentials, surfaced on the startup banner so
//! users can see whether submux is using their config file, an env var
//! override, or auto-discovered values from the official CLIs.

use crate::accounts::DiscoverySource;

/// Where the credentials for a given provider came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TokenSource {
    /// Provider has no configured credentials. Routes will 503.
    None,
    /// `SUBMUX_*` env var supplied the value.
    Env,
    /// The TOML config file supplied the value.
    File,
    /// Auto-discovered from a CLI credential store (claude / codex).
    Discovered(DiscoverySource),
}

impl TokenSource {
    /// Short human label for the startup banner.
    pub fn label(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::Env => "env",
            Self::File => "config file",
            Self::Discovered(DiscoverySource::ClaudeKeychain) => "claude keychain",
            Self::Discovered(DiscoverySource::ClaudeFile) => "claude credentials file",
            Self::Discovered(DiscoverySource::CodexAuthFile) => "codex login",
        }
    }
}
