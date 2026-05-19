use axum::{routing::get, Router};

use crate::server::app::AppState;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/healthz", get(|| async { "ok" }))
        .route("/readyz", get(|| async { "ok" }))
}
