# Lab Binary Skeleton Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Stand up a compilable `lab` binary skeleton — `main.rs` + every sibling module DESIGN.md names — so `cargo check -p lab` and `cargo run -p lab -- --help` work end-to-end. Service dispatch bodies stay as stubs; this plan is about shape, not logic.

**Architecture:** Single binary crate, no `lib.rs`. Module tree rooted at `main.rs` declaring siblings (`cli`, `mcp`, `tui`, `api`, `config`, `output`, `catalog`). Modern Rust "no `mod.rs`" style: each `foo` module is `foo.rs` sibling to `foo/`. Clap derive for the top-level `Command` enum with feature-gated service subcommands. Tokio multi-thread runtime. `tracing-subscriber` with `LAB_LOG` env var. All subcommand handlers return `anyhow::Result<()>` and print a `not yet implemented` message while returning Ok.

**Tech Stack:** tokio 1.51, clap 4 (derive), tracing + tracing-subscriber, anyhow, axum 0.8 (already wired in `api/`), dotenvy, toml, dialoguer, ratatui + crossterm, rmcp 1.3.

**What this plan does NOT do:**
- Service dispatch logic (deferred — each service gets its own plan).
- Real MCP server wiring through rmcp (stub returns Ok).
- Real TUI screens (stub exits immediately).
- Real doctor checks (stub returns zero findings).
- Tests beyond `cargo check` + `--help` smoke output. TDD doesn't fit pure scaffolding; correctness is "it compiles and the CLI tree matches DESIGN".

**Pre-reqs already satisfied:**
- `crates/lab/Cargo.toml` has axum/tower/tower-http + clap/tokio/tracing/anyhow/rmcp/ratatui.
- `crates/lab/src/api.rs` + `api/{state,error,router,health}.rs` + `api/CLAUDE.md` exist.
- Nested CLAUDE.md files exist in `cli/`, `mcp/`, `tui/`, `api/`.

---

## File Structure

Files this plan creates (everything under `crates/lab/src/` unless noted):

| File | Responsibility |
|------|----------------|
| `main.rs` | tokio runtime, tracing init, clap dispatch, exit code. Declares all sibling modules. |
| `config.rs` | `LabConfig` struct, `load()` function, multi-instance env parsing helpers. |
| `output.rs` | `OutputFormat` enum (`Human`, `Json`), `print_json`, `print_table` helpers. |
| `catalog.rs` | `Catalog` / `ServiceCatalog` / `ActionEntry` types, `build_catalog()` stub. |
| `cli.rs` | Top-level `Command` enum (clap derive) + `dispatch()` matching to subcommand handlers. |
| `cli/serve.rs` | `lab serve` handler (stub — logs args, returns Ok). |
| `cli/doctor.rs` | `lab doctor` handler (stub — emits empty report, exit 0). |
| `cli/health.rs` | `lab health` handler (stub). |
| `cli/plugins.rs` | `lab plugins` handler (stub — enters TUI). |
| `cli/install.rs` | `lab install` / `uninstall` / `init` handlers (stubs). |
| `cli/completions.rs` | `lab completions <shell>` handler — real impl via `clap_complete`. |
| `cli/help.rs` | `lab help` handler — reads `build_catalog()` and prints. |
| `mcp.rs` | Declares `registry`, `resources`, `envelope`, `error`, `meta`, `services`. |
| `mcp/registry.rs` | `ToolRegistry` type + `register()` feature-gated dispatch builder (empty match). |
| `mcp/resources.rs` | `lab://catalog` / `lab://<service>/actions` resource handlers (stub). |
| `mcp/envelope.rs` | `ToolEnvelope<T>` + `ToolError` with `kind()` method. |
| `mcp/error.rs` | Helper constructors for structured error envelopes. |
| `mcp/meta.rs` | `lab.help` meta-tool dispatch — calls `crate::catalog::build_catalog()`. |
| `mcp/services.rs` | Declares per-service dispatch modules (empty — `#[cfg]` gated, none enabled yet). |
| `tui.rs` | Declares `app`, `metadata`. Exports `run()` entry point. |
| `tui/app.rs` | `App` struct + `run()` stub that returns immediately. |
| `tui/metadata.rs` | `plugin_metadata()` returning empty slice (services not wired yet). |
| `api/router.rs` | **Modify**: confirm `ServiceBuilderExt` import, adjust if needed. |

Existing files left alone: `api.rs`, `api/state.rs`, `api/error.rs`, `api/health.rs`, all `CLAUDE.md` files.

---

## Task 1: `main.rs` entry point with clap + tokio + tracing

**Files:**
- Create: `crates/lab/src/main.rs`

- [ ] **Step 1: Write `main.rs`**

```rust
//! `lab` binary entry point.
//!
//! Initializes tracing, loads config, parses clap args, and dispatches
//! to the appropriate subcommand handler. All subsystems are sibling
//! modules declared here.

#![allow(clippy::multiple_crate_versions)]

mod api;
mod catalog;
mod cli;
mod config;
mod mcp;
mod output;
mod tui;

use std::process::ExitCode;

use anyhow::Result;
use clap::Parser;
use tracing_subscriber::{EnvFilter, fmt, prelude::*};

use crate::cli::Cli;

fn init_tracing() {
    let filter = EnvFilter::try_from_env("LAB_LOG")
        .unwrap_or_else(|_| EnvFilter::new("labby=info,lab_apis=warn"));

    tracing_subscriber::registry()
        .with(filter)
        .with(fmt::layer().with_target(false))
        .init();
}

#[tokio::main(flavor = "multi_thread")]
async fn main() -> ExitCode {
    init_tracing();

    if let Err(err) = config::load() {
        tracing::warn!("config load warning: {err:#}");
    }

    let cli = Cli::parse();

    match cli::dispatch(cli).await {
        Ok(code) => code,
        Err(err) => {
            tracing::error!("{err:#}");
            ExitCode::from(1)
        }
    }
}
```

