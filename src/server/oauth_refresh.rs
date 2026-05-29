//! Anthropic OAuth refresh-on-401 orchestration shared by request handlers.
//!
//! Wraps [`RefreshManager`] singleflight around the token exchange so concurrent
//! handlers that hit a 401 coalesce onto a single upstream refresh, then read
//! the freshly-stored access token back off the account.

use chrono::{Duration as ChronoDuration, Utc};
use std::sync::Arc;

use crate::accounts::Account;
use crate::accounts::refresh::RefreshOutcome;
use crate::providers::anthropic::oauth::{RefreshedTokens, exchange_refresh_token};
use crate::server::app::AppState;
use crate::telemetry::metrics;

/// Refresh the account's Anthropic OAuth token and return the new access token.
///
/// Returns `Err` with a human-readable reason when the account has no refresh
/// token, the upstream exchange fails, or the refreshed token is unexpectedly
/// absent afterwards. Callers treat any error as "could not refresh" and fall
/// back to surfacing the original upstream response.
pub(crate) async fn refresh_anthropic_account(
    state: &AppState,
    account: &Arc<Account>,
) -> Result<String, String> {
    let refresh_token = account
        .anthropic_refresh_token()
        .ok_or_else(|| "account has no refresh token".to_owned())?;
    let http = Arc::clone(&state.http);
    let account_clone = Arc::clone(account);
    let acct_id = account.id;

    let outcome: Result<RefreshOutcome, _> = state
        .refresh
        .refresh(acct_id, move || async move {
            let RefreshedTokens {
                access_token,
                refresh_token: new_refresh,
                expires_in_seconds,
                ..
            } = exchange_refresh_token(&http, &refresh_token).await?;
            let new_expires_at = Utc::now()
                + ChronoDuration::seconds(i64::try_from(expires_in_seconds).unwrap_or(i64::MAX));
            account_clone.update_anthropic_oauth_token(access_token, new_refresh, new_expires_at);
            Ok(RefreshOutcome {
                coalesced: false,
                new_expires_at,
            })
        })
        .await;

    match outcome {
        Ok(_) => {
            metrics::record_refresh_attempt(&acct_id.to_string(), true);
            account
                .anthropic_oauth_token()
                .ok_or_else(|| "post-refresh token missing".to_owned())
        }
        Err(e) => {
            metrics::record_refresh_attempt(&acct_id.to_string(), false);
            Err(e.to_string())
        }
    }
}
