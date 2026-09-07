//! Config loading for the `lab` binary.
//!
//! Order of precedence (highest wins):
//!   1. CLI flags / process environment variables
//!   2. `$LABBY_HOME/.env` (normally `~/.labby/.env`, loaded via `dotenvy`)
//!   3. `$LABBY_HOME/config.toml` (normally `~/.labby/config.toml`)
//!   4. Built-in defaults
//!
//! Service credentials and instance endpoints belong in `.env`. Non-secret
//! operator preferences and defaults (logging, CORS, MCP transport, admin
//! flags and workspace roots belong in `config.toml`.
//!
//! Multi-instance services follow the `S_<LABEL>_URL` pattern: a service
//! like `unraid` reads `UNRAID_URL` as the default instance and
//! `UNRAID_NODE2_URL` as an additional instance labeled `node2`.

pub mod depot;
#[cfg(test)]
mod depot_tests;
pub mod env_merge;
mod env_writer;
pub mod host_write;
#[cfg(test)]
mod host_write_tests;
mod paths;
pub(crate) mod secret_files;

pub use env_writer::{EnvCredential, write_env_pairs, write_service_creds};
#[cfg(test)]
use paths::resolve_usage_telemetry_enabled;
pub(crate) use paths::{access_db_path, file_stash_root_path, home_dir};
pub use paths::{
    codemode_journal_db_path, codemode_journal_enabled, config_toml_path, dotenv_path,
    toml_candidates, usage_db_path, usage_telemetry_enabled, workspace_root_for_home,
    workspace_root_path,
};
pub use secret_files::heal_env_file_permissions;

#[cfg(test)]
use std::sync::atomic::AtomicU8;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Mutex, OnceLock};
use std::{
    collections::BTreeMap,
    collections::HashMap,
    fs::OpenOptions,
    io::Write as _,
    path::{Path, PathBuf},
    time::Duration,
};

// Gateway startup/reload writes this process-wide flag whenever root
// `[code_mode]` changes. In-process peer MCP servers do not hold a
// GatewayManager, but they must still hide raw built-in tools when the root
// server is operating in Code Mode.
static PROCESS_CODE_MODE_ENABLED: AtomicBool = AtomicBool::new(false);
pub const CURRENT_CONFIG_VERSION: u32 = 1;

const fn current_config_version() -> u32 {
    CURRENT_CONFIG_VERSION
}

#[cfg(test)]
static PROCESS_CODE_MODE_TEST_LOCK: Mutex<()> = Mutex::new(());
#[cfg(test)]
static PROCESS_CODE_MODE_TEST_OVERRIDE: AtomicU8 = AtomicU8::new(0);

#[cfg(test)]
pub(crate) struct ProcessCodeModeTestGuard {
    _lock: std::sync::MutexGuard<'static, ()>,
    previous: bool,
}

#[cfg(test)]
impl Drop for ProcessCodeModeTestGuard {
    fn drop(&mut self) {
        set_process_code_mode_enabled(self.previous);
        PROCESS_CODE_MODE_TEST_OVERRIDE.store(0, Ordering::Release);
    }
}

#[cfg(test)]
pub(crate) fn process_code_mode_test_guard() -> ProcessCodeModeTestGuard {
    let lock = PROCESS_CODE_MODE_TEST_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let previous = PROCESS_CODE_MODE_ENABLED.load(Ordering::Acquire);
    PROCESS_CODE_MODE_TEST_OVERRIDE.store(u8::from(previous) + 1, Ordering::Release);
    ProcessCodeModeTestGuard {
        _lock: lock,
        previous,
    }
}

pub(crate) fn set_process_code_mode_enabled(enabled: bool) {
    let previous = PROCESS_CODE_MODE_ENABLED.swap(enabled, Ordering::AcqRel);
    if previous != enabled {
        tracing::info!(
            surface = "mcp",
            service = "code_mode",
            action = "code_mode.process_enablement",
            previous_enabled = previous,
            enabled,
            "process-wide code mode enablement changed"
        );
    }
}

#[cfg(test)]
pub(crate) fn set_process_code_mode_enabled_for_test(enabled: bool) {
    set_process_code_mode_enabled(enabled);
    PROCESS_CODE_MODE_TEST_OVERRIDE.store(u8::from(enabled) + 1, Ordering::Release);
}

pub(crate) fn process_code_mode_enabled() -> bool {
    #[cfg(test)]
    if let Some(enabled) = match PROCESS_CODE_MODE_TEST_OVERRIDE.load(Ordering::Acquire) {
        1 => Some(false),
        2 => Some(true),
        _ => None,
    } {
        return enabled;
    }
    PROCESS_CODE_MODE_ENABLED.load(Ordering::Acquire)
}

/// Parse a boolean env flag using the standard truthy set
/// (`1` / `true` / `TRUE` / `yes` / `YES`). Absent or any other value is false.
pub(crate) fn env_flag_enabled(name: &str) -> bool {
    std::env::var(name)
        .ok()
        .is_some_and(|value| matches!(value.as_str(), "1" | "true" | "TRUE" | "yes" | "YES"))
}

fn parse_bounded_ms(raw: &str, max: u64) -> Option<u64> {
    raw.parse::<u64>()
        .ok()
        .filter(|value| (1..=max).contains(value))
}

fn parse_bounded_ms_env(name: &str, raw: &str, max: u64) -> Option<u64> {
    let parsed = parse_bounded_ms(raw, max);
    if parsed.is_none() {
        tracing::warn!(
            env_var = name,
            value = raw,
            max_ms = max,
            "ignoring invalid millisecond timeout environment variable; expected 1..=max_ms"
        );
    }
    parsed
}

/// Whether mcp-ui widget -> host tool callbacks are permitted while the Code
/// Mode synthetic surface (`codemode`) is active.
///
/// Default: **off**. When the synthetic surface is on, raw upstream tools are
/// hidden from `list_tools` and normally not callable by name. Setting
/// `LABBY_CODE_MODE_WIDGET_CALLBACKS=1` (or `true`/`yes`) lets a rendered widget's
/// callback reach the upstream proxy by tool name — the tool stays out of
/// `list_tools`, so this only relaxes callability, never visibility. Operators
/// opt in knowingly because it also lets any caller on the session (including
/// the model) invoke a known upstream tool by name.
pub(crate) fn code_mode_widget_callbacks_enabled() -> bool {
    resolved_widget_callbacks_enabled()
}

// ─── Resolved config.toml/env preferences, process-wide ───────────────────
//
// These vars are read from deep call sites (tool dispatch, CLI theming, HTTP
// state construction) that don't have a `&LabConfig` in scope. Rather than
// thread a config reference through every caller, resolve config.toml +
// env-var precedence once at startup and cache the result process-wide,
// mirroring the existing `PROCESS_CODE_MODE_ENABLED` pattern above. Plain
// atomics/mutexes (not `OnceLock`) so tests can freely re-resolve.

static RESOLVED_SHOW_ALL: AtomicBool = AtomicBool::new(false);
static RESOLVED_DEV_MODE: AtomicBool = AtomicBool::new(false);
static RESOLVED_WIDGET_CALLBACKS: AtomicBool = AtomicBool::new(false);
static RESOLVED_INSTALL_ANDROID_SDK: AtomicBool = AtomicBool::new(false);
static RESOLVED_SYMBOLS: OnceLock<Mutex<Option<String>>> = OnceLock::new();
static RESOLVED_PROTECTED_MCP_TIMEOUT_SECS: OnceLock<Mutex<Option<u64>>> = OnceLock::new();
static RESOLVED_CATALOG_NOTIFICATION_TIMEOUT_MS: OnceLock<Mutex<Option<u64>>> = OnceLock::new();

fn resolved_symbols_cell() -> &'static Mutex<Option<String>> {
    RESOLVED_SYMBOLS.get_or_init(|| Mutex::new(None))
}

fn resolved_protected_mcp_timeout_cell() -> &'static Mutex<Option<u64>> {
    RESOLVED_PROTECTED_MCP_TIMEOUT_SECS.get_or_init(|| Mutex::new(None))
}

fn resolved_catalog_notification_timeout_cell() -> &'static Mutex<Option<u64>> {
    RESOLVED_CATALOG_NOTIFICATION_TIMEOUT_MS.get_or_init(|| Mutex::new(None))
}

/// Resolve config.toml + env-var precedence for the small set of
/// preferences read from call sites without direct config access, and cache
/// the result process-wide. Call once, early, right after `config.toml`
/// loads (before `.env` loads and before dispatch) — see `entrypoint.rs`.
pub(crate) fn install_resolved_preferences(config: &LabConfig) {
    RESOLVED_SHOW_ALL.store(
        env_flag_enabled("LABBY_SHOW_ALL") || config.mcp.show_all.unwrap_or(false),
        Ordering::Release,
    );
    RESOLVED_DEV_MODE.store(
        std::env::var("LABBY_DEV_MODE").as_deref() == Ok("1")
            || config.api.dev_mode.unwrap_or(false),
        Ordering::Release,
    );
    RESOLVED_WIDGET_CALLBACKS.store(
        env_flag_enabled("LABBY_CODE_MODE_WIDGET_CALLBACKS")
            || config.code_mode.widget_callbacks.unwrap_or(false),
        Ordering::Release,
    );
    RESOLVED_INSTALL_ANDROID_SDK.store(
        env_flag_enabled("LABBY_ENABLE_ANDROID_SDK")
            || config.setup.install_android_sdk.unwrap_or(false),
        Ordering::Release,
    );
    let symbols = std::env::var("LABBY_SYMBOLS")
        .ok()
        .or_else(|| config.output.symbols.clone());
    *resolved_symbols_cell()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner) = symbols;
    let protected_mcp_timeout_secs = std::env::var("LABBY_PROTECTED_MCP_CONNECT_TIMEOUT_SECS")
        .ok()
        .and_then(|v| v.parse::<u64>().ok())
        .or(config.api.protected_mcp_connect_timeout_secs);
    *resolved_protected_mcp_timeout_cell()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner) = protected_mcp_timeout_secs;
    let catalog_notification_timeout_ms =
        std::env::var("LABBY_MCP_CATALOG_NOTIFICATION_TIMEOUT_MS")
            .ok()
            .and_then(|raw| {
                parse_bounded_ms_env(
                    "LABBY_MCP_CATALOG_NOTIFICATION_TIMEOUT_MS",
                    &raw,
                    MAX_CATALOG_NOTIFICATION_TIMEOUT_MS,
                )
            })
            .or(config.mcp.catalog_notification_timeout_ms);
    *resolved_catalog_notification_timeout_cell()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner) = catalog_notification_timeout_ms;
}

pub(crate) fn resolved_show_all() -> bool {
    RESOLVED_SHOW_ALL.load(Ordering::Acquire)
}

pub(crate) fn resolved_dev_mode() -> bool {
    RESOLVED_DEV_MODE.load(Ordering::Acquire)
}

pub(crate) fn resolved_widget_callbacks_enabled() -> bool {
    RESOLVED_WIDGET_CALLBACKS.load(Ordering::Acquire)
}

/// Resolved "install android-sdk during provision" flag, folding
/// `LABBY_ENABLE_ANDROID_SDK=1` env over `[setup].install_android_sdk` config.
/// Read by the provision plan builder (`ActionKind::AndroidSdk`).
pub(crate) fn resolved_install_android_sdk() -> bool {
    RESOLVED_INSTALL_ANDROID_SDK.load(Ordering::Acquire)
}

pub(crate) fn resolved_symbols() -> Option<String> {
    resolved_symbols_cell()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .clone()
}

pub(crate) fn resolved_protected_mcp_connect_timeout_secs() -> Option<u64> {
    *resolved_protected_mcp_timeout_cell()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

pub(crate) fn resolved_catalog_notification_timeout() -> Duration {
    Duration::from_millis(
        resolved_catalog_notification_timeout_cell()
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .unwrap_or(DEFAULT_CATALOG_NOTIFICATION_TIMEOUT_MS),
    )
}

use anyhow::{Context, Result};
use labby_auth::config as auth_config;
use serde::{Deserialize, Serialize, Serializer};

pub const WEB_UI_AUTH_DISABLED_ENV: &str = "LABBY_WEB_UI_AUTH_DISABLED";
pub const WEB_UI_AUTH_DISABLED_LEGACY_ENV: &str = "LABBY_WEB_UI_DISABLE_AUTH";
const DEFAULT_UPSTREAM_REQUEST_TIMEOUT_MS: u64 = 30_000;
const DEFAULT_CATALOG_NOTIFICATION_TIMEOUT_MS: u64 = 5_000;
const MAX_CATALOG_NOTIFICATION_TIMEOUT_MS: u64 = 60_000;
/// Default deadline for a *relayed* upstream tool call (see
/// [`LabConfig::upstream_relay_timeout`]).
///
/// Relayed calls carry a human-in-the-loop round trip — the upstream raises an
/// `elicitation/create` that is forwarded to the downstream agent and answered
/// by a person — so the ordinary 30s `upstream_request_timeout` would abort
/// legitimate confirmations. The relay deadline defaults to 5 minutes to give a
/// human time to respond while still bounding the dedicated connection's
/// lifetime. Only the relay path uses this; the pooled hot path keeps
/// [`DEFAULT_UPSTREAM_REQUEST_TIMEOUT_MS`].
const DEFAULT_UPSTREAM_RELAY_TIMEOUT_MS: u64 = 300_000;
/// Headroom added to the longest configured upstream deadline when deriving the
/// hosted HTTP transport timeout (see [`LabConfig::http_request_timeout`]).
///
/// Covers the work bracketing the upstream call itself — auth, Code Mode
/// compilation, connection-pool checkout, response serialization — so the inner
/// deadline is always the one that fires on a slow upstream.
const HTTP_REQUEST_TIMEOUT_MARGIN: Duration = Duration::from_secs(30);
const CONFIG_BACKUP_RETENTION: usize = 10;
const CONFIG_BACKUP_MAX_AGE: Duration = Duration::from_hours(30 * 24);
const CONFIG_BACKUP_MAX_BYTES: u64 = 64 * 1024 * 1024;

#[derive(Clone, Debug)]
struct ConfigBackupCandidate {
    path: PathBuf,
    modified: std::time::SystemTime,
    bytes: u64,
}

#[derive(Clone, Copy, Debug)]
struct ConfigBackupRetention {
    max_count: usize,
    max_age: Duration,
    max_bytes: u64,
}

#[cfg(test)]
impl ConfigBackupCandidate {
    fn fixture(path: &str, bytes: u64, modified: std::time::SystemTime) -> Self {
        Self {
            path: PathBuf::from(path),
            modified,
            bytes,
        }
    }
}

#[cfg(test)]
static TEST_CONFIG_TOML_PATH: OnceLock<Mutex<Option<PathBuf>>> = OnceLock::new();

#[cfg(test)]
pub(crate) fn set_test_config_toml_path(path: Option<PathBuf>) {
    let slot = TEST_CONFIG_TOML_PATH.get_or_init(|| Mutex::new(None));
    *slot.lock().expect("test config path lock") = path;
}

/// Fully-resolved `lab` configuration, assembled from env + TOML.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LabConfig {
    /// Persisted configuration schema version. Missing legacy values migrate to v1.
    #[serde(default = "current_config_version")]
    pub config_version: u32,
    /// Instance-shared Depot discovery providers, independent of acquisition.
    #[serde(default)]
    pub depot: depot::DepotPreferences,
    /// Default output format for CLI commands that print tables.
    #[serde(default)]
    pub output: OutputPreferences,
    /// MCP server defaults.
    #[serde(default)]
    pub mcp: McpPreferences,
    /// Ephemeral stdio MCP proxy defaults.
    #[serde(default)]
    pub proxy: crate::proxy::config::ProxyPreferences,
    /// Logging preferences (overridden by `LABBY_LOG` / `LABBY_LOG_FORMAT` env vars).
    #[serde(default)]
    pub log: LogPreferences,
    /// Local Labby server-log subsystem preferences.
    #[serde(default)]
    pub local_logs: Option<LocalLogsPreferences>,
    /// HTTP API preferences.
    #[serde(default)]
    pub api: ApiPreferences,
    /// Web UI preferences.
    #[serde(default)]
    pub web: WebPreferences,
    /// Shared Labby workspace root for the optional filesystem browser.
    #[serde(default)]
    pub workspace: WorkspacePreferences,
    /// Principal-scoped durable File Stash storage.
    #[serde(default)]
    pub file_stash: FileStashPreferences,
    /// OAuth callback relay preferences.
    #[serde(default)]
    pub oauth: OauthPreferences,
    /// Admin tool settings.
    #[serde(default)]
    pub admin: AdminPreferences,
    /// Per-service preference overrides.
    #[serde(default)]
    pub services: ServicePreferences,
    /// Setup/provision preferences (operator toggles for `labby setup --provision`).
    #[serde(default)]
    pub setup: SetupPreferences,
    /// HTTP auth mode preferences.
    #[serde(default)]
    pub auth: Option<AuthFileConfig>,
    /// Gateway-wide Code Mode exposure and execution settings.
    #[serde(default)]
    pub code_mode: CodeModeConfig,
    /// Visibility of Labby-owned MCP App surfaces other than Code Mode.
    #[serde(default)]
    pub mcp_apps: McpAppsConfig,
    /// Optional server-held exact-revision Skill acquisition connections.
    #[serde(default)]
    pub artifacts: ArtifactPreferences,
    /// Maximum time to wait for one proxied upstream MCP tool/resource/prompt response.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub upstream_request_timeout_ms: Option<u64>,
    /// Maximum time to wait for one proxied MRTR-capable upstream tool call.
    /// This path preserves `input_required` responses for the downstream
    /// client and gets its own longer deadline (default 5 minutes; see
    /// [`LabConfig::upstream_relay_timeout`]).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub upstream_relay_timeout_ms: Option<u64>,
    /// Upstream MCP servers to proxy through the gateway.
    #[serde(default)]
    pub upstream: Vec<UpstreamConfig>,
    /// Imported upstreams removed by an operator. Auto-import honors this list
    /// so deleted external-config entries do not immediately return on restart.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub upstream_import_tombstones: Vec<UpstreamImportTombstone>,
    /// Discovered upstreams waiting for operator approval. Populated when
    /// `gateway_import_mode = "pending"`. Empty when mode is `"off"` or `"auto"`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub upstream_pending: Vec<UpstreamConfig>,
    /// Controls how external MCP config discovery behaves on startup.
    /// - `"off"` (default): discovery is disabled; no auto-import.
    /// - `"pending"`: discover on startup, queue for approval — never auto-apply.
    /// - `"auto"`: auto-import everything not tombstoned (legacy behavior).
    #[serde(default)]
    pub gateway_import_mode: GatewayImportMode,
    /// Named reusable gateway capability projections.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub loadouts: Vec<GatewayLoadoutConfig>,
    /// Public HTTP MCP routes protected by Lab OAuth and proxied by Lab.
    ///
    /// These are intentionally separate from `upstream`: upstreams import tools
    /// into Lab, while protected MCP routes expose a backend MCP server through
    /// Lab as an OAuth resource server.
    #[serde(default)]
    pub protected_mcp_routes: Vec<ProtectedMcpRouteConfig>,
    /// Virtual MCP servers backed by canonically configured Lab services.
    #[serde(default)]
    pub virtual_servers: Vec<VirtualServerConfig>,
    /// Virtual servers whose backing service is no longer registered in this binary.
    #[serde(default)]
    pub quarantined_virtual_servers: Vec<VirtualServerConfig>,
    /// Canonical public URL model for the app and MCP gateway.
    ///
    /// Use [`LabConfig::public_urls()`] to read resolved values with env-var
    /// precedence rather than accessing this field directly.
    #[serde(default)]
    pub public_urls: Option<PublicUrlsConfig>,
    /// Gateway spawn-guard and command-allowlist preferences.
    #[serde(default)]
    pub gateway: GatewayPreferences,
    /// Code Mode `openapi` local-provider spec configuration.
    ///
    /// Non-secret only (spec URL/path, label, mandatory base_url, allowlist);
    /// credentials are read from `OPENAPI_<LABEL>_*` env vars, never TOML.
    #[serde(default)]
    pub openapi: OpenApiTomlSection,
}

