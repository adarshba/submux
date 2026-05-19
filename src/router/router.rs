use chrono::Utc;
use std::future::Future;
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::Duration;

use crate::accounts::{Account, AccountState};
use crate::core::{AdapterError, SubmuxError};
use crate::router::cooldown::{CooldownCache, CooldownReason};
use crate::router::retry::{RetryDecision, RetryPolicy};
use crate::router::strategy::RoutingStrategy;

/// The router orchestrates account selection, retry, cooldown, and
/// surfaces fallback decisions to the caller (the protocol layer
/// resolves model-level fallbacks separately).
pub struct Router {
    pub strategy: Box<dyn RoutingStrategy>,
    pub cooldown: CooldownCache,
    pub retry: RetryPolicy,
}

impl std::fmt::Debug for Router {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Router")
            .field("strategy", &self.strategy.name())
            .field("cooldown", &self.cooldown)
            .field("retry", &self.retry)
            .finish()
    }
}

/// RAII guard that decrements `in_flight` when dropped, regardless of
/// whether the executor future panicked, was cancelled, or returned
/// normally.
struct InFlightGuard {
    state: Arc<AccountState>,
}

impl Drop for InFlightGuard {
    fn drop(&mut self) {
        self.state.in_flight.fetch_sub(1, Ordering::Relaxed);
    }
}

impl Router {
    pub fn new(
        strategy: Box<dyn RoutingStrategy>,
        cooldown: CooldownCache,
        retry: RetryPolicy,
    ) -> Self {
        Self {
            strategy,
            cooldown,
            retry,
        }
    }

    /// Run `executor` against a candidate account, picking via the
    /// configured strategy, respecting cooldowns, and honoring the
    /// retry policy. The returned error is one of:
    ///
    /// * `NoHealthyAccount` — no candidates available at start (after
    ///   cooldown filtering) or all candidates exhausted via
    ///   `SwitchAccount` decisions.
    /// * `RetryExhausted` — the attempt counter was bounded by
    ///   `RetryPolicy::max_attempts`.
    /// * `Internal` — the executor returned a `Terminal` or `Fallback`
    ///   classified error. Fallback chains are model-level and the
    ///   router does not own them, so it surfaces them upward.
    pub async fn dispatch<T, F, Fut>(
        &self,
        candidates: Vec<Arc<Account>>,
        executor: F,
    ) -> Result<T, SubmuxError>
    where
        F: Fn(Arc<Account>) -> Fut + Send + Sync,
        Fut: Future<Output = Result<T, AdapterError>> + Send,
        T: Send,
    {
        let mut pool: Vec<Arc<Account>> = Vec::with_capacity(candidates.len());
        for acct in candidates {
            if !self.cooldown.is_cooled(acct.id).await
                && !acct.state.permanently_disabled.load(Ordering::Relaxed)
            {
                pool.push(acct);
            }
        }
        if pool.is_empty() {
            return Err(SubmuxError::NoHealthyAccount {
                model_group: "<unknown>".to_owned(),
            });
        }

        let max_attempts = self.retry.max_attempts.max(1);
        let mut last_error: Option<AdapterError> = None;

        for attempt in 1..=max_attempts {
            let pick = match self.strategy.pick(&pool) {
                Some(p) => p.clone(),
                None => {
                    return Err(SubmuxError::NoHealthyAccount {
                        model_group: "<unknown>".to_owned(),
                    });
                }
            };

            tracing::debug!(
                account_id = %pick.id,
                attempt,
                strategy = self.strategy.name(),
                "dispatch attempt"
            );

            pick.state.in_flight.fetch_add(1, Ordering::Relaxed);
            let _guard = InFlightGuard {
                state: Arc::clone(&pick.state),
            };
            pick.state.last_used_at.store(Some(Arc::new(Utc::now())));

            let result = executor(Arc::clone(&pick)).await;
            drop(_guard);

            match result {
                Ok(value) => return Ok(value),
                Err(err) => {
                    let decision = self.retry.decide(attempt, &err);
                    tracing::debug!(
                        account_id = %pick.id,
                        attempt,
                        ?decision,
                        "dispatch decision"
                    );
                    match decision {
                        RetryDecision::Terminal => {
                            return Err(SubmuxError::Internal(err.to_string()));
                        }
                        RetryDecision::Fallback => {
                            return Err(SubmuxError::Internal(format!(
                                "fallback unhandled at router layer: {err}"
                            )));
                        }
                        RetryDecision::SwitchAccount => {
                            if let Some((reason, duration, status)) = cool_params_for(&err) {
                                self.cooldown.cool(pick.id, reason, duration, status).await;
                            }
                            pool.retain(|c| c.id != pick.id);
                            last_error = Some(err);
                            if pool.is_empty() {
                                return Err(SubmuxError::NoHealthyAccount {
                                    model_group: "<unknown>".to_owned(),
                                });
                            }
                        }
                        RetryDecision::RetrySameAccount(delay) => {
                            last_error = Some(err);
                            if !delay.is_zero() {
                                tokio::time::sleep(delay).await;
                            }
                        }
                    }
                }
            }
        }

        if let Some(err) = last_error {
            tracing::warn!(?err, "retry budget exhausted");
        }
        Err(SubmuxError::RetryExhausted {
            attempts: max_attempts,
        })
    }
}

