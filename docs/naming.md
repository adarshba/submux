# Naming Conventions

Detail behind the quick-reference rules in `CLAUDE.md`. Read the principles, then the per-kind rules.

## Principles

- Prioritize clarity over brevity.
- Use one canonical domain term — don't mix `account` / `user` / `session` / `tenant`. A `Session` is part of an `Account`, not a synonym.
- Prefer semantic naming over mechanical naming. `RoundRobin` describes a policy; `Strategy1` describes nothing.
- Avoid abbreviations unless industry-standard (`http`, `id`, `url`, `oauth`).
- Never use vague filenames: `utils.rs`, `helpers.rs`, `common.rs`, `misc.rs`, `manager.rs`.

## Files & directories

### Modules — `snake_case.rs`

Name by responsibility, not by what's inside.

Good: `refresh.rs` (manages refresh-on-401), `cooldown.rs` (per-account cooldown cache), `discovery.rs` (read official CLI credential stores).

Bad: `utils.rs`, `account_helpers.rs`, `misc.rs`, `things.rs`.

### Directories — `snake_case/` paired with a sibling `snake_case.rs`

Modern Rust 2018+ layout: `foo.rs` next to a `foo/` directory; `foo.rs` declares the submodules. No `mod.rs` files. Group by concern, not by layer: `providers/anthropic/` not `adapters/oauth/`. The domain noun comes first.

### Test files — none

Rust convention is `#[cfg(test)] mod tests { … }` at the bottom of the file under test. No `*_test.rs` files; no `tests/` directory unless an integration suite is added later.

## Symbols

### Functions & methods — verb-first `snake_case`

`exchange_refresh_token`, `parse_quota_headers`, `cloak_bytes`, `apply_quota`, `record_request`, `forward_response`.

Predicates start with `is_` / `has_` / `should_`: `is_streaming`, `has_refresh_token`.

Constructors start with `new` / `from_*` / `try_from_*`. Async constructors that perform I/O are named `connect`, `open`, `bind` — not `new_async`.

Bad: `do_stuff`, `process`, `handle` (without an object), `helper`.

### Variables — descriptive `snake_case`

Good: `passthrough`, `cooldown_until`, `account_id`, `model_group`, `outgoing_body`.

Bad: `res`, `tmp`, `obj`, `data` (when something more specific applies), `x` (outside a tight closure).

### Types & traits — `PascalCase`

`AccountPool`, `RefreshManager`, `CooldownCache`, `AnthropicProxy`, `CodexProxy`, `ResponseStream`, `AdapterError`, `ApiKey`.

Trait names describe a capability: `SseTranslator`, `ChunkEncoder`. Avoid `-er` suffixes when a noun fits.

### Errors — `*Error`

Every error enum ends in `Error`: `AdapterError`, `SealError`, `RouteError`. Variants are kind-first: `AdapterError::Transient { cause }`, not `AdapterError::TransientError`.

### Constants & statics — `SCREAMING_SNAKE_CASE`

`HOP_BY_HOP_HEADERS`, `CODEX_CLI_USER_AGENT`, `DEFAULT_BODY_LIMIT_BYTES`, `COOLDOWN_UTIL_THRESHOLD`.

Time / size constants carry units in the name: `_MS`, `_SECS`, `_BYTES`, `_KIB`, `_MIB`.

### Type parameters — single capital or short PascalCase

`T`, `S`, `E` for fully generic; `Stream`, `Backend`, `Fut` when the role is meaningful.

### Lifetimes — `'a`, `'b`, or `'static`

Named lifetimes only when the relationship needs a noun: `'req`, `'jar`. Avoid clever names; readability wins.

## Domain consistency

The canonical noun for an upstream identity is **`account`**. All new fields, functions, and variants use `account`.

- `Session` is the part of an `Account` that holds rotating credentials.
- `Provider` is the upstream service (`AnthropicSubscription`, `OpenAiSubscription`).
- `Adapter` is the code that talks to a provider.

External shapes (Anthropic's `messages`, OpenAI's `chat/completions`, Codex's `responses`) keep upstream wording — that's an outside contract. Translate at the protocol boundary.

## Refactor rules

When renaming:

1. Preserve business meaning.
2. Update `use` paths, `pub use` re-exports, type refs, string literals, doc links, and tests.
3. Detect duplicate semantic terms and pick one (`token` vs `bearer` vs `access_token` — `access_token` wins for the field, `Bearer` wins for the newtype).
4. For shared boundary contracts (HTTP header names, env var names, Postgres column names, metric labels), coordinate with consumers — don't break them silently.
5. Prefer consistency over personal preference.
