---
date: 2026-04-25 14:52:29 EST
repo: git@github.com:jmagar/lab.git
branch: bd-security/marketplace-p1-fixes
head: f168964b
agent: Codex
working directory: /home/jmagar/workspace/lab
worktree: /home/jmagar/workspace/lab f168964b [bd-security/marketplace-p1-fixes]
pr: "#29 fix(marketplace): P1 security fixes — path traversal, symlink following, installPath validation — https://github.com/jmagar/lab/pull/29"
---

# Session: MCP Registry Config and Shared Workspace Stash

## User Request

The session began with startup warnings:

```text
WARN  mcpregistry client unavailable; registry background sync disabled  error="{\"kind\":\"not_configured\",\"message\":\"MCPREGISTRY_URL not set\"}"
WARN  LAB_WORKSPACE_ROOT not set; fs service registered but every fs.* call will return workspace_not_configured until it is configured
```

The user asked to systematically debug why `MCPREGISTRY_URL` was not configured, what `LAB_WORKSPACE_ROOT` persisted, why workspace root was read-only, and whether it was the same root used for stash files. Follow-up requirements changed the design:

- Do not put `MCPREGISTRY_URL` in `~/.labby/.env`.
- Use the official MCP Registry URL by default.
- Load registry URL override from `~/.labby/config.toml`.
- Combine workspace root and stash root into one TOML config option.
- Use that shared root for the attachment picker and marketplace editable plugin stash.
- Move editable plugin mirrors from `~/.claude/plugins/workspaces/<plugin-id>/` to `~/.labby/stash/plugins/<plugin-id>/`.
- Migrate existing workspace mirrors if present.
- Verify the fixes, including live CLI/API/MCP behavior and tests.
- Save this session as a markdown document with concrete repo and git context.

## Session Overview

Implemented config-driven MCP Registry and workspace/stash behavior in the `lab` binary. `mcpregistry` now defaults to `https://registry.modelcontextprotocol.io` and can be overridden by `[mcpregistry].url` in `config.toml`. The fs workspace browser and marketplace editable plugin mirrors now use `[workspace].root`, defaulting to `~/.labby/stash`; marketplace plugin mirrors live under `<workspace.root>/plugins`.

During verification, the live `marketplace mcp.config` dispatch initially failed because `mcp.*` actions were implemented but missing from the marketplace action catalog used by CLI/API/MCP gates. That was fixed by combining marketplace plugin actions with `MCP_ACTIONS`. A second MCP live failure showed synthetic services without gateway metadata were blocked by gateway MCP action policy; that was fixed with a regression test. A full lib test run later exposed an fs validation regression for file targets, which was fixed by checking existing non-directory targets before `create_dir_all`.

## Sequence of Events

1. Confirmed the registry warning came from code requiring `MCPREGISTRY_URL`.
2. Changed the registry client construction to read `~/.labby/config.toml` and use the official MCP Registry URL as a default.
3. Removed the need for `MCPREGISTRY_URL` in `~/.labby/.env`.
4. Added `[workspace].root` config with default `~/.labby/stash`.
5. Changed fs workspace root resolution from `LAB_WORKSPACE_ROOT` to config-driven `workspace.root`.
6. Changed marketplace editable plugin mirrors to `<workspace.root>/plugins`.
7. Added migration from legacy `~/.claude/plugins/workspaces/<plugin-id>` to the new stash-backed location.
8. Updated local `/home/jmagar/.labby/config.toml` with `[workspace]` and `[mcpregistry]`.
9. Removed `MCPREGISTRY_URL=https://registry.modelcontextprotocol.io` from `/home/jmagar/.labby/.env`.
10. Ran targeted builds/tests and live CLI/API/MCP checks.
11. Found and fixed the missing marketplace `mcp.*` catalog exposure.
12. Found and fixed synthetic service MCP policy blocking `marketplace mcp.config`.
13. Found and fixed fs file-target validation regression exposed by the full lib test suite.
14. Reran the full `lab` lib suite and feature build successfully.
15. Gathered repo/git/session context and wrote this session document.

