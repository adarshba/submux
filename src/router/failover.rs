use serde::{Deserialize, Serialize};
use std::collections::HashMap;

use crate::core::AdapterError;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FallbackTier {
    Generic,
    ContextWindow,
    ContentPolicy,
}

fn default_max_fallbacks() -> u32 {
    3
}

/// Three-tier fallback chain. Mirrors LiteLLM `router.py:5751-6077` —
/// generic fallbacks plus specialized chains for context-window and
/// content-policy errors. `max_fallbacks` bounds traversal depth to
/// guarantee termination even with cyclic config.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FallbackChain {
    /// Ordered list of model names; on a generic error we hop to the
    /// entry *after* the current model.
    #[serde(default)]
    pub generic: Vec<String>,
    /// `current_model -> larger_context_model`.
    #[serde(default)]
    pub context_window: HashMap<String, String>,
    /// `current_model -> alternate_model_for_blocked_content`.
    #[serde(default)]
    pub content_policy: HashMap<String, String>,
    #[serde(default = "default_max_fallbacks")]
    pub max_fallbacks: u32,
}

impl Default for FallbackChain {
    fn default() -> Self {
        Self::empty()
    }
}

impl FallbackChain {
    /// An empty fallback chain (no fallbacks configured) with the
    /// default `max_fallbacks = 3` depth cap.
    pub fn empty() -> Self {
        Self {
            generic: Vec::new(),
            context_window: HashMap::new(),
            content_policy: HashMap::new(),
            max_fallbacks: default_max_fallbacks(),
        }
    }

    /// Compute the next fallback target given the current model, the
    /// error that triggered fallback, and the current depth (0-indexed
    /// — depth 0 is the first hop). Returns `None` when there is no
    /// configured next target or the depth cap has been reached.
    pub fn next_target(
        &self,
        current_model: &str,
        err: &AdapterError,
        depth: u32,
    ) -> Option<String> {
        if depth >= self.max_fallbacks {
            return None;
        }
        match err {
            AdapterError::ContextWindowExceeded { .. } => {
                self.context_window.get(current_model).cloned()
            }
            AdapterError::ContentPolicy { .. } => self.content_policy.get(current_model).cloned(),
            _ => {
                let idx = self.generic.iter().position(|m| m == current_model)?;
                self.generic.get(idx + 1).cloned()
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bytes::Bytes;

    fn upstream(status: u16) -> AdapterError {
        AdapterError::Upstream {
            status,
            body: Bytes::new(),
        }
    }

    #[test]
    fn depth_cap_blocks_traversal() {
        let mut chain = FallbackChain::empty();
        chain.generic = vec!["a".into(), "b".into(), "c".into()];
        chain.max_fallbacks = 1;
        assert_eq!(
            chain.next_target("a", &upstream(503), 0).as_deref(),
            Some("b")
        );
        assert!(chain.next_target("a", &upstream(503), 1).is_none());
        assert!(chain.next_target("a", &upstream(503), 5).is_none());
    }

    #[test]
    fn generic_walks_to_next_entry() {
        let mut chain = FallbackChain::empty();
        chain.generic = vec!["a".into(), "b".into(), "c".into()];
        assert_eq!(
            chain.next_target("a", &upstream(503), 0).as_deref(),
            Some("b")
        );
        assert_eq!(
            chain.next_target("b", &upstream(503), 0).as_deref(),
            Some("c")
        );
        assert!(chain.next_target("c", &upstream(503), 0).is_none());
        assert!(chain.next_target("missing", &upstream(503), 0).is_none());
    }

    #[test]
    fn context_window_uses_dedicated_map() {
        let mut chain = FallbackChain::empty();
        chain.generic = vec!["a".into(), "b".into()];
        chain.context_window.insert("a".into(), "a-200k".into());

        let err = AdapterError::ContextWindowExceeded {
            tokens_used: 200_000,
            limit: 100_000,
        };
        assert_eq!(chain.next_target("a", &err, 0).as_deref(), Some("a-200k"));
        assert!(chain.next_target("b", &err, 0).is_none());
    }

    #[test]
    fn content_policy_uses_dedicated_map() {
        let mut chain = FallbackChain::empty();
        chain
            .content_policy
            .insert("a".into(), "a-uncensored".into());

        let err = AdapterError::ContentPolicy {
            provider_code: "blocked".into(),
        };
        assert_eq!(
            chain.next_target("a", &err, 0).as_deref(),
            Some("a-uncensored")
        );
        assert!(chain.next_target("nope", &err, 0).is_none());
    }
}