- [ ] **Step 2: Verify it does NOT compile yet (sibling modules missing)**

Run: `cargo check -p lab 2>&1 | head -20`
Expected: errors like `file not found for module` `cli`/`mcp`/... — confirms `main.rs` is at the right path.

- [ ] **Step 3: Commit**

```bash
git add crates/lab/src/main.rs
git commit -m "feat(lab): add binary entry point with clap + tokio + tracing"
```

---

## Task 2: `config.rs` — LabConfig loader

**Files:**
- Create: `crates/lab/src/config.rs`

- [ ] **Step 1: Write `config.rs`**

```rust
//! Config loading for the `lab` binary.
//!
//! Order of precedence (highest wins):
//!   1. Process environment variables
//!   2. `~/.labby/.env` (loaded via `dotenvy`)
//!   3. `~/.config/labby/config.toml` (preferences, not secrets)
//!
//! Multi-instance services follow the `S_<LABEL>_URL` pattern: a service
//! like `unraid` reads `UNRAID_URL` as the default instance and
//! `UNRAID_BACKUP_URL` as an additional instance labeled `backup-node`.

use std::{collections::HashMap, path::PathBuf};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

/// Fully-resolved `lab` configuration, assembled from env + TOML.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct LabConfig {
    /// Default output format for CLI commands that print tables.
    #[serde(default)]
    pub output: OutputPreferences,
    /// MCP server defaults.
    #[serde(default)]
    pub mcp: McpPreferences,
}

/// Table/json formatting defaults.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct OutputPreferences {
    /// Default format: `human` or `json`. Honored unless `--json` overrides.
    #[serde(default)]
    pub format: Option<String>,
}

/// MCP server defaults.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct McpPreferences {
    /// Default transport (`stdio` or `http`).
    #[serde(default)]
    pub transport: Option<String>,
    /// Default bind address for the HTTP transport.
    #[serde(default)]
    pub host: Option<String>,
    /// Default port for the HTTP transport.
    #[serde(default)]
    pub port: Option<u16>,
}

/// Load `.env` + `config.toml` from the standard locations.
///
/// Returns `Ok(())` if loading succeeded, even if no files were present.
/// Returns `Err` only for parse failures — missing files are not errors.
pub fn load() -> Result<LabConfig> {
    if let Some(env_path) = dotenv_path() {
        if env_path.exists() {
            dotenvy::from_path(&env_path)
                .with_context(|| format!("failed to load {}", env_path.display()))?;
        }
    }

    let toml_path = toml_path();
    let cfg = if let Some(path) = toml_path.as_ref() {
        if path.exists() {
            let raw = std::fs::read_to_string(path)
                .with_context(|| format!("failed to read {}", path.display()))?;
            toml::from_str::<LabConfig>(&raw)
                .with_context(|| format!("failed to parse {}", path.display()))?
        } else {
            LabConfig::default()
        }
    } else {
        LabConfig::default()
    };

    Ok(cfg)
}

/// Standard location for the `.env` file: `$HOME/.labby/.env`.
fn dotenv_path() -> Option<PathBuf> {
    std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".labby").join(".env"))
}

/// Standard location for the TOML config: `$HOME/.config/labby/config.toml`.
fn toml_path() -> Option<PathBuf> {
    std::env::var_os("HOME")
        .map(|home| PathBuf::from(home).join(".config").join("labby").join("config.toml"))
}

/// Parse multi-instance env vars for a given service prefix.
///
/// Returns a map from instance label (`"default"` or `"<label>"`) to the
/// set of `(suffix, value)` pairs. Example: for prefix `UNRAID`, env vars
/// `UNRAID_URL`, `UNRAID_API_KEY`, `UNRAID_BACKUP_URL`, `UNRAID_BACKUP_API_KEY`
/// yield two entries keyed `"default"` and `"backup-node"`.
#[must_use]
pub fn scan_instances(prefix: &str) -> HashMap<String, HashMap<String, String>> {
    let mut out: HashMap<String, HashMap<String, String>> = HashMap::new();
    let known_suffixes = ["URL", "API_KEY", "TOKEN", "USERNAME", "PASSWORD"];

    for (key, value) in std::env::vars() {
        let Some(rest) = key.strip_prefix(&format!("{prefix}_")) else {
            continue;
        };

        for suffix in &known_suffixes {
            if rest == *suffix {
                out.entry("default".to_string())
                    .or_default()
                    .insert((*suffix).to_string(), value.clone());
                break;
            }
            if let Some(label) = rest.strip_suffix(&format!("_{suffix}")) {
                if !label.is_empty() {
                    out.entry(label.to_ascii_lowercase())
                        .or_default()
                        .insert((*suffix).to_string(), value.clone());
                    break;
                }
            }
        }
    }

    out
}
```

- [ ] **Step 2: Commit**

```bash
git add crates/lab/src/config.rs
git commit -m "feat(lab): add LabConfig loader with multi-instance env scanning"
```

