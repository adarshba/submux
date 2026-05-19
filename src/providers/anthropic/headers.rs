//! Stainless SDK + Claude Code fingerprint headers.
//!
//! Anthropic OAuth-issued tokens expect requests to look like Claude Code:
//! SDK identification headers, a stable HMAC-derived device id, the mandatory
//! `anthropic-beta`, and the dangerous-direct-browser-access flag.

use std::collections::HashMap;

use http::{HeaderMap, HeaderName, HeaderValue};
use uuid::Uuid;

use crate::constants::http_headers::ANTHROPIC_OAUTH_BETA;
use crate::constants::user_agents::{CLAUDE_CODE_USER_AGENT, CLAUDE_CODE_VERSION};
use crate::core::FingerprintProfile;

/// Stainless SDK headers as observed from Claude Code v2.1.87.
pub fn default_stainless_headers() -> HashMap<String, String> {
    let mut h = HashMap::new();
    h.insert("x-stainless-lang".into(), "js".into());
    h.insert("x-stainless-package-version".into(), "0.39.0".into());
    h.insert("x-stainless-os".into(), "MacOS".into());
    h.insert("x-stainless-arch".into(), "arm64".into());
    h.insert("x-stainless-runtime".into(), "node".into());
    h.insert("x-stainless-runtime-version".into(), "v20.18.0".into());
    h.insert("x-stainless-helper-method".into(), "stream".into());
    h.insert("x-stainless-retry-count".into(), "0".into());
    h.insert("x-stainless-timeout".into(), "600".into());
    h
}

/// Default `FingerprintProfile` for Claude Code. Pinned here so the rest of
/// the codebase has a stable seed; replace via config to track new releases.
pub fn default_claude_code_profile() -> FingerprintProfile {
    FingerprintProfile {
        name: "claude-code".to_owned(),
        version: CLAUDE_CODE_VERSION.to_owned(),
        user_agent: CLAUDE_CODE_USER_AGENT.to_owned(),
        stainless_headers: default_stainless_headers(),
        anthropic_betas: vec![ANTHROPIC_OAUTH_BETA.to_owned()],
    }
}

/// Build the static cloak headers — Authorization, beta, browser-access,
/// User-Agent, and the Stainless SDK identification.
pub fn cloak_headers(fingerprint: &FingerprintProfile, access_token: &str) -> HeaderMap {
    let mut headers = HeaderMap::new();

    if let Ok(bearer) = HeaderValue::from_str(&format!("Bearer {access_token}")) {
        headers.insert(http::header::AUTHORIZATION, bearer);
    }

    if let Ok(ua) = HeaderValue::from_str(&fingerprint.user_agent) {
        headers.insert(http::header::USER_AGENT, ua);
    }

    headers.insert(
        HeaderName::from_static("anthropic-dangerous-direct-browser-access"),
        HeaderValue::from_static("true"),
    );

    let mut betas: Vec<&str> = fingerprint
        .anthropic_betas
        .iter()
        .map(String::as_str)
        .collect();
    if !betas.contains(&ANTHROPIC_OAUTH_BETA) {
        betas.push(ANTHROPIC_OAUTH_BETA);
    }
    if let Ok(v) = HeaderValue::from_str(&betas.join(",")) {
        headers.insert(HeaderName::from_static("anthropic-beta"), v);
    }

    for (k, v) in &fingerprint.stainless_headers {
        let (Ok(name), Ok(value)) = (HeaderName::try_from(k.as_str()), HeaderValue::try_from(v))
        else {
            continue;
        };
        headers.insert(name, value);
    }

    headers
}

/// Stable per-seed device id derived via UUIDv5 in the URL namespace.
pub fn derive_device_id(seed: &str) -> Uuid {
    Uuid::new_v5(&Uuid::NAMESPACE_URL, seed.as_bytes())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cloak_headers_includes_oauth_beta() {
        let fp = default_claude_code_profile();
        let h = cloak_headers(&fp, "tok");
        let beta = h.get("anthropic-beta").unwrap().to_str().unwrap();
        assert!(beta.contains(ANTHROPIC_OAUTH_BETA));
        assert_eq!(h.get(http::header::AUTHORIZATION).unwrap(), "Bearer tok");
        assert_eq!(
            h.get("anthropic-dangerous-direct-browser-access").unwrap(),
            "true"
        );
    }

    #[test]
    fn device_id_is_deterministic() {
        let a = derive_device_id("account-abc");
        let b = derive_device_id("account-abc");
        let c = derive_device_id("account-xyz");
        assert_eq!(a, b);
        assert_ne!(a, c);
    }

    #[test]
    fn stainless_headers_contain_lang() {
        let h = default_stainless_headers();
        assert_eq!(h.get("x-stainless-lang"), Some(&"js".to_string()));
    }
}
