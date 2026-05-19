use std::sync::Arc;

use crate::accounts::Account;
use crate::router::strategy::{RoutingStrategy, StrategyKind};

/// Pick the account with the lowest predicted-busy quota utilization,
/// defined as `max(quota_5h_util, quota_7d_util)`. Missing samples count
/// as 0.0 so freshly-loaded accounts win against any account with
/// measured load.
#[derive(Debug, Default)]
pub struct QuotaAware;

fn util_score(account: &Arc<Account>) -> f32 {
    let five_h = account
        .state
        .quota_5h_util
        .load()
        .as_deref()
        .copied()
        .unwrap_or(0.0);
    let seven_d = account
        .state
        .quota_7d_util
        .load()
        .as_deref()
        .copied()
        .unwrap_or(0.0);
    five_h.max(seven_d)
}

impl RoutingStrategy for QuotaAware {
    fn name(&self) -> &'static str {
        "quota-aware"
    }

    fn kind(&self) -> StrategyKind {
        StrategyKind::QuotaAware
    }

    fn pick<'a>(&self, candidates: &'a [Arc<Account>]) -> Option<&'a Arc<Account>> {
        candidates.iter().min_by(|a, b| {
            let sa = util_score(a);
            let sb = util_score(b);
            sa.partial_cmp(&sb).unwrap_or(std::cmp::Ordering::Equal)
        })
    }
}