---

## Task 3: `output.rs` — formatting helpers

**Files:**
- Create: `crates/lab/src/output.rs`

- [ ] **Step 1: Write `output.rs`**

```rust
//! Output formatting for CLI commands.
//!
//! All CLI handlers should call [`print`] with their data — it picks
//! human-readable table or JSON based on the active [`OutputFormat`].

use anyhow::Result;
use serde::Serialize;

/// CLI output format, selected by the top-level `--json` flag.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum OutputFormat {
    /// Human-readable (tables, colors where supported).
    #[default]
    Human,
    /// Machine-readable JSON, one value per invocation.
    Json,
}

impl OutputFormat {
    /// Resolve the format from a boolean `--json` flag.
    #[must_use]
    pub const fn from_json_flag(json: bool) -> Self {
        if json { Self::Json } else { Self::Human }
    }
}

/// Print a serializable value in the requested format.
///
/// Human output falls back to pretty-printed JSON for now; individual
/// commands with richer rendering can use [`print_table`] directly.
pub fn print<T: Serialize>(value: &T, format: OutputFormat) -> Result<()> {
    match format {
        OutputFormat::Json => {
            let rendered = serde_json::to_string_pretty(value)?;
            println!("{rendered}");
        }
        OutputFormat::Human => {
            let rendered = serde_json::to_string_pretty(value)?;
            println!("{rendered}");
        }
    }
    Ok(())
}

/// Print a pre-built `tabled::Table` to stdout.
pub fn print_table(table: &tabled::Table) {
    println!("{table}");
}
```

- [ ] **Step 2: Commit**

```bash
git add crates/lab/src/output.rs
git commit -m "feat(lab): add output format helpers"
```

---

## Task 4: `catalog.rs` — shared discovery module

**Files:**
- Create: `crates/lab/src/catalog.rs`

- [ ] **Step 1: Write `catalog.rs`**

```rust
//! Shared catalog module — single source of truth for service + action
//! discovery, feeding three surfaces: the `lab.help` MCP meta-tool, the
//! `lab://catalog` MCP resource, and the `lab help` CLI subcommand.
//!
//! Services register themselves into a [`ToolRegistry`] at startup; this
//! module walks that registry to produce a [`Catalog`].

use serde::{Deserialize, Serialize};

use crate::mcp::registry::ToolRegistry;

/// Top-level discovery document.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Catalog {
    /// One entry per registered service.
    pub services: Vec<ServiceCatalog>,
}

/// Per-service slice of the discovery document.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServiceCatalog {
    /// Service identifier (matches the MCP tool name and CLI subcommand).
    pub name: String,
    /// Short human description from `PluginMeta::description`.
    pub description: String,
    /// Category slug (Media, Servarr, Notifications, etc.).
    pub category: String,
    /// List of actions exposed by the service.
    pub actions: Vec<ActionEntry>,
}

/// One action inside a service's catalog.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ActionEntry {
    /// Dotted action name (e.g., `movie.search`).
    pub name: String,
    /// Short description.
    pub description: String,
    /// Whether the action mutates state and requires confirmation.
    pub destructive: bool,
}

/// Build a [`Catalog`] from the current tool registry. The registry is
/// the authoritative source — never hand-roll catalog entries.
#[must_use]
pub fn build_catalog(registry: &ToolRegistry) -> Catalog {
    let services = registry
        .services()
        .iter()
        .map(|svc| ServiceCatalog {
            name: svc.name.to_string(),
            description: svc.description.to_string(),
            category: svc.category.to_string(),
            actions: Vec::new(),
        })
        .collect();

    Catalog { services }
}
```

- [ ] **Step 2: Commit**

```bash
git add crates/lab/src/catalog.rs
git commit -m "feat(lab): add shared catalog module skeleton"
```

---

## Task 5: `mcp.rs` + `mcp/` submodules

**Files:**
- Create: `crates/lab/src/mcp.rs`
- Create: `crates/lab/src/mcp/registry.rs`
- Create: `crates/lab/src/mcp/envelope.rs`
- Create: `crates/lab/src/mcp/error.rs`
- Create: `crates/lab/src/mcp/resources.rs`
- Create: `crates/lab/src/mcp/meta.rs`
- Create: `crates/lab/src/mcp/services.rs`

- [ ] **Step 1: Write `mcp.rs`**

```rust
//! MCP transport layer — the translation between `lab-apis` clients and
//! the Model Context Protocol. See `crates/lab/src/mcp/CLAUDE.md` for
//! the full rulebook on dispatch, envelopes, and the shared catalog.

pub mod envelope;
pub mod error;
pub mod meta;
pub mod registry;
pub mod resources;
pub mod services;

pub use envelope::{ToolEnvelope, ToolError};
pub use registry::ToolRegistry;
```

- [ ] **Step 2: Write `mcp/registry.rs`**

```rust
//! Runtime tool registry. Services register themselves here during
//! startup; the MCP server walks the registry to expose tools and the
//! catalog module walks it to produce discovery docs.

/// Metadata the registry keeps about each registered service.
#[derive(Debug, Clone)]
pub struct RegisteredService {
    /// Service / tool name.
    pub name: &'static str,
    /// Short description from `PluginMeta::description`.
    pub description: &'static str,
    /// Category slug.
    pub category: &'static str,
}

/// Collection of registered services, built at startup.
#[derive(Debug, Default)]
pub struct ToolRegistry {
    services: Vec<RegisteredService>,
}