## Key Findings

- Registry default is now defined in code as `DEFAULT_MCPREGISTRY_URL` at `crates/lab/src/config.rs:29`.
- TOML config now includes workspace and registry preference structs at `crates/lab/src/config.rs:645` and `crates/lab/src/config.rs:654`.
- Registry URL resolution falls back to the official default at `crates/lab/src/config.rs:810`.
- Workspace root resolution defaults to `~/.labby/stash` through `workspace_root_path` at `crates/lab/src/config.rs:830`.
- MCP Registry client construction now uses `configured_registry_url()` at `crates/lab/src/dispatch/marketplace/mcp_client.rs:25`.
- fs workspace root startup wiring now resolves config and logs `fs.workspace_root` at `crates/lab/src/cli/serve.rs:388`.
- fs root validation creates missing directories but rejects existing files with `InvalidInput` at `crates/lab/src/dispatch/fs/client.rs:43`.
- Marketplace workspace paths are rooted under config-derived `<workspace.root>/plugins` at `crates/lab/src/dispatch/marketplace/dispatch.rs:641`.
- Legacy marketplace workspace migration is implemented around `legacy_workspace_dir_for_plugin` and migration handling at `crates/lab/src/dispatch/marketplace/dispatch.rs:669` and `crates/lab/src/dispatch/marketplace/dispatch.rs:715`.
- `mcp.config` dispatch returns the configured registry URL at `crates/lab/src/dispatch/marketplace/mcp_dispatch.rs:49`.
- Marketplace API and CLI action validation now use `crate::dispatch::marketplace::actions()` at `crates/lab/src/api/services/marketplace.rs:59` and `crates/lab/src/cli/marketplace.rs:18`.
- Marketplace action catalog now appends `MCP_ACTIONS` at `crates/lab/src/dispatch/marketplace/catalog.rs:397`.
- Synthetic-service MCP action policy regression is covered at `crates/lab/src/dispatch/gateway/manager.rs:3939`.
- Central config docs now describe `[workspace]` at `docs/CONFIG.md:132` and `[mcpregistry]` at `docs/CONFIG.md:147`.
- Marketplace docs now describe `<workspace.root>/plugins` and legacy migration at `docs/MARKETPLACE.md:155`.
- MCP Registry coverage docs now describe the current `marketplace mcp.*` surface at `docs/coverage/mcpregistry.md:42`.

## Technical Decisions

- Registry URL is a non-secret operator preference, not a service credential, so it belongs in `config.toml` rather than `.env`.
- The official registry URL is a built-in default so a normal install does not require nonessential environment setup.
- Workspace root and stash root are one config option because the attachment picker and editable plugin stash are both local Lab-managed file surfaces.
- Marketplace editable plugin mirrors use a `plugins/` child directory under the shared workspace root to keep the root useful for other future stash/workspace content.
- fs startup creates the configured workspace root when missing. This allows the default `~/.labby/stash` to work on first run.
- Existing non-directory workspace root targets are rejected before `create_dir_all` so callers get the stable validation error shape expected by tests.
- Legacy marketplace mirrors are migrated on first access only when the new mirror does not already exist, avoiding overwriting newer stash content.
- `mcp.*` registry actions remain under the `marketplace` service surface, so the marketplace catalog must include those actions for CLI/API/MCP gates.
- Synthetic services without gateway service metadata should not be blocked by gateway MCP policy; they are allowed unless explicitly governed.

## Files Modified

Session-relevant files modified:

- `crates/lab/src/config.rs` — added default registry URL, `[workspace]`, `[mcpregistry]`, path expansion, root/url helper functions, and tests.
- `crates/lab/src/dispatch/marketplace/mcp_client.rs` — changed registry client construction from env-only to config/default-based.
- `crates/lab/src/dispatch/marketplace/mcp_dispatch.rs` — changed `mcp.config` to return config/default-derived registry URL.
- `crates/lab/src/dispatch/fs/client.rs` — changed workspace root resolution from `LAB_WORKSPACE_ROOT` to `[workspace].root`; added create-missing-dir behavior and file-target validation.
- `crates/lab/src/dispatch/fs.rs` — re-exported config-based root resolution.
- `crates/lab/src/dispatch/fs/dispatch.rs` — updated comments/tests around config-based workspace behavior.
- `crates/lab/src/cli/serve.rs` — changed startup fs root wiring and removed the registry not-configured warning path for missing env.
- `crates/lab/src/dispatch/marketplace/dispatch.rs` — moved editable plugin mirrors to `<workspace.root>/plugins`, added legacy migration, and updated tests.
- `crates/lab/src/dispatch/marketplace/client.rs` — added/used test helper behavior for marketplace workspace-root tests.
- `crates/lab/src/dispatch/marketplace/catalog.rs` — combined marketplace action catalog with `MCP_ACTIONS`.
- `crates/lab/src/dispatch/marketplace.rs` — exposed the combined marketplace `actions()` function.
- `crates/lab/src/api/services/marketplace.rs` — switched API validation to combined marketplace actions.
- `crates/lab/src/cli/marketplace.rs` — switched CLI parser/confirmation validation to combined marketplace actions.
- `crates/lab/src/dispatch/gateway/manager.rs` — allowed synthetic services without gateway metadata through MCP action policy and added regression test.
- `crates/lab/src/registry.rs` — registered marketplace with the combined action catalog.
- `crates/lab/src/api/state.rs` — updated workspace-root comments.
- `crates/lab/src/dispatch/CLAUDE.md` — updated fs registration notes to reference `[workspace].root`.
- `docs/CONFIG.md` — documented `[workspace]` and `[mcpregistry]`.
- `docs/MARKETPLACE.md` — documented `<workspace.root>/plugins` and legacy mirror migration.
- `docs/coverage/mcpregistry.md` — updated coverage docs from standalone `mcpregistry` surface to current `marketplace mcp.*` surface.
- `/home/jmagar/.labby/config.toml` — added `[workspace] root = "~/.labby/stash"` and `[mcpregistry] url = "https://registry.modelcontextprotocol.io"`.
- `/home/jmagar/.labby/.env` — removed the `MCPREGISTRY_URL` line.

Observed dirty worktree context:

- `git status --short` showed extensive pre-existing modifications across Rust crates, docs, and `apps/gateway-admin`.
- Relevant status examples included `crates/lab/src/config.rs`, `crates/lab/src/dispatch/fs/client.rs`, `crates/lab/src/dispatch/marketplace/dispatch.rs`, `crates/lab/src/dispatch/gateway/manager.rs`, `docs/CONFIG.md`, `docs/MARKETPLACE.md`, and `docs/coverage/mcpregistry.md`.
- The dirty worktree also showed many unrelated files and deleted legacy `crates/lab/src/mcp/services/*.rs` shims; this session did not attempt to revert unrelated changes.

## Commands Executed

Context gathering:

```bash
TZ=America/New_York date '+%Y-%m-%d %H:%M:%S EST'
# 2026-04-25 14:52:29 EST

git remote get-url origin
# git@github.com:jmagar/lab.git

git branch --show-current
# bd-security/marketplace-p1-fixes

git rev-parse --short HEAD
# f168964b

git log --oneline -5
# f168964b fix(lab-zxx5.32): R2 P3 roll-up — redact_home in errors, log tiering, sync_all, dead Sized
# 39266dce refactor(lab-f1t2): address simplify + review findings on the f1t2 wave
# b7f488af fix(lab-zxx5.30,lab-zxx5.31): partial-extraction detection + fail-closed walk
# 7b051062 fix(lab-zxx5.29): validate node install result shape
# 12eb0ea0 fix(lab-zxx5.28): typed error markers restore install taxonomy

pwd
# /home/jmagar/workspace/lab

git worktree list | grep "$(pwd)" | head -1
# /home/jmagar/workspace/lab f168964b [bd-security/marketplace-p1-fixes]

gh pr view --json number,title,url 2>/dev/null || echo "none"
# {"number":29,"title":"fix(marketplace): P1 security fixes — path traversal, symlink following, installPath validation","url":"https://github.com/jmagar/lab/pull/29"}
```

