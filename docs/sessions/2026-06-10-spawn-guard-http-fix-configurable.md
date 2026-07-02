---
date: 2026-06-10 18:49:07 EST
repo: git@github.com:jmagar/lab.git
branch: main
head: 57a15d5a
session id: 1d7ba140-4de0-44db-8724-f74c8ca1f0cb
transcript: /home/jmagar/.claude/projects/-home-jmagar-workspace-lab/1d7ba140-4de0-44db-8724-f74c8ca1f0cb.jsonl
working directory: /home/jmagar/workspace/lab
---

# Spawn guard: fix HTTP edit bug, make allowlist configurable

## User Request

Investigate why editing an HTTP MCP server in the Labby gateway UI was producing a
"stdio command doesn't match allowlist" error, fix the root cause, make the spawn guard
allowlist configurable via config, add a flag to disable it entirely, and eliminate the
duplicate allowlist-checking code in the marketplace dispatch.

## Session Overview

Two bugs were fixed and one long-standing structural problem (duplicated allowlist logic)
was resolved. The HTTP-edit-triggers-stdio-guard bug was caused by a serde deserialisation
flaw in `GatewayUpdatePatch` that prevented JSON `null` from actually clearing the
`command` field when switching a server to HTTP transport. A new `GatewayPreferences`
config struct was added to `LabConfig`, wiring `[gateway] extra_stdio_commands` and
`disable_spawn_guard` through the entire validation chain. The marketplace's local copy
of the allowlist check was replaced with a delegation to the shared `spawn_guard` module.
All docs and the example config were updated to reflect the new knobs, and the user's
live `~/.labby/config.toml` had the `[gateway]` section added with `disable_spawn_guard = true`.

## Sequence of Events

1. **Root cause investigation** — traced the "stdio command doesn't match allowlist" error
   on an HTTP upstream edit. Found that `validate_upstream` in `gateway/config.rs` branches
   on `(url, command)`: HTTP takes the `(Some(url), None)` arm (no stdio check), but `command`
   was never actually being set to `None` on the Rust side when the frontend sent `null`.
