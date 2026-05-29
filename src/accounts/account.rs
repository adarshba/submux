use arc_swap::ArcSwapOption;
use chrono::{DateTime, Utc};
use parking_lot::RwLock;
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU32};
use std::sync::Arc;
use tokio::sync::{Mutex, Semaphore};
use uuid::Uuid;

use crate::core::{
    AccountId, Credentials, FingerprintProfile, ProviderKind, SerializedCookieJar, Session,
};

pub struct Account {
    pub id: AccountId,
    pub provider: ProviderKind,
    pub display_name: String,
    pub fingerprint: RwLock<FingerprintProfile>,
    pub session: RwLock<Session>,
    pub state: Arc<AccountState>,
}

impl Account {
    /// Seed an Anthropic OAuth account from a static access token. Used by
    /// the bootstrap path (env var / config file) before the refresh
    /// manager is wired up; expiry is set far in the future so the
    /// passthrough path doesn't try to refresh.
    pub fn new_anthropic_oauth(
        display_name: impl Into<String>,
        access_token: impl Into<String>,
        refresh_token: Option<String>,
    ) -> Self {
        let credentials = Credentials::AnthropicOAuth {
            access_token: access_token.into(),
            refresh_token: refresh_token.unwrap_or_default(),
            expires_at: Utc::now() + chrono::Duration::days(365),
            scopes: vec!["user:inference".to_owned()],
            device_id: Uuid::new_v4(),
            account_uuid: Uuid::new_v4(),
            session_id: Uuid::new_v4(),
        };
        let fingerprint = FingerprintProfile {
            name: "claude-code".to_owned(),
            version: "2.1.87".to_owned(),
            user_agent: "claude-cli/2.1.87 (external, cli)".to_owned(),
            stainless_headers: HashMap::new(),
            anthropic_betas: vec!["oauth-2025-04-20".to_owned()],
        };
        let session = Session {
            credentials,
            fingerprint: fingerprint.clone(),
        };
        Self {
            id: AccountId::new(),
            provider: ProviderKind::AnthropicSubscription,
            display_name: display_name.into(),
            fingerprint: RwLock::new(fingerprint),
            session: RwLock::new(session),
            state: Arc::new(AccountState::new(64)),
        }
    }

    /// Returns the OAuth access token if this account holds Anthropic
    /// OAuth credentials.
    pub fn anthropic_oauth_token(&self) -> Option<String> {
        match &self.session.read().credentials {
            Credentials::AnthropicOAuth { access_token, .. } => Some(access_token.clone()),
            _ => None,
        }
    }

    /// Returns the OAuth refresh token if this account holds Anthropic
    /// OAuth credentials.
    pub fn anthropic_refresh_token(&self) -> Option<String> {
        match &self.session.read().credentials {
            Credentials::AnthropicOAuth { refresh_token, .. } => {
                if refresh_token.is_empty() {
                    None
                } else {
                    Some(refresh_token.clone())
                }
            }
            _ => None,
        }
    }

    /// Returns whether the cached access token expires within `slack`. Used
    /// by the adapter to refresh proactively before sending a request.
    pub fn anthropic_oauth_expires_within(&self, slack: chrono::Duration) -> bool {
        match &self.session.read().credentials {
            Credentials::AnthropicOAuth { expires_at, .. } => *expires_at - Utc::now() <= slack,
            _ => false,
        }
    }

    /// Replace the stored OAuth tokens after a successful refresh. The
    /// refresh endpoint may or may not rotate the refresh token; pass
    /// `None` to keep the existing one.
    pub fn update_anthropic_oauth_token(
        &self,
        new_access: String,
        new_refresh: Option<String>,
        new_expires_at: DateTime<Utc>,
    ) {
        let mut session = self.session.write();
        if let Credentials::AnthropicOAuth {
            access_token,
            refresh_token,
            expires_at,
            ..
        } = &mut session.credentials
        {
            *access_token = new_access;
            if let Some(rt) = new_refresh {
                *refresh_token = rt;
            }
            *expires_at = new_expires_at;
        }
    }