Critical implementation verification:

```bash
cargo build -p 'path+file:///home/jmagar/workspace/lab/crates/lab#0.11.0' --features fs,mcpregistry
# Finished `dev` profile ... exit 0

cargo test -p 'path+file:///home/jmagar/workspace/lab/crates/lab#0.11.0' --lib dispatch::upstream::pool::tests::in_process_registration_isolates_slow_services_from_fast_services --features fs,mcpregistry
# 1 passed; 0 failed

cargo test -p 'path+file:///home/jmagar/workspace/lab/crates/lab#0.11.0' --lib dispatch::marketplace::dispatch::tests --features fs,mcpregistry
# 16 passed; 0 failed

cargo test -p 'path+file:///home/jmagar/workspace/lab/crates/lab#0.11.0' --lib dispatch::gateway::manager::tests::synthetic_services_without_gateway_metadata_allow_mcp_actions --features fs,mcpregistry
# 1 passed; 0 failed

cargo test -p 'path+file:///home/jmagar/workspace/lab/crates/lab#0.11.0' --lib --features fs,mcpregistry
# 792 passed; 0 failed

git diff --check -- <touched files>
# exit 0
```

Live behavior verification:

```bash
target/debug/lab marketplace mcp.config --json
# {"url":"https://registry.modelcontextprotocol.io"}

LAB_LOG='labby=info,lab_apis=warn' target/debug/lab serve --services fs,marketplace --host 127.0.0.1 --port 18770
# startup log included: workspace filesystem browser enabled path=/home/jmagar/.labby/stash
# startup log included: registry sync complete
# startup log included: lab serve ready

curl -fsS -H "Authorization: Bearer <redacted>" http://127.0.0.1:18770/ready
# {"status":"ready"}

curl -fsS -H "Authorization: Bearer <redacted>" 'http://127.0.0.1:18770/v1/fs/list?path='
# {"entries":[{"name":"plugins","path":"plugins","kind":"dir"}],"truncated":false}

curl -fsS -H "Authorization: Bearer <redacted>" -H 'content-type: application/json' \
  -X POST http://127.0.0.1:18770/v1/marketplace \
  -d '{"action":"mcp.config","params":{}}'
# {"url":"https://registry.modelcontextprotocol.io"}

curl ... /mcp tools/call marketplace action=mcp.config
# {"ok":true,"service":"marketplace","action":"mcp.config","data":{"url":"https://registry.modelcontextprotocol.io"}}

curl -fsS -H "Authorization: Bearer <redacted>" -H 'content-type: application/json' \
  -X POST http://127.0.0.1:18770/v1/marketplace \
  -d '{"action":"plugin.save","params":{"id":"plugin-lab@claude-homelab","path":".lab-verification","content":"workspace root verification\n"}}'
# {"savedAt":"2026-04-25T04:47:10.020794011Z"}

test -f /home/jmagar/.labby/stash/plugins/plugin-lab@claude-homelab/.lab-verification
# yes

test -f /home/jmagar/.claude/plugins/workspaces/plugin-lab@claude-homelab/.lab-verification
# no
```

Config/env verification:

```bash
rg -n '^MCPREGISTRY_URL=' /home/jmagar/.labby/.env || true
# no output

sed -n '1,45p' /home/jmagar/.labby/config.toml | rg -n '\[workspace\]|root =|\[mcpregistry\]|url ='
# [workspace]
# root = "~/.labby/stash"
# [mcpregistry]
# url = "https://registry.modelcontextprotocol.io"
```

