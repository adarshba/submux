//! Metrics on the OpenTelemetry SDK: one instrument set, two readers — an OTLP
//! push exporter (when an endpoint is configured) and an `opentelemetry-prometheus`
//! reader backing `/metrics`.
//!
//! Surface (Prometheus names):
//! - `submux_requests_total{protocol,model_group,status,consumer}`
//! - `submux_request_duration_seconds{protocol,model_group,consumer}`
//! - `submux_tokens_total{consumer,protocol,direction,model}`
//! - `submux_refresh_attempts_total{account_id,result}`
//! - `submux_account_cooldown_active{account_id}`
//! - `submux_account_quota_utilization{account_id,window}`

use once_cell::sync::OnceCell;
use opentelemetry::metrics::{Counter, Gauge, Histogram, Meter};
use opentelemetry::{KeyValue, global};
use opentelemetry_otlp::{Protocol, WithExportConfig};
use opentelemetry_sdk::Resource;
use opentelemetry_sdk::metrics::SdkMeterProvider;
use prometheus::{Encoder, TextEncoder};
use thiserror::Error;

/// Histogram bucket boundaries (seconds) for request duration.
pub const DEFAULT_DURATION_BUCKETS: &[f64] = &[0.05, 0.1, 0.25, 0.5, 1.0, 2.5, 5.0, 10.0, 30.0];

/// Failure modes for the metrics pipeline setup.
#[derive(Debug, Error)]
pub enum MetricsError {
    /// The OpenTelemetry pipeline failed to initialize.
    #[error("metrics init failed: {0}")]
    Init(String),
}

/// Inputs for [`init`]. Built in `main.rs` from env/config.
#[derive(Debug, Clone)]
pub struct OtelConfig {
    /// `service.version` resource attribute.
    pub service_version: String,
    /// OTLP/HTTP base endpoint (e.g. `http://localhost:4318`). When `None`,
    /// only the Prometheus `/metrics` reader is installed — no push exporter.
    pub otlp_endpoint: Option<String>,
}

/// The process-wide instrument set, created once against the global meter.
struct Instruments {
    requests: Counter<u64>,
    request_duration: Histogram<f64>,
    tokens: Counter<u64>,
    refresh_attempts: Counter<u64>,
    account_cooldown_active: Gauge<u64>,
    account_quota_utilization: Gauge<f64>,
}

static INSTRUMENTS: OnceCell<Instruments> = OnceCell::new();
static PROM_REGISTRY: OnceCell<prometheus::Registry> = OnceCell::new();

/// Counter names omit `_total`; the exporters append it per convention.
fn build_instruments(meter: &Meter) -> Instruments {
    Instruments {
        requests: meter
            .u64_counter("submux_requests")
            .with_description("Total inbound requests by protocol, model group, status, consumer.")
            .build(),
        request_duration: meter
            .f64_histogram("submux_request_duration")
            .with_description("End-to-end request duration in seconds.")
            .with_unit("s")
            .with_boundaries(DEFAULT_DURATION_BUCKETS.to_vec())
            .build(),
        tokens: meter
            .u64_counter("submux_tokens")
            .with_description("Tokens accounted by consumer, protocol, direction, and model.")
            .build(),
        refresh_attempts: meter
            .u64_counter("submux_refresh_attempts")
            .with_description("Credential refresh attempts by account and result.")
            .build(),
        account_cooldown_active: meter
            .u64_gauge("submux_account_cooldown_active")
            .with_description("1 if the account is currently in cooldown, else 0.")
            .build(),
        account_quota_utilization: meter
            .f64_gauge("submux_account_quota_utilization")
            .with_description("Fraction of the account's quota consumed in the window (0..=1).")
            .build(),
    }
}

/// Initialize the metrics pipeline and install the global meter provider.
///
/// Returns the [`SdkMeterProvider`] so the caller can keep it alive for the
/// process lifetime and `shutdown()` it on exit to flush the final export.
pub fn init(config: OtelConfig) -> Result<SdkMeterProvider, MetricsError> {
    let resource = Resource::builder()
        .with_service_name("submux")
        .with_attribute(KeyValue::new("service.version", config.service_version))
        .build();

    let prom_registry = prometheus::Registry::new();
    let prom_reader = opentelemetry_prometheus::exporter()
        .with_registry(prom_registry.clone())
        .build()
        .map_err(|e| MetricsError::Init(e.to_string()))?;

    let mut builder = SdkMeterProvider::builder()
        .with_reader(prom_reader)
        .with_resource(resource);

    if let Some(endpoint) = config.otlp_endpoint.as_deref().filter(|e| !e.is_empty()) {
        let url = format!("{}/v1/metrics", endpoint.trim_end_matches('/'));
        let exporter = opentelemetry_otlp::MetricExporter::builder()
            .with_http()
            .with_protocol(Protocol::HttpBinary)
            .with_endpoint(url)
            .build()
            .map_err(|e| MetricsError::Init(e.to_string()))?;
        builder = builder.with_periodic_exporter(exporter);
    }

    let provider = builder.build();
    global::set_meter_provider(provider.clone());

    let meter = global::meter("submux");
    let _ = INSTRUMENTS.set(build_instruments(&meter));
    let _ = PROM_REGISTRY.set(prom_registry);

    Ok(provider)
}