    /// Seed an OpenAI (Codex / ChatGPT subscription) account from a
    /// captured session — bearer + cookie jar + device id. Mirrors
    /// `new_anthropic_oauth`: expiry is set far in the future so the
    /// passthrough path doesn't try to refresh before the refresh manager
    /// is wired up.
    pub fn new_openai_subscription(
        display_name: impl Into<String>,
        access_token: impl Into<String>,
        cookies: SerializedCookieJar,
        device_id: impl Into<String>,
    ) -> Self {
        let refresh_endpoint = "https://chatgpt.com/backend-api/codex/auth/refresh"
            .parse()
            .expect("static codex refresh endpoint is a valid URL");
        let credentials = Credentials::OpenAiChatGptSession {
            access_token: access_token.into(),
            cookies,
            device_id: device_id.into(),
            expires_at: Utc::now() + chrono::Duration::days(30),
            refresh_endpoint,
        };
        let fingerprint = FingerprintProfile {
            name: "codex-cli".to_owned(),
            version: "0.30.0".to_owned(),
            user_agent: "codex-cli/0.30.0 (Mac OS X; arm64)".to_owned(),
            stainless_headers: HashMap::new(),
            anthropic_betas: Vec::new(),
        };
        let session = Session {
            credentials,
            fingerprint: fingerprint.clone(),
        };
        Self {
            id: AccountId::new(),
            provider: ProviderKind::OpenAiSubscription,
            display_name: display_name.into(),
            fingerprint: RwLock::new(fingerprint),
            session: RwLock::new(session),
            state: Arc::new(AccountState::new(64)),
        }
    }

    /// Returns `(access_token, cookies, device_id)` if this account holds
    /// OpenAI Codex subscription credentials. The cookie jar is cloned so
    /// callers can pass it to `cookie_header_value` without holding the
    /// account lock across an await point.
    pub fn openai_credentials(&self) -> Option<(String, SerializedCookieJar, String)> {
        match &self.session.read().credentials {
            Credentials::OpenAiChatGptSession {
                access_token,
                cookies,
                device_id,
                ..
            } => Some((access_token.clone(), cookies.clone(), device_id.clone())),
            _ => None,
        }
    }
}

pub struct AccountState {
    pub health: ArcSwapOption<f32>,
    pub cooldown_until: ArcSwapOption<DateTime<Utc>>,
    pub last_429_at: ArcSwapOption<DateTime<Utc>>,
    pub last_used_at: ArcSwapOption<DateTime<Utc>>,
    pub consecutive_refresh_failures: AtomicU32,
    pub permanently_disabled: AtomicBool,
    pub needs_reauth: AtomicBool,
    pub in_flight: AtomicU32,
    pub quota_5h_util: ArcSwapOption<f32>,
    pub quota_7d_util: ArcSwapOption<f32>,
    pub quota_5h_reset_at: ArcSwapOption<DateTime<Utc>>,
    pub semaphore: Semaphore,
    pub refresh_lock: Mutex<()>,
}

impl AccountState {
    pub fn new(max_concurrent_per_account: usize) -> Self {
        Self {
            health: ArcSwapOption::empty(),
            cooldown_until: ArcSwapOption::empty(),
            last_429_at: ArcSwapOption::empty(),
            last_used_at: ArcSwapOption::empty(),
            consecutive_refresh_failures: AtomicU32::new(0),
            permanently_disabled: AtomicBool::new(false),
            needs_reauth: AtomicBool::new(false),
            in_flight: AtomicU32::new(0),
            quota_5h_util: ArcSwapOption::empty(),
            quota_7d_util: ArcSwapOption::empty(),
            quota_5h_reset_at: ArcSwapOption::empty(),
            semaphore: Semaphore::new(max_concurrent_per_account),
            refresh_lock: Mutex::new(()),
        }
    }
}
