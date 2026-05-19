use dashmap::DashMap;
use std::sync::Arc;

use crate::accounts::account::Account;
use crate::core::{AccountId, ProviderKind};

#[derive(Default)]
pub struct AccountPool {
    by_id: DashMap<AccountId, Arc<Account>>,
}

impl AccountPool {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn insert(&self, account: Arc<Account>) {
        self.by_id.insert(account.id, account);
    }

    pub fn get(&self, id: &AccountId) -> Option<Arc<Account>> {
        self.by_id.get(id).map(|e| Arc::clone(&*e))
    }

    pub fn all(&self) -> Vec<Arc<Account>> {
        self.by_id.iter().map(|e| Arc::clone(&*e)).collect()
    }

    pub fn by_provider(&self, provider: ProviderKind) -> Vec<Arc<Account>> {
        self.by_id
            .iter()
            .filter(|e| e.provider == provider)
            .map(|e| Arc::clone(&*e))
            .collect()
    }
}
