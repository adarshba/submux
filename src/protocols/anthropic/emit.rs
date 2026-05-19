use crate::core::{Delta, NormalizedResponse, SubmuxError};

pub fn render_response(_resp: &NormalizedResponse) -> Result<Vec<u8>, SubmuxError> {
    Err(SubmuxError::Internal("not implemented".into()))
}

pub fn encode_sse_event(_delta: &Delta) -> Result<Vec<u8>, SubmuxError> {
    Err(SubmuxError::Internal("not implemented".into()))
}
