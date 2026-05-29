//! Codex (ChatGPT Plus/Pro) provider: session passthrough + cookie jar.

pub mod cookies;
pub mod proxy;
pub mod session;

pub use proxy::CodexProxy;
pub use session::{CodexPassthroughResponse, CodexSessionAdapter};

use once_cell::sync::OnceCell;
use std::sync::Arc;

/// Process-wide handle for the Codex proxy.
///
/// Some non-route call-sites (refresh manager, future tooling) need to
/// reach the adapter without threading another field through every layer;
/// a `OnceCell` keeps the wiring shallow.
static CODEX_PROXY: OnceCell<Arc<CodexProxy>> = OnceCell::new();

/// Register the singleton Codex proxy. Idempotent — repeat calls are no-ops.
pub fn install(proxy: Arc<CodexProxy>) {
    let _ = CODEX_PROXY.set(proxy);
}

/// Return the installed Codex proxy if any.
pub fn current() -> Option<Arc<CodexProxy>> {
    CODEX_PROXY.get().cloned()
}
