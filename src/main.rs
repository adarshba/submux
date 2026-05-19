use std::sync::Arc;

use eyre::Result;
use submux::accounts::account::Account;
use submux::accounts::{AccountPool, RefreshManager};
use submux::constants::limits::{DEFAULT_COOLDOWN_CACHE_CAPACITY, DEFAULT_EVENT_BUS_CAPACITY};
use submux::constants::upstream_paths::{ANTHROPIC_DEFAULT_UPSTREAM, CODEX_DEFAULT_UPSTREAM};
use submux::core::SerializedCookieJar;
use submux::providers::anthropic::adapter::AnthropicOAuthAdapter;
use submux::providers::openai::{install_openai_adapter, OpenAiSubscriptionAdapter};
use submux::router::cooldown::CooldownCache;
use submux::router::retry::RetryPolicy;
use submux::router::strategies::round_robin::RoundRobin;
use submux::router::Router;
use submux::server::{app::AppState, build_app, shutdown::shutdown_signal};
use submux::storage::{PostgresAccountStore, Sealer};
use submux::telemetry::events::EventBus;
use tracing_subscriber::{fmt, prelude::*, EnvFilter};
use uuid::Uuid;

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::registry()
        .with(EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")))
        .with(fmt::layer().json())
        .init();

    let pool = Arc::new(AccountPool::new());
    seed_anthropic_account_from_env(&pool);
    seed_openai_account_from_env(&pool);
    load_persisted_accounts(&pool).await?;

    let cooldown = Arc::new(CooldownCache::new(DEFAULT_COOLDOWN_CACHE_CAPACITY));
    let router = Arc::new(Router::new(
        Box::new(RoundRobin::default()),
        (*cooldown).clone(),
        RetryPolicy::default(),
    ));

    let upstream = std::env::var("SUBMUX_ANTHROPIC_UPSTREAM")
        .unwrap_or_else(|_| ANTHROPIC_DEFAULT_UPSTREAM.to_owned());
    let upstream = upstream
        .parse::<url::Url>()
        .map_err(|e| eyre::eyre!("invalid SUBMUX_ANTHROPIC_UPSTREAM `{upstream}`: {e}"))?;
    let http = Arc::new(AnthropicOAuthAdapter::default_client());
    let anthropic = Arc::new(AnthropicOAuthAdapter::new(Arc::clone(&http), upstream));

    let openai_upstream = std::env::var("SUBMUX_OPENAI_UPSTREAM")
        .unwrap_or_else(|_| CODEX_DEFAULT_UPSTREAM.to_owned());
    let openai_upstream = openai_upstream
        .parse::<url::Url>()
        .map_err(|e| eyre::eyre!("invalid SUBMUX_OPENAI_UPSTREAM `{openai_upstream}`: {e}"))?;
    let openai_http = Arc::new(OpenAiSubscriptionAdapter::default_client());
    let openai = Arc::new(OpenAiSubscriptionAdapter::new(openai_http, openai_upstream));
    install_openai_adapter(Arc::clone(&openai));

    let refresh = Arc::new(RefreshManager::new());
    let events = Arc::new(EventBus::new(DEFAULT_EVENT_BUS_CAPACITY));

    let state = AppState {
        pool,
        router,
        anthropic,
        refresh,
        cooldown,
        events,
        http,
    };
    let app = build_app(state);

    let bind = std::env::var("SUBMUX_BIND").unwrap_or_else(|_| "127.0.0.1:8080".to_owned());
    tracing::info!(%bind, "submux starting");
    let listener = tokio::net::TcpListener::bind(&bind).await?;
    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal())
        .await?;
    Ok(())
}

fn seed_anthropic_account_from_env(pool: &Arc<AccountPool>) {
    let Ok(token) = std::env::var("SUBMUX_ANTHROPIC_OAUTH_TOKEN") else {
        tracing::warn!(
            "SUBMUX_ANTHROPIC_OAUTH_TOKEN not set; /v1/messages will return 503 until an account is seeded"
        );
        return;
    };
    let refresh = std::env::var("SUBMUX_ANTHROPIC_OAUTH_REFRESH_TOKEN").ok();
    let account = Account::new_anthropic_oauth("env-seed", token, refresh);
    let id = account.id;
    pool.insert(Arc::new(account));
    tracing::info!(account_id = %id, "seeded anthropic oauth account from env");
}

fn seed_openai_account_from_env(pool: &Arc<AccountPool>) {
    let Ok(token) = std::env::var("SUBMUX_OPENAI_ACCESS_TOKEN") else {
        return;
    };
    let cookies: SerializedCookieJar = std::env::var("SUBMUX_OPENAI_COOKIES")
        .ok()
        .and_then(|raw| serde_json::from_str(&raw).ok())
        .unwrap_or_default();
    let device_id =
        std::env::var("SUBMUX_OPENAI_DEVICE_ID").unwrap_or_else(|_| Uuid::new_v4().to_string());
    let account = Account::new_openai_subscription("env-seed-openai", token, cookies, device_id);
    let id = account.id;
    pool.insert(Arc::new(account));
    tracing::info!(account_id = %id, "seeded openai (codex) account from env");
}

/// Load accounts persisted in Postgres, if both `SUBMUX_DATABASE_URL` and
/// `SUBMUX_SEALER_KEY` are configured. Missing config is a no-op (we just
/// run with whatever the env seeders gave us). Missing key with a database
/// URL is a hard error in waiting — we log loudly and skip the load so the
/// operator notices.
async fn load_persisted_accounts(pool: &Arc<AccountPool>) -> Result<()> {
    let Ok(url) = std::env::var("SUBMUX_DATABASE_URL") else {
        return Ok(());
    };
    if std::env::var("SUBMUX_SEALER_KEY").is_err() {
        tracing::warn!(
            "SUBMUX_DATABASE_URL set but SUBMUX_SEALER_KEY missing; skipping persisted account load"
        );
        return Ok(());
    }
    let sealer = Sealer::from_env("SUBMUX_SEALER_KEY")?;
    let store = PostgresAccountStore::connect(&url, sealer).await?;
    store.migrate().await?;
    let persisted = store.load_all().await?;
    let n = persisted.len();
    for row in persisted {
        let acct = Account::from_persisted(row);
        pool.insert(Arc::new(acct));
    }
    tracing::info!(loaded = n, "loaded persisted accounts from postgres");
    Ok(())
}
