//! Inbound consumer identity, carried by the `X-Proxy-User-Id` header.
//!
//! A consumer is the calling machine/user — by convention `<whoami>_<uuid>`
//! (e.g. `adarsh.ba_3f2504e0-4f89-41d3-9a0c-0305e82c3301`), generated once and
//! persisted client-side. It is distinct from an [`crate::core::AccountId`] (the
//! upstream subscription that serves it); submux only reads and validates it.

const MAX_LEN: usize = 64;
const UNKNOWN: &str = "unknown";

/// A validated consumer identifier, or [`ConsumerId::unknown`].
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ConsumerId(String);

impl ConsumerId {
    /// The placeholder used when no valid id is supplied.
    pub fn unknown() -> Self {
        Self(UNKNOWN.to_owned())
    }

    /// Parse an id matching `[A-Za-z0-9._-]{1,64}` (e.g. `<whoami>_<uuid>`).
    /// Bounded charset and length keep it safe as a metric label value.
    pub fn parse(raw: &str) -> Option<Self> {
        if raw.is_empty() || raw.len() > MAX_LEN {
            return None;
        }
        let ok = raw
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'));
        ok.then(|| Self(raw.to_owned()))
    }

    /// Parse, folding a missing or invalid value into [`ConsumerId::unknown`].
    pub fn parse_or_unknown(raw: Option<&str>) -> Self {
        raw.and_then(Self::parse).unwrap_or_else(Self::unknown)
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for ConsumerId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_whoami_uuid_ids() {
        let id = "adarsh.ba_3f2504e0-4f89-41d3-9a0c-0305e82c3301";
        assert_eq!(ConsumerId::parse(id).unwrap().as_str(), id);
        assert!(ConsumerId::parse("laptop_1").is_some());
    }

    #[test]
    fn rejects_malformed_ids() {
        assert!(ConsumerId::parse("").is_none());
        assert!(ConsumerId::parse("has space").is_none());
        assert!(ConsumerId::parse("inject\"label").is_none());
        assert!(ConsumerId::parse(&"x".repeat(65)).is_none());
    }

    #[test]
    fn folds_missing_and_invalid_to_unknown() {
        assert_eq!(ConsumerId::parse_or_unknown(None).as_str(), "unknown");
        assert_eq!(
            ConsumerId::parse_or_unknown(Some("has space")).as_str(),
            "unknown"
        );
        assert_eq!(
            ConsumerId::parse_or_unknown(Some("alice_abc")).as_str(),
            "alice_abc"
        );
    }
}
