//! Command-line surface.
//!
//! `submux` is the only verb. Flags exist solely to let operators override
//! the config path and tune log verbosity. The TOML config file is the
//! source of truth for everything else; edit it with `$EDITOR` and restart.

use clap::Parser;
use std::path::PathBuf;

/// Subscription-native LLM gateway (Claude Max + ChatGPT Codex).
#[derive(Debug, Parser)]
#[command(name = "submux", version, about)]
pub struct Cli {
    /// Override the config file path (defaults to the XDG config home).
    #[arg(long)]
    pub config: Option<PathBuf>,

    /// Lower log verbosity to `warn`.
    #[arg(short, long, conflicts_with = "verbose")]
    pub quiet: bool,

    /// Raise log verbosity to `debug`.
    #[arg(short, long)]
    pub verbose: bool,
}

impl Cli {
    /// Translate `-q` / `-v` flags into a tracing filter directive.
    pub fn log_filter(&self) -> &'static str {
        match (self.quiet, self.verbose) {
            (true, _) => "warn",
            (_, true) => "debug",
            _ => "info",
        }
    }
}
