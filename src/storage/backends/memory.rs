use async_trait::async_trait;
use std::collections::HashMap;
use tokio::sync::RwLock;

use crate::core::{AccountId, SubmuxError};
use crate::storage::account_store::{AccountStore, StoredAccount};

#[derive(Default)]
pub struct MemoryAccountStore {
    inner: RwLock<HashMap<AccountId, StoredAccount>>,
}

#[async_trait]
impl AccountStore for MemoryAccountStore {
    async fn list(&self) -> Result<Vec<StoredAccount>, SubmuxError> {
        Ok(self.inner.read().await.values().cloned().collect())
    }

    async fn load(&self, id: &AccountId) -> Result<Option<StoredAccount>, SubmuxError> {
        Ok(self.inner.read().await.get(id).cloned())
    }

    async fn upsert(&self, account: &StoredAccount) -> Result<(), SubmuxError> {
        self.inner.write().await.insert(account.id, account.clone());
        Ok(())
    }

    async fn delete(&self, id: &AccountId) -> Result<(), SubmuxError> {
        self.inner.write().await.remove(id);
        Ok(())
    }
}
