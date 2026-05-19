//! Codex (ChatGPT Plus subscription) session adapter.
//!
//! Byte-for-byte passthrough to `chatgpt.com/backend-api/codex/responses` that
//! cloaks the request as legitimate Codex CLI traffic: Codex CLI User-Agent,
//! device id, session bearer, and the per-account cookie jar.

use bytes::Bytes;
use futures::StreamExt;
use http::{HeaderMap, HeaderName, HeaderValue};
use reqwest::StatusCode;
use std::sync::Arc;
use url::Url;

use crate::constants::http_headers::HOP_BY_HOP_CODEX;
use crate::constants::limits::{
    UPSTREAM_CONNECT_TIMEOUT, UPSTREAM_H2_KEEPALIVE_INTERVAL, UPSTREAM_H2_KEEPALIVE_TIMEOUT,
    UPSTREAM_POOL_IDLE_TIMEOUT, UPSTREAM_REQUEST_TIMEOUT, UPSTREAM_TCP_KEEPALIVE,
};
use crate::constants::upstream_paths::CODEX_RESPONSES_PATH;
use crate::constants::user_agents::CODEX_CLI_USER_AGENT;
use crate::core::{AdapterError, Credentials, ResponseStream, TransientKind};
use crate::providers::openai::cookies::cookie_header_value;

pub struct CodexSessionAdapter {
    pub http: Arc<reqwest::Client>,
    pub upstream: Url,
}

pub struct CodexPassthroughResponse {
    pub status: StatusCode,
    pub headers: HeaderMap,
    pub stream: ResponseStream,
}

impl CodexSessionAdapter {
    pub fn new(http: Arc<reqwest::Client>, upstream: Url) -> Self {
        Self { http, upstream }
    }

    pub fn default_client() -> reqwest::Client {
        reqwest::Client::builder()
            .pool_idle_timeout(UPSTREAM_POOL_IDLE_TIMEOUT)
            .tcp_keepalive(UPSTREAM_TCP_KEEPALIVE)
            .http2_keep_alive_interval(UPSTREAM_H2_KEEPALIVE_INTERVAL)
            .http2_keep_alive_timeout(UPSTREAM_H2_KEEPALIVE_TIMEOUT)
            .timeout(UPSTREAM_REQUEST_TIMEOUT)
            .connect_timeout(UPSTREAM_CONNECT_TIMEOUT)
            .build()
            .expect("static reqwest client config is valid")
    }

    /// Byte-for-byte passthrough to `POST {upstream}/backend-api/codex/responses`.
    ///
    /// Codex auth is Bearer + cookies; fingerprint is device id + sentinel pair.
    /// Body is forwarded verbatim — no JSON cloak (Codex doesn't inspect bodies
    /// the way Anthropic's OAuth gate does).
    pub async fn passthrough(
        &self,
        client_headers: &HeaderMap,
        body: Bytes,
        creds: &Credentials,
    ) -> Result<CodexPassthroughResponse, AdapterError> {
        let (access_token, cookies, device_id) = match creds {
            Credentials::OpenAiChatGptSession {
                access_token,
                cookies,
                device_id,
                ..
            } => (access_token, cookies, device_id),
            _ => {
                return Err(AdapterError::Internal(
                    "wrong credential kind for CodexSessionAdapter".into(),
                ));
            }
        };

        let mut upstream_url = self.upstream.clone();
        upstream_url.set_path(CODEX_RESPONSES_PATH);

        let mut req_headers = HeaderMap::new();
        for (name, value) in client_headers.iter() {
            if HOP_BY_HOP_CODEX
                .iter()
                .any(|h| name.as_str().eq_ignore_ascii_case(h))
            {
                continue;
            }
            req_headers.append(name.clone(), value.clone());
        }

        insert_static(
            &mut req_headers,
            "authorization",
            &format!("Bearer {access_token}"),
        );
        insert_static(&mut req_headers, "user-agent", CODEX_CLI_USER_AGENT);
        insert_static(&mut req_headers, "accept", "text/event-stream");
        insert_static(&mut req_headers, "origin", "https://chatgpt.com");
        insert_static(&mut req_headers, "referer", "https://chatgpt.com/");
        insert_static(&mut req_headers, "openai-device-id", device_id);

        // FIXME: sentinel handshake unimplemented; some Codex request classes will 4xx.
        insert_static(
            &mut req_headers,
            "openai-sentinel-chat-requirements-token",
            "",
        );
        insert_static(&mut req_headers, "openai-sentinel-arkose-token", "");

        if let Some(cookie_header) = cookie_header_value(cookies, &upstream_url) {
            if let Ok(v) = HeaderValue::from_str(&cookie_header) {
                req_headers.insert(HeaderName::from_static("cookie"), v);
            }
        }

        let response = self
            .http
            .post(upstream_url)
            .headers(req_headers)
            .body(body)
            .send()
            .await
            .map_err(map_reqwest_send_error)?;

        let status = response.status();
        let mut headers = HeaderMap::new();
        let mut set_cookies: Vec<String> = Vec::new();
        for (name, value) in response.headers().iter() {
            let n = name.as_str();
            if n.eq_ignore_ascii_case("set-cookie") {
                if let Ok(s) = value.to_str() {
                    set_cookies.push(s.to_owned());
                }
                continue;
            }
            if n.eq_ignore_ascii_case("content-length")
                || n.eq_ignore_ascii_case("transfer-encoding")
                || n.eq_ignore_ascii_case("connection")
            {
                continue;
            }
            headers.append(name.clone(), value.clone());
        }

        if !set_cookies.is_empty() {
            // TODO: merge upstream Set-Cookie back into the account jar once the route owns it.
            tracing::debug!(
                count = set_cookies.len(),
                "codex upstream returned Set-Cookie headers; account-side merge not wired"
            );
        }

        if !status.is_success() {
            let bytes = response.bytes().await.unwrap_or_default();
            let single =
                futures::stream::once(async move { Ok::<Bytes, AdapterError>(bytes) }).boxed();
            return Ok(CodexPassthroughResponse {
                status,
                headers,
                stream: single,
            });
        }

        let byte_stream = response
            .bytes_stream()
            .map(|chunk| chunk.map_err(map_reqwest_body_error))
            .boxed();

        Ok(CodexPassthroughResponse {
            status,
            headers,
            stream: byte_stream,
        })
    }
}

fn insert_static(headers: &mut HeaderMap, name: &'static str, value: &str) {
    if let Ok(v) = HeaderValue::from_str(value) {
        headers.insert(HeaderName::from_static(name), v);
    }
}

fn map_reqwest_send_error(e: reqwest::Error) -> AdapterError {
    if e.is_timeout() {
        AdapterError::Transient {
            cause: TransientKind::ReadTimeout,
        }
    } else if e.is_connect() {
        AdapterError::Transient {
            cause: TransientKind::Connect,
        }
    } else if e.is_request() {
        AdapterError::Internal(format!("request build failed: {e}"))
    } else {
        AdapterError::Transient {
            cause: TransientKind::Other,
        }
    }
}

fn map_reqwest_body_error(e: reqwest::Error) -> AdapterError {
    if e.is_timeout() {
        AdapterError::Transient {
            cause: TransientKind::ReadTimeout,
        }
    } else {
        AdapterError::Transient {
            cause: TransientKind::Reset,
        }
    }
}
