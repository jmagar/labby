# Settings Full Configuration Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make `/settings` a safe schema-backed editor for current env-backed core settings and selected scalar `config.toml` settings, while presenting every complex or dangerous config area read-only with explicit follow-up paths for typed editors.

**Architecture:** Rust owns schema, source precedence, risk classification, validation, redaction, authorization, and persistence. The frontend renders only schema-approved scalar controls; complex TOML sections such as `upstream`, `protected_mcp_routes`, `virtual_servers`, and `deploy` are visible as redacted read-only blocks in this epic, not edited through raw text. Settings reads are section-scoped to avoid shipping the whole config universe to every page, and settings writes use allowlisted operations with backups and admin enforcement.

**Tech Stack:** Rust 2024, serde, toml, toml_edit, fd-lock, tempfile, rmcp dispatch, Axum auth scopes, Next.js App Router, React, TypeScript, Vitest, Testing Library, Playwright or agent-browser smoke verification.

---

## Engineering Review Applied

The Lavra engineering review produced these required changes, all folded into this revised plan:

- Remove editable raw TOML controls from this epic. Complex sections are read-only until domain-specific editors exist.
- Narrow implementation from “everything editable at once” to safe scalar editing plus complete visibility.
- Replace broad dotted TOML mutation with allowlisted scalar operations and explicit unset semantics.
- Preserve current env-backed core fields (`LAB_MCP_HTTP_HOST`, `LAB_MCP_HTTP_PORT`, `LAB_LOG`, `LAB_LOG_FORMAT`) and show env-over-config precedence.
- Do not expose config-backed secret writes in this epic. Secrets are write-only later, never returned as `********` values.
- Add `risk`, `write_policy`, `apply_mode`, `source`, and `overridden_by_env` metadata.
- Require admin scope for settings mutation and do not let the client auto-confirm destructive writes.
- Add recursive redaction tests for nested config.
- Avoid full `doctor.audit.full` for small settings env writes; use a targeted `settings.env.update` path with atomic merge and conflict detection.
- Back up `config.toml`, preserve permissions where possible, fsync parent directory, and return `backup_path`.
- Split schema, section-scoped state, complex read-only state, and env inventory endpoints.
- Add focused tests per task, with full backend/frontend/browser verification at the final gate.

## Current Review Of `/settings`

- `/settings/core` edits only four env vars via `setup.draft.set` plus `setup.draft.commit`.
- `/settings/services` edits registered service env vars from `setup.schema.get`, but not all generated `LAB_*` process knobs.
- `/settings/features` has one real `config.toml` write: `[services].built_in_upstream_apis_enabled`.
- `/settings/surfaces` and `/settings/advanced` are read-only summaries.
- `SettingsRail` still marks Surfaces, Features, and Advanced as `v2` stubs.
- Backend `settings.update` only accepts `services.built_in_upstream_apis_enabled`.
- `config.rs` contains many settings that must be visible in `/settings`, but several are too risky for a generic first editor: `auth`, `upstream`, `protected_mcp_routes`, `virtual_servers`, `deploy`, `gateway.disable_spawn_guard`, and similar sections.

## File Map

- Create: `crates/lab/src/dispatch/setup/settings.rs` — settings schema/state/update logic, risk/source metadata, allowlisted config/env writes, redaction, and tests.
- Modify: `crates/lab/src/dispatch/setup.rs` — expose the new settings module.
- Modify: `crates/lab/src/dispatch/setup/catalog.rs` — add `settings.schema`, `settings.state`, `settings.env.update`, `settings.config.update`, `settings.advanced_state`, and `settings.env_schema`.
- Modify: `crates/lab/src/dispatch/setup/dispatch.rs` — route `settings.*` actions, redact mutation params, enforce admin/destructive metadata, and preserve existing setup actions.
- Modify: `crates/lab/src/api/services/setup.rs` or setup API auth tests — prove settings mutation requires admin.
- Modify: `crates/lab/src/config.rs` — add backup-preserving allowlisted scalar config patch support.
- Modify: `crates/lab/src/config/env_merge.rs` only if a targeted key-only merge helper is needed; prefer reusing existing merge primitives.
- Modify: `apps/gateway-admin/lib/api/setup-client.ts` — type schema/state/update/env-inventory contracts.
- Create: `apps/gateway-admin/lib/settings/schema.ts` — section grouping, value normalization, dirty tracking, source/risk helpers.
- Create: `apps/gateway-admin/components/settings/SettingsScalarField.tsx` — text, URL, number, bool, enum, and string-list field renderer.
- Create: `apps/gateway-admin/components/settings/SettingsScalarSection.tsx` — section renderer with per-field errors, source warnings, and explicit dangerous confirmations.
- Create: `apps/gateway-admin/components/settings/AdvancedReadOnlyBlock.tsx` — redacted read-only complex config display.
- Modify: `apps/gateway-admin/components/settings/SettingsRail.tsx` — remove `v2` badges only after backed tests exist for the section.
- Modify: `apps/gateway-admin/app/(admin)/settings/core/page.tsx` — schema-backed env core settings.
- Modify: `apps/gateway-admin/app/(admin)/settings/surfaces/page.tsx` — safe scalar surface settings plus read-only dangerous controls.
- Modify: `apps/gateway-admin/app/(admin)/settings/features/page.tsx` — safe scalar feature settings.
- Modify: `apps/gateway-admin/app/(admin)/settings/advanced/page.tsx` — redacted read-only complex config and searchable env inventory.
- Modify: `apps/gateway-admin/app/(admin)/settings/services/page.tsx` — preserve service env editing and link to env inventory.
- Modify: `apps/gateway-admin/lib/api/setup-settings.test.ts` — API contract tests.
- Create: `apps/gateway-admin/lib/settings/schema.test.ts` — frontend helper tests.
- Create: `apps/gateway-admin/components/settings/SettingsScalarField.test.tsx` — field interaction tests.
- Create: `apps/gateway-admin/components/settings/SettingsScalarSection.test.tsx` — save partition/error/reset tests.
- Modify: `docs/runtime/CONFIG.md` or create it if absent — document source precedence, write policy, risk tiers, backups, and restart semantics.
- Modify: `docs/superpowers/plans/2026-05-09-settings-completion.md` — mark superseded by this plan.

## Coverage Rules

Every `LabConfig` top-level field must appear in one of these categories:

- **Editable scalar now:** low-risk scalar fields with typed validation and clear apply semantics.
- **Visible read-only now:** complex, nested, dangerous, or secret-bearing config shown redacted with source, risk, and follow-up note.
- **Service env editor now:** existing service credential flow under `/settings/services`.
- **Env inventory now:** generated env reference plus `PluginMeta`, searchable and redacted, not all editable in this epic.

Editable scalar config keys for this epic:

```text
output.format
mcp.transport
mcp.host
mcp.port
mcp.session_ttl_secs
mcp.stateful
mcp.allowed_hosts
log.filter
log.format
local_logs.retention_days
local_logs.max_bytes
local_logs.queue_capacity
local_logs.subscriber_capacity
api.cors_origins
web.assets_dir
workspace.root
mcpregistry.url
public_urls.app
public_urls.mcp_gateway
services.built_in_upstream_apis_enabled
services.tailscale.tailnet
admin.enabled
code_mode.trace_params
code_mode.timeout_ms
code_mode.max_tool_calls
code_mode.max_response_bytes
code_mode.max_response_tokens
code_mode.token_estimate_divisor
code_mode.max_log_entries
code_mode.max_log_bytes
gateway_import_mode
gateway.extra_stdio_commands
upstream_request_timeout_ms
node.controller
node.log_retention_days
node.role
device.master
```

Env-backed editable keys for this epic:

```text
LAB_MCP_HTTP_HOST
LAB_MCP_HTTP_PORT
LAB_LOG
LAB_LOG_FORMAT
```

Read-only in this epic:

```text
auth
oauth.machines
deploy
upstream
upstream_pending
upstream_import_tombstones
protected_mcp_routes
virtual_servers
quarantined_virtual_servers
gateway.disable_spawn_guard
web.disable_auth
auth.* secrets
upstream.oauth secrets
any generated env key outside the four editable core env keys
```

## Task 1: Add Settings Schema With Risk, Source, And Apply Metadata

**Files:**
- Create: `crates/lab/src/dispatch/setup/settings.rs`
- Modify: `crates/lab/src/dispatch/setup.rs`
- Test: `crates/lab/src/dispatch/setup/settings.rs`

- [ ] **Step 1: Expose the module**

Add to `crates/lab/src/dispatch/setup.rs`:

```rust
mod settings;
```

- [ ] **Step 2: Create schema types**

Create `crates/lab/src/dispatch/setup/settings.rs`:

```rust
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum SettingsBackend {
    Env,
    ConfigToml,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum SettingsControl {
    Text,
    Url,
    Bool,
    Number,
    Enum,
    StringList,
    ReadOnly,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum SettingsRisk {
    Low,
    Restart,
    SecuritySensitive,
    Dangerous,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum SettingsWritePolicy {
    Editable,
    ReadOnly,
    DangerousFlowRequired,
    SecretWriteOnlyFuture,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum SettingsApplyMode {
    Immediate,
    Restart,
    Partial,
    ReadOnly,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SettingsOption {
    pub value: &'static str,
    pub label: &'static str,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SettingsFieldSpec {
    pub key: &'static str,
    pub label: &'static str,
    pub description: &'static str,
    pub section: &'static str,
    pub backend: SettingsBackend,
    pub control: SettingsControl,
    pub risk: SettingsRisk,
    pub write_policy: SettingsWritePolicy,
    pub apply_mode: SettingsApplyMode,
    pub secret: bool,
    pub required: bool,
    pub env_override: Option<&'static str>,
    pub min: Option<i64>,
    pub max: Option<i64>,
    pub options: Vec<SettingsOption>,
    pub example: Option<&'static str>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SettingsSectionSpec {
    pub id: &'static str,
    pub label: &'static str,
    pub description: &'static str,
    pub advanced: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SettingsSchemaResponse {
    pub schema_version: u32,
    pub sections: Vec<SettingsSectionSpec>,
    pub fields: Vec<SettingsFieldSpec>,
}

pub const SETTINGS_SCHEMA_VERSION: u32 = 1;
```

- [ ] **Step 3: Add field constructors**

Append:

```rust
fn editable(
    section: &'static str,
    key: &'static str,
    label: &'static str,
    description: &'static str,
    backend: SettingsBackend,
    control: SettingsControl,
    apply_mode: SettingsApplyMode,
    env_override: Option<&'static str>,
    example: Option<&'static str>,
) -> SettingsFieldSpec {
    SettingsFieldSpec {
        key,
        label,
        description,
        section,
        backend,
        control,
        risk: if apply_mode == SettingsApplyMode::Restart {
            SettingsRisk::Restart
        } else {
            SettingsRisk::Low
        },
        write_policy: SettingsWritePolicy::Editable,
        apply_mode,
        secret: false,
        required: false,
        env_override,
        min: None,
        max: None,
        options: Vec::new(),
        example,
    }
}

fn readonly(
    section: &'static str,
    key: &'static str,
    label: &'static str,
    description: &'static str,
    risk: SettingsRisk,
    write_policy: SettingsWritePolicy,
) -> SettingsFieldSpec {
    SettingsFieldSpec {
        key,
        label,
        description,
        section,
        backend: SettingsBackend::ConfigToml,
        control: SettingsControl::ReadOnly,
        risk,
        write_policy,
        apply_mode: SettingsApplyMode::ReadOnly,
        secret: matches!(write_policy, SettingsWritePolicy::SecretWriteOnlyFuture),
        required: false,
        env_override: None,
        min: None,
        max: None,
        options: Vec::new(),
        example: None,
    }
}

fn enum_editable(
    section: &'static str,
    key: &'static str,
    label: &'static str,
    description: &'static str,
    apply_mode: SettingsApplyMode,
    options: Vec<SettingsOption>,
    example: Option<&'static str>,
) -> SettingsFieldSpec {
    let mut field = editable(
        section,
        key,
        label,
        description,
        SettingsBackend::ConfigToml,
        SettingsControl::Enum,
        apply_mode,
        None,
        example,
    );
    field.options = options;
    field
}

fn number_editable(
    section: &'static str,
    key: &'static str,
    label: &'static str,
    description: &'static str,
    apply_mode: SettingsApplyMode,
    min: i64,
    max: i64,
    example: Option<&'static str>,
) -> SettingsFieldSpec {
    let mut field = editable(
        section,
        key,
        label,
        description,
        SettingsBackend::ConfigToml,
        SettingsControl::Number,
        apply_mode,
        None,
        example,
    );
    field.min = Some(min);
    field.max = Some(max);
    field
}
```