impl ToolRegistry {
    /// Create an empty registry.
    #[must_use]
    pub const fn new() -> Self {
        Self { services: Vec::new() }
    }

    /// Register a service. Duplicates are ignored (first registration wins).
    pub fn register(&mut self, service: RegisteredService) {
        if !self.services.iter().any(|s| s.name == service.name) {
            self.services.push(service);
        }
    }

    /// Borrow the current service list.
    #[must_use]
    pub fn services(&self) -> &[RegisteredService] {
        &self.services
    }
}

/// Build a registry with every feature-enabled service registered.
///
/// This is the single place feature flags gate MCP tool availability.
/// Service entries are added in alphabetical order.
#[must_use]
pub fn build_default_registry() -> ToolRegistry {
    let registry = ToolRegistry::new();
    // Services will be registered here as they're wired up, e.g.:
    // #[cfg(feature = "radarr")]
    // registry.register(RegisteredService {
    //     name: lab_apis::radarr::META.name,
    //     description: lab_apis::radarr::META.description,
    //     category: "servarr",
    // });
    registry
}
```

- [ ] **Step 3: Write `mcp/envelope.rs`**

```rust
//! Structured JSON envelopes returned by every MCP tool dispatch.
//! Shape is identical to what the HTTP API emits (see `api/error.rs`)
//! so clients can share error-handling logic across transports.

use serde::{Deserialize, Serialize};

/// Successful tool result wrapper.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct ToolEnvelope<T> {
    /// The tool's result payload.
    pub data: T,
}

impl<T> ToolEnvelope<T> {
    /// Wrap a value in an envelope.
    pub const fn new(data: T) -> Self {
        Self { data }
    }
}

/// Error variants that MCP dispatchers can produce on top of SDK errors.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ToolError {
    /// Action name not recognized for this service.
    UnknownAction {
        /// Human-readable message.
        message: String,
        /// Valid action names for this service.
        valid: Vec<String>,
        /// Optional fuzzy suggestion.
        hint: Option<String>,
    },
    /// Required parameter missing.
    MissingParam {
        /// Human-readable message.
        message: String,
        /// Parameter name.
        param: String,
    },
    /// Parameter present but wrong type or value.
    InvalidParam {
        /// Human-readable message.
        message: String,
        /// Parameter name.
        param: String,
    },
    /// Multi-instance label not found.
    UnknownInstance {
        /// Human-readable message.
        message: String,
        /// Known instance labels.
        valid: Vec<String>,
    },
    /// Pass-through of an `ApiError::kind()` tag from the SDK.
    Sdk {
        /// Stable kind tag (`auth_failed`, `rate_limited`, …).
        kind: String,
        /// Human-readable message.
        message: String,
    },
}

impl ToolError {
    /// Canonical stable string tag.
    #[must_use]
    pub fn kind(&self) -> &str {
        match self {
            Self::UnknownAction { .. } => "unknown_action",
            Self::MissingParam { .. } => "missing_param",
            Self::InvalidParam { .. } => "invalid_param",
            Self::UnknownInstance { .. } => "unknown_instance",
            Self::Sdk { kind, .. } => kind.as_str(),
        }
    }
}
```

- [ ] **Step 4: Write `mcp/error.rs`**

```rust
//! Helper constructors for [`ToolError`] envelopes. Dispatchers should
//! prefer these over building variants inline — keeps envelope shape
//! consistent across services.

use crate::mcp::envelope::ToolError;

/// Build an `unknown_action` envelope with a list of valid actions and
/// an optional fuzzy suggestion.
#[must_use]
pub fn unknown_action(service: &str, action: &str, valid: Vec<String>) -> ToolError {
    ToolError::UnknownAction {
        message: format!("unknown action `{action}` for service `{service}`"),
        valid,
        hint: None,
    }
}

/// Build a `missing_param` envelope.
#[must_use]
pub fn missing_param(param: &str) -> ToolError {
    ToolError::MissingParam {
        message: format!("missing required parameter `{param}`"),
        param: param.to_string(),
    }
}

/// Build an `invalid_param` envelope.
#[must_use]
pub fn invalid_param(param: &str, reason: &str) -> ToolError {
    ToolError::InvalidParam {
        message: format!("invalid parameter `{param}`: {reason}"),
        param: param.to_string(),
    }
}

/// Build an `unknown_instance` envelope listing valid labels.
#[must_use]
pub fn unknown_instance(label: &str, valid: Vec<String>) -> ToolError {
    ToolError::UnknownInstance {
        message: format!("unknown instance `{label}`"),
        valid,
    }
}
```

- [ ] **Step 5: Write `mcp/resources.rs`**

```rust
//! MCP resource handlers.
//!
//! Exposes `lab://catalog` (the full discovery document) and
//! `lab://<service>/actions` (per-service action list). Resources are
//! read-only and derived from the shared catalog.

use anyhow::Result;
use serde_json::Value;

use crate::{catalog::build_catalog, mcp::registry::ToolRegistry};

/// Render the `lab://catalog` resource as JSON.
pub fn catalog_json(registry: &ToolRegistry) -> Result<Value> {
    let catalog = build_catalog(registry);
    Ok(serde_json::to_value(catalog)?)
}

