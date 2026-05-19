use async_trait::async_trait;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::core::{AccountId, ProviderKind, SubmuxError};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StoredAccount {
    pub id: AccountId,
    pub provider: ProviderKind,
    pub display_name: String,
    pub fingerprint_json: serde_json::Value,
    pub sealed_credentials: Vec<u8>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[async_trait]
pub trait AccountStore: Send + Sync {
    async fn list(&self) -> Result<Vec<StoredAccount>, SubmuxError>;
    async fn load(&self, id: &AccountId) -> Result<Option<StoredAccount>, SubmuxError>;
    async fn upsert(&self, account: &StoredAccount) -> Result<(), SubmuxError>;
    async fn delete(&self, id: &AccountId) -> Result<(), SubmuxError>;
}
