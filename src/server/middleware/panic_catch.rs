//! Catch-panic middleware: converts handler panics into a structured JSON 500
//! instead of dropping the connection.

use std::any::Any;

use axum::body::Body;
use http::{header, HeaderValue, Response, StatusCode};
use tower_http::catch_panic::{CatchPanicLayer, ResponseForPanic};

const PANIC_BODY: &[u8] =
    br#"{"type":"error","error":{"type":"submux_panic","message":"internal server panic"}}"#;

/// Response-for-panic implementation that logs the panic and returns the
/// submux error envelope as JSON.
#[derive(Clone, Copy, Debug, Default)]
pub struct SubmuxPanicResponse;

impl ResponseForPanic for SubmuxPanicResponse {
    type ResponseBody = Body;

    fn response_for_panic(
        &mut self,
        err: Box<dyn Any + Send + 'static>,
    ) -> Response<Self::ResponseBody> {
        let message = if let Some(s) = err.downcast_ref::<&'static str>() {
            (*s).to_string()
        } else if let Some(s) = err.downcast_ref::<String>() {
            s.clone()
        } else {
            "unknown panic payload".to_string()
        };

        tracing::error!(target: "submux::panic", panic_message = %message, "handler panicked");

        let mut response = Response::new(Body::from(PANIC_BODY));
        *response.status_mut() = StatusCode::INTERNAL_SERVER_ERROR;
        response.headers_mut().insert(
            header::CONTENT_TYPE,
            HeaderValue::from_static("application/json"),
        );
        response
    }
}

/// Returns the `CatchPanicLayer` wired with our custom response handler.
pub fn layer() -> CatchPanicLayer<SubmuxPanicResponse> {
    CatchPanicLayer::custom(SubmuxPanicResponse)
}
