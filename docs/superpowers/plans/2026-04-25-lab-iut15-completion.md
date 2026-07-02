# lab-iut1.5 Update Detection and Preview Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [x]`) syntax for tracking.

**Goal:** Implement `artifact.update.check` and `artifact.update.preview` for forked marketplace artifacts so bead `lab-iut1.5` can close.

**Architecture:** Extend the settled `crates/lab/src/dispatch/marketplace/update.rs` marketplace domain module, preserving the apply/merge/config behavior added by `lab-iut1.6`. The module reads `.stash.json` metadata from `~/.labby/stash/plugins/<plugin_id>`, hardens git fetches, compares upstream versions from fetched refs, computes diff3 preview state with `diffy-imara`, and writes pending preview state to `.pending-update.json`.

**Tech Stack:** Rust 2024, Tokio process/timeouts, serde/serde_json, dashmap fetch guards, diffy-imara 0.3 for patches and 3-way merge, existing `ToolError` envelopes.

---

## File Structure

- Preserve `crates/lab/src/dispatch/marketplace.rs` with its existing `mod update;`.
- Preserve `crates/lab/src/dispatch/marketplace/dispatch.rs` artifact routing to `update::dispatch_update_action`.
- Modify `crates/lab/src/dispatch/marketplace/catalog.rs` to expose both actions and schemas.
- Modify `crates/lab/src/dispatch/marketplace/params.rs` to parse `UpdateCheckParams` and `UpdatePreviewParams`.
- Modify `crates/lab/src/dispatch/marketplace/update.rs` for stash metadata, safe marketplace path resolution, hardened git fetch, version lookup, update result caching, pending preview writing, diff generation, and tests.
- Modify `crates/lab/Cargo.toml` to add `diffy-imara = "0.3"`.
- Modify `docs/ERRORS.md` to document marketplace update-specific structured error kinds.
- Modify `docs/MCP.md` to document the two new marketplace actions.
- Create `docs/sessions/2026-04-25-lab-iut15-completion.md` after verification.

## Tasks

### Task 1: Wire action metadata and parameter parsing

**Files:**
- Modify: `crates/lab/src/dispatch/marketplace/catalog.rs`
- Modify: `crates/lab/src/dispatch/marketplace/params.rs`
- Modify: `crates/lab/src/dispatch/marketplace.rs`
- Modify: `crates/lab/src/dispatch/marketplace/dispatch.rs`

- [x] Add `artifact.update.check` ActionSpec with optional `plugin_id`.
- [x] Add `artifact.update.preview` ActionSpec with required `plugin_id`.
- [x] Add `UpdateCheckParams` and `UpdatePreviewParams` parsers that validate plugin ids through `parse_plugin_id`.
- [x] Preserve existing `mod update;` and artifact dispatch routing.

### Task 2: Implement update detection

**Files:**
- Modify: `crates/lab/src/dispatch/marketplace/update.rs`
- Modify: `crates/lab/Cargo.toml`

- [x] Add tolerant `StashMeta`, `ForkType`, `UpdateCheckResult`, and cache-state structs.
- [x] Resolve stash dirs from `workspace_root()/plugins` and scan `.stash.json` files when no `plugin_id` is passed.
- [x] Resolve marketplace source dirs from `known_marketplaces.json` and `~/.claude/plugins/marketplaces/<marketplace>` with canonical safe-root checks.
- [x] Add hardened git fetch with `GIT_TERMINAL_PROMPT=0`, `GIT_ASKPASS=/bin/true`, `GIT_CONFIG_NOSYSTEM=1`, `GIT_CONFIG_GLOBAL=/dev/null`, `GIT_CONFIG_COUNT=0`, config-disabling `-c` args, optional runtime `--no-config`, 30s timeout, and per-marketplace mutex guard.
- [x] Map missing git to `git_not_available` and exit code 128 to `marketplace_auth_required` without credential leakage.
- [x] Read remote plugin version from `origin/HEAD` fetched refs, compare against `.stash.json` `upstream_version`, cache `.update-check.json`, and return `UpdateCheckResult[]`.

### Task 3: Implement update preview

**Files:**
- Modify: `crates/lab/src/dispatch/marketplace/update.rs`

- [x] Resolve one fork stash dir and return `not_forked` when `.stash.json` is absent.
- [x] Resolve upstream plugin source path and current upstream commit/version.
- [x] For plugin forks, preview all upstream/stash/base artifact paths; for artifact forks, preview `forked_artifacts`.
- [x] Read base snapshots from `.base/<rel_path>`, yours from stash, and theirs from upstream.
- [x] Classify paths into `unchanged`, `upstream_only`, `user_only`, `clean_merges`, and `conflicts`.
- [x] Use `diffy-imara::merge` for 3-way conflict detection and `diffy-imara::create_patch` for diff display.
- [x] Include `upstream_commit` and write the preview to `.pending-update.json`.

### Task 4: Tests and docs

**Files:**
- Modify: `crates/lab/src/dispatch/marketplace/fork_update.rs`
- Modify: `crates/lab/src/dispatch/marketplace/params.rs`
- Modify: `crates/lab/src/dispatch/marketplace/dispatch.rs`
- Modify: `docs/ERRORS.md`
- Modify: `docs/MCP.md`

- [x] Add unit tests for param parsing and catalog routing.
- [x] Add update check tests for up-to-date, outdated, missing git, auth-failure mapping, and hardened git invocation construction.
- [x] Add preview tests for same-line conflicts, non-overlapping clean merges, unchanged files, pending preview persistence, and non-forked errors.
- [x] Document `git_not_available`, `marketplace_auth_required`, `not_forked`, and `stale_preview` in `docs/ERRORS.md`.
- [x] Document request/response shapes for `artifact.update.check` and `artifact.update.preview` in `docs/MCP.md`.

### Task 5: Verification and session report

**Files:**
- Create: `docs/sessions/2026-04-25-lab-iut15-completion.md`

- [x] Run focused tests for marketplace update check/preview.
- [x] Run `cargo test -p lab --all-features`.
- [x] Run `cargo clippy -p lab --all-features -- -D warnings`.
- [x] Gather required session context commands.
- [x] Write the session report with facts, files modified, verification evidence, risks, and closeability status.
