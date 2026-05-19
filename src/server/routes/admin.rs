use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use serde_json::{json, Value};
use std::str::FromStr;
use std::sync::atomic::Ordering;
use ulid::Ulid;

use crate::core::{AccountId, ProviderKind};
use crate::router::cooldown::CooldownReason;
use crate::server::app::AppState;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/admin/accounts", get(list_accounts))
        .route("/admin/cooldowns", get(list_cooldowns))
        .route("/admin/cooldowns/clear/:account_id", post(clear_cooldown))
}

async fn list_accounts(State(state): State<AppState>) -> Json<Value> {
    let now = chrono::Utc::now();
    let accounts: Vec<Value> = state
        .pool
        .all()
        .into_iter()
        .map(|acc| {
            let s = &acc.state;
            let cooldown_until = s.cooldown_until.load_full().map(|t| *t);
            let cooldown_active = cooldown_until.map(|t| t > now).unwrap_or(false);
            json!({
                "id": acc.id.to_string(),
                "provider": provider_str(acc.provider),
                "display_name": acc.display_name,
                "health": s.health.load_full().map(|h| *h),
                "in_flight": s.in_flight.load(Ordering::Relaxed),
                "cooldown_active": cooldown_active,
                "cooldown_until": cooldown_until.map(|t| t.to_rfc3339()),
                "needs_reauth": s.needs_reauth.load(Ordering::Relaxed),
                "permanently_disabled": s.permanently_disabled.load(Ordering::Relaxed),
                "quota_5h_util": s.quota_5h_util.load_full().map(|q| *q),
                "quota_7d_util": s.quota_7d_util.load_full().map(|q| *q),
                "quota_5h_reset_at": s.quota_5h_reset_at.load_full().map(|t| t.to_rfc3339()),
            })
        })
        .collect();
    Json(Value::Array(accounts))
}

async fn list_cooldowns(State(state): State<AppState>) -> Json<Value> {
    let entries = state.cooldown.entries().await;
    let payload: Vec<Value> = entries
        .into_iter()
        .map(|(id, e)| {
            json!({
                "account_id": id.to_string(),
                "reason": cooldown_reason_str(e.reason),
                "status_code": e.status_code,
                "set_at": e.set_at.to_rfc3339(),
                "cooldown_until": e.cooldown_until.to_rfc3339(),
            })
        })
        .collect();
    Json(Value::Array(payload))
}

async fn clear_cooldown(State(state): State<AppState>, Path(account_id): Path<String>) -> Response {
    let Ok(ulid) = Ulid::from_str(&account_id) else {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({
                "error": {
                    "type": "submux_admin_error",
                    "message": "invalid account_id: must be a ULID",
                }
            })),
        )
            .into_response();
    };
    state.cooldown.clear(AccountId(ulid)).await;
    StatusCode::NO_CONTENT.into_response()
}

fn provider_str(p: ProviderKind) -> &'static str {
    match p {
        ProviderKind::AnthropicSubscription => "anthropic_subscription",
        ProviderKind::OpenAiSubscription => "openai_subscription",
        ProviderKind::AnthropicApiKey => "anthropic_api_key",
        ProviderKind::OpenAiApiKey => "openai_api_key",
        ProviderKind::Custom => "custom",
    }
}

fn cooldown_reason_str(r: CooldownReason) -> &'static str {
    match r {
        CooldownReason::RateLimited => "RateLimited",
        CooldownReason::QuotaExhausted => "QuotaExhausted",
        CooldownReason::Auth => "Auth",
        CooldownReason::ServerError => "ServerError",
        CooldownReason::Manual => "Manual",
        CooldownReason::ChallengeRequired => "ChallengeRequired",
        CooldownReason::NeedsReauth => "NeedsReauth",
        CooldownReason::InteractiveChallenge => "InteractiveChallenge",
        CooldownReason::PercentFailureThreshold => "PercentFailureThreshold",
        CooldownReason::AllRequestsFailed => "AllRequestsFailed",
        CooldownReason::TransientNetwork => "TransientNetwork",
    }
}
