//! Resolved settings: TOML file overlaid with `SUBMUX_*` env vars.
//!
//! Three layers, env on top: `env > file > default`. The struct lives in
//! its own file so downstream modules can `use crate::config::Settings`
//! without pulling in the file-IO or resolver internals. Env vars are
//! read here once so nothing else in the crate touches `std::env`.

use std::net::SocketAddr;
use std::path::PathBuf;
use thiserror::Error;
use url::Url;
use uuid::Uuid;

use crate::accounts::discovery::{discover_anthropic, discover_codex};
use crate::constants::upstream_paths::{ANTHROPIC_DEFAULT_UPSTREAM, CODEX_DEFAULT_UPSTREAM};
use crate::core::{ApiKey, ApiKeyError, SerializedCookieJar};

use super::file::{self, ConfigFileError, FileConfig};
use super::token_source::TokenSource;

const DEFAULT_BIND: &str = "127.0.0.1:8080";

/// Failure modes when resolving the runtime config.
#[derive(Debug, Error)]
pub enum ConfigError {
    /// Wraps a config-file IO or parse failure.
    #[error(transparent)]
    File(#[from] ConfigFileError),

    /// `bind` was set but not a valid socket address.
    #[error("invalid bind address `{value}`: {source}")]
    InvalidBind {
        value: String,
        #[source]
        source: std::net::AddrParseError,
    },

    /// An upstream URL was set but did not parse.
    #[error("invalid {field} upstream `{value}`: {source}")]
    InvalidUpstream {
        field: &'static str,
        value: String,
        #[source]
        source: url::ParseError,
    },

    /// `api_key` was set but failed to parse.
    #[error("invalid api_key: {0}")]
    InvalidApiKey(#[from] ApiKeyError),

    /// OpenAI cookies field was set but was not valid JSON for the jar.
    #[error("invalid openai.cookies JSON: {0}")]
    InvalidCookies(#[source] serde_json::Error),
}

/// Fully resolved runtime config — every field is ready to consume.
#[derive(Debug)]
pub struct Settings {
    pub bind: SocketAddr,
    pub api_key: Option<ApiKey>,
    pub anthropic: AnthropicSettings,
    pub openai: OpenAiSettings,
    pub config_path: PathBuf,
    pub created_on_this_run: bool,
}

/// Resolved Anthropic provider settings.
#[derive(Debug, Clone)]
pub struct AnthropicSettings {
    pub oauth_token: Option<String>,
    pub refresh_token: Option<String>,
    pub upstream: Url,
    pub source: TokenSource,
}

/// Resolved OpenAI / Codex provider settings.
#[derive(Debug, Clone)]
pub struct OpenAiSettings {
    pub access_token: Option<String>,
    pub cookies: SerializedCookieJar,
    pub device_id: String,
    pub upstream: Url,
    pub source: TokenSource,
}

impl Settings {
    /// Load the file at `explicit_path` (or the platform default) and merge
    /// in env-var overrides.
    pub fn load(explicit_path: Option<PathBuf>) -> Result<Self, ConfigError> {
        let path = match explicit_path {
            Some(p) => p,
            None => file::default_path()?,
        };
        let (file_cfg, created) = file::load_or_init(&path)?;
        Self::from_layers(file_cfg, path, created)
    }

    fn from_layers(
        file: FileConfig,
        config_path: PathBuf,
        created_on_this_run: bool,
    ) -> Result<Self, ConfigError> {
        let bind_raw = first_present(
            env_var("SUBMUX_BIND"),
            file.bind,
            Some(DEFAULT_BIND.to_owned()),
        )
        .unwrap_or_else(|| DEFAULT_BIND.to_owned());
        let bind: SocketAddr = bind_raw
            .parse()
            .map_err(|source| ConfigError::InvalidBind {
                value: bind_raw.clone(),
                source,
            })?;

        let api_key = first_present(env_var("SUBMUX_API_KEY"), file.api_key, None)
            .filter(|s| !s.trim().is_empty())
            .map(|s| ApiKey::parse(&s))
            .transpose()?;

        let anthropic = resolve_anthropic(file.anthropic)?;
        let openai = resolve_openai(file.openai)?;

        Ok(Self {
            bind,
            api_key,
            anthropic,
            openai,
            config_path,
            created_on_this_run,
        })
    }
}

fn resolve_anthropic(file: super::file::AnthropicFile) -> Result<AnthropicSettings, ConfigError> {
    let env_token = env_var("SUBMUX_ANTHROPIC_OAUTH_TOKEN").filter(|s| !s.is_empty());
    let env_refresh = env_var("SUBMUX_ANTHROPIC_OAUTH_REFRESH_TOKEN").filter(|s| !s.is_empty());
    let file_token = file.oauth_token.filter(|s| !s.is_empty());
    let file_refresh = file.refresh_token.filter(|s| !s.is_empty());

    let (oauth_token, refresh_token, source) = if let Some(token) = env_token {
        (Some(token), env_refresh.or(file_refresh), TokenSource::Env)
    } else if let Some(token) = file_token {
        (Some(token), file_refresh, TokenSource::File)
    } else if let Some(discovered) = discover_anthropic() {
        (
            Some(discovered.access_token),
            discovered.refresh_token,
            TokenSource::Discovered(discovered.source),
        )
    } else {
        (None, None, TokenSource::None)
    };

    let upstream_raw = first_present(
        env_var("SUBMUX_ANTHROPIC_UPSTREAM"),
        file.upstream,
        Some(ANTHROPIC_DEFAULT_UPSTREAM.to_owned()),
    )
    .unwrap_or_else(|| ANTHROPIC_DEFAULT_UPSTREAM.to_owned());
    let upstream = upstream_raw
        .parse::<Url>()
        .map_err(|source| ConfigError::InvalidUpstream {
            field: "anthropic",
            value: upstream_raw,
            source,
        })?;
    Ok(AnthropicSettings {
        oauth_token,
        refresh_token,
        upstream,
        source,
    })
}

fn resolve_openai(file: super::file::OpenAiFile) -> Result<OpenAiSettings, ConfigError> {
    let env_token = env_var("SUBMUX_OPENAI_ACCESS_TOKEN").filter(|s| !s.is_empty());
    let file_token = file.access_token.filter(|s| !s.is_empty());

    let (access_token, source) = if let Some(token) = env_token {
        (Some(token), TokenSource::Env)
    } else if let Some(token) = file_token {
        (Some(token), TokenSource::File)
    } else if let Some(discovered) = discover_codex() {
        (
            Some(discovered.access_token),
            TokenSource::Discovered(crate::accounts::DiscoverySource::CodexAuthFile),
        )
    } else {
        (None, TokenSource::None)
    };

    let cookies_raw = first_present(env_var("SUBMUX_OPENAI_COOKIES"), file.cookies, None)
        .filter(|s| !s.trim().is_empty());
    let cookies: SerializedCookieJar = match cookies_raw {
        Some(raw) => serde_json::from_str(&raw).map_err(ConfigError::InvalidCookies)?,
        None => SerializedCookieJar::default(),
    };

    let device_id = first_present(env_var("SUBMUX_OPENAI_DEVICE_ID"), file.device_id, None)
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| Uuid::new_v4().to_string());

    let upstream_raw = first_present(
        env_var("SUBMUX_OPENAI_UPSTREAM"),
        file.upstream,
        Some(CODEX_DEFAULT_UPSTREAM.to_owned()),
    )
    .unwrap_or_else(|| CODEX_DEFAULT_UPSTREAM.to_owned());
    let upstream = upstream_raw
        .parse::<Url>()
        .map_err(|source| ConfigError::InvalidUpstream {
            field: "openai",
            value: upstream_raw,
            source,
        })?;

    Ok(OpenAiSettings {
        access_token,
        cookies,
        device_id,
        upstream,
        source,
    })
}

fn env_var(name: &str) -> Option<String> {
    std::env::var(name).ok().filter(|v| !v.is_empty())
}

fn first_present(
    env: Option<String>,
    file: Option<String>,
    default: Option<String>,
) -> Option<String> {
    env.or(file).or(default)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::file::{AnthropicFile, OpenAiFile};

    fn base_file() -> FileConfig {
        FileConfig {
            bind: Some("127.0.0.1:9090".to_owned()),
            api_key: Some("smx_live_filevalue".to_owned()),
            anthropic: AnthropicFile {
                oauth_token: Some("file-anthropic".to_owned()),
                refresh_token: None,
                upstream: None,
            },
            openai: OpenAiFile {
                access_token: Some("file-openai".to_owned()),
                cookies: None,
                device_id: Some("device-from-file".to_owned()),
                upstream: None,
            },
        }
    }

    static ENV_GUARD: std::sync::Mutex<()> = std::sync::Mutex::new(());

    const ALL_VARS: &[&str] = &[
        "SUBMUX_BIND",
        "SUBMUX_API_KEY",
        "SUBMUX_ANTHROPIC_OAUTH_TOKEN",
        "SUBMUX_ANTHROPIC_OAUTH_REFRESH_TOKEN",
        "SUBMUX_ANTHROPIC_UPSTREAM",
        "SUBMUX_OPENAI_ACCESS_TOKEN",
        "SUBMUX_OPENAI_COOKIES",
        "SUBMUX_OPENAI_DEVICE_ID",
        "SUBMUX_OPENAI_UPSTREAM",
    ];

    fn with_env<F: FnOnce() -> R, R>(vars: &[(&str, Option<&str>)], f: F) -> R {
        let lock = ENV_GUARD.lock().unwrap_or_else(|e| e.into_inner());
        let saved: Vec<(String, Option<String>)> = ALL_VARS
            .iter()
            .map(|k| ((*k).to_owned(), std::env::var(k).ok()))
            .collect();
        for k in ALL_VARS {
            std::env::remove_var(k);
        }
        for (k, v) in vars {
            if let Some(value) = v {
                std::env::set_var(k, value);
            }
        }
        let out = f();
        for (k, original) in saved {
            match original {
                Some(value) => std::env::set_var(&k, value),
                None => std::env::remove_var(&k),
            }
        }
        drop(lock);
        out
    }

    #[test]
    fn file_only_resolves_with_defaults() {
        let cfg = with_env(
            &[
                ("SUBMUX_BIND", None),
                ("SUBMUX_API_KEY", None),
                ("SUBMUX_ANTHROPIC_OAUTH_TOKEN", None),
                ("SUBMUX_OPENAI_ACCESS_TOKEN", None),
                ("SUBMUX_OPENAI_DEVICE_ID", None),
            ],
            || {
                Settings::from_layers(base_file(), PathBuf::from("/tmp/x.toml"), true)
                    .expect("resolve")
            },
        );
        assert_eq!(cfg.bind.to_string(), "127.0.0.1:9090");
        assert!(cfg.api_key.is_some());
        assert_eq!(cfg.anthropic.oauth_token.as_deref(), Some("file-anthropic"));
        assert_eq!(cfg.openai.access_token.as_deref(), Some("file-openai"));
        assert_eq!(cfg.openai.device_id, "device-from-file");
        assert!(cfg.created_on_this_run);
    }

    #[test]
    fn env_overrides_file() {
        let cfg = with_env(
            &[
                ("SUBMUX_BIND", Some("127.0.0.1:7070")),
                ("SUBMUX_API_KEY", Some("env-key")),
                ("SUBMUX_ANTHROPIC_OAUTH_TOKEN", Some("env-anthropic")),
                ("SUBMUX_OPENAI_ACCESS_TOKEN", Some("env-openai")),
                ("SUBMUX_OPENAI_DEVICE_ID", None),
            ],
            || {
                Settings::from_layers(base_file(), PathBuf::from("/tmp/x.toml"), false)
                    .expect("resolve")
            },
        );
        assert_eq!(cfg.bind.to_string(), "127.0.0.1:7070");
        assert_eq!(cfg.api_key.expect("set").as_str(), "env-key");
        assert_eq!(cfg.anthropic.oauth_token.as_deref(), Some("env-anthropic"));
        assert_eq!(cfg.openai.access_token.as_deref(), Some("env-openai"));
    }

    #[test]
    fn empty_api_key_means_open_mode() {
        let mut file = base_file();
        file.api_key = Some(String::new());
        let cfg = with_env(&[("SUBMUX_API_KEY", None)], || {
            Settings::from_layers(file, PathBuf::from("/tmp/x.toml"), false).expect("resolve")
        });
        assert!(cfg.api_key.is_none());
    }
}