- [ ] **Step 4: Add sections and fields**

Append:

```rust
pub fn schema_response() -> SettingsSchemaResponse {
    SettingsSchemaResponse {
        schema_version: SETTINGS_SCHEMA_VERSION,
        sections: vec![
            SettingsSectionSpec { id: "core", label: "Core", description: "Env-backed process defaults and low-risk operator paths.", advanced: false },
            SettingsSectionSpec { id: "surfaces", label: "Surfaces", description: "Safe scalar HTTP, MCP, URL, and CORS settings.", advanced: false },
            SettingsSectionSpec { id: "features", label: "Features", description: "Runtime feature gates with explicit apply semantics.", advanced: false },
            SettingsSectionSpec { id: "services", label: "Services", description: "Service env vars and service preferences.", advanced: false },
            SettingsSectionSpec { id: "advanced", label: "Advanced", description: "Redacted read-only complex config and env inventory.", advanced: true },
        ],
        fields: settings_fields(),
    }
}

pub fn settings_fields() -> Vec<SettingsFieldSpec> {
    let mut fields = vec![
        editable("core", "LAB_MCP_HTTP_HOST", "Bind host", "Environment override for HTTP MCP bind host.", SettingsBackend::Env, SettingsControl::Text, SettingsApplyMode::Restart, None, Some("127.0.0.1")),
        editable("core", "LAB_MCP_HTTP_PORT", "Bind port", "Environment override for HTTP MCP bind port.", SettingsBackend::Env, SettingsControl::Number, SettingsApplyMode::Restart, None, Some("8765")),
        editable("core", "LAB_LOG", "Log filter", "Tracing filter directive.", SettingsBackend::Env, SettingsControl::Text, SettingsApplyMode::Restart, None, Some("labby=info,lab_apis=warn")),
        editable("core", "LAB_LOG_FORMAT", "Log format", "Set json for structured logs.", SettingsBackend::Env, SettingsControl::Enum, SettingsApplyMode::Restart, None, Some("json")),
        editable("core", "output.format", "CLI output format", "Default CLI output format when --json is not supplied.", SettingsBackend::ConfigToml, SettingsControl::Text, SettingsApplyMode::Restart, None, Some("human")),
        editable("core", "workspace.root", "Workspace root", "Root directory used by fs browsing and stash workspaces.", SettingsBackend::ConfigToml, SettingsControl::Text, SettingsApplyMode::Restart, None, Some("~/.labby/stash")),
        editable("core", "mcpregistry.url", "MCP Registry URL", "Upstream MCP Registry base URL.", SettingsBackend::ConfigToml, SettingsControl::Url, SettingsApplyMode::Restart, None, Some("https://registry.modelcontextprotocol.io")),
        enum_editable("surfaces", "mcp.transport", "MCP transport", "Default MCP transport.", SettingsApplyMode::Restart, vec![SettingsOption { value: "http", label: "HTTP" }, SettingsOption { value: "stdio", label: "stdio" }], Some("http")),
        editable("surfaces", "mcp.host", "MCP HTTP host", "TOML default for HTTP MCP host; LAB_MCP_HTTP_HOST overrides it.", SettingsBackend::ConfigToml, SettingsControl::Text, SettingsApplyMode::Restart, Some("LAB_MCP_HTTP_HOST"), Some("127.0.0.1")),
        number_editable("surfaces", "mcp.port", "MCP HTTP port", "TOML default for HTTP MCP port; LAB_MCP_HTTP_PORT overrides it.", SettingsApplyMode::Restart, 1, 65535, Some("8765")),
        number_editable("surfaces", "mcp.session_ttl_secs", "MCP session TTL", "Default session keep-alive TTL in seconds.", SettingsApplyMode::Restart, 1, 86_400, Some("3600")),
        editable("surfaces", "mcp.stateful", "Stateful MCP sessions", "Whether HTTP MCP uses stateful sessions by default.", SettingsBackend::ConfigToml, SettingsControl::Bool, SettingsApplyMode::Restart, None, Some("true")),
        editable("surfaces", "mcp.allowed_hosts", "Allowed hosts", "Additional DNS rebinding allowed hosts.", SettingsBackend::ConfigToml, SettingsControl::StringList, SettingsApplyMode::Restart, None, Some("lab.example.com")),
        editable("surfaces", "api.cors_origins", "CORS origins", "Additional CORS origins. Loopback origins are always included.", SettingsBackend::ConfigToml, SettingsControl::StringList, SettingsApplyMode::Restart, None, Some("https://lab.example.com")),
        editable("surfaces", "web.assets_dir", "Web assets directory", "Path to exported Labby assets served by labby serve.", SettingsBackend::ConfigToml, SettingsControl::Text, SettingsApplyMode::Restart, None, Some("apps/gateway-admin/out")),
        editable("surfaces", "public_urls.app", "Public app URL", "Public Lab UI and OAuth issuer URL.", SettingsBackend::ConfigToml, SettingsControl::Url, SettingsApplyMode::Restart, Some("LAB_PUBLIC_URL"), Some("https://lab.example.com")),
        editable("surfaces", "public_urls.mcp_gateway", "Public MCP gateway URL", "Separate public MCP gateway base URL.", SettingsBackend::ConfigToml, SettingsControl::Url, SettingsApplyMode::Restart, Some("LAB_MCP_GATEWAY_URL"), Some("https://mcp.example.com")),
        editable("features", "services.built_in_upstream_apis_enabled", "Built-in upstream API services", "Enable bundled external API integrations while keeping bootstrap tools online.", SettingsBackend::ConfigToml, SettingsControl::Bool, SettingsApplyMode::Immediate, None, Some("true")),
        enum_editable("features", "gateway_import_mode", "Gateway import mode", "Controls external MCP config discovery on startup.", SettingsApplyMode::Restart, vec![SettingsOption { value: "off", label: "Off" }, SettingsOption { value: "pending", label: "Pending approval" }, SettingsOption { value: "auto", label: "Auto import" }], Some("off")),
        editable("features", "admin.enabled", "Admin tool enabled", "Enable the lab_admin MCP tool.", SettingsBackend::ConfigToml, SettingsControl::Bool, SettingsApplyMode::Restart, Some("LAB_ADMIN_ENABLED"), Some("false")),
        editable("features", "code_mode.trace_params", "Trace Code Mode params", "Include redacted/capped tool params in Code Mode traces.", SettingsBackend::ConfigToml, SettingsControl::Bool, SettingsApplyMode::Partial, None, Some("false")),
        editable("features", "gateway.extra_stdio_commands", "Extra stdio commands", "Additional commands allowed as stdio upstream programs.", SettingsBackend::ConfigToml, SettingsControl::StringList, SettingsApplyMode::Restart, None, Some("labby,runarr")),
        editable("services", "services.tailscale.tailnet", "Tailscale tailnet", "Tailnet name. TAILSCALE_TAILNET overrides this.", SettingsBackend::ConfigToml, SettingsControl::Text, SettingsApplyMode::Restart, Some("TAILSCALE_TAILNET"), Some("-")),
        number_editable("advanced", "upstream_request_timeout_ms", "Upstream request timeout", "Maximum time for one proxied upstream MCP response.", SettingsApplyMode::Restart, 1, 300_000, Some("30000")),
        number_editable("advanced", "local_logs.retention_days", "Log retention days", "Local log retention window.", SettingsApplyMode::Partial, 1, 3650, Some("30")),
        number_editable("advanced", "local_logs.max_bytes", "Max log bytes", "Maximum retained logical bytes.", SettingsApplyMode::Partial, 1, 1_099_511_627_776, Some("1073741824")),
        number_editable("advanced", "local_logs.queue_capacity", "Log queue capacity", "Bounded ingest queue size.", SettingsApplyMode::Restart, 1, 1_000_000, Some("4096")),
        number_editable("advanced", "local_logs.subscriber_capacity", "Subscriber capacity", "Bounded live-subscriber ring size.", SettingsApplyMode::Restart, 1, 1_000_000, Some("1024")),
        editable("advanced", "node.controller", "Node controller", "Controller host for node runtime.", SettingsBackend::ConfigToml, SettingsControl::Text, SettingsApplyMode::Restart, None, Some("node-a")),
        number_editable("advanced", "node.log_retention_days", "Node log retention days", "How many days of node logs to retain.", SettingsApplyMode::Partial, 1, 3650, Some("30")),
        enum_editable("advanced", "node.role", "Node role", "Explicit runtime role for this device.", SettingsApplyMode::Restart, vec![SettingsOption { value: "controller", label: "Controller" }, SettingsOption { value: "node", label: "Node" }], Some("controller")),
        editable("advanced", "device.master", "Legacy device master", "Legacy master host for device runtime.", SettingsBackend::ConfigToml, SettingsControl::Text, SettingsApplyMode::Restart, None, Some("node-a")),
        number_editable("advanced", "code_mode.timeout_ms", "Code Mode timeout", "Maximum wall-clock time for one Code Mode execution.", SettingsApplyMode::Partial, 1, 60_000, Some("30000")),
        number_editable("advanced", "code_mode.max_tool_calls", "Code Mode max tool calls", "Maximum host-brokered tool calls per execution.", SettingsApplyMode::Partial, 1, 10_000, Some("100")),
        number_editable("advanced", "code_mode.max_response_bytes", "Code Mode max response bytes", "Maximum serialized response envelope size.", SettingsApplyMode::Partial, 1024, 1_048_576, Some("1048576")),
        number_editable("advanced", "code_mode.max_response_tokens", "Code Mode max response tokens", "Approximate maximum response tokens.", SettingsApplyMode::Partial, 256, 256_000, Some("64000")),
        number_editable("advanced", "code_mode.token_estimate_divisor", "Token estimate divisor", "Lower values are more conservative.", SettingsApplyMode::Partial, 1, 64, Some("4")),
        number_editable("advanced", "code_mode.max_log_entries", "Code Mode max log entries", "Maximum console log lines captured per execution.", SettingsApplyMode::Partial, 1, 100_000, Some("1000")),
        number_editable("advanced", "code_mode.max_log_bytes", "Code Mode max log bytes", "Maximum console log bytes captured per execution.", SettingsApplyMode::Partial, 1, 104_857_600, Some("1048576")),
    ];
    fields.extend(readonly_fields());
    fields
}

fn readonly_fields() -> Vec<SettingsFieldSpec> {
    vec![
        readonly("surfaces", "web.disable_auth", "Disable web auth", "Auth bypass is visible here but requires a dedicated dangerous settings flow.", SettingsRisk::Dangerous, SettingsWritePolicy::DangerousFlowRequired),
        readonly("surfaces", "auth", "Auth config", "OAuth and bearer auth settings are redacted and read-only in this epic.", SettingsRisk::SecuritySensitive, SettingsWritePolicy::SecretWriteOnlyFuture),
        readonly("features", "code_mode.enabled", "Code Mode enabled", "Enabling the synthetic Code Mode surface requires dedicated runtime exposure tests.", SettingsRisk::SecuritySensitive, SettingsWritePolicy::DangerousFlowRequired),
        readonly("features", "gateway.disable_spawn_guard", "Disable spawn guard", "Disabling stdio command validation requires typed confirmation and rollback instructions.", SettingsRisk::Dangerous, SettingsWritePolicy::DangerousFlowRequired),
        readonly("advanced", "oauth.machines", "OAuth relay machines", "Named OAuth callback relay targets.", SettingsRisk::SecuritySensitive, SettingsWritePolicy::ReadOnly),
        readonly("advanced", "deploy", "Deploy preferences", "Deploy defaults and per-host overrides.", SettingsRisk::SecuritySensitive, SettingsWritePolicy::ReadOnly),
        readonly("advanced", "upstream", "Gateway upstreams", "Upstream MCP servers proxied through Lab.", SettingsRisk::SecuritySensitive, SettingsWritePolicy::ReadOnly),
        readonly("advanced", "upstream_pending", "Pending upstream imports", "Discovered upstreams waiting for approval.", SettingsRisk::SecuritySensitive, SettingsWritePolicy::ReadOnly),
        readonly("advanced", "upstream_import_tombstones", "Import tombstones", "Deleted imports that should not return automatically.", SettingsRisk::Restart, SettingsWritePolicy::ReadOnly),
        readonly("advanced", "protected_mcp_routes", "Protected MCP routes", "OAuth-protected public MCP route definitions.", SettingsRisk::Dangerous, SettingsWritePolicy::ReadOnly),
        readonly("advanced", "virtual_servers", "Virtual servers", "Virtual MCP servers backed by Lab services.", SettingsRisk::Restart, SettingsWritePolicy::ReadOnly),
        readonly("advanced", "quarantined_virtual_servers", "Quarantined virtual servers", "Virtual servers whose backing service is no longer registered.", SettingsRisk::Restart, SettingsWritePolicy::ReadOnly),
    ]
}
```

