//! Size, time, and capacity limits referenced across the server and runtime.

use std::time::Duration;

pub const REQUEST_BODY_LIMIT_BYTES: usize = 8 * 1024 * 1024;
pub const REQUEST_TIMEOUT: Duration = Duration::from_secs(610);

pub const DEFAULT_COOLDOWN_CACHE_CAPACITY: u64 = 1024;
pub const DEFAULT_EVENT_BUS_CAPACITY: usize = 2048;
pub const DEFAULT_PER_ACCOUNT_CONCURRENCY: usize = 64;

pub const UPSTREAM_POOL_IDLE_TIMEOUT: Duration = Duration::from_secs(90);
pub const UPSTREAM_TCP_KEEPALIVE: Duration = Duration::from_secs(30);
pub const UPSTREAM_H2_KEEPALIVE_INTERVAL: Duration = Duration::from_secs(15);
pub const UPSTREAM_H2_KEEPALIVE_TIMEOUT: Duration = Duration::from_secs(30);
pub const UPSTREAM_REQUEST_TIMEOUT: Duration = Duration::from_secs(600);
pub const UPSTREAM_CONNECT_TIMEOUT: Duration = Duration::from_secs(10);

pub const COOLDOWN_FALLBACK_SECS: i64 = 60;
pub const COOLDOWN_UTIL_THRESHOLD: f32 = 0.95;