/// Classify an adapter error into a `(reason, duration, status)` tuple
/// used to populate the cooldown cache on `SwitchAccount` decisions.
/// Returns `None` for errors that should switch but not cool the
/// account (e.g. a stale 401 that the refresh path may fix in
/// milliseconds).
fn cool_params_for(err: &AdapterError) -> Option<(CooldownReason, Duration, Option<u16>)> {
    match err {
        AdapterError::RateLimited { retry_after, .. } => Some((
            CooldownReason::RateLimited,
            retry_after.unwrap_or(Duration::from_secs(30)),
            Some(429),
        )),
        AdapterError::QuotaExhausted { reset_at } => {
            let now = Utc::now();
            let remaining = (*reset_at - now)
                .to_std()
                .unwrap_or(Duration::from_secs(60));
            let duration = remaining.max(Duration::from_secs(60));
            Some((CooldownReason::QuotaExhausted, duration, None))
        }
        AdapterError::TokenInvalid { .. } => {
            Some((CooldownReason::Auth, Duration::from_secs(60), Some(401)))
        }
        AdapterError::InteractiveChallenge { .. } => Some((
            CooldownReason::ChallengeRequired,
            Duration::from_secs(3600),
            None,
        )),
        AdapterError::Upstream { status, .. } if *status >= 500 => Some((
            CooldownReason::ServerError,
            Duration::from_secs(30),
            Some(*status),
        )),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::{RlScope, TransientKind};
    use crate::router::strategies::round_robin::RoundRobin;
    use std::sync::atomic::AtomicU32;
    use std::sync::Mutex;

    fn make_router() -> Router {
        Router::new(
            Box::new(RoundRobin::default()),
            CooldownCache::new(16),
            RetryPolicy {
                max_attempts: 3,
                base_backoff: Duration::from_millis(1),
                max_backoff: Duration::from_millis(2),
                jitter_factor: 0.0,
            },
        )
    }

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

    #[tokio::test]
    async fn dispatch_returns_ok_on_first_success() {
        let router = make_router();
        let accounts = make_accounts(2);
        let calls = Arc::new(AtomicU32::new(0));
        let calls_c = Arc::clone(&calls);
        let result: Result<u32, _> = router
            .dispatch(accounts, move |_acct| {
                let c = Arc::clone(&calls_c);
                async move {
                    c.fetch_add(1, Ordering::Relaxed);
                    Ok::<u32, AdapterError>(42)
                }
            })
            .await;
        assert!(matches!(result, Ok(42)));
        assert_eq!(calls.load(Ordering::Relaxed), 1);
    }

    #[tokio::test]
    async fn dispatch_empty_candidates_no_healthy_account() {
        let router = make_router();
        let result: Result<u32, _> = router
            .dispatch(vec![], |_a| async { Ok::<u32, AdapterError>(0) })
            .await;
        assert!(matches!(result, Err(SubmuxError::NoHealthyAccount { .. })));
    }

    #[tokio::test]
    async fn dispatch_transient_retries_then_succeeds() {
        let router = make_router();
        let accounts = make_accounts(1);
        let calls = Arc::new(AtomicU32::new(0));
        let calls_c = Arc::clone(&calls);
        let result: Result<u32, _> = router
            .dispatch(accounts, move |_a| {
                let c = Arc::clone(&calls_c);
                async move {
                    let n = c.fetch_add(1, Ordering::Relaxed);
                    if n == 0 {
                        Err(AdapterError::Transient {
                            cause: TransientKind::ReadTimeout,
                        })
                    } else {
                        Ok(7)
                    }
                }
            })
            .await;
        assert!(matches!(result, Ok(7)));
        assert_eq!(calls.load(Ordering::Relaxed), 2);
    }

    #[tokio::test]
    async fn dispatch_switch_account_cools_and_drops() {
        let router = make_router();
        let accounts = make_accounts(2);
        let seen: Arc<Mutex<Vec<crate::core::AccountId>>> = Arc::new(Mutex::new(Vec::new()));
        let seen_c = Arc::clone(&seen);
        let _: Result<u32, _> = router
            .dispatch(accounts.clone(), move |acct| {
                let s = Arc::clone(&seen_c);
                async move {
                    s.lock().expect("lock").push(acct.id);
                    Err::<u32, _>(AdapterError::RateLimited {
                        retry_after: Some(Duration::from_secs(120)),
                        scope: RlScope::Account,
                    })
                }
            })
            .await;
        let s = seen.lock().expect("lock");
        let mut ids: Vec<_> = s.clone();
        ids.sort_by_key(|id| id.to_string());
        let mut expected: Vec<_> = accounts.iter().map(|a| a.id).collect();
        expected.sort_by_key(|id| id.to_string());
        assert_eq!(ids, expected);
    }

    #[tokio::test]
    async fn dispatch_bad_request_terminal() {
        let router = make_router();
        let accounts = make_accounts(2);
        let result: Result<u32, _> = router
            .dispatch(accounts, |_a| async {
                Err::<u32, _>(AdapterError::BadRequest {
                    provider_msg: "boom".into(),
                })
            })
            .await;
        assert!(matches!(result, Err(SubmuxError::Internal(_))));
    }

    #[tokio::test]
    async fn dispatch_fallback_surfaces_as_internal() {
        let router = make_router();
        let accounts = make_accounts(1);
        let result: Result<u32, _> = router
            .dispatch(accounts, |_a| async {
                Err::<u32, _>(AdapterError::ContextWindowExceeded {
                    tokens_used: 200_000,
                    limit: 100_000,
                })
            })
            .await;
        match result {
            Err(SubmuxError::Internal(msg)) => assert!(msg.contains("fallback unhandled")),
            other => panic!("expected Internal, got {other:?}"),
        }
    }
}
