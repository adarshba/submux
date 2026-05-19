//! Cross-module shared constants, grouped by topic.
//!
//! A constant belongs here when it is referenced by more than one module or
//! defines an external contract (HTTP header name, upstream URL path, fingerprint
//! string). Single-module values stay local as a `const` in the file that uses them.

pub mod http_headers;
pub mod limits;
pub mod upstream_paths;
pub mod user_agents;
