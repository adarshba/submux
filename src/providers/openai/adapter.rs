use async_trait::async_trait;
use std::sync::Arc;
use url::Url;

use crate::core::{
    AccountHandle, AdapterError, NormalizedRequest, ProviderAdapter, ProviderHealth, ResponseStream,
};
use crate::providers::openai::chatgpt_session::CodexSessionAdapter;

/// `ProviderAdapter` impl wrapping a `CodexSessionAdapter`. The struct name
/// reflects the subscription tier we serve — OpenAI's Codex product behind
/// a ChatGPT Plus/Pro login — to avoid conflating it with the OpenAI public
/// API-key adapter (which lives elsewhere).
pub struct OpenAiSubscriptionAdapter {
    pub codex: Arc<CodexSessionAdapter>,
}

impl OpenAiSubscriptionAdapter {
    pub fn new(http: Arc<reqwest::Client>, upstream: Url) -> Self {
        Self {
            codex: Arc::new(CodexSessionAdapter::new(http, upstream)),
        }
    }

    /// Default client used when the server doesn't pre-build one.
    pub fn default_client() -> reqwest::Client {
        CodexSessionAdapter::default_client()
    }
}

#[async_trait]
impl ProviderAdapter for OpenAiSubscriptionAdapter {
    fn name(&self) -> &'static str {
        "openai-subscription"
    }

    async fn execute(
        &self,
        _request: NormalizedRequest,
        _account: AccountHandle,
    ) -> Result<ResponseStream, AdapterError> {
        Err(AdapterError::Internal(
            "normalized execute() not wired yet — use CodexSessionAdapter::passthrough()".into(),
        ))
    }

    async fn refresh_session(&self, _account: &AccountHandle) -> Result<(), AdapterError> {
        Err(AdapterError::Internal("refresh not implemented".into()))
    }

    async fn health_check(&self, _account: &AccountHandle) -> ProviderHealth {
        ProviderHealth {
            score: 1.0,
            last_success_at: None,
            last_failure_at: None,
            last_latency_ms: None,
            consecutive_failures: 0,
            note: None,
        }
    }
}
