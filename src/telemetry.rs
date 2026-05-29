//! Telemetry: OTel metrics (OTLP push + Prometheus), exporters, tracer ids.

pub mod exporters;
pub mod metrics;
pub mod tracer;

pub use metrics::{MetricsError, OtelConfig};
pub use tracer::{TracerError, init_otel, new_request_id};
