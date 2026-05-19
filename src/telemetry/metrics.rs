//! Prometheus + OTLP metrics.
//!
//! This is a hand-rolled minimal Prometheus 0.0.4 exposition-format encoder. It
//! is intentionally not based on the `prometheus` or `metrics` crates — the
//! gateway only exposes a small, fixed surface, so an in-process registry built
//! on `DashMap` + atomics is plenty.
//!
//! ## Surface
//!
//! - `submux_requests_total{protocol,model_group,status}` — Counter
//! - `submux_request_duration_seconds{protocol,model_group}` — Histogram
//! - `submux_account_health{account_id,provider}` — Gauge
//! - `submux_account_cooldown_active{account_id}` — Gauge (0/1)
//! - `submux_account_quota_utilization{account_id,window}` — Gauge
//! - `submux_account_in_flight{account_id}` — Gauge
//! - `submux_tokens_total{account_id,direction}` — Counter
//! - `submux_refresh_attempts_total{account_id,result}` — Counter
//! - `submux_stream_translate_chunks_total{from_protocol,to_protocol}` — Counter
//! - `submux_stream_backpressure_drops_total` — Counter (no labels)

use std::fmt::Write as _;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use dashmap::DashMap;
use once_cell::sync::OnceCell;
use parking_lot::RwLock;

/// Histogram bucket boundaries (upper bounds, excluding +Inf). The `+Inf`
/// overflow bucket is implicit — it always lives at index `buckets.len()` in
/// the `counts` vector.
pub const DEFAULT_DURATION_BUCKETS: &[f64] = &[0.05, 0.1, 0.25, 0.5, 1.0, 2.5, 5.0, 10.0, 30.0];

/// Canonical, hashable label set. Pairs are sorted by name on construction so
/// `("a","x")("b","y")` and `("b","y")("a","x")` collide on the same bucket.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct LabelSet {
    pairs: Vec<(&'static str, String)>,
}

