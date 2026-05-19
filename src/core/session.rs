use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use url::Url;
use uuid::Uuid;

#[derive(Debug, Clone)]
pub struct Session {
    pub credentials: Credentials,
    pub fingerprint: FingerprintProfile,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Credentials {
    AnthropicOAuth {
        access_token: String,
        refresh_token: String,
        expires_at: DateTime<Utc>,
        scopes: Vec<String>,
        device_id: Uuid,
        account_uuid: Uuid,
        session_id: Uuid,
    },
    OpenAiChatGptSession {
        access_token: String,
        cookies: SerializedCookieJar,
        device_id: String,
        expires_at: DateTime<Utc>,
        refresh_endpoint: Url,
    },
    ApiKey {
        key: String,
        source: KeySource,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum KeySource {
    Env,
    Operator,
    Imported,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SerializedCookieJar {
    pub jar: HashMap<String, Vec<SerializedCookie>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SerializedCookie {
    pub name: String,
    pub value: String,
    pub path: String,
    pub secure: bool,
    pub http_only: bool,
    pub expires_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FingerprintProfile {
    pub name: String,
    pub version: String,
    pub user_agent: String,
    pub stainless_headers: HashMap<String, String>,
    pub anthropic_betas: Vec<String>,
}
