# Architecture

## Layout

```
src/
├── main.rs                       Binary entry: tracing init, env-seed accounts, build AppState, axum::serve.
├── lib.rs                        Re-export root.
├── core/                         Pure types: AccountId, ProviderKind, Credentials, Session, AdapterError, ResponseStream.
├── accounts/                     Account, AccountPool, RefreshManager (singleflight), CookieJar, fingerprint.
├── providers/
│   ├── anthropic/                OAuth body cloak + Stainless header cloak + refresh-token exchange.
│   └── openai/                   Codex (chatgpt.com) session adapter + cookie jar handling.
├── protocols/
│   ├── anthropic/                Anthropic Messages parse/emit + SSE event types.
│   └── openai/                   OpenAI Chat ↔ Anthropic Messages translation.
├── router/                       Strategy trait, RoundRobin / HealthAware / QuotaAware / LRU / LatencyAware, cooldown cache, retry policy, failover.
├── streaming/                    SSE parser/emitter, anthropic/openai chunk shapes, translator state machine, checkpointing, tee.
├── coordination/                 CoordinationBackend trait, InMemory default, Redis placeholder behind `redis` feature.
├── storage/                      Postgres-backed account store, XChaCha20Poly1305 sealer, secret newtype.
├── telemetry/                    EventBus (tokio broadcast), Prometheus exporter (hand-rolled), tracer ids, metrics typed helpers.
├── server/
│   ├── app.rs                    AppState + build_app (router merge + middleware stack).
│   ├── middleware/               request_id, panic_catch, trace, auth, rate_limit.
│   ├── routes/                   health, metrics, admin, messages, chat, codex.
│   └── shutdown.rs               Graceful shutdown signal.
├── config/                       Config-file plumbing (deferred; env is canonical today).
└── constants/                    Cross-module constants by topic (http_headers, upstream_paths, user_agents, limits).
```

## Request path

There are three inbound shapes, each with one route handler. None skip the adapter.

1. **`POST /v1/messages`** — Anthropic Messages passthrough.
   `routes/messages.rs` → `AnthropicOAuthAdapter::passthrough(&headers, body, &token)` → `https://api.anthropic.com/v1/messages`.
   On `401` with a refresh token: `RefreshManager::refresh(...)` coalesces concurrent refreshers via singleflight, then retries once.
   Quota headers are parsed (`providers::anthropic::quota::parse_quota_headers`) and applied to the account's cooldown after every passthrough.

2. **`POST /v1/chat/completions`** — OpenAI Chat → Anthropic Messages translation.
   `routes/chat.rs` parses an OpenAI Chat body, calls `protocols::openai::translate_in::openai_to_normalized`, forces `stream: true` to upstream, sends to the Anthropic adapter, then either streams via `streaming::translate` (Anthropic SSE → OpenAI SSE chunks) or collapses chunks into a non-streaming `chat.completion` object.

3. **`POST /codex/responses`** — Codex (chatgpt.com) passthrough.
   `routes/codex.rs` → `OpenAiSubscriptionAdapter::codex.passthrough(&headers, body, &creds)` → `https://chatgpt.com/backend-api/codex/responses` with Codex CLI headers (Authorization Bearer, `codex-cli/<v>` UA, device id, cookies).

Admin paths: `routes/admin.rs` (`/admin/accounts`, `/admin/cooldowns`, `/admin/cooldowns/clear/:id`). Health: `routes/health.rs` (`/healthz`, `/readyz`). Metrics: `routes/metrics.rs` (`/metrics`, Prometheus text exposition).

## State

`AppState` (`server/app.rs`) holds Arc-shared singletons:

- `pool: Arc<AccountPool>` — `DashMap<AccountId, Arc<Account>>` + `by_provider` index.
- `router: Arc<Router>` — strategy + cooldown + retry policy. Routes don't talk to it directly today (passthrough handlers reach for an account themselves); the strategies are exercised by the normalized-request path that lands in a later phase.
- `anthropic: Arc<AnthropicOAuthAdapter>` — pinned at boot.
- `refresh: Arc<RefreshManager>` — singleflight for refresh-on-401.
- `cooldown: Arc<CooldownCache>` — `Moka<AccountId, CooldownEntry>` L1.
- `events: Arc<EventBus>` — tokio broadcast for `RequestEvent`.
- `http: Arc<reqwest::Client>` — shared HTTP/2 pool, also passed to the Anthropic adapter.

The OpenAI (Codex) adapter is installed into a process-wide `OnceCell` at boot (`providers::openai::install_openai_adapter`) because it doesn't need to be threaded through every layer. `openai_adapter()` is the read accessor.

Per-account state (`AccountState`) is held on the `Account` struct: atomics for `in_flight` / `permanently_disabled` / `needs_reauth`, `ArcSwapOption<DateTime<Utc>>` for `cooldown_until` / `last_429_at` / `last_used_at` / `quota_5h_reset_at`, `Semaphore` for per-account concurrency, `Mutex<()>` for the refresh lease.

## Adapters

### Anthropic OAuth (`providers/anthropic/adapter.rs`)

- Strips hop-by-hop and Stainless-prefixed headers from the inbound client request.
- Layers `cloak_headers(&fingerprint, oauth_token)` — Authorization, UA, `anthropic-beta: oauth-2025-04-20`, `anthropic-dangerous-direct-browser-access: true`, Stainless fingerprint headers.
- Merges any `anthropic-beta` values the client sent alongside ours.
- Cloaks the JSON body via `cloak::cloak_bytes` — appends Claude Code's identity block to the last `system` entry. Non-JSON bodies pass through.
- Streams the response back without buffering; non-2xx bodies are still streamed as a single `Bytes` so callers can branch on the status.