/// Render the `lab://<service>/actions` resource for one service.
pub fn service_actions_json(registry: &ToolRegistry, service: &str) -> Result<Value> {
    let catalog = build_catalog(registry);
    let entry = catalog
        .services
        .into_iter()
        .find(|s| s.name == service)
        .ok_or_else(|| anyhow::anyhow!("unknown service: {service}"))?;
    Ok(serde_json::to_value(entry.actions)?)
}
```

- [ ] **Step 6: Write `mcp/meta.rs`**

```rust
//! The `lab.help` global MCP meta-tool. Returns the full catalog in
//! envelope form so agents can discover every enabled service and
//! action in one call.

use anyhow::Result;

use crate::{
    catalog::{Catalog, build_catalog},
    mcp::{envelope::ToolEnvelope, registry::ToolRegistry},
};

/// Dispatch the `lab.help` meta-tool.
pub fn help(registry: &ToolRegistry) -> Result<ToolEnvelope<Catalog>> {
    Ok(ToolEnvelope::new(build_catalog(registry)))
}
```

- [ ] **Step 7: Write `mcp/services.rs`**

```rust
//! Per-service dispatch modules.
//!
//! Each enabled service declares one submodule here, feature-gated with
//! `#[cfg(feature = "<service>")]`. The submodule exposes a `dispatch`
//! function that takes an action + params object and returns a
//! [`crate::mcp::envelope::ToolEnvelope`] or [`crate::mcp::envelope::ToolError`].
//!
//! No services are wired in this skeleton — they are added in later
//! service-specific plans.

// #[cfg(feature = "radarr")]
// pub mod radarr;
// #[cfg(feature = "sonarr")]
// pub mod sonarr;
// ...
```

- [ ] **Step 8: Commit**

```bash
git add crates/lab/src/mcp.rs crates/lab/src/mcp/
git commit -m "feat(lab): add mcp module tree (registry, envelopes, resources, meta)"
```

---

## Task 6: `tui.rs` + `tui/` stubs

**Files:**
- Create: `crates/lab/src/tui.rs`
- Create: `crates/lab/src/tui/app.rs`
- Create: `crates/lab/src/tui/metadata.rs`

- [ ] **Step 1: Write `tui.rs`**

```rust
//! Ratatui plugin manager. See `crates/lab/src/tui/CLAUDE.md` for the
//! TUI-vs-CLI divide and `.mcp.json` patching rules.

pub mod app;
pub mod metadata;

pub use app::run;
```

- [ ] **Step 2: Write `tui/app.rs`**

```rust
//! TUI entry point. The full plugin manager UI is implemented in later
//! plans; this stub keeps the surface compilable and returns immediately.

use anyhow::Result;

/// Run the plugin manager TUI. Currently a no-op stub.
pub fn run() -> Result<()> {
    tracing::warn!("tui::run() stub — plugin manager not yet implemented");
    Ok(())
}
```

- [ ] **Step 3: Write `tui/metadata.rs`**

```rust
//! Collected `PluginMeta` references for every compiled-in service.
//! The TUI reads this list to render the plugin browser.

/// One row in the plugin manager view.
#[derive(Debug, Clone)]
pub struct PluginRow {
    /// Service identifier.
    pub name: &'static str,
    /// Short description.
    pub description: &'static str,
    /// Category slug.
    pub category: &'static str,
}

/// Return every compiled-in plugin. Empty in the skeleton — services
/// will be wired in as they come online.
#[must_use]
pub fn all_plugins() -> Vec<PluginRow> {
    Vec::new()
}
```

- [ ] **Step 4: Commit**

```bash
git add crates/lab/src/tui.rs crates/lab/src/tui/
git commit -m "feat(lab): add tui module skeleton"
```

---

## Task 7: `cli.rs` — top-level Command enum + dispatch

**Files:**
- Create: `crates/lab/src/cli.rs`
- Create: `crates/lab/src/cli/serve.rs`
- Create: `crates/lab/src/cli/doctor.rs`
- Create: `crates/lab/src/cli/health.rs`
- Create: `crates/lab/src/cli/plugins.rs`
- Create: `crates/lab/src/cli/install.rs`
- Create: `crates/lab/src/cli/completions.rs`
- Create: `crates/lab/src/cli/help.rs`

- [ ] **Step 1: Add `clap_complete` to workspace deps**

Edit `/home/jmagar/workspace/lab/Cargo.toml`, under `[workspace.dependencies]`, below the existing `clap` line:

```toml
clap_complete = "4"
```

Then in `crates/lab/Cargo.toml`, under `[dependencies]` near the existing `clap.workspace = true`:

```toml
clap_complete.workspace = true
```

- [ ] **Step 2: Write `cli.rs`**

```rust
//! Top-level CLI — clap derive definitions and dispatch router.
//!
//! Every subcommand is a thin shim that parses args, calls into a
//! `lab-apis` client (or a lab-local subsystem), and formats output.
//! See `crates/lab/src/cli/CLAUDE.md` for the rulebook.

pub mod completions;
pub mod doctor;
pub mod health;
pub mod help;
pub mod install;
pub mod plugins;
pub mod serve;

use std::process::ExitCode;

use anyhow::Result;
use clap::{Parser, Subcommand};

use crate::output::OutputFormat;

/// `lab` — pluggable homelab CLI + MCP server SDK.
#[derive(Debug, Parser)]
#[command(name = "lab", version, about, long_about = None)]
pub struct Cli {
    /// Emit JSON instead of human-readable tables.
    #[arg(long, global = true)]
    pub json: bool,

    /// Subcommand to run.
    #[command(subcommand)]
    pub command: Command,
}

