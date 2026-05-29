//! Translate a [`Settings`] into seeded accounts.
//!
//! `main.rs` calls [`from_settings`] once at startup. Missing provider
//! credentials are not an error — the corresponding routes simply return
//! 503 until an account is configured.

use std::sync::Arc;

use eyre::Result;

use crate::accounts::{Account, AccountPool};
use crate::config::Settings;

/// Counts of what was seeded. Banner reads these to render provider lines.
#[derive(Debug, Default, Clone, Copy)]
pub struct SeedOutcome {
    pub anthropic_seeded: bool,
    pub openai_seeded: bool,
}

/// Insert file/env-driven accounts into `pool`.
pub async fn from_settings(settings: &Settings, pool: &Arc<AccountPool>) -> Result<SeedOutcome> {
    Ok(SeedOutcome {
        anthropic_seeded: seed_anthropic(settings, pool),
        openai_seeded: seed_codex(settings, pool),
    })
}

fn seed_anthropic(settings: &Settings, pool: &Arc<AccountPool>) -> bool {
    let Some(token) = settings.anthropic.oauth_token.clone() else {
        return false;
    };
    let account = Account::new_anthropic_oauth(
        "config-seed",
        token,
        settings.anthropic.refresh_token.clone(),
    );
    let id = account.id;
    pool.insert(Arc::new(account));
    tracing::info!(account_id = %id, "seeded anthropic oauth account from config");
    true
}

fn seed_codex(settings: &Settings, pool: &Arc<AccountPool>) -> bool {
    let Some(token) = settings.openai.access_token.clone() else {
        return false;
    };
    let account = Account::new_openai_subscription(
        "config-seed-codex",
        token,
        settings.openai.cookies.clone(),
        settings.openai.device_id.clone(),
    );
    let id = account.id;
    pool.insert(Arc::new(account));
    tracing::info!(account_id = %id, "seeded codex (chatgpt) account from config");
    true
}