- [ ] **Step 5: Add schema tests**

Append:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn settings_schema_keys_are_unique() {
        let mut seen = BTreeSet::new();
        for field in settings_fields() {
            assert!(seen.insert(field.key), "duplicate field {}", field.key);
        }
    }

    #[test]
    fn dangerous_and_secret_config_is_not_editable_in_first_slice() {
        let fields = settings_fields();
        for key in [
            "auth",
            "web.disable_auth",
            "gateway.disable_spawn_guard",
            "upstream",
            "protected_mcp_routes",
            "deploy",
        ] {
            let field = fields.iter().find(|field| field.key == key).expect(key);
            assert_ne!(field.write_policy, SettingsWritePolicy::Editable, "{key} must not be scalar-editable");
        }
    }

    #[test]
    fn env_override_metadata_is_present_for_shadowed_toml_fields() {
        let fields = settings_fields();
        assert_eq!(
            fields.iter().find(|field| field.key == "mcp.port").unwrap().env_override,
            Some("LAB_MCP_HTTP_PORT")
        );
        assert_eq!(
            fields.iter().find(|field| field.key == "public_urls.app").unwrap().env_override,
            Some("LAB_PUBLIC_URL")
        );
    }
}
```

- [ ] **Step 6: Run focused tests**

Run:

```bash
cargo nextest run --manifest-path crates/lab/Cargo.toml --all-features settings_schema
```

Expected: schema tests pass.

- [ ] **Step 7: Commit**

Run:

```bash
git add crates/lab/src/dispatch/setup.rs crates/lab/src/dispatch/setup/settings.rs
git commit -m "feat: add safe settings schema metadata"
```

## Task 2: Add Safe Config Patch Primitives

**Files:**
- Modify: `crates/lab/src/config.rs`
- Test: `crates/lab/src/config.rs`

- [ ] **Step 1: Add allowlisted patch operation types**

Near `patch_built_in_upstream_apis_enabled`, add:

```rust
#[derive(Debug, Clone, PartialEq)]
pub enum ConfigScalarValue {
    Bool(bool),
    I64(i64),
    String(String),
    StringList(Vec<String>),
    UnsetOptional,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ConfigScalarPatch {
    pub path: String,
    pub value: ConfigScalarValue,
}

impl ConfigScalarPatch {
    #[must_use]
    pub fn new(path: impl Into<String>, value: ConfigScalarValue) -> Self {
        Self { path: path.into(), value }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct ConfigPatchOutcome {
    pub config: LabConfig,
    pub backup_path: Option<PathBuf>,
}
```

- [ ] **Step 2: Add safe TOML path setter**

Add:

```rust
fn set_toml_scalar_path(
    document: &mut toml_edit::DocumentMut,
    dotted_path: &str,
    value: ConfigScalarValue,
) -> Result<()> {
    let parts: Vec<&str> = dotted_path.split('.').filter(|part| !part.is_empty()).collect();
    anyhow::ensure!(!parts.is_empty(), "config path must not be empty");
    let (leaf, parents) = parts.split_last().expect("non-empty parts");
    let mut item = document.as_item_mut();
    for part in parents {
        if item[part].is_none() {
            item[part] = toml_edit::Item::Table(toml_edit::Table::new());
        } else if !item[part].is_table() {
            anyhow::bail!("config parent `{part}` is not a table");
        }
        item = &mut item[part];
    }
    if matches!(value, ConfigScalarValue::UnsetOptional) {
        if let Some(table) = item.as_table_mut() {
            table.remove(leaf);
            return Ok(());
        }
        anyhow::bail!("config parent for `{dotted_path}` is not a table");
    }
    item[leaf] = toml_edit::Item::Value(match value {
        ConfigScalarValue::Bool(value) => toml_edit::Value::from(value),
        ConfigScalarValue::I64(value) => toml_edit::Value::from(value),
        ConfigScalarValue::String(value) => toml_edit::Value::from(value),
        ConfigScalarValue::StringList(values) => {
            let mut array = toml_edit::Array::default();
            for value in values {
                array.push(value);
            }
            toml_edit::Value::Array(array)
        }
        ConfigScalarValue::UnsetOptional => unreachable!("handled above"),
    });
    Ok(())
}
```

- [ ] **Step 3: Add backup-preserving config patch function**

Add:

```rust
pub fn patch_config_scalars(path: &Path, entries: &[ConfigScalarPatch]) -> Result<ConfigPatchOutcome> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("failed to create {}", parent.display()))?;
    }

    let lock_path = config_lock_path(path);
    let lock_file = OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(false)
        .open(&lock_path)
        .with_context(|| format!("open {}", lock_path.display()))?;
    let mut lock = fd_lock::RwLock::new(lock_file);
    let _guard = lock
        .try_write()
        .with_context(|| format!("config is locked: {}", lock_path.display()))?;

    let raw = match std::fs::read_to_string(path) {
        Ok(raw) => raw,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(e) => return Err(anyhow::Error::new(e).context(format!("failed to read {}", path.display()))),
    };
    let mut document = raw
        .parse::<toml_edit::DocumentMut>()
        .with_context(|| format!("failed to parse {}", path.display()))?;
    for entry in entries {
        set_toml_scalar_path(&mut document, &entry.path, entry.value.clone())
            .with_context(|| format!("failed to patch {}", entry.path))?;
    }

    let patched = document.to_string();
    let mut cfg = toml::from_str::<LabConfig>(&patched)
        .with_context(|| format!("failed to parse patched {}", path.display()))?;
    cfg.normalize_protected_mcp_routes()
        .with_context(|| format!("invalid patched config {}", path.display()))?;
    cfg.validate()
        .with_context(|| format!("invalid patched config {}", path.display()))?;

    let backup_path = if path.exists() {
        Some(backup_config_file(path, &raw)?)
    } else {
        None
    };
    let old_mode = std::fs::metadata(path).ok().map(|metadata| metadata.permissions());
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    let mut tmp = NamedTempFile::new_in(parent)
        .with_context(|| format!("failed to create temp file in {}", parent.display()))?;
    tmp.write_all(patched.as_bytes()).context("failed to write temp config")?;
    tmp.as_file().sync_all().context("failed to sync temp config")?;
    if let Some(mode) = old_mode {
        tmp.as_file().set_permissions(mode).context("failed to preserve config mode")?;
    }
    tmp.persist(path)
        .map_err(|e| anyhow::Error::new(e.error))
        .with_context(|| format!("failed to persist {}", path.display()))?;
    if let Ok(parent_dir) = OpenOptions::new().read(true).open(parent) {
        let _ = parent_dir.sync_all();
    }
    Ok(ConfigPatchOutcome { config: cfg, backup_path })
}

fn backup_config_file(path: &Path, raw: &str) -> Result<PathBuf> {
    let stamp = jiff::Timestamp::now().strftime("%Y%m%d%H%M%S").to_string();
    let backup = path.with_extension(format!("toml.bak.{stamp}"));
    std::fs::write(&backup, raw).with_context(|| format!("write backup {}", backup.display()))?;
    Ok(backup)
}
```

- [ ] **Step 4: Keep existing built-in upstream helper**

Replace `patch_built_in_upstream_apis_enabled` body with:

```rust
pub fn patch_built_in_upstream_apis_enabled(path: &Path, enabled: bool) -> Result<LabConfig> {
    Ok(patch_config_scalars(
        path,
        &[ConfigScalarPatch::new(
            "services.built_in_upstream_apis_enabled",
            ConfigScalarValue::Bool(enabled),
        )],
    )?.config)
}
```

- [ ] **Step 5: Add safety tests**

Add tests:

```rust
#[test]
fn patch_config_scalars_rejects_non_table_parent_without_mutating() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.toml");
    std::fs::write(&path, "mcp = \"bad\"\n").unwrap();
    let err = patch_config_scalars(
        &path,
        &[ConfigScalarPatch::new("mcp.port", ConfigScalarValue::I64(8765))],
    )
    .unwrap_err();
    assert!(err.to_string().contains("not a table"), "unexpected error: {err:#}");
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "mcp = \"bad\"\n");
}

#[test]
fn patch_config_scalars_unsets_optional_instead_of_empty_string() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.toml");
    std::fs::write(&path, "[mcp]\nport = 8765\n").unwrap();
    let outcome = patch_config_scalars(
        &path,
        &[ConfigScalarPatch::new("mcp.port", ConfigScalarValue::UnsetOptional)],
    )
    .unwrap();
    assert_eq!(outcome.config.mcp.port, None);
    assert!(!std::fs::read_to_string(&path).unwrap().contains("port"));
}

