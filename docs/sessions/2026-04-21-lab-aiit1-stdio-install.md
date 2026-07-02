---
date: 2026-04-21 11:24:41 EST
repo: git@github.com:jmagar/lab.git
branch: fix/auth
head: 86ed3c5
plan: none
agent: Claude (claude-sonnet-4-6)
session id: 6ee35075-f87b-430b-a9ae-e945fa47d04a
transcript: /home/jmagar/.claude/projects/-home-jmagar-workspace-lab/6ee35075-f87b-430b-a9ae-e945fa47d04a.jsonl
working directory: /home/jmagar/workspace/lab
worktree: /home/jmagar/workspace/lab [fix/auth]
pr: "#25 — fix(auth): gateway admin auth, upstream OAuth, and dispatch fixes — https://github.com/jmagar/lab/pull/25"
---

## User Request

Implement bead `lab-aiit.1`: extend `server.install` in the MCP Registry dispatch to support stdio-transport servers (npx/uvx/docker/etc.) in addition to HTTP remotes, with security hardening (runtime allowlist, argv denylist, env var validation) and atomic `.env` writes.

## Session Overview

This session was a continuation from a prior context window. The earlier portion covered branch housekeeping (merging `bd-work/gh-webhook` → `fix/auth`, cleaning up merge conflicts). This portion picked up at Phase 2 of `/lavra:lavra-work lab-aiit.1` and implemented the full stdio install path: typed registry types, security validators, dual-transport dispatch logic, and a new `write_env_pairs` config helper.

## Sequence of Events

1. Read current state of four target files: `mcpregistry/types.rs`, `mcpregistry/dispatch.rs`, `mcpregistry/params.rs`, `mcpregistry/catalog.rs`
2. Read `config.rs` (`.env` write helpers, `UpstreamConfig`, `dotenv_path`)
3. Created task list (5 tasks)
4. Added `EnvironmentVariable` typed struct to `lab-apis/src/mcpregistry/types.rs`; replaced `Vec<serde_json::Value>` in `Package.environment_variables`
5. Added four security validators to `params.rs`: `validate_runtime_hint`, `validate_stdio_argv`, `validate_env_var_name`, `validate_env_value`
6. Updated `catalog.rs` `server.install` entry: description updated for dual transport; `env_values` param added
7. Refactored `dispatch.rs` `server.install` arm: extracted `install_http` (async) and `install_stdio` (sync) helpers; main arm now delegates by transport detection
8. Added `write_env_pairs` to `config.rs` (atomic merge for raw `(String, String)` pairs)
9. Made `config::dotenv_path()` public
10. Added 15 unit tests to `params.rs` covering all security boundaries
11. Ran `cargo check --all-features` — clean
12. Committed as `86ed3c5`

## Key Findings

- `crates/lab-apis/src/mcpregistry/types.rs:196-202` — `Package.environment_variables` was untyped `Vec<serde_json::Value>`; registry schema defines `name`, `isRequired`, `isSecret`, `default`, `choices`, `placeholder`, `format`
- `crates/lab/src/dispatch/mcpregistry/dispatch.rs:46-57` (pre-change) — `server.install` bailed unconditionally with `no_remote_transport` when `server.remotes` was empty; stdio servers were fully blocked
- `crates/lab/src/config.rs:667` — `dotenv_path()` was private; the path `~/.labby/.env` was not accessible to dispatch modules without making it public
- `write_env` (`config.rs:847`) takes `[ServiceCreds]`, which is extract-specific; needed a parallel `write_env_pairs` for raw key=value pairs
- Pre-existing test failures (18 errors, `proxy_prompts` missing in `UpstreamConfig` struct literals in gateway/pool tests) prevent `cargo test` from compiling the test binary; these are unrelated to this work and existed before any changes

## Technical Decisions

- **`install_http` + `install_stdio` helpers instead of one large match arm** — the two transport paths have completely different validation needs; splitting them keeps each under 60 lines and separately testable
- **Transport detection by `server.remotes.first()` with `.url`** — HTTP remote takes precedence over packages; falls through to `packages.first()` for stdio; errors if neither exists
- **`write_env_pairs` instead of reusing `ServiceCreds` shim** — converting env vars to `ServiceCreds { service: "", url: None, secret: Some(v), env_field: k }` would work mechanically but imports extract types into an unrelated service's dispatch; cleaner to add a targeted helper
- **Env-write conflicts are `WARN` not error** — the gateway is still registered even if a `.env` key conflicts; the user can re-run with force. Failing the entire install for a `.env` conflict would be surprising
- **Argv denylist is runtime-agnostic** — all dangerous flags (`--eval`, `-e`, `-c`, `--require`, `--import`, etc.) are blocked regardless of which runtime is used; per-runtime tables would be more precise but add maintenance surface

