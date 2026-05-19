use serde::{Deserialize, Serialize};
use std::sync::Arc;
use ulid::Ulid;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct AccountId(pub Ulid);

impl AccountId {
    pub fn new() -> Self {
        Self(Ulid::new())
    }
}

impl Default for AccountId {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Display for AccountId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(f)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProviderKind {
    AnthropicSubscription,
    OpenAiSubscription,
    AnthropicApiKey,
    OpenAiApiKey,
    Custom,
}

pub struct AccountHandle {
    pub id: AccountId,
    pub provider: ProviderKind,
    pub inner: Arc<dyn AccountInner>,
}

pub trait AccountInner: Send + Sync {
    fn id(&self) -> AccountId;
    fn provider(&self) -> ProviderKind;
    fn is_healthy(&self) -> bool;
}

impl std::fmt::Debug for AccountHandle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AccountHandle")
            .field("id", &self.id)
            .field("provider", &self.provider)
            .finish()
    }
}
