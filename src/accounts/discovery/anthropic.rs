//! Auto-discovery of Claude Code OAuth credentials.
//!
//! Mirrors the storage convention used by the official `claude` CLI so that
//! `submux` can pick up whatever the user already logged in with — no token
//! pasting required.
//!
//! - **macOS**: a generic keychain item named `Claude Code-credentials`
//!   under the current user's account. The stored value is either raw
//!   UTF-8 JSON (older installs) or hex-encoded JSON (newer installs).
//! - **Linux / fallback**: `~/.claude/.credentials.json` containing the same
//!   JSON shape directly.
//!
//! In both cases the parsed JSON has a `claudeAiOauth` object with
//! `accessToken`, `refreshToken`, and `expiresAt` (unix ms) fields.

use std::path::PathBuf;

use serde::Deserialize;

use crate::accounts::discovery::DiscoverySource;

const KEYCHAIN_SERVICE: &str = "Claude Code-credentials";
const LINUX_CREDENTIALS_PATH: &str = ".claude/.credentials.json";

/// Credentials discovered on disk or in the keychain.
#[derive(Debug, Clone)]
pub struct DiscoveredAnthropic {
    pub access_token: String,
    pub refresh_token: Option<String>,
    pub source: DiscoverySource,
}

/// Try every known credential location for the host platform. Returns
/// `None` if no source had usable credentials.
pub fn discover() -> Option<DiscoveredAnthropic> {
    if cfg!(target_os = "macos") {
        if let Some(creds) = read_keychain() {
            return Some(creds);
        }
    }
    read_credentials_file()
}

#[derive(Debug, Deserialize)]
struct CredentialsFile {
    #[serde(rename = "claudeAiOauth")]
    claude_ai_oauth: Option<OauthBlock>,
}

#[derive(Debug, Deserialize)]
struct OauthBlock {
    #[serde(rename = "accessToken")]
    access_token: String,
    #[serde(rename = "refreshToken")]
    refresh_token: Option<String>,
}

fn read_keychain() -> Option<DiscoveredAnthropic> {
    let account = whoami_username()?;
    let output = std::process::Command::new("/usr/bin/security")
        .args([
            "find-generic-password",
            "-s",
            KEYCHAIN_SERVICE,
            "-a",
            &account,
            "-w",
        ])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let raw = std::str::from_utf8(&output.stdout).ok()?.trim();
    let parsed = parse_payload(raw)?;
    Some(DiscoveredAnthropic {
        access_token: parsed.access_token,
        refresh_token: parsed.refresh_token.filter(|s| !s.is_empty()),
        source: DiscoverySource::ClaudeKeychain,
    })
}

fn read_credentials_file() -> Option<DiscoveredAnthropic> {
    let path = home_dir()?.join(LINUX_CREDENTIALS_PATH);
    let raw = std::fs::read_to_string(&path).ok()?;
    let parsed = parse_payload(raw.trim())?;
    Some(DiscoveredAnthropic {
        access_token: parsed.access_token,
        refresh_token: parsed.refresh_token.filter(|s| !s.is_empty()),
        source: DiscoverySource::ClaudeFile,
    })
}

fn parse_payload(raw: &str) -> Option<OauthBlock> {
    let json = if raw.starts_with('{') {
        raw.to_owned()
    } else {
        hex_decode(raw).and_then(|bytes| String::from_utf8(bytes).ok())?
    };
    let file: CredentialsFile = serde_json::from_str(&json).ok()?;
    file.claude_ai_oauth
}

fn hex_decode(s: &str) -> Option<Vec<u8>> {
    if s.len() % 2 != 0 || !s.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(s.len() / 2);
    for chunk in bytes.chunks_exact(2) {
        let hi = hex_nibble(chunk[0])?;
        let lo = hex_nibble(chunk[1])?;
        out.push((hi << 4) | lo);
    }
    Some(out)
}

fn hex_nibble(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        b'A'..=b'F' => Some(b - b'A' + 10),
        _ => None,
    }
}

fn home_dir() -> Option<PathBuf> {
    directories::UserDirs::new().map(|d| d.home_dir().to_path_buf())
}

fn whoami_username() -> Option<String> {
    std::env::var("USER")
        .ok()
        .filter(|s| !s.is_empty())
        .or_else(|| std::env::var("LOGNAME").ok().filter(|s| !s.is_empty()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_raw_json_payload() {
        let raw = r#"{"claudeAiOauth":{"accessToken":"sk-ant-oat01-abc","refreshToken":"sk-ant-ort01-def","expiresAt":1780000000000}}"#;
        let parsed = parse_payload(raw).expect("parse");
        assert_eq!(parsed.access_token, "sk-ant-oat01-abc");
        assert_eq!(parsed.refresh_token.as_deref(), Some("sk-ant-ort01-def"));
    }

    #[test]
    fn parses_hex_encoded_payload() {
        let raw_json = r#"{"claudeAiOauth":{"accessToken":"sk-ant-oat01-xyz"}}"#;
        let hex: String = raw_json.bytes().map(|b| format!("{b:02x}")).collect();
        let parsed = parse_payload(&hex).expect("parse");
        assert_eq!(parsed.access_token, "sk-ant-oat01-xyz");
        assert!(parsed.refresh_token.is_none());
    }

    #[test]
    fn rejects_garbage() {
        assert!(parse_payload("not json or hex").is_none());
        assert!(parse_payload("{not json").is_none());
    }

    #[test]
    fn rejects_missing_claude_block() {
        assert!(parse_payload(r#"{"otherKey":42}"#).is_none());
    }

    #[test]
    fn hex_decode_round_trip() {
        assert_eq!(hex_decode("48656c6c6f"), Some(b"Hello".to_vec()));
        assert_eq!(hex_decode("HELLO"), None);
        assert_eq!(hex_decode("abc"), None);
    }
}
