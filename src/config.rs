//! Configuration: on-disk TOML file + env-var resolver.
//!
//! Submux reads its operational config from `~/.config/submux/config.toml`
//! (auto-created on first run) and overlays any matching `SUBMUX_*` env var.
//! [`Settings`] is the single resolved value downstream modules consume;
//! nothing else touches `std::env`.

pub mod file;
pub mod settings;
pub mod token_source;

pub use file::{ConfigFileError, FileConfig};
pub use settings::{AnthropicSettings, ConfigError, OpenAiSettings, Settings};
pub use token_source::TokenSource;