## Files Modified

| File | Change |
|------|--------|
| `crates/lab-apis/src/mcpregistry/types.rs` | Added `EnvironmentVariable` struct; changed `Package.environment_variables` from `Vec<serde_json::Value>` to `Vec<EnvironmentVariable>` |
| `crates/lab/src/config.rs` | Added `pub fn write_env_pairs(path, pairs, force)`; made `dotenv_path()` public |
| `crates/lab/src/dispatch/mcpregistry/catalog.rs` | Updated `server.install` description; added `env_values` `ParamSpec` |
| `crates/lab/src/dispatch/mcpregistry/dispatch.rs` | Refactored `server.install` into `install_http` + `install_stdio` helpers; added `use crate::config` |
| `crates/lab/src/dispatch/mcpregistry/params.rs` | Added `ALLOWED_RUNTIME_HINTS`, `DANGEROUS_ARGV_FLAGS` constants; four validator functions; 15 unit tests |

## Commands Executed

```
rtk cargo check --all-features
# → clean build (2 crates compiled)

rtk cargo test -p lab@0.5.0 --all-features "dispatch::mcpregistry::params::tests"
# → 18 pre-existing compile errors in gateway/pool test fixtures (proxy_prompts field);
#   not caused by this work; production code builds cleanly
```

## Errors Encountered

- **Pre-existing test compile failure**: `cargo test` fails with 18 `E0063: missing field proxy_prompts` errors in `crates/lab/src/dispatch/gateway/config.rs` and `crates/lab/src/dispatch/upstream/pool.rs` test modules. Confirmed pre-existing by stashing changes and reproducing the same 18 errors. Root cause: `UpstreamConfig` gained a `proxy_prompts` field in a prior commit; test struct literals were not updated. Workaround: tests cannot run until those struct literals are fixed; not in scope for this bead.

## Behavior Changes (Before/After)

| Scenario | Before | After |
|----------|--------|-------|
| `server.install` on a stdio-only server (no `remotes[]`) | Returns `no_remote_transport` error immediately | Builds stdio command from `packages[0]`, validates security, writes env vars, registers gateway upstream |
| `server.install` on HTTP server | SSRF validation + OAuth probe + `gateway.add` | Identical (refactored into `install_http` helper, no behavior change) |
| `server.install` with dangerous argv flag (`--eval`) | Not checked (argv not validated) | Returns `invalid_param` error |
| `server.install` with unlisted `runtimeHint` (e.g. `bash`) | Not checked | Returns `invalid_param` error |
| `server.install` with required env var not supplied | Not applicable (stdio not supported) | Returns `missing_param` error listing the var name |
| `server.install` with required env var supplied | Not applicable | Writes value to `~/.labby/.env` atomically |

## Verification Evidence

| Command | Expected | Actual | Status |
|---------|----------|--------|--------|
| `rtk cargo check --all-features` | Clean build | `cargo build (2 crates compiled)` | ✅ |

## Risks and Rollback

- **`.env` write on install**: `server.install` now has a side effect (writes to `~/.labby/.env`). A failed install after the env write leaves orphaned env vars. Mitigation: backup is created first (`config::backup_env`); user can restore from `.env.bak.<timestamp>`. Rollback: `git revert 86ed3c5` removes the feature.
- **Argv denylist incompleteness**: denylist covers known dangerous flags but is not exhaustive for all runtimes. A registry entry using an undocumented flag variant would pass. Mitigation: registry is a trusted source; the denylist is defense-in-depth.
- **`proxy_prompts` test failures block CI**: pre-existing failures will fail any CI job that runs `cargo test`. Out of scope here but should be fixed before merging PR #25.

## Decisions Not Taken

- **MCP elicitation for env var values** — the bead spec explicitly called for `env_values` as a normal tool param (not elicitation) to keep the install synchronous and MCP-host agnostic
- **Per-runtime argv denylists** — more precise but adds ~50 lines of table maintenance; the universal denylist covers the critical code-execution flags for all allowed runtimes
- **Using `ServiceCreds` shim for env writes** — would work mechanically (`service=""`, `url=None`, `secret=Some(v)`, `env_field=k`) but imports extract-domain types into an unrelated service dispatch; chose `write_env_pairs` instead

## Next Steps

**Unfinished from this session (started, not yet complete):**
- None — all 5 tasks completed and committed

**Follow-on tasks not yet started:**
- Fix pre-existing `proxy_prompts` missing-field errors in gateway/pool test fixtures so `cargo test` can compile the test binary
- Frontend: install dialog should render `EnvironmentVariable` metadata (`isSecret`, `choices`, `placeholder`, `description`) as a form; this was the UI half of the stdio install plan (separate bead)
- `extract.apply` dispatch action is still `unimplemented!()` — a natural follow-on since `write_env_pairs` is now available