## Errors Encountered

- Initial startup warning: `MCPREGISTRY_URL not set`. Root cause: registry client construction required an env var for a nonessential public URL. Resolution: use `[mcpregistry].url` from TOML and default to `https://registry.modelcontextprotocol.io`.
- Initial startup warning: `LAB_WORKSPACE_ROOT not set`. Root cause: fs service used a separate env var for workspace root while marketplace stash wanted a separate path. Resolution: use `[workspace].root`, defaulting to `~/.labby/stash`.
- Live `mcp.config` initially failed through API/MCP as `unknown_action`. Root cause: `mcp.*` dispatch existed, but `MCP_ACTIONS` were not included in the marketplace action catalog used by validation gates. Resolution: combine marketplace plugin actions with `MCP_ACTIONS`.
- Live MCP `tools/call` for `marketplace mcp.config` initially failed as `action not exposed`. Root cause: gateway MCP policy blocked services without gateway metadata, including synthetic `marketplace`. Resolution: allow services without gateway metadata unless explicitly governed and add a regression test.
- Full lib test initially failed at `dispatch::fs::client::tests::canonicalize_existing_dir_rejects_file_target`. Root cause: `create_dir_all` on an existing file returned `AlreadyExists` before the explicit `InvalidInput` validation. Resolution: check existing non-directory targets before `create_dir_all`.
- A parallel Cargo run produced an apparent upstream test compile error referencing `InProcessConnector` and `InProcessRegistration`. A serial rerun of the exact test passed; the full lib suite later passed. No code change was needed for that apparent failure.

## Behavior Changes (Before/After)

| Area | Before | After |
|------|--------|-------|
| MCP Registry URL | Required `MCPREGISTRY_URL` env var. Missing env disabled registry sync. | Defaults to official MCP Registry URL and optionally reads `[mcpregistry].url`. |
| `.env` | Contained or was expected to contain `MCPREGISTRY_URL`. | No registry URL needed in `.env`. |
| fs workspace root | Required `LAB_WORKSPACE_ROOT`. Missing env caused `workspace_not_configured`. | Uses `[workspace].root`, defaulting to `~/.labby/stash`, and creates the missing directory. |
| Marketplace edit mirrors | Used `~/.claude/plugins/workspaces/<plugin-id>/`. | Uses `<workspace.root>/plugins/<plugin-id>/`, defaulting to `~/.labby/stash/plugins/<plugin-id>/`. |
| Legacy mirrors | Remained under `~/.claude/plugins/workspaces`. | Migrated on first access if the new mirror does not already exist. |
| Marketplace `mcp.*` actions | Dispatch existed but validation gates rejected them. | CLI/API/MCP action gates accept `mcp.*` actions. |
| Synthetic service MCP policy | `marketplace mcp.config` could be rejected by gateway action policy. | Synthetic services without gateway metadata are allowed unless explicitly governed. |

## Verification Evidence

