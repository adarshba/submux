//! Parse Anthropic rate-limit response headers and apply them to an `Account`.
//!
//! NOTE: predictive cooldown triggers at `COOLDOWN_UTIL_THRESHOLD` so we skip
//! the account before the next request would hard-fail with a 429 — Anthropic's
//! published utilization can lag actual consumption.

use chrono::{DateTime, Utc};
use http::HeaderMap;
use std::sync::Arc;
use std::time::Duration;

use crate::accounts::{quota::QuotaSnapshot, Account};
use crate::constants::http_headers::{
    H_RL_5H_RESET, H_RL_5H_UTIL, H_RL_7D_RESET, H_RL_7D_UTIL, H_RL_FALLBACK, H_RL_STATUS,
};
use crate::constants::limits::{COOLDOWN_FALLBACK_SECS, COOLDOWN_UTIL_THRESHOLD};
use crate::router::cooldown::{CooldownCache, CooldownReason};
use crate::telemetry::metrics;

fn parse_f32(headers: &HeaderMap, name: &str) -> Option<f32> {
    headers
        .get(name)
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.trim().parse::<f32>().ok())
}

fn parse_datetime(headers: &HeaderMap, name: &str) -> Option<DateTime<Utc>> {
    headers
        .get(name)
        .and_then(|v| v.to_str().ok())
        .and_then(|s| DateTime::parse_from_rfc3339(s.trim()).ok())
        .map(|d| d.with_timezone(&Utc))
}

fn parse_string(headers: &HeaderMap, name: &str) -> Option<String> {
    headers
        .get(name)
        .and_then(|v| v.to_str().ok())
        .map(|s| s.trim().to_owned())
        .filter(|s| !s.is_empty())
}

/// Parse the Anthropic unified rate-limit headers into a `QuotaSnapshot`.
/// Each field is independently optional; missing or malformed headers yield
/// `None` for that field rather than an error.
pub fn parse_quota_headers(headers: &HeaderMap) -> QuotaSnapshot {
    QuotaSnapshot {
        five_hour_util: parse_f32(headers, H_RL_5H_UTIL),
        seven_day_util: parse_f32(headers, H_RL_7D_UTIL),
        five_hour_reset_at: parse_datetime(headers, H_RL_5H_RESET),
        seven_day_reset_at: parse_datetime(headers, H_RL_7D_RESET),
        fallback_percentage: parse_f32(headers, H_RL_FALLBACK),
        overage_status: parse_string(headers, H_RL_STATUS),
    }
}

/// Mirror a parsed snapshot onto `AccountState`, emit gauges, and trigger a
/// predictive cooldown when the 5h window is at/above
/// `COOLDOWN_UTIL_THRESHOLD`.
pub async fn apply_quota(
    account: &Arc<Account>,
    cooldown: &CooldownCache,
    snapshot: &QuotaSnapshot,
) {
    let state = &account.state;

    state
        .quota_5h_util
        .store(snapshot.five_hour_util.map(Arc::new));
    state
        .quota_7d_util
        .store(snapshot.seven_day_util.map(Arc::new));
    state
        .quota_5h_reset_at
        .store(snapshot.five_hour_reset_at.map(Arc::new));

    let account_id_str = account.id.to_string();

    if let Some(util) = snapshot.five_hour_util {
        metrics::set_account_quota_utilization(&account_id_str, "5h", util as f64);
    }
    if let Some(util) = snapshot.seven_day_util {
        metrics::set_account_quota_utilization(&account_id_str, "7d", util as f64);
    }

    if let (Some(util), Some(reset_at)) = (snapshot.five_hour_util, snapshot.five_hour_reset_at) {
        if util >= COOLDOWN_UTIL_THRESHOLD {
            let delta = reset_at - Utc::now();
            let dur = delta
                .to_std()
                .unwrap_or_else(|_| Duration::from_secs(COOLDOWN_FALLBACK_SECS as u64));
            cooldown
                .cool(account.id, CooldownReason::QuotaExhausted, dur, None)
                .await;
            metrics::set_account_cooldown_active(&account_id_str, true);
            tracing::info!(
                account_id = %account_id_str,
                util_5h = util,
                reset_at = %reset_at,
                "predictive cooldown: 5h quota near exhaustion"
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use http::{HeaderMap, HeaderValue};

    fn populated_headers() -> HeaderMap {
        let mut h = HeaderMap::new();
        h.insert(H_RL_5H_UTIL, HeaderValue::from_static("0.42"));
        h.insert(H_RL_7D_UTIL, HeaderValue::from_static("0.61"));
        h.insert(
            H_RL_5H_RESET,
            HeaderValue::from_static("2025-05-19T20:00:00Z"),
        );
        h.insert(
            H_RL_7D_RESET,
            HeaderValue::from_static("2025-05-26T00:00:00Z"),
        );
        h.insert(H_RL_FALLBACK, HeaderValue::from_static("0.0"));
        h.insert(H_RL_STATUS, HeaderValue::from_static("ok"));
        h
    }

    #[test]
    fn parses_all_fields_when_present() {
        let snap = parse_quota_headers(&populated_headers());
        assert!((snap.five_hour_util.unwrap() - 0.42).abs() < 1e-6);
        assert!((snap.seven_day_util.unwrap() - 0.61).abs() < 1e-6);
        assert_eq!(
            snap.five_hour_reset_at.unwrap().to_rfc3339(),
            "2025-05-19T20:00:00+00:00"
        );
        assert_eq!(
            snap.seven_day_reset_at.unwrap().to_rfc3339(),
            "2025-05-26T00:00:00+00:00"
        );
        assert_eq!(snap.fallback_percentage, Some(0.0));
        assert_eq!(snap.overage_status.as_deref(), Some("ok"));
    }

    #[test]
    fn empty_headermap_yields_all_none() {
        let snap = parse_quota_headers(&HeaderMap::new());
        assert!(snap.five_hour_util.is_none());
        assert!(snap.seven_day_util.is_none());
        assert!(snap.five_hour_reset_at.is_none());
        assert!(snap.seven_day_reset_at.is_none());
        assert!(snap.fallback_percentage.is_none());
        assert!(snap.overage_status.is_none());
    }

    #[test]
    fn malformed_numeric_is_ignored() {
        let mut h = HeaderMap::new();
        h.insert(H_RL_5H_UTIL, HeaderValue::from_static("not-a-number"));
        h.insert(H_RL_7D_UTIL, HeaderValue::from_static("0.5"));
        let snap = parse_quota_headers(&h);
        assert!(snap.five_hour_util.is_none());
        assert_eq!(snap.seven_day_util, Some(0.5));
    }

    #[test]
    fn malformed_datetime_is_ignored() {
        let mut h = HeaderMap::new();
        h.insert(H_RL_5H_RESET, HeaderValue::from_static("tomorrow-ish"));
        let snap = parse_quota_headers(&h);
        assert!(snap.five_hour_reset_at.is_none());
    }
}