2. **Serde bug identified** — `GatewayUpdatePatch.url` and `.command` used plain
   `#[serde(default)]` on `Option<Option<String>>`, making JSON `null` deserialise as
   `None` (don't-touch) instead of `Some(None)` (clear). The `deserialize_nullable`
   helper already existed in the file for other fields but had not been applied to these two.
3. **Serde fix applied** — added `deserialize_with = "deserialize_nullable"` to `url` and
   `command` in `GatewayUpdatePatch` (`dispatch/gateway/params.rs:138-143`).
4. **GatewayPreferences struct added** — new `[gateway]` config section in `config.rs`
   with `extra_stdio_commands: Vec<String>` and `disable_spawn_guard: bool`.
5. **spawn_guard signatures updated** — `validate_stdio_command` and `validate_stdio_spec`
   in `spawn_guard.rs` gained `extra: &[String], bypass: bool` parameters. The bypass
   path returns `Ok(())` immediately; the extra list is checked alongside the built-in
   `ALLOWED_RUNTIME_HINTS`.
6. **GatewayPreferences threaded** — `validate_upstream`, `validate_upstreams`, and
   `validate_stdio_upstream` in `gateway/config.rs` all gained a `prefs: &GatewayPreferences`
   parameter. Call sites (`insert_upstream`, `update_upstream`, `validate_config`) load
   prefs from `crate::config::load()`.
7. **Marketplace deduplication** — `mcp_params::validate_runtime_hint` was a standalone
   copy of the allowlist check. Replaced with a delegation to
   `spawn_guard::validate_stdio_command`, re-wrapping the error kind to preserve the
   existing `unsupported_runtime_hint` shape.
8. **Test callsites bulk-fixed** — a `sed` pass updated single-line test calls; two
   multi-line calls at lines 1041 and 1054 in `mcp_dispatch.rs` and one at line 386 in
   `mcp_params.rs` were fixed manually.
9. **First build** — `cargo check --all-features` passed clean. `cargo nextest run
   --all-features` failed on the one remaining test callsite in `mcp_params.rs:386`.
10. **Final fix and green build** — fixed the missed callsite; 1837/1837 tests passed.
11. **Commits pushed** — two commits: `b9c05dad` (fix) and `57a15d5a` (docs).
12. **Docs and config updated** — `config/config.example.toml`, `docs/runtime/CONFIG.md`,
    `docs/services/GATEWAY.md`, and `~/.labby/config.toml` all updated with the new
    `[gateway]` section.

## Key Findings

- `dispatch/gateway/params.rs:138-143` — `url` and `command` in `GatewayUpdatePatch`
  both lacked `deserialize_with = "deserialize_nullable"`, causing JSON `null` to be
  silently ignored. The `deserialize_nullable` helper was already in the same file for
  other fields.
- `dispatch/gateway/config.rs` — `validate_upstream` only reaches stdio validation when
  `(url, command) = (None, Some(_))`. Because `command` never cleared, an HTTP server
  edit stayed in the `(None, Some(_))` branch and triggered the spawn guard.
- `dispatch/marketplace/mcp_params.rs:31` — `validate_runtime_hint` was an independent
  copy of the allowlist check, not delegating to `spawn_guard`. This was a silent
  maintenance risk: any change to the allowlist would need to be made in two places.
- `dispatch/upstream/spawn_guard.rs:19` — `ALLOWED_RUNTIME_HINTS` is the single source
  of truth for the built-in runtime list: `npx, uvx, docker, dnx, pipx, node, python,
  python3, deno`.
- The user's live config has four non-standard stdio binaries not in the built-in list:
  `synapse`, `ytdl-mcp`, `claude`, `axon`.

## Technical Decisions

- **`deserialize_nullable` over a custom type** — the helper was already in
  `params.rs` for `expose_tools`, `expose_resources`, etc. Applying it to `url` and
  `command` is the zero-new-code fix consistent with the file's existing pattern.
- **`extra: &[String], bypass: bool` parameters instead of a context struct** — keeps
  the function signatures simple. Both call sites (gateway and marketplace) pass concrete
  slices; no trait dispatch needed.
- **`disable_spawn_guard = true` in user's config** — the user explicitly requested a
  kill switch and expressed frustration at being blocked by a guard that shouldn't apply
  to their own binaries. The guard is still on by default; this is an operator opt-out.
- **Marketplace re-wrap preserves `unsupported_runtime_hint` kind** — the marketplace
  surface historically returned `sdk_kind: "unsupported_runtime_hint"` while spawn_guard
  returns `invalid_param`. The delegation wrapper translates the error kind so existing
  marketplace callers see no behaviour change.
- **`extra_stdio_commands` populated with actual user binaries** — `synapse`, `ytdl-mcp`,
  `claude`, `axon` were derived by inspecting which stdio upstreams in the live config
  have basenames not in `ALLOWED_RUNTIME_HINTS`. Kept alongside `disable_spawn_guard`
  as documentation of what the custom binaries actually are.

## Files Changed

| Status | Path | Purpose |
|--------|------|---------|
| modified | `crates/lab/src/config.rs` | Added `GatewayPreferences` struct and `pub gateway: GatewayPreferences` field to `LabConfig` |
| modified | `crates/lab/src/dispatch/gateway/params.rs` | Added `deserialize_with = "deserialize_nullable"` to `url` and `command` in `GatewayUpdatePatch` |
| modified | `crates/lab/src/dispatch/gateway/config.rs` | Threaded `GatewayPreferences` through `validate_upstream`, `validate_upstreams`, `validate_stdio_upstream`; updated all call sites |
| modified | `crates/lab/src/dispatch/upstream/spawn_guard.rs` | Added `extra: &[String], bypass: bool` params to `validate_stdio_command` and `validate_stdio_spec`; added tests for extra allowlist and bypass |
| modified | `crates/lab/src/dispatch/marketplace/mcp_params.rs` | Replaced duplicate `validate_runtime_hint` with delegation to `spawn_guard::validate_stdio_command`; fixed test callsite |
| modified | `crates/lab/src/dispatch/marketplace/mcp_dispatch.rs` | Updated all `install_stdio`, `build_stdio_command`, `mcp_client_config` signatures to accept `&GatewayPreferences`; fixed all test callsites |
| modified | `config/config.example.toml` | Added `[gateway]` section with commented-out `extra_stdio_commands` and `disable_spawn_guard` examples |
| modified | `docs/runtime/CONFIG.md` | Added `[gateway]` section reference table with rules and examples |
| modified | `docs/services/GATEWAY.md` | Added "Spawn Guard" subsection under "Stdio Gateways" |
| modified | `~/.labby/config.toml` | Added `[gateway]` section with `extra_stdio_commands = ["synapse","ytdl-mcp","claude","axon"]` and `disable_spawn_guard = true` |

## Beads Activity

No bead activity observed. This was a direct bug-fix session initiated from a user
complaint; no issue was tracked in beads before or during the session.

## Repository Maintenance

### Plans
- `docs/plans/fleet-ws-plan-lab-n07n.md` — active brainstorm/plan for WebSocket fleet
  transport (bead `lab-n07n`, status open). Not touched by this session. Left in place.
- `docs/plans/mcp-streamable-http-oauth-proxy.md` — active implementation plan for
  streamable HTTP + OAuth proxy. Not touched by this session. Left in place.
- No plans completed this session; `docs/plans/complete/` not created.

### Worktrees and branches
- `wt-jouhb` worktree at `.claude/worktrees/jouhb` — branch `wt-jouhb` is 3 commits
  ahead of `main` (Windows Job Object reaping for stdio upstream and Code Mode children,
  PR #108 review+fix pass). Active unmerged work. Left untouched.
- No stale worktrees or branches found.

### Stale docs
- `docs/runtime/CONFIG.md`, `docs/services/GATEWAY.md`, `config/config.example.toml`
  were all updated during the session to reflect the new `[gateway]` knobs. These were
  the only docs that needed changing.
- `docs/UPSTREAM.md` defers to `GATEWAY.md` for the stdio security model — already
  correct, no edit needed.
- `docs/surfaces/TRANSPORT.md` references the gateway trust model without listing config
  knobs — already correct, no edit needed.
- `docs/dev/` had no spawn-guard references — confirmed by grep, no edit needed.

## Tools and Skills Used

- **Shell / Bash** — `cargo check`, `cargo nextest run`, `git add`, `git commit`,
  `git push`, `grep`, `find` for file discovery and verification.
- **Read / Edit / Write tools** — file inspection and targeted edits to Rust source,
  TOML config, and Markdown docs.
- **superpowers:systematic-debugging skill** — invoked at session start to enforce
  root-cause-first discipline before proposing fixes.

## Commands Executed

| Command | Result |
|---------|--------|
| `cargo check --all-features` | Clean after all changes |
| `cargo nextest run --all-features` | 1st run: 1 compile error (missed callsite in `mcp_params.rs:386`) |
| `cargo nextest run --all-features` | 2nd run: 1837/1837 passed, 27 skipped |
| `git add <6 source files> && git commit` | Commit `b9c05dad` |
| `git push` | Pushed to `origin/main` |
| `git add <3 doc files> && git commit` | Commit `57a15d5a` |
| `git push` | Pushed to `origin/main` |

## Errors Encountered

- **Missed test callsite in `mcp_params.rs:386`** — `validate_runtime_hint("/tmp/evil")` was
  called with 1 argument after the function signature grew to 3. A `sed` bulk-fix pass
  covered single-line calls in `mcp_dispatch.rs` but not the test in `mcp_params.rs`
  itself. Caught by `cargo nextest` compile error; fixed manually.
- **Two multi-line test calls in `mcp_dispatch.rs`** (`install_stdio` at lines 1041 and
  1054) were not matched by the `sed` pattern (which targeted single-line calls). Fixed
  manually after the build failure.

## Behavior Changes (Before / After)

| Area | Before | After |
|------|--------|-------|
| Editing an HTTP upstream in the gateway UI | Triggered "stdio command doesn't match allowlist" error because JSON `null` for `command` was silently ignored, leaving stale `command` set alongside new `url` | No error — `command` is correctly cleared to `None`, routing validation through the HTTP path |
| Adding a custom stdio binary (e.g. `labby`) | Blocked by hardcoded `ALLOWED_RUNTIME_HINTS` with no override | Can add via `[gateway] extra_stdio_commands = ["labby"]` in `config.toml` |
| Operators who don't want the guard at all | No escape hatch | `[gateway] disable_spawn_guard = true` bypasses all command validation |
| Marketplace `mcp.install` runtime hint check | Used a local copy of `ALLOWED_RUNTIME_HINTS` in `mcp_params.rs` | Delegates to `spawn_guard::validate_stdio_command` — single source of truth |

## Verification Evidence

| Command | Expected | Actual | Status |
|---------|----------|--------|--------|
| `cargo check --all-features` | No errors | No errors | pass |
| `cargo nextest run --all-features` | 1837 passed | 1837 passed, 27 skipped | pass |
| `git show --name-only b9c05dad` | 6 source files | `config.rs`, `gateway/config.rs`, `gateway/params.rs`, `mcp_dispatch.rs`, `mcp_params.rs`, `spawn_guard.rs` | pass |
| `git show --name-only 57a15d5a` | 3 doc files | `config/config.example.toml`, `docs/runtime/CONFIG.md`, `docs/services/GATEWAY.md` | pass |

## Risks and Rollback

- **`disable_spawn_guard = true` in live config** — the guard is the primary defence
  against operators being tricked into adding arbitrary-command stdio upstreams via
  `gateway.add`. With the guard off, the HTTP auth layer and the destructive-confirm
  gate on `gateway.add` / `gateway.update` remain as the only controls. This is an
  intentional operator choice on a trusted single-user homelab instance.
- **Rollback** — remove `[gateway]` section from `~/.labby/config.toml` and reload the
  gateway (`labby gateway reload`). The code change is backward-compatible: omitting the
  section defaults to `extra_stdio_commands = []` and `disable_spawn_guard = false`,
  which is identical to pre-patch behaviour.

## Next Steps

- **Reload the gateway** to pick up the new `[gateway]` config: `labby gateway reload`
  (or restart `labby serve`). The `disable_spawn_guard` flag is read at config load time.
- **wt-jouhb / PR #108** (Windows Job Object reaping) — the branch is ahead of main with
  active work. Review and merge when ready.
- **fleet-ws plan (`lab-n07n`)** and **mcp-streamable-http-oauth-proxy plan** remain open;
  no action needed from this session.
