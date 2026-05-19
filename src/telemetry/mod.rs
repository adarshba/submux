//! Telemetry — metrics registry, typed event channel, tracing seam, exporters.

pub mod events;
pub mod exporters;
pub mod metrics;
pub mod tracer;

pub use events::{EventBus, RequestEvent};
pub use metrics::{registry, Registry};
pub use tracer::{init_otel, new_request_id, TracerError};
