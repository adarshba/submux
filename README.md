# Submux

Subscription-native LLM gateway in Rust. Treats **accounts** — Claude Max sessions, ChatGPT Plus (Codex) sessions, future session-oriented providers — as the first-class primitive, not API keys.

Four inbound surfaces, all wired to a shared account pool, cooldown cache, and refresh-singleflight manager:

| Route | Upstream | Mode |
|---|---|---|
| `POST /v1/messages` | `api.anthropic.com/v1/messages` | Byte-for-byte passthrough with OAuth body cloak + Stainless headers + `anthropic-beta: oauth-2025-04-20`. Refresh-on-401. |
| `POST /v1/chat/completions` | `api.anthropic.com/v1/messages` | OpenAI Chat → Anthropic Messages translation in, Anthropic SSE → OpenAI chunks out. Streaming and non-streaming. |
| `POST /codex/responses` | `chatgpt.com/backend-api/codex/responses` | Codex CLI passthrough with cookie jar + device id + `codex-cli/<v>` fingerprint. |
| `POST /codex/v1/messages` | `chatgpt.com/backend-api/codex/responses` | Anthropic Messages → Codex Responses. Lets Claude Code (`ANTHROPIC_BASE_URL=http://submux/codex`) drive a ChatGPT Plus account. |

## Setup

### 1. Log in with the official CLIs (one-time)

Submux reads OAuth credentials from wherever the official CLIs already store them. If you've already used them, skip to step 2.

```bash
# Anthropic Claude Max — stores in macOS Keychain / ~/.claude/.credentials.json
claude login

# ChatGPT Codex — stores in ~/.codex/auth.json
codex login
```

You only need the providers you actually want submux to serve. Both are independent.

### 2. Install and run

```bash
cargo install submux
submux
```

> Requires Rust. Install it from [rustup.rs](https://rustup.rs) if you don't have it.

That's it — the proxy is now running on `http://127.0.0.1:8080`. On first run submux:

1. Writes a config skeleton to `~/.config/submux/config.toml` (mode `0600`).
2. Auto-discovers your `claude` and `codex` credentials from the standard locations.
3. Prints a banner showing where each provider's tokens came from.

The startup banner looks like this:

```
submux 0.1.0
  listening:    http://127.0.0.1:8080
  config:       /Users/you/.config/submux/config.toml  (created on this run)
  api key:      ⚠ open access — anyone on 127.0.0.1:8080 can use this proxy
  anthropic:    ready (refresh: yes, source: claude keychain)
  codex:        ready (device: a149…, source: codex login)
```

If a provider says `not configured`, run the corresponding `claude login` / `codex login` and restart submux. Or override explicitly — see step 4.

### 3. Lock the proxy down (optional)

By default the proxy is open: anyone who can hit `127.0.0.1:8080` can use your subscription. To require a shared secret on every request, set `api_key` in the config file to any string you like (a long random one is recommended) and restart:

```toml
api_key = "smx_live_2c5fa1b3..."
```

On the next start, every `/v1/*` and `/codex/*` request must include the key:

```
Authorization: Bearer smx_live_...
# or
x-api-key: smx_live_...
```

Clients pointing at submux can use either form. Anthropic SDK and OpenAI SDK clients can both set their `*_API_KEY` to this value and submux will accept it. `/health`, `/ready`, and `/metrics` stay open regardless.

`SUBMUX_API_KEY=<key> submux` overrides whatever's in the file — useful for container deployments.

### 4. Point your client at it

| Use case | Environment variable to set |
|---|---|
| Claude Code — Claude Max | `ANTHROPIC_BASE_URL=http://127.0.0.1:8080` |
| Any OpenAI-compatible client | `base_url=http://127.0.0.1:8080` |
| Claude Code — ChatGPT Plus (Codex) | `ANTHROPIC_BASE_URL=http://127.0.0.1:8080/codex` |

If you locked the proxy down, also set the matching `ANTHROPIC_API_KEY` / `OPENAI_API_KEY` to your `smx_live_…` key.

### 5. Observability (optional)

Metrics are always exposed for scrape at `GET /metrics`. Set
`SUBMUX_OTLP_ENDPOINT=http://localhost:4318` to also push them to an
OpenTelemetry collector (e.g. the `lgtm-autostart` stack). Per-consumer usage is
attributed when clients send an `X-Proxy-User-Id: <whoami>_<uuid>` header. An
importable Grafana dashboard and wiring live in [`examples/`](./examples).

### CLI reference

```
submux                          start the proxy
submux --config <path>          use a config file outside the XDG default
submux -q | -v                  lower / raise log verbosity (warn / debug)
submux --help                   print help
submux --version                print version
```

To inspect or rotate config values, edit the TOML file directly and restart:

```bash
$EDITOR ~/.config/submux/config.toml   # macOS/Linux XDG default
```

## Project layout

```
src/
├── core/         Pure types (AccountId, ApiKey, Credentials, …) — no I/O
├── accounts/     Account aggregate, pool, refresh singleflight, cookies, cooldown
├── providers/    Outbound proxies: anthropic OAuth + codex (ChatGPT) session
├── protocols/    Anthropic ↔ OpenAI request/response translation
├── streaming/    SSE parser/emitter + shared relay state machine
├── telemetry/    OTel metrics (OTLP push + Prometheus /metrics), tracer ids
├── server/       axum app, middleware, route handlers, banner, shutdown
├── constants/    Header names, upstream paths, user agents, limits
├── config/       TOML config file + env resolver → Settings
├── cli.rs        clap flags only (--config, -q, -v)
├── lib.rs        Module roots + re-exports
└── main.rs       Binary entry: parse → resolve → seed → serve
```

## Documentation

- [`CLAUDE.md`](./CLAUDE.md) — operational rules and module map for contributors and code-aware agents.
- [`docs/architecture.md`](./docs/architecture.md) — module layout, request paths, state, proxies, translation, refresh-on-401, telemetry, middleware.
- [`docs/naming.md`](./docs/naming.md) — naming conventions (modules, types, functions, constants).
- [`docs/commenting.md`](./docs/commenting.md) — rustdoc + label policy.
- [`docs/types.md`](./docs/types.md) — type-safety rules (no `unwrap()` outside tests, newtypes, async ownership).
- [`docs/workflow.md`](./docs/workflow.md) — Conventional Commits, branch naming, PR template, verify chain.

## Build & test (contributors)

```bash
cargo fmt --check
cargo clippy --lib --tests -- -D warnings
cargo test --lib
cargo build --release
```

All three checks must pass before merge.

## License

Apache-2.0.