impl Default for LabConfig {
    fn default() -> Self {
        toml::from_str("").expect("the empty built-in LabConfig must deserialize")
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ArtifactPreferences {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub sources: Vec<ArtifactSourceConfig>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ArtifactSourceConfig {
    pub id: String,
    pub kind: ArtifactSourceKind,
    /// Exact-revision acquisition endpoint used by `artifacts.import`.
    pub endpoint: String,
    /// Depot HTTP origin used for curated operations and raw uploads.
    ///
    /// This is deliberately separate from `endpoint`: the latter accepts the
    /// exact-acquisition POST contract, while this value is an origin to which
    /// Labby appends fixed `/api/operations/...` and `/uploads/...` paths.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub control_plane_url: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub pinned_addresses: Vec<std::net::IpAddr>,
    /// Name of an environment variable containing the server-held bearer secret.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bearer_token_env: Option<String>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ArtifactSourceKind {
    Depot,
    Repository,
}

/// `[openapi]` config section: a list of `[[openapi.specs]]` tables.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OpenApiTomlSection {
    /// Configured specs.
    #[serde(default)]
    pub specs: Vec<OpenApiSpecToml>,
}

/// One `[[openapi.specs]]` table. Non-secret fields only.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OpenApiSpecToml {
    /// Provider label (`openapi::<label>.<operationId>`).
    #[serde(default)]
    pub label: String,
    /// Mandatory base URL for outbound requests (validated at load time).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base_url: Option<String>,
    /// Spec document URL (mutually exclusive with `spec_path`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub spec_url: Option<String>,
    /// Spec document filesystem path (mutually exclusive with `spec_url`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub spec_path: Option<String>,
    /// Header name for `OPENAPI_<LABEL>_API_KEY` injection (default `X-API-Key`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub api_key_header: Option<String>,
    /// Deny-by-default allowlist of raw operationIds.
    #[serde(default)]
    pub allowed_operations: Vec<String>,
}

// `GatewayPreferences` moved to `labby_runtime::gateway_config`; re-exported above.

impl LabConfig {
    /// Resolve the canonical public URL pair after env-over-config merge.
    ///
    /// Precedence (highest wins):
    ///   1. `LABBY_PUBLIC_URL` env var (app), `LABBY_MCP_GATEWAY_URL` env var (gateway)
    ///   2. `config.toml` `[public_urls]` section
    ///   3. Legacy `[auth].public_url` field (app only, for backward compat)
    pub fn public_urls(&self) -> ResolvedPublicUrls {
        // Env wins
        let env_app = std::env::var("LABBY_PUBLIC_URL")
            .ok()
            .filter(|v| !v.is_empty());
        let env_gw = std::env::var("LABBY_MCP_GATEWAY_URL")
            .ok()
            .filter(|v| !v.is_empty());

        let app = env_app
            .or_else(|| self.public_urls.as_ref().and_then(|p| p.app.clone()))
            .or_else(|| {
                // Backward compat: fall back to [auth].public_url
                self.auth.as_ref().and_then(|a| a.public_url.clone())
            });

        let mcp_gateway = env_gw.or_else(|| {
            self.public_urls
                .as_ref()
                .and_then(|p| p.mcp_gateway.clone())
        });

        ResolvedPublicUrls { app, mcp_gateway }
    }

    /// Project the gateway-relevant slice of this config into the surface-neutral
    /// [`GatewayConfig`] DTO the `GatewayManager` owns in memory.
    #[must_use]
    pub fn to_gateway_config(&self) -> GatewayConfig {
        GatewayConfig {
            code_mode: self.code_mode.clone(),
            mcp_apps: self.mcp_apps,
            upstream_request_timeout_ms: self.upstream_request_timeout_ms,
            upstream_relay_timeout_ms: self.upstream_relay_timeout_ms,
            upstream: self.upstream.clone(),
            upstream_import_tombstones: self.upstream_import_tombstones.clone(),
            upstream_pending: self.upstream_pending.clone(),
            loadouts: self.loadouts.clone(),
            protected_mcp_routes: self.protected_mcp_routes.clone(),
            virtual_servers: self.virtual_servers.clone(),
            quarantined_virtual_servers: self.quarantined_virtual_servers.clone(),
            gateway: self.gateway.clone(),
        }
    }

    /// Overwrite the gateway-owned sections of this config from `gw`, leaving
    /// every non-gateway section (and any foreign top-level keys preserved by
    /// the toml_edit render path) untouched.
    pub fn apply_gateway_config(&mut self, gw: &GatewayConfig) {
        self.code_mode = gw.code_mode.clone();
        self.mcp_apps = gw.mcp_apps;
        self.upstream_request_timeout_ms = gw.upstream_request_timeout_ms;
        self.upstream_relay_timeout_ms = gw.upstream_relay_timeout_ms;
        self.upstream = gw.upstream.clone();
        self.upstream_import_tombstones = gw.upstream_import_tombstones.clone();
        self.upstream_pending = gw.upstream_pending.clone();
        self.loadouts = gw.loadouts.clone();
        self.protected_mcp_routes = gw.protected_mcp_routes.clone();
        self.virtual_servers = gw.virtual_servers.clone();
        self.quarantined_virtual_servers = gw.quarantined_virtual_servers.clone();
        self.gateway = gw.gateway.clone();
    }
}

impl From<&LabConfig> for GatewayConfig {
    fn from(cfg: &LabConfig) -> Self {
        cfg.to_gateway_config()
    }
}

impl LabConfig {
    pub fn validate(&self) -> Result<(), ConfigError> {
        if self.config_version != CURRENT_CONFIG_VERSION {
            return Err(ConfigError::InvalidProxyConfig {
                reason: format!(
                    "config_version {} is unsupported; expected {}",
                    self.config_version, CURRENT_CONFIG_VERSION
                ),
            });
        }
        self.code_mode.validate()?;
        self.file_stash.validate()?;
        self.proxy
            .validate()
            .map_err(|error| ConfigError::InvalidProxyConfig {
                reason: error.to_string(),
            })?;
        if let Some(value) = self.upstream_request_timeout_ms
            && !(1..=300_000).contains(&value)
        {
            return Err(ConfigError::InvalidUpstreamRequestTimeout { value });
        }
        // The relay deadline allows a wider ceiling (30 min) than the pooled
        // request timeout because it spans a human answering an elicitation.
        if let Some(value) = self.upstream_relay_timeout_ms
            && !(1..=1_800_000).contains(&value)
        {
            return Err(ConfigError::InvalidUpstreamRelayTimeout { value });
        }
        if let Some(value) = self.mcp.catalog_notification_timeout_ms
            && !(1..=MAX_CATALOG_NOTIFICATION_TIMEOUT_MS).contains(&value)
        {
            return Err(ConfigError::InvalidCatalogNotificationTimeout { value });
        }
        for upstream in &self.upstream {
            upstream.validate()?;
        }
        validate_protected_mcp_routes_for_startup(self)?;
        Ok(())
    }

    pub fn upstream_request_timeout(&self) -> Duration {
        Duration::from_millis(
            self.upstream_request_timeout_ms
                .unwrap_or(DEFAULT_UPSTREAM_REQUEST_TIMEOUT_MS),
        )
    }

    /// Deadline for a single *relayed* upstream tool call.
    ///
    /// Distinct from [`Self::upstream_request_timeout`] because the relay path
    /// blocks on a human answering an elicitation forwarded from the upstream;
    /// reusing the 30s request timeout would abort real confirmations. Defaults
    /// to `DEFAULT_UPSTREAM_RELAY_TIMEOUT_MS` (5 minutes) when unset.
    pub fn upstream_relay_timeout(&self) -> Duration {
        Duration::from_millis(
            self.upstream_relay_timeout_ms
                .unwrap_or(DEFAULT_UPSTREAM_RELAY_TIMEOUT_MS),
        )
    }

    /// Transport-level deadline for one hosted HTTP request.
    ///
    /// This is a backstop for requests that outlive every inner deadline, not a
    /// product timeout. It is derived from the configured upstream deadlines so
    /// it can never fire *before* the timeout it is supposed to wrap: a request
    /// that exceeds `upstream_request_timeout` / `upstream_relay_timeout` must
    /// fail with a structured MCP error from the dispatch layer, not a bare 504
    /// from the HTTP stack.
    ///
    /// A fixed cap here previously overrode both settings — an operator raising
    /// `upstream_request_timeout_ms` past 30s got no effect, because the
    /// transport killed the response first and discarded a tool call that had
    /// already succeeded.
    pub fn http_request_timeout(&self) -> Duration {
        self.upstream_request_timeout()
            .max(self.upstream_relay_timeout())
            .saturating_add(HTTP_REQUEST_TIMEOUT_MARGIN)
    }

    pub fn normalize_protected_mcp_routes(&mut self) -> Result<(), ConfigError> {
        for route in &mut self.protected_mcp_routes {
            route.upstream = route
                .upstream
                .take()
                .map(|name| name.trim().to_string())
                .filter(|name| !name.is_empty());
            if let Some(ProtectedMcpRouteTarget::GatewaySubset(target)) = &mut route.target {
                if let Some(project_id) = target.project_id.take() {
                    let project_id = project_id.trim().to_string();
                    if !labby_runtime::gateway_config::is_canonical_project_id(&project_id) {
                        return Err(ConfigError::InvalidProtectedRoute {
                            name: route.name.clone(),
                            field: "target.project_id",
                            value: "project binding must not be empty".to_string(),
                        });
                    }
                    target.project_id = Some(project_id);
                }
                target.loadout = target
                    .loadout
                    .take()
                    .map(|name| name.trim().to_string())
                    .filter(|name| !name.is_empty());
                normalize_string_list(&mut target.upstreams, "target.upstreams").map_err(
                    |field| ConfigError::InvalidProtectedRoute {
                        name: route.name.clone(),
                        field,
                        value: "gateway_subset target entries must not be empty".to_string(),
                    },
                )?;
                normalize_string_list(&mut target.services, "target.services").map_err(
                    |field| ConfigError::InvalidProtectedRoute {
                        name: route.name.clone(),
                        field,
                        value: "gateway_subset target entries must not be empty".to_string(),
                    },
                )?;
                // Mirrors the identical guard in
                // `labby_runtime::gateway_config::GatewayConfig::normalize_protected_mcp_routes`.
                // THIS is the copy `load_toml` runs, and therefore the copy the
                // mounted route scopes in `cli/serve.rs` are built from — the
                // runtime copy alone left the guard off the serve path
                // (review finding on lab-eyeuv). Keep the two in sync.
                if let Some(reserved) = target
                    .upstreams
                    .iter()
                    .find(|name| name.starts_with(IN_PROCESS_UPSTREAM_PREFIX))
                {
                    return Err(ConfigError::InvalidProtectedRoute {
                        name: route.name.clone(),
                        field: "target.upstreams",
                        value: format!(
                            "`{reserved}` uses the reserved `{IN_PROCESS_UPSTREAM_PREFIX}` \
                             prefix; built-in service peers cannot be routed to a protected \
                             subset — list the service under `target.services` instead"
                        ),
                    });
                }
            }
            if route.target.is_some()
                && (route.upstream.is_some() || !route.backend_url.trim().is_empty())
            {
                return Err(ConfigError::InvalidProtectedRoute {
                    name: route.name.clone(),
                    field: "target",
                    value:
                        "protected MCP route target cannot be combined with upstream or backend_url"
                            .to_string(),
                });
            }
            if route.target.is_some() {
                route.backend_url = String::new();
                route.backend_mcp_path = default_mcp_path();
                continue;
            }
            if route.upstream.is_some() && route.backend_url.trim().is_empty() {
                route.backend_url = String::new();
            } else {
                route.backend_url =
                    normalize_protected_backend_url(&route.backend_url, &route.backend_mcp_path)
                        .map_err(|_| ConfigError::InvalidProtectedRoute {
                            name: route.name.clone(),
                            field: "backend_url",
                            value: route.backend_url.clone(),
                        })?;
            }
            route.backend_mcp_path = default_mcp_path();
        }
        Ok(())
    }
}

fn normalize_string_list(
    values: &mut Vec<String>,
    field: &'static str,
) -> Result<(), &'static str> {
    let mut normalized = Vec::new();
    for value in std::mem::take(values) {
        let name = value.trim().to_string();
        if name.is_empty() {
            return Err(field);
        }
        if !normalized.contains(&name) {
            normalized.push(name);
        }
    }
    *values = normalized;
    Ok(())
}

fn validate_protected_mcp_routes_for_startup(cfg: &LabConfig) -> Result<(), ConfigError> {
    let mut names = std::collections::HashSet::new();
    let mut enabled_keys = std::collections::HashSet::new();
    let upstream_names: std::collections::HashSet<&str> = cfg
        .upstream
        .iter()
        .map(|upstream| upstream.name.as_str())
        .collect();
    let registry = crate::registry::build_docs_registry();
    let service_names: std::collections::HashSet<&str> = registry
        .services()
        .iter()
        .filter(|service| registry.supports_context_free_dispatch(service.name))
        .map(|service| service.name)
        .collect();
    let loadout_names: std::collections::HashSet<&str> = cfg
        .loadouts
        .iter()
        .map(|loadout| loadout.name.as_str())
        .collect();

    for route in &cfg.protected_mcp_routes {
        validate_protected_mcp_route_for_startup(
            route,
            &upstream_names,
            &service_names,
            &loadout_names,
        )?;
        if !names.insert(route.name.trim().to_string()) {
            return Err(ConfigError::InvalidProtectedRoute {
                name: route.name.clone(),
                field: "name",
                value: format!(
                    "protected MCP route `{}` appears more than once",
                    route.name
                ),
            });
        }
        if route.enabled {
            let key = (
                route.public_host.trim().to_ascii_lowercase(),
                route.public_path.trim().to_string(),
            );
            if !enabled_keys.insert(key) {
                return Err(ConfigError::InvalidProtectedRoute {
                    name: route.name.clone(),
                    field: "public_path",
                    value: format!(
                        "duplicate enabled protected MCP route for {}{}",
                        route.public_host, route.public_path
                    ),
                });
            }
        }
    }
    Ok(())
}

fn validate_protected_mcp_route_for_startup(
    route: &ProtectedMcpRouteConfig,
    upstream_names: &std::collections::HashSet<&str>,
    service_names: &std::collections::HashSet<&str>,
    loadout_names: &std::collections::HashSet<&str>,
) -> Result<(), ConfigError> {
    if route.name.trim().is_empty() {
        return invalid_protected_route(
            route,
            "name",
            "protected MCP route name must not be empty",
        );
    }
    validate_protected_public_path_for_startup(route, route.public_path.trim())?;
    if route.target.is_some() && (route.upstream.is_some() || !route.backend_url.trim().is_empty())
    {
        return invalid_protected_route(
            route,
            "target",
            "protected MCP route target cannot be combined with upstream or backend_url",
        );
    }

    if let Some(ProtectedMcpRouteTarget::GatewaySubset(target)) = &route.target {
        if target.loadout.is_some()
            && (!target.upstreams.is_empty()
                || !target.services.is_empty()
                || target.expose_code_mode)
        {
            return invalid_protected_route(
                route,
                "target.loadout",
                "gateway_subset target with `loadout` cannot also set inline upstreams, services, or expose_code_mode",
            );
        }
        if target.loadout.is_none()
            && target.upstreams.is_empty()
            && target.services.is_empty()
            && !target.expose_code_mode
        {
            return invalid_protected_route(
                route,
                "target",
                "gateway_subset target must set a loadout or expose at least one upstream, service, or Code Mode",
            );
        }
        if let Some(loadout) = target.loadout.as_deref()
            && !loadout_names.contains(loadout)
        {
            return invalid_protected_route(
                route,
                "target.loadout",
                format!("unknown gateway_subset loadout `{loadout}`"),
            );
        }
        if route.enabled {
            for upstream in &target.upstreams {
                if !upstream_names.contains(upstream.as_str()) {
                    return invalid_protected_route(
                        route,
                        "target.upstreams",
                        format!("unknown gateway_subset upstream `{upstream}`"),
                    );
                }
            }
            for service in &target.services {
                if !service_names.contains(service.as_str()) {
                    return invalid_protected_route(
                        route,
                        "target.services",
                        format!("unknown gateway_subset service `{service}`"),
                    );
                }
            }
        }
        return Ok(());
    }

    match (
        route.upstream.as_deref(),
        route.backend_url.trim().is_empty(),
    ) {
        (Some(_), true) | (None, false) => Ok(()),
        (Some(_), false) => invalid_protected_route(
            route,
            "upstream",
            "protected MCP route must set either upstream or backend_url, not both",
        ),
        (None, true) => invalid_protected_route(
            route,
            "backend_url",
            "protected MCP route must set upstream or backend_url",
        ),
    }
}

fn validate_protected_public_path_for_startup(
    route: &ProtectedMcpRouteConfig,
    path: &str,
) -> Result<(), ConfigError> {
    if path == "/" {
        return invalid_protected_route(
            route,
            "public_path",
            "public_path must include a service segment",
        );
    }
    let lower = path.to_ascii_lowercase();
    if lower.starts_with("/.well-known")
        || lower.starts_with("/v1")
        || crate::oauth::public_relay::is_reserved_public_relay_path(path)
    {
        return invalid_protected_route(
            route,
            "public_path",
            "public_path conflicts with Lab reserved routes",
        );
    }
    if lower.contains("%2f")
        || lower.contains("%5c")
        || lower.contains("%2e")
        || path.contains('\\')
        || path
            .split('/')
            .any(|segment| segment == "." || segment == "..")
        || path.contains("//")
    {
        return invalid_protected_route(
            route,
            "public_path",
            "public_path contains unsafe or ambiguous path segments",
        );
    }
    Ok(())
}

fn invalid_protected_route(
    route: &ProtectedMcpRouteConfig,
    field: &'static str,
    value: impl Into<String>,
) -> Result<(), ConfigError> {
    Err(ConfigError::InvalidProtectedRoute {
        name: route.name.clone(),
        field,
        value: value.into(),
    })
}

// Gateway config DTOs and their dependency closure now live in
// `labby_runtime::gateway_config`. They are re-exported below so the rest of
// this module and all external callers keep their existing import paths.
// Serde shape (defaults, renames, skip rules) is preserved exactly there.
// Some entries are only referenced from tests after the gateway runtime moved to
// `labby-gateway`; keep them as the public `labby::config` surface and silence the
// bin-target unused-import lint.
#[allow(unused_imports)]
pub use labby_runtime::gateway_config::IN_PROCESS_UPSTREAM_PREFIX;
pub use labby_runtime::gateway_config::{
    CodeModeConfig, CodeModeResultShapePolicy, ConfigError, GatewayConfig, GatewayImportMode,
    GatewayLoadoutConfig, GatewayPreferences, ImportSource, McpAppsConfig,
    ProtectedGatewaySubsetTarget, ProtectedMcpRouteConfig, ProtectedMcpRouteEffectiveTarget,
    ProtectedMcpRouteTarget, ResolvedPublicUrls, UpstreamConfig, UpstreamImportTombstone,
    UpstreamOauthConfig, UpstreamOauthCredentialSource, UpstreamOauthMode,
    UpstreamOauthRegistration, VirtualServerConfig, VirtualServerMcpPolicyConfig,
    VirtualServerSurfacesConfig, WebPreferences, default_mcp_path, default_true,
    normalize_protected_backend_url,
};
// Re-exported for the public `labby::config` API surface (consumed by the
// `upstream_oauth` integration test); not referenced within the binary build,
// so silence the bin-target unused-import lint.
#[allow(unused_imports)]
pub use labby_runtime::gateway_config::canonicalize_upstream_url;

/// Table/json formatting defaults.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OutputPreferences {
    /// Default format: `human` or `json`. Honored unless `--json` overrides.
    #[serde(default)]
    pub format: Option<String>,
    /// Symbol set for CLI output: `"unicode"` (default) or `"ascii"`.
    /// Overridden by `LABBY_SYMBOLS` env var.
    #[serde(default)]
    pub symbols: Option<String>,
}

/// MCP server defaults.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct McpPreferences {
    /// Default transport (`stdio`, `http`, or `unix_socket`).
    #[serde(default)]
    pub transport: Option<String>,
    /// Default bind address for the HTTP transport.
    #[serde(default)]
    pub host: Option<String>,
    /// Default port for the HTTP transport.
    #[serde(default)]
    pub port: Option<u16>,
    /// Filesystem Unix-domain socket path, or Linux abstract `@name` notation,
    /// used when `transport = "unix_socket"`.
    #[serde(default)]
    pub socket_path: Option<PathBuf>,
    /// Filesystem socket mode in octal, such as `0660` or `0o660`.
    #[serde(default)]
    pub socket_mode: Option<String>,
    /// Optional owner UID applied after binding a filesystem socket.
    #[serde(default)]
    pub socket_uid: Option<u32>,
    /// Optional owner GID applied after binding a filesystem socket.
    #[serde(default)]
    pub socket_gid: Option<u32>,
    /// Optional kernel peer UID allowlist for Unix-socket authorization.
    #[serde(default)]
    pub peer_uid: Option<u32>,
    /// Optional kernel peer GID allowlist for Unix-socket authorization.
    #[serde(default)]
    pub peer_gid: Option<u32>,
    /// Additional allowed hosts for DNS rebinding protection.
    #[serde(default)]
    pub allowed_hosts: Option<Vec<String>>,
    /// Show the full service catalog regardless of env-var presence.
    /// Overridden by `LABBY_SHOW_ALL` env var.
    #[serde(default)]
    pub show_all: Option<bool>,
    /// Maximum time to wait for one MCP peer catalog-change notification.
    /// Overridden by `LABBY_MCP_CATALOG_NOTIFICATION_TIMEOUT_MS`.
    #[serde(default)]
    pub catalog_notification_timeout_ms: Option<u64>,
}

/// Canonical public URL model.
///
/// `app` is the Lab UI and OAuth issuer, e.g. `https://lab.example.com`.
/// `mcp_gateway` is the MCP endpoint base URL when hosted on a separate hostname,
/// e.g. `https://mcp.example.com`.  When absent the gateway is assumed to be
/// reachable at the app URL.
///
/// Values are read from config.toml; env vars `LABBY_PUBLIC_URL` (app) and
/// `LABBY_MCP_GATEWAY_URL` (mcp_gateway) take precedence and may be set in
/// `~/.labby/.env`.
///
/// Accessor: [`LabConfig::public_urls()`] returns a resolved [`ResolvedPublicUrls`].
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct PublicUrlsConfig {
    /// Public app (UI + OAuth) base URL, e.g. `https://lab.example.com`.
    #[serde(default)]
    pub app: Option<String>,
    /// Separate MCP gateway base URL, e.g. `https://mcp.example.com`.
    /// Leave blank when the app and MCP gateway share the same hostname.
    #[serde(default)]
    pub mcp_gateway: Option<String>,
}

// `ResolvedPublicUrls` moved to `labby_runtime::gateway_config`; re-exported above.

