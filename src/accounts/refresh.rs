//! Singleflight refresh manager.
//!
//! When N concurrent requests hit a 401 simultaneously we must coalesce all
//! refresh attempts into one in-flight task. Each waiter then sees the same
//! result. Built on `futures::future::Shared` keyed by `AccountId`.

use dashmap::DashMap;
use futures::FutureExt;
use futures::future::{BoxFuture, Shared};
use std::sync::Arc;

use crate::core::{AccountId, AdapterError};

type RefreshOutput = Result<RefreshOutcome, Arc<AdapterError>>;
type SharedRefresh = Shared<BoxFuture<'static, RefreshOutput>>;

/// Returned to the caller after a refresh completes successfully. The token
/// has already been applied to the account session by the refresh closure;
/// this is metadata the caller can log or observe.
#[derive(Debug, Clone)]
pub struct RefreshOutcome {
    pub coalesced: bool,
    pub new_expires_at: chrono::DateTime<chrono::Utc>,
}

#[derive(Default)]
pub struct RefreshManager {
    in_flight: DashMap<AccountId, SharedRefresh>,
}

impl RefreshManager {
    pub fn new() -> Self {
        Self::default()
    }

    /// Coalesce a refresh for `account` — only one `factory()` runs even
    /// under heavy contention. The returned future resolves to the same
    /// `RefreshOutput` for every concurrent waiter.
    ///
    /// On error, the in-flight entry is dropped so the next caller triggers
    /// a fresh attempt (don't poison the cache with permanent failure).
    pub async fn refresh<F, Fut>(&self, account: AccountId, factory: F) -> RefreshOutput
    where
        F: FnOnce() -> Fut,
        Fut: std::future::Future<Output = Result<RefreshOutcome, AdapterError>> + Send + 'static,
    {
        let (shared, we_own_it) = {
            let entry = self.in_flight.entry(account);
            match entry {
                dashmap::mapref::entry::Entry::Occupied(o) => (o.get().clone(), false),
                dashmap::mapref::entry::Entry::Vacant(v) => {
                    let owned: SharedRefresh =
                        factory().map(|r| r.map_err(Arc::new)).boxed().shared();
                    v.insert(owned.clone());
                    (owned, true)
                }
            }
        };

        let result = shared.await;

        if we_own_it {
            self.in_flight.remove(&account);
        }

        match result {
            Ok(mut outcome) => {
                if !we_own_it {
                    outcome.coalesced = true;
                }
                Ok(outcome)
            }
            Err(e) => Err(e),
        }
    }

    pub fn in_flight_count(&self) -> usize {
        self.in_flight.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU32, Ordering};

    #[tokio::test]
    async fn singleflight_coalesces_concurrent_callers() {
        let mgr = Arc::new(RefreshManager::new());
        let counter = Arc::new(AtomicU32::new(0));
        let id = AccountId::new();

        let mut handles = Vec::new();
        for _ in 0..50 {
            let mgr = Arc::clone(&mgr);
            let counter = Arc::clone(&counter);
            handles.push(tokio::spawn(async move {
                mgr.refresh(id, || {
                    let counter = Arc::clone(&counter);
                    async move {
                        counter.fetch_add(1, Ordering::Relaxed);
                        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
                        Ok(RefreshOutcome {
                            coalesced: false,
                            new_expires_at: chrono::Utc::now() + chrono::Duration::hours(1),
                        })
                    }
                })
                .await
            }));
        }
        for h in handles {
            h.await.unwrap().expect("refresh ok");
        }
        assert_eq!(counter.load(Ordering::Relaxed), 1);
    }

    #[tokio::test]
    async fn error_clears_in_flight_so_retry_kicks_a_new_factory() {
        let mgr = Arc::new(RefreshManager::new());
        let id = AccountId::new();

        let _ = mgr
            .refresh(id, || async {
                Err(AdapterError::Internal("first attempt".into()))
            })
            .await;
        assert_eq!(mgr.in_flight_count(), 0);

        let outcome = mgr
            .refresh(id, || async {
                Ok(RefreshOutcome {
                    coalesced: false,
                    new_expires_at: chrono::Utc::now() + chrono::Duration::hours(1),
                })
            })
            .await
            .expect("second attempt ok");
        assert!(!outcome.coalesced);
    }
}
