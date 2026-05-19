use rand::Rng;
use std::time::Duration;

use crate::core::AdapterError;

/// Retry policy parameters. Decoupled from `RetryDecision` so the same
/// policy can be re-used across requests.
///
/// `max_attempts` is the upper bound on attempts per request (counting
/// the first attempt as 1). Same-account retries and switch-account
/// retries both consume from this budget.
#[derive(Debug, Clone)]
pub struct RetryPolicy {
    pub max_attempts: u32,
    pub base_backoff: Duration,
    pub max_backoff: Duration,
    /// Jitter as a fraction of the base delay, applied symmetrically
    /// (e.g. 0.2 = ±20%).
    pub jitter_factor: f32,
}

impl Default for RetryPolicy {
    fn default() -> Self {
        Self {
            max_attempts: 3,
            base_backoff: Duration::from_millis(250),
            max_backoff: Duration::from_secs(8),
            jitter_factor: 0.2,
        }
    }
}

/// The decision a `RetryPolicy` returns for a given (attempt, error)
/// pair. The router consumes this and acts accordingly — sleeping and
/// retrying the same account, cooling and switching, walking the
/// fallback chain, or surfacing terminal failure to the client.
#[derive(Debug)]
pub enum RetryDecision {
    /// Sleep for the given duration, then retry the same account.
    RetrySameAccount(Duration),
    /// Cool the current account (router decides duration/reason) and
    /// pick a new candidate.
    SwitchAccount,
    /// Bail out of account selection and walk the model-level fallback
    /// chain (context-window, content-policy, generic).
    Fallback,
    /// No further retries are useful. Surface the error to the client.
    Terminal,
}

impl RetryPolicy {
    /// Decide what to do given the most recent error and the current
    /// attempt number (1-indexed).
    pub fn decide(&self, attempt: u32, err: &AdapterError) -> RetryDecision {
        use AdapterError::*;
        match err {
            TokenInvalid { .. } => RetryDecision::SwitchAccount,

            RateLimited { retry_after, .. } => match retry_after {
                Some(after) if *after > Duration::from_secs(5) => RetryDecision::SwitchAccount,
                Some(after) => RetryDecision::RetrySameAccount(self.apply_jitter(*after)),
                None => RetryDecision::RetrySameAccount(self.backoff(attempt)),
            },

            QuotaExhausted { .. } => RetryDecision::SwitchAccount,

            InteractiveChallenge { .. } => RetryDecision::SwitchAccount,

            Transient { .. } => {
                if attempt >= self.max_attempts {
                    RetryDecision::Terminal
                } else {
                    RetryDecision::RetrySameAccount(self.backoff(attempt))
                }
            }

            ContextWindowExceeded { .. } => RetryDecision::Fallback,
            ContentPolicy { .. } => RetryDecision::Fallback,

            BadRequest { .. } => RetryDecision::Terminal,

            Upstream { status, .. } => {
                if *status >= 500 && attempt < self.max_attempts {
                    RetryDecision::RetrySameAccount(self.backoff(attempt))
                } else {
                    RetryDecision::Terminal
                }
            }

            Internal(_) => RetryDecision::Terminal,
        }
    }

    /// Exponential backoff with jitter, clamped to `max_backoff`.
    ///
    /// `attempt` is 1-indexed: attempt 1 sleeps `base`, attempt 2
    /// sleeps `2 * base`, etc. Using `f64` math avoids the integer
    /// overflow risk of `1u32 << large_attempt`.
    pub fn backoff(&self, attempt: u32) -> Duration {
        let exp = attempt.saturating_sub(1).min(30) as i32;
        let base_secs = self.base_backoff.as_secs_f64();
        let raw_secs = base_secs * 2f64.powi(exp);
        let max_secs = self.max_backoff.as_secs_f64();
        let capped = raw_secs.min(max_secs);
        let jittered = self.jitter_f64(capped);
        let clamped = jittered.clamp(0.0, max_secs);
        Duration::from_secs_f64(clamped)
    }

    /// Apply jitter to an externally-supplied duration (e.g. an
    /// upstream `Retry-After` header), clamped to non-negative.
    fn apply_jitter(&self, base: Duration) -> Duration {
        let secs = self.jitter_f64(base.as_secs_f64()).max(0.0);
        Duration::from_secs_f64(secs)
    }

