# Settings Completion Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Finish the Lab settings surface so operators can safely inspect and change backed settings, including one master switch for all built-in upstream API integrations.

**Architecture:** Keep `lab-apis` pure and put persistence/runtime policy in the `lab` crate. Use `crates/lab/src/config.rs` for non-secret `[services]` preferences, `crates/lab/src/registry.rs` as the single runtime service gate, and project backend state into the Next.js settings UI instead of duplicating canonical service lists in TypeScript.

**Tech Stack:** Rust 2024, clap, axum, rmcp, serde/toml, cargo-nextest, Next.js App Router, React, TypeScript, Vitest/Testing Library.

---

## Execution Handoff

Implement this in a fresh worktree.

1. Create and enter a new worktree for this implementation.
2. Run `$superpowers:executing-plans` against this plan in the worktree.
3. Create a PR from the worktree branch when implementation is finished.
4. Run `$lavra:lavra-review` in the worktree and address ALL issues from that review in the worktree.
5. Dispatch the `pr-review-toolkit:full-review` agent/skill against the PR and address ALL issues in the worktree.
6. Dispatch the `code_simplifier` agent against all touched code and address ALL issues in the worktree.
7. Execute `$vibin:gh-address-comments` and address ALL comments from the PR in the worktree.

## File Map

- Modify: `crates/lab/src/config.rs` — add typed non-secret settings fields and config validation/default tests.
- Modify: `crates/lab/src/registry.rs` — add built-in upstream API classification and runtime filtering.
- Modify: `crates/lab/src/cli/serve.rs` — apply persistent registry policy before `--services` narrowing.
- Modify: `crates/lab/src/api/router.rs` — keep route mounting tied to the already-filtered runtime registry.
- Modify: `crates/lab/src/dispatch/setup/catalog.rs` — remove duplicate setup action specs and add the `settings.state` and `settings.update` actions.
- Modify: `crates/lab/src/dispatch/setup/dispatch.rs` — implement typed settings/config read/write actions, redacted output, and validation.
- Modify: `apps/gateway-admin/lib/setup/*.ts` — expose any new setup/settings client calls with abort support.
- Modify: `apps/gateway-admin/app/(admin)/settings/core/page.tsx` — finalize write semantics for empty values, secret placeholders, and failures.
- Modify: `apps/gateway-admin/app/(admin)/settings/services/page.tsx` — show classified/disabled service state from backend projection.
- Modify: `apps/gateway-admin/app/(admin)/settings/services/[service]/service-client.tsx` — keep probe/save semantics aligned with committed values.
- Modify: `apps/gateway-admin/app/(admin)/settings/surfaces/page.tsx` — replace placeholder with backed/read-only controls.
- Modify: `apps/gateway-admin/app/(admin)/settings/features/page.tsx` — replace placeholder with real feature controls or backend-projected empty state.
- Modify: `apps/gateway-admin/app/(admin)/settings/extract/page.tsx` — add preview/diff and clear draft/commit wording.
- Modify: `apps/gateway-admin/app/(admin)/settings/advanced/page.tsx` — add redacted effective-config viewer; write only if backend validation/diff exists.
- Modify: `docs/runtime/CONFIG.md`, `docs/surfaces/MCP.md`, generated docs under `docs/generated/` — document settings behavior and refresh generated artifacts.

## Task 1: Reconcile Tracker And Setup Catalog

**Files:**
- Modify: `crates/lab/src/dispatch/setup/catalog.rs`
- Modify: `crates/lab/src/dispatch/setup/dispatch.rs`
- Test: colocated tests in `crates/lab/src/dispatch/setup/catalog.rs` or the existing setup dispatch test module
- Tracker: `lab-8re5.17.1`, `lab-8re5.17.2`

- [ ] Inspect setup catalog duplicates:

```bash
rg -n '"(installed_plugins|services_status|install_plugin|uninstall_plugin)"' crates/lab/src/dispatch/setup
```

Expected: duplicate `ActionSpec` entries in `catalog.rs` for the listed setup actions.

- [ ] Remove duplicate `ActionSpec` entries and keep the canonical action names stable:

```rust
// Keep exactly one ActionSpec for each action name.
// Do not rename installed_plugins, services_status, install_plugin, or uninstall_plugin.
```

- [ ] Add a duplicate-action test:

```rust
#[test]
fn setup_actions_are_unique() {
    let mut seen = std::collections::BTreeSet::new();
    for action in super::ACTIONS {
        assert!(seen.insert(action.name), "duplicate setup action {}", action.name);
    }
}
```

- [ ] Add dispatch/catalog parity coverage for every handled setup action:

```rust
#[test]
fn setup_catalog_covers_dispatch_actions() {
    let names: std::collections::BTreeSet<&str> =
        super::ACTIONS.iter().map(|action| action.name).collect();

    for required in [
        "schema.get",
        "state",
        "draft.set",
        "draft.commit",
        "finalize",
        "installed_plugins",
        "services_status",
        "install_plugin",
        "uninstall_plugin",
    ] {
        assert!(names.contains(required), "missing setup action {required}");
    }
}
```

- [ ] Run focused Rust tests:

```bash
cargo nextest run --manifest-path crates/lab/Cargo.toml --all-features setup_actions_are_unique setup_catalog_covers_dispatch_actions
```

Expected: both tests pass.

- [ ] Update `lab-8re5.17.1` and `lab-8re5.17.2` with evidence comments before moving to new UI work.

## Task 2: Add Built-In Upstream API Classification

**Files:**
- Modify: `crates/lab/src/config.rs`
- Modify: `crates/lab/src/registry.rs`
- Test: `crates/lab/src/registry.rs`
- Tracker: `lab-8re5.17.9`

- [ ] Add a default-enabled non-secret field to `ServicePreferences`:

```rust
/// Per-service preference overrides (non-secret values only).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServicePreferences {
    /// Enable built-in integrations that call external service APIs.
    ///
    /// Default: true. When false, runtime registries keep bootstrap/operator
    /// tools available but remove built-in upstream API integrations.
    #[serde(default = "default_true")]
    pub built_in_upstream_apis_enabled: bool,
    /// Tailscale preferences.
    #[serde(default)]
    pub tailscale: TailscalePreferences,
}

impl Default for ServicePreferences {
    fn default() -> Self {
        Self {
            built_in_upstream_apis_enabled: true,
            tailscale: TailscalePreferences::default(),
        }
    }
}

fn default_true() -> bool {
    true
}
```

- [ ] Add config parsing tests for default and explicit false:

```rust
#[test]
fn service_preferences_default_enable_upstream_apis() {
    let cfg: LabConfig = toml::from_str("").expect("empty config parses");
    assert!(cfg.services.built_in_upstream_apis_enabled);
}

#[test]
fn service_preferences_can_disable_upstream_apis() {
    let cfg: LabConfig = toml::from_str(
        r#"
        [services]
        built_in_upstream_apis_enabled = false
        "#,
    )
    .expect("services config parses");

    assert!(!cfg.services.built_in_upstream_apis_enabled);
}
```

- [ ] Add one Rust-owned classification helper in `registry.rs`:

```rust
#[must_use]
pub fn is_built_in_upstream_api_service(service: &str) -> bool {
    matches!(
        service,
        "radarr"
            | "sonarr"
            | "prowlarr"
            | "plex"
            | "tautulli"
            | "overseerr"
            | "jellyfin"
            | "navidrome"
            | "immich"
            | "sabnzbd"
            | "qbittorrent"
            | "linkding"
            | "memos"
            | "bytestash"
            | "paperless"
            | "freshrss"
            | "tailscale"
            | "arcane"
            | "unraid"
            | "unifi"
            | "dozzle"
            | "scrutiny"
            | "adguard"
            | "glances"
            | "uptime_kuma"
            | "pihole"
            | "gotify"
            | "apprise"
            | "openacp"
            | "openai"
            | "notebooklm"
            | "qdrant"
            | "tei"
            | "neo4j"
    )
}
```

- [ ] Add an explicit non-upstream assertion for bootstrap/local services:

