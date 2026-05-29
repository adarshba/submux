//! Auto-discovery of Codex (ChatGPT) credentials.
//!
//! Reads the file the official Codex CLI writes after `codex login`:
//! `~/.codex/auth.json`. Shape (subset we care about):
//!
//! ```json
//! {
//!   "auth_mode": "chatgpt",
//!   "tokens": {
//!     "access_token": "eyJ…",
//!     "refresh_token": "rt_…",
//!     "account_id": "uuid"
//!   }
//! }
//! ```
//!
//! Submux uses the `access_token` as the bearer. Refresh is handled out of
//! band by re-running `codex login` (a proper in-process refresh against
//! ChatGPT's token endpoint is on the roadmap).

use std::path::PathBuf;

use serde::Deserialize;

const AUTH_FILE: &str = ".codex/auth.json";

/// Credentials discovered in `~/.codex/auth.json`.
#[derive(Debug, Clone)]
pub struct DiscoveredCodex {
    pub access_token: String,
    pub account_id: Option<String>,
}

/// Read the Codex CLI credential file. Returns `None` when the file does
/// not exist, is unreadable, or the user is not signed in with a ChatGPT
/// subscription (`auth_mode != "chatgpt"`).
pub fn discover() -> Option<DiscoveredCodex> {
    let path = home_dir()?.join(AUTH_FILE);
    let raw = std::fs::read_to_string(&path).ok()?;
    parse(&raw)
}

#[derive(Debug, Deserialize)]
struct AuthFile {
    #[serde(default)]
    auth_mode: Option<String>,
    tokens: Option<TokensBlock>,
}

#[derive(Debug, Deserialize)]
struct TokensBlock {
    access_token: Option<String>,
    account_id: Option<String>,
}

fn parse(raw: &str) -> Option<DiscoveredCodex> {
    let file: AuthFile = serde_json::from_str(raw).ok()?;
    if file.auth_mode.as_deref() != Some("chatgpt") {
        return None;
    }
    let tokens = file.tokens?;
    let access_token = tokens.access_token?;
    if access_token.is_empty() {
        return None;
    }
    Some(DiscoveredCodex {
        access_token,
        account_id: tokens.account_id.filter(|s| !s.is_empty()),
    })
}

fn home_dir() -> Option<PathBuf> {
    directories::UserDirs::new().map(|d| d.home_dir().to_path_buf())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_chatgpt_session() {
        let raw = r#"{
            "auth_mode": "chatgpt",
            "tokens": {
                "access_token": "eyJabc",
                "refresh_token": "rt_xyz",
                "account_id": "5c2f6b9c-ea16-4358-8875-b9d848"
            }
        }"#;
        let parsed = parse(raw).expect("parse");
        assert_eq!(parsed.access_token, "eyJabc");
        assert_eq!(
            parsed.account_id.as_deref(),
            Some("5c2f6b9c-ea16-4358-8875-b9d848")
        );
    }

    #[test]
    fn rejects_api_key_mode() {
        let raw = r#"{"auth_mode":"api_key","tokens":{"access_token":"sk-..."}}"#;
        assert!(parse(raw).is_none());
    }

    #[test]
    fn rejects_missing_tokens_block() {
        let raw = r#"{"auth_mode":"chatgpt"}"#;
        assert!(parse(raw).is_none());
    }

    #[test]
    fn rejects_empty_access_token() {
        let raw = r#"{"auth_mode":"chatgpt","tokens":{"access_token":""}}"#;
        assert!(parse(raw).is_none());
    }

    #[test]
    fn rejects_garbage() {
        assert!(parse("not json").is_none());
    }
}
