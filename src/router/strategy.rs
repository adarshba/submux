use serde::{Deserialize, Serialize};
use std::sync::Arc;

use crate::accounts::Account;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum StrategyKind {
    RoundRobin,
    LeastRecentlyUsed,
    QuotaAware,
    LatencyAware,
    HealthAware,
}

/// A strategy picks one account out of a non-empty candidate slice. The
/// candidates are assumed to have already been filtered for cooldown /
/// disabled status by the router.
///
/// Implementations are synchronous and cheap; selection runs on the hot
/// path of every request. Strategies that need richer signals (latency,
/// load) read from the `Account`'s `AccountState` atomics.
pub trait RoutingStrategy: Send + Sync + std::fmt::Debug {
    /// Human-readable name for logs and metrics.
    fn name(&self) -> &'static str;

    /// Strategy enum tag — kept in sync with the config surface.
    fn kind(&self) -> StrategyKind;

    /// Pick one candidate. Returns `None` only if `candidates` is empty.
    fn pick<'a>(&self, candidates: &'a [Arc<Account>]) -> Option<&'a Arc<Account>>;
}