impl LabelSet {
    pub fn new(pairs: &[(&'static str, &str)]) -> Self {
        let mut v: Vec<(&'static str, String)> =
            pairs.iter().map(|(k, v)| (*k, (*v).to_string())).collect();
        v.sort_by(|a, b| a.0.cmp(b.0));
        Self { pairs: v }
    }

    pub fn empty() -> Self {
        Self { pairs: Vec::new() }
    }

    pub fn pairs(&self) -> &[(&'static str, String)] {
        &self.pairs
    }

    /// Render `{k1="v1",k2="v2"}` (empty string for no labels), with values
    /// escaped per the Prometheus text format.
    fn render(&self) -> String {
        if self.pairs.is_empty() {
            return String::new();
        }
        let mut s = String::with_capacity(32);
        s.push('{');
        for (i, (k, v)) in self.pairs.iter().enumerate() {
            if i > 0 {
                s.push(',');
            }
            s.push_str(k);
            s.push('=');
            s.push('"');
            escape_label_value_into(v, &mut s);
            s.push('"');
        }
        s.push('}');
        s
    }
}

fn escape_label_value_into(v: &str, out: &mut String) {
    for c in v.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '\n' => out.push_str("\\n"),
            other => out.push(other),
        }
    }
}

#[derive(Debug, Default)]
pub struct Counter {
    v: AtomicU64,
}

impl Counter {
    pub fn new() -> Self {
        Self {
            v: AtomicU64::new(0),
        }
    }
    pub fn inc(&self) {
        self.v.fetch_add(1, Ordering::Relaxed);
    }
    pub fn inc_by(&self, n: u64) {
        self.v.fetch_add(n, Ordering::Relaxed);
    }
    pub fn get(&self) -> u64 {
        self.v.load(Ordering::Relaxed)
    }
}

#[derive(Debug, Default)]
pub struct Gauge {
    v: RwLock<f64>,
}

impl Gauge {
    pub fn new() -> Self {
        Self {
            v: RwLock::new(0.0),
        }
    }
    pub fn set(&self, x: f64) {
        *self.v.write() = x;
    }
    pub fn add(&self, x: f64) {
        let mut g = self.v.write();
        *g += x;
    }
    pub fn get(&self) -> f64 {
        *self.v.read()
    }
}

#[derive(Debug)]
pub struct Histogram {
    /// Upper bounds, ascending, NOT including +Inf.
    buckets: Vec<f64>,
    /// Cumulative-by-bucket counts; len = buckets.len() + 1 (last = +Inf).
    counts: Vec<AtomicU64>,
    sum: RwLock<f64>,
    count: AtomicU64,
}

impl Histogram {
    pub fn new(buckets: &[f64]) -> Self {
        let mut b = buckets.to_vec();
        b.sort_by(|a, c| a.partial_cmp(c).unwrap_or(std::cmp::Ordering::Equal));
        let counts = (0..=b.len()).map(|_| AtomicU64::new(0)).collect();
        Self {
            buckets: b,
            counts,
            sum: RwLock::new(0.0),
            count: AtomicU64::new(0),
        }
    }

    pub fn observe(&self, v: f64) {
        let idx = match self.buckets.iter().position(|b| v <= *b) {
            Some(i) => i,
            None => self.buckets.len(),
        };
        self.counts[idx].fetch_add(1, Ordering::Relaxed);
        self.count.fetch_add(1, Ordering::Relaxed);
        let mut s = self.sum.write();
        *s += v;
    }

    /// Returns cumulative bucket counts (each `_bucket{le="x"}` is the count of
    /// observations `<= x`, per the Prometheus contract).
    fn cumulative_counts(&self) -> Vec<u64> {
        let mut acc = 0u64;
        let mut out = Vec::with_capacity(self.counts.len());
        for c in &self.counts {
            acc = acc.saturating_add(c.load(Ordering::Relaxed));
            out.push(acc);
        }
        out
    }
}

#[derive(Debug)]
pub struct LabeledCounter {
    name: &'static str,
    help: &'static str,
    series: DashMap<LabelSet, Arc<Counter>>,
}

impl LabeledCounter {
    pub fn new(name: &'static str, help: &'static str) -> Self {
        Self {
            name,
            help,
            series: DashMap::new(),
        }
    }

    pub fn inc(&self, labels: &[(&'static str, &str)]) {
        self.inc_by(labels, 1);
    }

    pub fn inc_by(&self, labels: &[(&'static str, &str)], n: u64) {
        let key = LabelSet::new(labels);
        let entry = self
            .series
            .entry(key)
            .or_insert_with(|| Arc::new(Counter::new()));
        entry.inc_by(n);
    }
}

#[derive(Debug)]
pub struct LabeledGauge {
    name: &'static str,
    help: &'static str,
    series: DashMap<LabelSet, Arc<Gauge>>,
}

impl LabeledGauge {
    pub fn new(name: &'static str, help: &'static str) -> Self {
        Self {
            name,
            help,
            series: DashMap::new(),
        }
    }

    pub fn set(&self, labels: &[(&'static str, &str)], v: f64) {
        let key = LabelSet::new(labels);
        let entry = self
            .series
            .entry(key)
            .or_insert_with(|| Arc::new(Gauge::new()));
        entry.set(v);
    }

    pub fn add(&self, labels: &[(&'static str, &str)], v: f64) {
        let key = LabelSet::new(labels);
        let entry = self
            .series
            .entry(key)
            .or_insert_with(|| Arc::new(Gauge::new()));
        entry.add(v);
    }
}

#[derive(Debug)]
pub struct LabeledHistogram {
    name: &'static str,
    help: &'static str,
    buckets: Vec<f64>,
    series: DashMap<LabelSet, Arc<Histogram>>,
}

impl LabeledHistogram {
    pub fn new(name: &'static str, help: &'static str, buckets: &[f64]) -> Self {
        Self {
            name,
            help,
            buckets: buckets.to_vec(),
            series: DashMap::new(),
        }
    }

    pub fn observe(&self, labels: &[(&'static str, &str)], v: f64) {
        let key = LabelSet::new(labels);
        let entry = self
            .series
            .entry(key)
            .or_insert_with(|| Arc::new(Histogram::new(&self.buckets)));
        entry.observe(v);
    }
}

#[derive(Debug)]
pub struct Registry {
    pub requests_total: LabeledCounter,
    pub request_duration_seconds: LabeledHistogram,
    pub account_health: LabeledGauge,
    pub account_cooldown_active: LabeledGauge,
    pub account_quota_utilization: LabeledGauge,
    pub account_in_flight: LabeledGauge,
    pub tokens_total: LabeledCounter,
    pub refresh_attempts_total: LabeledCounter,
    pub stream_translate_chunks_total: LabeledCounter,
    pub stream_backpressure_drops_total: Counter,
}

impl Registry {
    pub fn new() -> Self {
        Self {
            requests_total: LabeledCounter::new(
                "submux_requests_total",
                "Total inbound requests, labelled by protocol, model group, and terminal status.",
            ),
            request_duration_seconds: LabeledHistogram::new(
                "submux_request_duration_seconds",
                "End-to-end request duration in seconds, labelled by protocol and model group.",
                DEFAULT_DURATION_BUCKETS,
            ),
            account_health: LabeledGauge::new(
                "submux_account_health",
                "Per-account health score (0.0 = unhealthy, 1.0 = healthy).",
            ),
            account_cooldown_active: LabeledGauge::new(
                "submux_account_cooldown_active",
                "1 if the account is currently in cooldown, 0 otherwise.",
            ),
            account_quota_utilization: LabeledGauge::new(
                "submux_account_quota_utilization",
                "Fraction of the account's quota consumed in the labelled window (0.0..=1.0).",
            ),
            account_in_flight: LabeledGauge::new(
                "submux_account_in_flight",
                "Number of in-flight upstream requests currently leased against the account.",
            ),
            tokens_total: LabeledCounter::new(
                "submux_tokens_total",
                "Total tokens accounted, labelled by account and direction (input|output).",
            ),
            refresh_attempts_total: LabeledCounter::new(
                "submux_refresh_attempts_total",
                "Total credential refresh attempts, labelled by account and result (success|failure).",
            ),
            stream_translate_chunks_total: LabeledCounter::new(
                "submux_stream_translate_chunks_total",
                "Total SSE chunks emitted by the cross-protocol stream translator.",
            ),
            stream_backpressure_drops_total: Counter::new(),
        }
    }

    /// Render the full registry as a Prometheus 0.0.4 text-format snapshot.
    /// Output is fully sorted (metric name, then label set) so admin tooling
    /// can diff snapshots without flapping.
    pub fn render_prometheus(&self) -> String {
        let mut out = String::with_capacity(4096);

        render_gauge(&self.account_cooldown_active, &mut out);
        render_gauge(&self.account_health, &mut out);
        render_gauge(&self.account_in_flight, &mut out);
        render_gauge(&self.account_quota_utilization, &mut out);
        render_counter(&self.refresh_attempts_total, &mut out);
        render_histogram(&self.request_duration_seconds, &mut out);
        render_counter(&self.requests_total, &mut out);
        render_unlabeled_counter(
            "submux_stream_backpressure_drops_total",
            "Total telemetry-side stream chunks dropped due to backpressure (client never drops).",
            &self.stream_backpressure_drops_total,
            &mut out,
        );
        render_counter(&self.stream_translate_chunks_total, &mut out);
        render_counter(&self.tokens_total, &mut out);

        out
    }
}

impl Default for Registry {
    fn default() -> Self {
        Self::new()
    }
}

fn render_counter(c: &LabeledCounter, out: &mut String) {
    writeln!(out, "# HELP {} {}", c.name, c.help).ok();
    writeln!(out, "# TYPE {} counter", c.name).ok();
    let mut rows: Vec<(LabelSet, u64)> = c
        .series
        .iter()
        .map(|kv| (kv.key().clone(), kv.value().get()))
        .collect();
    rows.sort_by(|a, b| a.0.pairs().cmp(b.0.pairs()));
    for (labels, v) in rows {
        writeln!(out, "{}{} {}", c.name, labels.render(), v).ok();
    }
}

fn render_unlabeled_counter(name: &str, help: &str, c: &Counter, out: &mut String) {
    writeln!(out, "# HELP {} {}", name, help).ok();
    writeln!(out, "# TYPE {} counter", name).ok();
    writeln!(out, "{} {}", name, c.get()).ok();
}

fn render_gauge(g: &LabeledGauge, out: &mut String) {
    writeln!(out, "# HELP {} {}", g.name, g.help).ok();
    writeln!(out, "# TYPE {} gauge", g.name).ok();
    let mut rows: Vec<(LabelSet, f64)> = g
        .series
        .iter()
        .map(|kv| (kv.key().clone(), kv.value().get()))
        .collect();
    rows.sort_by(|a, b| a.0.pairs().cmp(b.0.pairs()));
    for (labels, v) in rows {
        writeln!(out, "{}{} {}", g.name, labels.render(), format_float(v)).ok();
    }
}

fn render_histogram(h: &LabeledHistogram, out: &mut String) {
    writeln!(out, "# HELP {} {}", h.name, h.help).ok();
    writeln!(out, "# TYPE {} histogram", h.name).ok();
    let mut rows: Vec<(LabelSet, Arc<Histogram>)> = h
        .series
        .iter()
        .map(|kv| (kv.key().clone(), kv.value().clone()))
        .collect();
    rows.sort_by(|a, b| a.0.pairs().cmp(b.0.pairs()));
    for (labels, hist) in rows {
        let base = labels.render();
        let base_inner: &str = if base.is_empty() {
            ""
        } else {
            &base[1..base.len() - 1]
        };
        let cumulative = hist.cumulative_counts();
        for (i, ub) in hist.buckets.iter().enumerate() {
            let labels_str = if base_inner.is_empty() {
                format!("{{le=\"{}\"}}", format_float(*ub))
            } else {
                format!("{{{},le=\"{}\"}}", base_inner, format_float(*ub))
            };
            writeln!(out, "{}_bucket{} {}", h.name, labels_str, cumulative[i]).ok();
        }
        let inf_labels = if base_inner.is_empty() {
            "{le=\"+Inf\"}".to_string()
        } else {
            format!("{{{},le=\"+Inf\"}}", base_inner)
        };
        writeln!(
            out,
            "{}_bucket{} {}",
            h.name,
            inf_labels,
            cumulative[hist.buckets.len()]
        )
        .ok();
        let sum = *hist.sum.read();
        let count = hist.count.load(Ordering::Relaxed);
        writeln!(out, "{}_sum{} {}", h.name, base, format_float(sum)).ok();
        writeln!(out, "{}_count{} {}", h.name, base, count).ok();
    }
}

/// Render an f64 in a Prometheus-friendly way: integers stay un-fractional,
/// otherwise use the default Display impl (no scientific notation issues for
/// the magnitudes we emit).
fn format_float(v: f64) -> String {
    if v.is_finite() && v.fract() == 0.0 && v.abs() < 1e16 {
        format!("{}", v as i64)
    } else {
        format!("{}", v)
    }
}

static REGISTRY: OnceCell<Arc<Registry>> = OnceCell::new();

/// Returns the process-wide metrics registry, initializing it on first use.
pub fn registry() -> Arc<Registry> {
    REGISTRY.get_or_init(|| Arc::new(Registry::new())).clone()
}

/// Record a completed request: bumps `submux_requests_total` and observes the
/// elapsed time on `submux_request_duration_seconds`.
pub fn record_request(protocol: &str, model_group: &str, status: &str, duration_secs: f64) {
    let r = registry();
    r.requests_total.inc(&[
        ("protocol", protocol),
        ("model_group", model_group),
        ("status", status),
    ]);
    r.request_duration_seconds.observe(
        &[("protocol", protocol), ("model_group", model_group)],
        duration_secs,
    );
}

pub fn set_account_health(account_id: &str, provider: &str, health: f64) {
    registry().account_health.set(
        &[("account_id", account_id), ("provider", provider)],
        health,
    );
}

pub fn set_account_cooldown_active(account_id: &str, active: bool) {
    registry().account_cooldown_active.set(
        &[("account_id", account_id)],
        if active { 1.0 } else { 0.0 },
    );
}

pub fn set_account_quota_utilization(account_id: &str, window: &str, utilization: f64) {
    registry().account_quota_utilization.set(
        &[("account_id", account_id), ("window", window)],
        utilization,
    );
}

pub fn set_account_in_flight(account_id: &str, n: u32) {
    registry()
        .account_in_flight
        .set(&[("account_id", account_id)], n as f64);
}

pub fn add_tokens(account_id: &str, direction: &str, n: u64) {
    registry()
        .tokens_total
        .inc_by(&[("account_id", account_id), ("direction", direction)], n);
}

pub fn record_refresh_attempt(account_id: &str, success: bool) {
    let result = if success { "success" } else { "failure" };
    registry()
        .refresh_attempts_total
        .inc(&[("account_id", account_id), ("result", result)]);
}

pub fn record_stream_translate_chunk(from_protocol: &str, to_protocol: &str) {
    registry().stream_translate_chunks_total.inc(&[
        ("from_protocol", from_protocol),
        ("to_protocol", to_protocol),
    ]);
}

pub fn record_stream_backpressure_drop() {
    registry().stream_backpressure_drops_total.inc();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn counter_inc_accumulates() {
        let c = Counter::new();
        c.inc();
        c.inc_by(4);
        assert_eq!(c.get(), 5);
    }

    #[test]
    fn gauge_set_overwrites_and_add_accumulates() {
        let g = Gauge::new();
        g.set(3.5);
        assert!((g.get() - 3.5).abs() < 1e-9);
        g.add(1.0);
        assert!((g.get() - 4.5).abs() < 1e-9);
    }

    #[test]
    fn histogram_observe_distributes_into_buckets() {
        let h = Histogram::new(&[0.1, 1.0, 10.0]);
        h.observe(0.05);
        h.observe(0.5);
        h.observe(2.0);
        h.observe(100.0);
        let cum = h.cumulative_counts();
        assert_eq!(cum, vec![1, 2, 3, 4]);
        assert_eq!(h.count.load(Ordering::Relaxed), 4);
        let sum = *h.sum.read();
        assert!((sum - (0.05 + 0.5 + 2.0 + 100.0)).abs() < 1e-9);
    }

    #[test]
    fn label_set_is_canonicalized() {
        let a = LabelSet::new(&[("b", "2"), ("a", "1")]);
        let b = LabelSet::new(&[("a", "1"), ("b", "2")]);
        assert_eq!(a, b);
        assert_eq!(a.render(), r#"{a="1",b="2"}"#);
    }

    #[test]
    fn label_value_escaping() {
        let l = LabelSet::new(&[("k", "a\\b\"c\nd")]);
        assert_eq!(l.render(), r#"{k="a\\b\"c\nd"}"#);
    }

    #[test]
    fn render_prometheus_snapshot_for_known_input() {
        let r = Registry::new();
        r.requests_total.inc(&[
            ("protocol", "anthropic"),
            ("model_group", "claude-3-5-sonnet"),
            ("status", "ok"),
        ]);
        r.request_duration_seconds.observe(
            &[
                ("protocol", "anthropic"),
                ("model_group", "claude-3-5-sonnet"),
            ],
            0.3,
        );
        r.account_in_flight.set(&[("account_id", "acct_1")], 2.0);
        r.tokens_total
            .inc_by(&[("account_id", "acct_1"), ("direction", "input")], 100);
        r.stream_backpressure_drops_total.inc();

        let out = r.render_prometheus();

        assert!(out.contains("# TYPE submux_requests_total counter"));
        assert!(out.contains(
            "submux_requests_total{model_group=\"claude-3-5-sonnet\",protocol=\"anthropic\",status=\"ok\"} 1"
        ));
        assert!(out.contains("# TYPE submux_request_duration_seconds histogram"));
        assert!(out.contains(
            "submux_request_duration_seconds_bucket{model_group=\"claude-3-5-sonnet\",protocol=\"anthropic\",le=\"0.5\"} 1"
        ));
        assert!(out.contains(
            "submux_request_duration_seconds_bucket{model_group=\"claude-3-5-sonnet\",protocol=\"anthropic\",le=\"+Inf\"} 1"
        ));
        assert!(out.contains(
            "submux_request_duration_seconds_count{model_group=\"claude-3-5-sonnet\",protocol=\"anthropic\"} 1"
        ));
        assert!(out.contains("submux_account_in_flight{account_id=\"acct_1\"} 2"));
        assert!(out.contains("submux_tokens_total{account_id=\"acct_1\",direction=\"input\"} 100"));
        assert!(out.contains("submux_stream_backpressure_drops_total 1"));
    }

    #[test]
    fn typed_helpers_route_to_correct_metrics() {
        record_request("openai", "gpt-4o", "ok", 0.12);
        set_account_in_flight("acct_2", 3);
        add_tokens("acct_2", "output", 42);
        record_refresh_attempt("acct_2", true);
        record_stream_translate_chunk("anthropic", "openai");
        record_stream_backpressure_drop();

        let out = registry().render_prometheus();
        assert!(out.contains("submux_requests_total"));
        assert!(out.contains("model_group=\"gpt-4o\""));
        assert!(out.contains("submux_refresh_attempts_total"));
        assert!(out.contains("result=\"success\""));
        assert!(out.contains("from_protocol=\"anthropic\""));
        assert!(out.contains("to_protocol=\"openai\""));
    }
}