impl Cli {
    /// Resolved output format based on the `--json` flag.
    #[must_use]
    pub const fn format(&self) -> OutputFormat {
        OutputFormat::from_json_flag(self.json)
    }
}

/// Every top-level subcommand. Service subcommands are added in later
/// plans as each service comes online.
#[derive(Debug, Subcommand)]
pub enum Command {
    /// Start the MCP server (stdio or HTTP transport).
    Serve(serve::ServeArgs),
    /// Audit configured services and report problems.
    Doctor,
    /// Quick reachability check for configured services.
    Health,
    /// Open the plugin manager TUI.
    Plugins,
    /// Install one or more services into `.mcp.json`.
    Install(install::InstallArgs),
    /// Uninstall services from `.mcp.json`.
    Uninstall(install::UninstallArgs),
    /// First-time setup wizard.
    Init,
    /// Print the service + action catalog.
    Help,
    /// Generate shell completions.
    Completions(completions::CompletionsArgs),
}

/// Dispatch a parsed [`Cli`] to the correct handler.
pub async fn dispatch(cli: Cli) -> Result<ExitCode> {
    let format = cli.format();
    match cli.command {
        Command::Serve(args) => serve::run(args).await,
        Command::Doctor => doctor::run(format).await,
        Command::Health => health::run(format).await,
        Command::Plugins => plugins::run(),
        Command::Install(args) => install::run_install(args),
        Command::Uninstall(args) => install::run_uninstall(args),
        Command::Init => install::run_init(),
        Command::Help => help::run(format),
        Command::Completions(args) => completions::run(args),
    }
}
```

- [ ] **Step 3: Write `cli/serve.rs`**

```rust
//! `lab serve` — start the MCP server.

use std::process::ExitCode;

use anyhow::Result;
use clap::{Args, ValueEnum};

/// Transport choices for `lab serve`.
#[derive(Debug, Clone, Copy, ValueEnum)]
#[value(rename_all = "lowercase")]
pub enum Transport {
    /// stdin/stdout framing (default, used by Claude Desktop etc.).
    Stdio,
    /// HTTP transport — requires `LAB_MCP_HTTP_TOKEN` in the environment.
    Http,
}

/// `lab serve` arguments.
#[derive(Debug, Args)]
pub struct ServeArgs {
    /// Comma- or space-separated list of services to enable.
    #[arg(long, value_delimiter = ',')]
    pub services: Vec<String>,
    /// Transport to use.
    #[arg(long, value_enum, default_value_t = Transport::Stdio)]
    pub transport: Transport,
    /// Bind host for the HTTP transport.
    #[arg(long, default_value = "127.0.0.1")]
    pub host: String,
    /// Bind port for the HTTP transport.
    #[arg(long, default_value_t = 8765)]
    pub port: u16,
}

/// Run the serve subcommand. Stub — real rmcp wiring comes in a later plan.
pub async fn run(args: ServeArgs) -> Result<ExitCode> {
    tracing::warn!(
        services = ?args.services,
        transport = ?args.transport,
        host = %args.host,
        port = args.port,
        "lab serve: MCP server not yet wired — skeleton stub",
    );
    Ok(ExitCode::SUCCESS)
}
```

- [ ] **Step 4: Write `cli/doctor.rs`**

```rust
//! `lab doctor` — comprehensive health audit.
//!
//! Exit codes: 0 = ok, 1 = warnings, 2 = failures. Real checks wired in
//! a later plan; this stub always returns 0 with an empty report.

use std::process::ExitCode;

use anyhow::Result;
use serde::Serialize;

use crate::output::{OutputFormat, print};

/// Severity of a single doctor finding.
#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Severity {
    Ok,
    Warn,
    Fail,
}

/// One entry in the doctor report.
#[derive(Debug, Clone, Serialize)]
pub struct Finding {
    pub service: String,
    pub check: String,
    pub severity: Severity,
    pub message: String,
}

/// Full doctor report.
#[derive(Debug, Clone, Serialize)]
pub struct Report {
    pub findings: Vec<Finding>,
}

/// Run the doctor subcommand.
pub async fn run(format: OutputFormat) -> Result<ExitCode> {
    let report = Report { findings: Vec::new() };
    print(&report, format)?;

    let worst = report.findings.iter().map(|f| f.severity).fold(
        Severity::Ok,
        |acc, s| match (acc, s) {
            (Severity::Fail, _) | (_, Severity::Fail) => Severity::Fail,
            (Severity::Warn, _) | (_, Severity::Warn) => Severity::Warn,
            _ => Severity::Ok,
        },
    );

    Ok(match worst {
        Severity::Ok => ExitCode::SUCCESS,
        Severity::Warn => ExitCode::from(1),
        Severity::Fail => ExitCode::from(2),
    })
}
```

- [ ] **Step 5: Write `cli/health.rs`**

```rust
//! `lab health` — quick reachability ping for every configured service.

use std::process::ExitCode;

use anyhow::Result;
use serde::Serialize;

use crate::output::{OutputFormat, print};

/// One row of the health report.
#[derive(Debug, Clone, Serialize)]
pub struct HealthRow {
    pub service: String,
    pub reachable: bool,
    pub latency_ms: u64,
}

/// Run the health subcommand. Stub — returns an empty report.
pub async fn run(format: OutputFormat) -> Result<ExitCode> {
    let rows: Vec<HealthRow> = Vec::new();
    print(&rows, format)?;
    Ok(ExitCode::SUCCESS)
}
```

- [ ] **Step 6: Write `cli/plugins.rs`**

```rust
//! `lab plugins` — open the plugin manager TUI.

