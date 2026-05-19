pub mod adapter;
pub mod chatgpt_session;
pub mod cookies;

pub use adapter::OpenAiSubscriptionAdapter;
pub use chatgpt_session::{CodexPassthroughResponse, CodexSessionAdapter};

use once_cell::sync::OnceCell;
use std::sync::Arc;

/// Process-wide handle for the OpenAI (Codex) subscription adapter.
///
/// We don't extend `AppState` for this — Phase 7 only needs the wiring to
/// exist so future routes (and the refresh manager) can reach the adapter
/// without threading another field through every layer.
static OPENAI_ADAPTER: OnceCell<Arc<OpenAiSubscriptionAdapter>> = OnceCell::new();

pub fn install_openai_adapter(adapter: Arc<OpenAiSubscriptionAdapter>) {
    let _ = OPENAI_ADAPTER.set(adapter);
}

pub fn openai_adapter() -> Option<Arc<OpenAiSubscriptionAdapter>> {
    OPENAI_ADAPTER.get().cloned()
}