### Codex session (`providers/openai/chatgpt_session.rs`)

- Strips hop-by-hop, Authorization, Cookie, User-Agent from inbound headers.
- Layers Codex CLI cloak: `Bearer <access_token>`, `codex-cli/<v>` UA, `Origin: https://chatgpt.com`, `Referer: https://chatgpt.com/`, `OpenAI-Device-Id`.
- Sentinel + arkose tokens are sent empty (`FIXME` — handshake unimplemented).
- Cookies come from the per-account `SerializedCookieJar`, joined via `cookies::cookie_header_value`.
- Response Set-Cookie headers are logged but not merged back into the jar yet.

## Translation

`protocols/openai/translate_in.rs` converts an OpenAI Chat request into our internal `NormalizedRequest` shape. `streaming::translate` runs an SSE state machine that consumes Anthropic events and emits OpenAI `chat.completion.chunk` frames. The non-streaming path in `routes/chat.rs` collapses chunks into a single `chat.completion` JSON via `collapse_chunks_to_completion`.

The Codex path does not translate — Codex consumers speak the OpenAI Responses API natively, and our gateway is a raw passthrough for that surface.

## Refresh-on-401 (Anthropic)

1. Adapter returns `PassthroughResponse { status: 401, … }`.
2. Route calls `RefreshManager::refresh(account_id, || async { exchange_refresh_token(...) })`.
3. `RefreshManager` deduplicates concurrent callers via `DashMap::entry` + `futures::future::Shared`. Only one refresh hits the network per account.
4. On success, `Account::update_anthropic_oauth_token` swaps tokens + `expires_at`.
5. Route retries the passthrough once with the new bearer.

## Coordination

`CoordinationBackend` (trait) lets a multi-replica deployment announce cooldowns, acquire refresh leases across replicas, and broadcast cookie-jar mutations. `InMemoryCoordinator` is the default. The `redis` feature flag is reserved for a Redis-backed impl; the placeholder file does not yet implement the trait.

## Storage

`PostgresAccountStore` (`storage/postgres.rs`) persists accounts in `submux_accounts (id, provider, display_name, sealed_creds, created_at, updated_at)`. Credentials are serialized to JSON, sealed via XChaCha20Poly1305 (`storage/sealer.rs`), and stored as `BYTEA`. The sealer key is loaded from `SUBMUX_SEALER_KEY` as 32-byte hex, base64, or SHA-256-of-passphrase.

Persisted accounts load at boot in `main::load_persisted_accounts` when both `SUBMUX_DATABASE_URL` and `SUBMUX_SEALER_KEY` are set. Missing key with a URL is a warn-and-skip, not a hard error.

## Telemetry

- **Events** (`telemetry/events.rs`) — `tokio::broadcast::channel` with a fixed capacity; lagged subscribers observe `RecvError::Lagged(n)` and resume.
- **Metrics** (`telemetry/metrics.rs`) — Hand-rolled counters and histograms. `submux_requests_total{protocol, model_group, status}` is the canonical request counter; `submux_request_duration_seconds` is the histogram. The exporter renders Prometheus text in `telemetry/exporters/prometheus.rs`.
- **Tracer** (`telemetry/tracer.rs`) — `new_request_id()` returns a ULID-formatted string for log correlation. OpenTelemetry init is a stub.

## Middleware

`build_app` stacks (outermost first):

1. `request_id_middleware` — reuses inbound `X-Request-Id` or generates one.
2. `tower_http::trace::TraceLayer::new_for_http()`.
3. `tower_http::limit::RequestBodyLimitLayer::new(BODY_LIMIT_BYTES)`.
4. `tower_http::timeout::TimeoutLayer::new(REQUEST_TIMEOUT)`.
5. `panic_catch::layer()` — converts panics to a typed `submux_panic` JSON 500.

Body limit and timeout are in `src/constants/limits.rs`.

## Constants

Located in `src/constants/`. Anything referenced by more than one module or defining an external contract belongs here. Magic numbers used by a single function stay local.

- `http_headers.rs` — header name strings + the `HOP_BY_HOP` list.
- `upstream_paths.rs` — `ANTHROPIC_MESSAGES_PATH`, `CODEX_RESPONSES_PATH`, refresh endpoints.
- `user_agents.rs` — `CLAUDE_CODE_USER_AGENT`, `CODEX_CLI_USER_AGENT` (with the pinned versions).
- `limits.rs` — body limits, timeouts, semaphore caps, broadcast capacities.
- `quota.rs` — `COOLDOWN_UTIL_THRESHOLD`, default cooldown window.

## Testing

Inline `#[cfg(test)] mod tests` per file. Run with `cargo test --lib`. No integration suite today — when one is added it lives in `tests/` at the workspace root.

The full smoke test is in [`README.md`](../README.md) under "Local smoke test"; it spins the gateway against real upstreams with fake tokens and validates that `401`s come back from `api.anthropic.com` and `chatgpt.com` with their own request IDs.

## Deployment

Single binary, single config surface (env vars). No external state required unless `SUBMUX_DATABASE_URL` is set. Compose / Kubernetes manifests are out of scope today.