/// File-backed auth preferences merged with environment variables at startup.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AuthFileConfig {
    /// `bearer` preserves LABBY_MCP_HTTP_TOKEN; `oauth` enables the internal auth server.
    #[serde(default)]
    pub mode: Option<String>,
    /// Public URL used for metadata and Google callback construction.
    #[serde(default)]
    pub public_url: Option<String>,
    /// Optional path override for the SQLite auth store.
    #[serde(default)]
    pub sqlite_path: Option<PathBuf>,
    /// Optional path override for the persisted JWT signing key.
    #[serde(default)]
    pub key_path: Option<PathBuf>,
    /// Bootstrap secret required for dynamic client registration.
    #[serde(default)]
    pub bootstrap_secret: Option<String>,
    /// Additional redirect URI patterns allowed for dynamic client registration.
    #[serde(default)]
    pub allowed_client_redirect_uris: Option<Vec<String>>,
    /// Google Workspace hosted domains whose members may log in, in addition to
    /// the admin email and the per-email allowlist.
    ///
    /// Matched against the Google ID token's `hd` claim, so only accounts truly
    /// hosted in the domain qualify.
    #[serde(default)]
    pub allowed_email_domains: Option<Vec<String>>,
    /// Active inbound human identity provider (`google` or `authelia`).
    #[serde(default)]
    pub provider: Option<String>,
    /// Authelia OpenID Connect issuer URL.
    #[serde(default)]
    pub authelia_issuer_url: Option<String>,
    /// Authelia confidential client ID.
    #[serde(default)]
    pub authelia_client_id: Option<String>,
    /// Authelia confidential client secret.
    #[serde(default)]
    pub authelia_client_secret: Option<String>,
    /// Exact explicitly trusted private issuer origin.
    #[serde(default)]
    pub authelia_trusted_private_origin: Option<String>,
    /// Optional PEM CA certificate used only for this exact Authelia origin.
    #[serde(default)]
    pub authelia_ca_certificate_path: Option<PathBuf>,
    /// Google OAuth client ID.
    #[serde(default)]
    pub google_client_id: Option<String>,
    /// Google OAuth client secret.
    #[serde(default)]
    pub google_client_secret: Option<String>,
    /// Optional callback path override.
    #[serde(default)]
    pub google_callback_path: Option<String>,
    /// Optional comma-separated scope list.
    #[serde(default)]
    pub google_scopes: Option<Vec<String>>,
    /// Optional access-token lifetime override in seconds.
    #[serde(default)]
    pub access_token_ttl_secs: Option<u64>,
    /// Optional refresh-token lifetime override in seconds.
    #[serde(default)]
    pub refresh_token_ttl_secs: Option<u64>,
    /// Optional authorization-code lifetime override in seconds.
    #[serde(default)]
    pub auth_code_ttl_secs: Option<u64>,
    /// Bootstrap admin Google email — required in oauth mode.
    #[serde(default)]
    pub admin_email: Option<String>,
    /// Per-IP rate limit for the dynamic-client-registration endpoint
    /// (requests per minute). Overridden by `LABBY_AUTH_REGISTER_REQUESTS_PER_MINUTE`.
    #[serde(default)]
    pub register_requests_per_minute: Option<u32>,
    /// Per-IP rate limit for the `/authorize` endpoint (requests per minute).
    /// Overridden by `LABBY_AUTH_AUTHORIZE_REQUESTS_PER_MINUTE`.
    #[serde(default)]
    pub authorize_requests_per_minute: Option<u32>,
    /// Per-IP rate limit for the `/token` endpoint (requests per minute).
    /// Overridden by `LABBY_AUTH_TOKEN_REQUESTS_PER_MINUTE`.
    #[serde(default)]
    pub token_requests_per_minute: Option<u32>,
    /// Out-of-band machine OAuth clients.
    #[serde(default)]
    pub machine_clients: Option<Vec<auth_config::MachineClientConfig>>,
    /// Trusted enterprise ID-JAG issuers.
    #[serde(default)]
    pub enterprise_issuers: Option<Vec<auth_config::EnterpriseIssuerConfig>>,
    /// Max in-flight OAuth state rows. Overridden by
    /// `LABBY_AUTH_MAX_PENDING_OAUTH_STATES`.
    #[serde(default)]
    pub max_pending_oauth_states: Option<usize>,
    /// Work around Codex clients that strip the RFC 9207 response issuer.
    /// Overridden by `LABBY_AUTH_CODEX_ISSUER_COMPATIBILITY`.
    #[serde(default)]
    pub codex_issuer_compatibility: Option<bool>,
}

const DEFAULT_CLIENT_REDIRECT_URI_PATTERNS: &[&str] = &[
    "https://chatgpt.com/aip/plugin-callback",
    "https://chat.openai.com/aip/plugin-callback",
    "https://chatgpt.com/connector/oauth/*",
    "https://chatgpt.com/connector_platform_oauth_redirect",
    "https://claude.ai/api/mcp/auth_callback",
    "https://claude.com/api/mcp/auth_callback",
];

/// Resolve auth configuration from a full `LabConfig`.
///
/// This is the preferred entry point. Precedence for the public URL is:
/// 1. `[auth].public_url` (legacy field, preserved for backward compatibility)
/// 2. `[public_urls].app` (canonical new location)
/// 3. `LABBY_PUBLIC_URL` env var (handled downstream by [`resolve_auth`])
///
/// When `[auth].public_url` is absent, `[public_urls].app` is promoted into the
/// auth config so downstream code resolves a consistent effective URL.
pub fn resolve_auth_for_config(cfg: &LabConfig) -> Result<auth_config::AuthConfig> {
    // Compute the effective public URL: [auth].public_url > [public_urls].app.
    // The env var LABBY_PUBLIC_URL is handled downstream by resolve_auth().
    let effective_public_url = cfg
        .auth
        .as_ref()
        .and_then(|a| a.public_url.clone())
        .or_else(|| cfg.public_urls().app);

    // Build a synthetic auth config that overlays the effective public URL.
    let mut auth = cfg.auth.clone().unwrap_or_default();
    if auth.public_url.is_none() {
        auth.public_url = effective_public_url;
    }
    resolve_auth(Some(&auth))
}

/// Resolve auth configuration from config file + environment variables.
///
/// Env vars take precedence over config file values.
/// Prefer [`resolve_auth_for_config`] when a full `LabConfig` is available,
/// so that `[public_urls].app` is used as a fallback for `LABBY_PUBLIC_URL`.
pub fn resolve_auth(config: Option<&AuthFileConfig>) -> Result<auth_config::AuthConfig> {
    resolve_auth_with_env(config, std::env::vars())
}

fn resolve_auth_with_env(
    config: Option<&AuthFileConfig>,
    env_vars: impl IntoIterator<Item = (String, String)>,
) -> Result<auth_config::AuthConfig> {
    let mut merged: HashMap<String, String> = HashMap::new();

    if let Some(config) = config {
        insert_if_some(&mut merged, "LABBY_AUTH_MODE", config.mode.clone());
        insert_if_some(&mut merged, "LABBY_PUBLIC_URL", config.public_url.clone());
        insert_if_some(
            &mut merged,
            "LABBY_AUTH_SQLITE_PATH",
            config
                .sqlite_path
                .as_ref()
                .map(|path| path.display().to_string()),
        );
        insert_if_some(
            &mut merged,
            "LABBY_AUTH_KEY_PATH",
            config
                .key_path
                .as_ref()
                .map(|path| path.display().to_string()),
        );
        insert_if_some(
            &mut merged,
            "LABBY_AUTH_BOOTSTRAP_SECRET",
            config.bootstrap_secret.clone(),
        );
        if let Some(patterns) = config.allowed_client_redirect_uris.as_ref() {
            merged.insert(
                "LABBY_AUTH_ALLOWED_REDIRECT_URIS".to_string(),
                patterns.join(","),
            );
        }
        if let Some(domains) = config.allowed_email_domains.as_ref() {
            merged.insert(
                "LABBY_AUTH_ALLOWED_EMAIL_DOMAINS".to_string(),
                domains.join(","),
            );
        }
        insert_if_some(
            &mut merged,
            "LABBY_GOOGLE_CLIENT_ID",
            config.google_client_id.clone(),
        );
        insert_if_some(&mut merged, "LABBY_AUTH_PROVIDER", config.provider.clone());
        insert_if_some(
            &mut merged,
            "LABBY_AUTHELIA_ISSUER_URL",
            config.authelia_issuer_url.clone(),
        );
        insert_if_some(
            &mut merged,
            "LABBY_AUTHELIA_CLIENT_ID",
            config.authelia_client_id.clone(),
        );
        insert_if_some(
            &mut merged,
            "LABBY_AUTHELIA_CLIENT_SECRET",
            config.authelia_client_secret.clone(),
        );
        insert_if_some(
            &mut merged,
            "LABBY_AUTHELIA_TRUSTED_PRIVATE_ORIGIN",
            config.authelia_trusted_private_origin.clone(),
        );
        insert_if_some(
            &mut merged,
            "LABBY_AUTHELIA_CA_CERT_PATH",
            config
                .authelia_ca_certificate_path
                .as_ref()
                .map(|path| path.display().to_string()),
        );
        insert_if_some(
            &mut merged,
            "LABBY_GOOGLE_CLIENT_SECRET",
            config.google_client_secret.clone(),
        );
        insert_if_some(
            &mut merged,
            "LABBY_GOOGLE_CALLBACK_PATH",
            config.google_callback_path.clone(),
        );
        if let Some(scopes) = config.google_scopes.as_ref() {
            insert_if_some(&mut merged, "LABBY_GOOGLE_SCOPES", Some(scopes.join(",")));
        }
        insert_if_some(
            &mut merged,
            "LABBY_AUTH_ACCESS_TOKEN_TTL_SECS",
            config.access_token_ttl_secs.map(|value| value.to_string()),
        );
        insert_if_some(
            &mut merged,
            "LABBY_AUTH_REFRESH_TOKEN_TTL_SECS",
            config.refresh_token_ttl_secs.map(|value| value.to_string()),
        );
        insert_if_some(
            &mut merged,
            "LABBY_AUTH_CODE_TTL_SECS",
            config.auth_code_ttl_secs.map(|value| value.to_string()),
        );
        insert_if_some(
            &mut merged,
            "LABBY_AUTH_ADMIN_EMAIL",
            config.admin_email.clone(),
        );
        insert_if_some(
            &mut merged,
            "LABBY_AUTH_REGISTER_REQUESTS_PER_MINUTE",
            config
                .register_requests_per_minute
                .map(|value| value.to_string()),
        );
        insert_if_some(
            &mut merged,
            "LABBY_AUTH_AUTHORIZE_REQUESTS_PER_MINUTE",
            config
                .authorize_requests_per_minute
                .map(|value| value.to_string()),
        );
        insert_if_some(
            &mut merged,
            "LABBY_AUTH_TOKEN_REQUESTS_PER_MINUTE",
            config
                .token_requests_per_minute
                .map(|value| value.to_string()),
        );
        if let Some(machine_clients) = config.machine_clients.as_ref() {
            merged.insert(
                "LABBY_AUTH_MACHINE_CLIENTS_JSON".to_string(),
                serde_json::to_string(machine_clients).context("serialize auth.machine_clients")?,
            );
        }
        if let Some(enterprise_issuers) = config.enterprise_issuers.as_ref() {
            merged.insert(
                "LABBY_AUTH_ENTERPRISE_ISSUERS_JSON".to_string(),
                serde_json::to_string(enterprise_issuers)
                    .context("serialize auth.enterprise_issuers")?,
            );
        }
        insert_if_some(
            &mut merged,
            "LABBY_AUTH_MAX_PENDING_OAUTH_STATES",
            config
                .max_pending_oauth_states
                .map(|value| value.to_string()),
        );
        insert_if_some(
            &mut merged,
            "LABBY_AUTH_CODEX_ISSUER_COMPATIBILITY",
            config
                .codex_issuer_compatibility
                .map(|value| value.to_string()),
        );
    }

    for (key, value) in env_vars {
        if key.starts_with("LABBY_AUTH_")
            || key == "LABBY_PUBLIC_URL"
            || key.starts_with("LABBY_GOOGLE_")
            || key.starts_with("LABBY_AUTHELIA_")
            || key == "LABBY_TOKEN_ENCRYPTION_KEY"
        {
            merged.insert(key, value);
        }
    }

    merged
        .entry("LABBY_AUTH_ALLOWED_REDIRECT_URIS".to_string())
        .or_insert_with(|| DEFAULT_CLIENT_REDIRECT_URI_PATTERNS.join(","));

    // An explicit provider selection owns precedence across configuration
    // sources. Credentials for the non-selected legacy provider must not make
    // an env override appear ambiguous.
    match merged.get("LABBY_AUTH_PROVIDER").map(String::as_str) {
        Some("authelia") => {
            merged.remove("LABBY_GOOGLE_CLIENT_ID");
            merged.remove("LABBY_GOOGLE_CLIENT_SECRET");
        }
        Some("google") => {
            merged.remove("LABBY_AUTHELIA_ISSUER_URL");
            merged.remove("LABBY_AUTHELIA_CLIENT_ID");
            merged.remove("LABBY_AUTHELIA_CLIENT_SECRET");
            merged.remove("LABBY_AUTHELIA_TRUSTED_PRIVATE_ORIGIN");
            merged.remove("LABBY_AUTHELIA_CA_CERT_PATH");
        }
        _ => {}
    }

    auth_config::AuthConfigBuilder::new()
        .env_prefix("LABBY")
        .build_from_sources(merged)
        .map_err(anyhow::Error::from)
}

fn insert_if_some(target: &mut HashMap<String, String>, key: &str, value: Option<String>) {
    if let Some(value) = value
        && !value.trim().is_empty()
    {
        target.insert(key.to_string(), value);
    }
}

/// Load `.env` + `config.toml` from the standard locations.
///
/// These map to `LABBY_LOG` and `LABBY_LOG_FORMAT` env vars but live in TOML so
/// operators don't need to clutter `.env` with non-secret preferences.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LogPreferences {
    /// Tracing filter directive (e.g. `"labby=info,labby_apis=warn"`).
    /// Overridden by `LABBY_LOG` env var.
    #[serde(default)]
    pub filter: Option<String>,
    /// Log format: `"text"` (default) or `"json"`.
    /// Overridden by `LABBY_LOG_FORMAT` env var.
    #[serde(default)]
    pub format: Option<String>,
    /// Force or disable ANSI color: `"force"`/`"always"`/`"1"` or
    /// `"plain"`/`"never"`/`"0"`. Overridden by `LABBY_LOG_COLOR` env var.
    /// This field is read directly from `config.toml` at startup, before
    /// `.env` loads, so it is the only reliable way to set log color from a
    /// file rather than real process/shell env.
    #[serde(default)]
    pub color: Option<String>,
    /// Directory for rolling log files. Defaults to `~/.local/share/labby/logs`.
    /// Overridden by `LABBY_LOG_DIR` env var. Read directly from `config.toml`
    /// at startup, before `.env` loads, for the same reason as `color`.
    #[serde(default)]
    pub dir: Option<PathBuf>,
}

/// Local-master log store and retention preferences.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LocalLogsPreferences {
    /// Optional path override for the embedded log store.
    #[serde(default)]
    pub store_path: Option<PathBuf>,
    /// Retention window in days.
    #[serde(default)]
    pub retention_days: Option<u64>,
    /// Max retained logical bytes. Oldest events are evicted first.
    #[serde(default)]
    pub max_bytes: Option<u64>,
    /// Bounded ingest queue size for the long-lived runtime.
    #[serde(default)]
    pub queue_capacity: Option<usize>,
    /// Bounded live-subscriber ring size for the SSE stream hub.
    #[serde(default)]
    pub subscriber_capacity: Option<usize>,
}

/// HTTP API preferences.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ApiPreferences {
    /// Additional CORS origins (comma-separated string or TOML array).
    /// Loopback origins are always included.
    /// Overridden by `LABBY_CORS_ORIGINS` env var.
    #[serde(default)]
    pub cors_origins: Vec<String>,
    /// Enable additional dev-only CORS origins (3000/5173/8080). Default: off.
    /// Overridden by `LABBY_DEV_MODE=1` env var.
    #[serde(default)]
    pub dev_mode: Option<bool>,
    /// Connect timeout in seconds for protected MCP route backends.
    /// Overridden by `LABBY_PROTECTED_MCP_CONNECT_TIMEOUT_SECS` env var.
    #[serde(default)]
    pub protected_mcp_connect_timeout_secs: Option<u64>,
    /// Trust reverse-proxy authority headers for virtual protected-route
    /// selection. Off by default: direct clients control these headers.
    #[serde(default)]
    pub trust_forwarded_headers: bool,
}

// `WebPreferences` moved to `labby_runtime::gateway_config`; re-exported above.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WebUiAuthDisabledEnv {
    pub disabled: bool,
    pub source: &'static str,
    pub legacy_alias: bool,
}

pub fn resolve_web_ui_auth_disabled_env() -> Result<Option<WebUiAuthDisabledEnv>> {
    resolve_web_ui_auth_disabled_values(
        std::env::var(WEB_UI_AUTH_DISABLED_ENV).ok().as_deref(),
        std::env::var(WEB_UI_AUTH_DISABLED_LEGACY_ENV)
            .ok()
            .as_deref(),
    )
}

pub fn resolve_web_ui_auth_disabled_values(
    canonical: Option<&str>,
    legacy: Option<&str>,
) -> Result<Option<WebUiAuthDisabledEnv>> {
    if let Some(value) = canonical.filter(|value| !value.trim().is_empty()) {
        return Ok(Some(WebUiAuthDisabledEnv {
            disabled: parse_web_ui_auth_disabled_bool(WEB_UI_AUTH_DISABLED_ENV, value)?,
            source: WEB_UI_AUTH_DISABLED_ENV,
            legacy_alias: false,
        }));
    }

    if let Some(value) = legacy.filter(|value| !value.trim().is_empty()) {
        return Ok(Some(WebUiAuthDisabledEnv {
            disabled: parse_web_ui_auth_disabled_bool(WEB_UI_AUTH_DISABLED_LEGACY_ENV, value)?,
            source: WEB_UI_AUTH_DISABLED_LEGACY_ENV,
            legacy_alias: true,
        }));
    }

    Ok(None)
}

fn parse_web_ui_auth_disabled_bool(name: &str, value: &str) -> Result<bool> {
    match value.trim().to_ascii_lowercase().as_str() {
        "true" | "1" => Ok(true),
        "false" | "0" => Ok(false),
        _ => anyhow::bail!("invalid {name} value `{value}`; expected true/false or 1/0"),
    }
}

/// Shared workspace root for Lab-managed files.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkspacePreferences {
    /// Root directory used by the supported filesystem browser.
    /// Defaults to `~/.labby/workspace`.
    #[serde(default)]
    pub root: Option<PathBuf>,
}

/// Durable storage preferences for the principal-scoped File Stash.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FileStashPreferences {
    /// Dedicated metadata and blob root. Defaults to `~/.labby/file-stash`.
    #[serde(default)]
    pub root: Option<PathBuf>,
    #[serde(default = "default_stash_file_bytes")]
    pub max_file_bytes: u64,
    #[serde(default = "default_stash_principal_bytes")]
    pub principal_quota_bytes: u64,
    #[serde(default = "default_stash_instance_bytes")]
    pub instance_quota_bytes: u64,
    #[serde(default = "default_stash_live_files")]
    pub max_live_files_per_principal: u32,
    #[serde(default = "default_stash_instance_live_files")]
    pub max_live_files_per_instance: u32,
    #[serde(default = "default_stash_page_size")]
    pub page_size: u16,
    #[serde(default = "default_stash_query_bytes")]
    pub max_query_bytes: usize,
    #[serde(default = "default_stash_header_bytes")]
    pub max_header_bytes: usize,
    #[serde(default = "default_stash_recipients")]
    pub grant_recipients_page_size: u16,
    #[serde(default = "default_stash_mcp_read_bytes")]
    pub max_mcp_read_bytes: u64,
    #[serde(default = "default_stash_queue_capacity")]
    pub queue_capacity: usize,
    #[serde(default = "default_stash_database_deadline_ms")]
    pub database_deadline_ms: u64,
    #[serde(default = "default_stash_principal_uploads")]
    pub max_concurrent_uploads_per_principal: usize,
    #[serde(default = "default_stash_instance_uploads")]
    pub max_concurrent_uploads_per_instance: usize,
    #[serde(default = "default_stash_downloads")]
    pub max_concurrent_downloads: usize,
    #[serde(default = "default_stash_mcp_reads")]
    pub max_concurrent_mcp_reads: usize,
    #[serde(default = "default_stash_idle_seconds")]
    pub upload_idle_seconds: u64,
    #[serde(default = "default_stash_total_seconds")]
    pub upload_total_seconds: u64,
    #[serde(default = "default_stash_idle_seconds")]
    pub download_idle_seconds: u64,
    #[serde(default = "default_stash_total_seconds")]
    pub download_total_seconds: u64,
    #[serde(default = "default_stash_pending_seconds")]
    pub pending_ttl_seconds: u64,
    #[serde(default = "default_stash_janitor_batch")]
    pub janitor_batch_size: usize,
    #[serde(default = "default_stash_janitor_backoff_seconds")]
    pub janitor_backoff_max_seconds: u64,
    #[serde(default = "default_stash_janitor_interval_seconds")]
    pub janitor_interval_seconds: u64,
}

