//! On-disk TOML config file at the XDG config path.
//!
//! Submux can run with zero config (env vars only), but writing a skeleton
//! file on first boot gives users a stable place to drop tokens and lock
//! down the proxy without having to memorize env var names.
//!
//! Field semantics: every field is `Option<…>`. A missing field means "fall
//! back to env / default". An empty string means "explicitly clear" (used
//! today only for `api_key` to express open-access mode).

use directories::ProjectDirs;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use thiserror::Error;
use uuid::Uuid;

use crate::core::ApiKey;

const SKELETON_TEMPLATE: &str = include_str!("skeleton.toml");
const DEVICE_ID_PLACEHOLDER: &str = "__SUBMUX_DEVICE_ID__";

/// Failure modes for reading or writing the config file.
#[derive(Debug, Error)]
pub enum ConfigFileError {
    /// XDG could not resolve a config home and no explicit path was given.
    #[error("config path unknown; set SUBMUX_CONFIG or pass --config <path>")]
    NoPath,

    /// Filesystem read failed.
    #[error("read `{path}`: {source}")]
    Read {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    /// Filesystem write failed.
    #[error("write `{path}`: {source}")]
    Write {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    /// Existing file blocked overwrite and `force` was not set.
    #[error("`{path}` already exists; pass --force to overwrite")]
    AlreadyExists { path: PathBuf },

    /// TOML deserialization failed.
    #[error("parse `{path}`: {source}")]
    Parse {
        path: PathBuf,
        #[source]
        source: toml::de::Error,
    },
}

/// Top-level deserialized config-file shape. Every field is optional and
/// defaults to "fall back to the next layer" (env or hardcoded default).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct FileConfig {
    #[serde(default)]
    pub bind: Option<String>,
    #[serde(default)]
    pub api_key: Option<String>,
    #[serde(default)]
    pub anthropic: AnthropicFile,
    #[serde(default)]
    pub openai: OpenAiFile,
}

/// Anthropic provider section of the config file.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct AnthropicFile {
    #[serde(default)]
    pub oauth_token: Option<String>,
    #[serde(default)]
    pub refresh_token: Option<String>,
    #[serde(default)]
    pub upstream: Option<String>,
}

/// OpenAI / Codex provider section of the config file.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct OpenAiFile {
    #[serde(default)]
    pub access_token: Option<String>,
    #[serde(default)]
    pub cookies: Option<String>,
    #[serde(default)]
    pub device_id: Option<String>,
    #[serde(default)]
    pub upstream: Option<String>,
}

/// Resolve the default config path: `$SUBMUX_CONFIG`, else
/// `$XDG_CONFIG_HOME/submux/config.toml` (Linux) or the platform equivalent
/// via the `directories` crate.
pub fn default_path() -> Result<PathBuf, ConfigFileError> {
    if let Ok(custom) = std::env::var("SUBMUX_CONFIG") {
        if !custom.trim().is_empty() {
            return Ok(PathBuf::from(custom));
        }
    }
    ProjectDirs::from("io", "submux", "submux")
        .map(|p| p.config_dir().join("config.toml"))
        .ok_or(ConfigFileError::NoPath)
}

/// Load the file at `path`, or write a fresh skeleton if it does not exist.
/// Returns `(config, created)` where `created` is true iff a new skeleton
/// was just written.
pub fn load_or_init(path: &Path) -> Result<(FileConfig, bool), ConfigFileError> {
    if path.exists() {
        let cfg = read(path)?;
        return Ok((cfg, false));
    }
    write_skeleton(path)?;
    let cfg = read(path)?;
    Ok((cfg, true))
}

/// Force-write a fresh skeleton. Refuses to overwrite a non-empty file
/// unless `overwrite` is set.
pub fn write_fresh(path: &Path, overwrite: bool) -> Result<(), ConfigFileError> {
    if path.exists() && !overwrite {
        return Err(ConfigFileError::AlreadyExists { path: path.into() });
    }
    write_skeleton(path)
}

/// Replace the `api_key` line in an existing config file with a freshly
/// generated key, preserving every other line (including user comments).
pub fn rotate_api_key(path: &Path) -> Result<ApiKey, ConfigFileError> {
    let raw = std::fs::read_to_string(path).map_err(|source| ConfigFileError::Read {
        path: path.into(),
        source,
    })?;
    let new_key = ApiKey::generate();
    let replacement = format!("api_key = \"{}\"", new_key.as_str());
    let mut found = false;
    let mut out = String::with_capacity(raw.len() + replacement.len());
    for line in raw.lines() {
        if !found && is_top_level_api_key_line(line) {
            out.push_str(&replacement);
            found = true;
        } else {
            out.push_str(line);
        }
        out.push('\n');
    }
    if !found {
        out.push_str(&replacement);
        out.push('\n');
    }
    write_atomic(path, &out)?;
    Ok(new_key)
}

/// Render the config back to TOML for `submux config show`. The `api_key`
/// field is redacted unless `reveal` is true.
pub fn render(cfg: &FileConfig, reveal: bool) -> String {
    let mut shown = cfg.clone();
    if !reveal {
        shown.api_key = shown
            .api_key
            .as_deref()
            .and_then(|raw| ApiKey::parse(raw).ok().map(|k| k.redacted()));
        if let Some(token) = &shown.anthropic.oauth_token {
            shown.anthropic.oauth_token = Some(redact_secret(token));
        }
        if let Some(token) = &shown.anthropic.refresh_token {
            shown.anthropic.refresh_token = Some(redact_secret(token));
        }
        if let Some(token) = &shown.openai.access_token {
            shown.openai.access_token = Some(redact_secret(token));
        }
    }
    toml::to_string_pretty(&shown).unwrap_or_else(|_| String::from("# <unrenderable>"))
}

