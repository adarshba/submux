//! ChatGPT Codex subscription proxy.
//!
//! Thin handle over [`CodexSessionAdapter`] that lives in
//! [`crate::server::app::AppState`] and is also installed into a
//! process-wide `OnceCell` so non-route callers can reach the same instance.

use std::sync::Arc;
use url::Url;

use crate::providers::codex::session::CodexSessionAdapter;

/// Top-level Codex proxy. Owns the underlying [`CodexSessionAdapter`] and
/// exposes the `codex` handle every route reaches for.
pub struct CodexProxy {
    pub codex: Arc<CodexSessionAdapter>,
}

impl CodexProxy {
    /// Wrap a fresh [`CodexSessionAdapter`].
    pub fn new(http: Arc<reqwest::Client>, upstream: Url) -> Self {
        Self {
            codex: Arc::new(CodexSessionAdapter::new(http, upstream)),
        }
    }

    /// Default reqwest client used when the server doesn't pre-build one.
    pub fn default_client() -> reqwest::Client {
        CodexSessionAdapter::default_client()
    }
}
