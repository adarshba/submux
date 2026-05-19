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

- `SUBMUX_BIND` — listen address, default `127.0.0.1:8080`.
- `SUBMUX_ANTHROPIC_UPSTREAM` — default `https://api.anthropic.com`.
- `SUBMUX_ANTHROPIC_OAUTH_TOKEN` — seeds one Anthropic account at boot. Without it, `/v1/messages` returns 503.
- `SUBMUX_ANTHROPIC_OAUTH_REFRESH_TOKEN` — optional; enables refresh-on-401.
- `SUBMUX_OPENAI_UPSTREAM` — default `https://chatgpt.com`.
- `SUBMUX_OPENAI_ACCESS_TOKEN` — seeds one Codex subscription account.
- `SUBMUX_OPENAI_COOKIES` — JSON matching `SerializedCookieJar`. Optional; empty jar is allowed.
- `SUBMUX_OPENAI_DEVICE_ID` — UUID; auto-generated if missing.
- `SUBMUX_DATABASE_URL` — Postgres URL. If set with `SUBMUX_SEALER_KEY`, persisted accounts load at boot.
- `SUBMUX_SEALER_KEY` — 32-byte key as hex / base64 / SHA-256-of-passphrase. Required when persisting credentials.

Never commit a real token or `SUBMUX_SEALER_KEY` value. Use `.env` locally; the repo ships only `.env.example`.

## Module map

Defined in `Cargo.toml` as a single library + binary. Module roots are the directories under `src/`:

- `core/` — pure types: `AccountId`, `ProviderKind`, `Credentials`, `Session`, `AdapterError`, `ResponseStream`, request/response shapes.
- `accounts/` — `Account`, `AccountPool`, `RefreshManager` (singleflight), cookie jar, fingerprint state.
- `providers/` — outbound adapters. `anthropic/` (OAuth + body cloak) and `openai/` (Codex CLI fingerprint + cookie auth).
- `protocols/` — wire-format parsers/emitters and OpenAI↔Anthropic translation.
- `router/` — picks an account, applies cooldowns, retry policy, strategy traits.
- `streaming/` — SSE parser, emitter, translator state machines.
- `coordination/` — multi-replica coordination trait + `InMemory` default. `redis` feature is a placeholder.
- `storage/` — Postgres-backed account store, XChaCha20Poly1305 sealer.
- `telemetry/` — events bus, hand-rolled Prometheus exposition, tracer ids.
- `server/` — axum app, middleware (request id, panic catch), routes (`/v1/messages`, `/v1/chat/completions`, `/codex/responses`, `/admin/*`, `/healthz`, `/metrics`).
- `config/` — config-file plumbing (deferred — env is canonical today).
- `constants/` — cross-module shared constants by topic.

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
- Never log a token, cookie, or sealer key — even at `tracing::debug!`. Use the `Redacted` newtype if you must reference one.
- Never commit `.env`, `target/`, `Cargo.lock` for libraries (we're a binary, lockfile stays).

## Naming rules

Detail in `docs/naming.md`. Quick reference:

- **Modules / files**: `snake_case.rs`, name by responsibility (`refresh.rs`, `cooldown.rs`, `sealer.rs`).
- **Types & traits**: `PascalCase` (`AccountPool`, `ProviderAdapter`, `CooldownCache`).
- **Functions & methods**: verb-first `snake_case` (`exchange_refresh_token`, `parse_quota_headers`, `cloak_bytes`).
- **Constants & statics**: `SCREAMING_SNAKE_CASE` (`HOP_BY_HOP_HEADERS`, `CODEX_CLI_USER_AGENT`).
- **Type parameters**: single capital letter or short PascalCase noun (`T`, `S`, `Stream`, `Backend`).
- **Errors**: every error enum ends in `Error` (`AdapterError`, `SealError`); transient/permanent split via inner enum, not type.
- **Domain noun**: `account` is canonical for "a single upstream identity"; never mix `user` / `session` / `tenant` in new code (a `Session` is *part of* an account, not a synonym).
- **`*_test.rs` files are forbidden** — Rust uses inline `#[cfg(test)] mod tests` per file.

## Comments

Detail in `docs/commenting.md`. Default to no comments. Allowed labels: `TODO`, `FIXME`, `HACK`, `BUG`, `NOTE`, `SAFETY`. One line. Explain _why_, never _what_. No commented-out code — use git.

`///` rustdoc is mandatory on every `pub` item exported from a module root (the module's `mod.rs` re-exports). Internal `pub(crate)` items get rustdoc only when the contract isn't obvious from the signature.

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
- **No singletons in feature code.** `once_cell::sync::OnceCell` is allowed for process-wide install hooks (`providers::openai::install_openai_adapter`) but not for business state — business state goes through `AppState`.

## Outstanding work

Search `TODO` / `FIXME` in the codebase for current items. Known longer-lived ones:

- `providers/openai/chatgpt_session.rs` — sentinel handshake (`/backend-api/sentinel/chat-requirements`) is unimplemented; arkose token is empty. Some Codex request classes will be rejected.
- `providers/openai/chatgpt_session.rs` — upstream `Set-Cookie` headers are logged but not merged back into the account jar.
- `coordination/redis.rs` — feature flag is wired but the impl is a stub; multi-replica deployments don't actually coordinate yet.
- `core::provider::ProviderAdapter::execute` — normalized request path is unimplemented for both adapters. Routes use `passthrough()` directly.

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