```rust
#[test]
fn bootstrap_services_are_not_built_in_upstream_apis() {
    for service in [
        "gateway",
        "setup",
        "doctor",
        "extract",
        "logs",
        "device",
        "marketplace",
        "acp",
        "stash",
        "deploy",
        "fs",
        "lab_admin",
        "beads",
        "loggifly",
    ] {
        assert!(
            !is_built_in_upstream_api_service(service),
            "{service} must remain available when upstream APIs are disabled"
        );
    }
}
```

- [ ] Run focused tests:

```bash
cargo nextest run --manifest-path crates/lab/Cargo.toml --all-features service_preferences_default_enable_upstream_apis service_preferences_can_disable_upstream_apis bootstrap_services_are_not_built_in_upstream_apis
```

Expected: all focused tests pass.

## Task 3: Filter Runtime Registry From Config

**Files:**
- Modify: `crates/lab/src/registry.rs`
- Modify: `crates/lab/src/cli/serve.rs`
- Test: `crates/lab/src/registry.rs`, `crates/lab/src/cli/serve.rs`
- Tracker: `lab-8re5.17.9`

- [ ] Add a registry filter that preserves all non-upstream services:

```rust
#[must_use]
pub fn filter_built_in_upstream_apis(
    registry: ToolRegistry,
    enabled: bool,
) -> ToolRegistry {
    if enabled {
        return registry;
    }

    let mut filtered = ToolRegistry::new();
    for service in registry.services() {
        if !is_built_in_upstream_api_service(service.name) {
            filtered.register(service.clone());
        }
    }
    filtered
}
```

- [ ] Apply the filter in `serve.rs` before `--services` narrowing:

```rust
let registry = build_default_registry();
let registry = crate::registry::filter_built_in_upstream_apis(
    registry,
    config.services.built_in_upstream_apis_enabled,
);
let registry = filter_registry(registry, &args.services)?;
```

This makes persistent disabled state win by default. `--services radarr` should fail as unknown when upstream APIs are disabled.

- [ ] Add a registry policy test:

```rust
#[test]
fn upstream_api_filter_removes_upstreams_and_keeps_bootstrap() {
    let reg = filter_built_in_upstream_apis(build_default_registry(), false);
    let names: std::collections::BTreeSet<&str> =
        reg.services().iter().map(|service| service.name).collect();

    for removed in ["radarr", "sonarr", "tailscale", "openai"] {
        assert!(!names.contains(removed), "{removed} should be disabled");
    }

    for kept in ["setup", "doctor", "extract", "gateway", "marketplace", "acp", "stash"] {
        assert!(names.contains(kept), "{kept} should stay available");
    }
}
```

- [ ] Add a `--services` precedence test around `filter_registry` if the function remains private:

```rust
#[test]
fn services_allowlist_does_not_reenable_globally_disabled_upstreams() {
    let reg = crate::registry::filter_built_in_upstream_apis(build_default_registry(), false);
    let error = super::filter_registry(reg, &["radarr".to_string()])
        .expect_err("disabled radarr should be unknown to --services");
    assert!(error.to_string().contains("unknown service"));
}
```

- [ ] Run focused registry/serve tests:

```bash
cargo nextest run --manifest-path crates/lab/Cargo.toml --all-features upstream_api_filter_removes_upstreams_and_keeps_bootstrap services_allowlist_does_not_reenable_globally_disabled_upstreams
```

Expected: both tests pass.

## Task 4: Project Settings State Through Setup

**Files:**
- Modify: `crates/lab/src/dispatch/setup/catalog.rs`
- Modify: `crates/lab/src/dispatch/setup/dispatch.rs`
- Modify: `apps/gateway-admin/lib/setup/*.ts`
- Tracker: `lab-8re5.17.4`, `lab-8re5.17.5`, `lab-8re5.17.9`

- [ ] Add setup actions for typed runtime/settings state:

```rust
ActionSpec {
    name: "settings.state",
    description: "Return non-secret operator settings and service runtime policy state",
    destructive: false,
    ..ActionSpec::default()
}

ActionSpec {
    name: "settings.update",
    description: "Update non-secret operator settings with validation",
    destructive: false,
    ..ActionSpec::default()
}
```

- [ ] Return a redacted, typed settings payload:

