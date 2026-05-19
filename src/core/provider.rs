use async_trait::async_trait;
use chrono::{DateTime, Utc};

use crate::core::{AccountHandle, AdapterError, NormalizedRequest, ResponseStream};

#[derive(Debug, Clone)]
pub struct ProviderHealth {
    pub score: f32,
    pub last_success_at: Option<DateTime<Utc>>,
    pub last_failure_at: Option<DateTime<Utc>>,
    pub last_latency_ms: Option<u32>,
    pub consecutive_failures: u32,
    pub note: Option<String>,
}

#[async_trait]
pub trait ProviderAdapter: Send + Sync {
    fn name(&self) -> &'static str;

    async fn execute(
        &self,
        request: NormalizedRequest,
        account: AccountHandle,
    ) -> Result<ResponseStream, AdapterError>;

    async fn refresh_session(&self, account: &AccountHandle) -> Result<(), AdapterError>;

    async fn health_check(&self, account: &AccountHandle) -> ProviderHealth;
}
