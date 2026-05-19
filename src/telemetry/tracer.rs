//! Tracing seam. The real `opentelemetry` / `opentelemetry-otlp` dependencies
//! are deferred; this module documents where the exporter plugs in and
//! provides the request-id generator that the rest of the stack uses to
//! correlate spans, logs, and event-bus messages.
//!
//! `tracing` + `tracing-subscriber` are initialized in `main.rs`; this module
//! does **not** install a subscriber. Once the OTel crates land, swap the
//! body of [`init_otel`] for the real pipeline builder (tracer provider,
//! batch span processor, OTLP exporter, propagator registration).

use ulid::Ulid;

/// Errors surfaced by the (eventual) OTel pipeline setup.
#[derive(Debug, thiserror::Error)]
pub enum TracerError {
    /// The OpenTelemetry pipeline failed to initialize.
    #[error("otel init failed: {0}")]
    Init(String),
}

/// Initialize the OpenTelemetry pipeline.
///
/// Currently a no-op: the `opentelemetry-*` crates are not yet in `Cargo.toml`.
/// Stdout JSON logs (via `tracing-subscriber`) cover observability in the
/// interim. When the OTel deps land, this should:
///
/// 1. Build an OTLP span exporter pointed at `endpoint` (default
///    `http://localhost:4317` if `None`).
/// 2. Wrap it in a `BatchSpanProcessor`.
/// 3. Register a `TracerProvider` with `service.name = service_name`.
/// 4. Install a `tracing-opentelemetry` layer onto the global subscriber.
/// 5. Register the W3C trace-context propagator so inbound `traceparent`
///    headers chain into the upstream's trace.
pub fn init_otel(_service_name: &str, _endpoint: Option<&str>) -> Result<(), TracerError> {
    tracing::info!(
        target: "submux::telemetry::tracer",
        "init_otel: deferred — opentelemetry crates not yet enabled; using tracing-subscriber only",
    );
    Ok(())
}

/// Generate a new request-id for log + span + event correlation.
///
/// ULID gives us a 26-char base32, k-sortable id that's safe in URLs and
/// log lines.
pub fn new_request_id() -> String {
    format!("req_{}", Ulid::new())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_request_id_has_expected_shape() {
        let id = new_request_id();
        assert!(id.starts_with("req_"), "got: {id}");
        assert_eq!(id.len(), 30, "got: {id}");
    }

    #[test]
    fn new_request_id_is_unique_per_call() {
        let a = new_request_id();
        let b = new_request_id();
        assert_ne!(a, b);
    }

    #[test]
    fn init_otel_is_currently_noop_and_ok() {
        assert!(init_otel("submux", None).is_ok());
        assert!(init_otel("submux", Some("http://collector:4317")).is_ok());
    }
}
