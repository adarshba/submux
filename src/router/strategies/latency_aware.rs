use std::sync::Arc;

use crate::accounts::Account;
use crate::router::strategy::{RoutingStrategy, StrategyKind};

/// Latency-aware selection. AccountState does not yet carry a latency
/// EWMA field, so for now this delegates to LRU as a coarse proxy
/// (cold/idle accounts get traffic, busy accounts cool off).
///
// TODO: replace with real EWMA when AccountState carries it.
#[derive(Debug, Default)]
pub struct LatencyAware;

impl RoutingStrategy for LatencyAware {
    fn name(&self) -> &'static str {
        "latency-aware"
    }

    fn kind(&self) -> StrategyKind {
        StrategyKind::LatencyAware
    }

    fn pick<'a>(&self, candidates: &'a [Arc<Account>]) -> Option<&'a Arc<Account>> {
        candidates.iter().min_by(|a, b| {
            let la = a.state.last_used_at.load();
            let lb = b.state.last_used_at.load();
            match (la.as_deref(), lb.as_deref()) {
                (None, None) => std::cmp::Ordering::Equal,
                (None, Some(_)) => std::cmp::Ordering::Less,
                (Some(_), None) => std::cmp::Ordering::Greater,
                (Some(x), Some(y)) => x.cmp(y),
            }
        })
    }
}
