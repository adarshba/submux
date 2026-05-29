# CLAUDE.md

Operational rules for Claude Code in this repo. Deep reference lives in `docs/`.

## Project

Submux — subscription-native LLM gateway. Proxies Claude Max (Anthropic OAuth) and Codex (OpenAI ChatGPT Plus/Pro subscription via `chatgpt.com/backend-api/codex/...`). Not an API-key gateway. Single binary, single crate, Rust 2021, axum 0.7 + tokio 1.42 + reqwest 0.12.

## Commands

```bash
cargo build                # debug build
cargo build --release      # production binary at target/release/submux
cargo check --workspace    # type-check, no codegen
cargo test --lib           # unit tests (no integration suite today)
cargo fmt --all            # rustfmt
cargo clippy --all-targets -- -D warnings   # lint (must pass)
cargo run                  # debug run, reads SUBMUX_* env vars
```

`cargo verify` is not a built-in alias — use the chain `cargo fmt --all -- --check && cargo clippy --all-targets -- -D warnings && cargo test --lib` before declaring a task complete.

## Environment

Read at startup in `src/main.rs`. Missing values are warned, not errors, so the gateway can boot half-configured for local dev.

- `SUBMUX_CONFIG` — override the config file path. Default: `$XDG_CONFIG_HOME/submux/config.toml`.
- `SUBMUX_BIND` — listen address, default `127.0.0.1:8080`.
- `SUBMUX_API_KEY` — inbound shared secret. When set, every proxy route requires `Authorization: Bearer <key>` or `x-api-key: <key>`.
- `SUBMUX_ANTHROPIC_UPSTREAM` — default `https://api.anthropic.com`.
- `SUBMUX_ANTHROPIC_OAUTH_TOKEN` — Anthropic OAuth access token. Overrides auto-discovery from the macOS keychain / `~/.claude/.credentials.json`.
- `SUBMUX_ANTHROPIC_OAUTH_REFRESH_TOKEN` — optional; enables refresh-on-401.
- `SUBMUX_OPENAI_UPSTREAM` — default `https://chatgpt.com`.
- `SUBMUX_OPENAI_ACCESS_TOKEN` — Codex bearer. Overrides auto-discovery from `~/.codex/auth.json`.
- `SUBMUX_OPENAI_COOKIES` — JSON matching `SerializedCookieJar`. Optional; empty jar is allowed.
- `SUBMUX_OPENAI_DEVICE_ID` — UUID; auto-generated and persisted to the config file if missing.

Credential resolution per provider: **env > config file > auto-discovery > none**. Auto-discovery only fires when both env and file are unset; explicit values always win.

Never commit a real token or `SUBMUX_API_KEY` value. Use `.env` locally; the repo ships only `.env.example`.

## Module map

Defined in `Cargo.toml` as a single library + binary. Module roots are the directories under `src/`:

- `core/` — pure types: `AccountId`, `ApiKey`, `ProviderKind`, `Credentials`, `Session`, `AdapterError`, `ResponseStream`.
- `accounts/` — `Account`, `AccountPool`, `RefreshManager` (singleflight), `CooldownCache`, cookie jar, fingerprint state, seeding.
- `providers/` — outbound proxies. `anthropic/` (`AnthropicProxy` + body cloak) and `codex/` (`CodexProxy` + ChatGPT session + cookie auth).
- `protocols/` — wire-format parsers/emitters, OpenAI↔Anthropic translation, and chunk-to-completion buffering.
- `streaming/` — SSE parser, emitter, translators, and the shared relay state machine (`relay::drive`).
- `telemetry/` — metrics registry, Prometheus exporter, tracer ids.
- `server/` — axum app, middleware (request id, auth, panic catch), routes (`/v1/messages`, `/v1/chat/completions`, `/codex/responses`, `/codex/v1/messages`, `/health`, `/ready`, `/metrics`), banner, shutdown.
- `config/` — TOML config file + env resolver → `Settings`.
- `constants/` — cross-module shared constants by topic.
- `cli.rs` — clap flags (`--config`, `-q`, `-v`). No subcommands.

## Forbidden patterns

Hard constraints. Treat each as a compile error.

