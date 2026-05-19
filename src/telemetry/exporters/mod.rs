//! Telemetry exporters. Today: Prometheus scrape endpoint. Future: OTLP
//! span/metric pushers will live here too, each as a sibling module so the
//! seam from `mod.rs` stays clean.

pub mod prometheus;
