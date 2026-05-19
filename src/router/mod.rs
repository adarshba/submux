//! Routing, retry, cooldown, failover.

pub mod cooldown;
pub mod failover;
pub mod retry;
#[allow(clippy::module_inception)]
pub mod router;
pub mod strategies;
pub mod strategy;

pub use cooldown::{CooldownCache, CooldownEntry, CooldownReason};
pub use failover::{FallbackChain, FallbackTier};
pub use retry::{RetryDecision, RetryPolicy};
pub use router::Router;
pub use strategies::health_aware::HealthAware;
pub use strategies::latency_aware::LatencyAware;
pub use strategies::least_recently_used::LeastRecentlyUsed;
pub use strategies::quota_aware::QuotaAware;
pub use strategies::round_robin::RoundRobin;
pub use strategy::{RoutingStrategy, StrategyKind};
