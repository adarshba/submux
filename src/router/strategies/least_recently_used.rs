use std::sync::Arc;

use crate::accounts::Account;
use crate::router::strategy::{RoutingStrategy, StrategyKind};

/// Pick the account with the oldest `last_used_at`. An account that has
/// never been used (`last_used_at == None`) is preferred over any used
/// account (None < Some), which biases cold-start traffic toward
/// distributing load across the whole pool.
#[derive(Debug, Default)]
pub struct LeastRecentlyUsed;

impl RoutingStrategy for LeastRecentlyUsed {
    fn name(&self) -> &'static str {
        "least-recently-used"
    }

    fn kind(&self) -> StrategyKind {
        StrategyKind::LeastRecentlyUsed
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