impl Default for FileStashPreferences {
    fn default() -> Self {
        Self {
            root: None,
            max_file_bytes: default_stash_file_bytes(),
            principal_quota_bytes: default_stash_principal_bytes(),
            instance_quota_bytes: default_stash_instance_bytes(),
            max_live_files_per_principal: default_stash_live_files(),
            max_live_files_per_instance: default_stash_instance_live_files(),
            page_size: default_stash_page_size(),
            max_query_bytes: default_stash_query_bytes(),
            max_header_bytes: default_stash_header_bytes(),
            grant_recipients_page_size: default_stash_recipients(),
            max_mcp_read_bytes: default_stash_mcp_read_bytes(),
            queue_capacity: default_stash_queue_capacity(),
            database_deadline_ms: default_stash_database_deadline_ms(),
            max_concurrent_uploads_per_principal: default_stash_principal_uploads(),
            max_concurrent_uploads_per_instance: default_stash_instance_uploads(),
            max_concurrent_downloads: default_stash_downloads(),
            max_concurrent_mcp_reads: default_stash_mcp_reads(),
            upload_idle_seconds: default_stash_idle_seconds(),
            upload_total_seconds: default_stash_total_seconds(),
            download_idle_seconds: default_stash_idle_seconds(),
            download_total_seconds: default_stash_total_seconds(),
            pending_ttl_seconds: default_stash_pending_seconds(),
            janitor_batch_size: default_stash_janitor_batch(),
            janitor_backoff_max_seconds: default_stash_janitor_backoff_seconds(),
            janitor_interval_seconds: default_stash_janitor_interval_seconds(),
        }
    }
}

fn default_stash_file_bytes() -> u64 {
    104_857_600
}
fn default_stash_principal_bytes() -> u64 {
    1_073_741_824
}
fn default_stash_instance_bytes() -> u64 {
    10_737_418_240
}
fn default_stash_live_files() -> u32 {
    1_000
}
fn default_stash_instance_live_files() -> u32 {
    100_000
}
fn default_stash_page_size() -> u16 {
    50
}
fn default_stash_query_bytes() -> usize {
    128
}
fn default_stash_header_bytes() -> usize {
    16_384
}
fn default_stash_recipients() -> u16 {
    50
}
fn default_stash_mcp_read_bytes() -> u64 {
    10_485_760
}
fn default_stash_queue_capacity() -> usize {
    64
}
fn default_stash_database_deadline_ms() -> u64 {
    100
}
fn default_stash_principal_uploads() -> usize {
    2
}
fn default_stash_instance_uploads() -> usize {
    8
}
fn default_stash_downloads() -> usize {
    16
}
fn default_stash_mcp_reads() -> usize {
    4
}
fn default_stash_idle_seconds() -> u64 {
    30
}
fn default_stash_total_seconds() -> u64 {
    600
}
fn default_stash_pending_seconds() -> u64 {
    1_800
}
const STASH_PENDING_MARGIN_SECONDS: u64 = 60;
fn default_stash_janitor_batch() -> usize {
    100
}
fn default_stash_janitor_backoff_seconds() -> u64 {
    300
}
fn default_stash_janitor_interval_seconds() -> u64 {
    60
}

impl FileStashPreferences {
    fn validate(&self) -> Result<(), ConfigError> {
        let valid = (1..=1_073_741_824).contains(&self.max_file_bytes)
            && (self.max_file_bytes..=107_374_182_400).contains(&self.principal_quota_bytes)
            && (self.principal_quota_bytes..=1_099_511_627_776)
                .contains(&self.instance_quota_bytes)
            && (1..=100_000).contains(&self.max_live_files_per_principal)
            && (self.max_live_files_per_principal..=1_000_000)
                .contains(&self.max_live_files_per_instance)
            && (1..=200).contains(&self.page_size)
            && (1..=1_024).contains(&self.max_query_bytes)
            && (1..=65_536).contains(&self.max_header_bytes)
            && (1..=200).contains(&self.grant_recipients_page_size)
            && (1..=26_214_400).contains(&self.max_mcp_read_bytes)
            && (1..=1_024).contains(&self.queue_capacity)
            && (1..=30_000).contains(&self.database_deadline_ms)
            && (1..=2).contains(&self.max_concurrent_uploads_per_principal)
            && (self.max_concurrent_uploads_per_principal..=8)
                .contains(&self.max_concurrent_uploads_per_instance)
            && (1..=256).contains(&self.max_concurrent_downloads)
            && (1..=4).contains(&self.max_concurrent_mcp_reads)
            && (1..=30).contains(&self.upload_idle_seconds)
            && (self.upload_idle_seconds..=600).contains(&self.upload_total_seconds)
            && (1..=30).contains(&self.download_idle_seconds)
            && (self.download_idle_seconds..=600).contains(&self.download_total_seconds)
            && self.pending_ttl_seconds
                >= self
                    .upload_total_seconds
                    .saturating_add(STASH_PENDING_MARGIN_SECONDS)
            && self.pending_ttl_seconds <= 1_800
            && (1..=100).contains(&self.janitor_batch_size)
            && (1..=300).contains(&self.janitor_backoff_max_seconds)
            && (1..=3_600).contains(&self.janitor_interval_seconds)
            && self.janitor_backoff_max_seconds >= self.janitor_interval_seconds;
        if !valid {
            return Err(ConfigError::InvalidProxyConfig {
                reason:
                    "file_stash limits must be positive and within their documented safety bounds"
                        .into(),
            });
        }
        Ok(())
    }
}

/// OAuth local relay preferences.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OauthPreferences {
    /// Named callback relay targets.
    #[serde(default)]
    pub machines: BTreeMap<String, OauthMachineConfig>,
}

/// A named OAuth callback relay target.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OauthMachineConfig {
    /// Full callback target base URL.
    pub target_url: String,
    /// Optional operator-facing description.
    #[serde(default)]
    pub description: Option<String>,
    /// Optional preferred callback port for the browser-local listener.
    #[serde(default)]
    pub default_port: Option<u16>,
}

/// Admin tool settings.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AdminPreferences {
    /// Enable the `lab_admin` MCP tool. Default: `false`.
    /// Overridden by `LABBY_ADMIN_ENABLED=1` env var.
    #[serde(default)]
    pub enabled: bool,
}

/// Per-service preference overrides (non-secret values only).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
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

/// Tailscale non-secret preferences.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TailscalePreferences {
    /// Tailnet name. Overridden by `TAILSCALE_TAILNET` env var.
    /// Default: `"-"` (auto-detect).
    #[serde(default)]
    pub tailnet: Option<String>,
}

/// `[setup]` preferences: operator toggles for `labby setup --provision`.
///
/// These are non-secret capabilities that provision can install on demand,
/// kept out of the baked Incus image to slim it. Env overrides still apply.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SetupPreferences {
    /// Install `android-sdk` during provision. Needed only by the
    /// claude-in-mobile MCP server. Default: false.
    /// Env override: `LABBY_ENABLE_ANDROID_SDK=1`.
    #[serde(default)]
    pub install_android_sdk: Option<bool>,
}

/// Load `config.toml` only — no `.env`, no side effects beyond file reads.
///
/// Called early in `main()` before tracing is initialized so that `[log]`
/// preferences can feed into `init_tracing()`. Safe to call before any
/// other subsystem.
///
/// Config TOML resolves from the one installation root selected by
/// `LABBY_HOME`, normally `~/.labby/config.toml`.
pub fn load_toml(candidates: &[PathBuf]) -> Result<LabConfig> {
    // Do not let an explicitly invalid LABBY_HOME silently fall through to a
    // different installation's user-home config.
    if std::env::var_os("LABBY_HOME").is_some_and(|value| !value.is_empty()) {
        crate::installation::InstallationPaths::resolve().context("invalid explicit LABBY_HOME")?;
    }
    load_toml_from_paths(candidates)
}

/// Load an already-authoritative fixed path without consulting caller process
/// root variables. Used for lifecycle management of the fixed daemon account.
pub(crate) fn load_toml_from_fixed_root(candidates: &[PathBuf]) -> Result<LabConfig> {
    load_toml_from_paths(candidates)
}

fn load_toml_from_paths(candidates: &[PathBuf]) -> Result<LabConfig> {
    for path in candidates {
        match std::fs::symlink_metadata(path) {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(error) => return Err(error.into()),
            Ok(_) => {
                let lock = host_write::HostConfigLock::acquire(path)?;
                let raw = lock.read_raw()?;
                validate_top_level_extension_boundary(&raw)
                    .with_context(|| format!("failed to parse {}", path.display()))?;
                let mut cfg = toml::from_str::<LabConfig>(&raw)
                    .with_context(|| format!("failed to parse {}", path.display()))?;
                cfg.normalize_protected_mcp_routes()
                    .with_context(|| format!("invalid config {}", path.display()))?;
                // Validate all upstream configs eagerly at startup so that
                // invalid configuration (conflicting auth, bad URL scheme, etc.)
                // is discovered immediately rather than at first OAuth attempt.
                cfg.validate()
                    .with_context(|| format!("invalid config {}", path.display()))?;
                return Ok(cfg);
            }
        }
    }
    Ok(LabConfig::default())
}

fn validate_top_level_extension_boundary(raw: &str) -> Result<()> {
    let table = raw.parse::<toml::Table>()?;
    const OWNED: &[&str] = &[
        "config_version",
        "output",
        "mcp",
        "proxy",
        "log",
        "local_logs",
        "api",
        "web",
        "workspace",
        "file_stash",
        "oauth",
        "admin",
        "services",
        "setup",
        "auth",
        "code_mode",
        "mcp_apps",
        "skill_library",
        "upstream_request_timeout_ms",
        "upstream_relay_timeout_ms",
        "upstream",
        "upstream_import_tombstones",
        "upstream_pending",
        "gateway_import_mode",
        "loadouts",
        "protected_mcp_routes",
        "virtual_servers",
        "quarantined_virtual_servers",
        "public_urls",
        "gateway",
        "openapi",
    ];
    for (key, value) in table {
        if !OWNED.contains(&key.as_str()) && !value.is_table() {
            anyhow::bail!("unknown top-level scalar `{key}`; extensions must use a named table")
        }
    }
    Ok(())
}

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
        Self {
            path: path.into(),
            value,
        }
    }
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct ConfigPatchOutcome {
    pub config: LabConfig,
    pub backup_path: Option<PathBuf>,
    /// A durable commit succeeded, but best-effort backup maintenance did not.
    /// Callers must report this without claiming the requested mutation failed.
    pub maintenance_warning: Option<String>,
}

#[derive(Debug, Clone)]
pub struct ExpectedConfigScalar {
    pub path: String,
    pub value: serde_json::Value,
}

impl ExpectedConfigScalar {
    #[must_use]
    pub fn new(path: impl Into<String>, value: serde_json::Value) -> Self {
        Self {
            path: path.into(),
            value,
        }
    }
}

static CONFIG_BACKUP_COUNTER: AtomicU32 = AtomicU32::new(0);

fn inline_table_to_table(inline: &toml_edit::InlineTable) -> toml_edit::Table {
    let mut table = toml_edit::Table::new();
    for (key, value) in inline {
        table[key] = toml_edit::Item::Value(value.clone());
    }
    table
}