| Command | Expected | Actual | Status |
|---------|----------|--------|--------|
| `cargo build -p lab --features fs,mcpregistry` | Feature build succeeds | Finished dev profile, exit 0 | Pass |
| `cargo test -p lab --lib dispatch::upstream::pool::tests::in_process_registration_isolates_slow_services_from_fast_services --features fs,mcpregistry` | Previously blocking upstream test passes | 1 passed, 0 failed | Pass |
| `cargo test -p lab --lib dispatch::marketplace::dispatch::tests --features fs,mcpregistry` | Marketplace workspace/migration tests pass | 16 passed, 0 failed | Pass |
| `cargo test -p lab --lib dispatch::gateway::manager::tests::synthetic_services_without_gateway_metadata_allow_mcp_actions --features fs,mcpregistry` | Synthetic service policy regression passes | 1 passed, 0 failed | Pass |
| `cargo test -p lab --lib --features fs,mcpregistry` | Full `lab` lib suite passes | 792 passed, 0 failed | Pass |
| `target/debug/lab marketplace mcp.config --json` | Official registry URL | `{"url":"https://registry.modelcontextprotocol.io"}` | Pass |
| Fresh `lab serve --services fs,marketplace --port 18770` | No registry/workspace warnings | Logs showed stash workspace enabled, registry sync complete, server ready | Pass |
| HTTP `/v1/fs/list` | Lists stash root content | Returned `plugins` dir | Pass |
| HTTP `/v1/marketplace` `mcp.config` | Official registry URL | `{"url":"https://registry.modelcontextprotocol.io"}` | Pass |
| MCP `tools/call` `marketplace` with `action=mcp.config` | Success envelope with URL | `ok:true`, URL `https://registry.modelcontextprotocol.io` | Pass |
| `plugin.save` verification | Writes under `~/.labby/stash/plugins` | File existed at new stash path and not at legacy path | Pass |
| `git diff --check -- <touched files>` | No whitespace errors | exit 0 | Pass |

## Risks and Rollback

- Risk: changing the workspace root default means first-run behavior creates `~/.labby/stash`; rollback would restore env-based `LAB_WORKSPACE_ROOT` resolution in `crates/lab/src/dispatch/fs/client.rs` and `crates/lab/src/cli/serve.rs`.
- Risk: marketplace edit mirrors now use `<workspace.root>/plugins`; rollback would restore the previous `~/.claude/plugins/workspaces` root in `crates/lab/src/dispatch/marketplace/dispatch.rs`.
- Risk: synthetic-service MCP policy now allows services without gateway metadata. Rollback would remove the early allow branch in `GatewayManager::mcp_action_allowed_for_service`, but that would re-break `marketplace mcp.config`.
- Operational rollback: restore `MCPREGISTRY_URL` env reading in `mcp_client.rs` only if the product decision changes back to env-based registry configuration.

## Decisions Not Taken

- Did not keep `MCPREGISTRY_URL` in `.env`; it is non-secret and nonessential.
- Did not keep separate workspace-root and stash-root settings; the user explicitly wanted them combined.
- Did not overwrite a new workspace mirror with a legacy mirror during migration; migration only happens when the new mirror is absent.
- Did not run broad `cargo fmt` to avoid unrelated formatting churn in a dirty worktree.
- Did not kill all existing `lab serve mcp --stdio` processes; live verification used fresh local server processes on test ports.

## References

- Active PR observed by `gh pr view`: https://github.com/jmagar/lab/pull/29
- Related plan file observed in repo search: `docs/superpowers/plans/2026-04-24-stash-implementation-plan.md`
- Config docs updated: `docs/CONFIG.md`
- Marketplace docs updated: `docs/MARKETPLACE.md`
- MCP Registry coverage docs updated: `docs/coverage/mcpregistry.md`

## Open Questions

- No transcript path or session identifier was exposed in the current environment.
- No active plan state was exposed by the environment. A related stash implementation plan exists at `docs/superpowers/plans/2026-04-24-stash-implementation-plan.md`, but this session did not verify that it was active.
- Existing long-running `lab serve mcp --stdio` clients were observed earlier in the session, but this session did not restart user-owned clients.

## Next Steps

Unfinished from this session:

- Restart any existing long-running `lab serve mcp --stdio` clients that should pick up the rebuilt binary/config.

Follow-on tasks not yet started:

- Decide whether to do a separate formatting-only cleanup if the repo owner wants to normalize rustfmt drift.
- Decide whether to document migration behavior in any additional operator docs beyond `docs/CONFIG.md`, `docs/MARKETPLACE.md`, and `docs/coverage/mcpregistry.md`.
- Review the large dirty worktree before committing, because many unrelated files were dirty before and during this session.
