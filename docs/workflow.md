# Workflow

How work flows from idea → issue → branch → commit → PR → merge. Keep it boring and consistent so history stays grep-able.

## The flow

1. **Open an issue first.** Use a template from `.github/ISSUE_TEMPLATE/`. Every PR resolves a tracked issue. If the change is genuinely trivial (typo, comment, dep bump), skip the issue and say so in the PR body — but default to opening one.
2. **Branch off `main`.** Name it `<type>/<issue-number>-<short-slug>` (e.g. `feat/42-codex-route`, `fix/57-refresh-singleflight`). Type matches the commit prefix below.
3. **Commit in small, semantic units** following Conventional Commits (see below). One logical change per commit.
4. **Open a PR using the template.** Title mirrors the commit format. Body must include `Closes #<issue>`.
5. **Run the verify chain before pushing.** Type-check, lint, and tests must pass:
   ```bash
   cargo fmt --all -- --check && \
   cargo clippy --all-targets -- -D warnings && \
   cargo test --lib
   ```
6. **Squash-merge by default.** Final commit message on `main` keeps the Conventional Commit format.

## Commit messages

Format: `<type>(<scope>)?: <subject>`

```
feat(codex): add /codex/responses passthrough route
fix(refresh): coalesce concurrent refreshes with singleflight
chore: drop unused tower-http auth feature
refactor(messages): extract error_response helper to server::responses
docs(workflow): add Conventional Commits guide
perf(metrics): avoid string clone in record_request label path
```

### Types

| Type       | Use for                                                             |
| ---------- | ------------------------------------------------------------------- |
| `feat`     | New user-facing capability or new module.                           |
| `fix`      | Bug fix. The subject names the bug, not the fix.                    |
| `refactor` | Internal restructure with no behaviour change.                      |
| `perf`     | Performance improvement with no behaviour change.                   |
| `chore`    | Tooling, deps, config, file moves, cleanup.                         |
| `docs`     | Documentation only.                                                 |
| `style`    | Formatting / whitespace. Almost never needed — `rustfmt` handles it.|
| `test`     | Test-only changes.                                                  |
| `revert`   | Reverts a prior commit. Body must reference the SHA being reverted. |

### Scope

Optional, lowercase, one word. Usually the module touched (`anthropic`, `codex`, `router`, `storage`, `telemetry`). Skip if the change is repo-wide.

### Subject

- Imperative mood: "add", not "added" / "adds".
- Lowercase, no trailing period.
- ≤ 72 characters.
- Describe the _change_, not the file. `fix(refresh): coalesce concurrent callers` not `fix: update refresh.rs`.

### Body (optional)

Wrap at 100 chars. Explain _why_ if non-obvious, list constraints, link related issues. Skip if the subject is self-evident.

### Footer

- `Closes #<n>` — for the issue this commit resolves on merge.
- `Refs #<n>` — for issues this touches but doesn't close.
- `BREAKING CHANGE:` — followed by a description, when env vars, HTTP shapes, or DB schema change.

## Branch naming

`<type>/<issue>-<slug>` — type matches commit type, slug is 2–4 kebab-case words.

```
feat/42-codex-route
fix/57-refresh-singleflight
chore/61-bump-axum-0.7
refactor/64-extract-server-responses
```

Never branch off another feature branch unless you explicitly need to stack PRs.

## Pull requests

- **Title** uses the Conventional Commit format. On squash-merge this becomes the commit on `main`, so write it like a commit.
- **Body** follows `.github/pull_request_template.md`. Mandatory fields: Summary, Linked issue (`Closes #N`), Test plan.
- **Size**: aim for ≤ 400 LOC diff. Split larger work into stacked PRs.
- **Self-review** before requesting review. Read your own diff in the GitHub UI; you will spot something.
- **Don't merge with red CI.** Don't bypass hooks (`--no-verify`) to push.

## Issues

Two templates under `.github/ISSUE_TEMPLATE/`:

- **`feature.md`** — user-visible capability. Captures motivation, acceptance criteria, out-of-scope.
- **`bug.md`** — broken behaviour. Captures steps to reproduce, expected vs actual, env vars in play.

Label issues at creation. Link related issues in the body, not just by reference in commits.

## Cargo workspace hygiene

- `Cargo.lock` is committed (this is a binary, not a library crate).
- Bumping a dep: separate `chore(deps):` commit, with the upstream changelog or release notes in the body if behaviour changed.
- Adding a dep: justify in the PR body. Prefer features over new crates when the workspace already pulls the dep.
- Feature flags: only add a flag when there's a real consumer with the off-path. `redis` is the live example — its placeholder doesn't earn the flag, but the eventual impl will.

## Quick checklist before opening a PR

- [ ] Issue exists and is linked via `Closes #N`.
- [ ] Branch name matches `<type>/<issue>-<slug>`.
- [ ] Every commit follows Conventional Commits.
- [ ] `cargo fmt --all -- --check` passes.
- [ ] `cargo clippy --all-targets -- -D warnings` passes.
- [ ] `cargo test --lib` passes.
- [ ] PR title is a valid Conventional Commit.
- [ ] PR body has Summary + Test plan.
