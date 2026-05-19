//! Redis-backed coordination — DEFERRED. Requires a live Redis cluster
//! to test; currently a placeholder so the architecture is clear.
//!
//! Activation: enable the `redis` Cargo feature and add the `redis`
//! crate as a dependency. The mapping is:
//!
//! - `announce_cooldown`         → `PUBLISH submux:cooldowns <payload>`
//! - `subscribe_cooldowns`       → `SUBSCRIBE submux:cooldowns`
//! - `try_acquire_refresh_lease` → `SET submux:refresh:<id> <token> NX EX <ttl_seconds>`
//! - `publish_cookie_jar`        → `PUBLISH submux:cookies <payload>`
//! - `subscribe_cookie_jars`     → `SUBSCRIBE submux:cookies`
//!
//! Implementation is intentionally absent: we cannot meaningfully test
//! it without a live Redis instance, and writing speculative code now
//! would lie about the trait being implemented. Whoever turns on the
//! `redis` feature will be the one to wire up the real adapter.

#[cfg(feature = "redis")]
pub struct RedisCoordinator {
    // TODO: connection manager, channel names, replica id, etc.
}

#[cfg(feature = "redis")]
mod redis_impl {}

#[cfg(not(feature = "redis"))]
/// Placeholder marker type. The `redis` feature is OFF by default; when
/// enabled, this type is replaced by a real coordinator implementation.
pub struct RedisCoordinator;