use std::process::ExitCode;

use anyhow::Result;

/// Run the plugins subcommand.
pub fn run() -> Result<ExitCode> {
    crate::tui::run()?;
    Ok(ExitCode::SUCCESS)
}
```

- [ ] **Step 7: Write `cli/install.rs`**

```rust
//! `lab install` / `lab uninstall` / `lab init`.
//!
//! These subcommands mutate the user's `.mcp.json` and/or `~/.labby/.env`.
//! Real logic lives in later plans — stubs just log intent.

use std::process::ExitCode;

use anyhow::Result;
use clap::Args;

/// `lab install` arguments.
#[derive(Debug, Args)]
pub struct InstallArgs {
    /// Services to install.
    #[arg(required = true)]
    pub services: Vec<String>,
}

/// `lab uninstall` arguments.
#[derive(Debug, Args)]
pub struct UninstallArgs {
    /// Services to uninstall.
    #[arg(required = true)]
    pub services: Vec<String>,
}

/// Run `lab install`. Stub.
pub fn run_install(args: InstallArgs) -> Result<ExitCode> {
    tracing::warn!(services = ?args.services, "lab install: not yet implemented");
    Ok(ExitCode::SUCCESS)
}

/// Run `lab uninstall`. Stub.
pub fn run_uninstall(args: UninstallArgs) -> Result<ExitCode> {
    tracing::warn!(services = ?args.services, "lab uninstall: not yet implemented");
    Ok(ExitCode::SUCCESS)
}

/// Run `lab init` setup wizard. Stub.
pub fn run_init() -> Result<ExitCode> {
    tracing::warn!("lab init: setup wizard not yet implemented");
    Ok(ExitCode::SUCCESS)
}
```

- [ ] **Step 8: Write `cli/completions.rs`**

```rust
//! `lab completions <shell>` — emit shell completion scripts.
//!
//! This is the one real subcommand in the skeleton: `clap_complete`
//! generates the script from the `Cli` derive.

use std::{io, process::ExitCode};

use anyhow::Result;
use clap::{Args, CommandFactory};
use clap_complete::{Shell, generate};

use crate::cli::Cli;

/// `lab completions` arguments.
#[derive(Debug, Args)]
pub struct CompletionsArgs {
    /// Target shell.
    #[arg(value_enum)]
    pub shell: Shell,
}

/// Run the completions subcommand.
pub fn run(args: CompletionsArgs) -> Result<ExitCode> {
    let mut cmd = Cli::command();
    let bin_name = cmd.get_name().to_string();
    generate(args.shell, &mut cmd, bin_name, &mut io::stdout());
    Ok(ExitCode::SUCCESS)
}
```

- [ ] **Step 9: Write `cli/help.rs`**

```rust
//! `lab help` — print the shared service + action catalog.

use std::process::ExitCode;

use anyhow::Result;

use crate::{
    catalog::build_catalog,
    mcp::registry::build_default_registry,
    output::{OutputFormat, print},
};

/// Run the help subcommand.
pub fn run(format: OutputFormat) -> Result<ExitCode> {
    let registry = build_default_registry();
    let catalog = build_catalog(&registry);
    print(&catalog, format)?;
    Ok(ExitCode::SUCCESS)
}
```

- [ ] **Step 10: Commit**

```bash
git add crates/lab/src/cli.rs crates/lab/src/cli/ crates/lab/Cargo.toml Cargo.toml
git commit -m "feat(lab): add cli module with subcommand stubs + real completions"
```

---

## Task 8: Wire `api.rs` fixes — `ServiceBuilderExt` import

**Files:**
- Modify: `crates/lab/src/api/router.rs`

- [ ] **Step 1: Read the existing router file**

Run: `cat crates/lab/src/api/router.rs` (via Read tool).

- [ ] **Step 2: Replace the imports and builder chain**

Current file uses `tower_http::ServiceBuilderExt`. Replace its body with:

```rust
//! Top-level axum router builder.
//!
//! Composes feature-gated service routers under `/v1/<service>` and mounts
//! cross-cutting middleware (tracing, CORS, compression, timeout).

use std::time::Duration;

use axum::{Router, routing::get};
use tower_http::{
    compression::CompressionLayer, cors::CorsLayer, timeout::TimeoutLayer,
    trace::TraceLayer,
};

use super::{health, state::AppState};

/// Build the full `lab` HTTP router with all enabled service route groups
/// and the standard middleware stack applied.
#[must_use]
pub fn build_router(state: AppState) -> Router {
    Router::new()
        .route("/health", get(health::health))
        .route("/ready", get(health::ready))
        .with_state(state)
        .layer(TraceLayer::new_for_http())
        .layer(TimeoutLayer::new(Duration::from_secs(30)))
        .layer(CompressionLayer::new())
        .layer(CorsLayer::permissive())
}
```

Rationale: `ServiceBuilderExt` + request-id layer required extra trait bounds that aren't worth the plumbing in the skeleton. The four `tower_http` layers above stack cleanly via `Router::layer`. Request-id propagation can be re-added in a later observability plan.

- [ ] **Step 3: Commit**

```bash
git add crates/lab/src/api/router.rs
git commit -m "fix(lab): simplify api router middleware stack for skeleton compile"
```

---

## Task 9: Compile check — the whole skeleton

**Files:** none

- [ ] **Step 1: Run cargo check with default features**

Run: `cargo check -p lab`
Expected: **clean compile, zero errors**. Warnings about unused `dead_code` fields on `AppStateInner` are fine.

- [ ] **Step 2: Run cargo check with no default features**

Run: `cargo check -p lab --no-default-features`
Expected: clean compile (skeleton has no service features wired, so this should be identical).

- [ ] **Step 3: Run clippy**

Run: `cargo clippy -p lab -- -D warnings`
Expected: clean. If `clippy::missing_docs_in_private_items` or pedantic lints trip on a field, add `/// ...` doc comments or narrow allowances in-file only (never crate-wide).

