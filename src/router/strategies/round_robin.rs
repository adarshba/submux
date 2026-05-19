use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use crate::accounts::Account;
use crate::router::strategy::{RoutingStrategy, StrategyKind};

/// Stateless round-robin. The counter is monotonically incremented and
/// taken modulo the candidate length on each call, so wraparound is
/// automatic.
#[derive(Debug, Default)]
pub struct RoundRobin {
    counter: AtomicUsize,
}

impl RoutingStrategy for RoundRobin {
    fn name(&self) -> &'static str {
        "round-robin"
    }

    fn kind(&self) -> StrategyKind {
        StrategyKind::RoundRobin
    }

    fn pick<'a>(&self, candidates: &'a [Arc<Account>]) -> Option<&'a Arc<Account>> {
        if candidates.is_empty() {
            return None;
        }
        let idx = self.counter.fetch_add(1, Ordering::Relaxed) % candidates.len();
        candidates.get(idx)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::accounts::Account;

    fn make_accounts(n: usize) -> Vec<Arc<Account>> {
        (0..n)
            .map(|i| {
                Arc::new(Account::new_anthropic_oauth(
                    format!("acct-{i}"),
                    "tok",
                    None,
                ))
            })
            .collect()
    }

    #[test]
    fn wraps_around() {
        let rr = RoundRobin::default();
        let accounts = make_accounts(3);
        let first = rr.pick(&accounts).expect("pick").id;
        let second = rr.pick(&accounts).expect("pick").id;
        let third = rr.pick(&accounts).expect("pick").id;
        let fourth = rr.pick(&accounts).expect("pick").id;
        assert_eq!(first, fourth);
        let mut ids = vec![first, second, third];
        ids.sort_by_key(|a| a.to_string());
        let mut expected: Vec<_> = accounts.iter().map(|a| a.id).collect();
        expected.sort_by_key(|a| a.to_string());
        assert_eq!(ids, expected);
    }

    #[test]
    fn empty_candidates_returns_none() {
        let rr = RoundRobin::default();
        let empty: Vec<Arc<Account>> = vec![];
        assert!(rr.pick(&empty).is_none());
    }
}
