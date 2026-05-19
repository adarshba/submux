# Submux

Subscription-native LLM gateway in Rust. Treats **accounts** — Claude Max sessions, ChatGPT Plus (Codex) sessions, future session-oriented providers — as the first-class primitive, not API keys.

Four inbound surfaces, all wired to a shared account pool, cooldown cache, and refresh-singleflight manager:

| Route | Upstream | Mode |
|---|---|---|
| `POST /v1/messages` | `api.anthropic.com/v1/messages` | Byte-for-byte passthrough with OAuth body cloak + Stainless headers + `anthropic-beta: oauth-2025-04-20`. Refresh-on-401. |
| `POST /v1/chat/completions` | `api.anthropic.com/v1/messages` | OpenAI Chat → Anthropic Messages translation in, Anthropic SSE → OpenAI chunks out. Streaming and non-streaming. |
| `POST /codex/responses` | `chatgpt.com/backend-api/codex/responses` | Codex CLI passthrough with cookie jar + device id + `codex-cli/<v>` fingerprint. |
| `POST /codex/v1/messages` | `chatgpt.com/backend-api/codex/responses` | Anthropic Messages → Codex Responses. Lets Claude Code (`ANTHROPIC_BASE_URL=http://submux/codex`) drive a ChatGPT Plus account. |

## Quick start

```bash
# Anthropic OAuth account, seeded from env at startup:
export SUBMUX_ANTHROPIC_OAUTH_TOKEN="sk-ant-oat01-..."
export SUBMUX_ANTHROPIC_OAUTH_REFRESH_TOKEN="sk-ant-ort01-..."

# OpenAI (Codex) account:
export SUBMUX_OPENAI_ACCESS_TOKEN="eyJhbGciOi..."
export SUBMUX_OPENAI_COOKIES='{"jar":{...}}'   # optional, see docs

# Optional persistent storage (skipped if either is unset):
export SUBMUX_DATABASE_URL="postgres://localhost/submux"
export SUBMUX_SEALER_KEY="$(openssl rand -hex 32)"

cargo run --release
```

Default bind is `127.0.0.1:8080`; override via `SUBMUX_BIND`.

## Project layout

```
src/
├── core/         Pure types and traits (no I/O)
├── accounts/     Account aggregate, pool, refresh singleflight, cookie jar
├── providers/    Outbound adapters: anthropic OAuth + openai (Codex) session
├── protocols/    Inbound parsers and outbound emitters (anthropic, openai)
├── router/       Strategy trait, cooldown cache, retry policy, failover
├── streaming/    SSE parser/emitter, anthropic↔openai translator, checkpointing
├── coordination/ Cross-replica trait, in-memory default, redis placeholder
├── storage/      Postgres account store + XChaCha20Poly1305 sealing
├── telemetry/    Typed event bus, hand-rolled Prometheus registry, tracer ids
├── server/       axum app, middleware, route handlers, shared response helpers
├── constants/    Header names, upstream paths, user agents, limits
├── config/       Config loading (env-driven today)
├── lib.rs        Module roots + re-exports
└── main.rs       Binary entry: tracing init, env-seed accounts, axum::serve
```

## Documentation

- [`CLAUDE.md`](./CLAUDE.md) — operational rules and module map for contributors and code-aware agents.
- [`docs/architecture.md`](./docs/architecture.md) — module layout, request paths, state, adapters, refresh-on-401, coordination, storage, telemetry, middleware.
- [`docs/naming.md`](./docs/naming.md) — naming conventions (modules, types, functions, constants).
- [`docs/commenting.md`](./docs/commenting.md) — rustdoc + label policy.
- [`docs/types.md`](./docs/types.md) — type-safety rules (no `unwrap()` outside tests, newtypes, async ownership).
- [`docs/workflow.md`](./docs/workflow.md) — Conventional Commits, branch naming, PR template, verify chain.

## Build & test

```bash
cargo fmt --check
cargo clippy --lib --tests -- -D warnings
cargo test --lib
cargo build --release
```

All three checks must pass before merge.

## License

Apache-2.0.
