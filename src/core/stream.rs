use bytes::Bytes;
use futures::stream::BoxStream;

use crate::core::AdapterError;

pub type ResponseStream = BoxStream<'static, Result<Bytes, AdapterError>>;
