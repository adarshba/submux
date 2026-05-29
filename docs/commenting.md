# Commenting Policy

Detail behind the comment rules in `CLAUDE.md`.

## Default: no comments

Well-named code doesn't need narration. Prefer:

- Clear naming.
- Small functions with a single responsibility.
- Focused abstractions.

A comment is only justified when removing it would confuse a future reader who can already see the code.

## Forbidden

- Tutorial / step-by-step narration (`// 1. parse the body`, `// 2. send the request`).
- Redundant restatement of what the code does (`// increment the counter`).
- Historical storytelling (`// we used to do X, now we do Y`).
- Dead commented-out code — use git history.
- Block comments with `*` ASCII boxes.

Bad:

```rust
// Parse the headers
let headers = parse_headers(&body);
```

Bad:

```rust
// Old implementation
// let result = old_parser(data);
```

## Allowed labels

Use only these structured labels. One line. Explain _why_, not _what_.

### `TODO`

Actionable future work outside current scope.

```rust
// TODO: switch to streaming refresh once Anthropic publishes the websocket spec.
```

### `FIXME`

Known broken or incorrect behavior currently tolerated. Stronger than TODO.

```rust
// FIXME: sentinel handshake unimplemented; some Codex request classes will 4xx.
```

### `HACK`

Temporary workaround. Explain why it exists.

```rust
// HACK: reqwest::Client doesn't expose its connection pool, so we leak one per upstream.
```

### `BUG`

Known unexpected runtime behavior we have not yet root-caused.

```rust
// BUG: Anthropic occasionally returns 200 with an SSE error frame; status alone isn't enough.
```

### `NOTE`

Context for counterintuitive logic. Use sparingly — only when code alone can't communicate intent.

```rust
// NOTE: order matters — apply_quota must run before forward_response so headers aren't moved.
```

### `SAFETY`

Required next to any `unsafe { … }` block. Names the invariant the caller is upholding.

```rust
// SAFETY: ptr was produced by Box::into_raw immediately above and is non-null.
```

## Rustdoc (`///` and `//!`)

Mandatory for:

- Every `pub` item exported from a module root (the `foo.rs` re-exports next to `foo/`).
- Every `pub trait` and its methods.
- Every error variant (`#[error("…")]` counts as documentation; expand it when the message alone is ambiguous).
- Every module file (`//!`) that defines a non-trivial concept — a state machine, an external protocol, a security boundary.

Allowed without rustdoc:

- `pub(crate)` items whose signature is self-explanatory.
- Trivial getters / `From` / `Default` impls.
- Re-export shims.

Good rustdoc explains intent, contract, and side effects — not types.

```rust
/// Byte-for-byte passthrough to `POST {upstream}/v1/messages`.
///
/// Strips client auth, injects the account OAuth bearer + `anthropic-beta: oauth-2025-04-20`,
/// and streams the response body without buffering. Non-2xx responses are still returned as a
/// stream so callers can decide whether to surface or branch on the status.
pub async fn passthrough(&self, ...) -> Result<PassthroughResponse, AdapterError>
```

Bad rustdoc restates types:

```rust
/// Passes the headers and body to the upstream.
/// Returns a PassthroughResponse or an AdapterError.
pub async fn passthrough(...)
```

## Length

One-line comments preferred. Rustdoc may be a paragraph when the contract genuinely requires it, but most pub items need 1–3 lines.

If a non-rustdoc comment grows past one line, the code probably needs a named helper instead.

Good:

```rust
// HACK: reqwest folds Set-Cookie into a single value; we re-split on commas before parsing.
```

Bad:

```rust
// This is here because reqwest's HTTP/2 implementation merges multiple Set-Cookie
// response headers into a single comma-separated value, which is wrong per RFC 6265
// but they refuse to fix it, so we split it back out manually.
```

(If that paragraph is genuinely necessary, lift it into a `//!` at the top of `cookies.rs` and link the upstream issue.)
