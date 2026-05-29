# Architecture

## Layout

```
src/
├── main.rs                       Binary entry: parse → resolve → seed → serve.
├── lib.rs                        Module roots + re-exports.
├── cli.rs                        clap flags (--config, -q, -v). No subcommands.
├── core/                         Pure types: AccountId, ApiKey, Credentials, Session, ProviderKind, AdapterError, ResponseStream.
├── config/
│   ├── file.rs                   TOML loader + skeleton writer + redacted render.
│   ├── settings.rs               Resolved Settings (env > file > auto-discovery > none).
│   └── token_source.rs           TokenSource enum surfaced on the banner.
├── accounts/
│   ├── account.rs                Account + AccountState (atomics + ArcSwap).
│   ├── pool.rs                   AccountPool (DashMap by id + by_provider).
│   ├── refresh.rs                RefreshManager (singleflight via DashMap + Shared).
│   ├── cookie_jar.rs             Per-account cookie jar shape.
│   ├── cooldown.rs               CooldownCache (Moka L1).
│   ├── quota.rs                  QuotaSnapshot.
│   ├── seed.rs                   Settings → AccountPool.
│   └── discovery/                Read official CLI credential stores at boot.
│       ├── anthropic.rs          Claude Code keychain item + Linux fallback file.
│       └── codex.rs              ~/.codex/auth.json reader.
├── providers/
│   ├── anthropic/
│   │   ├── proxy.rs              AnthropicProxy (passthrough + body cloak + headers).
│   │   ├── headers.rs            Stainless + Claude Code header builders.
│   │   ├── cloak.rs              JSON body cloak (`cloak_bytes`).
│   │   ├── oauth.rs              `exchange_refresh_token` for refresh-on-401.
│   │   └── quota.rs              Parse Anthropic ratelimit headers → cooldown.
│   └── codex/
│       ├── proxy.rs              CodexProxy (handle around CodexSessionAdapter).
│       ├── session.rs            Codex passthrough — Bearer + device-id + cookies.
│       └── cookies.rs            Cookie jar → Cookie header value.
├── protocols/
│   ├── anthropic/                Anthropic Messages parse/emit + Anthropic ↔ Responses translator.
│   └── openai/                   OpenAI Chat parse + translate_in (→ Normalized) +
│                                 translate_out (AnthropicToOpenAiTranslator) +
│                                 emit (chunk_to_sse) + collapse (chunks → completion).
├── streaming/
│   ├── sse_parser.rs             SseStreamParser (line-buffer → SseEvent).
│   ├── sse_emitter.rs            Encode SseEvent → bytes.
│   ├── anthropic_events.rs       Typed Anthropic SSE event variants.
│   ├── openai_chunks.rs          Typed OpenAI chat-completion chunk variants.
│   ├── translate.rs              AnthropicToOpenAiTranslator state machine.
│   ├── translate_responses_to_anthropic.rs
│   │                             Codex Responses → Anthropic Messages translator.
│   ├── relay.rs                  SseTranslator trait + relay::drive shared unfold loop.
│   └── chat_relay.rs             Wrapper adapting AnthropicToOpenAiTranslator to relay::drive.
├── server/
│   ├── app.rs                    AppState + build_app (open + gated route groups).
│   ├── banner.rs                 Startup banner + structured tracing event.
│   ├── responses.rs              Error envelope helpers + metric stamping.
│   ├── shutdown.rs               Ctrl-C / SIGTERM signal listener.
│   ├── middleware/
│   │   ├── auth.rs               Optional API key gate (Authorization / x-api-key).
│   │   ├── request_id.rs         X-Request-Id propagation.
│   │   └── panic_catch.rs        Panic → typed JSON 500.
│   └── routes/
│       ├── messages.rs           POST /v1/messages
│       ├── chat.rs               POST /v1/chat/completions
│       ├── codex.rs              POST /codex/responses
│       ├── codex_messages.rs     POST /codex/v1/messages
│       ├── health.rs             GET /health, /ready
│       └── metrics.rs            GET /metrics (Prometheus text)
├── telemetry/
│   ├── metrics.rs                Registry + typed helpers.
│   ├── tracer.rs                 new_request_id() + OTel stub.
│   └── exporters/prometheus.rs   GET /metrics handler.
└── constants/
    ├── http_headers.rs
    ├── upstream_paths.rs
    ├── user_agents.rs
    └── limits.rs
```

## Request path

Four inbound shapes, each with one route handler. All four reach the
provider through `AppState`; nothing skips the proxy layer.

1. **`POST /v1/messages`** — Anthropic Messages passthrough.
   `routes/messages.rs` → `AnthropicProxy::passthrough(&headers, body, &token)` →
   `https://api.anthropic.com/v1/messages`. On 401 with a refresh token,
   `RefreshManager::refresh` coalesces concurrent refreshers via singleflight
   and the route retries once. Quota headers are parsed
   (`providers::anthropic::quota::parse_quota_headers`) and applied to the
   account's cooldown after every passthrough.

