//! Inbound shared-secret authentication primitive.
//!
//! When the operator configures an [`ApiKey`], every proxy route requires the
//! same value on the inbound side. Clients pass it via `Authorization: Bearer`
//! or `x-api-key`. Mirrors meridian's `MERIDIAN_API_KEY` design.
//!
//! Comparison is constant-time over a SHA-256 of both sides so equal-length
//! comparison holds for any candidate, and the raw key is never compared
//! byte-for-byte against attacker-controlled input.

use rand::RngCore;
use sha2::{Digest, Sha256};
use std::fmt;
use thiserror::Error;

/// Prefix applied to every generated key, matching the `sk-…` convention used
/// by the upstream providers we proxy. `smx_` = submux, `live_` so future
/// `test_` keys can coexist without ambiguity.
pub const API_KEY_PREFIX: &str = "smx_live_";

const RAW_BYTES: usize = 16;
const HEX: &[u8; 16] = b"0123456789abcdef";

/// Failure modes when parsing an [`ApiKey`] from a string.
#[derive(Debug, Error, PartialEq, Eq)]
pub enum ApiKeyError {
    /// The supplied value was empty or whitespace-only.
    #[error("api key is empty")]
    Empty,
}

/// A configured shared secret used to gate inbound proxy traffic.
///
/// The struct owns both the raw string (so the banner can echo a redacted
/// form) and a precomputed SHA-256 (so per-request verification is a single
/// hash + a constant-time 32-byte compare).
#[derive(Clone)]
pub struct ApiKey {
    raw: String,
    hash: [u8; 32],
}

impl ApiKey {
    /// Generate a fresh random key in the canonical `smx_live_<32 hex>` form.
    pub fn generate() -> Self {
        let mut bytes = [0u8; RAW_BYTES];
        rand::thread_rng().fill_bytes(&mut bytes);
        let mut raw = String::with_capacity(API_KEY_PREFIX.len() + RAW_BYTES * 2);
        raw.push_str(API_KEY_PREFIX);
        for b in bytes {
            raw.push(char::from(HEX[usize::from(b >> 4)]));
            raw.push(char::from(HEX[usize::from(b & 0x0f)]));
        }
        Self::from_raw(raw)
    }

    /// Parse an existing key supplied by the operator (env var or config file).
    /// Whitespace is trimmed; an empty value is rejected.
    pub fn parse(value: &str) -> Result<Self, ApiKeyError> {
        let trimmed = value.trim();
        if trimmed.is_empty() {
            return Err(ApiKeyError::Empty);
        }
        Ok(Self::from_raw(trimmed.to_owned()))
    }

    fn from_raw(raw: String) -> Self {
        let mut hasher = Sha256::new();
        hasher.update(raw.as_bytes());
        let hash = hasher.finalize().into();
        Self { raw, hash }
    }

    /// Constant-time check against a candidate value.
    pub fn verify(&self, candidate: &[u8]) -> bool {
        let mut hasher = Sha256::new();
        hasher.update(candidate);
        let cand: [u8; 32] = hasher.finalize().into();
        let mut diff: u8 = 0;
        for (a, b) in self.hash.iter().zip(cand.iter()) {
            diff |= a ^ b;
        }
        diff == 0
    }

    /// `smx_live_2c5f…` — short prefix preserved, body collapsed.
    /// Safe to log, print on the startup banner, or expose via `config show`.
    pub fn redacted(&self) -> String {
        let tail: String = self
            .raw
            .chars()
            .skip(API_KEY_PREFIX.len())
            .take(4)
            .collect();
        format!("{API_KEY_PREFIX}{tail}…")
    }

    /// Full key value. Never log this — use [`Self::redacted`].
    pub fn as_str(&self) -> &str {
        &self.raw
    }
}

impl fmt::Debug for ApiKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ApiKey")
            .field("redacted", &self.redacted())
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generate_produces_canonical_shape() {
        let key = ApiKey::generate();
        assert!(key.as_str().starts_with(API_KEY_PREFIX));
        assert_eq!(key.as_str().len(), API_KEY_PREFIX.len() + 32);
        assert!(key
            .as_str()
            .chars()
            .skip(API_KEY_PREFIX.len())
            .all(|c| c.is_ascii_hexdigit()));
    }

    #[test]
    fn parse_rejects_empty() {
        assert!(matches!(ApiKey::parse(""), Err(ApiKeyError::Empty)));
        assert!(matches!(ApiKey::parse("   "), Err(ApiKeyError::Empty)));
    }

    #[test]
    fn parse_trims_whitespace() {
        let key = ApiKey::parse("  hello  ").expect("non-empty");
        assert_eq!(key.as_str(), "hello");
    }

    #[test]
    fn verify_accepts_matching_value() {
        let key = ApiKey::generate();
        assert!(key.verify(key.as_str().as_bytes()));
    }

    #[test]
    fn verify_rejects_mismatched_value() {
        let key = ApiKey::generate();
        assert!(!key.verify(b"different"));
        assert!(!key.verify(b""));
    }

    #[test]
    fn redacted_hides_body() {
        let key = ApiKey::parse("smx_live_2c5fa1b3c4d5e6f7").expect("valid");
        assert_eq!(key.redacted(), "smx_live_2c5f…");
    }

    #[test]
    fn debug_never_leaks_raw() {
        let key = ApiKey::generate();
        let dbg = format!("{key:?}");
        assert!(!dbg.contains(key.as_str()));
        assert!(dbg.contains("smx_live_"));
    }
}