    fn jitter_f64(&self, base_secs: f64) -> f64 {
        let j = self.jitter_factor as f64;
        if j <= 0.0 {
            return base_secs;
        }
        let mut rng = rand::thread_rng();
        let delta = rng.gen_range(-j..=j) * base_secs;
        base_secs + delta
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::{ChallengeKind, RlScope, TransientKind};
    use bytes::Bytes;
    use chrono::Utc;

    fn policy() -> RetryPolicy {
        RetryPolicy::default()
    }

    #[test]
    fn backoff_grows_exponentially_and_caps() {
        let p = RetryPolicy {
            max_attempts: 6,
            base_backoff: Duration::from_millis(100),
            max_backoff: Duration::from_secs(2),
            jitter_factor: 0.0,
        };
        assert_eq!(p.backoff(1), Duration::from_millis(100));
        assert_eq!(p.backoff(2), Duration::from_millis(200));
        assert_eq!(p.backoff(3), Duration::from_millis(400));
        assert_eq!(p.backoff(6), Duration::from_secs(2));
    }

    #[test]
    fn backoff_jitter_stays_within_bounds() {
        let p = RetryPolicy {
            max_attempts: 3,
            base_backoff: Duration::from_millis(500),
            max_backoff: Duration::from_secs(10),
            jitter_factor: 0.2,
        };
        for _ in 0..200 {
            let d = p.backoff(2);
            assert!(
                d >= Duration::from_millis(800) && d <= Duration::from_millis(1200),
                "backoff out of bounds: {d:?}",
            );
        }
    }

    #[test]
    fn decide_token_invalid_switches() {
        let p = policy();
        let err = AdapterError::TokenInvalid {
            needs_reauth: false,
        };
        assert!(matches!(p.decide(1, &err), RetryDecision::SwitchAccount));
    }

    #[test]
    fn decide_rate_limited_short_retry_after_same_account() {
        let p = policy();
        let err = AdapterError::RateLimited {
            retry_after: Some(Duration::from_secs(2)),
            scope: RlScope::Account,
        };
        match p.decide(1, &err) {
            RetryDecision::RetrySameAccount(d) => {
                assert!(d <= Duration::from_secs(3));
            }
            other => panic!("expected RetrySameAccount, got {other:?}"),
        }
    }

    #[test]
    fn decide_rate_limited_long_retry_after_switches() {
        let p = policy();
        let err = AdapterError::RateLimited {
            retry_after: Some(Duration::from_secs(30)),
            scope: RlScope::Account,
        };
        assert!(matches!(p.decide(1, &err), RetryDecision::SwitchAccount));
    }

    #[test]
    fn decide_rate_limited_no_retry_after_uses_backoff() {
        let p = policy();
        let err = AdapterError::RateLimited {
            retry_after: None,
            scope: RlScope::Account,
        };
        assert!(matches!(
            p.decide(1, &err),
            RetryDecision::RetrySameAccount(_)
        ));
    }

    #[test]
    fn decide_quota_exhausted_switches() {
        let p = policy();
        let err = AdapterError::QuotaExhausted {
            reset_at: Utc::now(),
        };
        assert!(matches!(p.decide(1, &err), RetryDecision::SwitchAccount));
    }

    #[test]
    fn decide_interactive_challenge_switches() {
        let p = policy();
        let err = AdapterError::InteractiveChallenge {
            kind: ChallengeKind::Captcha,
        };
        assert!(matches!(p.decide(1, &err), RetryDecision::SwitchAccount));
    }

    #[test]
    fn decide_transient_under_budget_retries_same() {
        let p = policy();
        let err = AdapterError::Transient {
            cause: TransientKind::ReadTimeout,
        };
        assert!(matches!(
            p.decide(1, &err),
            RetryDecision::RetrySameAccount(_)
        ));
    }

    #[test]
    fn decide_transient_at_budget_terminal() {
        let p = policy();
        let err = AdapterError::Transient {
            cause: TransientKind::ReadTimeout,
        };
        assert!(matches!(
            p.decide(p.max_attempts, &err),
            RetryDecision::Terminal
        ));
    }

    #[test]
    fn decide_context_window_falls_back() {
        let p = policy();
        let err = AdapterError::ContextWindowExceeded {
            tokens_used: 200_000,
            limit: 100_000,
        };
        assert!(matches!(p.decide(1, &err), RetryDecision::Fallback));
    }

    #[test]
    fn decide_content_policy_falls_back() {
        let p = policy();
        let err = AdapterError::ContentPolicy {
            provider_code: "blocked".into(),
        };
        assert!(matches!(p.decide(1, &err), RetryDecision::Fallback));
    }

    #[test]
    fn decide_bad_request_terminal() {
        let p = policy();
        let err = AdapterError::BadRequest {
            provider_msg: "malformed".into(),
        };
        assert!(matches!(p.decide(1, &err), RetryDecision::Terminal));
    }

    #[test]
    fn decide_upstream_5xx_retries_then_terminal() {
        let p = policy();
        let err = AdapterError::Upstream {
            status: 502,
            body: Bytes::new(),
        };
        assert!(matches!(
            p.decide(1, &err),
            RetryDecision::RetrySameAccount(_)
        ));
        assert!(matches!(
            p.decide(p.max_attempts, &err),
            RetryDecision::Terminal
        ));
    }

    #[test]
    fn decide_upstream_4xx_terminal() {
        let p = policy();
        let err = AdapterError::Upstream {
            status: 404,
            body: Bytes::new(),
        };
        assert!(matches!(p.decide(1, &err), RetryDecision::Terminal));
    }

    #[test]
    fn decide_internal_terminal() {
        let p = policy();
        let err = AdapterError::Internal("boom".into());
        assert!(matches!(p.decide(1, &err), RetryDecision::Terminal));
    }
}