2. **`POST /v1/chat/completions`** — OpenAI Chat → Anthropic translation.
   `routes/chat.rs` parses an OpenAI Chat body, calls
   `protocols::openai::translate_in::openai_to_normalized`, forces
   `stream: true` to upstream, sends to the Anthropic proxy, then either
   streams via `streaming::relay::drive` with an `AnthropicToOpenAiRelay`
   translator (Anthropic SSE → OpenAI chunks), or collapses chunks into a
   single non-streaming `chat.completion` via
   `protocols::openai::collapse::chunks_to_completion`.

3. **`POST /codex/responses`** — Codex passthrough.
   `routes/codex.rs` → `CodexProxy::codex.passthrough(&headers, body, &creds)` →
   `https://chatgpt.com/backend-api/codex/responses` with the Codex CLI
   fingerprint (Bearer + `codex-cli/<v>` UA + device id + cookies if any).

4. **`POST /codex/v1/messages`** — Anthropic Messages → Codex Responses.
   `routes/codex_messages.rs` translates the request body
   (`protocols::anthropic::translate_to_responses::anthropic_to_responses_body`),
   drives the Codex passthrough, and translates the SSE stream back via
   `streaming::relay::drive` with a `ResponsesToAnthropicTranslator`.
   Lets stock Claude Code clients
   (`ANTHROPIC_BASE_URL=http://submux/codex`) speak to a ChatGPT
   subscription end to end.

Health: `routes/health.rs` (`/health`, `/ready`). Metrics:
`routes/metrics.rs` (`/metrics`, Prometheus text exposition). These three
stay open even when the inbound API key gate is enabled.

## Configuration

`Settings` (`config/settings.rs`) is the single resolved value all other
modules consume. Resolution order, per field:

```
env (SUBMUX_*)  >  TOML config file  >  auto-discovery  >  default / none
```

Auto-discovery (`accounts/discovery/`) reads from where the official CLIs
already store credentials:

- **Anthropic**: macOS keychain item `Claude Code-credentials` (raw or
  hex-encoded JSON, both formats supported) → Linux fallback
  `~/.claude/.credentials.json`.
- **Codex**: `~/.codex/auth.json` when `auth_mode == "chatgpt"`.

Each provider's `Settings` block carries a `source: TokenSource` so the
startup banner shows exactly where the credentials came from (env, config
file, claude keychain, claude file, or codex login).

## State

`AppState` (`server/app.rs`) holds `Arc`-shared singletons:

- `pool: Arc<AccountPool>` — `DashMap<AccountId, Arc<Account>>` + `by_provider` index.
- `anthropic: Arc<AnthropicProxy>` — pinned at boot.
- `refresh: Arc<RefreshManager>` — singleflight for refresh-on-401.
- `cooldown: Arc<CooldownCache>` — Moka L1 keyed by `AccountId`.
- `http: Arc<reqwest::Client>` — shared HTTP/2 pool, also reused by the Anthropic proxy.
- `api_key: Option<Arc<ApiKey>>` — when `Some`, the inbound auth gate is applied to all proxy routes.

The Codex proxy is installed into a process-wide `OnceCell` at boot
(`providers::codex::install`) so it doesn't need to be threaded through
every layer. `providers::codex::current()` is the read accessor used by
the two Codex route handlers.

Per-account state (`AccountState`) lives on each `Account`: atomics for
`in_flight` / `permanently_disabled` / `needs_reauth`, `ArcSwapOption`
for `cooldown_until` / `last_429_at` / `last_used_at` /
`quota_5h_reset_at`, `Semaphore` for per-account concurrency, `Mutex<()>`
for the refresh lease.

## Proxies

### AnthropicProxy (`providers/anthropic/proxy.rs`)

- Strips hop-by-hop and Stainless-prefixed headers from the inbound request.
- Layers `cloak_headers(&fingerprint, oauth_token)` — Authorization, UA,
  `anthropic-beta: oauth-2025-04-20`,
  `anthropic-dangerous-direct-browser-access: true`, Stainless fingerprint
  headers.
- Merges any `anthropic-beta` values the client sent alongside ours.
- Cloaks the JSON body via `cloak::cloak_bytes` — appends Claude Code's
  identity block to the last `system` entry. Non-JSON bodies pass through
  untouched.
- Streams the response back without buffering; non-2xx bodies are still
  streamed as a single `Bytes` so callers can branch on the status.

### CodexProxy / CodexSessionAdapter (`providers/codex/`)

- Strips hop-by-hop, Authorization, Cookie, User-Agent from inbound
  headers.
- Layers the Codex CLI cloak: `Bearer <access_token>`,
  `codex-cli/<v>` UA, `Origin: https://chatgpt.com`,
  `Referer: https://chatgpt.com/`, `OpenAI-Device-Id`.
