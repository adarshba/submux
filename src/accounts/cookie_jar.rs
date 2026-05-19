//! Per-account cookie jar — bound to a per-account `reqwest::Client` so
//! cookies cannot cross-contaminate accounts via connection reuse.