```json
{
  "services": {
    "built_in_upstream_apis_enabled": true,
    "built_in_upstream_api_services": ["radarr", "sonarr", "openai"],
    "bootstrap_services": ["setup", "doctor", "extract", "gateway"]
  }
}
```

- [ ] Implement `settings.update` as a typed config write with validation and backup, or mark the field read-only until the write path is added. Do not let the frontend write raw TOML.

- [ ] Add TypeScript client wrappers with abort support:

```ts
export async function settingsState(signal?: AbortSignal): Promise<SettingsState> {
  return setupApi.call<SettingsState>("settings.state", {}, signal)
}

export async function settingsUpdate(
  patch: SettingsUpdate,
  signal?: AbortSignal,
): Promise<SettingsState> {
  return setupApi.call<SettingsState>("settings.update", patch, signal)
}
```

- [ ] Add backend tests proving no secret values appear in the settings state payload.

- [ ] Run focused tests:

```bash
cargo nextest run --manifest-path crates/lab/Cargo.toml --all-features settings_state settings_update
pnpm --dir apps/gateway-admin test -- settings
```

Expected: Rust settings action tests pass and gateway-admin settings client tests pass.

## Task 5: Finish Core And Services Write Semantics

**Files:**
- Modify: `apps/gateway-admin/app/(admin)/settings/core/page.tsx`
- Modify: `apps/gateway-admin/app/(admin)/settings/services/[service]/service-client.tsx`
- Modify: `apps/gateway-admin/components/settings/ServiceForm.tsx` if that component owns field semantics
- Tracker: `lab-8re5.17.3`

- [ ] Define and implement empty-value behavior consistently:

```ts
type EmptyValueBehavior = "skip" | "clear"

const emptyValueBehavior: EmptyValueBehavior = "clear"
```

If clear requires a backend remove action, add that action before exposing clearing in the UI. If clear is not supported, render an explicit clear-disabled affordance instead of silently skipping.

- [ ] Prevent placeholder secret writes:

```ts
function shouldWriteField(value: string, secret: boolean): boolean {
  if (secret && (value === "***" || value === "********" || value === "STORED_SECRET_MARKER")) {
    return false
  }
  return true
}
```

- [ ] Preserve edited field values on commit failure and show the error beside the field/form that failed.

- [ ] Ensure every post-await update checks abort state:

```ts
if (signal?.aborted) return
setSaveState({ status: "saved" })
```

- [ ] Align probe semantics: either probe committed live config only and label it that way, or commit draft before probe. Do not imply probes use unsaved values if they do not.

- [ ] Add frontend tests for success, backend failure, abort, secret placeholder skip, and empty-value behavior.

- [ ] Run focused frontend tests:

```bash
pnpm --dir apps/gateway-admin test -- settings/core settings/services
```

Expected: all settings write tests pass.

## Task 6: Replace Placeholder Settings Panels

**Files:**
- Modify: `apps/gateway-admin/app/(admin)/settings/surfaces/page.tsx`
- Modify: `apps/gateway-admin/app/(admin)/settings/features/page.tsx`
- Modify: `apps/gateway-admin/app/(admin)/settings/advanced/page.tsx`
- Tracker: `lab-8re5.17.4`, `lab-8re5.17.5`, `lab-8re5.17.7`

- [ ] For Surfaces, render only controls backed by config/env state. Use read-only rows for current values that cannot persist yet:

```ts
type SettingRow =
  | { kind: "toggle"; key: string; label: string; value: boolean; writable: true }
  | { kind: "readonly"; key: string; label: string; value: string; reason: string }
```

- [ ] For Features, render real backed feature settings only. Put the built-in upstream API master switch here or in Services, but make it one clear home in the Settings nav.

- [ ] For Advanced, start with a redacted effective-config viewer:

```ts
function isSecretKey(key: string): boolean {
  return /token|secret|password|api[_-]?key/i.test(key)
}
```

Never render raw secret values. Show paths and reload/restart notes rather than browser raw-file editing unless backend diff/backup/validation exists.

- [ ] Add tests that the old placeholder copy is gone:

```ts
expect(screen.queryByText(/Coming in v2/i)).not.toBeInTheDocument()
```

- [ ] Add tests that unsupported values render read-only with a reason instead of fake toggles.

- [ ] Run focused frontend tests:

```bash
pnpm --dir apps/gateway-admin test -- settings/surfaces settings/features settings/advanced
```

Expected: placeholder removal and backed/read-only behavior tests pass.

## Task 7: Finish Extract Apply Flow

**Files:**
- Modify: `apps/gateway-admin/app/(admin)/settings/extract/page.tsx`
- Modify: `apps/gateway-admin/lib/setup/*.ts` if additional draft/commit helpers are needed
- Tracker: `lab-8re5.17.6`

- [ ] Add a preview/diff before applying discovered values:

```ts
type ExtractPreviewRow = {
  service: string
  key: string
  current: string | null
  discovered: string
  secret: boolean
  writable: boolean
}
```

- [ ] Make redacted secrets non-writable and route the operator to the service settings form for manual entry.

- [ ] Label actions explicitly:

```ts
const APPLY_TO_DRAFT_LABEL = "Apply selected values to draft"
const APPLY_AND_COMMIT_LABEL = "Apply selected values and commit"
```

- [ ] If adding Apply + Commit, reuse `setup.draft.commit` and preserve audit failure details.

- [ ] Add tests for no results, warnings, selected apply, redacted secret behavior, draft-only status, and commit status.

- [ ] Run focused frontend tests:

```bash
pnpm --dir apps/gateway-admin test -- settings/extract
```

Expected: Extract preview/apply tests pass.

## Task 8: Docs, Generated Artifacts, And Final Verification

**Files:**
- Modify: `docs/runtime/CONFIG.md`
- Modify: `docs/surfaces/MCP.md`
- Modify: `docs/generated/mcp-help.json`
- Modify: `docs/generated/openapi.json`
- Modify: `docs/generated/feature-matrix.md`
- Tracker: `lab-8re5.17.8`

- [ ] Document the new config key:

```toml
[services]
built_in_upstream_apis_enabled = true
```

Include default, disabled behavior, credential preservation, `--services` precedence, and restart/reload expectations.

- [ ] Document that generated/static docs use `build_docs_registry()` and do not change based on local operator config.

- [ ] Refresh generated artifacts using the repo’s existing generation command. Find it with:

```bash
rg -n "generated|mcp-help|openapi|feature-matrix" Justfile docs scripts crates/lab
```

Run the narrow generation command found there, then inspect `git diff docs/generated`.

- [ ] Run focused Rust tests:

```bash
cargo nextest run --manifest-path crates/lab/Cargo.toml --all-features setup registry settings
```

Expected: focused Rust tests pass. If the filter syntax is too broad for nextest, run the exact test names added in Tasks 1-4.

- [ ] Run focused frontend tests:

```bash
pnpm --dir apps/gateway-admin test -- settings
```

Expected: focused settings tests pass.

- [ ] Run the repo default verification for this slice:

```bash
cargo nextest run --workspace --all-features
cargo build --workspace --all-features
```

Expected: all-features tests and build pass. If failures are unrelated pre-existing failures, record exact failing commands, tests, and evidence on `lab-8re5.17` before deciding whether to split follow-up beads.

- [ ] Update Beads:

```bash
bd comment lab-8re5.17 "Verification: <exact commands and outcomes>"
bd comment lab-8re5.17.9 "Implemented built-in upstream API master switch: <files and tests>"
```

Close only beads whose acceptance criteria are satisfied with file/test evidence.

## Self-Review Checklist

- [ ] No settings page shows “Coming in v2” for in-scope MVP behavior.
- [ ] The master upstream API toggle disables all listed upstream services and preserves bootstrap/operator services.
- [ ] The canonical upstream service list exists in Rust and is projected to the UI.
- [ ] Persistent disablement wins over `--services` by default.
- [ ] Stored credentials are preserved and never written back as redacted placeholders.
- [ ] Advanced and setup/settings state responses redact secrets.
- [ ] Generated docs are refreshed or an explicit reason is recorded.
- [ ] Final verification evidence is recorded on `lab-8re5.17`.