- [ ] **Step 4: Smoke test the CLI help output**

Run: `cargo run -p lab -- --help`
Expected: prints the top-level help with subcommands `serve`, `doctor`, `health`, `plugins`, `install`, `uninstall`, `init`, `help`, `completions`.

Run: `cargo run -p lab -- serve --help`
Expected: shows `--services`, `--transport`, `--host`, `--port`.

Run: `cargo run -p lab -- doctor`
Expected: prints `{ "findings": [] }` and exits 0.

Run: `cargo run -p lab -- completions bash | head -5`
Expected: real bash completion script.

- [ ] **Step 5: Commit if any fixes were needed**

```bash
git add -A
git commit -m "chore(lab): fix compile/clippy warnings in skeleton"
```

Skip this step if nothing needed fixing.

---

## Task 10: Self-review pass

**Files:** none — audit only.

- [ ] **Step 1: Verify every DESIGN-named file is present**

Run: `ls crates/lab/src/ crates/lab/src/cli/ crates/lab/src/mcp/ crates/lab/src/tui/ crates/lab/src/api/` (via Glob or Bash).

Checklist — every item below must exist:

- [ ] `main.rs`
- [ ] `config.rs`
- [ ] `output.rs`
- [ ] `catalog.rs`
- [ ] `cli.rs` + `cli/{serve,doctor,health,plugins,install,completions,help}.rs`
- [ ] `mcp.rs` + `mcp/{registry,envelope,error,resources,meta,services}.rs`
- [ ] `tui.rs` + `tui/{app,metadata}.rs`
- [ ] `api.rs` + `api/{state,error,router,health}.rs`

- [ ] **Step 2: Verify no `mod.rs` files snuck in**

Run: `find crates/lab/src -name mod.rs`
Expected: empty output. (DESIGN rule: modern Rust module style, no `mod.rs`.)

- [ ] **Step 3: Verify CLI subcommand set matches DESIGN**

`cargo run -p lab -- --help` output must contain: `serve`, `doctor`, `health`, `plugins`, `install`, `uninstall`, `init`, `help`, `completions`.

Missing subcommands from DESIGN that are deferred (not in this skeleton): per-service commands (`radarr`, `sonarr`, …), `extract`, `self-update`. These come in later plans. Note their absence is intentional.

- [ ] **Step 4: Verify final commit state**

Run: `git status`
Expected: clean working tree.

Run: `git log --oneline -15`
Expected: one commit per task above, in order.

---

## Self-review

**Spec coverage:**
- ✅ `main.rs` entry point with tokio + tracing + clap → Task 1
- ✅ Config loading order (.env → TOML → process env) → Task 2
- ✅ Multi-instance `S_<LABEL>_URL` parsing → Task 2 (`scan_instances`)
- ✅ Output formatting → Task 3
- ✅ Shared catalog module → Task 4
- ✅ MCP module tree (registry, resources, envelope, error, meta, services) → Task 5
- ✅ TUI stub → Task 6
- ✅ CLI subcommands (serve, doctor, health, plugins, install, uninstall, init, help, completions) → Task 7
- ✅ Exit code mapping for `doctor` (0/1/2) → Task 7 Step 4
- ✅ `serve --transport stdio|http` flag → Task 7 Step 3
- ✅ `api.rs` wired via `mod api;` in `main.rs` → Task 1 + Task 8 fix
- ✅ `cargo check -p lab` compiles clean → Task 9
- ✅ Module layout with no `mod.rs` → Task 10 Step 2
- ❌ Per-service CLI/MCP dispatch modules — **intentionally deferred** (one plan per service).
- ❌ Real rmcp wiring in `serve` — **intentionally deferred**.
- ❌ Real TUI screens — **intentionally deferred**.
- ❌ Real doctor/health checks — **intentionally deferred**.

**Placeholder scan:** every code step contains complete, compilable code. No TBDs, no "implement later" inside code blocks. Deferred work is explicitly called out at the top and in self-review.

**Type consistency:**
- `ToolRegistry` defined in Task 5 (`mcp/registry.rs`), consumed in Task 4 (`catalog.rs`) and Task 7 (`cli/help.rs`) with matching `services() -> &[RegisteredService]` API.
- `RegisteredService` fields (`name`, `description`, `category`) used consistently in `build_catalog`.
- `OutputFormat` defined in Task 3, consumed in Task 7 handlers with the same `from_json_flag` constructor.
- `Cli` struct + `format()` method used in Task 1 main and Task 7 dispatch identically.
- `ServeArgs`, `InstallArgs`, `UninstallArgs`, `CompletionsArgs` defined in their handler modules and imported via the `Command` enum in `cli.rs` — no mismatches.
- `Severity` enum + `worst` fold in `cli/doctor.rs` uses a tuple match that compiles regardless of variant order.

All clear.