- Sentinel + arkose tokens are sent empty. Deferred — works for the
  common case; some Codex request classes will hit a 4xx until the
  upstream anti-abuse contract is reverse-engineered.
- Cookies are pulled from the per-account `SerializedCookieJar` via
  `cookies::cookie_header_value` only when the jar is non-empty.
  Auto-discovered Codex credentials carry no cookies; the JWT bearer is
  sufficient.

## Translation

`protocols/openai/translate_in.rs` converts an OpenAI Chat request into
the internal `NormalizedRequest`. `streaming/translate.rs` runs an SSE
state machine that consumes typed `AnthropicEvent`s and emits typed
`OpenAiChatChunk`s. `protocols/openai/emit.rs` encodes those chunks to
SSE bytes; `protocols/openai/collapse.rs` folds them into a single
non-streaming `chat.completion` JSON.

`protocols/anthropic/translate_to_responses.rs` converts an Anthropic
Messages request body into the Codex Responses input shape; the inverse
translator (`streaming/translate_responses_to_anthropic.rs`) handles the
SSE stream from Codex back to Anthropic Messages SSE.

Image / document / `thinking` content blocks are dropped during the
Anthropic → Responses translation today. Text and `tool_use` translate
cleanly.

## Streaming relay

Both the chat (`Anthropic → OpenAI`) and codex-messages
(`Responses → Anthropic`) routes share a single unfold loop in
`streaming/relay.rs`. The trait is `SseTranslator { ingest(SseEvent) →
Vec<Bytes>, finalize() → Vec<Bytes> }`. `relay::drive(upstream,
translator, trailer)` returns an `impl Stream` ready to plug into
`axum::body::Body::from_stream`. The trailer is the only difference
between the two routes: chat appends `data: [DONE]\n\n`, codex appends
nothing.

## Refresh-on-401 (Anthropic)

1. Proxy returns `PassthroughResponse { status: 401, … }`.
2. Route calls `RefreshManager::refresh(account_id, || async {
   exchange_refresh_token(...) })`.
3. `RefreshManager` deduplicates concurrent callers via `DashMap::entry`
   + `futures::future::Shared`. Only one refresh hits the network per
   account.
4. On success, `Account::update_anthropic_oauth_token` swaps tokens +
   `expires_at`.
5. Route retries the passthrough once with the new bearer.

## Telemetry

- **Metrics** (`telemetry/metrics.rs`) — Hand-rolled counters and
  histograms. `submux_requests_total{protocol, model_group, status}` is
  the canonical request counter;
  `submux_request_duration_seconds` is the histogram. The exporter
  renders Prometheus text in `telemetry/exporters/prometheus.rs`.
- **Tracer** (`telemetry/tracer.rs`) — `new_request_id()` returns a
  ULID-formatted string for log correlation. OpenTelemetry init is a
  stub today; structured `tracing` lines already flow into OpenObserve /
  Grafana Loki / similar via stdout/stderr.

There is no event bus. Routes call `telemetry::metrics::record_*`
directly.

## Middleware

`build_app` splits routes into two groups:

- **Open**: `/health`, `/ready`, `/metrics`.
- **Gated**: every `/v1/*` and `/codex/*` route. The
  `require_api_key` middleware is applied here when `AppState::api_key`
  is `Some`.

Then layers (outermost first):

1. `request_id_middleware` — reuses inbound `X-Request-Id` or generates one.
2. `tower_http::trace::TraceLayer::new_for_http()`.
3. `tower_http::limit::RequestBodyLimitLayer::new(REQUEST_BODY_LIMIT_BYTES)`.
4. `tower_http::timeout::TimeoutLayer::with_status_code(504, REQUEST_TIMEOUT)`.
5. `panic_catch::layer()` — converts panics to a typed `submux_panic` JSON 500.

Body limit and timeout are constants in `src/constants/limits.rs`.

## Constants

Anything referenced by more than one module or defining an external
contract belongs in `src/constants/`. Magic numbers used by a single
function stay local.

- `http_headers.rs` — header name `HeaderName` constants and the
  `HOP_BY_HOP_*` lists.
- `upstream_paths.rs` — `ANTHROPIC_MESSAGES_PATH`,
  `CODEX_RESPONSES_PATH`, `CODEX_REFRESH_ENDPOINT`, default upstream
  hosts.
- `user_agents.rs` — `CODEX_CLI_USER_AGENT` (with the pinned version).
- `limits.rs` — body limits, timeouts, semaphore caps, cooldown
  thresholds.

## Testing

Inline `#[cfg(test)] mod tests` per file. Run with `cargo test --lib`.
No integration suite today.

## Deployment

Single binary, single config surface (TOML + env), no external state.
The config skeleton is written at first boot to
`$XDG_CONFIG_HOME/submux/config.toml` with mode `0600` on Unix.
