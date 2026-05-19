//! Single-replica in-process implementation of [`CoordinationBackend`].
//!
//! All fan-out is via `tokio::sync::broadcast` and the refresh-lease map
//! is a plain `DashMap`. This is the default backend for single-replica
//! deployments and the test double for the rest of the system.

use async_trait::async_trait;
use dashmap::DashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::broadcast;

use crate::core::AccountId;

use super::traits::{CooldownAnnouncement, CoordinationBackend, RefreshLeaseGuard};

/// In-process coordinator. Fans out announcements via local broadcast
/// channels and gates refresh leases with a `DashMap`.
pub struct InMemoryCoordinator {
    cooldown_tx: broadcast::Sender<CooldownAnnouncement>,
    cookie_tx: broadcast::Sender<(AccountId, Vec<u8>)>,
    /// Active leases keyed by account; value is the lease expiration.
    leases: Arc<DashMap<AccountId, Instant>>,
}

impl InMemoryCoordinator {
    /// Build a new coordinator with the given broadcast buffer capacity.
    pub fn new(capacity: usize) -> Self {
        let (cooldown_tx, _) = broadcast::channel(capacity);
        let (cookie_tx, _) = broadcast::channel(capacity);
        Self {
            cooldown_tx,
            cookie_tx,
            leases: Arc::new(DashMap::new()),
        }
    }

    /// Default broadcast capacity. Chosen large enough that bursts during
    /// a mass-cooldown event don't drop subscribers under normal load.
    pub const fn default_capacity() -> usize {
        2048
    }
}

impl Default for InMemoryCoordinator {
    fn default() -> Self {
        Self::new(Self::default_capacity())
    }
}

impl std::fmt::Debug for InMemoryCoordinator {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("InMemoryCoordinator")
            .field("active_leases", &self.leases.len())
            .field("cooldown_subscribers", &self.cooldown_tx.receiver_count())
            .field("cookie_subscribers", &self.cookie_tx.receiver_count())
            .finish()
    }
}

#[async_trait]
impl CoordinationBackend for InMemoryCoordinator {
    async fn announce_cooldown(&self, ann: CooldownAnnouncement) {
        let _ = self.cooldown_tx.send(ann);
    }

    fn subscribe_cooldowns(&self) -> broadcast::Receiver<CooldownAnnouncement> {
        self.cooldown_tx.subscribe()
    }

    async fn try_acquire_refresh_lease(
        &self,
        account_id: AccountId,
        lease_ttl: Duration,
    ) -> Option<RefreshLeaseGuard> {
        let now = Instant::now();
        let new_expiry = now + lease_ttl;

        let mut acquired = false;
        match self.leases.entry(account_id) {
            dashmap::mapref::entry::Entry::Vacant(slot) => {
                slot.insert(new_expiry);
                acquired = true;
            }
            dashmap::mapref::entry::Entry::Occupied(mut slot) => {
                if *slot.get() <= now {
                    slot.insert(new_expiry);
                    acquired = true;
                }
            }
        }

        if !acquired {
            return None;
        }

        let leases = Arc::clone(&self.leases);
        Some(RefreshLeaseGuard {
            release: Box::new(move || {
                leases.remove_if(&account_id, |_, exp| *exp == new_expiry);
            }),
        })
    }

    async fn publish_cookie_jar(&self, account_id: AccountId, sealed_jar_blob: Vec<u8>) {
        let _ = self.cookie_tx.send((account_id, sealed_jar_blob));
    }

    fn subscribe_cookie_jars(&self) -> broadcast::Receiver<(AccountId, Vec<u8>)> {
        self.cookie_tx.subscribe()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::router::cooldown::CooldownReason;
    use chrono::Utc;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;
    use tokio::time::{sleep, Duration as TokioDuration};

    fn sample_announcement(id: AccountId) -> CooldownAnnouncement {
        let now = Utc::now();
        CooldownAnnouncement {
            account_id: id,
            reason: CooldownReason::RateLimited,
            status_code: Some(429),
            set_at: now,
            cooldown_until: now + chrono::Duration::seconds(30),
        }
    }

    #[tokio::test]
    async fn cooldown_broadcast_to_subscriber() {
        let coord = InMemoryCoordinator::default();
        let mut rx = coord.subscribe_cooldowns();
        let id = AccountId::new();
        let ann = sample_announcement(id);

        coord.announce_cooldown(ann.clone()).await;

        let received = rx.recv().await.expect("should receive announcement");
        assert_eq!(received.account_id, ann.account_id);
        assert_eq!(received.status_code, ann.status_code);
        assert_eq!(received.reason, ann.reason);
    }

    #[tokio::test]
    async fn refresh_lease_singleflight() {
        let coord = Arc::new(InMemoryCoordinator::default());
        let id = AccountId::new();
        let winners = Arc::new(AtomicUsize::new(0));

        let mut handles = Vec::with_capacity(50);
        for _ in 0..50 {
            let coord = Arc::clone(&coord);
            let winners = Arc::clone(&winners);
            handles.push(tokio::spawn(async move {
                let guard = coord
                    .try_acquire_refresh_lease(id, Duration::from_secs(30))
                    .await;
                if let Some(g) = guard {
                    winners.fetch_add(1, Ordering::SeqCst);
                    sleep(TokioDuration::from_millis(50)).await;
                    drop(g);
                }
            }));
        }

        for h in handles {
            h.await.unwrap();
        }
        assert_eq!(winners.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn refresh_lease_releases_on_drop() {
        let coord = InMemoryCoordinator::default();
        let id = AccountId::new();

        let guard = coord
            .try_acquire_refresh_lease(id, Duration::from_secs(60))
            .await
            .expect("first acquire should succeed");
        assert!(coord
            .try_acquire_refresh_lease(id, Duration::from_secs(60))
            .await
            .is_none());
        drop(guard);

        assert!(coord
            .try_acquire_refresh_lease(id, Duration::from_secs(60))
            .await
            .is_some());
    }

    #[tokio::test]
    async fn refresh_lease_expires() {
        let coord = InMemoryCoordinator::default();
        let id = AccountId::new();

        let g = coord
            .try_acquire_refresh_lease(id, Duration::from_millis(50))
            .await
            .expect("initial acquire");
        std::mem::forget(g);

        sleep(TokioDuration::from_millis(100)).await;

        assert!(coord
            .try_acquire_refresh_lease(id, Duration::from_secs(30))
            .await
            .is_some());
    }

    #[tokio::test]
    async fn cookie_jar_broadcast() {
        let coord = InMemoryCoordinator::default();
        let mut rx = coord.subscribe_cookie_jars();
        let id = AccountId::new();
        let blob = vec![1u8, 2, 3, 4, 5];

        coord.publish_cookie_jar(id, blob.clone()).await;

        let (recv_id, recv_blob) = rx.recv().await.expect("should receive jar");
        assert_eq!(recv_id, id);
        assert_eq!(recv_blob, blob);
    }
}