fn set_toml_scalar_path(
    document: &mut toml_edit::DocumentMut,
    dotted_path: &str,
    value: ConfigScalarValue,
) -> Result<()> {
    let parts: Vec<&str> = dotted_path
        .split('.')
        .filter(|part| !part.is_empty())
        .collect();
    anyhow::ensure!(!parts.is_empty(), "config path must not be empty");
    let (leaf, parents) = parts.split_last().expect("non-empty parts");
    let mut item = document.as_item_mut();
    for part in parents {
        let table = item
            .as_table_mut()
            .ok_or_else(|| anyhow::anyhow!("config parent `{part}` is not a table"))?;
        if !table.contains_key(part) {
            table.insert(part, toml_edit::Item::Table(toml_edit::Table::new()));
        }
        let child = table
            .get_mut(part)
            .ok_or_else(|| anyhow::anyhow!("config parent `{part}` was not created"))?;
        if !child.is_table() {
            let converted = child
                .as_value()
                .and_then(toml_edit::Value::as_inline_table)
                .map(inline_table_to_table);
            if let Some(table) = converted {
                *child = toml_edit::Item::Table(table);
            } else {
                anyhow::bail!("config parent `{part}` is not a table");
            }
        }
        item = child;
    }
    if matches!(value, ConfigScalarValue::UnsetOptional) {
        if let Some(table) = item.as_table_mut() {
            table.remove(leaf);
            return Ok(());
        }
        anyhow::bail!("config parent for `{dotted_path}` is not a table");
    }
    item[*leaf] = toml_edit::Item::Value(match value {
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

pub fn patch_config_scalars(
    path: &Path,
    entries: &[ConfigScalarPatch],
) -> Result<ConfigPatchOutcome> {
    patch_config_scalars_checked(path, entries, &[])
}

pub fn patch_config_scalars_checked(
    path: &Path,
    entries: &[ConfigScalarPatch],
    expected: &[ExpectedConfigScalar],
) -> Result<ConfigPatchOutcome> {
    let host_lock = host_write::HostConfigLock::acquire(path)?;
    let raw = host_lock.read_raw()?;
    let mut document = raw
        .parse::<toml_edit::DocumentMut>()
        .with_context(|| format!("failed to parse {}", path.display()))?;
    if !expected.is_empty() {
        let mut current_cfg = toml::from_str::<LabConfig>(&raw)
            .with_context(|| format!("failed to parse {}", path.display()))?;
        current_cfg
            .normalize_protected_mcp_routes()
            .with_context(|| format!("invalid config {}", path.display()))?;
        current_cfg
            .validate()
            .with_context(|| format!("invalid config {}", path.display()))?;
        for item in expected {
            let current = config_json_value_for_path(&current_cfg, &item.path);
            anyhow::ensure!(
                current == item.value,
                "setting `{}` changed since it was loaded",
                item.path
            );
        }
    }
    for entry in entries {
        set_toml_scalar_path(&mut document, &entry.path, entry.value.clone())
            .with_context(|| format!("failed to patch {}", entry.path))?;
    }
    if document.to_string() != raw && !document.contains_key("config_version") {
        document["config_version"] = toml_edit::value(i64::from(CURRENT_CONFIG_VERSION));
    }
    let patched = document.to_string();
    let mut cfg = toml::from_str::<LabConfig>(&patched)
        .with_context(|| format!("failed to parse patched {}", path.display()))?;
    cfg.normalize_protected_mcp_routes()
        .with_context(|| format!("invalid patched config {}", path.display()))?;
    cfg.validate()
        .with_context(|| format!("invalid patched config {}", path.display()))?;

    if patched == raw {
        return Ok(ConfigPatchOutcome {
            config: cfg,
            backup_path: None,
            maintenance_warning: None,
        });
    }

    let backup_path = if path.exists() {
        Some(backup_config_file(path, &raw)?)
    } else {
        None
    };
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    host_lock.write(&patched)?;
    let maintenance_warning = (|| -> Result<()> {
        #[cfg(test)]
        if parent
            .join(".labby-test-config-maintenance-failure")
            .exists()
        {
            anyhow::bail!("injected post-commit maintenance failure");
        }
        sync_config_parent(parent)
            .with_context(|| format!("parent sync failed for {}", path.display()))?;
        if prune_config_backups(parent, path)? > 0 {
            sync_config_parent(parent)
                .with_context(|| format!("backup-prune sync failed for {}", path.display()))?;
        }
        Ok(())
    })()
    .err()
    .map(|error| {
        format!("configuration was committed, but post-commit maintenance failed: {error:#}")
    });

    Ok(ConfigPatchOutcome {
        config: cfg,
        backup_path,
        maintenance_warning,
    })
}

#[cfg(unix)]
fn sync_config_parent(parent: &Path) -> std::io::Result<()> {
    OpenOptions::new().read(true).open(parent)?.sync_all()
}

#[cfg(windows)]
fn sync_config_parent(_parent: &Path) -> std::io::Result<()> {
    Ok(())
}

fn prune_config_backups(parent: &Path, target: &Path) -> Result<usize> {
    let Some(target_name) = target.file_name().and_then(|name| name.to_str()) else {
        return Ok(0);
    };
    let prefix = format!("{target_name}.bak.");
    let backups = std::fs::read_dir(parent)
        .with_context(|| format!("read config backup directory {}", parent.display()))?
        .map(|entry| {
            entry.with_context(|| format!("read config backup entry in {}", parent.display()))
        })
        .filter_map(|entry| match entry {
            Ok(entry) if entry.file_name().to_string_lossy().starts_with(&prefix) => {
                Some(Ok(entry))
            }
            Ok(_) => None,
            Err(error) => Some(Err(error)),
        })
        .map(|entry| {
            let entry = entry?;
            let path = entry.path();
            let metadata = entry
                .metadata()
                .with_context(|| format!("inspect config backup {}", path.display()))?;
            let modified = metadata
                .modified()
                .with_context(|| format!("inspect config backup {}", path.display()))?;
            Ok(ConfigBackupCandidate {
                path,
                modified,
                bytes: metadata.len(),
            })
        })
        .collect::<Result<Vec<_>>>()?;
    let removals = select_config_backups_to_prune(
        backups,
        std::time::SystemTime::now(),
        ConfigBackupRetention {
            max_count: CONFIG_BACKUP_RETENTION,
            max_age: CONFIG_BACKUP_MAX_AGE,
            max_bytes: CONFIG_BACKUP_MAX_BYTES,
        },
    );
    for backup in &removals {
        std::fs::remove_file(backup)
            .with_context(|| format!("remove old config backup {}", backup.display()))?;
    }
    Ok(removals.len())
}

fn select_config_backups_to_prune(
    mut backups: Vec<ConfigBackupCandidate>,
    now: std::time::SystemTime,
    retention: ConfigBackupRetention,
) -> Vec<PathBuf> {
    backups.sort_by(|left, right| {
        left.modified
            .cmp(&right.modified)
            .then_with(|| left.path.cmp(&right.path))
    });
    let Some(newest) = backups.last().map(|candidate| candidate.path.clone()) else {
        return Vec::new();
    };
    let mut retained_count = backups.len();
    let mut retained_bytes = backups.iter().fold(0_u64, |total, candidate| {
        total.saturating_add(candidate.bytes)
    });
    let mut removals = Vec::new();
    for candidate in backups {
        if candidate.path == newest {
            continue;
        }
        let expired = now
            .duration_since(candidate.modified)
            .is_ok_and(|age| age > retention.max_age);
        let over_count = retained_count > retention.max_count.max(1);
        let over_bytes = retained_bytes > retention.max_bytes;
        if expired || over_count || over_bytes {
            retained_count = retained_count.saturating_sub(1);
            retained_bytes = retained_bytes.saturating_sub(candidate.bytes);
            removals.push(candidate.path);
        }
    }
    removals
}

fn backup_config_file(path: &Path, raw: &str) -> Result<PathBuf> {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |duration| duration.as_nanos());
    let pid = std::process::id();
    for _ in 0..10 {
        let counter = CONFIG_BACKUP_COUNTER.fetch_add(1, Ordering::Relaxed);
        let backup = path.with_extension(format!("toml.bak.{nanos}.{pid}.{counter}"));
        let mut options = OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        match options.open(&backup) {
            Ok(mut file) => {
                if let Err(error) = secret_files::restrict_secret_file_permissions(&backup) {
                    drop(file);
                    drop(std::fs::remove_file(&backup));
                    return Err(error).with_context(|| {
                        format!("restrict backup {} before writing", backup.display())
                    });
                }
                file.write_all(raw.as_bytes())
                    .with_context(|| format!("write backup {}", backup.display()))?;
                file.sync_all()
                    .with_context(|| format!("sync backup {}", backup.display()))?;
                return Ok(backup);
            }
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(e) => {
                return Err(
                    anyhow::Error::new(e).context(format!("create backup {}", backup.display()))
                );
            }
        }
    }
    anyhow::bail!("failed to create unique backup for {}", path.display())
}

pub(crate) fn config_json_value_for_path(cfg: &LabConfig, path: &str) -> serde_json::Value {
    match path {
        "output.format" => serde_json::json!(cfg.output.format),
        "mcp.transport" => serde_json::json!(cfg.mcp.transport),
        "mcp.host" => serde_json::json!(cfg.mcp.host),
        "mcp.port" => serde_json::json!(cfg.mcp.port),
        "mcp.allowed_hosts" => serde_json::json!(cfg.mcp.allowed_hosts),
        "log.filter" => serde_json::json!(cfg.log.filter),
        "log.format" => serde_json::json!(cfg.log.format),
        "local_logs.retention_days" => {
            serde_json::json!(
                cfg.local_logs
                    .as_ref()
                    .and_then(|value| value.retention_days)
            )
        }
        "local_logs.max_bytes" => {
            serde_json::json!(cfg.local_logs.as_ref().and_then(|value| value.max_bytes))
        }
        "local_logs.queue_capacity" => {
            serde_json::json!(
                cfg.local_logs
                    .as_ref()
                    .and_then(|value| value.queue_capacity)
            )
        }
        "local_logs.subscriber_capacity" => {
            serde_json::json!(
                cfg.local_logs
                    .as_ref()
                    .and_then(|value| value.subscriber_capacity)
            )
        }
        "api.cors_origins" => serde_json::json!(cfg.api.cors_origins),
        "web.assets_dir" => {
            serde_json::json!(
                cfg.web
                    .assets_dir
                    .as_ref()
                    .map(|path| path.display().to_string())
            )
        }
        "workspace.root" => {
            serde_json::json!(
                cfg.workspace
                    .root
                    .as_ref()
                    .map(|path| path.display().to_string())
            )
        }
        "public_urls.app" => {
            serde_json::json!(cfg.public_urls.as_ref().and_then(|value| value.app.clone()))
        }
        "public_urls.mcp_gateway" => serde_json::json!(
            cfg.public_urls
                .as_ref()
                .and_then(|value| value.mcp_gateway.clone())
        ),
        "services.built_in_upstream_apis_enabled" => {
            serde_json::json!(cfg.services.built_in_upstream_apis_enabled)
        }
        "services.tailscale.tailnet" => serde_json::json!(cfg.services.tailscale.tailnet),
        "admin.enabled" => serde_json::json!(cfg.admin.enabled),
        "code_mode.trace_params" => serde_json::json!(cfg.code_mode.trace_params),
        "code_mode.timeout_ms" => serde_json::json!(cfg.code_mode.timeout_ms),
        "code_mode.max_source_bytes" => serde_json::json!(cfg.code_mode.max_source_bytes),
        "code_mode.max_response_bytes" => serde_json::json!(cfg.code_mode.max_response_bytes),
        "code_mode.max_response_tokens" => serde_json::json!(cfg.code_mode.max_response_tokens),
        "code_mode.token_estimate_divisor" => {
            serde_json::json!(cfg.code_mode.token_estimate_divisor)
        }
        "code_mode.max_log_entries" => serde_json::json!(cfg.code_mode.max_log_entries),
        "code_mode.max_log_bytes" => serde_json::json!(cfg.code_mode.max_log_bytes),
        "gateway_import_mode" => serde_json::json!(cfg.gateway_import_mode),
        "gateway.extra_stdio_commands" => serde_json::json!(cfg.gateway.extra_stdio_commands),
        "upstream_request_timeout_ms" => serde_json::json!(cfg.upstream_request_timeout_ms),
        "upstream_relay_timeout_ms" => serde_json::json!(cfg.upstream_relay_timeout_ms),
        "web.disable_auth" => serde_json::json!(cfg.web.disable_auth),
        "auth" => serde_json::to_value(&cfg.auth).unwrap_or(serde_json::Value::Null),
        "code_mode.enabled" => serde_json::json!(cfg.code_mode.enabled),
        "gateway.disable_spawn_guard" => serde_json::json!(cfg.gateway.disable_spawn_guard),
        "oauth.machines" => {
            serde_json::to_value(&cfg.oauth.machines).unwrap_or(serde_json::Value::Null)
        }
        "upstream" => serde_json::to_value(&cfg.upstream).unwrap_or(serde_json::Value::Null),
        "upstream_pending" => {
            serde_json::to_value(&cfg.upstream_pending).unwrap_or(serde_json::Value::Null)
        }
        "upstream_import_tombstones" => {
            serde_json::to_value(&cfg.upstream_import_tombstones).unwrap_or(serde_json::Value::Null)
        }
        "protected_mcp_routes" => {
            serde_json::to_value(&cfg.protected_mcp_routes).unwrap_or(serde_json::Value::Null)
        }
        "virtual_servers" => {
            serde_json::to_value(&cfg.virtual_servers).unwrap_or(serde_json::Value::Null)
        }
        "quarantined_virtual_servers" => serde_json::to_value(&cfg.quarantined_virtual_servers)
            .unwrap_or(serde_json::Value::Null),
        _ => serde_json::Value::Null,
    }
}

/// Patch the non-secret built-in upstream API preference without rewriting
/// unrelated TOML content.
///
/// This intentionally edits only `[services].built_in_upstream_apis_enabled`.
/// It preserves comments, unknown keys, and plugin-owned sections that the
/// full typed `LabConfig` serializer cannot round-trip.
pub fn patch_built_in_upstream_apis_enabled(path: &Path, enabled: bool) -> Result<LabConfig> {
    Ok(patch_config_scalars(
        path,
        &[ConfigScalarPatch::new(
            "services.built_in_upstream_apis_enabled",
            ConfigScalarValue::Bool(enabled),
        )],
    )?
    .config)
}

#[allow(dead_code)]
fn config_lock_path(path: &Path) -> PathBuf {
    let mut lock = path.to_path_buf();
    let file_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("config.toml");
    lock.set_file_name(format!("{file_name}.lock"));
    lock
}

/// Load `.env` files into the process environment.
///
/// Called after `load_toml()` and tracing init. Env vars loaded here
/// override config.toml values at the point of use (each consumer checks
/// env first, then falls back to config).
pub fn load_dotenv() -> Result<()> {
    // Candidates are ordered from authoritative installation state to the
    // implicit development fallback. dotenvy preserves values loaded by an
    // earlier candidate. An explicit LABBY_HOME excludes the CWD fallback.
    for env_path in paths::dotenv_candidates()? {
        if env_path.exists() {
            dotenvy::from_path(&env_path)
                .with_context(|| format!("failed to load {}", env_path.display()))?;
        }
    }

    Ok(())
}

/// Load `.env` + `config.toml` in a single call (convenience for tests).
#[allow(dead_code)]
pub fn load() -> Result<LabConfig> {
    let cfg = load_toml(&toml_candidates()?)?;
    load_dotenv()?;
    Ok(cfg)
}

/// Resolve the Code Mode `openapi` provider config from the parsed `[openapi]`
/// TOML section plus `OPENAPI_<LABEL>_*` env vars.
///
/// Non-secret fields come from TOML; credentials (`OPENAPI_<LABEL>_TOKEN` /
/// `OPENAPI_<LABEL>_API_KEY`) come from `env`. `base_url` is mandatory; reserved
/// or duplicate labels, a missing/invalid base_url, an invalid spec_url, and an
/// ambiguous spec source are all hard config errors that fail boot.
///
/// `env` is injected (rather than read from the process environment directly) so
/// tests stay hermetic. In production callers pass a `std::env::var`-backed closure.
#[cfg(feature = "gateway")]
pub fn load_openapi_provider_config(
    section: &OpenApiTomlSection,
    env: &dyn Fn(&str) -> Option<String>,
) -> std::result::Result<labby_openapi::OpenApiProviderConfig, ConfigError> {
    use labby_openapi::{OpenApiCredential, OpenApiProviderConfig, OpenApiSpecConfig, SpecSource};

    let mut specs = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for raw in &section.specs {
        let label = raw.label.trim().to_string();
        // The wire dispatch key is `openapi::<label>.<operationId>`, split on the
        // first `.` (operationIds may themselves contain `.`). A label containing
        // `.`, `:`, or whitespace would misroute that split, so restrict labels to
        // an unambiguous charset. Also keeps the `OPENAPI_<LABEL>_*` credential
        // env-var lookup well-formed.
        if label.is_empty()
            || !label
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
        {
            return Err(ConfigError::InvalidLabel { label });
        }
        if labby_openapi::RESERVED_NAMESPACES.contains(&label.as_str()) {
            return Err(ConfigError::ReservedLabel { label });
        }
        if !seen.insert(label.clone()) {
            return Err(ConfigError::DuplicateLabel { label });
        }
        let base_url: url::Url = raw
            .base_url
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .ok_or_else(|| ConfigError::MissingBaseUrl {
                label: label.clone(),
            })?
            .parse()
            .map_err(|_| ConfigError::InvalidBaseUrl {
                label: label.clone(),
            })?;

        let upper = label.to_uppercase();
        let credential = env(&format!("OPENAPI_{upper}_TOKEN"))
            .filter(|t| !t.is_empty())
            .map(OpenApiCredential::BearerToken)
            .or_else(|| {
                env(&format!("OPENAPI_{upper}_API_KEY"))
                    .filter(|k| !k.is_empty())
                    .map(|value| OpenApiCredential::ApiKey {
                        header: raw
                            .api_key_header
                            .clone()
                            .filter(|h| !h.trim().is_empty())
                            .unwrap_or_else(|| "X-API-Key".into()),
                        value,
                    })
            });

        let spec_source = match (
            raw.spec_url
                .as_deref()
                .map(str::trim)
                .filter(|s| !s.is_empty()),
            raw.spec_path
                .as_deref()
                .map(str::trim)
                .filter(|s| !s.is_empty()),
        ) {
            (Some(u), None) => {
                SpecSource::Url(u.parse().map_err(|_| ConfigError::InvalidSpecUrl {
                    label: label.clone(),
                })?)
            }
            (None, Some(p)) => SpecSource::Path(p.into()),
            _ => {
                return Err(ConfigError::SpecSourceAmbiguous {
                    label: label.clone(),
                });
            }
        };

        specs.push(OpenApiSpecConfig {
            label,
            spec_source,
            base_url,
            allowed_operations: raw.allowed_operations.clone(),
            credential,
        });
    }
    Ok(OpenApiProviderConfig { specs })
}

/// A string value that redacts itself in `Debug` and `Display` output.
///
/// Use for secret env values (`API_KEY`, `TOKEN`, `PASSWORD`) so they
/// never leak through `Debug`-printing config structs or tracing fields.
#[allow(dead_code)]
#[derive(Clone, Deserialize, PartialEq, Eq)]
pub struct Secret(String);

impl Secret {
    #[must_use]
    pub const fn new(value: String) -> Self {
        Self(value)
    }

    #[must_use]
    #[allow(dead_code)]
    pub fn expose(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Debug for Secret {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("[REDACTED]")
    }
}

impl std::fmt::Display for Secret {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("[REDACTED]")
    }
}

impl Serialize for Secret {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str("***REDACTED***")
    }
}

/// Value from an instance env var — either plain text or a secret.
///
/// Always constructed programmatically via the private `scan_instances_from` helper; never
/// deserialized from JSON. `Deserialize` is intentionally omitted — `Secret`
/// serializes as `"***REDACTED***"` (a plain string), so an `#[serde(untagged)]`
/// impl would silently pick `Plain` for every value, bypassing redaction.
#[allow(dead_code)]
#[derive(Debug, Clone, Serialize)]
pub enum InstanceValue {
    Plain(String),
    Redacted(Secret),
}

impl InstanceValue {
    #[must_use]
    #[allow(dead_code)]
    pub fn expose(&self) -> &str {
        match self {
            Self::Plain(s) => s,
            Self::Redacted(s) => s.expose(),
        }
    }
}

/// Suffixes that carry secret values and must be wrapped in [`Secret`].
#[allow(dead_code)]
const SECRET_SUFFIXES: &[&str] = &["API_KEY", "TOKEN", "PASSWORD"];

/// Parse multi-instance env vars for a given service prefix.
///
/// Returns a map from instance label (`"default"` or `"<label>"`) to the
/// set of `(suffix, value)` pairs. Example: for prefix `UNRAID`, env vars
/// `UNRAID_URL`, `UNRAID_API_KEY`, `UNRAID_NODE2_URL`, `UNRAID_NODE2_API_KEY`
/// yield two entries keyed `"default"` and `"node2"`.
///
/// Suffixes are matched longest-first to avoid collisions when a label
/// contains a shorter suffix as a substring.
#[must_use]
#[allow(dead_code)]
pub fn scan_instances(prefix: &str) -> HashMap<String, HashMap<String, InstanceValue>> {
    scan_instances_from(prefix, std::env::vars())
}

/// Inner implementation testable without mutating process env.
fn scan_instances_from(
    prefix: &str,
    vars: impl Iterator<Item = (String, String)>,
) -> HashMap<String, HashMap<String, InstanceValue>> {
    let mut out: HashMap<String, HashMap<String, InstanceValue>> = HashMap::new();

    let mut known_suffixes = ["URL", "API_KEY", "TOKEN", "USERNAME", "PASSWORD"];
    known_suffixes.sort_by_key(|s| std::cmp::Reverse(s.len()));

    let prefix_under = format!("{prefix}_");

    for (key, value) in vars {
        let Some(rest) = key.strip_prefix(&prefix_under) else {
            continue;
        };

        for suffix in &known_suffixes {
            let wrap = |v: String| {
                if SECRET_SUFFIXES.contains(suffix) {
                    InstanceValue::Redacted(Secret::new(v))
                } else {
                    InstanceValue::Plain(v)
                }
            };

            if rest == *suffix {
                out.entry("default".to_string())
                    .or_default()
                    .insert((*suffix).to_string(), wrap(value.clone()));
                break;
            }
            if let Some(label) = rest.strip_suffix(&format!("_{suffix}"))
                && !label.is_empty()
            {
                out.entry(label.to_ascii_lowercase())
                    .or_default()
                    .insert((*suffix).to_string(), wrap(value.clone()));
                break;
            }
        }
    }

    out
}

#[cfg(test)]
#[allow(clippy::panic)]
mod tests {
    use super::*;

    #[test]
    fn owned_section_typos_fail_while_foreign_top_level_extensions_survive() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(
            &path,
            "[mcp]\nporrt = 9876\n\n[external_extension]\nmode = \"custom\"\n",
        )
        .unwrap();

        let error = load_toml(std::slice::from_ref(&path)).unwrap_err();
        let detail = format!("{error:#}");
        assert!(detail.contains("porrt"), "{detail}");

        std::fs::write(&path, "[external_extension]\nmode = \"custom\"\n").unwrap();
        assert!(load_toml(&[path]).is_ok());
    }

    #[test]
    fn top_level_scalar_typos_fail_but_named_extension_tables_survive() {
        for typo in ["mcpp = 1\n", "config_verzion = 1\n"] {
            let error = validate_top_level_extension_boundary(typo).unwrap_err();
            assert!(error.to_string().contains("unknown top-level scalar"));
        }
        validate_top_level_extension_boundary("[vendor.example]\nenabled = true\n").unwrap();
    }

    #[test]
    fn missing_config_version_migrates_to_current_and_future_versions_fail() {
        let legacy: LabConfig = toml::from_str("[mcp]\nport = 9876\n").unwrap();
        assert_eq!(legacy.config_version, CURRENT_CONFIG_VERSION);
        legacy.validate().unwrap();

        let future: LabConfig = toml::from_str("config_version = 999\n").unwrap();
        let error = future.validate().unwrap_err();
        assert!(error.to_string().contains("config_version 999"));
    }

    #[test]
    fn patching_legacy_config_persists_the_migrated_format_version() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(&path, "[mcp]\nport = 8765\n").unwrap();

        patch_config_scalars(
            &path,
            &[ConfigScalarPatch::new(
                "mcp.port",
                ConfigScalarValue::I64(9876),
            )],
        )
        .unwrap();

        let persisted = std::fs::read_to_string(path).unwrap();
        assert!(persisted.contains("config_version = 1"), "{persisted}");
    }

    #[test]
    fn config_mutations_retain_only_the_ten_newest_backups() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(&path, "config_version = 1\n[mcp]\nport = 8765\n").unwrap();
        for port in 8800..8815 {
            patch_config_scalars(
                &path,
                &[ConfigScalarPatch::new(
                    "mcp.port",
                    ConfigScalarValue::I64(port),
                )],
            )
            .unwrap();
        }

        let backups = std::fs::read_dir(dir.path())
            .unwrap()
            .filter_map(Result::ok)
            .filter(|entry| entry.file_name().to_string_lossy().contains(".toml.bak."))
            .count();
        assert_eq!(backups, 10);
    }

    #[test]
    fn committed_config_reports_post_commit_maintenance_failure_as_warning() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(&path, "config_version = 1\n[mcp]\nport = 8765\n").unwrap();
        std::fs::write(
            dir.path().join(".labby-test-config-maintenance-failure"),
            b"inject",
        )
        .unwrap();

        let outcome = patch_config_scalars(
            &path,
            &[ConfigScalarPatch::new(
                "mcp.port",
                ConfigScalarValue::I64(9876),
            )],
        )
        .expect("durable commit must not be reported as failed");

        assert_eq!(outcome.config.mcp.port, Some(9876));
        assert!(
            std::fs::read_to_string(&path)
                .unwrap()
                .contains("port = 9876")
        );
        assert!(
            outcome
                .maintenance_warning
                .as_deref()
                .is_some_and(|warning| warning.contains("configuration was committed"))
        );
        assert!(outcome.backup_path.is_some());
    }

    #[test]
    fn backup_retention_enforces_age_and_bytes_without_pruning_newest() {
        let now = std::time::UNIX_EPOCH + Duration::from_secs(10_000);
        let candidates = vec![
            ConfigBackupCandidate::fixture("old", 100, now - Duration::from_secs(5_000)),
            ConfigBackupCandidate::fixture("large", 900, now - Duration::from_secs(20)),
            ConfigBackupCandidate::fixture("newest", 900, now - Duration::from_secs(10)),
        ];

        let pruned = select_config_backups_to_prune(
            candidates,
            now,
            ConfigBackupRetention {
                max_count: 10,
                max_age: Duration::from_secs(1_000),
                max_bytes: 500,
            },
        );

        assert_eq!(pruned, vec![PathBuf::from("old"), PathBuf::from("large")]);
    }

    #[test]
    fn backup_retention_is_deterministic_for_equal_timestamps() {
        let now = std::time::UNIX_EPOCH + Duration::from_secs(10_000);
        let modified = now - Duration::from_secs(10);
        let candidates = vec![
            ConfigBackupCandidate::fixture("c", 1, modified),
            ConfigBackupCandidate::fixture("a", 1, modified),
            ConfigBackupCandidate::fixture("b", 1, modified),
        ];

        let pruned = select_config_backups_to_prune(
            candidates,
            now,
            ConfigBackupRetention {
                max_count: 2,
                max_age: Duration::MAX,
                max_bytes: u64::MAX,
            },
        );

        assert_eq!(pruned, vec![PathBuf::from("a")]);
    }

    fn resolve_oauth_fixture(config: &AuthFileConfig) -> auth_config::AuthConfig {
        resolve_auth_with_env(
            Some(config),
            [("LABBY_TOKEN_ENCRYPTION_KEY".to_string(), "11".repeat(32))],
        )
        .expect("OAuth fixture should resolve")
    }

    /// `install_resolved_preferences` must pick up config.toml values when no
    /// overriding env var is set. This test does not touch process env, so
    /// it's safe under both nextest's per-process isolation and cargo test's
    /// threaded model, unlike a test that would need to mutate `std::env`.
    #[test]
    fn install_resolved_preferences_picks_up_config_toml_values() {
        let mut config = LabConfig::default();
        config.mcp.show_all = Some(true);
        config.api.dev_mode = Some(true);
        config.api.protected_mcp_connect_timeout_secs = Some(42);
        config.mcp.catalog_notification_timeout_ms = Some(2_500);
        config.code_mode.widget_callbacks = Some(true);
        config.output.symbols = Some("ascii".to_string());

        install_resolved_preferences(&config);

        assert!(resolved_show_all(), "mcp.show_all should resolve true");
        assert!(resolved_dev_mode(), "api.dev_mode should resolve true");
        assert!(
            resolved_widget_callbacks_enabled(),
            "code_mode.widget_callbacks should resolve true"
        );
        assert_eq!(resolved_symbols().as_deref(), Some("ascii"));
        assert_eq!(resolved_protected_mcp_connect_timeout_secs(), Some(42));
        assert_eq!(
            resolved_catalog_notification_timeout(),
            Duration::from_millis(2_500)
        );

        // Restore defaults so this test doesn't leak state into whichever
        // test the process/thread runs next (matches the existing
        // process_code_mode_enabled restore-after-test convention below).
        install_resolved_preferences(&LabConfig::default());
        assert!(!resolved_show_all());
        assert!(!resolved_dev_mode());
        assert!(!resolved_widget_callbacks_enabled());
        assert_eq!(resolved_symbols(), None);
        assert_eq!(resolved_protected_mcp_connect_timeout_secs(), None);
        assert_eq!(
            resolved_catalog_notification_timeout(),
            Duration::from_millis(DEFAULT_CATALOG_NOTIFICATION_TIMEOUT_MS)
        );
    }

    fn parse_normalized_config(toml: &str) -> LabConfig {
        let mut cfg: LabConfig = toml::from_str(toml).expect("parse");
        cfg.normalize_protected_mcp_routes().expect("normalize");
        cfg
    }

    #[test]
    fn artifact_authority_urls_have_separate_config_contracts() {
        let cfg = parse_normalized_config(
            r#"
[[artifacts.sources]]
id = "primary"
kind = "depot"
endpoint = "https://depot.example/api/artifacts/exact"
control_plane_url = "https://depot.example"
pinned_addresses = ["8.8.8.8"]
bearer_token_env = "LABBY_DEPOT_TOKEN"
"#,
        );
        let source = &cfg.artifacts.sources[0];
        assert_eq!(source.endpoint, "https://depot.example/api/artifacts/exact");
        assert_eq!(
            source.control_plane_url.as_deref(),
            Some("https://depot.example")
        );
        assert_eq!(
            source.bearer_token_env.as_deref(),
            Some("LABBY_DEPOT_TOKEN")
        );
    }

    #[cfg(feature = "gateway")]
    fn openapi_section(toml: &str) -> OpenApiTomlSection {
        toml::from_str::<LabConfig>(toml).expect("parse").openapi
    }

    #[cfg(feature = "gateway")]
    #[test]
    fn openapi_reserved_label_rejected() {
        let toml = r#"[[openapi.specs]]
label = "git"
base_url = "https://api.example.com"
spec_url = "https://api.example.com/openapi.json"
allowed_operations = ["getUser"]"#;
        let err = load_openapi_provider_config(&openapi_section(toml), &|_| None).unwrap_err();
        assert!(matches!(err, ConfigError::ReservedLabel { ref label } if label == "git"));
    }

    #[cfg(feature = "gateway")]
    #[test]
    fn openapi_dotted_label_rejected() {
        // A label containing `.` would misroute the `openapi::<label>.<operationId>`
        // dispatch split — reject it at config load.
        let toml = r#"[[openapi.specs]]
label = "ven.dor"
base_url = "https://api.example.com"
spec_url = "https://api.example.com/openapi.json"
allowed_operations = ["getUser"]"#;
        let err = load_openapi_provider_config(&openapi_section(toml), &|_| None).unwrap_err();
        assert!(matches!(err, ConfigError::InvalidLabel { ref label } if label == "ven.dor"));
    }

    #[cfg(feature = "gateway")]
    #[test]
    fn openapi_missing_base_url_rejected() {
        let toml = r#"[[openapi.specs]]
label = "vendor"
spec_url = "https://api.example.com/openapi.json"
allowed_operations = ["getUser"]"#;
        let err = load_openapi_provider_config(&openapi_section(toml), &|_| None).unwrap_err();
        assert!(matches!(err, ConfigError::MissingBaseUrl { .. }));
    }

    #[cfg(feature = "gateway")]
    #[test]
    fn openapi_duplicate_label_rejected() {
        let toml = r#"[[openapi.specs]]
label = "vendor"
base_url = "https://api.example.com"
spec_url = "https://api.example.com/openapi.json"

[[openapi.specs]]
label = "vendor"
base_url = "https://api2.example.com"
spec_url = "https://api2.example.com/openapi.json""#;
        let err = load_openapi_provider_config(&openapi_section(toml), &|_| None).unwrap_err();
        assert!(matches!(err, ConfigError::DuplicateLabel { ref label } if label == "vendor"));
    }

    #[cfg(feature = "gateway")]
    #[test]
    fn openapi_ambiguous_spec_source_rejected() {
        let toml = r#"[[openapi.specs]]
label = "vendor"
base_url = "https://api.example.com"
spec_url = "https://api.example.com/openapi.json"
spec_path = "/tmp/openapi.json""#;
        let err = load_openapi_provider_config(&openapi_section(toml), &|_| None).unwrap_err();
        assert!(matches!(err, ConfigError::SpecSourceAmbiguous { .. }));
    }

    #[cfg(feature = "gateway")]
    #[test]
    fn openapi_credential_read_from_env_not_toml() {
        let toml = r#"[[openapi.specs]]
label = "vendor"
base_url = "https://api.example.com"
spec_url = "https://api.example.com/openapi.json"
allowed_operations = ["getUser"]"#;
        let env = |k: &str| (k == "OPENAPI_VENDOR_TOKEN").then(|| "tok-123".to_string());
        let cfg = load_openapi_provider_config(&openapi_section(toml), &env).unwrap();
        assert!(cfg.specs[0].credential.is_some());
        // Credential must NEVER round-trip through the TOML struct.
        assert!(!format!("{:?}", cfg.specs[0]).contains("tok-123"));
    }

    #[cfg(feature = "gateway")]
    #[test]
    fn openapi_api_key_uses_configured_header() {
        let toml = r#"[[openapi.specs]]
label = "vendor"
base_url = "https://api.example.com"
spec_url = "https://api.example.com/openapi.json"
api_key_header = "X-Custom-Key""#;
        let env = |k: &str| (k == "OPENAPI_VENDOR_API_KEY").then(|| "sk-abc".to_string());
        let cfg = load_openapi_provider_config(&openapi_section(toml), &env).unwrap();
        match &cfg.specs[0].credential {
            Some(labby_openapi::OpenApiCredential::ApiKey { header, .. }) => {
                assert_eq!(header, "X-Custom-Key");
            }
            other => panic!("expected ApiKey credential, got {other:?}"),
        }
    }

    fn vars<'a>(pairs: &'a [(&'a str, &'a str)]) -> impl Iterator<Item = (String, String)> + 'a {
        pairs
            .iter()
            .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
    }

    #[test]
    fn service_preferences_default_enable_upstream_apis() {
        let cfg = toml::from_str::<LabConfig>("").expect("empty config should parse");
        assert!(cfg.services.built_in_upstream_apis_enabled);
    }

    #[test]
    fn service_preferences_can_disable_upstream_apis() {
        let cfg = toml::from_str::<LabConfig>(
            r"
            [services]
            built_in_upstream_apis_enabled = false
            ",
        )
        .expect("services config should parse");

        assert!(!cfg.services.built_in_upstream_apis_enabled);
    }

    #[test]
    fn patch_built_in_upstream_apis_preserves_comments_and_unknown_sections() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(
            &path,
            r#"# operator note
[services]
# keep this comment
built_in_upstream_apis_enabled = true

[plugin_owned]
future = "keep"
"#,
        )
        .unwrap();

        let cfg = patch_built_in_upstream_apis_enabled(&path, false).unwrap();
        assert!(!cfg.services.built_in_upstream_apis_enabled);
        let raw = std::fs::read_to_string(&path).unwrap();
        assert!(raw.contains("# operator note"));
        assert!(raw.contains("# keep this comment"));
        assert!(raw.contains("[plugin_owned]"));
        assert!(raw.contains("future = \"keep\""));
        assert!(raw.contains("built_in_upstream_apis_enabled = false"));
    }

    #[test]
    fn patch_config_scalars_rejects_non_table_parent_without_mutating() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(&path, "mcp = \"bad\"\n").unwrap();
        let err = patch_config_scalars(
            &path,
            &[ConfigScalarPatch::new(
                "mcp.port",
                ConfigScalarValue::I64(8765),
            )],
        )
        .unwrap_err();
        assert!(
            format!("{err:#}").contains("not a table"),
            "unexpected error: {err:#}"
        );
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "mcp = \"bad\"\n");
    }

    #[test]
    fn patch_config_scalars_updates_inline_table_parent() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(
            &path,
            "services = { built_in_upstream_apis_enabled = true }\n",
        )
        .unwrap();
        let outcome = patch_config_scalars(
            &path,
            &[ConfigScalarPatch::new(
                "services.built_in_upstream_apis_enabled",
                ConfigScalarValue::Bool(false),
            )],
        )
        .unwrap();
        assert!(!outcome.config.services.built_in_upstream_apis_enabled);
        let raw = std::fs::read_to_string(&path).unwrap();
        assert!(raw.contains("built_in_upstream_apis_enabled = false"));
    }

    #[test]
    fn patch_config_scalars_unsets_optional_instead_of_empty_string() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(&path, "[mcp]\nport = 8765\n").unwrap();
        let outcome = patch_config_scalars(
            &path,
            &[ConfigScalarPatch::new(
                "mcp.port",
                ConfigScalarValue::UnsetOptional,
            )],
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
            &[ConfigScalarPatch::new(
                "mcp.port",
                ConfigScalarValue::I64(8765),
            )],
        )
        .unwrap();
        let backup_path = outcome.backup_path.unwrap();
        assert!(backup_path.is_file());
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                std::fs::metadata(&backup_path)
                    .unwrap()
                    .permissions()
                    .mode()
                    & 0o777,
                0o600
            );
            assert_eq!(
                std::fs::metadata(config_lock_path(&path))
                    .unwrap()
                    .permissions()
                    .mode()
                    & 0o777,
                0o600,
                "the persistent config lock must use the secret-file policy"
            );
        }
        let raw = std::fs::read_to_string(&path).unwrap();
        assert!(raw.contains("# keep"));
        assert!(raw.contains("port = 8765"));
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
                0o600,
                "the replacement must not preserve a group/world-readable source mode"
            );
        }
    }

    #[test]
    fn patch_config_scalars_skips_backup_and_write_for_noop() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        let raw = "[mcp]\nport = 8765\n";
        std::fs::write(&path, raw).unwrap();
        let outcome = patch_config_scalars(
            &path,
            &[ConfigScalarPatch::new(
                "mcp.port",
                ConfigScalarValue::I64(8765),
            )],
        )
        .unwrap();
        assert_eq!(outcome.backup_path, None);
        assert_eq!(std::fs::read_to_string(&path).unwrap(), raw);
    }

    #[test]
    fn patch_config_scalars_checked_rejects_stale_expected_value() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        let raw = "[mcp]\nport = 8765\n";
        std::fs::write(&path, raw).unwrap();
        let err = patch_config_scalars_checked(
            &path,
            &[ConfigScalarPatch::new(
                "mcp.port",
                ConfigScalarValue::I64(8766),
            )],
            &[ExpectedConfigScalar::new(
                "mcp.port",
                serde_json::json!(9000),
            )],
        )
        .unwrap_err();
        assert!(
            format!("{err:#}").contains("changed since it was loaded"),
            "unexpected error: {err:#}"
        );
        assert_eq!(std::fs::read_to_string(&path).unwrap(), raw);
    }

    #[test]
    fn resolve_auth_reads_ttls_from_config_toml_fields() {
        let cfg = AuthFileConfig {
            mode: Some("oauth".to_string()),
            public_url: Some("https://lab.example.com".to_string()),
            sqlite_path: None,
            key_path: None,
            bootstrap_secret: Some("bootstrap".to_string()),
            allowed_client_redirect_uris: Some(vec![
                "https://callback.example.com/callback/*".to_string(),
            ]),
            allowed_email_domains: None,
            provider: None,
            authelia_issuer_url: None,
            authelia_client_id: None,
            authelia_client_secret: None,
            authelia_trusted_private_origin: None,
            authelia_ca_certificate_path: None,
            google_client_id: Some("client-id".to_string()),
            google_client_secret: Some("client-secret".to_string()),
            google_callback_path: Some("/auth/google/callback".to_string()),
            google_scopes: Some(vec!["openid".to_string(), "email".to_string()]),
            access_token_ttl_secs: Some(120),
            refresh_token_ttl_secs: Some(3600),
            auth_code_ttl_secs: Some(45),
            admin_email: Some("admin@example.com".to_string()),
            register_requests_per_minute: Some(5),
            authorize_requests_per_minute: Some(15),
            token_requests_per_minute: Some(25),
            machine_clients: None,
            enterprise_issuers: None,
            max_pending_oauth_states: Some(256),
            codex_issuer_compatibility: Some(true),
        };

        let resolved = resolve_oauth_fixture(&cfg);
        assert_eq!(resolved.access_token_ttl.as_secs(), 120);
        assert_eq!(resolved.refresh_token_ttl.as_secs(), 3600);
        assert_eq!(resolved.auth_code_ttl.as_secs(), 45);
        assert_eq!(
            resolved.allowed_client_redirect_uris,
            vec!["https://callback.example.com/callback/*".to_string()]
        );
        assert_eq!(resolved.register_requests_per_minute, 5);
        assert_eq!(resolved.authorize_requests_per_minute, 15);
        assert_eq!(resolved.token_requests_per_minute, 25);
        assert_eq!(resolved.max_pending_oauth_states, 256);
        assert!(resolved.codex_issuer_compatibility);
    }

    fn minimal_oauth_file_config() -> AuthFileConfig {
        AuthFileConfig {
            mode: Some("oauth".to_string()),
            public_url: Some("https://lab.example.com".to_string()),
            bootstrap_secret: Some("bootstrap".to_string()),
            google_client_id: Some("client-id".to_string()),
            google_client_secret: Some("client-secret".to_string()),
            admin_email: Some("admin@example.com".to_string()),
            ..Default::default()
        }
    }

    #[test]
    fn resolve_auth_accepts_authelia_only_file_configuration() {
        let cfg = AuthFileConfig {
            mode: Some("oauth".into()),
            public_url: Some("https://lab.example.com/base".into()),
            provider: Some("authelia".into()),
            authelia_issuer_url: Some("https://auth.example.com/application/o/labby".into()),
            authelia_client_id: Some("labby".into()),
            authelia_client_secret: Some("secret".into()),
            authelia_ca_certificate_path: Some("/etc/labby/authelia-ca.pem".into()),
            admin_email: Some("admin@example.com".into()),
            ..Default::default()
        };
        let resolved = resolve_auth_with_env(
            Some(&cfg),
            [(
                "LABBY_TOKEN_ENCRYPTION_KEY".into(),
                "000102030405060708090a0b0c0d0e0f101112131415161718191a1b1c1d1e1f".into(),
            )],
        )
        .unwrap();
        assert_eq!(
            resolved.inbound_provider,
            Some(auth_config::InboundProviderKind::Authelia)
        );
        assert_eq!(
            resolved.authelia.unwrap().ca_certificate_path,
            Some(PathBuf::from("/etc/labby/authelia-ca.pem"))
        );
        assert!(resolved.google.client_id.is_empty());
    }

    #[test]
    fn resolve_auth_authelia_environment_overrides_file_configuration() {
        let cfg = AuthFileConfig {
            mode: Some("oauth".into()),
            public_url: Some("https://lab.example.com".into()),
            provider: Some("google".into()),
            google_client_id: Some("legacy".into()),
            google_client_secret: Some("legacy-secret".into()),
            admin_email: Some("admin@example.com".into()),
            ..Default::default()
        };
        let resolved = resolve_auth_with_env(
            Some(&cfg),
            [
                ("LABBY_AUTH_PROVIDER".into(), "authelia".into()),
                (
                    "LABBY_AUTHELIA_ISSUER_URL".into(),
                    "https://auth.example.com/application/o/labby".into(),
                ),
                ("LABBY_AUTHELIA_CLIENT_ID".into(), "labby".into()),
                (
                    "LABBY_AUTHELIA_CLIENT_SECRET".into(),
                    "authelia-secret".into(),
                ),
                (
                    "LABBY_AUTHELIA_CA_CERT_PATH".into(),
                    "/run/secrets/authelia-ca.pem".into(),
                ),
                (
                    "LABBY_TOKEN_ENCRYPTION_KEY".into(),
                    "000102030405060708090a0b0c0d0e0f101112131415161718191a1b1c1d1e1f".into(),
                ),
            ],
        )
        .unwrap();
        assert_eq!(
            resolved.inbound_provider,
            Some(auth_config::InboundProviderKind::Authelia)
        );
        let authelia = resolved.authelia.unwrap();
        assert_eq!(authelia.client_id, "labby");
        assert_eq!(
            authelia.ca_certificate_path,
            Some(PathBuf::from("/run/secrets/authelia-ca.pem"))
        );
    }

    #[test]
    fn doctor_projection_uses_resolved_file_only_authelia_provider() {
        let cfg = AuthFileConfig {
            mode: Some("oauth".into()),
            public_url: Some("https://lab.example.com".into()),
            provider: Some("authelia".into()),
            authelia_issuer_url: Some("https://auth.example.com/application/o/labby".into()),
            authelia_client_id: Some("labby".into()),
            authelia_client_secret: Some("secret".into()),
            admin_email: Some("admin@example.com".into()),
            ..Default::default()
        };
        let resolved = resolve_auth_with_env(
            Some(&cfg),
            [(
                "LABBY_TOKEN_ENCRYPTION_KEY".into(),
                "000102030405060708090a0b0c0d0e0f101112131415161718191a1b1c1d1e1f".into(),
            )],
        )
        .unwrap();
        let findings = crate::dispatch::doctor::run_auth_checks_with_config(Some(&resolved));
        let provider = findings
            .iter()
            .find(|finding| finding.check == "auth:provider")
            .unwrap();
        assert!(matches!(
            provider.severity,
            crate::dispatch::doctor::Severity::Ok
        ));
        assert!(provider.message.contains("authelia"));
        assert!(
            !findings
                .iter()
                .any(|finding| finding.check.starts_with("auth:google-")
                    && matches!(finding.severity, crate::dispatch::doctor::Severity::Fail))
        );
    }

    #[test]
    fn resolve_auth_reads_allowed_email_domains_from_config_toml() {
        let mut cfg = minimal_oauth_file_config();
        cfg.allowed_email_domains = Some(vec![
            "Lime-Technology.com".to_string(),
            "@example.org".to_string(),
        ]);

        let resolved = resolve_oauth_fixture(&cfg);

        // Normalized to lowercase with any leading `@` stripped, so operators can
        // write either `example.org` or `@example.org`.
        assert_eq!(
            resolved.allowed_email_domains,
            vec!["lime-technology.com".to_string(), "example.org".to_string()]
        );
    }

    #[test]
    fn resolve_auth_defaults_allowed_email_domains_to_empty() {
        let resolved = resolve_oauth_fixture(&minimal_oauth_file_config());
        assert!(resolved.allowed_email_domains.is_empty());
    }

    #[test]
    fn resolve_auth_preserves_structured_machine_and_enterprise_configuration() {
        let machine = auth_config::MachineClientConfig {
            client_id: "ci-agent".to_string(),
            client_secret: Some("secret".to_string()),
            jwks: None,
            scopes: vec!["lab".to_string()],
            resources: vec!["https://lab.example.com/mcp".to_string()],
        };
        let issuer = auth_config::EnterpriseIssuerConfig {
            issuer: "https://idp.example.com".to_string(),
            jwks_uri: Some("https://idp.example.com/jwks".parse().unwrap()),
            jwks: None,
            allowed_client_ids: vec!["ci-agent".to_string()],
        };
        let cfg = AuthFileConfig {
            mode: Some("oauth".to_string()),
            public_url: Some("https://lab.example.com".to_string()),
            google_client_id: Some("google-client".to_string()),
            google_client_secret: Some("google-secret".to_string()),
            admin_email: Some("admin@example.com".to_string()),
            machine_clients: Some(vec![machine.clone()]),
            enterprise_issuers: Some(vec![issuer.clone()]),
            ..AuthFileConfig::default()
        };

        let resolved = resolve_oauth_fixture(&cfg);
        assert_eq!(resolved.machine_clients, vec![machine]);
        assert_eq!(resolved.enterprise_issuers, vec![issuer]);
    }

    #[test]
    fn resolve_auth_uses_curated_client_redirects_by_default() {
        let cfg = AuthFileConfig {
            mode: Some("oauth".to_string()),
            public_url: Some("https://lab.example.com".to_string()),
            google_client_id: Some("client-id".to_string()),
            google_client_secret: Some("client-secret".to_string()),
            admin_email: Some("admin@example.com".to_string()),
            ..AuthFileConfig::default()
        };

        let resolved = resolve_oauth_fixture(&cfg);

        assert_eq!(
            resolved.allowed_client_redirect_uris,
            vec![
                "https://chatgpt.com/aip/plugin-callback".to_string(),
                "https://chat.openai.com/aip/plugin-callback".to_string(),
                "https://chatgpt.com/connector/oauth/*".to_string(),
                "https://chatgpt.com/connector_platform_oauth_redirect".to_string(),
                "https://claude.ai/api/mcp/auth_callback".to_string(),
                "https://claude.com/api/mcp/auth_callback".to_string(),
            ]
        );
    }

    #[test]
    fn resolve_auth_explicit_empty_redirects_disable_product_defaults() {
        let cfg = AuthFileConfig {
            mode: Some("oauth".to_string()),
            public_url: Some("https://lab.example.com".to_string()),
            google_client_id: Some("client-id".to_string()),
            google_client_secret: Some("client-secret".to_string()),
            admin_email: Some("admin@example.com".to_string()),
            allowed_client_redirect_uris: Some(Vec::new()),
            ..AuthFileConfig::default()
        };

        let resolved = resolve_oauth_fixture(&cfg);

        assert_eq!(resolved.allowed_client_redirect_uris, Vec::<String>::new());
    }

    #[test]
    fn resolve_auth_preserves_explicit_all_https_redirect_opt_in() {
        let cfg = AuthFileConfig {
            mode: Some("oauth".to_string()),
            public_url: Some("https://lab.example.com".to_string()),
            google_client_id: Some("client-id".to_string()),
            google_client_secret: Some("client-secret".to_string()),
            admin_email: Some("admin@example.com".to_string()),
            allowed_client_redirect_uris: Some(vec!["https://*".to_string()]),
            ..AuthFileConfig::default()
        };

        let resolved = resolve_oauth_fixture(&cfg);

        assert_eq!(
            resolved.allowed_client_redirect_uris,
            vec!["https://*".to_string()]
        );
    }

    #[test]
    fn oauth_machine_config_deserializes() {
        let cfg = toml::from_str::<LabConfig>(
            r#"
[oauth.machines.node-a]
target_url = "http://100.64.0.10:38935/callback/node-a"
description = "Node A Claude callback target"
default_port = 38935
"#,
        )
        .expect("oauth machine config should parse");

        assert_eq!(
            cfg.oauth.machines["node-a"].target_url,
            "http://100.64.0.10:38935/callback/node-a"
        );
        assert_eq!(
            cfg.oauth.machines["node-a"].description.as_deref(),
            Some("Node A Claude callback target")
        );
        assert_eq!(cfg.oauth.machines["node-a"].default_port, Some(38935));
    }

    #[test]
    fn oauth_machine_defaults_keep_partial_configs_valid() {
        let cfg = toml::from_str::<LabConfig>(
            r#"
[web]
assets_dir = "/tmp/labby"
"#,
        )
        .expect("config without oauth section should still parse");

        assert!(cfg.oauth.machines.is_empty());
        assert_eq!(cfg.web.assets_dir, Some(PathBuf::from("/tmp/labby")));
    }

    #[test]
    fn quarantined_virtual_servers_round_trip_through_toml() {
        let raw = r#"
[[quarantined_virtual_servers]]
id = "missing-service"
service = "missing-service"
enabled = true

[quarantined_virtual_servers.surfaces]
mcp = true
"#;
        let cfg = toml::from_str::<LabConfig>(raw).expect("quarantine config should parse");
        assert_eq!(cfg.quarantined_virtual_servers.len(), 1);
        assert_eq!(cfg.quarantined_virtual_servers[0].id, "missing-service");
        assert_eq!(
            cfg.quarantined_virtual_servers[0].service,
            "missing-service"
        );
        assert!(cfg.quarantined_virtual_servers[0].surfaces.mcp);

        let serialized = toml::to_string(&cfg).expect("config should serialize");
        let reparsed =
            toml::from_str::<LabConfig>(&serialized).expect("serialized config should parse");
        assert_eq!(reparsed.quarantined_virtual_servers.len(), 1);
        assert_eq!(
            reparsed.quarantined_virtual_servers[0].id,
            "missing-service"
        );
    }

    #[test]
    fn workspace_root_defaults_under_labby_home() {
        let cfg = toml::from_str::<LabConfig>("").expect("empty config should parse");
        let home = Path::new("/tmp/lab-home");

        assert_eq!(
            workspace_root_for_home(&cfg, home),
            home.join(".labby").join("workspace")
        );
    }

    #[test]
    fn file_stash_defaults_are_bounded_and_invalid_limits_fail_startup_validation() {
        let mut config = LabConfig::default();
        assert_eq!(config.file_stash.max_file_bytes, 104_857_600);
        assert_eq!(config.file_stash.principal_quota_bytes, 1_073_741_824);
        assert_eq!(config.file_stash.instance_quota_bytes, 10_737_418_240);
        assert_eq!(config.file_stash.max_live_files_per_instance, 100_000);
        assert_eq!(config.file_stash.queue_capacity, 64);
        assert_eq!(config.file_stash.max_concurrent_uploads_per_principal, 2);
        assert_eq!(config.file_stash.max_concurrent_uploads_per_instance, 8);
        assert_eq!(config.file_stash.max_concurrent_downloads, 16);
        assert_eq!(config.file_stash.download_idle_seconds, 30);
        assert_eq!(config.file_stash.download_total_seconds, 600);
        assert!(config.validate().is_ok());
        config.file_stash.queue_capacity = 0;
        assert!(config.validate().is_err());
        let mut config = LabConfig::default();
        config.file_stash.pending_ttl_seconds = config.file_stash.upload_total_seconds;
        assert!(config.validate().is_err());
    }

    #[test]
    fn file_stash_rejects_every_resource_limit_outside_its_boundary() {
        macro_rules! invalid {
            ($field:ident, $value:expr) => {{
                let mut config = LabConfig::default();
                config.file_stash.$field = $value;
                assert!(
                    config.validate().is_err(),
                    "{} accepted an invalid boundary value",
                    stringify!($field)
                );
            }};
        }
        invalid!(max_file_bytes, 0);
        invalid!(principal_quota_bytes, 0);
        invalid!(instance_quota_bytes, 0);
        invalid!(max_live_files_per_principal, 0);
        invalid!(max_live_files_per_instance, 1);
        invalid!(page_size, 0);
        invalid!(max_query_bytes, 0);
        invalid!(max_header_bytes, 0);
        invalid!(grant_recipients_page_size, 0);
        invalid!(max_mcp_read_bytes, 0);
        invalid!(queue_capacity, 0);
        invalid!(database_deadline_ms, 0);
        invalid!(max_concurrent_uploads_per_principal, 0);
        invalid!(max_concurrent_uploads_per_instance, 0);
        invalid!(max_concurrent_downloads, 0);
        invalid!(max_concurrent_mcp_reads, 0);
        invalid!(upload_idle_seconds, 0);
        invalid!(upload_total_seconds, 0);
        invalid!(download_idle_seconds, 0);
        invalid!(download_total_seconds, 0);
        invalid!(pending_ttl_seconds, 0);
        invalid!(janitor_batch_size, 0);
        invalid!(janitor_backoff_max_seconds, 0);
        invalid!(janitor_interval_seconds, 0);

        invalid!(max_file_bytes, 1_073_741_825);
        invalid!(principal_quota_bytes, 107_374_182_401);
        invalid!(instance_quota_bytes, 1_099_511_627_777);
        invalid!(max_live_files_per_principal, 100_001);
        invalid!(max_live_files_per_instance, 1_000_001);
        invalid!(page_size, 201);
        invalid!(max_query_bytes, 1_025);
        invalid!(max_header_bytes, 65_537);
        invalid!(grant_recipients_page_size, 201);
        invalid!(max_mcp_read_bytes, 26_214_401);
        invalid!(queue_capacity, 1_025);
        invalid!(database_deadline_ms, 30_001);
        invalid!(max_concurrent_uploads_per_principal, 3);
        invalid!(max_concurrent_uploads_per_instance, 9);
        invalid!(max_concurrent_downloads, 257);
        invalid!(max_concurrent_mcp_reads, 5);
        invalid!(upload_idle_seconds, 31);
        invalid!(upload_total_seconds, 601);
        invalid!(download_idle_seconds, 31);
        invalid!(download_total_seconds, 601);
        invalid!(pending_ttl_seconds, 1_801);
        invalid!(janitor_batch_size, 101);
        invalid!(janitor_backoff_max_seconds, 301);
        invalid!(janitor_interval_seconds, 3_601);

        let mut config = LabConfig::default();
        config.file_stash.janitor_backoff_max_seconds = 1;
        assert!(config.validate().is_err());
    }

    #[test]
    fn workspace_root_reads_config_toml_value() {
        let cfg = toml::from_str::<LabConfig>(
            r#"
[workspace]
root = "/srv/labby-workspace"
"#,
        )
        .expect("workspace config should parse");

        assert_eq!(
            workspace_root_for_home(&cfg, Path::new("/tmp/ignored")),
            PathBuf::from("/srv/labby-workspace")
        );
    }

    #[test]
    fn web_ui_auth_disabled_env_prefers_canonical_alias() {
        let setting = resolve_web_ui_auth_disabled_values(Some("true"), Some("false"))
            .expect("env values should parse")
            .expect("setting should resolve");

        assert!(setting.disabled);
        assert_eq!(setting.source, WEB_UI_AUTH_DISABLED_ENV);
        assert!(!setting.legacy_alias);
    }

    #[test]
    fn web_ui_auth_disabled_env_accepts_legacy_alias() {
        let setting = resolve_web_ui_auth_disabled_values(None, Some("1"))
            .expect("env values should parse")
            .expect("setting should resolve");

        assert!(setting.disabled);
        assert_eq!(setting.source, WEB_UI_AUTH_DISABLED_LEGACY_ENV);
        assert!(setting.legacy_alias);
    }

    #[test]
    fn web_ui_auth_disabled_env_rejects_invalid_values() {
        let error = resolve_web_ui_auth_disabled_values(Some("sometimes"), None)
            .expect_err("invalid bool should fail");

        assert!(
            error
                .to_string()
                .contains("invalid LABBY_WEB_UI_AUTH_DISABLED value")
        );
    }

    #[test]
    fn secret_debug_redacts() {
        let s = Secret::new("hunter2".into());
        assert_eq!(format!("{s:?}"), "[REDACTED]");
        assert_eq!(format!("{s}"), "[REDACTED]");
        assert_eq!(s.expose(), "hunter2");
    }

    #[test]
    fn secret_serialize_emits_placeholder_not_plaintext() {
        let s = Secret::new("super-secret-api-key".into());
        let json = serde_json::to_string(&s).expect("serialize must not fail");
        assert_eq!(
            json, "\"***REDACTED***\"",
            "Secret must serialize to placeholder"
        );
        assert!(
            !json.contains("super-secret-api-key"),
            "Secret must never emit plaintext through serde"
        );
    }

    #[test]
    fn suffix_collision_longest_wins() {
        let env = [("S_NODE_API_KEY_URL", "http://example.com")];
        let result = scan_instances_from("S", vars(&env));
        let inst = result
            .get("node_api_key")
            .expect("should find instance node_api_key");
        assert_eq!(
            inst.get("URL").expect("should have URL").expose(),
            "http://example.com"
        );
    }

    #[test]
    fn default_instance_parsed() {
        let env = [
            ("SVC_URL", "http://localhost"),
            ("SVC_API_KEY", "secret123"),
        ];
        let result = scan_instances_from("SVC", vars(&env));
        let def = result.get("default").expect("should find default");
        assert_eq!(def.get("URL").expect("URL").expose(), "http://localhost");
        assert_eq!(def.get("API_KEY").expect("API_KEY").expose(), "secret123");
        assert!(format!("{:?}", def.get("API_KEY").unwrap()).contains("[REDACTED]"));
    }

    #[test]
    fn named_instance_parsed() {
        let env = [
            ("UNRAID_NODE2_URL", "http://node2"),
            ("UNRAID_NODE2_TOKEN", "tok"),
        ];
        let result = scan_instances_from("UNRAID", vars(&env));
        let inst = result.get("node2").expect("should find node2");
        assert_eq!(inst.get("URL").expect("URL").expose(), "http://node2");
        assert_eq!(inst.get("TOKEN").expect("TOKEN").expose(), "tok");
        assert!(format!("{:?}", inst.get("TOKEN").unwrap()).contains("[REDACTED]"));
    }

    #[test]
    fn unrelated_vars_ignored() {
        let env = [
            ("SVC_URL", "http://localhost"),
            ("OTHER_URL", "http://other"),
        ];
        let result = scan_instances_from("SVC", vars(&env));
        assert_eq!(result.len(), 1);
        assert!(result.contains_key("default"));
    }

    #[test]
    fn username_is_plain_not_secret() {
        let env = [("SVC_USERNAME", "admin")];
        let result = scan_instances_from("SVC", vars(&env));
        let def = result.get("default").expect("should find default");
        assert!(!format!("{:?}", def.get("USERNAME").unwrap()).contains("[REDACTED]"));
    }

    // ─── write_service_creds tests ──────────────────────────────────────────

    fn example_cred() -> EnvCredential {
        EnvCredential {
            service: "example".to_owned(),
            url: Some("http://localhost:7878".to_owned()),
            secret: Some("abc123".to_owned()),
            env_field: "EXAMPLE_API_KEY".to_owned(),
        }
    }

    #[test]
    fn write_service_creds_adds_new_keys() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(".env");
        let outcome = write_service_creds(&path, &[example_cred()], false).unwrap();
        assert!(outcome.skipped.is_empty());
        assert_eq!(outcome.written, 2);
        let content = std::fs::read_to_string(&path).unwrap();
        assert!(content.contains("EXAMPLE_URL=http://localhost:7878"));
        assert!(content.contains("EXAMPLE_API_KEY=abc123"));
    }

    #[test]
    fn write_service_creds_preserves_comments_and_blanks() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(".env");
        std::fs::write(&path, "# my comment\nOTHER=val\n").unwrap();
        write_service_creds(&path, &[example_cred()], false).unwrap();
        let content = std::fs::read_to_string(&path).unwrap();
        assert!(content.contains("# my comment"));
        assert!(content.contains("OTHER=val"));
    }

    #[test]
    fn write_service_creds_conflict_skip_without_force() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(".env");
        std::fs::write(&path, "EXAMPLE_API_KEY=oldvalue\n").unwrap();
        let outcome = write_service_creds(&path, &[example_cred()], false).unwrap();
        assert!(!outcome.skipped.is_empty());
        let content = std::fs::read_to_string(&path).unwrap();
        assert!(content.contains("oldvalue"));
        assert!(!content.contains("abc123"));
    }

    #[test]
    fn write_service_creds_conflict_overwrite_with_force() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(".env");
        std::fs::write(&path, "EXAMPLE_API_KEY=oldvalue\n").unwrap();
        let outcome = write_service_creds(&path, &[example_cred()], true).unwrap();
        assert!(outcome.skipped.is_empty());
        let content = std::fs::read_to_string(&path).unwrap();
        assert!(content.contains("abc123"));
        assert!(!content.contains("oldvalue"));
    }

    #[test]
    fn write_service_creds_is_idempotent_when_matching() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(".env");
        write_service_creds(&path, &[example_cred()], false).unwrap();
        // Re-running with the exact same creds must be a written=0 no-op --
        // this is the signal crate::dispatch::gateway::config_store relies on
        // to skip a service-client refresh cycle.
        let outcome = write_service_creds(&path, &[example_cred()], false).unwrap();
        assert_eq!(outcome.written, 0);
        assert!(outcome.backup_path.is_none());
    }

    #[test]
    fn write_service_creds_quotes_value_with_special_chars() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(".env");
        let cred = EnvCredential {
            service: "svc".to_owned(),
            url: None,
            secret: Some("has space".to_owned()),
            env_field: "SVC_KEY".to_owned(),
        };
        write_service_creds(&path, &[cred], false).unwrap();
        let content = std::fs::read_to_string(&path).unwrap();
        assert!(content.contains("SVC_KEY=\"has space\""));
    }

    #[test]
    fn upstream_oauth_pkce_parses() {
        let cfg = toml::from_str::<LabConfig>(
            r#"
[[upstream]]
name = "acme"
url = "https://acme.example.com/mcp"

[upstream.oauth]
mode = "authorization_code_pkce"
scopes = ["mcp"]

[upstream.oauth.registration]
strategy = "client_metadata_document"
url = "https://acme.example.com/.well-known/oauth-client"
"#,
        )
        .expect("pkce config should parse");

        let upstream = &cfg.upstream[0];
        let oauth = upstream.oauth.as_ref().expect("oauth present");
        assert!(matches!(
            oauth.mode,
            UpstreamOauthMode::AuthorizationCodePkce
        ));
        assert_eq!(oauth.scopes.as_deref(), Some(&["mcp".to_string()][..]));
        match &oauth.registration {
            UpstreamOauthRegistration::ClientMetadataDocument { url } => {
                assert_eq!(url, "https://acme.example.com/.well-known/oauth-client");
            }
            other => panic!("unexpected registration: {other:?}"),
        }
        upstream.validate().expect("validate ok");
    }

    #[test]
    fn upstream_oauth_preregistered_parses() {
        let cfg = toml::from_str::<LabConfig>(
            r#"
[[upstream]]
name = "acme"
url = "https://acme.example.com/mcp"

[upstream.oauth]
mode = "authorization_code_pkce"

[upstream.oauth.registration]
strategy = "preregistered"
client_id = "my-client"
"#,
        )
        .expect("preregistered config should parse");

        let upstream = &cfg.upstream[0];
        let oauth = upstream.oauth.as_ref().unwrap();
        match &oauth.registration {
            UpstreamOauthRegistration::Preregistered {
                client_id,
                client_secret_env,
            } => {
                assert_eq!(client_id, "my-client");
                assert!(client_secret_env.is_none());
            }
            other => panic!("unexpected registration: {other:?}"),
        }
    }

    #[test]
    fn upstream_oauth_google_provider_credential_source_parses() {
        let cfg = toml::from_str::<LabConfig>(
            r#"
[[upstream]]
name = "google-calendar"
url = "https://calendarmcp.googleapis.com/mcp/v1"

[upstream.oauth]
mode = "authorization_code_pkce"
scopes = ["https://www.googleapis.com/auth/calendar.events.readonly"]

[upstream.oauth.credential]
source = "google_provider"
account = "admin@example.com"

[upstream.oauth.registration]
strategy = "preregistered"
client_id = "google-client"
client_secret_env = "LABBY_GOOGLE_CLIENT_SECRET"
"#,
        )
        .expect("google provider credential config should parse");

        let oauth = cfg.upstream[0].oauth.as_ref().unwrap();
        assert_eq!(
            oauth.credential,
            UpstreamOauthCredentialSource::GoogleProvider {
                account: Some("admin@example.com".to_string()),
            }
        );
        cfg.upstream[0]
            .validate()
            .expect("shared Google configuration should validate");
    }

    #[test]
    fn upstream_oauth_credential_source_defaults_to_dedicated() {
        let cfg = toml::from_str::<LabConfig>(
            r#"
[[upstream]]
name = "acme"
url = "https://acme.example.com/mcp"

[upstream.oauth]
mode = "authorization_code_pkce"

[upstream.oauth.registration]
strategy = "preregistered"
client_id = "my-client"
"#,
        )
        .unwrap();
        assert_eq!(
            cfg.upstream[0].oauth.as_ref().unwrap().credential,
            UpstreamOauthCredentialSource::Dedicated
        );
    }

    #[test]
    fn upstream_oauth_preregistered_with_secret_parses() {
        let cfg = toml::from_str::<LabConfig>(
            r#"
[[upstream]]
name = "acme"
url = "https://acme.example.com/mcp"

[upstream.oauth]
mode = "authorization_code_pkce"

[upstream.oauth.registration]
strategy = "preregistered"
client_id = "my-client"
client_secret_env = "ACME_CLIENT_SECRET"
"#,
        )
        .expect("preregistered+secret config should parse");

        let upstream = &cfg.upstream[0];
        let oauth = upstream.oauth.as_ref().unwrap();
        match &oauth.registration {
            UpstreamOauthRegistration::Preregistered {
                client_id,
                client_secret_env,
            } => {
                assert_eq!(client_id, "my-client");
                assert_eq!(client_secret_env.as_deref(), Some("ACME_CLIENT_SECRET"));
            }
            other => panic!("unexpected registration: {other:?}"),
        }
    }

    #[test]
    fn upstream_oauth_dynamic_parses() {
        let cfg = toml::from_str::<LabConfig>(
            r#"
[[upstream]]
name = "acme"
url = "https://acme.example.com/mcp"

[upstream.oauth]
mode = "authorization_code_pkce"

[upstream.oauth.registration]
strategy = "dynamic"
"#,
        )
        .expect("dynamic config should parse");

        let upstream = &cfg.upstream[0];
        let oauth = upstream.oauth.as_ref().unwrap();
        assert!(matches!(
            oauth.registration,
            UpstreamOauthRegistration::Dynamic
        ));
    }

    #[test]
    fn upstream_oauth_conflicts_with_bearer_token_env() {
        let cfg = toml::from_str::<LabConfig>(
            r#"
[[upstream]]
name = "acme"
url = "https://acme.example.com/mcp"
bearer_token_env = "ACME_TOKEN"

[upstream.oauth]
mode = "authorization_code_pkce"

[upstream.oauth.registration]
strategy = "dynamic"
"#,
        )
        .expect("config parses; validation is a separate step");

        let err = cfg.upstream[0].validate().unwrap_err();
        match err {
            ConfigError::ConflictingAuth { name } => assert_eq!(name, "acme"),
            other => panic!("expected ConflictingAuth, got {other:?}"),
        }
    }

    #[test]
    fn code_mode_is_root_level_config() {
        let cfg = toml::from_str::<LabConfig>(
            r#"
[code_mode]
enabled = true
timeout_ms = 2500

[[upstream]]
name = "acme"
url = "https://acme.example.com/mcp"
"#,
        )
        .expect("root code_mode parses");

        assert!(cfg.code_mode.enabled);
        assert_eq!(cfg.code_mode.timeout_ms, 2500);
        cfg.validate().expect("root code_mode validates");
    }

    #[test]
    fn code_mode_is_root_level_config_with_default_limits() {
        let default_cfg = LabConfig::default();
        assert_eq!(default_cfg.code_mode.timeout_ms, 30_000);
        assert_eq!(default_cfg.code_mode.max_source_bytes, 128 * 1024);
        assert_eq!(default_cfg.code_mode.max_response_bytes, 24 * 1024);
        assert_eq!(default_cfg.code_mode.max_response_tokens, 6000);

        let cfg = toml::from_str::<LabConfig>(
            r"
[code_mode]
timeout_ms = 2500
max_source_bytes = 65536
max_response_bytes = 12000
max_response_tokens = 3000
",
        )
        .expect("root code_mode parses");

        assert_eq!(cfg.code_mode.timeout_ms, 2500);
        assert_eq!(cfg.code_mode.max_source_bytes, 65_536);
        assert_eq!(cfg.code_mode.max_response_bytes, 12000);
        assert_eq!(cfg.code_mode.max_response_tokens, 3000);
    }

    #[test]
    fn upstream_request_timeout_is_root_level_config() {
        let default_cfg = LabConfig::default();
        assert_eq!(
            default_cfg.upstream_request_timeout(),
            Duration::from_secs(30)
        );

        let cfg = toml::from_str::<LabConfig>(
            r"
upstream_request_timeout_ms = 60000
",
        )
        .expect("root upstream request timeout parses");

        assert_eq!(cfg.upstream_request_timeout_ms, Some(60_000));
        assert_eq!(cfg.upstream_request_timeout(), Duration::from_mins(1));
        cfg.validate().expect("timeout validates");
    }

    /// The HTTP transport backstop must never fire before the upstream deadline
    /// it wraps. A fixed 30s cap in the router used to override both settings:
    /// a 60s `upstream_request_timeout_ms` still returned a bare 504 at 30s,
    /// discarding a tool call that went on to succeed.
    #[test]
    fn http_request_timeout_never_undercuts_configured_upstream_deadlines() {
        for toml_src in [
            "",
            "upstream_request_timeout_ms = 60000",
            // Both knobs at the top of their validated ranges.
            "upstream_request_timeout_ms = 300000\nupstream_relay_timeout_ms = 1800000",
            // Relay left at its 5 minute default while the pooled path is raised.
            "upstream_request_timeout_ms = 120000",
            // Relay raised while the pooled path stays at its default.
            "upstream_relay_timeout_ms = 900000",
        ] {
            let cfg = toml::from_str::<LabConfig>(toml_src).expect("config parses");
            cfg.validate().expect("config validates");

            let http = cfg.http_request_timeout();
            assert!(
                http > cfg.upstream_request_timeout(),
                "http timeout {http:?} must exceed the pooled upstream deadline {:?} for {toml_src:?}",
                cfg.upstream_request_timeout(),
            );
            assert!(
                http > cfg.upstream_relay_timeout(),
                "http timeout {http:?} must exceed the relay deadline {:?} for {toml_src:?}",
                cfg.upstream_relay_timeout(),
            );
        }
    }

    /// The 5 minute relay default is the binding constraint out of the box, so
    /// a default deployment must not cap HTTP requests at the 30s pooled value.
    #[test]
    fn http_request_timeout_default_accommodates_the_relay_path() {
        let cfg = LabConfig::default();
        assert_eq!(
            cfg.http_request_timeout(),
            cfg.upstream_relay_timeout() + HTTP_REQUEST_TIMEOUT_MARGIN,
        );
        assert!(cfg.http_request_timeout() > Duration::from_secs(30));
    }

    #[test]
    fn upstream_relay_timeout_defaults_to_five_minutes_and_is_configurable() {
        // Unset → 5 minute default (NOT the 30s request-timeout default), so a
        // relayed elicitation is not aborted while a human is answering.
        let default_cfg = LabConfig::default();
        assert_eq!(default_cfg.upstream_relay_timeout_ms, None);
        assert_eq!(default_cfg.upstream_relay_timeout(), Duration::from_mins(5));

        let cfg = toml::from_str::<LabConfig>(
            r"
upstream_relay_timeout_ms = 600000
",
        )
        .expect("root upstream relay timeout parses");
        assert_eq!(cfg.upstream_relay_timeout_ms, Some(600_000));
        assert_eq!(cfg.upstream_relay_timeout(), Duration::from_mins(10));
        cfg.validate().expect("relay timeout validates");
    }

    #[test]
    fn upstream_relay_timeout_rejects_out_of_range() {
        // Above the 30 min ceiling.
        let too_big = LabConfig {
            upstream_relay_timeout_ms: Some(1_800_001),
            ..LabConfig::default()
        };
        assert!(matches!(
            too_big.validate(),
            Err(ConfigError::InvalidUpstreamRelayTimeout { value: 1_800_001 })
        ));

        // Zero is rejected just like the request timeout.
        let zero = LabConfig {
            upstream_relay_timeout_ms: Some(0),
            ..LabConfig::default()
        };
        assert!(matches!(
            zero.validate(),
            Err(ConfigError::InvalidUpstreamRelayTimeout { value: 0 })
        ));
    }

    #[test]
    fn catalog_notification_timeout_defaults_to_five_seconds_and_is_configurable() {
        let default_cfg = LabConfig::default();
        assert_eq!(default_cfg.mcp.catalog_notification_timeout_ms, None);

        let cfg = toml::from_str::<LabConfig>(
            r"
[mcp]
catalog_notification_timeout_ms = 2500
",
        )
        .expect("mcp catalog notification timeout parses");

        assert_eq!(cfg.mcp.catalog_notification_timeout_ms, Some(2_500));
        cfg.validate()
            .expect("catalog notification timeout validates");
    }

    #[test]
    fn catalog_notification_timeout_rejects_out_of_range() {
        let too_big = LabConfig {
            mcp: McpPreferences {
                catalog_notification_timeout_ms: Some(60_001),
                ..McpPreferences::default()
            },
            ..LabConfig::default()
        };
        assert!(matches!(
            too_big.validate(),
            Err(ConfigError::InvalidCatalogNotificationTimeout { value: 60_001 })
        ));

        let zero = LabConfig {
            mcp: McpPreferences {
                catalog_notification_timeout_ms: Some(0),
                ..McpPreferences::default()
            },
            ..LabConfig::default()
        };
        assert!(matches!(
            zero.validate(),
            Err(ConfigError::InvalidCatalogNotificationTimeout { value: 0 })
        ));
    }

    #[test]
    fn code_mode_validation_rejects_unbounded_execution_settings() {
        let cfg = toml::from_str::<LabConfig>(
            r"
[code_mode]
timeout_ms = 0
",
        )
        .expect("code_mode parses");
        assert!(matches!(
            cfg.validate(),
            Err(ConfigError::InvalidCodeModeTimeout { value: 0 })
        ));

        let cfg = toml::from_str::<LabConfig>(
            r"
[code_mode]
timeout_ms = 5000
max_response_bytes = 100
",
        )
        .expect("code_mode parses");
        assert!(matches!(
            cfg.validate(),
            Err(ConfigError::InvalidCodeModeMaxResponseBytes { value: 100 })
        ));

        let cfg = toml::from_str::<LabConfig>(
            r"
[code_mode]
timeout_ms = 5000
max_response_tokens = 100
",
        )
        .expect("code_mode parses");
        assert!(matches!(
            cfg.validate(),
            Err(ConfigError::InvalidCodeModeMaxResponseTokens { value: 100 })
        ));
    }

    #[test]
    fn protected_route_legacy_backend_path_folds_into_backend_url() {
        let mut cfg = toml::from_str::<LabConfig>(
            r#"
[[protected_mcp_routes]]
name = "tools"
enabled = true
public_host = "mcp.example.com"
public_path = "/tools"
backend_url = "http://10.0.0.12:3100"
backend_mcp_path = "/mcp"
"#,
        )
        .expect("protected route parses");

        cfg.normalize_protected_mcp_routes()
            .expect("protected route normalizes");

        assert_eq!(
            cfg.protected_mcp_routes[0].backend_url,
            "http://10.0.0.12:3100/mcp"
        );
        assert_eq!(cfg.protected_mcp_routes[0].backend_mcp_path, "/mcp");
    }

    #[test]
    fn protected_route_named_upstream_allows_empty_backend_url() {
        let mut cfg = toml::from_str::<LabConfig>(
            r#"
[[protected_mcp_routes]]
name = "telemetry"
enabled = true
public_host = "mcp.example.com"
public_path = "/telemetry"
upstream = " telemetry "
"#,
        )
        .expect("protected route parses");

        cfg.normalize_protected_mcp_routes()
            .expect("upstream route normalizes");

        assert_eq!(
            cfg.protected_mcp_routes[0].upstream.as_deref(),
            Some("telemetry")
        );
        assert_eq!(cfg.protected_mcp_routes[0].backend_url, "");
        assert_eq!(cfg.protected_mcp_routes[0].backend_mcp_path, "/mcp");
    }

    #[test]
    fn protected_route_gateway_subset_target_parses() {
        let toml = r#"
[[protected_mcp_routes]]
name = "ops"
public_host = "mcp.example.com"
public_path = "/ops"
scopes = ["mcp:ops"]

[protected_mcp_routes.target]
kind = "gateway_subset"
upstreams = ["gateway-alpha", "gateway-beta", " gateway-gamma "]
services = ["gateway"]
expose_code_mode = true
"#;

        let cfg = parse_normalized_config(toml);
        let route = &cfg.protected_mcp_routes[0];

        assert_eq!(route.name, "ops");
        assert_eq!(route.backend_url, "");
        assert_eq!(route.upstream, None);
        assert!(route.is_gateway_subset());
        let target = route.gateway_subset_target().expect("gateway subset");
        assert_eq!(
            target.upstreams,
            vec!["gateway-alpha", "gateway-beta", "gateway-gamma"]
        );
        assert_eq!(target.services, vec!["gateway"]);
        assert!(target.expose_code_mode);
    }

    #[test]
    fn protected_route_legacy_backend_url_maps_to_proxy_target() {
        let toml = r#"
[[protected_mcp_routes]]
name = "telemetry"
public_host = "mcp.example.com"
public_path = "/telemetry"
backend_url = "http://10.0.0.2:3100/mcp"
"#;

        let cfg = parse_normalized_config(toml);
        let route = &cfg.protected_mcp_routes[0];

        assert!(matches!(
            route.effective_target(),
            ProtectedMcpRouteEffectiveTarget::BackendUrl { .. }
        ));
    }

    #[test]
    fn protected_route_rejects_target_with_legacy_backend() {
        let toml = r#"
[[protected_mcp_routes]]
name = "bad"
public_host = "mcp.example.com"
public_path = "/bad"
backend_url = "http://10.0.0.2:3100/mcp"

[protected_mcp_routes.target]
kind = "gateway_subset"
upstreams = ["gateway-beta"]
"#;

        let mut cfg: LabConfig = toml::from_str(toml).expect("parse");
        let err = cfg
            .normalize_protected_mcp_routes()
            .expect_err("target and backend_url must conflict");
        assert!(err.to_string().contains(
            "protected MCP route target cannot be combined with upstream or backend_url"
        ));
    }

    #[test]
    fn protected_route_rejects_empty_gateway_subset_entries() {
        let toml = r#"
[[protected_mcp_routes]]
name = "bad"
public_host = "mcp.example.com"
public_path = "/bad"

[protected_mcp_routes.target]
kind = "gateway_subset"
upstreams = ["gateway-alpha", " "]
"#;

        let mut cfg: LabConfig = toml::from_str(toml).expect("parse");
        let err = cfg
            .normalize_protected_mcp_routes()
            .expect_err("empty upstream entry must fail");
        assert!(err.to_string().contains("target.upstreams"));
        assert!(
            err.to_string()
                .contains("gateway_subset target entries must not be empty")
        );
    }

    #[test]
    fn protected_route_allows_same_gateway_subset_path_on_different_hosts() {
        let toml = r#"
[[upstream]]
name = "gateway-alpha"
enabled = false
url = "https://gateway-alpha.example.com/mcp"

[[protected_mcp_routes]]
name = "media-a"
public_host = "mcp-a.example.com"
public_path = "/ops"

[protected_mcp_routes.target]
kind = "gateway_subset"
upstreams = ["gateway-alpha"]

[[protected_mcp_routes]]
name = "media-b"
public_host = "mcp-b.example.com"
public_path = "/ops"

[protected_mcp_routes.target]
kind = "gateway_subset"
upstreams = ["gateway-alpha"]
"#;

        let mut cfg: LabConfig = toml::from_str(toml).expect("parse");
        cfg.normalize_protected_mcp_routes()
            .expect("host and path together identify a protected route");
        cfg.validate()
            .expect("same subset path on distinct hosts is valid");
    }

    #[test]
    fn config_validation_rejects_reserved_protected_route_path() {
        let toml = r#"
[[protected_mcp_routes]]
name = "bad"
public_host = "mcp.example.com"
public_path = "/v1"
backend_url = "http://10.0.0.2:3100/mcp"
"#;

        let mut cfg: LabConfig = toml::from_str(toml).expect("parse");
        cfg.normalize_protected_mcp_routes()
            .expect("normalization should not hide validation failure");
        let err = cfg
            .validate()
            .expect_err("reserved protected route path must fail validation");

        assert!(err.to_string().contains("public_path"));
        assert!(err.to_string().contains("reserved"));
    }

    #[test]
    fn config_validation_rejects_public_callback_relay_protected_route_path() {
        let toml = r#"
[[protected_mcp_routes]]
name = "bad"
public_host = "mcp.example.com"
public_path = "/callback/devhost"
backend_url = "http://10.0.0.2:3100/mcp"
"#;

        let mut cfg: LabConfig = toml::from_str(toml).expect("parse");
        cfg.normalize_protected_mcp_routes()
            .expect("normalization should not hide validation failure");
        let err = cfg
            .validate()
            .expect_err("callback relay protected route path must fail validation");

        assert!(err.to_string().contains("public_path"));
        assert!(err.to_string().contains("reserved"));
    }

    #[test]
    fn config_validation_rejects_empty_gateway_subset_target() {
        let toml = r#"
[[protected_mcp_routes]]
name = "empty"
public_host = "mcp.example.com"
public_path = "/empty"

[protected_mcp_routes.target]
kind = "gateway_subset"
"#;

        let mut cfg: LabConfig = toml::from_str(toml).expect("parse");
        cfg.normalize_protected_mcp_routes()
            .expect("normalization should not hide validation failure");
        let err = cfg
            .validate()
            .expect_err("empty gateway_subset target must fail validation");

        assert!(err.to_string().contains("gateway_subset target"));
    }

    #[test]
    fn config_validation_accepts_gateway_subset_loadout_target() {
        let toml = r#"
[[upstream]]
name = "gateway-alpha"
enabled = false
url = "https://gateway-alpha.example.com/mcp"

[[loadouts]]
name = "sd"
upstreams = ["gateway-alpha"]

[[protected_mcp_routes]]
name = "sd"
public_host = "sd.example.com"
public_path = "/mcp"
scopes = ["mcp:read"]

[protected_mcp_routes.target]
kind = "gateway_subset"
loadout = " sd "
"#;
        let mut cfg: LabConfig = toml::from_str(toml).expect("parse");

        cfg.normalize_protected_mcp_routes()
            .expect("loadout route normalization succeeds");
        cfg.validate().expect("known loadout route is valid");
        let ProtectedMcpRouteTarget::GatewaySubset(target) = cfg.protected_mcp_routes[0]
            .target
            .as_ref()
            .expect("gateway subset target");
        assert_eq!(target.loadout.as_deref(), Some("sd"));
    }

    #[test]
    fn config_validation_rejects_unknown_gateway_subset_targets() {
        let toml = r#"
[[upstream]]
name = "gateway-alpha"
url = "https://gateway_alpha.example.com/mcp"

[[protected_mcp_routes]]
name = "ops"
public_host = "mcp.example.com"
public_path = "/ops"

[protected_mcp_routes.target]
kind = "gateway_subset"
upstreams = ["sonnar"]
services = ["gateway", "nope"]
"#;

        let mut cfg: LabConfig = toml::from_str(toml).expect("parse");
        cfg.normalize_protected_mcp_routes()
            .expect("normalization should not hide validation failure");
        let err = cfg
            .validate()
            .expect_err("unknown gateway_subset targets must fail validation");

        assert!(
            err.to_string().contains("sonnar") || err.to_string().contains("nope"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn config_validation_allows_stale_targets_on_disabled_gateway_subset_routes() {
        let toml = r#"
[[protected_mcp_routes]]
name = "retired-ops"
enabled = false
public_host = "mcp.example.com"
public_path = "/ops"

[protected_mcp_routes.target]
kind = "gateway_subset"
upstreams = ["removed-upstream"]
services = ["removed-service"]
"#;

        let mut cfg: LabConfig = toml::from_str(toml).expect("parse");
        cfg.normalize_protected_mcp_routes()
            .expect("disabled route remains structurally valid");
        cfg.validate()
            .expect("disabled routes must not block gateway startup");
    }

    // ── Code Mode: CodeModeConfig defaults ───────────────────────────────────

    #[test]
    fn code_mode_config_token_estimate_divisor_defaults_to_4() {
        let config = CodeModeConfig::default();
        // PRESENCE: default divisor is exactly 4
        assert_eq!(
            config.token_estimate_divisor, 4,
            "token_estimate_divisor default must be 4"
        );
        // ABSENCE: it is not 0 or 1 (which would drastically change truncation)
        assert_ne!(config.token_estimate_divisor, 0);
        assert_ne!(config.token_estimate_divisor, 1);
    }

    #[test]
    fn code_mode_config_defaults_are_sane() {
        let config = CodeModeConfig::default();
        // PRESENCE: timeout and output limits are positive
        assert!(config.timeout_ms > 0);
        assert!(config.max_response_bytes > 0);
        assert!(config.max_response_tokens > 0);
        // ABSENCE: not wildly large (sanity bounds)
        assert!(config.timeout_ms <= 60_000);
    }

    // ── Process-wide atomic flags ─────────────────────────────────────────────

    #[test]
    fn process_code_mode_flag_round_trips() {
        let _guard = process_code_mode_test_guard();

        set_process_code_mode_enabled_for_test(true);
        assert!(
            process_code_mode_enabled(),
            "code_mode must be true after set_process_code_mode_enabled(true)"
        );

        set_process_code_mode_enabled_for_test(false);
        assert!(
            !process_code_mode_enabled(),
            "code_mode must be false after set_process_code_mode_enabled(false)"
        );
    }

    // ── T3: secret file permission tests (S2) ────────────────────────────────

    #[cfg(unix)]
    fn file_mode(path: &Path) -> u32 {
        use std::os::unix::fs::PermissionsExt;
        std::fs::metadata(path)
            .unwrap_or_else(|e| panic!("metadata {}: {e}", path.display()))
            .permissions()
            .mode()
            & 0o777
    }

    #[test]
    #[cfg(unix)]
    fn write_service_creds_creates_file_with_mode_0o600() {
        let dir = tempfile::tempdir().expect("tempdir");
        let env_path = dir.path().join(".env");

        let creds = [EnvCredential {
            service: "myservice".to_string(),
            url: None,
            secret: Some("supersecret".to_string()),
            env_field: "MYSERVICE_TOKEN".to_string(),
        }];

        write_service_creds(&env_path, &creds, false).expect("write_service_creds");

        assert_eq!(
            file_mode(&env_path),
            0o600,
            ".env must be 0o600 after write_service_creds"
        );
    }

    // Backup-file 0o600 perms and retention pruning are covered directly by
    // env_merge's own unix_perms_set_to_0600 / backup_pruning_keeps_last_ten
    // tests -- write_service_creds delegates entirely to env_merge::merge for
    // that behavior and adds no file-handling logic of its own.

    #[test]
    #[cfg(unix)]
    fn heal_env_file_permissions_tightens_loose_env() {
        use std::os::unix::fs::PermissionsExt;

        let dir = tempfile::tempdir().expect("tempdir");
        let env_path = dir.path().join(".env");
        std::fs::write(&env_path, "TOKEN=secret\n").expect("write");
        std::fs::set_permissions(&env_path, std::fs::Permissions::from_mode(0o644))
            .expect("chmod 644");

        heal_env_file_permissions(&env_path);

        assert_eq!(
            file_mode(&env_path),
            0o600,
            "heal must tighten .env to 0o600"
        );
    }

    #[test]
    #[cfg(unix)]
    fn heal_env_file_permissions_tightens_backup_files() {
        use std::os::unix::fs::PermissionsExt;

        let dir = tempfile::tempdir().expect("tempdir");
        let env_path = dir.path().join(".env");
        let bak_path = dir.path().join(".env.bak.1234567890");

        std::fs::write(&env_path, "TOKEN=secret\n").expect("write env");
        std::fs::write(&bak_path, "TOKEN=oldsecret\n").expect("write bak");

        for p in [&env_path, &bak_path] {
            std::fs::set_permissions(p, std::fs::Permissions::from_mode(0o644)).expect("chmod 644");
        }

        heal_env_file_permissions(&env_path);

        assert_eq!(file_mode(&env_path), 0o600, ".env must be healed");
        assert_eq!(file_mode(&bak_path), 0o600, ".env.bak.* must be healed");
    }

    // usage_telemetry_enabled() delegates to the pure resolve_usage_telemetry_enabled()
    // so these tests never need to mutate process env (this crate forbids
    // `unsafe`, and `std::env::set_var`/`remove_var` are `unsafe fn` as of
    // Rust 2024) — same shape as the resolve_web_ui_auth_disabled_values
    // tests above.
    #[test]
    fn usage_telemetry_enabled_defaults_true_when_unset() {
        assert!(
            resolve_usage_telemetry_enabled(None),
            "usage telemetry must default to enabled when the env var is unset"
        );
    }

    #[test]
    fn usage_telemetry_enabled_false_when_set_to_1() {
        assert!(
            !resolve_usage_telemetry_enabled(Some("1")),
            "usage telemetry must be disabled when the env var is \"1\""
        );
    }

    #[test]
    fn usage_telemetry_enabled_true_for_other_values() {
        assert!(
            resolve_usage_telemetry_enabled(Some("true")),
            "only the exact value \"1\" should disable usage telemetry"
        );
        assert!(
            resolve_usage_telemetry_enabled(Some("0")),
            "\"0\" is not the disable sentinel; telemetry stays enabled"
        );
    }

    #[test]
    fn usage_db_path_is_under_dot_labby_home_dir() {
        let path = usage_db_path().unwrap();
        assert_eq!(path.file_name().and_then(|n| n.to_str()), Some("usage.db"));
        assert_eq!(
            path.parent()
                .and_then(|p| p.file_name())
                .and_then(|n| n.to_str()),
            Some(".labby")
        );
    }

    /// Review finding on lab-eyeuv: the reserved-prefix guard was added to
    /// `labby_runtime`'s copy of `normalize_protected_mcp_routes`, but
    /// `load_toml` runs THIS copy — and the route scopes mounted by
    /// `cli/serve.rs` are built from this config. The guard has to live on
    /// both, and this test pins the serve-path copy specifically.
    #[test]
    fn serve_path_normalization_rejects_reserved_in_process_upstreams() {
        let mut route: ProtectedMcpRouteConfig = toml::from_str(
            "name=\"scoped\"\npublic_host=\"mcp.example.com\"\npublic_path=\"/svc\"\n",
        )
        .unwrap();
        route.backend_url = String::new();
        route.target = Some(ProtectedMcpRouteTarget::GatewaySubset(
            ProtectedGatewaySubsetTarget {
                project_id: None,
                upstreams: vec![format!("{IN_PROCESS_UPSTREAM_PREFIX}setup")],
                services: Vec::new(),
                expose_code_mode: false,
                loadout: None,
            },
        ));
        let mut cfg = LabConfig {
            protected_mcp_routes: vec![route],
            ..LabConfig::default()
        };

        let error = cfg
            .normalize_protected_mcp_routes()
            .expect_err("the serve-path copy must reject the reserved prefix");
        let rendered = error.to_string();
        assert!(rendered.contains("__in_process__setup"), "{rendered}");
    }

    #[test]
    fn serve_path_project_id_normalization_is_bounded_and_compatible() {
        fn config(project_id: Option<String>) -> LabConfig {
            let mut route: ProtectedMcpRouteConfig = toml::from_str(
                "name=\"scoped\"\npublic_host=\"mcp.example.com\"\npublic_path=\"/svc\"\n",
            )
            .unwrap();
            route.backend_url = String::new();
            route.target = Some(ProtectedMcpRouteTarget::GatewaySubset(
                ProtectedGatewaySubsetTarget {
                    project_id,
                    ..Default::default()
                },
            ));
            LabConfig {
                protected_mcp_routes: vec![route],
                ..Default::default()
            }
        }

        let mut omitted = config(None);
        omitted
            .normalize_protected_mcp_routes()
            .expect("legacy None");
        let Some(ProtectedMcpRouteTarget::GatewaySubset(target)) =
            omitted.protected_mcp_routes[0].target.as_ref()
        else {
            unreachable!()
        };
        assert_eq!(target.project_id, None);

        let max = labby_runtime::gateway_config::MAX_PROJECT_ID_LEN;
        let expected = "x".repeat(max);
        let mut trimmed = config(Some(format!("  {expected}  ")));
        trimmed.normalize_protected_mcp_routes().expect("128 bytes");
        let Some(ProtectedMcpRouteTarget::GatewaySubset(target)) =
            trimmed.protected_mcp_routes[0].target.as_ref()
        else {
            unreachable!()
        };
        assert_eq!(target.project_id.as_deref(), Some(expected.as_str()));

        for invalid in ["   ".to_string(), "x".repeat(max + 1)] {
            assert!(
                config(Some(invalid))
                    .normalize_protected_mcp_routes()
                    .is_err()
            );
        }
    }
}
