use std::sync::Arc;

use crate::accounts::Account;
use crate::router::strategy::{RoutingStrategy, StrategyKind};

/// Pick the account with the highest health score (0.0..=1.0). Accounts
/// with no health sample yet are assigned a neutral 0.5 so a fresh
/// account is preferred over an unhealthy one but loses to a confirmed
/// healthy one.
#[derive(Debug, Default)]
pub struct HealthAware;

fn health_score(account: &Arc<Account>) -> f32 {
    account
        .state
        .health
        .load()
        .as_deref()
        .copied()
        .unwrap_or(0.5)
}

impl RoutingStrategy for HealthAware {
    fn name(&self) -> &'static str {
        "health-aware"
    }

    fn kind(&self) -> StrategyKind {
        StrategyKind::HealthAware
    }

    fn pick<'a>(&self, candidates: &'a [Arc<Account>]) -> Option<&'a Arc<Account>> {
        candidates.iter().max_by(|a, b| {
            let ha = health_score(a);
            let hb = health_score(b);
            ha.partial_cmp(&hb).unwrap_or(std::cmp::Ordering::Equal)
        })
    }
}
