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

### 1. Install

```bash
cargo install submux
```

> Requires Rust. Install it from [rustup.rs](https://rustup.rs) if you don't have it — one command, a few minutes.

### 2. Get your tokens

Submux uses your existing Claude Max and/or ChatGPT Plus subscriptions. You need to copy a token out of your browser. This takes about two minutes and requires no coding.

**What is the Network tab?** It's a panel built into every browser that shows you what requests your browser is making in the background. You don't need to understand it — you just need to find one value in it.

---

#### Anthropic (Claude Max) — for `/v1/messages` and `/v1/chat/completions`

1. Open **Chrome** or **Firefox** and go to [claude.ai](https://claude.ai). Log in if you aren't already.
2. Open Developer Tools:
   - **Mac**: press `Cmd + Option + I`
   - **Windows / Linux**: press `F12`
3. Click the **Network** tab along the top of the panel that opened.
4. Go back to the Claude chat and **send any message** — something short like "hi" is fine.
5. A list of requests will appear. Look for one whose name contains `api.anthropic.com`. Click it.
6. A side panel opens. Click the **Headers** tab inside it.
7. Scroll down to **Request Headers**. Find the line that starts with `authorization:`.
8. The value looks like `Bearer sk-ant-oat01-...`. Copy everything **after** `Bearer ` (not including the space). That is your `SUBMUX_ANTHROPIC_OAUTH_TOKEN`.
9. For the refresh token: look through the same request's headers and the **Response Headers** tab for any value starting with `sk-ant-ort01-...`. If you find one, that is your `SUBMUX_ANTHROPIC_OAUTH_REFRESH_TOKEN`. It is optional but lets submux renew your session automatically when it expires.

---

#### Codex (ChatGPT Plus) — for `/codex/responses` and `/codex/v1/messages`

1. Go to [chatgpt.com](https://chatgpt.com). Log in.
2. Open Developer Tools (`Cmd + Option + I` on Mac, `F12` on Windows/Linux).
3. Click the **Network** tab.
4. **Send any message** in ChatGPT.
5. In the Network panel, find a request whose URL contains `chatgpt.com/backend-api`. Click it.
6. Click the **Headers** tab in the side panel.
7. Under **Request Headers**, find `authorization:`. Copy everything after `Bearer `. That is your `SUBMUX_OPENAI_ACCESS_TOKEN`.

---

### 3. Export your tokens

Open a terminal and paste the values you copied. You only need the sections for the providers you use.

```bash
# Anthropic (Claude Max)
export SUBMUX_ANTHROPIC_OAUTH_TOKEN="sk-ant-oat01-..."
export SUBMUX_ANTHROPIC_OAUTH_REFRESH_TOKEN="sk-ant-ort01-..."   # optional but recommended

# Codex (ChatGPT Plus) — only needed if you want /codex/* routes
export SUBMUX_OPENAI_ACCESS_TOKEN="eyJhbGciOi..."
```

Alternatively, copy `.env.example` to `.env`, fill in your values, and load it:

```bash
cp .env.example .env
# open .env in any text editor and paste your tokens
source .env
```

### 4. Run

```bash
submux
```

Binds to `127.0.0.1:8080` by default. Override with `SUBMUX_BIND=0.0.0.0:9000 submux`.

### 5. Point your client at it

| Use case | Environment variable to set |
|---|---|
| Claude Code — Claude Max | `ANTHROPIC_BASE_URL=http://127.0.0.1:8080` |
| Any OpenAI-compatible client | `base_url=http://127.0.0.1:8080` |
| Claude Code — ChatGPT Plus (Codex) | `ANTHROPIC_BASE_URL=http://127.0.0.1:8080/codex` |

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
