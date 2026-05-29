use std::sync::Arc;

use clap::Parser;
use eyre::Result;
use is_terminal::IsTerminal;
use submux::accounts::{AccountPool, CooldownCache, RefreshManager, seed_from_settings};
use submux::cli::Cli;
use submux::config::Settings;
use submux::constants::limits::DEFAULT_COOLDOWN_CACHE_CAPACITY;
use submux::providers::anthropic::AnthropicProxy;
use submux::providers::codex::{CodexProxy, install as install_codex_proxy};
use submux::server::{app::AppState, banner, build_app, shutdown::shutdown_signal};
use submux::telemetry::metrics::{self, OtelConfig};
use tracing_subscriber::{EnvFilter, fmt, prelude::*};

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    init_tracing(cli.log_filter());

    let meter_provider = metrics::init(OtelConfig {
        service_version: env!("CARGO_PKG_VERSION").to_owned(),
        otlp_endpoint: std::env::var("SUBMUX_OTLP_ENDPOINT")
            .ok()
            .filter(|e| !e.trim().is_empty()),
    })?;

    let settings = Settings::load(cli.config)?;

    let pool = Arc::new(AccountPool::new());
    let outcome = seed_from_settings(&settings, &pool).await?;

    let cooldown = Arc::new(CooldownCache::new(DEFAULT_COOLDOWN_CACHE_CAPACITY));

    let http = Arc::new(AnthropicProxy::default_client());
    let anthropic = Arc::new(AnthropicProxy::new(
        Arc::clone(&http),
        settings.anthropic.upstream.clone(),
    ));

    let codex_http = Arc::new(CodexProxy::default_client());
    let codex = Arc::new(CodexProxy::new(
        codex_http,
        settings.openai.upstream.clone(),
    ));
    install_codex_proxy(Arc::clone(&codex));

    let refresh = Arc::new(RefreshManager::new());

    let state = AppState {
        pool,
        anthropic,
        refresh,
        cooldown,
        http,
        api_key: settings.api_key.clone().map(Arc::new),
    };

    let listener = tokio::net::TcpListener::bind(&settings.bind).await?;
    let app = build_app(state);
    banner::print(&settings, &outcome);
    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal())
        .await?;

    if let Err(err) = meter_provider.shutdown() {
        tracing::warn!(error = %err, "metrics shutdown flush failed");
    }
    Ok(())
}

fn init_tracing(default_filter: &str) {
    let filter =
        EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new(default_filter));
    let registry = tracing_subscriber::registry().with(filter);
    if std::io::stderr().is_terminal() {
        registry.with(fmt::layer()).init();
    } else {
        registry.with(fmt::layer().json()).init();
    }
}
