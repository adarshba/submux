# Type Safety

Detail behind the type rules in `CLAUDE.md`. Hard constraints first, then organization, then examples.

## Hard constraints

- **No `unwrap()` in production code.** Use `?`, `expect("static contract: <invariant>")`, or branch. `unwrap()` is allowed in `#[cfg(test)]` and in const-time builders that are infallible by construction (`HeaderValue::from_static`).
- **No bare `expect("...")` without an invariant.** `expect("static cookie name is a valid header")` is fine. `expect("should work")` is forbidden.
- **No `as` for narrowing casts.** `let n: u32 = x as u32;` where `x: u64` is forbidden. Use `u32::try_from(x)?` and branch on overflow. `as` is allowed for widening (`u32 as u64`) and for `f32`/`f64` interop.
- **No `String` where a domain newtype fits.** `AccountId(Ulid)`, `Bearer(String)`, `DeviceId(String)`. The compiler stops you from passing a refresh token where an access token is wanted.
- **No `Box<dyn Error>` in public APIs.** Use a typed enum with `thiserror`. `eyre::Report` is allowed only in `main.rs` and bootstrap code.
- **No `Vec<u8>` for "some bytes" at module boundaries.** Use `Bytes` from the `bytes` crate; it's cheap to clone and integrates with axum / reqwest streaming.
- **No `Arc<Mutex<HashMap<K, V>>>`.** Use `DashMap<K, V>` for concurrent map state. A `Mutex<HashMap>` is a serialization point; `DashMap` is sharded.

If a third-party crate forces an unsafe boundary, isolate it in one function, document the invariant with `SAFETY:`, and treat the wrapper as the safe surface for everyone else.

## Type organization

Domain types live in `src/core/`:

- `core/account.rs` — `AccountHandle`, `AccountId`, `AccountInner`, `ProviderKind`.
- `core/api_key.rs` — `ApiKey` newtype (constant-time verify, redacted display).
- `core/session.rs` — `Session`, `Credentials`, `FingerprintProfile`, `SerializedCookieJar`.
- `core/request.rs` / `response.rs` / `stream.rs` — `NormalizedRequest`, `NormalizedResponse`, `ResponseStream`.
- `core/error.rs` — `SubmuxError`, `AdapterError`, `TransientKind`, `ChallengeKind`, `RlScope`.
- `core/protocol.rs` / `message.rs` — wire-format enums and message shapes.

Per-module types live in their own file under the module that owns them (`config/settings.rs`, `accounts/cooldown.rs`, `providers/anthropic/proxy.rs`). Never declare a non-trivial `struct` or `enum` inside a function body — extract to a sibling file and re-export.

Bad: redeclaring `PassthroughResponse` in two adapter files, or defining a streaming state machine `struct State { … }` inline in a route handler.

Good: lift the shared shape into a dedicated file (`providers/anthropic/proxy.rs::PassthroughResponse`, `streaming/relay.rs::SseTranslator`).

## Newtypes vs aliases

Use a newtype (`struct AccountId(Ulid)`) when:

- The value crosses module boundaries.
- The value carries an invariant (non-empty, range, format).
- Passing the wrong primitive of the same shape would be a bug.

Use an alias (`type ResponseStream = BoxStream<'static, Result<Bytes, AdapterError>>;`) when:

- The thing is a complex generic the call site shouldn't have to spell.
- There's no invariant to enforce, just typing ergonomics.

## Errors

Every error enum:

- Ends in `Error`.
- Derives `thiserror::Error` and `Debug`.
- Splits transient vs permanent in the variant, not in two separate types.
- Names a `cause` field for the underlying source where applicable.

Good:

```rust
#[derive(thiserror::Error, Debug)]
pub enum AdapterError {
    #[error("transient upstream failure: {cause:?}")]
    Transient { cause: TransientKind },
    #[error("permanent upstream failure: {status} {body}")]
    Permanent { status: StatusCode, body: String },
    #[error("internal adapter error: {0}")]
    Internal(String),
}
```

Convert to HTTP status in `server/responses.rs`, never inline in a route.

## Async ownership

- `Arc<T>` — shared immutable. The default for `AppState` fields.
- `Arc<RwLock<T>>` — shared mutable across tasks where reads dominate.
- `tokio::sync::RwLock` — when the guard is held across `.await`.
- `parking_lot::RwLock` — when the guard is short and never held across `.await`.
- `ArcSwapOption<T>` — when you swap a whole value atomically, never mutate in place (e.g., `cooldown_until: ArcSwapOption<DateTime<Utc>>`).
- `DashMap<K, V>` — concurrent map. Default for per-account state collections.
- `OnceCell<T>` — process-wide install hooks (`install_openai_adapter`). Not for business state.

Lifetimes of long-lived state belong on `AccountState` (atomics + `ArcSwapOption`) or on `AppState` (Arc-shared singletons). Per-request state lives on the stack of the handler.

## Generic bounds

Be explicit; lean on `where` clauses for readability when bounds get long.

```rust
pub async fn refresh<F, Fut>(&self, id: AccountId, op: F) -> Result<RefreshOutcome, RefreshError>
where
    F: FnOnce() -> Fut + Send + 'static,
    Fut: Future<Output = Result<RefreshOutcome, RefreshError>> + Send + 'static,
{ ... }
```

Avoid `impl Trait` in public function signatures where naming the type aids documentation.

## Serialization shapes

Public response/event types derive `Serialize + Deserialize`. They live next to the route or module that owns them, in a `types` submodule when the file would otherwise grow past ~300 LOC.

Use `#[serde(rename_all = "snake_case")]` on enums whose variants are emitted into JSON (matches our metric label style).

## Casting policy summary

| Pattern | Verdict | Alternative |
|---|---|---|
| `x as u32` (narrowing) | ❌ | `u32::try_from(x)?` |
| `x as u64` (widening) | ✅ | — |
| `x as f64` | ✅ for telemetry | — |
| `&str as *const u8` | ❌ outside FFI | `.as_ptr()` |
| `Box<T>` ↔ `Arc<T>` via raw ptr | ❌ | `Arc::new(*boxed)` |
| `transmute` | ❌ unless documented `SAFETY:` block | A typed conversion |
