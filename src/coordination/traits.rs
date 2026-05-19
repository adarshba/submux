//! Trait surface for cross-replica coordination. See module-level docs.

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use std::sync::Arc;
use tokio::sync::broadcast;

use crate::core::AccountId;
use crate::router::cooldown::CooldownReason;

/// Announcement that a single replica has placed an account in cooldown.
/// Other replicas mirror the cooldown locally so they don't pick the
/// account until it heals.
#[derive(Debug, Clone)]
pub struct CooldownAnnouncement {
    pub account_id: AccountId,
    pub reason: CooldownReason,
    pub status_code: Option<u16>,
    pub set_at: DateTime<Utc>,
    pub cooldown_until: DateTime<Utc>,
}

/// RAII handle for an acquired cross-replica refresh lease. Dropping it
/// releases the lease so other replicas can take the next refresh slot.
pub struct RefreshLeaseGuard {
    pub release: Box<dyn FnOnce() + Send + Sync>,
}

impl RefreshLeaseGuard {
    /// Construct a guard that does nothing on drop. Useful in tests and
    /// as a sentinel for "already-released" states.
    pub fn noop() -> Self {
        Self {
            release: Box::new(|| {}),
        }
    }
}

impl std::fmt::Debug for RefreshLeaseGuard {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RefreshLeaseGuard").finish_non_exhaustive()
    }
}

impl Drop for RefreshLeaseGuard {
    fn drop(&mut self) {
        let release = std::mem::replace(&mut self.release, Box::new(|| {}));
        release();
    }
}

/// Cross-replica coordination plane: cooldown fan-out, refresh
/// singleflight leasing, and cookie-jar pubsub.
///
/// Implementations must be cheap to clone via `Arc` and safe to share
/// across the whole process.
#[async_trait]
pub trait CoordinationBackend: Send + Sync + 'static {
    /// Announce that this replica has placed `account_id` into cooldown.
    /// Other replicas should mirror the cooldown in their local L1 cache.
    async fn announce_cooldown(&self, ann: CooldownAnnouncement);

    /// Subscribe to cooldown announcements from other replicas. The
    /// returned receiver yields every announcement in fan-out order.
    fn subscribe_cooldowns(&self) -> broadcast::Receiver<CooldownAnnouncement>;

    /// Try to acquire the cross-replica refresh lease for `account_id`.
    /// Returns `Some(guard)` iff we got the lease, `None` if another
    /// replica holds it (caller should poll/wait for the announcement).
    async fn try_acquire_refresh_lease(
        &self,
        account_id: AccountId,
        lease_ttl: std::time::Duration,
    ) -> Option<RefreshLeaseGuard>;

    /// Publish a fresh serialized cookie jar so peers can mirror it.
    async fn publish_cookie_jar(&self, account_id: AccountId, sealed_jar_blob: Vec<u8>);

    /// Subscribe to cookie-jar updates from peers.
    fn subscribe_cookie_jars(&self) -> broadcast::Receiver<(AccountId, Vec<u8>)>;
}

/// Convenience alias for a shared coordinator handle.
pub type SharedCoordinator = Arc<dyn CoordinationBackend>;