- Never use `unwrap()` on a `Result` outside of `#[cfg(test)]` or `*::default()`-style infallible builders. Use `?`, `expect("static contract: …")`, or branch on the error.
- Never use `expect(…)` with a message that doesn't name the invariant it relies on.
- Never use `as` for narrowing casts (`u64 as u32`, `usize as u32`). Use `TryFrom` / `try_into()` and branch on overflow.
- Never `clone()` to silence a borrow-checker error — fix lifetimes first. `Arc::clone(&x)` and `Bytes::clone()` are cheap and allowed by design.
- Never call `block_on` inside async code.
- Never read environment variables outside `src/main.rs` or a `*::from_env` constructor. Everything else takes config in by argument.
- Never spawn a tokio task that doesn't propagate or log its error. No fire-and-forget `tokio::spawn(async move { … });` with an unwrapped `Result`.
- Never declare HTTP header names, upstream paths, or upstream hosts inline. They belong in `src/constants/*.rs` (`http_headers.rs`, `upstream_paths.rs`, `user_agents.rs`).
- Never declare a struct's full fingerprint / cloak payload inline at a call site. Build it through `providers::*::headers` or a constants module.
- Never use vague filenames: `utils.rs`, `helpers.rs`, `common.rs`, `misc.rs`, `manager.rs`. Name by responsibility.
- Never log a token, cookie, refresh token, or API key — even at `tracing::debug!`. Use `ApiKey::redacted()` (or the same pattern) when the value must appear in a log line.
- Never commit `.env`, `target/`, `Cargo.lock` for libraries (we're a binary, lockfile stays).

## Naming rules

Detail in `docs/naming.md`. Quick reference:

- **Modules / files**: `snake_case.rs`, name by responsibility (`refresh.rs`, `cooldown.rs`, `discovery.rs`).
- **Types & traits**: `PascalCase` (`AccountPool`, `AnthropicProxy`, `CooldownCache`).
- **Functions & methods**: verb-first `snake_case` (`exchange_refresh_token`, `parse_quota_headers`, `cloak_bytes`).
- **Constants & statics**: `SCREAMING_SNAKE_CASE` (`HOP_BY_HOP_HEADERS`, `CODEX_CLI_USER_AGENT`).
- **Type parameters**: single capital letter or short PascalCase noun (`T`, `S`, `Stream`, `Backend`).
- **Errors**: every error enum ends in `Error` (`AdapterError`, `SealError`); transient/permanent split via inner enum, not type.
- **Domain noun**: `account` is canonical for "a single upstream identity"; never mix `user` / `session` / `tenant` in new code (a `Session` is *part of* an account, not a synonym).
- **`*_test.rs` files are forbidden** — Rust uses inline `#[cfg(test)] mod tests` per file.

## Project structure (modern Rust 2018+ layout)

- **No `mod.rs` files in new code.** Use `foo.rs` next to a `foo/` directory; `foo.rs` declares submodules. Only keep `mod.rs` if the file already exists for an unrelated reason.
- **Organize by domain, not by technical layer.** `accounts/`, `providers/anthropic/`, `streaming/` — not `controllers/`, `services/`, `repositories/`.
- **Method-first over free functions.** Operations hang off a type (`Settings::load`, `RefreshManager::refresh`). Free functions only for genuinely stateless helpers (a hex encoder, a deterministic byte transform).
- **One concept per file.** A type definition lives in a file named after it. No inline-defined `struct`/`enum` inside function bodies — extract to a sibling file and re-export.
- **Re-export from module roots.** Consumers write `use crate::accounts::AccountPool`, not `use crate::accounts::pool::AccountPool`.
- **Minimize `pub`.** Default to private; promote to `pub(crate)` or `pub` only when an external caller needs it.
- **Avoid glob imports.** `use crate::accounts::{Account, AccountPool}`, never `use crate::accounts::*`.
- **`main.rs` stays thin.** Parse args → resolve config → build app → serve. Anything reusable lives in the library.
- **Target file size**: ≤300 lines ideal, ≤500 lines acceptable. Past 500, split.
- **No narrowing `as` casts anywhere.** Use `TryFrom`/`try_into` and branch on overflow. Lossless widening uses `From`/`Into` (`f64::from(u32)`, `char::from(u8)`).

## Comments

Detail in `docs/commenting.md`. Default to no comments. Allowed labels: `TODO`, `FIXME`, `HACK`, `BUG`, `NOTE`, `SAFETY`. One line. Explain _why_, never _what_. No commented-out code — use git.

`///` rustdoc is mandatory on every `pub` item exported from a module root (the `foo.rs` re-exports next to `foo/`). Internal `pub(crate)` items get rustdoc only when the contract isn't obvious from the signature.

`//!` module-level rustdoc is mandatory on every file that defines a non-trivial concept (a state machine, a protocol, an external integration). Skip for re-export shims.

## Type-safety rules

Detail in `docs/types.md`. Hard rules:

- Newtypes over primitives at module boundaries. `AccountId(Ulid)`, not `String`. `Bearer(String)`, not `String`.
- No `String` for header names — `http::HeaderName::from_static(...)`.
- Errors are `enum`, derive `thiserror::Error`. Use `eyre::Result` only in `main.rs` and integration glue.
- `Arc<T>` is shared ownership across tasks. `Arc<RwLock<T>>` is shared mutable state across tasks. Never `Mutex<HashMap<K, V>>` when `DashMap` fits.
- Use `tokio::sync::RwLock` for state held across `.await`. Use `parking_lot::RwLock` for non-async critical sections.
- Public response/event types are `Serialize + Deserialize` and live next to the route or module that owns them, in a `types` submodule.

## Architecture rules

Detail in `docs/architecture.md`. Quick reference:

- **Request path**: inbound axum handler → `AppState` → provider adapter `passthrough(...)` → `reqwest` → upstream. No layer skips the adapter.
- **Translation lives in `protocols/`**, never in route handlers. A handler may call a translator but does not inline header/body massaging.
- **Adapter cloaking lives in `providers/*/headers.rs` and `providers/*/cloak.rs`**. Routes never set `Authorization`, `User-Agent`, or fingerprint headers directly.
- **Constants** referenced by more than one module live in `src/constants/<topic>.rs`. Single-module values stay local as `const`.
- **State**: `AppState` holds `Arc`-shared singletons. Per-request mutable state is owned by the handler frame. Long-lived per-account state is on `AccountState` (atomics + `ArcSwapOption`).
- **Failure mode**: every adapter error maps to one of `AdapterError::{Transient, Permanent, Internal}`. Routes translate to HTTP status via `server/responses.rs`, never inline.
- **No singletons in feature code.** `once_cell::sync::OnceCell` is allowed for process-wide install hooks (`providers::codex::install`) but not for business state — business state goes through `AppState`.

## Outstanding work

Search `TODO` / `FIXME` in the codebase for current items. Known longer-lived ones:

- `providers/codex/session.rs` — sentinel handshake (`/backend-api/sentinel/chat-requirements`) is deferred. Empty `openai-sentinel-*` headers work for the common request classes; some inputs will hit a 4xx until the handshake contract is reverse-engineered. The upstream Set-Cookie response header is intentionally dropped — submux is a stateless proxy and auto-discovered credentials carry no session cookies.
- `protocols/anthropic/translate_to_responses.rs` — Anthropic `image` / `document` / `thinking` content blocks are dropped during translation to Codex Responses. Text + tool_use translate cleanly today.

## Workflow

Follow `docs/workflow.md` for issue → branch → commit → PR flow. Commits use Conventional Commits (`feat:`, `fix:`, `chore:`, `refactor:`, `docs:`, `perf:`). PR bodies must include `Closes #<issue>`.

- Before editing a file, re-read it. Tool results can be stale.
- After every structural rename, search for: direct refs, type refs, string literals, dynamic imports, feature flags, tests/mocks. Grep is not semantic.
- Run `cargo fmt --all -- --check && cargo clippy --all-targets -- -D warnings && cargo test --lib` before reporting any task complete. If it fails, the task is not done.
- For 3+ step or architectural tasks, plan first. For multi-file refactors, work in phases of ≤5 files and verify between phases.
- Fix root causes. Never bypass with `cargo --no-fail-fast`, `#[allow(...)]` blankets, `unwrap()`, or feature-flagged skips.

## Deep reference

- [`docs/architecture.md`](docs/architecture.md) — module layout, request path, state, adapters.
- [`docs/naming.md`](docs/naming.md) — full Rust naming convention spec.
- [`docs/types.md`](docs/types.md) — newtypes, errors, async ownership.
- [`docs/commenting.md`](docs/commenting.md) — comment + rustdoc policy with examples.
- [`docs/workflow.md`](docs/workflow.md) — issue, branch, commit, and PR conventions.
