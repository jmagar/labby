---
date: 2026-04-26 23:34:28 EST
repo: git@github.com:jmagar/lab.git
branch: feat/oauth-email-allowlist (worktree)
head: merged to main at 2026-04-27T03:34:18Z (squash merge of PR #33)
agent: Claude (claude-sonnet-4-6 / claude-opus-4-7)
session id: 57f625d6-fcbd-4f64-a502-06e563a51d27
transcript: /home/jmagar/.claude/projects/-home-jmagar-workspace-lab/57f625d6-fcbd-4f64-a502-06e563a51d27.jsonl
working directory: /home/jmagar/workspace/lab
worktree: /home/jmagar/workspace/lab/.worktrees/oauth-email-allowlist
pr: "#33 — feat(lab-auth): Google OAuth email allowlist — https://github.com/jmagar/lab/pull/33 (MERGED)"
---

## User Request

Implement the bead `lab-dzhw` (Google OAuth email allowlist for lab-auth) using the full `/lavra-work` → `/simplify` → `/lavra-review` → `/gh-address-comments` workflow in an isolated git worktree, then extend the bootstrap model with a SQLite-backed multi-user allowlist managed via the Labby web UI, and create a PR.

---

## Session Overview

Full lifecycle from bead execution to merged PR: implemented Google OAuth email allowlist in `crates/lab-auth`, pivoted the design mid-session from a CSV allowlist env var to a fail-closed `LAB_AUTH_ADMIN_EMAIL` bootstrap model, planned and implemented a SQLite-backed multi-user allowlist with Axum REST API and gateway-admin UI panel, survived multiple CI failures, and merged PR #33 to main.

---

## Sequence of Events

1. Created git worktree at `.worktrees/oauth-email-allowlist` from `main` on branch `feat/oauth-email-allowlist`
2. Dispatched agent to execute bead `lab-dzhw` (Google OAuth Email Allowlist epic) — 7 commits, 76 lab-auth tests
3. Ran `/simplify` (3 parallel agents): found layering bug in `state.rs` (direct `std::env::var` read bypassing `AuthConfig::from_sources`) — fixed in 1 commit
4. Rebased branch onto `origin/main` (parent PR #31 had squash-merged); pushed; created PR #33
5. Ran `/lavra-review` (5 parallel agents: security-sentinel, architecture-strategist, rust-code-review, performance-oracle, code-simplicity-reviewer) — fixed error→warn log levels, incoming email trimming, `url::Url` vs `reqwest::Url`, `AuthError::Config` vs `Server`, dead test helper deletion
6. Ran `/gh-address-comments`: 3 reviewer threads addressed (warn levels, RFC 6749 error_description from AuthError, duplicate log removal)
7. **Design pivot**: user changed `LAB_AUTH_ALLOWED_EMAILS` (CSV, optional) → `LAB_AUTH_ADMIN_EMAIL` (single, required in oauth mode) for fail-closed default
8. Updated all docs: `docs/OAUTH.md`, `docs/ENV.md`, `docs/CONFIG.md`, `docs/OPERATIONS.md`, `docs/CHANGELOG.md`, `README.md`, `.env.example`
9. Added `LAB_AUTH_ADMIN_EMAIL=jmagar@gmail.com` to `~/.labby/.env`
10. Ran `/lavra-plan` to design multi-user allowlist epic `lab-1bri` (4 child beads, sequential chain)
11. Executed `lab-1bri` via `/lavra-work` across 4 waves:
    - Wave 1 (`lab-1bri.1`): `allowed_users` SQLite table + `list/add/remove_allowed_user` store CRUD
    - Wave 2 (`lab-1bri.2`): `AuthState::resolve_allowed_emails()` merging admin + db rows into both callback branches
    - Wave 3 (`lab-1bri.3`): REST API `GET/POST/DELETE /v1/auth/allowed-emails`, `require_admin` gate, `is_admin` on `/auth/session`
    - Wave 4 (`lab-1bri.4`): `AllowedUsersPanel` in gateway-admin settings, `auth-admin-client.ts`
12. Fixed CI failures: `AuthFileConfig` missing `admin_email` field broke config test; `cargo fmt` issues in agent-generated code
13. Watched CI via monitor, merged PR #33 on all-green

---

## Key Findings

- `crates/lab-auth/src/sqlite.rs:787-869` — schema uses inline `CREATE TABLE IF NOT EXISTS` batch in `open_connection()`; no migration framework. Extending it is append-only.
- `crates/lab/src/api/oauth.rs:9-16` — `AuthContext.email` is `Some` **only** on the browser-session path; JWT bearer sets it to `None`. Admin gate must be browser-session-only.
- `crates/lab/src/api/router.rs:172-328` — auth middleware populates `AuthContext` from three credential types: static bearer, JWT, browser session cookie.
- `crates/lab/src/config.rs:508` — `AuthFileConfig` (TOML deserialization) was missing the `admin_email` field, causing `config::tests::resolve_auth_reads_ttls_from_config_toml_fields` to fail in CI when our new `validate()` required the field.
- `check_email_allowlist` in `authorize.rs` already took `&[String]` — widening from `std::slice::from_ref(&admin_email)` to the merged Vec was a one-line change per call site.

---

## Technical Decisions

- **`LAB_AUTH_ADMIN_EMAIL` required (fail-closed)** — previous design allowed any Google account when allowlist was unset. User explicitly wanted fail-closed: startup aborts in oauth mode if the env var is missing.
- **Browser-session-only admin API gate** — `AuthContext.email` is `None` for JWT callers; rather than propagate email into JWT claims (a wider change), restricted the `/v1/auth/allowed-emails` endpoints to browser-session only with 403 for JWT callers.
- **`AuthError::Config` for unreachable redirect parse failure** — review caught `AuthError::Server` mapped to 502; the correct mapping for an internal invariant violation is `Config` → 500.
- **`allowed_emails_was_empty` dropped** — originally a transient parse signal was stored as a permanent field on `AuthConfig`; moved the startup warning directly into `AuthConfig::from_sources` where the raw value is in scope, and dropped the field.
- **`isAdmin?: boolean` (optional) in `BrowserSessionState` TypeScript type** — making it required would have broken ~10 existing test fixtures that use `__setBrowserSessionStateForTests` without the field. Optional with strict `=== true` guard keeps existing tests passing.
- **SQLite allowlist uses `AuthError::Validation` for duplicate inserts** (not a new `Conflict` variant) — `docs/ERRORS.md` doesn't list `conflict` as a stable kind; reusing Validation avoids spec drift.
- **`resolve_allowed_emails()` deduplicates** — when admin email also exists in the `allowed_users` table, it's only included once; `eq_ignore_ascii_case` used because admin_email is not guaranteed lowercase while DB rows are always lowercased.

---

## Files Modified

### crates/lab-auth
| File | Purpose |
|------|---------|
| `src/google.rs` | Added `email_verified: Option<bool>` to `GoogleIdTokenClaims` and `GoogleExchange` |
| `src/config.rs` | Added `admin_email: String` field (replaces `allowed_emails: Vec<String>`); validation requires it in oauth mode |
| `src/authorize.rs` | `check_email_allowlist` helper + `email_verified` enforcement; both callback call sites use `resolve_allowed_emails()` |
| `src/state.rs` | `resolve_allowed_emails()` async method merging admin + db allowlist |
| `src/sqlite.rs` | `allowed_users` table schema + `list/add/remove_allowed_user` CRUD |
| `src/types.rs` | `AllowedUserRow { email, added_by, created_at }` |
| `src/lib.rs` | Made `pub mod util` public for API crate access |
| `src/util.rs` | Made `fingerprint` and `now_unix` pub |

### crates/lab
| File | Purpose |
|------|---------|
| `src/api/services/auth_admin.rs` | New: REST handlers for `/v1/auth/allowed-emails` + `require_admin` helper |
| `src/api/services.rs` | Added `pub mod auth_admin` |
| `src/api/router.rs` | Nested auth admin routes in always-on `/v1` group |
| `src/api/browser_session.rs` | Extended `/auth/session` response with `is_admin: bool` |
| `src/config.rs` | Added `admin_email` field to `AuthFileConfig` + `resolve_auth()` wiring |
| `tests/auth_admin_api.rs` | New: 21 integration tests (anon/JWT/non-admin/admin) × (list/add/delete) |

### apps/gateway-admin
| File | Purpose |
|------|---------|
| `lib/api/auth-admin-client.ts` | New REST client: `listAllowedEmails`, `addAllowedEmail`, `removeAllowedEmail` |
| `lib/api/auth-admin-client.test.ts` | 9 colocated tests |
| `components/allowed-users-panel.tsx` | New: table + add form + remove confirm dialog |
| `components/allowed-users-panel.test.tsx` | 4 static render tests |
| `app/(admin)/settings/page.tsx` | Mounts `AllowedUsersPanel` for admin sessions |
| `lib/auth/session-store.ts` | Added `is_admin: boolean` to `SessionPayload`; `isAdmin?: boolean` to `BrowserSessionState` |

### Docs / Config
| File | Purpose |
|------|---------|
| `docs/OAUTH.md` | `LAB_AUTH_ADMIN_EMAIL` in env table + startup failure conditions + callback flow step |
| `docs/ENV.md` | Added to oauth example block + rules |
| `docs/CONFIG.md` | Added row to auth env table |
| `docs/OPERATIONS.md` | Added fail-closed startup requirement |
| `docs/CHANGELOG.md` | Added entry + Breaking-auth Changed entry |
| `README.md` | Updated OAuth row + env reference table |
| `.env.example` | Replaced `LAB_AUTH_ALLOWED_EMAILS` with `LAB_AUTH_ADMIN_EMAIL` |
| `~/.labby/.env` | Added `LAB_AUTH_ADMIN_EMAIL=jmagar@gmail.com` |

---

## Commands Executed

```bash
# Worktree creation
git worktree add .worktrees/oauth-email-allowlist -b feat/oauth-email-allowlist

# Rebase onto main after parent PR #31 squash-merged
git rebase --onto origin/main 7475f13e feat/oauth-email-allowlist

# CI fix: format + test
cargo fmt --all
cargo test -p lab@0.11.1 config::tests::resolve_auth_reads_ttls_from_config_toml_fields

# Merge
gh pr merge 33 --squash --delete-branch
```

---

## Errors Encountered

| Error | Root Cause | Resolution |
|-------|-----------|------------|
| Rebase conflict on parent branch commits | Parent PR #31 was squash-merged to main; branch included pre-existing commits | Used `git rebase --onto origin/main 7475f13e` to replay only our 8 commits |
| CI Format failure (first run) | Wave 3 agent left `auth_admin.rs` with expanded function signature style | `cargo fmt --all` locally, committed fmt fixes |
| CI Test failure: `resolve_auth_reads_ttls_from_config_toml_fields` | `AuthFileConfig` TOML struct missing `admin_email` field; our new `validate()` required it in oauth mode | Added `admin_email: Option<String>` to struct, wired through `resolve_auth()`, added field to test fixture |
| CI Format failure (second run) | Agent-generated code in `authorize.rs` and `state.rs` had style drift | `cargo fmt -p lab-auth && cargo fmt -p lab`, committed |

---

## Behavior Changes (Before/After)

| Aspect | Before | After |
|--------|--------|-------|
| OAuth login gate | Any Google account permitted when no allowlist configured (fail-open) | Startup fails if `LAB_AUTH_ADMIN_EMAIL` unset in oauth mode (fail-closed) |
| Allowlist env var | `LAB_AUTH_ALLOWED_EMAILS` (CSV, optional) | `LAB_AUTH_ADMIN_EMAIL` (single email, required) |
| Additional user access | Not possible at runtime | Admin can grant/revoke via `POST/DELETE /v1/auth/allowed-emails` or web UI |
| `/auth/session` response | `{ sub, email, ... }` | `{ sub, email, is_admin: bool, ... }` |
| Unverified Google email | Not checked | Rejected even if address matches allowlist (`email_verified` claim enforced) |
| JWT bearer at admin API | N/A (didn't exist) | 403 (authenticated but not admin-session) |

---

## Verification Evidence

| Command | Expected | Actual | Status |
|---------|----------|--------|--------|
| `cargo test -p lab-auth --all-features` | 89 pass | 89 pass | ✅ |
| `cargo test -p lab --all-features` (integration) | New tests pass | 21 new pass | ✅ |
| `cd apps/gateway-admin && pnpm test` | 192 pass | 192 pass | ✅ |
| `cargo clippy --all-features --tests -- -D warnings` (our files only) | 0 errors | 0 errors | ✅ |
| CI: Check / Clippy / Cargo Deny / Format / Test | All pass | All pass | ✅ |
| PR #33 merged | Merged to main | Merged 2026-04-27T03:34:18Z | ✅ |

---

## Risks and Rollback

- **Breaking change**: `LAB_AUTH_ALLOWED_EMAILS` removed. Any operator running `LAB_AUTH_MODE=oauth` must add `LAB_AUTH_ADMIN_EMAIL` before `lab serve` will start. Documented in `docs/CHANGELOG.md` and `docs/ENV.md`.
- **Rollback**: Revert the squash merge commit on main. The removed env var logic can be restored from git history.
- **Session table**: `allowed_users` rows persist in `~/.labby/auth.db`. Removing a user from the UI revokes future logins but does not invalidate existing browser sessions (sessions expire naturally via TTL).

---

## Decisions Not Taken

- **`LAB_AUTH_ALLOWED_EMAILS` CSV allowlist** (original design) — replaced with single `LAB_AUTH_ADMIN_EMAIL` so the admin is forced to be explicit; additional users are managed via UI.
- **JWT email claim propagation** — would allow JWT callers to use the admin API, but required widening JWT claims. Deferred; browser-session-only gate is sufficient for the UI use case.
- **`HashSet<String>` for allowlist lookup** — O(n) linear scan is negligible at typical allowlist sizes (<50 emails) vs Google token round-trip (~100ms).
- **Bulk CSV import/export** — deferred to a future bead.
- **Domain-level allowlists** (`*@example.com`) — deferred.
- **Roles / multiple admin tiers** — deferred; single admin via env is sufficient.

---

## References

- Bead `lab-dzhw`: Google OAuth Email Allowlist (closed)
- Epic `lab-1bri`: Multi-user email allowlist SQLite + web UI (closed)
- PR #33: https://github.com/jmagar/lab/pull/33 (merged)
- RFC 6749 §4.1.2.1: OAuth error redirect requirement (implemented in oauth-client callback branch)
- `docs/OBSERVABILITY.md`: standard dispatch fields (surface/service/action/elapsed_ms)
- `docs/ERRORS.md`: stable error kind vocabulary

---

## Open Questions

- Should existing browser sessions be invalidated when an email is removed from the allowlist? Currently sessions live until their TTL expires (no forced logout).
- Should the admin be able to transfer admin status to another user, or is `LAB_AUTH_ADMIN_EMAIL` always the sole permanent admin?

---

## Next Steps

### Unfinished (started but incomplete)
- None — all bead work closed, PR merged.

### Follow-on (not yet started)
- **Activity log UI** (`lab-hjpg` epic): surfacing audit trail for allowlist mutations in the Labby web UI
- **Bulk email import/export** for the allowlist (CSV or paste-delimited)
- **Domain-level allowlists** (`*@example.com` patterns)
- **Forced session invalidation** when a user is removed from the allowlist
- **Gateway-admin command palette** (PR #37, separate branch `feat/gateway-admin-command-palette`) — unrelated in-progress work on main session branch
