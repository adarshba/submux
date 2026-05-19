use chrono::{DateTime, Utc};
use moka::future::Cache;
use std::time::Duration;

use crate::core::AccountId;

/// Reason an account was placed in cooldown. Membership in the cache is
/// the source of truth; the `reason` field is metadata for admin
/// introspection and metrics.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CooldownReason {
    RateLimited,
    QuotaExhausted,
    Auth,
    ServerError,
    Manual,
    ChallengeRequired,
    /// Retained for backward compatibility with earlier scaffolding.
    NeedsReauth,
    InteractiveChallenge,
    PercentFailureThreshold,
    AllRequestsFailed,
    TransientNetwork,
}

/// One cooldown entry. `cooldown_until` is the authoritative "is this
/// account still cooled" timestamp — Moka's TTL is used as a best-effort
/// upper bound for eviction (see comment in `CooldownCache::cool`).
#[derive(Debug, Clone)]
pub struct CooldownEntry {
    pub reason: CooldownReason,
    pub status_code: Option<u16>,
    pub set_at: DateTime<Utc>,
    pub cooldown_until: DateTime<Utc>,
    /// Original requested cooldown duration (for backward compat /
    /// telemetry). Equals `cooldown_until - set_at`.
    pub cooldown_for: Duration,
    /// Alias of `status_code` retained for backward compat with the
    /// pre-refactor field name.
    pub source_error_code: Option<u16>,
}

/// LiteLLM-style cooldown cache, keyed per `AccountId`. Membership
/// (entry present AND `cooldown_until > now`) means "cooled".
///
/// Note: Moka does not support per-entry TTL via the standard builder
/// (only via `expire_after` policies). We use a generous fixed TTL for
/// auto-eviction (1 hour) and treat the `cooldown_until` field as the
/// real membership predicate. Stale entries past their `cooldown_until`
/// but still within the Moka TTL are treated as "not cooled".
#[derive(Clone)]
pub struct CooldownCache {
    l1: Cache<AccountId, CooldownEntry>,
}

impl std::fmt::Debug for CooldownCache {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CooldownCache")
            .field("entries", &self.l1.entry_count())
            .finish()
    }
}

impl CooldownCache {
    /// Build a new cache with the given max entry count. The Moka TTL is
    /// set to 1 hour; per-entry expiry is enforced by `cooldown_until`
    /// rather than the cache layer.
    pub fn new(capacity: u64) -> Self {
        Self {
            l1: Cache::builder()
                .max_capacity(capacity)
                .time_to_live(Duration::from_secs(60 * 60))
                .build(),
        }
    }

    /// Place an account in cooldown for `duration`. Overwrites any
    /// existing entry.
    pub async fn cool(
        &self,
        id: AccountId,
        reason: CooldownReason,
        duration: Duration,
        status_code: Option<u16>,
    ) {
        let now = Utc::now();
        let cooldown_until = now
            + chrono::Duration::from_std(duration)
                .unwrap_or_else(|_| chrono::Duration::seconds(60));
        let entry = CooldownEntry {
            reason,
            status_code,
            set_at: now,
            cooldown_until,
            cooldown_for: duration,
            source_error_code: status_code,
        };
        self.l1.insert(id, entry).await;
    }

    /// Backward-compat alias used by legacy callers — same semantics as
    /// `cool` but takes a prebuilt `CooldownEntry`.
    pub async fn set(&self, id: AccountId, entry: CooldownEntry) {
        self.l1.insert(id, entry).await;
    }

    /// True iff an entry exists and its `cooldown_until` is still in
    /// the future.
    pub async fn is_cooled(&self, id: AccountId) -> bool {
        match self.l1.get(&id).await {
            Some(entry) => entry.cooldown_until > Utc::now(),
            None => false,
        }
    }

    /// Fetch the entry (or `None`) without checking expiry. Callers
    /// that need the strict "is currently cooled" semantics should use
    /// `is_cooled`.
    pub async fn entry(&self, id: AccountId) -> Option<CooldownEntry> {
        self.l1.get(&id).await
    }

    /// Snapshot every entry. Intended for admin endpoints and
    /// diagnostics; do not call on the request hot path.
    pub async fn entries(&self) -> Vec<(AccountId, CooldownEntry)> {
        self.l1.iter().map(|(k, v)| (*k, v)).collect()
    }

    /// Forcibly clear a cooldown entry.
    pub async fn clear(&self, id: AccountId) {
        self.l1.invalidate(&id).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn cool_then_is_cooled_then_clear_roundtrip() {
        let cache = CooldownCache::new(16);
        let id = AccountId::new();

        assert!(!cache.is_cooled(id).await, "fresh account not cooled");

        cache
            .cool(
                id,
                CooldownReason::RateLimited,
                Duration::from_secs(60),
                Some(429),
            )
            .await;

        assert!(cache.is_cooled(id).await, "must be cooled right after cool");
        let entry = cache.entry(id).await.expect("entry present");
        assert_eq!(entry.reason, CooldownReason::RateLimited);
        assert_eq!(entry.status_code, Some(429));
        assert!(entry.cooldown_until > entry.set_at);

        cache.clear(id).await;
        assert!(!cache.is_cooled(id).await, "cleared entry not cooled");
        assert!(cache.entry(id).await.is_none());
    }

    #[tokio::test]
    async fn expired_entry_is_not_cooled() {
        let cache = CooldownCache::new(16);
        let id = AccountId::new();
        cache
            .cool(id, CooldownReason::Manual, Duration::from_millis(0), None)
            .await;
        tokio::time::sleep(Duration::from_millis(5)).await;
        assert!(!cache.is_cooled(id).await);
    }

    #[tokio::test]
    async fn entries_snapshot_returns_all() {
        let cache = CooldownCache::new(16);
        let id1 = AccountId::new();
        let id2 = AccountId::new();
        cache
            .cool(id1, CooldownReason::Auth, Duration::from_secs(30), None)
            .await;
        cache
            .cool(
                id2,
                CooldownReason::ServerError,
                Duration::from_secs(30),
                Some(503),
            )
            .await;
        cache.l1.run_pending_tasks().await;
        let snap = cache.entries().await;
        assert_eq!(snap.len(), 2);
    }
}
