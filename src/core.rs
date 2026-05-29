//! Core types and traits. No I/O, no runtime.

pub mod account;
pub mod api_key;
pub mod error;
pub mod message;
pub mod protocol;
pub mod request;
pub mod response;
pub mod session;
pub mod stream;

pub use account::{AccountHandle, AccountId, AccountInner, ProviderKind};
pub use api_key::{ApiKey, ApiKeyError, API_KEY_PREFIX};
pub use error::{AdapterError, ChallengeKind, RlScope, SubmuxError, TransientKind};
pub use message::{ContentBlock, ImageSource, Message, Role, ToolUse};
pub use protocol::ProtocolKind;
pub use request::{NormalizedRequest, Tool};
pub use response::{Delta, FinishReason, NormalizedResponse, Usage};
pub use session::{
    Credentials, FingerprintProfile, KeySource, SerializedCookie, SerializedCookieJar, Session,
};
pub use stream::ResponseStream;
