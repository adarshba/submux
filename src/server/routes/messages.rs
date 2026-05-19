use axum::{
    body::Body,
    extract::State,
    http::{HeaderMap, StatusCode},
    response::Response,
    routing::post,
    Router,
};
use bytes::Bytes;
use chrono::{Duration as ChronoDuration, Utc};
use std::sync::Arc;
use std::time::Instant;

use crate::accounts::refresh::RefreshOutcome;
use crate::accounts::Account;
use crate::core::ProviderKind;
use crate::providers::anthropic::adapter::PassthroughResponse;
use crate::providers::anthropic::oauth::{exchange_refresh_token, RefreshedTokens};
use crate::providers::anthropic::quota::{apply_quota, parse_quota_headers};
use crate::server::app::AppState;
use crate::server::responses::{
    anthropic_error_response, record_terminal_metric, sniff_model_hint,
};
use crate::telemetry::events::RequestEvent;
use crate::telemetry::{metrics, tracer};

const PROVIDER: &str = "anthropic";

pub fn router() -> Router<AppState> {
    Router::new().route("/v1/messages", post(handle_messages))
}

async fn handle_messages(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let started = Instant::now();
    let request_id = tracer::new_request_id();
    let model_hint = sniff_model_hint(&body);
    let model_group = model_hint.clone().unwrap_or_else(|| "unknown".to_owned());

    state.events.publish(RequestEvent::Accepted {
        request_id: request_id.clone(),
        protocol: "anthropic".to_owned(),
        model_hint: model_hint.clone(),
        timestamp: Utc::now(),
    });

    let accounts = state.pool.by_provider(ProviderKind::AnthropicSubscription);
    let Some(account) = accounts.into_iter().next() else {
        record_terminal_metric(PROVIDER, &model_group, "no_account", started);
        return anthropic_error_response(
            StatusCode::SERVICE_UNAVAILABLE,
            "no Anthropic OAuth account registered — set SUBMUX_ANTHROPIC_OAUTH_TOKEN at startup",
        );
    };
    let Some(mut token) = account.anthropic_oauth_token() else {
        record_terminal_metric(PROVIDER, &model_group, "wrong_credential_kind", started);
        return anthropic_error_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            "account is not an Anthropic OAuth account",
        );
    };

    state.events.publish(RequestEvent::AccountPicked {
        request_id: request_id.clone(),
        account_id: account.id,
        provider: "anthropic".to_owned(),
        timestamp: Utc::now(),
    });
    state.events.publish(RequestEvent::UpstreamStart {
        request_id: request_id.clone(),
        account_id: account.id,
        timestamp: Utc::now(),
    });

    let mut passthrough = match state
        .anthropic
        .passthrough(&headers, body.clone(), &token)
        .await
    {
        Ok(p) => {
            apply_quota(&account, &state.cooldown, &parse_quota_headers(&p.headers)).await;
            p
        }
        Err(err) => {
            state.events.publish(RequestEvent::UpstreamError {
                request_id,
                account_id: Some(account.id),
                error: err.to_string(),
                timestamp: Utc::now(),
            });
            tracing::warn!(error = %err, "anthropic passthrough failed");
            record_terminal_metric(PROVIDER, &model_group, "upstream_error", started);
            return anthropic_error_response(
                StatusCode::BAD_GATEWAY,
                &format!("upstream error: {err}"),
            );
        }
    };

    if passthrough.status == StatusCode::UNAUTHORIZED && account.anthropic_refresh_token().is_some()
    {
        match refresh_account(&state, &account).await {
            Ok(new_token) => {
                token = new_token;
                state.events.publish(RequestEvent::UpstreamStart {
                    request_id: request_id.clone(),
                    account_id: account.id,
                    timestamp: Utc::now(),
                });
                match state.anthropic.passthrough(&headers, body, &token).await {
                    Ok(retry) => {
                        apply_quota(
                            &account,
                            &state.cooldown,
                            &parse_quota_headers(&retry.headers),
                        )
                        .await;
                        passthrough = retry;
                    }
                    Err(err) => {
                        state.events.publish(RequestEvent::UpstreamError {
                            request_id,
                            account_id: Some(account.id),
                            error: err.to_string(),
                            timestamp: Utc::now(),
                        });
                        record_terminal_metric(PROVIDER, &model_group, "upstream_error", started);
                        return anthropic_error_response(
                            StatusCode::BAD_GATEWAY,
                            &format!("retry after refresh failed: {err}"),
                        );
                    }
                }
            }
            Err(err) => {
                tracing::warn!(account = %account.id, error = %err, "refresh-on-401 failed");
            }
        }
    }

    let status = passthrough.status;
    state.events.publish(RequestEvent::UpstreamComplete {
        request_id,
        account_id: account.id,
        status: status.as_u16(),
        latency_ms: started.elapsed().as_millis() as u64,
        input_tokens: None,
        output_tokens: None,
        timestamp: Utc::now(),
    });
    record_terminal_metric(
        PROVIDER,
        &model_group,
        &status.as_u16().to_string(),
        started,
    );

    forward_response(passthrough)
}

fn forward_response(passthrough: PassthroughResponse) -> Response {
    let mut builder = Response::builder().status(passthrough.status);
    if let Some(response_headers) = builder.headers_mut() {
        for (name, value) in passthrough.headers.iter() {
            response_headers.append(name.clone(), value.clone());
        }
    }
    builder
        .body(Body::from_stream(passthrough.stream))
        .unwrap_or_else(|e| {
            tracing::error!(error = %e, "failed to build response body");
            anthropic_error_response(StatusCode::INTERNAL_SERVER_ERROR, "response build failed")
        })
}

async fn refresh_account(state: &AppState, account: &Arc<Account>) -> Result<String, String> {
    let refresh_token = account
        .anthropic_refresh_token()
        .ok_or_else(|| "account has no refresh token".to_owned())?;
    let http = Arc::clone(&state.http);
    let account_clone = Arc::clone(account);
    let acct_id = account.id;
    let events = Arc::clone(&state.events);

    let outcome: Result<RefreshOutcome, _> = state
        .refresh
        .refresh(acct_id, move || async move {
            let RefreshedTokens {
                access_token,
                refresh_token: new_refresh,
                expires_in_seconds,
                ..
            } = exchange_refresh_token(&http, &refresh_token).await?;
            let new_expires_at = Utc::now() + ChronoDuration::seconds(expires_in_seconds as i64);
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
            events.publish(RequestEvent::RefreshAttempt {
                account_id: acct_id,
                success: true,
                timestamp: Utc::now(),
            });
            account
                .anthropic_oauth_token()
                .ok_or_else(|| "post-refresh token missing".to_owned())
        }
        Err(e) => {
            metrics::record_refresh_attempt(&acct_id.to_string(), false);
            events.publish(RequestEvent::RefreshAttempt {
                account_id: acct_id,
                success: false,
                timestamp: Utc::now(),
            });
            Err(e.to_string())
        }
    }
}
