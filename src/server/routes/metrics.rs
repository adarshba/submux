use axum::{Router, routing::get};

use crate::server::app::AppState;
use crate::telemetry::exporters::prometheus::metrics_handler;

pub fn router() -> Router<AppState> {
    Router::new().route("/metrics", get(metrics_handler))
}