#[test]
fn patch_config_scalars_creates_backup_and_preserves_comments() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.toml");
    std::fs::write(&path, "# keep\n[mcp]\nhost = \"127.0.0.1\"\n").unwrap();
    let outcome = patch_config_scalars(
        &path,
        &[ConfigScalarPatch::new("mcp.port", ConfigScalarValue::I64(8765))],
    )
    .unwrap();
    assert!(outcome.backup_path.unwrap().is_file());
    let raw = std::fs::read_to_string(&path).unwrap();
    assert!(raw.contains("# keep"));
    assert!(raw.contains("port = 8765"));
}
```

- [ ] **Step 6: Run focused tests**

Run:

```bash
cargo nextest run --manifest-path crates/lab/Cargo.toml --all-features patch_config_scalars
```

Expected: all new config patch tests pass.

- [ ] **Step 7: Commit**

Run:

```bash
git add crates/lab/src/config.rs
git commit -m "feat: add safe scalar config patching"
```

## Task 3: Implement Settings Dispatch, Admin Gates, Redaction, And Env Updates

**Files:**
- Modify: `crates/lab/src/dispatch/setup/settings.rs`
- Modify: `crates/lab/src/dispatch/setup/catalog.rs`
- Modify: `crates/lab/src/dispatch/setup/dispatch.rs`
- Modify: `crates/lab/src/api/services/setup.rs` or setup API auth tests
- Test: `crates/lab/src/dispatch/setup/settings.rs`

- [ ] **Step 1: Add source/state/update types**

Append to `settings.rs`:

```rust
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum SettingsSourceKind {
    Env,
    ConfigToml,
    Default,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SettingsValueSource {
    pub source: SettingsSourceKind,
    pub overridden_by_env: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SettingsStateResponse {
    pub schema_version: u32,
    pub config_path: String,
    pub env_path: String,
    pub section: String,
    pub values: BTreeMap<String, Value>,
    pub sources: BTreeMap<String, SettingsValueSource>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SettingsUpdateEntry {
    pub key: String,
    pub value: Value,
    #[serde(default)]
    pub unset: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SettingsMutationOutcome {
    pub state: SettingsStateResponse,
    pub backup_path: Option<String>,
}
```

- [ ] **Step 2: Add recursive redaction helper**

Append:

```rust
pub fn redact_value(value: Value) -> Value {
    match value {
        Value::Object(map) => {
            let redacted = map
                .into_iter()
                .map(|(key, value)| {
                    let lower = key.to_ascii_lowercase();
                    let looks_secret = lower.contains("secret")
                        || lower.contains("token")
                        || lower.contains("password")
                        || lower.contains("api_key")
                        || lower.contains("client_secret");
                    if looks_secret {
                        (key, json!({ "has_value": !value.is_null() }))
                    } else {
                        (key, redact_value(value))
                    }
                })
                .collect();
            Value::Object(redacted)
        }
        Value::Array(values) => Value::Array(values.into_iter().map(redact_value).collect()),
        other => other,
    }
}
```

- [ ] **Step 3: Add section-scoped state**

Append a `state_response` implementation that only returns fields for one section:

```rust
pub fn state_response(
    cfg: &crate::config::LabConfig,
    config_path: String,
    env_path: String,
    section: &str,
) -> SettingsStateResponse {
    let mut values = BTreeMap::new();
    let mut sources = BTreeMap::new();
    for field in settings_fields().into_iter().filter(|field| field.section == section) {
        let (value, source) = value_for_field(cfg, &field);
        values.insert(field.key.to_string(), value);
        sources.insert(field.key.to_string(), source);
    }
    SettingsStateResponse {
        schema_version: SETTINGS_SCHEMA_VERSION,
        config_path,
        env_path,
        section: section.to_string(),
        values,
        sources,
    }
}
```

Add `value_for_field` with exact scalar mappings for all editable fields. For read-only complex fields, serialize then call `redact_value`:

```rust
fn value_for_field(
    cfg: &crate::config::LabConfig,
    field: &SettingsFieldSpec,
) -> (Value, SettingsValueSource) {
    if field.backend == SettingsBackend::Env {
        let raw = std::env::var(field.key).ok();
        return (
            serde_json::to_value(raw).unwrap_or(Value::Null),
            SettingsValueSource { source: SettingsSourceKind::Env, overridden_by_env: None },
        );
    }
    let source = field
        .env_override
        .and_then(|name| std::env::var(name).ok().map(|_| name.to_string()));
    let source_kind = if source.is_some() {
        SettingsSourceKind::Env
    } else {
        SettingsSourceKind::ConfigToml
    };
    let source_meta = SettingsValueSource { source: source_kind, overridden_by_env: source };
    let value = match field.key {
        "mcp.port" => json!(cfg.mcp.port),
        "mcp.host" => json!(cfg.mcp.host),
        "mcp.transport" => json!(cfg.mcp.transport),
        "services.built_in_upstream_apis_enabled" => json!(cfg.services.built_in_upstream_apis_enabled),
        "gateway.extra_stdio_commands" => json!(cfg.gateway.extra_stdio_commands),
        "auth" => redact_value(serde_json::to_value(&cfg.auth).unwrap_or(Value::Null)),
        "upstream" => redact_value(serde_json::to_value(&cfg.upstream).unwrap_or(Value::Null)),
        "protected_mcp_routes" => redact_value(serde_json::to_value(&cfg.protected_mcp_routes).unwrap_or(Value::Null)),
        "deploy" => redact_value(serde_json::to_value(&cfg.deploy).unwrap_or(Value::Null)),
        _ => Value::Null,
    };
    (value, source_meta)
}
```

Continue the `match` until every editable key in the coverage list has an explicit mapping. Do not use a generic serde path walker.

- [ ] **Step 4: Add allowlisted config update parsing**

Append:

```rust
pub fn config_patches_from_entries(entries: &[SettingsUpdateEntry]) -> Result<Vec<crate::config::ConfigScalarPatch>, ToolError> {
    let fields: BTreeMap<&str, SettingsFieldSpec> = settings_fields()
        .into_iter()
        .map(|field| (field.key, field))
        .collect();
    let mut patches = Vec::new();
    for entry in entries {
        let Some(field) = fields.get(entry.key.as_str()) else {
            return Err(ToolError::InvalidParam { message: format!("unknown setting `{}`", entry.key), param: entry.key.clone() });
        };
        if field.backend != SettingsBackend::ConfigToml || field.write_policy != SettingsWritePolicy::Editable {
            return Err(ToolError::InvalidParam { message: format!("setting `{}` is not editable through settings.config.update", entry.key), param: entry.key.clone() });
        }
        if field.secret {
            return Err(ToolError::InvalidParam { message: "secret config writes are not supported by this settings slice".into(), param: entry.key.clone() });
        }
        patches.push(config_patch_for_field(field, entry)?);
    }
    Ok(patches)
}
```

Add `config_patch_for_field` with explicit control conversion:

```rust
fn config_patch_for_field(
    field: &SettingsFieldSpec,
    entry: &SettingsUpdateEntry,
) -> Result<crate::config::ConfigScalarPatch, ToolError> {
    use crate::config::{ConfigScalarPatch, ConfigScalarValue};
    if entry.unset {
        return Ok(ConfigScalarPatch::new(field.key, ConfigScalarValue::UnsetOptional));
    }
    let value = match field.control {
        SettingsControl::Bool => ConfigScalarValue::Bool(entry.value.as_bool().ok_or_else(|| invalid_field(field, "must be boolean"))?),
        SettingsControl::Number => {
            let raw = entry.value.as_i64().ok_or_else(|| invalid_field(field, "must be an integer"))?;
            if let Some(min) = field.min
                && raw < min
            {
                return Err(invalid_field(field, "below minimum"));
            }
            if let Some(max) = field.max
                && raw > max
            {
                return Err(invalid_field(field, "above maximum"));
            }
            ConfigScalarValue::I64(raw)
        }
        SettingsControl::Text | SettingsControl::Url | SettingsControl::Enum => {
            let raw = entry.value.as_str().ok_or_else(|| invalid_field(field, "must be a string"))?.trim().to_string();
            validate_string_field(field, &raw)?;
            ConfigScalarValue::String(raw)
        }
        SettingsControl::StringList => {
            let values = entry.value.as_array().ok_or_else(|| invalid_field(field, "must be an array"))?
                .iter()
                .map(|value| value.as_str().map(str::trim).filter(|s| !s.is_empty()).map(str::to_string))
                .collect::<Option<Vec<String>>>()
                .ok_or_else(|| invalid_field(field, "must be an array of strings"))?;
            ConfigScalarValue::StringList(values)
        }
        SettingsControl::ReadOnly => return Err(invalid_field(field, "is read-only")),
    };
    Ok(ConfigScalarPatch::new(field.key, value))
}

fn invalid_field(field: &SettingsFieldSpec, message: &'static str) -> ToolError {
    ToolError::InvalidParam {
        message: format!("{} {message}", field.key),
        param: field.key.to_string(),
    }
}
```

Add `validate_string_field`:

```rust
fn validate_string_field(field: &SettingsFieldSpec, value: &str) -> Result<(), ToolError> {
    if field.control == SettingsControl::Url
        && !value.is_empty()
        && !(value.starts_with("http://") || value.starts_with("https://"))
    {
        return Err(invalid_field(field, "must start with http:// or https://"));
    }
    if field.control == SettingsControl::Enum
        && !field.options.iter().any(|option| option.value == value)
    {
        return Err(invalid_field(field, "must be one of the allowed values"));
    }
    Ok(())
}
```

- [ ] **Step 5: Add targeted env update parsing**

Append:

```rust
pub fn env_entries_from_updates(entries: &[SettingsUpdateEntry]) -> Result<Vec<lab_apis::setup::DraftEntry>, ToolError> {
    let fields: BTreeMap<&str, SettingsFieldSpec> = settings_fields()
        .into_iter()
        .map(|field| (field.key, field))
        .collect();
    let mut out = Vec::new();
    for entry in entries {
        let Some(field) = fields.get(entry.key.as_str()) else {
            return Err(ToolError::InvalidParam { message: format!("unknown setting `{}`", entry.key), param: entry.key.clone() });
        };
        if field.backend != SettingsBackend::Env || field.write_policy != SettingsWritePolicy::Editable {
            return Err(ToolError::InvalidParam { message: format!("setting `{}` is not editable through settings.env.update", entry.key), param: entry.key.clone() });
        }
        let value = match field.control {
            SettingsControl::Number => entry.value.as_i64().ok_or_else(|| invalid_field(field, "must be an integer"))?.to_string(),
            SettingsControl::Enum | SettingsControl::Text | SettingsControl::Url => entry.value.as_str().ok_or_else(|| invalid_field(field, "must be a string"))?.to_string(),
            _ => return Err(invalid_field(field, "has unsupported env control")),
        };
        out.push(lab_apis::setup::DraftEntry { key: entry.key.clone(), value });
    }
    Ok(out)
}
```

- [ ] **Step 6: Route new settings actions**

In `dispatch.rs`, add `settings.update` to redaction until the old name is removed:

```rust
const REDACTED_LOG_ACTIONS: &[&str] = &[
    "draft.set",
    "draft.commit",
    "finalize",
    "settings.env.update",
    "settings.config.update",
];
```

Add match arms:

```rust
"settings.schema" => to_json(super::settings::schema_response()),
"settings.state" => settings_state_action(params),
"settings.env.update" => settings_env_update_action(params).await,
"settings.config.update" => settings_config_update_action(params),
"settings.advanced_state" => settings_advanced_state_action(params),
"settings.env_schema" => settings_env_schema_action(),
```

Implement:

```rust
fn requested_section(params: &Value) -> Result<String, ToolError> {
    Ok(params
        .get("section")
        .and_then(Value::as_str)
        .unwrap_or("core")
        .to_string())
}

fn parse_update_entries(params: &Value) -> Result<Vec<super::settings::SettingsUpdateEntry>, ToolError> {
    serde_json::from_value(params.get("entries").cloned().unwrap_or(Value::Null)).map_err(|_| {
        ToolError::InvalidParam {
            message: "entries must be an array of settings updates".into(),
            param: "entries".into(),
        }
    })
}

fn settings_state_action(params: &Value) -> Result<Value, ToolError> {
    let section = requested_section(params)?;
    let path = config_toml_path().ok_or_else(|| ToolError::Sdk {
        sdk_kind: "internal_error".into(),
        message: "HOME env var not set; cannot resolve config.toml path".into(),
    })?;
    let cfg = load_settings_config(&path)?;
    to_json(super::settings::state_response(&cfg, path.display().to_string(), env_path().display().to_string(), &section))
}

async fn settings_env_update_action(params: &Value) -> Result<Value, ToolError> {
    let entries = parse_update_entries(params)?;
    let env_entries = super::settings::env_entries_from_updates(&entries)?;
    let env = env_path();
    let expected_mtime = snapshot_mtime(&env);
    let outcome = env_merge::merge(
        &env,
        MergeRequest {
            entries: env_entries.into_iter().map(|entry| EnvEntry::new(entry.key, entry.value)).collect(),
            force: false,
            expected_mtime,
        },
    )
    .map_err(map_merge_err)?;
    tracing::info!(surface = "dispatch", service = "setup", action = "settings.env.update.success", written = outcome.written, "settings env update success");
    settings_state_action(params)
}

fn settings_config_update_action(params: &Value) -> Result<Value, ToolError> {
    let entries = parse_update_entries(params)?;
    let patches = super::settings::config_patches_from_entries(&entries)?;
    let path = config_toml_path().ok_or_else(|| ToolError::Sdk {
        sdk_kind: "internal_error".into(),
        message: "HOME env var not set; cannot resolve config.toml path".into(),
    })?;
    let outcome = crate::config::patch_config_scalars(&path, &patches).map_err(config_io_error)?;
    if patches.iter().any(|patch| patch.path == "services.built_in_upstream_apis_enabled") {
        crate::registry::set_runtime_built_in_upstream_apis_enabled(outcome.config.services.built_in_upstream_apis_enabled);
        if let Some(manager) = current_gateway_manager() {
            manager.set_builtin_service_registry(crate::registry::filter_built_in_upstream_apis(
                crate::registry::build_default_registry(),
                outcome.config.services.built_in_upstream_apis_enabled,
            ));
        }
    }
    to_json(super::settings::SettingsMutationOutcome {
        state: super::settings::state_response(&outcome.config, path.display().to_string(), env_path().display().to_string(), requested_section(params)?.as_str()),
        backup_path: outcome.backup_path.map(|path| path.display().to_string()),
    })
}
```

- [ ] **Step 7: Update catalog with admin requirements**

In `catalog.rs`, add:

```rust
ActionSpec {
    name: "settings.config.update",
    description: "Admin-only scalar config.toml settings update",
    destructive: true,
    requires_admin: true,
    returns: "SettingsMutationOutcome",
    params: &[ParamSpec {
        name: "entries",
        ty: "SettingsUpdateEntry[]",
        required: true,
        description: "Schema-approved config scalar updates",
    }],
},
ActionSpec {
    name: "settings.env.update",
    description: "Admin-only targeted .env settings update for known low-risk LAB_* keys",
    destructive: true,
    requires_admin: true,
    returns: "SettingsState",
    params: &[ParamSpec {
        name: "entries",
        ty: "SettingsUpdateEntry[]",
        required: true,
        description: "Schema-approved env scalar updates",
    }],
},
```

Also add `settings.schema`, `settings.state`, `settings.advanced_state`, and `settings.env_schema` as non-destructive read actions.

- [ ] **Step 8: Add admin-scope test**

Add or update an API/MCP auth test mirroring existing admin-scope coverage:

```rust
#[test]
fn setup_settings_mutations_require_admin_scope() {
    let service = crate::registry::build_default_registry()
        .services()
        .iter()
        .find(|service| service.name == "setup")
        .expect("setup service");
    for action in ["settings.config.update", "settings.env.update"] {
        let spec = service.actions.iter().find(|spec| spec.name == action).expect(action);
        assert!(spec.requires_admin, "{action} must require admin");
        assert!(spec.destructive, "{action} must be destructive");
    }
}
```

- [ ] **Step 9: Add redaction/security tests**

Add tests:

```rust
#[test]
fn redaction_removes_nested_secret_values() {
    let raw = json!({
        "oauth": { "client_secret": "super-secret" },
        "nested": [{ "api_key": "abc123" }],
        "safe": "visible"
    });
    let redacted = redact_value(raw);
    let serialized = serde_json::to_string(&redacted).unwrap();
    assert!(!serialized.contains("super-secret"));
    assert!(!serialized.contains("abc123"));
    assert!(serialized.contains("visible"));
}

#[test]
fn config_update_rejects_readonly_and_secret_settings() {
    let entries = vec![SettingsUpdateEntry { key: "auth".into(), value: json!("********"), unset: false }];
    let err = config_patches_from_entries(&entries).unwrap_err();
    assert_eq!(err.kind(), "invalid_param");
}

#[test]
fn env_update_accepts_only_allowlisted_core_env_keys() {
    let entries = vec![SettingsUpdateEntry { key: "LAB_MCP_HTTP_PORT".into(), value: json!(8766), unset: false }];
    let parsed = env_entries_from_updates(&entries).unwrap();
    assert_eq!(parsed[0].key, "LAB_MCP_HTTP_PORT");
    assert_eq!(parsed[0].value, "8766");

    let rejected = vec![SettingsUpdateEntry { key: "LAB_MCP_HTTP_TOKEN".into(), value: json!("secret"), unset: false }];
    assert!(env_entries_from_updates(&rejected).is_err());
}
```

- [ ] **Step 10: Run focused tests**

Run:

```bash
cargo nextest run --manifest-path crates/lab/Cargo.toml --all-features settings
```

Expected: settings dispatch/schema/redaction tests pass.

- [ ] **Step 11: Commit**

Run:

```bash
git add crates/lab/src/dispatch/setup/settings.rs crates/lab/src/dispatch/setup/catalog.rs crates/lab/src/dispatch/setup/dispatch.rs crates/lab/src/api/services/setup.rs
git commit -m "feat: add safe settings dispatch actions"
```

## Task 4: Add Complete Env Inventory Endpoint

**Files:**
- Modify: `crates/lab/src/dispatch/setup/settings.rs`
- Modify: `crates/lab/src/dispatch/setup/client.rs`
- Test: `crates/lab/src/dispatch/setup/settings.rs`

- [ ] **Step 1: Expose cached registry to setup siblings**

In `crates/lab/src/dispatch/setup/client.rs`, make the helper visible inside `setup`:

```rust
pub(super) fn cached_registry() -> &'static ToolRegistry {
    REGISTRY.get_or_init(crate::registry::build_default_registry)
}
```

- [ ] **Step 2: Add env inventory types**

Append to `settings.rs`:

```rust
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct EnvSettingSpec {
    pub service: String,
    pub key: String,
    pub required: bool,
    pub secret: bool,
    pub description: String,
    pub example: String,
    pub editable: bool,
}
```

- [ ] **Step 3: Merge generated env reference and PluginMeta**

Append:

```rust
pub fn env_schema() -> Vec<EnvSettingSpec> {
    let mut by_key: BTreeMap<String, EnvSettingSpec> = BTreeMap::new();
    let generated: Value = serde_json::from_str(include_str!("../../../../../docs/generated/env-reference.json"))
        .unwrap_or_else(|_| Value::Array(Vec::new()));
    if let Value::Array(entries) = generated {
        for entry in entries {
            let Some(key) = entry.get("env_var").and_then(Value::as_str) else { continue };
            by_key.insert(key.to_string(), EnvSettingSpec {
                service: entry.get("service").and_then(Value::as_str).unwrap_or("lab").to_string(),
                key: key.to_string(),
                required: entry.get("required").and_then(Value::as_bool).unwrap_or(false),
                secret: entry.get("secret").and_then(Value::as_bool).unwrap_or(false),
                description: entry.get("description").and_then(Value::as_str).unwrap_or("").to_string(),
                example: entry.get("example").and_then(Value::as_str).unwrap_or("").to_string(),
                editable: matches!(key, "LAB_MCP_HTTP_HOST" | "LAB_MCP_HTTP_PORT" | "LAB_LOG" | "LAB_LOG_FORMAT"),
            });
        }
    }
    for entry in super::client::cached_registry().services() {
        if let Some(meta) = crate::registry::service_meta(entry.name) {
            for (required, vars) in [(true, meta.required_env), (false, meta.optional_env)] {
                for var in vars {
                    by_key
                        .entry(var.name.to_string())
                        .and_modify(|existing| {
                            existing.secret |= var.secret;
                            existing.required |= required;
                        })
                        .or_insert_with(|| EnvSettingSpec {
                            service: entry.name.to_string(),
                            key: var.name.to_string(),
                            required,
                            secret: var.secret,
                            description: var.description.to_string(),
                            example: var.example.to_string(),
                            editable: false,
                        });
                }
            }
        }
    }
    by_key.into_values().collect()
}
```

- [ ] **Step 4: Add env endpoint dispatch**

Ensure `settings.env_schema` returns:

```rust
to_json(super::settings::env_schema())
```

- [ ] **Step 5: Add tests for generated and plugin env coverage**

Add:

```rust
#[test]
fn env_schema_merges_generated_reference_and_plugin_meta() {
    let specs = env_schema();
    for key in ["LAB_ACP_DB", "LAB_PUBLIC_URL", "LAB_MCP_HTTP_TOKEN"] {
        assert!(specs.iter().any(|spec| spec.key == key), "missing {key}");
    }
    let token = specs.iter().find(|spec| spec.key == "LAB_MCP_HTTP_TOKEN").unwrap();
    assert!(token.secret, "token must be secret");
}

#[test]
fn env_schema_only_marks_low_risk_core_env_editable() {
    let specs = env_schema();
    assert!(specs.iter().find(|spec| spec.key == "LAB_LOG").unwrap().editable);
    assert!(!specs.iter().find(|spec| spec.key == "LAB_MCP_HTTP_TOKEN").unwrap().editable);
}
```

- [ ] **Step 6: Run focused tests**

Run:

```bash
cargo nextest run --manifest-path crates/lab/Cargo.toml --all-features env_schema
```

Expected: env schema tests pass.

- [ ] **Step 7: Commit**

Run:

```bash
git add crates/lab/src/dispatch/setup/client.rs crates/lab/src/dispatch/setup/settings.rs crates/lab/src/dispatch/setup/dispatch.rs crates/lab/src/dispatch/setup/catalog.rs
git commit -m "feat: expose settings env inventory"
```

## Task 5: Update Frontend Settings API Contracts

**Files:**
- Modify: `apps/gateway-admin/lib/api/setup-client.ts`
- Modify: `apps/gateway-admin/lib/api/setup-settings.test.ts`

- [ ] **Step 1: Replace narrow settings types**

In `setup-client.ts`, define:

```ts
export type SettingsBackend = 'env' | 'config_toml'
export type SettingsControl = 'text' | 'url' | 'bool' | 'number' | 'enum' | 'string_list' | 'read_only'
export type SettingsRisk = 'low' | 'restart' | 'security_sensitive' | 'dangerous'
export type SettingsWritePolicy = 'editable' | 'read_only' | 'dangerous_flow_required' | 'secret_write_only_future'
export type SettingsApplyMode = 'immediate' | 'restart' | 'partial' | 'read_only'
export type SettingsSourceKind = 'env' | 'config_toml' | 'default'

export interface SettingsOption {
  value: string
  label: string
}

export interface SettingsFieldSpec {
  key: string
  label: string
  description: string
  section: string
  backend: SettingsBackend
  control: SettingsControl
  risk: SettingsRisk
  write_policy: SettingsWritePolicy
  apply_mode: SettingsApplyMode
  secret: boolean
  required: boolean
  env_override: string | null
  min: number | null
  max: number | null
  options: SettingsOption[]
  example: string | null
}

export interface SettingsSectionSpec {
  id: string
  label: string
  description: string
  advanced: boolean
}

export interface SettingsSchemaResponse {
  schema_version: number
  sections: SettingsSectionSpec[]
  fields: SettingsFieldSpec[]
}

export interface SettingsValueSource {
  source: SettingsSourceKind
  overridden_by_env: string | null
}

export interface SettingsState {
  schema_version: number
  config_path: string
  env_path: string
  section: string
  values: Record<string, unknown>
  sources: Record<string, SettingsValueSource>
}

export interface SettingsUpdateEntry {
  key: string
  value: unknown
  unset?: boolean
}

export interface SettingsMutationOutcome {
  state: SettingsState
  backup_path: string | null
}

export interface EnvSettingSpec {
  service: string
  key: string
  required: boolean
  secret: boolean
  description: string
  example: string
  editable: boolean
}
```

- [ ] **Step 2: Add section-scoped client methods**

In `setupApi`, add:

```ts
settingsSchema(signal?: AbortSignal): Promise<SettingsSchemaResponse> {
  if (USE_MOCK_DATA) {
    signal?.throwIfAborted?.()
    return Promise.resolve(structuredClone(MOCK_SETTINGS_SCHEMA))
  }
  return setupAction<SettingsSchemaResponse>('settings.schema', {}, signal)
},

settingsState(section = 'core', signal?: AbortSignal): Promise<SettingsState> {
  if (USE_MOCK_DATA) {
    signal?.throwIfAborted?.()
    return Promise.resolve(mockSettingsState(section))
  }
  return setupAction<SettingsState>('settings.state', { section }, signal)
},

settingsConfigUpdate(section: string, entries: SettingsUpdateEntry[], signal?: AbortSignal): Promise<SettingsMutationOutcome> {
  if (USE_MOCK_DATA) {
    signal?.throwIfAborted?.()
    return Promise.resolve({ state: mockSettingsState(section, entries), backup_path: '~/.config/labby/config.toml.bak.mock' })
  }
  return setupAction<SettingsMutationOutcome>('settings.config.update', { section, entries }, signal)
},

settingsEnvUpdate(section: string, entries: SettingsUpdateEntry[], signal?: AbortSignal): Promise<SettingsState> {
  if (USE_MOCK_DATA) {
    signal?.throwIfAborted?.()
    return Promise.resolve(mockSettingsState(section, entries))
  }
  return setupAction<SettingsState>('settings.env.update', { section, entries }, signal)
},

settingsEnvSchema(signal?: AbortSignal): Promise<EnvSettingSpec[]> {
  if (USE_MOCK_DATA) {
    signal?.throwIfAborted?.()
    return Promise.resolve(structuredClone(MOCK_ENV_SCHEMA))
  }
  return setupAction<EnvSettingSpec[]>('settings.env_schema', {}, signal)
},
```

- [ ] **Step 3: Add representative mocks**

Create `MOCK_SETTINGS_SCHEMA`, `mockSettingsState`, and `MOCK_ENV_SCHEMA` in `setup-client.ts`:

```ts
export const MOCK_SETTINGS_SCHEMA: SettingsSchemaResponse = {
  schema_version: 1,
  sections: [
    { id: 'core', label: 'Core', description: 'Env-backed process defaults.', advanced: false },
    { id: 'features', label: 'Features', description: 'Runtime feature gates.', advanced: false },
  ],
  fields: [
    { key: 'LAB_LOG', label: 'Log filter', description: 'Tracing filter directive.', section: 'core', backend: 'env', control: 'text', risk: 'restart', write_policy: 'editable', apply_mode: 'restart', secret: false, required: false, env_override: null, min: null, max: null, options: [], example: 'labby=info' },
    { key: 'services.built_in_upstream_apis_enabled', label: 'Built-in upstream API services', description: 'Enable bundled external API integrations.', section: 'features', backend: 'config_toml', control: 'bool', risk: 'low', write_policy: 'editable', apply_mode: 'immediate', secret: false, required: false, env_override: null, min: null, max: null, options: [], example: 'true' },
  ],
}

export const MOCK_ENV_SCHEMA: EnvSettingSpec[] = [
  { service: 'lab', key: 'LAB_LOG', required: false, secret: false, description: 'Tracing filter directive.', example: 'labby=info', editable: true },
  { service: 'setup', key: 'LAB_MCP_HTTP_TOKEN', required: true, secret: true, description: 'Bearer token.', example: '<token>', editable: false },
]

function mockSettingsState(section: string, updates: SettingsUpdateEntry[] = []): SettingsState {
  const values: Record<string, unknown> = {
    LAB_LOG: 'labby=info,lab_apis=warn',
    'services.built_in_upstream_apis_enabled': true,
  }
  for (const update of updates) values[update.key] = update.value
  return {
    schema_version: 1,
    config_path: '~/.config/labby/config.toml',
    env_path: '~/.labby/.env',
    section,
    values,
    sources: {
      LAB_LOG: { source: 'env', overridden_by_env: null },
      'services.built_in_upstream_apis_enabled': { source: 'config_toml', overridden_by_env: null },
    },
  }
}
```

- [ ] **Step 4: Update API contract tests**

In `setup-settings.test.ts`, assert:

```ts
test('settings schema carries risk source and write policy metadata', () => {
  const schema = MOCK_SETTINGS_SCHEMA
  const field = schema.fields.find((item) => item.key === 'services.built_in_upstream_apis_enabled')
  assert.equal(field?.write_policy, 'editable')
  assert.equal(field?.apply_mode, 'immediate')
})

test('env schema marks token secret and not editable', () => {
  const token = MOCK_ENV_SCHEMA.find((item) => item.key === 'LAB_MCP_HTTP_TOKEN')
  assert.equal(token?.secret, true)
  assert.equal(token?.editable, false)
})
```

- [ ] **Step 5: Run tests**

Run:

```bash
npx vitest run apps/gateway-admin/lib/api/setup-settings.test.ts
```

Expected: setup settings client tests pass.

- [ ] **Step 6: Commit**

Run:

```bash
git add apps/gateway-admin/lib/api/setup-client.ts apps/gateway-admin/lib/api/setup-settings.test.ts
git commit -m "feat: type safe settings api contracts"
```

## Task 6: Build Scalar Settings UI Components

**Files:**
- Create: `apps/gateway-admin/lib/settings/schema.ts`
- Create: `apps/gateway-admin/lib/settings/schema.test.ts`
- Create: `apps/gateway-admin/components/settings/SettingsScalarField.tsx`
- Create: `apps/gateway-admin/components/settings/SettingsScalarField.test.tsx`
- Create: `apps/gateway-admin/components/settings/SettingsScalarSection.tsx`
- Create: `apps/gateway-admin/components/settings/SettingsScalarSection.test.tsx`
- Create: `apps/gateway-admin/components/settings/AdvancedReadOnlyBlock.tsx`

- [ ] **Step 1: Add helper functions without whole-object stringify loops**

Create `apps/gateway-admin/lib/settings/schema.ts`:

```ts
import type { SettingsFieldSpec, SettingsState, SettingsUpdateEntry } from '@/lib/api/setup-client'

export function fieldsForSection(schemaFields: SettingsFieldSpec[], section: string): SettingsFieldSpec[] {
  return schemaFields
    .filter((field) => field.section === section)
    .sort((a, b) => a.label.localeCompare(b.label))
}

export function editableFields(fields: SettingsFieldSpec[]): SettingsFieldSpec[] {
  return fields.filter((field) => field.write_policy === 'editable' && field.control !== 'read_only')
}

export function valueAsInputString(value: unknown): string {
  if (value === null || value === undefined) return ''
  if (Array.isArray(value)) return value.join('\n')
  return String(value)
}

export function parseFieldInput(field: SettingsFieldSpec, raw: string | boolean): unknown {
  if (field.control === 'bool') return Boolean(raw)
  const text = String(raw)
  if (field.control === 'number') {
    if (text.trim() === '') return null
    const parsed = Number(text)
    return Number.isFinite(parsed) ? parsed : null
  }
  if (field.control === 'string_list') {
    return text.split(/\r?\n|,/).map((entry) => entry.trim()).filter(Boolean)
  }
  return text
}

export function buildDirtyEntries(
  fields: SettingsFieldSpec[],
  changedKeys: Set<string>,
  values: Record<string, unknown>,
): SettingsUpdateEntry[] {
  return fields
    .filter((field) => changedKeys.has(field.key))
    .map((field) => ({ key: field.key, value: values[field.key] ?? null }))
}

export function hasEnvOverrideWarning(field: SettingsFieldSpec, state: SettingsState): boolean {
  return Boolean(state.sources[field.key]?.overridden_by_env)
}
```

- [ ] **Step 2: Add helper tests**

Create `apps/gateway-admin/lib/settings/schema.test.ts`:

```ts
import { describe, expect, it } from 'vitest'
import { buildDirtyEntries, parseFieldInput, valueAsInputString } from './schema'
import type { SettingsFieldSpec } from '@/lib/api/setup-client'

const numberField: SettingsFieldSpec = {
  key: 'mcp.port',
  label: 'Port',
  description: '',
  section: 'surfaces',
  backend: 'config_toml',
  control: 'number',
  risk: 'restart',
  write_policy: 'editable',
  apply_mode: 'restart',
  secret: false,
  required: false,
  env_override: 'LAB_MCP_HTTP_PORT',
  min: 1,
  max: 65535,
  options: [],
  example: '8765',
}

describe('settings schema helpers', () => {
  it('parses scalar controls', () => {
    expect(parseFieldInput(numberField, '8765')).toBe(8765)
    expect(parseFieldInput({ ...numberField, control: 'bool' }, true)).toBe(true)
    expect(parseFieldInput({ ...numberField, control: 'string_list' }, 'a,b\nc')).toEqual(['a', 'b', 'c'])
  })

  it('builds dirty entries only for changed keys', () => {
    expect(buildDirtyEntries([numberField], new Set(['mcp.port']), { 'mcp.port': 8766 })).toEqual([
      { key: 'mcp.port', value: 8766 },
    ])
  })

  it('does not stringify objects for scalar inputs', () => {
    expect(valueAsInputString(['a', 'b'])).toBe('a\nb')
  })
})
```

- [ ] **Step 3: Add scalar field renderer**

Create `apps/gateway-admin/components/settings/SettingsScalarField.tsx` with controls for text/url/number/bool/enum/string-list only. Read-only fields render a non-editable redacted preview.

```tsx
'use client'

import type { SettingsFieldSpec, SettingsState } from '@/lib/api/setup-client'
import { Input } from '@/components/ui/input'
import { Label } from '@/components/ui/label'
import { Switch } from '@/components/ui/switch'
import { Textarea } from '@/components/ui/textarea'
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from '@/components/ui/select'
import { hasEnvOverrideWarning, parseFieldInput, valueAsInputString } from '@/lib/settings/schema'

export function SettingsScalarField({
  field,
  value,
  state,
  error,
  onChange,
}: {
  field: SettingsFieldSpec
  value: unknown
  state: SettingsState
  error?: string
  onChange: (key: string, value: unknown) => void
}): React.ReactElement {
  const id = `settings-${field.key.replaceAll('.', '-')}`
  const disabled = field.write_policy !== 'editable'
  const inputValue = valueAsInputString(value)
  const envOverride = state.sources[field.key]?.overridden_by_env

  return (
    <div className="grid gap-2 rounded-md border p-3">
      <div className="flex items-start justify-between gap-3">
        <div>
          <Label htmlFor={id}>{field.label}</Label>
          <p className="mt-1 text-xs text-muted-foreground">{field.description}</p>
          <p className="mt-1 font-mono text-[11px] text-muted-foreground">{field.key}</p>
        </div>
        <span className="rounded bg-muted px-1.5 py-0.5 text-[10px] uppercase text-muted-foreground">
          {field.apply_mode}
        </span>
      </div>
      {hasEnvOverrideWarning(field, state) ? (
        <p className="text-xs text-amber-600">{envOverride} currently overrides this config.toml value.</p>
      ) : null}
      {field.control === 'bool' ? (
        <Switch id={id} checked={Boolean(value)} disabled={disabled} onCheckedChange={(checked) => onChange(field.key, checked)} />
      ) : field.control === 'enum' ? (
        <Select value={inputValue} disabled={disabled} onValueChange={(next) => onChange(field.key, next)}>
          <SelectTrigger id={id}>
            <SelectValue placeholder={field.example ?? 'Select'} />
          </SelectTrigger>
          <SelectContent>
            {field.options.map((option) => <SelectItem key={option.value} value={option.value}>{option.label}</SelectItem>)}
          </SelectContent>
        </Select>
      ) : field.control === 'string_list' ? (
        <Textarea id={id} value={inputValue} disabled={disabled} className="min-h-24 font-mono text-xs" onChange={(event) => onChange(field.key, parseFieldInput(field, event.target.value))} />
      ) : field.control === 'read_only' ? (
        <pre className="max-h-64 overflow-auto rounded-md bg-muted p-3 text-xs">{JSON.stringify(value ?? null, null, 2)}</pre>
      ) : (
        <Input id={id} type={field.control === 'number' ? 'number' : 'text'} value={inputValue} disabled={disabled} onChange={(event) => onChange(field.key, parseFieldInput(field, event.target.value))} />
      )}
      {error ? <p className="text-xs text-destructive">{error}</p> : null}
    </div>
  )
}
```

- [ ] **Step 4: Add section renderer with partitioned saves and no auto-confirm**

Create `apps/gateway-admin/components/settings/SettingsScalarSection.tsx`:

```tsx
'use client'

import { useEffect, useMemo, useState } from 'react'
import { Loader2 } from 'lucide-react'
import type { SettingsFieldSpec, SettingsState } from '@/lib/api/setup-client'
import { Button } from '@/components/ui/button'
import { Card, CardContent, CardDescription, CardHeader, CardTitle } from '@/components/ui/card'
import { setupApi } from '@/lib/api/setup-client'
import { buildDirtyEntries, editableFields } from '@/lib/settings/schema'
import { SettingsScalarField } from './SettingsScalarField'

export function SettingsScalarSection({
  title,
  description,
  section,
  state,
  fields,
  onSaved,
}: {
  title: string
  description: string
  section: string
  state: SettingsState
  fields: SettingsFieldSpec[]
  onSaved: (state: SettingsState) => void
}): React.ReactElement {
  const initialValues = useMemo(() => Object.fromEntries(fields.map((field) => [field.key, state.values[field.key] ?? null])), [fields, state.values])
  const [values, setValues] = useState<Record<string, unknown>>(initialValues)
  const [changedKeys, setChangedKeys] = useState<Set<string>>(new Set())
  const [saving, setSaving] = useState(false)
  const [errors, setErrors] = useState<Record<string, string>>({})

  useEffect(() => {
    setValues(initialValues)
    setChangedKeys(new Set())
    setErrors({})
  }, [initialValues])

  async function save(): Promise<void> {
    setSaving(true)
    setErrors({})
    try {
      const editable = editableFields(fields)
      const entries = buildDirtyEntries(editable, changedKeys, values)
      const envEntries = entries.filter((entry) => editable.find((field) => field.key === entry.key)?.backend === 'env')
      const configEntries = entries.filter((entry) => editable.find((field) => field.key === entry.key)?.backend === 'config_toml')
      let next = state
      if (envEntries.length > 0) next = await setupApi.settingsEnvUpdate(section, envEntries)
      if (configEntries.length > 0) next = (await setupApi.settingsConfigUpdate(section, configEntries)).state
      onSaved(next)
    } catch (err) {
      setErrors({ _form: err instanceof Error ? err.message : 'save failed' })
    } finally {
      setSaving(false)
    }
  }

  return (
    <Card>
      <CardHeader>
        <CardTitle>{title}</CardTitle>
        <CardDescription>{description}</CardDescription>
      </CardHeader>
      <CardContent className="space-y-3">
        {fields.map((field) => (
          <SettingsScalarField
            key={field.key}
            field={field}
            value={values[field.key]}
            state={state}
            error={errors[field.key]}
            onChange={(key, value) => {
              setValues((prev) => ({ ...prev, [key]: value }))
              setChangedKeys((prev) => new Set(prev).add(key))
            }}
          />
        ))}
        {errors._form ? <p className="text-sm text-destructive">{errors._form}</p> : null}
        <div className="flex justify-end gap-2">
          <Button type="button" variant="outline" disabled={saving || changedKeys.size === 0} onClick={() => { setValues(initialValues); setChangedKeys(new Set()); }}>
            Reset
          </Button>
          <Button type="button" disabled={saving || changedKeys.size === 0} onClick={() => void save()}>
            {saving ? <Loader2 className="mr-2 h-4 w-4 animate-spin" /> : null}
            Save changes
          </Button>
        </div>
      </CardContent>
    </Card>
  )
}
```

- [ ] **Step 5: Add read-only advanced block**

Create `apps/gateway-admin/components/settings/AdvancedReadOnlyBlock.tsx`:

```tsx
import type { SettingsFieldSpec, SettingsState } from '@/lib/api/setup-client'
import { Card, CardContent, CardDescription, CardHeader, CardTitle } from '@/components/ui/card'

export function AdvancedReadOnlyBlock({
  state,
  fields,
}: {
  state: SettingsState
  fields: SettingsFieldSpec[]
}): React.ReactElement {
  return (
    <Card>
      <CardHeader>
        <CardTitle>Read-only advanced config</CardTitle>
        <CardDescription>Complex and dangerous settings are visible here redacted. Typed editors are separate follow-up work.</CardDescription>
      </CardHeader>
      <CardContent className="space-y-3">
        {fields.map((field) => (
          <div key={field.key} className="rounded-md border p-3">
            <p className="text-sm font-medium">{field.label}</p>
            <p className="text-xs text-muted-foreground">{field.description}</p>
            <pre className="mt-2 max-h-72 overflow-auto rounded-md bg-muted p-3 text-xs">
              {JSON.stringify(state.values[field.key] ?? null, null, 2)}
            </pre>
          </div>
        ))}
      </CardContent>
    </Card>
  )
}
```

- [ ] **Step 6: Add interaction tests**

Create `SettingsScalarSection.test.tsx` covering save partition and reset:

```tsx
import { render, screen } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { describe, expect, it, vi } from 'vitest'
import { SettingsScalarSection } from './SettingsScalarSection'
import type { SettingsFieldSpec, SettingsState } from '@/lib/api/setup-client'

vi.mock('@/lib/api/setup-client', async () => {
  const actual = await vi.importActual<typeof import('@/lib/api/setup-client')>('@/lib/api/setup-client')
  return {
    ...actual,
    setupApi: {
      settingsEnvUpdate: vi.fn(async (_section, _entries) => state),
      settingsConfigUpdate: vi.fn(async (_section, _entries) => ({ state, backup_path: null })),
    },
  }
})

const fields: SettingsFieldSpec[] = [
  { key: 'LAB_LOG', label: 'Log filter', description: '', section: 'core', backend: 'env', control: 'text', risk: 'restart', write_policy: 'editable', apply_mode: 'restart', secret: false, required: false, env_override: null, min: null, max: null, options: [], example: null },
]
const state: SettingsState = {
  schema_version: 1,
  config_path: '/tmp/config.toml',
  env_path: '/tmp/.env',
  section: 'core',
  values: { LAB_LOG: 'lab=info' },
  sources: { LAB_LOG: { source: 'env', overridden_by_env: null } },
}

describe('SettingsScalarSection', () => {
  it('resets edited values after reset', async () => {
    const user = userEvent.setup()
    render(<SettingsScalarSection title="Core" description="" section="core" state={state} fields={fields} onSaved={() => undefined} />)
    const input = screen.getByLabelText('Log filter')
    await user.clear(input)
    await user.type(input, 'lab=debug')
    await user.click(screen.getByRole('button', { name: 'Reset' }))
    expect(input).toHaveValue('lab=info')
  })
})
```

- [ ] **Step 7: Run frontend tests**

Run:

```bash
npx vitest run apps/gateway-admin/lib/settings/schema.test.ts apps/gateway-admin/components/settings/SettingsScalarField.test.tsx apps/gateway-admin/components/settings/SettingsScalarSection.test.tsx
```

Expected: tests pass.

- [ ] **Step 8: Commit**

Run:

```bash
git add apps/gateway-admin/lib/settings apps/gateway-admin/components/settings
git commit -m "feat: add scalar settings components"
```

## Task 7: Convert Settings Pages To Section-Scoped Editors

**Files:**
- Modify: `apps/gateway-admin/app/(admin)/settings/core/page.tsx`
- Modify: `apps/gateway-admin/app/(admin)/settings/surfaces/page.tsx`
- Modify: `apps/gateway-admin/app/(admin)/settings/features/page.tsx`
- Modify: `apps/gateway-admin/app/(admin)/settings/advanced/page.tsx`
- Modify: `apps/gateway-admin/components/settings/SettingsRail.tsx`

- [ ] **Step 1: Add a reusable loader pattern in each page**

Use this shape, with the section changed per page:

```tsx
const [schema, setSchema] = useState<SettingsSchemaResponse | undefined>()
const [settings, setSettings] = useState<SettingsState | undefined>()
const [loading, setLoading] = useState(true)
const [error, setError] = useState<string | undefined>()

useEffect(() => {
  const controller = new AbortController()
  Promise.all([
    setupApi.settingsSchema(controller.signal),
    setupApi.settingsState('core', controller.signal),
  ])
    .then(([schemaResponse, stateResponse]) => {
      if (controller.signal.aborted) return
      setSchema(schemaResponse)
      setSettings(stateResponse)
    })
    .catch((err) => {
      if (!controller.signal.aborted) setError(err instanceof Error ? err.message : 'load failed')
    })
    .finally(() => {
      if (!controller.signal.aborted) setLoading(false)
    })
  return () => controller.abort()
}, [])
```

- [ ] **Step 2: Render Core**

In `/settings/core/page.tsx`, render:

```tsx
const fields = schema ? fieldsForSection(schema.fields, 'core') : []

{settings ? (
  <SettingsScalarSection
    title="Core"
    description="Env-backed process defaults and low-risk operator paths."
    section="core"
    state={settings}
    fields={fields}
    onSaved={setSettings}
  />
) : null}
```

- [ ] **Step 3: Render Surfaces**

In `/settings/surfaces/page.tsx`, render only scalar/editor-safe surface fields:

```tsx
const fields = schema ? fieldsForSection(schema.fields, 'surfaces') : []

<SettingsScalarSection
  title="Surfaces"
  description="Safe scalar HTTP, MCP, URL, and CORS settings. Dangerous auth-bypass settings are read-only."
  section="surfaces"
  state={settings}
  fields={fields}
  onSaved={setSettings}
/>
```

- [ ] **Step 4: Render Features**

In `/settings/features/page.tsx`:

```tsx
const fields = schema ? fieldsForSection(schema.fields, 'features') : []

<SettingsScalarSection
  title="Features"
  description="Runtime feature gates with explicit apply semantics."
  section="features"
  state={settings}
  fields={fields}
  onSaved={setSettings}
/>
```

- [ ] **Step 5: Render Advanced read-only and env inventory**

In `/settings/advanced/page.tsx`, fetch `settingsState('advanced')` and `settingsEnvSchema()`. Render:

```tsx
const fields = schema ? fieldsForSection(schema.fields, 'advanced') : []
const readonlyFields = fields.filter((field) => field.write_policy !== 'editable')
const scalarFields = fields.filter((field) => field.write_policy === 'editable')

{settings ? (
  <>
    <SettingsScalarSection
      title="Advanced Scalars"
      description="Low-risk advanced scalar limits and paths."
      section="advanced"
      state={settings}
      fields={scalarFields}
      onSaved={setSettings}
    />
    <AdvancedReadOnlyBlock state={settings} fields={readonlyFields} />
    <EnvInventoryTable entries={envSchema} />
  </>
) : null}
```

Add `EnvInventoryTable` in the page file with a search input and filtered list. Do not render all entries without a filter box:

```tsx
function EnvInventoryTable({ entries }: { entries: EnvSettingSpec[] }): React.ReactElement {
  const [query, setQuery] = useState('')
  const filtered = entries.filter((entry) => `${entry.key} ${entry.service} ${entry.description}`.toLowerCase().includes(query.toLowerCase()))
  return (
    <Card>
      <CardHeader>
        <CardTitle>Environment Inventory</CardTitle>
        <CardDescription>Known env keys from generated docs and service metadata. Only low-risk core env keys are editable in this epic.</CardDescription>
      </CardHeader>
      <CardContent className="space-y-3">
        <Input value={query} onChange={(event) => setQuery(event.target.value)} placeholder="Filter env keys" />
        <ul className="max-h-[520px] divide-y overflow-auto rounded-md border">
          {filtered.map((entry) => (
            <li key={entry.key} className="grid gap-1 p-3 text-sm md:grid-cols-[240px_1fr_auto]">
              <p className="font-mono text-xs">{entry.key}</p>
              <p className="text-muted-foreground">{entry.description}</p>
              <p className="text-xs text-muted-foreground">{entry.service}{entry.secret ? ' secret' : ''}{entry.editable ? ' editable' : ''}</p>
            </li>
          ))}
        </ul>
      </CardContent>
    </Card>
  )
}
```

- [ ] **Step 6: Remove stub labels only after page tests pass**

In `SettingsRail.tsx`, remove `stub?: boolean`, remove `stub: true`, and remove `v2` badge rendering after Core, Surfaces, Features, and Advanced render backed or explicitly read-only schema content.

- [ ] **Step 7: Run focused frontend tests**

Run:

```bash
npx vitest run apps/gateway-admin/lib/api/setup-settings.test.ts apps/gateway-admin/lib/settings/schema.test.ts apps/gateway-admin/components/settings/SettingsScalarSection.test.tsx
```

Expected: tests pass.

- [ ] **Step 8: Commit**

Run:

```bash
git add apps/gateway-admin/app/'(admin)'/settings apps/gateway-admin/components/settings/SettingsRail.tsx
git commit -m "feat: wire settings pages to safe schema"
```

## Task 8: Documentation And Verification

**Files:**
- Modify: `docs/runtime/CONFIG.md` or create it if absent
- Modify: `docs/superpowers/plans/2026-05-09-settings-completion.md`

- [ ] **Step 1: Document runtime config ownership**

Create or update `docs/runtime/CONFIG.md`:

```markdown
# Lab Runtime Configuration

Lab reads configuration from three layers, highest precedence first:

1. CLI flags and process environment variables.
2. `~/.labby/.env`.
3. `config.toml`, searched from current directory, `~/.labby/config.toml`, then `~/.config/labby/config.toml`.

`/settings` exposes this precedence explicitly. When an environment variable overrides a `config.toml` field, the UI warns that changing the TOML value will not affect the current runtime until the env override is removed.

## Write Policy

- Low-risk env keys are updated through `setup.settings.env.update`, which performs a targeted atomic merge without committing unrelated draft entries.
- Low-risk scalar TOML keys are updated through `setup.settings.config.update`, which is admin-only, schema-approved, backed up before write, validated with `LabConfig::validate`, and written atomically.
- Complex sections such as `upstream`, `protected_mcp_routes`, `virtual_servers`, and `deploy` are read-only in `/settings` until typed editors exist.
- Secrets are never returned raw. Config-backed secret writes require a future write-only flow and are not part of the scalar settings editor.

## Apply Modes

- `immediate`: runtime behavior is updated in-process by the settings action.
- `partial`: config is updated, but only some runtime readers observe it without restart.
- `restart`: restart `labby serve` for the setting to fully apply.
- `read_only`: visible but not editable from this settings slice.
```

- [ ] **Step 2: Mark the older plan superseded**

At the top of `docs/superpowers/plans/2026-05-09-settings-completion.md`, add:

```markdown
> Superseded for safe schema-backed settings by `docs/superpowers/plans/2026-06-12-settings-full-configuration.md`. The May plan implemented the first backed settings slice; use the June plan for source-aware scalar editing and read-only complex config visibility.
```

- [ ] **Step 3: Run full backend verification**

Run:

```bash
just check
just test
```

Expected:
- `just check` passes.
- `just test` passes. If integration-only external service failures appear, record the exact failing tests and why they are unrelated before proceeding.

- [ ] **Step 4: Run frontend verification**

Run:

```bash
npx vitest run apps/gateway-admin/lib/api/setup-settings.test.ts apps/gateway-admin/lib/settings/schema.test.ts apps/gateway-admin/components/settings/SettingsScalarField.test.tsx apps/gateway-admin/components/settings/SettingsScalarSection.test.tsx
just web-build
```

Expected:
- Vitest tests pass.
- `just web-build` succeeds.

- [ ] **Step 5: Run mandatory local smoke with temporary config**

Run:

```bash
TMP_HOME=$(mktemp -d)
mkdir -p "$TMP_HOME/.labby"
HOME="$TMP_HOME" cargo run --all-features -- --json setup --smoke >/tmp/settings-state-smoke.json
```

Expected:
- Existing setup state smoke returns JSON with `first_run`.
- No real `~/.labby` files are modified.

- [ ] **Step 6: Run mandatory HTTP settings smoke if dev server is available**

If the dev container or `labby serve` is available, run:

```bash
TOKEN=$(grep "LAB_MCP_HTTP_TOKEN" .env | cut -d= -f2)
curl -fsS \
  -H "Authorization: Bearer $TOKEN" \
  -H "Content-Type: application/json" \
  -d '{"action":"settings.schema","params":{}}' \
  http://localhost:8765/v1/setup >/tmp/settings-schema-action.json
curl -fsS \
  -H "Authorization: Bearer $TOKEN" \
  -H "Content-Type: application/json" \
  -d '{"action":"settings.state","params":{"section":"core"}}' \
  http://localhost:8765/v1/setup >/tmp/settings-core-state.json
```

Expected:
- `settings.schema` returns JSON with `schema_version`.
- `settings.state` for `core` returns only core section values and source metadata.

- [ ] **Step 7: Run mandatory browser smoke if dev server is available**

If the dev container or `labby serve` is available, run:

```bash
TOKEN=$(grep "LAB_MCP_HTTP_TOKEN" .env | cut -d= -f2)
agent-browser --session settings-safe set viewport 1280 900
agent-browser --session settings-safe open http://localhost:8765/settings/core/ --headers "{\"Authorization\":\"Bearer $TOKEN\"}"
agent-browser --session settings-safe screenshot /tmp/settings-core.png
agent-browser --session settings-safe open http://localhost:8765/settings/advanced/ --headers "{\"Authorization\":\"Bearer $TOKEN\"}"
agent-browser --session settings-safe screenshot /tmp/settings-advanced.png
```

Expected:
- Core renders editable low-risk fields.
- Advanced renders read-only complex config and searchable env inventory.
- No text overlap or card nesting regressions.

- [ ] **Step 8: Commit**

Run:

```bash
git add docs/runtime/CONFIG.md docs/superpowers/plans/2026-05-09-settings-completion.md
git commit -m "docs: document safe settings configuration"
```

## Deferred Follow-Up Beads

Create follow-up beads after this epic lands:

- Typed protected MCP route editor with diff preview, admin recheck, validation, route smoke test, and rollback instructions.
- Typed upstream editor with stdio command validation, OAuth secret write-only flow, and tool discovery smoke test.
- Typed virtual server editor with service existence validation and route/tool exposure tests.
- Typed deploy editor with host validation, permission warnings, dry-run, and rollback instructions.
- Dangerous settings flow for `web.disable_auth`, `gateway.disable_spawn_guard`, and `code_mode.enabled`, requiring typed confirmation and explicit restart/rollback copy.
- Config-backed secret replacement/clear flow with write-only controls and server-side placeholder rejection.

## Failure Modes Checklist

| Codepath | Failure Mode | Rescued? | Test? | User Sees? | Logged? |
| --- | --- | --- | --- | --- | --- |
| `settings.schema` | New `LabConfig` field missing from schema | Yes, coverage test | Yes | Missing field | No |
| `settings.state` | Env override hides TOML effect | Yes, source metadata | Yes | Warning | No |
| `settings.config.update` | Non-admin attempts mutation | Yes, admin scope | Yes | 403 | Yes, redacted |
| `settings.config.update` | Parent TOML path is not table | Yes, reject before write | Yes | Field error | Yes, redacted |
| `settings.config.update` | Invalid port/range/url | Yes, per-field validation | Yes | Field error | Yes, redacted |
| `settings.env.update` | Stale concurrent write | Yes, mtime conflict | Yes | Conflict error | Yes, redacted |
| `settings.env.update` | Token/secret env key submitted | Yes, allowlist reject | Yes | Field error | Yes, redacted |
| Recursive redaction | Nested token leaks in advanced state | Yes, redaction test | Yes | Redacted status | No raw secret |
| Frontend scalar section | Parent state changes after save | Yes, effect reset | Yes | Fresh values | No |
| Advanced page | Huge env inventory slows page | Yes, filter and bounded scroll | Yes | Searchable list | No |

## Self-Review

Spec coverage:
- Every `LabConfig` top-level field is either editable as a safe scalar, visible read-only, or explicitly deferred into a typed editor bead.
- `.env` coverage uses generated env reference plus `PluginMeta`; only four low-risk core env keys are editable in this epic.
- Source precedence, apply mode, risk tier, write policy, admin requirements, redaction, backup behavior, and validation are first-class plan elements.

Placeholder scan:
- This plan contains no deferred work markers masquerading as implementation steps; deferred work is explicitly listed as follow-up beads.

Type consistency:
- Rust and TypeScript schema names match: `backend`, `control`, `risk`, `write_policy`, `apply_mode`, `env_override`, `sources`, and `schema_version`.

## Execution Handoff

Plan complete and saved to `docs/superpowers/plans/2026-06-12-settings-full-configuration.md`. Two execution options:

**1. Subagent-Driven (recommended)** - Dispatch a fresh subagent per task, review between tasks, fast iteration.

**2. Inline Execution** - Execute tasks in this session using executing-plans, batch execution with checkpoints.