fn read(path: &Path) -> Result<FileConfig, ConfigFileError> {
    let raw = std::fs::read_to_string(path).map_err(|source| ConfigFileError::Read {
        path: path.into(),
        source,
    })?;
    toml::from_str(&raw).map_err(|source| ConfigFileError::Parse {
        path: path.into(),
        source,
    })
}

fn write_skeleton(path: &Path) -> Result<(), ConfigFileError> {
    let device_id = Uuid::new_v4().to_string();
    let body = SKELETON_TEMPLATE.replace(DEVICE_ID_PLACEHOLDER, &device_id);
    write_atomic(path, &body)
}

fn write_atomic(path: &Path, contents: &str) -> Result<(), ConfigFileError> {
    if let Some(parent) = path.parent() {
        if !parent.as_os_str().is_empty() {
            std::fs::create_dir_all(parent).map_err(|source| ConfigFileError::Write {
                path: parent.into(),
                source,
            })?;
        }
    }
    write_with_mode(path, contents.as_bytes())
}

#[cfg(unix)]
fn write_with_mode(path: &Path, bytes: &[u8]) -> Result<(), ConfigFileError> {
    use std::io::Write as _;
    use std::os::unix::fs::OpenOptionsExt;

    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(path)
        .map_err(|source| ConfigFileError::Write {
            path: path.into(),
            source,
        })?;
    file.write_all(bytes)
        .map_err(|source| ConfigFileError::Write {
            path: path.into(),
            source,
        })?;
    Ok(())
}

#[cfg(not(unix))]
fn write_with_mode(path: &Path, bytes: &[u8]) -> Result<(), ConfigFileError> {
    std::fs::write(path, bytes).map_err(|source| ConfigFileError::Write {
        path: path.into(),
        source,
    })
}

fn is_top_level_api_key_line(line: &str) -> bool {
    let trimmed = line.trim_start();
    if trimmed.starts_with('#') {
        return false;
    }
    let mut rest = match trimmed.strip_prefix("api_key") {
        Some(r) => r,
        None => return false,
    };
    rest = rest.trim_start();
    rest.starts_with('=')
}

fn redact_secret(value: &str) -> String {
    let head: String = value.chars().take(8).collect();
    format!("{head}…")
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn load_or_init_creates_skeleton() {
        let dir = tempdir().expect("temp dir");
        let path = dir.path().join("config.toml");
        let (cfg, created) = load_or_init(&path).expect("init");
        assert!(created);
        assert!(path.exists());
        assert_eq!(cfg.bind.as_deref(), Some("127.0.0.1:8080"));
        assert!(cfg.openai.device_id.is_some());
        assert_eq!(cfg.api_key.as_deref(), Some(""));
    }

    #[test]
    fn load_or_init_reads_existing() {
        let dir = tempdir().expect("temp dir");
        let path = dir.path().join("config.toml");
        std::fs::write(&path, "api_key = \"hello\"\n").expect("write");
        let (cfg, created) = load_or_init(&path).expect("read");
        assert!(!created);
        assert_eq!(cfg.api_key.as_deref(), Some("hello"));
    }

    #[test]
    fn write_fresh_refuses_existing_without_force() {
        let dir = tempdir().expect("temp dir");
        let path = dir.path().join("config.toml");
        std::fs::write(&path, "x = 1").expect("seed");
        let err = write_fresh(&path, false).expect_err("must refuse");
        assert!(matches!(err, ConfigFileError::AlreadyExists { .. }));
    }

    #[test]
    fn write_fresh_overwrites_with_force() {
        let dir = tempdir().expect("temp dir");
        let path = dir.path().join("config.toml");
        std::fs::write(&path, "x = 1").expect("seed");
        write_fresh(&path, true).expect("force overwrite");
        let body = std::fs::read_to_string(&path).expect("read");
        assert!(body.contains("bind"));
    }

    #[test]
    fn rotate_api_key_replaces_line_and_keeps_comments() {
        let dir = tempdir().expect("temp dir");
        let path = dir.path().join("config.toml");
        let original = "# leading comment\napi_key = \"\"\n# trailing\n[anthropic]\n";
        std::fs::write(&path, original).expect("seed");
        let new_key = rotate_api_key(&path).expect("rotate");
        let body = std::fs::read_to_string(&path).expect("read");
        assert!(body.contains("# leading comment"));
        assert!(body.contains("# trailing"));
        assert!(body.contains(new_key.as_str()));
        assert!(!body.contains("api_key = \"\""));
    }

    #[test]
    fn rotate_api_key_appends_when_missing() {
        let dir = tempdir().expect("temp dir");
        let path = dir.path().join("config.toml");
        std::fs::write(&path, "[anthropic]\n").expect("seed");
        let new_key = rotate_api_key(&path).expect("rotate");
        let body = std::fs::read_to_string(&path).expect("read");
        assert!(body.contains(new_key.as_str()));
    }

    #[test]
    fn render_redacts_by_default() {
        let cfg = FileConfig {
            api_key: Some("smx_live_secretsecret".to_owned()),
            anthropic: AnthropicFile {
                oauth_token: Some("sk-ant-oat01-very-secret".to_owned()),
                ..Default::default()
            },
            ..Default::default()
        };
        let out = render(&cfg, false);
        assert!(!out.contains("secretsecret"));
        assert!(!out.contains("very-secret"));
        assert!(out.contains("smx_live_secr…") || out.contains("smx_live_secr\\u2026"));
    }
}
