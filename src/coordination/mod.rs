//! Cross-replica coordination abstractions.
//!
//! For single-replica deployments use [`InMemoryCoordinator`] (the default).
//! For multi-replica deployments implement [`CoordinationBackend`] against
//! Redis (see `redis.rs`, currently behind the `redis` feature flag and
//! unimplemented).

pub mod in_memory;
pub mod redis;
pub mod traits;

pub use in_memory::InMemoryCoordinator;
pub use traits::{CooldownAnnouncement, CoordinationBackend, RefreshLeaseGuard, SharedCoordinator};