/// Render the current metrics as Prometheus 0.0.4 exposition text. Empty when
/// [`init`] has not run (e.g. in unit tests that bypass startup).
pub fn render_prometheus_text() -> String {
    let Some(registry) = PROM_REGISTRY.get() else {
        return String::new();
    };
    let mut buf = Vec::new();
    let encoder = TextEncoder::new();
    let families = registry.gather();
    if encoder.encode(&families, &mut buf).is_err() {
        return String::new();
    }
    String::from_utf8(buf).unwrap_or_default()
}

/// Record a completed request: bumps `submux_requests_total` and observes the
/// elapsed time on `submux_request_duration_seconds`.
pub fn record_request(
    protocol: &str,
    model_group: &str,
    status: &str,
    consumer: &str,
    duration_secs: f64,
) {
    let Some(i) = INSTRUMENTS.get() else {
        return;
    };
    i.requests.add(
        1,
        &[
            KeyValue::new("protocol", protocol.to_owned()),
            KeyValue::new("model_group", model_group.to_owned()),
            KeyValue::new("status", status.to_owned()),
            KeyValue::new("consumer", consumer.to_owned()),
        ],
    );
    i.request_duration.record(
        duration_secs,
        &[
            KeyValue::new("protocol", protocol.to_owned()),
            KeyValue::new("model_group", model_group.to_owned()),
            KeyValue::new("consumer", consumer.to_owned()),
        ],
    );
}

/// Add token usage for a consumer, split by protocol, direction
/// (`input`|`output`), and model.
pub fn add_tokens(consumer: &str, protocol: &str, direction: &str, model: &str, n: u64) {
    let Some(i) = INSTRUMENTS.get() else {
        return;
    };
    i.tokens.add(
        n,
        &[
            KeyValue::new("consumer", consumer.to_owned()),
            KeyValue::new("protocol", protocol.to_owned()),
            KeyValue::new("direction", direction.to_owned()),
            KeyValue::new("model", model.to_owned()),
        ],
    );
}

pub fn set_account_cooldown_active(account_id: &str, active: bool) {
    if let Some(i) = INSTRUMENTS.get() {
        i.account_cooldown_active.record(
            u64::from(active),
            &[KeyValue::new("account_id", account_id.to_owned())],
        );
    }
}

pub fn set_account_quota_utilization(account_id: &str, window: &str, utilization: f64) {
    if let Some(i) = INSTRUMENTS.get() {
        i.account_quota_utilization.record(
            utilization,
            &[
                KeyValue::new("account_id", account_id.to_owned()),
                KeyValue::new("window", window.to_owned()),
            ],
        );
    }
}

pub fn record_refresh_attempt(account_id: &str, success: bool) {
    let Some(i) = INSTRUMENTS.get() else {
        return;
    };
    let result = if success { "success" } else { "failure" };
    i.refresh_attempts.add(
        1,
        &[
            KeyValue::new("account_id", account_id.to_owned()),
            KeyValue::new("result", result),
        ],
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use opentelemetry::metrics::MeterProvider as _;

    #[test]
    fn prometheus_names_match_dashboard_contract() {
        let registry = prometheus::Registry::new();
        let reader = opentelemetry_prometheus::exporter()
            .with_registry(registry.clone())
            .build()
            .expect("prometheus exporter builds");
        let provider = SdkMeterProvider::builder().with_reader(reader).build();
        let meter = provider.meter("submux");
        let i = build_instruments(&meter);

        i.requests.add(
            1,
            &[
                KeyValue::new("protocol", "anthropic"),
                KeyValue::new("model_group", "claude-opus-4-8"),
                KeyValue::new("status", "200"),
                KeyValue::new("consumer", "usr_test"),
            ],
        );
        i.request_duration
            .record(0.3, &[KeyValue::new("protocol", "anthropic")]);
        i.tokens.add(
            42,
            &[
                KeyValue::new("consumer", "usr_test"),
                KeyValue::new("protocol", "anthropic"),
                KeyValue::new("direction", "output"),
                KeyValue::new("model", "claude-opus-4-8"),
            ],
        );
        i.account_quota_utilization.record(
            0.4,
            &[
                KeyValue::new("account_id", "acct_1"),
                KeyValue::new("window", "5h"),
            ],
        );

        let mut buf = Vec::new();
        TextEncoder::new()
            .encode(&registry.gather(), &mut buf)
            .expect("encode");
        let out = String::from_utf8(buf).expect("utf8");

        assert!(
            out.contains("submux_requests_total"),
            "missing requests_total in:\n{out}"
        );
        assert!(
            out.contains("consumer=\"usr_test\""),
            "missing consumer label"
        );
        assert!(
            out.contains("submux_request_duration_seconds"),
            "missing duration_seconds in:\n{out}"
        );
        assert!(
            out.contains("submux_tokens_total"),
            "missing tokens_total in:\n{out}"
        );
        assert!(
            out.contains("submux_account_quota_utilization"),
            "missing account_quota_utilization in:\n{out}"
        );
    }
}
