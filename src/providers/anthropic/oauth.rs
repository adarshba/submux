//! Anthropic OAuth token endpoint client.
//!
//! Tries both canonical refresh endpoints in order; transient and 5xx failures
//! cascade to the next endpoint, terminal errors abort immediately.

use serde::{Deserialize, Serialize};

use crate::constants::upstream_paths::{
    ANTHROPIC_OAUTH_TOKEN_ENDPOINTS, CLAUDE_CODE_OAUTH_CLIENT_ID,
};
use crate::core::{AdapterError, TransientKind};

#[derive(Debug, Clone)]
pub struct RefreshedTokens {
    pub access_token: String,
    pub refresh_token: Option<String>,
    pub expires_in_seconds: u64,
    pub scope: Option<String>,
}

#[derive(Debug, Serialize)]
struct RefreshRequest<'a> {
    grant_type: &'static str,
    refresh_token: &'a str,
    client_id: &'static str,
}

#[derive(Debug, Deserialize)]
struct RefreshResponse {
    access_token: String,
    #[serde(default)]
    refresh_token: Option<String>,
    expires_in: u64,
    #[serde(default)]
    scope: Option<String>,
}

#[derive(Debug, Deserialize)]
struct TokenErrorResponse {
    error: String,
}

/// Exchange a `refresh_token` for a fresh `(access_token, refresh_token)` pair.
pub async fn exchange_refresh_token(
    http: &reqwest::Client,
    refresh_token: &str,
) -> Result<RefreshedTokens, AdapterError> {
    let body = RefreshRequest {
        grant_type: "refresh_token",
        refresh_token,
        client_id: CLAUDE_CODE_OAUTH_CLIENT_ID,
    };
    let body_json = serde_json::to_vec(&body)
        .map_err(|e| AdapterError::Internal(format!("serialize refresh body: {e}")))?;

    let mut last_transient: Option<AdapterError> = None;

    for endpoint in ANTHROPIC_OAUTH_TOKEN_ENDPOINTS {
        match attempt(http, endpoint, &body_json).await {
            Ok(tokens) => return Ok(tokens),
            Err(err @ AdapterError::Transient { .. }) => {
                tracing::warn!(
                    endpoint = endpoint,
                    error = %err,
                    "refresh transient — trying next endpoint"
                );
                last_transient = Some(err);
                continue;
            }
            Err(
                err @ AdapterError::Upstream {
                    status: 500..=599, ..
                },
            ) => {
                tracing::warn!(
                    endpoint = endpoint,
                    error = %err,
                    "refresh 5xx — trying next endpoint"
                );
                last_transient = Some(err);
                continue;
            }
            Err(other) => return Err(other),
        }
    }

    Err(last_transient.unwrap_or(AdapterError::Internal(
        "all refresh endpoints exhausted".into(),
    )))
}

async fn attempt(
    http: &reqwest::Client,
    endpoint: &str,
    body: &[u8],
) -> Result<RefreshedTokens, AdapterError> {
    let response = http
        .post(endpoint)
        .header(http::header::CONTENT_TYPE, "application/json")
        .header(http::header::ACCEPT, "application/json")
        .body(body.to_vec())
        .send()
        .await
        .map_err(map_send_error)?;

    let status = response.status();
    let bytes = response
        .bytes()
        .await
        .map_err(|e| AdapterError::Transient {
            cause: if e.is_timeout() {
                TransientKind::ReadTimeout
            } else {
                TransientKind::Reset
            },
        })?;

    if status.is_success() {
        let parsed: RefreshResponse = serde_json::from_slice(&bytes)
            .map_err(|e| AdapterError::Internal(format!("parse refresh success body: {e}")))?;
        return Ok(RefreshedTokens {
            access_token: parsed.access_token,
            refresh_token: parsed.refresh_token,
            expires_in_seconds: parsed.expires_in,
            scope: parsed.scope,
        });
    }

    if let Ok(err) = serde_json::from_slice::<TokenErrorResponse>(&bytes) {
        if matches!(
            err.error.as_str(),
            "invalid_grant" | "invalid_token" | "unauthorized_client"
        ) {
            return Err(AdapterError::TokenInvalid { needs_reauth: true });
        }
    }

    Err(AdapterError::Upstream {
        status: status.as_u16(),
        body: bytes,
    })
}

fn map_send_error(e: reqwest::Error) -> AdapterError {
    if e.is_timeout() {
        AdapterError::Transient {
            cause: TransientKind::ReadTimeout,
        }
    } else if e.is_connect() {
        AdapterError::Transient {
            cause: TransientKind::Connect,
        }
    } else if e.is_request() {
        AdapterError::Internal(format!("request build: {e}"))
    } else {
        AdapterError::Transient {
            cause: TransientKind::Other,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn refresh_request_body_shape() {
        let body = RefreshRequest {
            grant_type: "refresh_token",
            refresh_token: "rt-abc",
            client_id: CLAUDE_CODE_OAUTH_CLIENT_ID,
        };
        let json: serde_json::Value =
            serde_json::from_slice(&serde_json::to_vec(&body).unwrap()).unwrap();
        assert_eq!(json["grant_type"], "refresh_token");
        assert_eq!(json["refresh_token"], "rt-abc");
        assert_eq!(json["client_id"], CLAUDE_CODE_OAUTH_CLIENT_ID);
    }

    #[test]
    fn endpoints_contain_both_hosts() {
        assert!(ANTHROPIC_OAUTH_TOKEN_ENDPOINTS
            .iter()
            .any(|e| e.contains("api.anthropic.com")));
        assert!(ANTHROPIC_OAUTH_TOKEN_ENDPOINTS
            .iter()
            .any(|e| e.contains("console.anthropic.com")));
    }
}
