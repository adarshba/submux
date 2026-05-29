//! Startup banner printed once after the listener binds.
//!
//! Plain text on stderr, gated on TTY so container logs and JSON tracing
//! stay clean. The same information is also emitted as a structured
//! `tracing::info!` event for log scrapers (OpenObserve, Grafana Loki, …).

use is_terminal::IsTerminal;
use std::io::Write;

use crate::accounts::SeedOutcome;
use crate::config::Settings;

const VERSION: &str = env!("CARGO_PKG_VERSION");

/// Render the banner to stderr if attached to a TTY, and always emit one
/// structured tracing line so the same info reaches log aggregators.
pub fn print(settings: &Settings, outcome: &SeedOutcome) {
    tracing::info!(
        bind = %settings.bind,
        config = %settings.config_path.display(),
        anthropic = outcome.anthropic_seeded,
        codex = outcome.openai_seeded,
        "submux ready",
    );

    let mut stderr = std::io::stderr();
    if !stderr.is_terminal() {
        return;
    }
    let text = render(settings, outcome);
    let _ = stderr.write_all(text.as_bytes());
}

fn render(settings: &Settings, outcome: &SeedOutcome) -> String {
    let mut out = String::new();
    out.push_str(&format!("\nsubmux {VERSION}\n"));
    out.push_str(&format!("  listening:    http://{}\n", settings.bind));

    let suffix = if settings.created_on_this_run {
        "  (created on this run)"
    } else {
        ""
    };
    out.push_str(&format!(
        "  config:       {}{}\n",
        settings.config_path.display(),
        suffix
    ));

    match &settings.api_key {
        Some(key) => {
            out.push_str(&format!(
                "  api key:      {}            \u{2190} set ANTHROPIC_API_KEY in your client\n",
                key.redacted()
            ));
        }
        None => {
            out.push_str(&format!(
                "  api key:      \u{26a0} open access \u{2014} anyone on {} can use this proxy\n",
                settings.bind
            ));
            out.push_str("                set `api_key` in the config file to lock it down\n");
        }
    }

    out.push_str(&format!(
        "  anthropic:    {}\n",
        anthropic_line(settings, outcome)
    ));
    out.push_str(&format!(
        "  codex:        {}\n",
        codex_line(settings, outcome)
    ));
    out.push('\n');
    out
}

fn anthropic_line(settings: &Settings, outcome: &SeedOutcome) -> String {
    if !outcome.anthropic_seeded {
        return "not configured \u{2014} run `claude login`, set anthropic.oauth_token, or export SUBMUX_ANTHROPIC_OAUTH_TOKEN"
            .to_owned();
    }
    let refresh = if settings.anthropic.refresh_token.is_some() {
        "yes"
    } else {
        "no"
    };
    format!(
        "ready (refresh: {refresh}, source: {})",
        settings.anthropic.source.label()
    )
}

fn codex_line(settings: &Settings, outcome: &SeedOutcome) -> String {
    if !outcome.openai_seeded {
        return "not configured \u{2014} run `codex login`, set openai.access_token, or export SUBMUX_OPENAI_ACCESS_TOKEN"
            .to_owned();
    }
    let device = settings
        .openai
        .device_id
        .chars()
        .take(4)
        .collect::<String>();
    format!(
        "ready (device: {device}\u{2026}, source: {})",
        settings.openai.source.label()
    )
}
