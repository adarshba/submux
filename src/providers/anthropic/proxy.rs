//! Anthropic OAuth passthrough proxy.
//!
//! Strips client auth/fingerprint headers, layers Claude Code cloak headers
//! and body block, and streams the response without buffering.

use bytes::Bytes;
use futures::StreamExt;
use http::{HeaderMap, HeaderName, HeaderValue};
use reqwest::StatusCode;
use std::sync::Arc;

use crate::constants::http_headers::{HOP_BY_HOP_ANTHROPIC, STAINLESS_PREFIX};
use crate::constants::limits::{
    UPSTREAM_CONNECT_TIMEOUT, UPSTREAM_H2_KEEPALIVE_INTERVAL, UPSTREAM_H2_KEEPALIVE_TIMEOUT,
    UPSTREAM_POOL_IDLE_TIMEOUT, UPSTREAM_REQUEST_TIMEOUT, UPSTREAM_TCP_KEEPALIVE,
};
use crate::constants::upstream_paths::ANTHROPIC_MESSAGES_PATH;
use crate::core::{AdapterError, FingerprintProfile, ResponseStream, TransientKind};
use crate::providers::anthropic::cloak::cloak_bytes;
use crate::providers::anthropic::headers::{cloak_headers, default_claude_code_profile};

pub struct AnthropicProxy {
    pub http: Arc<reqwest::Client>,
    pub upstream: url::Url,
    pub fingerprint: FingerprintProfile,
}

pub struct PassthroughResponse {
    pub status: StatusCode,
    pub headers: HeaderMap,
    pub stream: ResponseStream,
}

impl AnthropicProxy {
    pub fn new(http: Arc<reqwest::Client>, upstream: url::Url) -> Self {
        Self {
            http,
            upstream,
            fingerprint: default_claude_code_profile(),
        }
    }

    /// Override the fingerprint profile after `new`.
    pub fn with_fingerprint(mut self, fingerprint: FingerprintProfile) -> Self {
        self.fingerprint = fingerprint;
        self
    }

    /// Default `reqwest::Client` tuned for long streaming upstream requests.
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

    /// Byte-for-byte passthrough to `POST {upstream}/v1/messages`. Strips
    /// client auth, layers OAuth cloak headers, merges client-side
    /// `anthropic-beta` values alongside ours, and applies the body cloak.
    pub async fn passthrough(
        &self,
        client_headers: &HeaderMap,
        body: Bytes,
        oauth_token: &str,
    ) -> Result<PassthroughResponse, AdapterError> {
        let mut upstream_url = self.upstream.clone();
        upstream_url.set_path(ANTHROPIC_MESSAGES_PATH);

        let req_headers = self.build_request_headers(client_headers, oauth_token);
        let outgoing_body = cloak_bytes(&body).map(Bytes::from).unwrap_or(body);

        let response = self
            .http
            .post(upstream_url)
            .headers(req_headers)
            .body(outgoing_body)
            .send()
            .await
            .map_err(map_reqwest_send_error)?;

        Ok(finalize_response(response).await)
    }

    /// Bodyless passthrough to `GET {upstream}{path}`, used for model-metadata
    /// endpoints (`/v1/models`, `/v1/models/{id}`) that Claude Code probes to
    /// validate the selected model. Reuses the same OAuth cloak headers as the
    /// messages path so the subscription token is accepted upstream.
    pub async fn passthrough_get(
        &self,
        client_headers: &HeaderMap,
        path: &str,
        oauth_token: &str,
    ) -> Result<PassthroughResponse, AdapterError> {
        let mut upstream_url = self.upstream.clone();
        upstream_url.set_path(path);

        let req_headers = self.build_request_headers(client_headers, oauth_token);

        let response = self
            .http
            .get(upstream_url)
            .headers(req_headers)
            .send()
            .await
            .map_err(map_reqwest_send_error)?;

        Ok(finalize_response(response).await)
    }

    /// Strip client auth/fingerprint headers, layer the OAuth cloak headers,
    /// and merge any client-supplied `anthropic-beta` values alongside ours.
    fn build_request_headers(&self, client_headers: &HeaderMap, oauth_token: &str) -> HeaderMap {
        let beta_name = HeaderName::from_static("anthropic-beta");
        let mut client_betas: Vec<String> = Vec::new();
        let mut req_headers = HeaderMap::new();
        for (name, value) in client_headers.iter() {
            if name == beta_name {
                if let Ok(s) = value.to_str() {
                    client_betas.push(s.to_owned());
                }
                continue;
            }
            if HOP_BY_HOP_ANTHROPIC
                .iter()
                .any(|h| name.as_str().eq_ignore_ascii_case(h))
                || name.as_str().starts_with(STAINLESS_PREFIX)
            {
                continue;
            }
            req_headers.append(name.clone(), value.clone());
        }

        let cloak = cloak_headers(&self.fingerprint, oauth_token);
        for (name, value) in cloak.iter() {
            req_headers.insert(name.clone(), value.clone());
        }

        if !client_betas.is_empty() {
            let mut betas: Vec<String> = req_headers
                .get(&beta_name)
                .and_then(|v| v.to_str().ok())
                .map(|s| s.split(',').map(|p| p.trim().to_owned()).collect())
                .unwrap_or_default();
            for b in client_betas.iter().flat_map(|v| v.split(',')) {
                let trimmed = b.trim();
                if !trimmed.is_empty() && !betas.iter().any(|x| x == trimmed) {
                    betas.push(trimmed.to_owned());
                }
            }
            if let Ok(merged) = HeaderValue::from_str(&betas.join(",")) {
                req_headers.insert(&beta_name, merged);
            }
        }

        req_headers
    }
}

/// Split an upstream `reqwest::Response` into a [`PassthroughResponse`]. Error
/// statuses are buffered into a single chunk; success bodies stream unbuffered.
async fn finalize_response(response: reqwest::Response) -> PassthroughResponse {
    let status = response.status();
    let headers = strip_hop_by_hop(response.headers());

    if !status.is_success() {
        let bytes = response.bytes().await.unwrap_or_default();
        let single = futures::stream::once(async move { Ok::<Bytes, AdapterError>(bytes) }).boxed();
        return PassthroughResponse {
            status,
            headers,
            stream: single,
        };
    }

    let byte_stream = response
        .bytes_stream()
        .map(|chunk| chunk.map_err(map_reqwest_body_error))
        .boxed();

    PassthroughResponse {
        status,
        headers,
        stream: byte_stream,
    }
}

fn strip_hop_by_hop(src: &HeaderMap) -> HeaderMap {
    let mut headers = HeaderMap::new();
    for (name, value) in src.iter() {
        let n = name.as_str();
        if n.eq_ignore_ascii_case("content-length")
            || n.eq_ignore_ascii_case("transfer-encoding")
            || n.eq_ignore_ascii_case("connection")
        {
            continue;
        }
        headers.append(name.clone(), value.clone());
    }
    headers
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
