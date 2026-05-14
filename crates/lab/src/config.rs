//! Config loading for the `lab` binary.
//!
//! Order of precedence (highest wins):
//!   1. CLI flags / process environment variables
//!   2. `~/.lab/.env` (loaded via `dotenvy`)
//!   3. `config.toml` (searched: `./` → `~/.lab/` → `~/.config/lab/`)
//!   4. Built-in defaults
//!
//! Service credentials and instance endpoints belong in `.env`. Non-secret
//! operator preferences and defaults (logging, CORS, MCP transport, admin
//! flags, registry URLs, workspace roots) belong in `config.toml`.
//!
//! Multi-instance services follow the `S_<LABEL>_URL` pattern: a service
//! like `unraid` reads `UNRAID_URL` as the default instance and
//! `UNRAID_NODE2_URL` as an additional instance labeled `node2`.

pub mod env_merge;

use std::sync::atomic::{AtomicBool, Ordering};
#[cfg(test)]
use std::sync::{Mutex, OnceLock};
use std::{
    collections::BTreeMap,
    collections::HashMap,
    fs::OpenOptions,
    io::Write as _,
    path::{Path, PathBuf},
};

// Gateway startup/reload writes this process-wide flag whenever root
// `[tool_search]` changes. In-process peer MCP servers do not hold a
// GatewayManager, but they must still hide raw built-in tools when the root
// server is operating in synthetic `tool_search`/`tool_execute` mode.
static PROCESS_TOOL_SEARCH_ENABLED: AtomicBool = AtomicBool::new(false);

pub(crate) fn set_process_tool_search_enabled(enabled: bool) {
    let previous = PROCESS_TOOL_SEARCH_ENABLED.swap(enabled, Ordering::AcqRel);
    if previous != enabled {
        tracing::info!(
            surface = "mcp",
            service = "tool_search",
            action = "tool_search.process_enablement",
            previous_enabled = previous,
            enabled,
            "process-wide tool search enablement changed"
        );
    }
}

pub(crate) fn process_tool_search_enabled() -> bool {
    PROCESS_TOOL_SEARCH_ENABLED.load(Ordering::Acquire)
}

use anyhow::{Context, Result};
use lab_apis::extract::types::ServiceCreds;
use lab_auth::config as auth_config;
use serde::{Deserialize, Serialize, Serializer};
use tempfile::NamedTempFile;

pub const DEFAULT_MCPREGISTRY_URL: &str = "https://registry.modelcontextprotocol.io";
pub const WEB_UI_AUTH_DISABLED_ENV: &str = "LAB_WEB_UI_AUTH_DISABLED";
pub const WEB_UI_AUTH_DISABLED_LEGACY_ENV: &str = "LAB_WEB_UI_DISABLE_AUTH";

#[cfg(test)]
static TEST_CONFIG_TOML_PATH: OnceLock<Mutex<Option<PathBuf>>> = OnceLock::new();

#[cfg(test)]
pub(crate) fn set_test_config_toml_path(path: Option<PathBuf>) {
    let slot = TEST_CONFIG_TOML_PATH.get_or_init(|| Mutex::new(None));
    *slot.lock().expect("test config path lock") = path;
}

/// Fully-resolved `lab` configuration, assembled from env + TOML.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct LabConfig {
    /// Default output format for CLI commands that print tables.
    #[serde(default)]
    pub output: OutputPreferences,
    /// MCP server defaults.
    #[serde(default)]
    pub mcp: McpPreferences,
    /// Logging preferences (overridden by `LAB_LOG` / `LAB_LOG_FORMAT` env vars).
    #[serde(default)]
    pub log: LogPreferences,
    /// Local-master log subsystem preferences.
    #[serde(default)]
    pub local_logs: Option<LocalLogsPreferences>,
    /// HTTP API preferences.
    #[serde(default)]
    pub api: ApiPreferences,
    /// Web UI preferences.
    #[serde(default)]
    pub web: WebPreferences,
    /// Shared Lab workspace root. Backs the read-only attachment picker and
    /// local writable stash workspaces.
    #[serde(default)]
    pub workspace: WorkspacePreferences,
    /// MCP Registry upstream preferences.
    #[serde(default)]
    pub mcpregistry: McpRegistryPreferences,
    /// OAuth callback relay preferences.
    #[serde(default)]
    pub oauth: OauthPreferences,
    /// Device runtime preferences.
    #[serde(default)]
    pub device: Option<DevicePreferences>,
    /// Node runtime preferences.
    #[serde(default)]
    pub node: Option<NodePreferences>,
    /// Admin tool settings.
    #[serde(default)]
    pub admin: AdminPreferences,
    /// Per-service preference overrides.
    #[serde(default)]
    pub services: ServicePreferences,
    /// HTTP auth mode preferences.
    #[serde(default)]
    pub auth: Option<AuthFileConfig>,
    /// Gateway-wide tool-search mode for all exposed upstream tools.
    #[serde(default)]
    pub tool_search: ToolSearchConfig,
    /// Upstream MCP servers to proxy through the gateway.
    #[serde(default)]
    pub upstream: Vec<UpstreamConfig>,
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
    /// Deploy service preferences (feature-gated at the consumer level).
    #[serde(default)]
    pub deploy: Option<DeployPreferences>,
    /// Canonical public URL model for the app and MCP gateway.
    ///
    /// Use [`LabConfig::public_urls()`] to read resolved values with env-var
    /// precedence rather than accessing this field directly.
    #[serde(default)]
    pub public_urls: Option<PublicUrlsConfig>,
}

impl LabConfig {
    /// Resolve the canonical public URL pair after env-over-config merge.
    ///
    /// Precedence (highest wins):
    ///   1. `LAB_PUBLIC_URL` env var (app), `LAB_MCP_GATEWAY_URL` env var (gateway)
    ///   2. `config.toml` `[public_urls]` section
    ///   3. Legacy `[auth].public_url` field (app only, for backward compat)
    pub fn public_urls(&self) -> ResolvedPublicUrls {
        // Env wins
        let env_app = std::env::var("LAB_PUBLIC_URL")
            .ok()
            .filter(|v| !v.is_empty());
        let env_gw = std::env::var("LAB_MCP_GATEWAY_URL")
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
}

/// Deploy service preferences — defaults plus per-host overrides.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct DeployPreferences {
    #[serde(default)]
    pub defaults: Option<DeployDefaults>,
    #[serde(default)]
    pub hosts: BTreeMap<String, DeployHostOverride>,
}

/// Artifact role for deploy targets.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ArtifactRole {
    Controller,
    Node,
}

/// Default policy applied to every deploy target unless overridden.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct DeployDefaults {
    pub remote_path: Option<String>,
    pub service: Option<String>,
    #[serde(default)]
    pub restart: Option<RestartModel>,
    pub service_scope: Option<ServiceScope>,
    pub max_parallel: Option<u32>,
    #[serde(default)]
    pub canary_hosts: Vec<String>,
    /// Base URL of the master lab instance that deployed hosts should phone home to.
    /// e.g. "http://dookie:8765". If absent, phone-home is skipped.
    pub master_url: Option<String>,
    /// Artifact role for this deploy target.
    #[serde(default)]
    pub artifact_role: Option<ArtifactRole>,
    /// Cross-compilation target triple, e.g. "aarch64-unknown-linux-gnu".
    #[serde(default)]
    pub target_triple: Option<String>,
    /// Maximum build time in seconds before declaring the build failed.
    #[serde(default)]
    pub build_timeout_secs: Option<u64>,
}

/// Per-host policy overrides for deploy.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct DeployHostOverride {
    pub remote_path: Option<String>,
    pub service: Option<String>,
    #[serde(default)]
    pub restart: Option<RestartModel>,
    pub service_scope: Option<ServiceScope>,
    /// Artifact role override for this specific host.
    #[serde(default)]
    pub artifact_role: Option<ArtifactRole>,
    /// Cross-compilation target triple override for this specific host.
    #[serde(default)]
    pub target_triple: Option<String>,
    /// Build timeout override in seconds for this specific host.
    #[serde(default)]
    pub build_timeout_secs: Option<u64>,
}

/// Restart policy used by rollout/update flows after a binary install.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum RestartModel {
    SystemService { service: String },
    UserService { service: String },
    WrapperCommand { command: Vec<String> },
}

/// Systemd scope for the unit restarted by deploy.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ServiceScope {
    System,
    User,
}

/// Device runtime preferences.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct DevicePreferences {
    #[serde(default)]
    pub master: Option<String>,
}

/// Explicit runtime role for this node, set in config or via CLI `--role`.
///
/// This is the user-facing vocabulary; the internal runtime maps
/// `Controller → NodeRole::Master` and `Node → NodeRole::NonMaster`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NodeRuntimeRole {
    Controller,
    Node,
}

/// Node runtime preferences.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct NodePreferences {
    #[serde(default)]
    pub controller: Option<String>,
    /// How many days of node logs to retain in the SQLite log store.
    /// Defaults to 30 days when absent.
    #[serde(default)]
    pub log_retention_days: Option<u32>,
    /// Explicit runtime role for this device.
    /// When present, skips hostname-based role inference.
    #[serde(default)]
    pub role: Option<NodeRuntimeRole>,
}

/// Runtime role for the current device.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum DeviceRole {
    Master,
    NonMaster,
}

/// Alias for [`DeviceRole`] used after the `device → node` module rename.
pub type NodeRole = DeviceRole;

/// Alias for [`ResolvedDeviceRuntime`] used after the `device → node` module rename.
pub type ResolvedNodeRuntime = ResolvedDeviceRuntime;

/// Resolved device runtime configuration after comparing local and master hosts.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResolvedDeviceRuntime {
    pub local_host: String,
    pub master_host: String,
    pub role: DeviceRole,
}

impl LabConfig {
    pub fn normalize_legacy_tool_search(&mut self, root_tool_search_present: bool) {
        if root_tool_search_present || self.tool_search.enabled {
            return;
        }

        let enabled: Vec<_> = self
            .upstream
            .iter()
            .filter(|u| u.tool_search.enabled)
            .collect();

        let Some(first) = enabled.first() else {
            return;
        };

        self.tool_search = first.tool_search.clone();

        let conflicting: Vec<&str> = enabled[1..]
            .iter()
            .filter(|u| u.tool_search != first.tool_search)
            .map(|u| u.name.as_str())
            .collect();

        if !conflicting.is_empty() {
            tracing::warn!(
                promoted = first.name.as_str(),
                discarded = ?conflicting,
                "normalize_legacy_tool_search: multiple upstreams had different \
                 tool_search configs; promoting first, discarding others — \
                 add a root [tool_search] section to config.toml to pin the value"
            );
        }
    }

    pub fn validate(&self) -> Result<(), ConfigError> {
        self.tool_search.validate()?;
        for upstream in &self.upstream {
            upstream.validate()?;
        }
        Ok(())
    }

    pub fn normalize_protected_mcp_routes(&mut self) -> Result<(), ConfigError> {
        for route in &mut self.protected_mcp_routes {
            route.upstream = route
                .upstream
                .take()
                .map(|name| name.trim().to_string())
                .filter(|name| !name.is_empty());
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

    #[must_use]
    pub fn controller_host(&self) -> Option<&str> {
        self.node
            .as_ref()
            .and_then(|prefs| prefs.controller.as_deref())
            .or_else(|| {
                self.device
                    .as_ref()
                    .and_then(|prefs| prefs.master.as_deref())
            })
    }
}

pub(crate) fn root_tool_search_present(raw: &str) -> bool {
    toml::from_str::<toml::Value>(raw)
        .ok()
        .and_then(|value| {
            value
                .as_table()
                .map(|table| table.contains_key("tool_search"))
        })
        .unwrap_or(false)
}

fn default_true() -> bool {
    true
}

fn default_tool_search_top_k() -> usize {
    10
}

fn default_tool_search_max_tools() -> usize {
    5000
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ToolSearchConfig {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default = "default_tool_search_top_k")]
    pub top_k_default: usize,
    #[serde(default = "default_tool_search_max_tools")]
    pub max_tools: usize,
}

impl Default for ToolSearchConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            top_k_default: default_tool_search_top_k(),
            max_tools: default_tool_search_max_tools(),
        }
    }
}

impl ToolSearchConfig {
    pub fn validate(&self) -> Result<(), ConfigError> {
        if !(1..=50).contains(&self.top_k_default) {
            return Err(ConfigError::InvalidToolSearchTopKDefault {
                value: self.top_k_default,
            });
        }
        if !(1..=10_000).contains(&self.max_tools) {
            return Err(ConfigError::InvalidToolSearchMaxTools {
                value: self.max_tools,
            });
        }
        Ok(())
    }
}

/// Provenance record for an upstream imported from an external MCP config.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ImportSource {
    /// Which client config type this was discovered in (e.g. "cursor", "claude-code", "vscode").
    pub client: String,
    /// Absolute path to the config file the server was read from.
    pub path: String,
    /// ISO 8601 timestamp of when the import was recorded.
    pub imported_at: String,
}

impl ImportSource {
    pub fn new(
        client: impl Into<String>,
        path: impl Into<String>,
        imported_at: impl Into<String>,
    ) -> Self {
        Self {
            client: client.into(),
            path: path.into(),
            imported_at: imported_at.into(),
        }
    }

    pub fn now(client: impl Into<String>, path: impl Into<String>) -> Self {
        Self::new(client, path, jiff::Timestamp::now().to_string())
    }
}

/// Configuration for a single upstream MCP server.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UpstreamConfig {
    /// Human-readable name for this upstream (used as tool-name prefix).
    pub name: String,
    /// Whether this upstream is enabled for discovery and proxying. Defaults to true.
    #[serde(default = "default_true")]
    pub enabled: bool,
    /// URL of the upstream MCP server (must be `http://`, `https://`, `ws://`, or `wss://`).
    /// For stdio upstreams, omit `url` and use `command`/`args` fields instead.
    #[serde(default)]
    pub url: Option<String>,
    /// Name of an env var holding the bearer token (not the token itself).
    #[serde(default)]
    pub bearer_token_env: Option<String>,
    /// Command to run for stdio transport upstreams.
    #[serde(default)]
    pub command: Option<String>,
    /// Arguments to pass to the stdio command.
    #[serde(default)]
    pub args: Vec<String>,
    /// Environment variables to inject when spawning a stdio transport process.
    /// Import discovery records env key counts, but does not copy raw values from
    /// external config files into Lab config.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub env: BTreeMap<String, String>,
    /// Whether to proxy resources from this upstream. Defaults to true.
    #[serde(default = "default_true")]
    pub proxy_resources: bool,
    /// Whether to proxy prompts from this upstream. Defaults to true.
    #[serde(default = "default_true")]
    pub proxy_prompts: bool,
    /// Optional allowlist of tool names/patterns to expose from this upstream.
    #[serde(default)]
    pub expose_tools: Option<Vec<String>>,
    /// Optional allowlist of resource URIs/patterns to expose from this upstream.
    #[serde(default)]
    pub expose_resources: Option<Vec<String>>,
    /// Optional allowlist of prompt names/patterns to expose from this upstream.
    #[serde(default)]
    pub expose_prompts: Option<Vec<String>>,
    /// Optional outbound OAuth configuration. Mutually exclusive with
    /// `bearer_token_env` — setting both is a config error.
    #[serde(default)]
    pub oauth: Option<UpstreamOauthConfig>,
    /// Import provenance — present when this upstream was discovered from an
    /// external MCP config rather than added manually. Omitted for manual entries.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub imported_from: Option<ImportSource>,
    /// Deprecated compatibility field. Tool search is gateway-wide via root
    /// `[tool_search]`; this field is only read to migrate older configs.
    #[serde(default, skip_serializing)]
    pub tool_search: ToolSearchConfig,
}

/// Gateway-managed public MCP route protected by Lab OAuth.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProtectedMcpRouteConfig {
    /// Stable operator-facing identifier.
    pub name: String,
    /// Whether this route is active for metadata, auth, and proxy resolution.
    #[serde(default = "default_true")]
    pub enabled: bool,
    /// Public host that reaches Lab through the edge proxy, e.g. `mcp.tootie.tv`.
    pub public_host: String,
    /// Public path prefix on that host, e.g. `/syslog`.
    pub public_path: String,
    /// Optional named Gateway upstream to publish at this protected route.
    /// When set, Lab uses the upstream registry and its configured upstream
    /// auth instead of proxying directly to `backend_url`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub upstream: Option<String>,
    /// Full backend MCP endpoint URL, e.g. `http://100.88.16.79:3100/mcp`.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub backend_url: String,
    /// Deprecated compatibility field. New configs put the MCP path in
    /// `backend_url`; this field is folded into `backend_url` when loading
    /// older origin-only route entries.
    #[serde(
        default = "default_mcp_path",
        skip_serializing_if = "is_default_mcp_path"
    )]
    pub backend_mcp_path: String,
    /// OAuth scopes advertised and enforced for this route.
    #[serde(default = "default_mcp_scopes")]
    pub scopes: Vec<String>,
    /// Optional backend health path used by route test actions.
    #[serde(default)]
    pub health_path: Option<String>,
}

impl ProtectedMcpRouteConfig {
    #[must_use]
    pub fn public_resource(&self) -> String {
        format!("https://{}{}", self.public_host, self.public_path)
    }
}

fn default_mcp_path() -> String {
    "/mcp".to_string()
}

fn is_default_mcp_path(path: &str) -> bool {
    path == "/mcp"
}

fn default_mcp_scopes() -> Vec<String> {
    vec!["mcp:read".to_string(), "mcp:write".to_string()]
}

fn normalize_protected_backend_url(
    raw: &str,
    legacy_path: &str,
) -> Result<String, url::ParseError> {
    let mut parsed = url::Url::parse(raw.trim())?;
    if parsed.query().is_some() || parsed.fragment().is_some() {
        return Err(url::ParseError::RelativeUrlWithoutBase);
    }

    let current_path = parsed.path();
    if current_path.is_empty() || current_path == "/" {
        parsed.set_path(&normalize_mcp_route_path(legacy_path));
    }
    Ok(parsed.to_string().trim_end_matches('/').to_string())
}

fn normalize_mcp_route_path(raw: &str) -> String {
    let trimmed = raw.trim();
    let with_slash = if trimmed.starts_with('/') {
        trimmed.to_string()
    } else {
        format!("/{trimmed}")
    };
    let normalized = with_slash
        .split('/')
        .filter(|segment| !segment.is_empty())
        .collect::<Vec<_>>()
        .join("/");
    if normalized.is_empty() {
        "/mcp".to_string()
    } else {
        format!("/{normalized}")
    }
}

impl UpstreamConfig {
    /// Validate the upstream name and mutually-exclusive auth shapes.
    /// `bearer_token_env` and `oauth` both configured is a config error.
    pub fn validate(&self) -> Result<(), ConfigError> {
        // Name must not be empty.
        if self.name.trim().is_empty() {
            return Err(ConfigError::InvalidName {
                name: self.name.clone(),
                reason: "must not be empty".to_string(),
            });
        }
        // Name must not exceed 128 characters.
        if self.name.len() > 128 {
            return Err(ConfigError::InvalidName {
                name: self.name.clone(),
                reason: "must not exceed 128 characters".to_string(),
            });
        }
        // Name must use only safe ASCII characters.
        if !self
            .name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
        {
            return Err(ConfigError::InvalidName {
                name: self.name.clone(),
                reason: "must contain only ASCII letters, digits, hyphens, underscores, and dots"
                    .to_string(),
            });
        }
        if self.bearer_token_env.is_some() && self.oauth.is_some() {
            return Err(ConfigError::ConflictingAuth {
                name: self.name.clone(),
            });
        }
        if self.oauth.is_some() && self.url.is_none() {
            return Err(ConfigError::MissingOauthUrl {
                name: self.name.clone(),
            });
        }
        if let Some(raw) = self.url.as_deref() {
            let canonical =
                canonicalize_upstream_url(raw).map_err(|_| ConfigError::InvalidUrl {
                    name: self.name.clone(),
                    url: raw.to_string(),
                })?;
            // Only HTTP(S) and WebSocket upstream URLs are allowed.
            // Other schemes (file://, ftp://, etc.) are rejected at validation time
            // rather than discovered at connection time.
            let scheme = canonical.split("://").next().unwrap_or("");
            if scheme != "http" && scheme != "https" && scheme != "ws" && scheme != "wss" {
                return Err(ConfigError::InvalidUrl {
                    name: self.name.clone(),
                    url: raw.to_string(),
                });
            }
        }
        Ok(())
    }

    /// Return the RFC 3986 §6.2.2-canonical form of `url` used as the OAuth
    /// `resource` indicator. The canonical string is the single source of truth
    /// for the `resource` parameter sent to authorize, token, and (where rmcp
    /// supports it) refresh endpoints. Returns `None` when no URL is set.
    pub fn canonical_url(&self) -> Option<Result<String, ConfigError>> {
        self.url.as_deref().map(|raw| {
            canonicalize_upstream_url(raw).map_err(|_| ConfigError::InvalidUrl {
                name: self.name.clone(),
                url: raw.to_string(),
            })
        })
    }
}

/// Canonicalize an upstream URL per RFC 3986 §6.2.2 (scheme/host lowercase,
/// default port stripped, dot-segment removal, percent-encoding case
/// normalization). Trailing slashes are preserved — they are semantically
/// significant in HTTP paths.
pub fn canonicalize_upstream_url(raw: &str) -> Result<String, url::ParseError> {
    let parsed = url::Url::parse(raw.trim())?;
    Ok(parsed.to_string())
}

/// Config-layer errors surfaced by `UpstreamConfig::validate` and sibling helpers.
#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error("upstream '{name}' has invalid name: {reason}")]
    InvalidName { name: String, reason: String },
    #[error("upstream '{name}' has both bearer_token_env and oauth configured — pick one")]
    ConflictingAuth { name: String },
    #[error("upstream '{name}' has invalid url: {url}")]
    InvalidUrl { name: String, url: String },
    #[error("upstream '{name}' has oauth configured but no url — oauth requires an HTTP url")]
    MissingOauthUrl { name: String },
    #[error("gateway tool_search.top_k_default={value} is invalid — expected 1..=50")]
    InvalidToolSearchTopKDefault { value: usize },
    #[error("gateway tool_search.max_tools={value} is invalid — expected 1..=10000")]
    InvalidToolSearchMaxTools { value: usize },
    #[error("protected MCP route '{name}' has invalid {field}: {value}")]
    InvalidProtectedRoute {
        name: String,
        field: &'static str,
        value: String,
    },
}

/// Outbound OAuth configuration for an upstream MCP server.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UpstreamOauthConfig {
    pub mode: UpstreamOauthMode,
    pub registration: UpstreamOauthRegistration,
    #[serde(default)]
    pub scopes: Option<Vec<String>>,
}

/// Outbound OAuth mode. Currently only `authorization_code_pkce` is supported.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UpstreamOauthMode {
    AuthorizationCodePkce,
}

/// Outbound OAuth client-registration strategy.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "strategy", rename_all = "snake_case")]
pub enum UpstreamOauthRegistration {
    ClientMetadataDocument {
        url: String,
    },
    Preregistered {
        client_id: String,
        #[serde(default)]
        client_secret_env: Option<String>,
    },
    Dynamic,
}

/// Persisted state for a Lab-backed virtual server shown in the gateway.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct VirtualServerConfig {
    pub id: String,
    pub service: String,
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub surfaces: VirtualServerSurfacesConfig,
    #[serde(default)]
    pub mcp_policy: Option<VirtualServerMcpPolicyConfig>,
}

/// Per-surface exposure flags for a virtual server.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct VirtualServerSurfacesConfig {
    #[serde(default)]
    pub cli: bool,
    #[serde(default)]
    pub api: bool,
    #[serde(default)]
    pub mcp: bool,
    #[serde(default)]
    pub webui: bool,
}

/// Action-level policy for Lab-backed single-tool MCP services.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct VirtualServerMcpPolicyConfig {
    #[serde(default)]
    pub allowed_actions: Vec<String>,
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
    /// Default session keep-alive TTL in seconds for HTTP MCP sessions.
    #[serde(default)]
    pub session_ttl_secs: Option<u64>,
    /// Whether HTTP MCP should use stateful sessions by default.
    #[serde(default)]
    pub stateful: Option<bool>,
    /// Additional allowed hosts for DNS rebinding protection.
    #[serde(default)]
    pub allowed_hosts: Option<Vec<String>>,
}

/// Canonical public URL model.
///
/// `app` is the Lab UI and OAuth issuer, e.g. `https://lab.example.com`.
/// `mcp_gateway` is the MCP endpoint base URL when hosted on a separate hostname,
/// e.g. `https://mcp.example.com`.  When absent the gateway is assumed to be
/// reachable at the app URL.
///
/// Values are read from config.toml; env vars `LAB_PUBLIC_URL` (app) and
/// `LAB_MCP_GATEWAY_URL` (mcp_gateway) take precedence and may be set in
/// `~/.lab/.env`.
///
/// Accessor: [`LabConfig::public_urls()`] returns a resolved [`ResolvedPublicUrls`].
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct PublicUrlsConfig {
    /// Public app (UI + OAuth) base URL, e.g. `https://lab.example.com`.
    #[serde(default)]
    pub app: Option<String>,
    /// Separate MCP gateway base URL, e.g. `https://mcp.example.com`.
    /// Leave blank when the app and MCP gateway share the same hostname.
    #[serde(default)]
    pub mcp_gateway: Option<String>,
}

/// Resolved public URLs after env-over-config merge.
#[derive(Debug, Clone)]
pub struct ResolvedPublicUrls {
    /// Public app URL.  May be `None` when the operator has not configured one.
    pub app: Option<String>,
    /// Public MCP gateway URL.  Falls back to `app` when not separately configured.
    pub mcp_gateway: Option<String>,
}

impl ResolvedPublicUrls {
    /// Convenience: return the effective MCP gateway URL, preferring a
    /// separately configured gateway URL over the app URL.
    pub fn effective_mcp_gateway(&self) -> Option<&str> {
        self.mcp_gateway.as_deref().or(self.app.as_deref())
    }
}

/// File-backed auth preferences merged with environment variables at startup.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct AuthFileConfig {
    /// `bearer` preserves LAB_MCP_HTTP_TOKEN; `oauth` enables the internal auth server.
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
}

/// Resolve auth configuration from a full `LabConfig`.
///
/// This is the preferred entry point. Precedence for the public URL is:
/// 1. `[auth].public_url` (legacy field, preserved for backward compatibility)
/// 2. `[public_urls].app` (canonical new location)
/// 3. `LAB_PUBLIC_URL` env var (handled downstream by [`resolve_auth`])
///
/// When `[auth].public_url` is absent, `[public_urls].app` is promoted into the
/// auth config so downstream code resolves a consistent effective URL.
pub fn resolve_auth_for_config(cfg: &LabConfig) -> Result<auth_config::AuthConfig> {
    // Compute the effective public URL: [auth].public_url > [public_urls].app.
    // The env var LAB_PUBLIC_URL is handled downstream by resolve_auth().
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
/// so that `[public_urls].app` is used as a fallback for `LAB_PUBLIC_URL`.
pub fn resolve_auth(config: Option<&AuthFileConfig>) -> Result<auth_config::AuthConfig> {
    let mut merged: HashMap<String, String> = HashMap::new();

    if let Some(config) = config {
        insert_if_some(&mut merged, "LAB_AUTH_MODE", config.mode.clone());
        insert_if_some(&mut merged, "LAB_PUBLIC_URL", config.public_url.clone());
        insert_if_some(
            &mut merged,
            "LAB_AUTH_SQLITE_PATH",
            config
                .sqlite_path
                .as_ref()
                .map(|path| path.display().to_string()),
        );
        insert_if_some(
            &mut merged,
            "LAB_AUTH_KEY_PATH",
            config
                .key_path
                .as_ref()
                .map(|path| path.display().to_string()),
        );
        insert_if_some(
            &mut merged,
            "LAB_AUTH_BOOTSTRAP_SECRET",
            config.bootstrap_secret.clone(),
        );
        if let Some(patterns) = config.allowed_client_redirect_uris.as_ref() {
            insert_if_some(
                &mut merged,
                "LAB_AUTH_ALLOWED_REDIRECT_URIS",
                Some(patterns.join(",")),
            );
        }
        insert_if_some(
            &mut merged,
            "LAB_GOOGLE_CLIENT_ID",
            config.google_client_id.clone(),
        );
        insert_if_some(
            &mut merged,
            "LAB_GOOGLE_CLIENT_SECRET",
            config.google_client_secret.clone(),
        );
        insert_if_some(
            &mut merged,
            "LAB_GOOGLE_CALLBACK_PATH",
            config.google_callback_path.clone(),
        );
        if let Some(scopes) = config.google_scopes.as_ref() {
            insert_if_some(&mut merged, "LAB_GOOGLE_SCOPES", Some(scopes.join(",")));
        }
        insert_if_some(
            &mut merged,
            "LAB_AUTH_ACCESS_TOKEN_TTL_SECS",
            config.access_token_ttl_secs.map(|value| value.to_string()),
        );
        insert_if_some(
            &mut merged,
            "LAB_AUTH_REFRESH_TOKEN_TTL_SECS",
            config.refresh_token_ttl_secs.map(|value| value.to_string()),
        );
        insert_if_some(
            &mut merged,
            "LAB_AUTH_CODE_TTL_SECS",
            config.auth_code_ttl_secs.map(|value| value.to_string()),
        );
        insert_if_some(
            &mut merged,
            "LAB_AUTH_ADMIN_EMAIL",
            config.admin_email.clone(),
        );
    }

    for (key, value) in std::env::vars() {
        if key.starts_with("LAB_AUTH_") || key == "LAB_PUBLIC_URL" || key.starts_with("LAB_GOOGLE_")
        {
            merged.insert(key, value);
        }
    }

    auth_config::AuthConfig::from_sources(merged).map_err(anyhow::Error::from)
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
/// These map to `LAB_LOG` and `LAB_LOG_FORMAT` env vars but live in TOML so
/// operators don't need to clutter `.env` with non-secret preferences.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct LogPreferences {
    /// Tracing filter directive (e.g. `"labby=info,lab_apis=warn"`).
    /// Overridden by `LAB_LOG` env var.
    #[serde(default)]
    pub filter: Option<String>,
    /// Log format: `"text"` (default) or `"json"`.
    /// Overridden by `LAB_LOG_FORMAT` env var.
    #[serde(default)]
    pub format: Option<String>,
}

/// Local-master log store and retention preferences.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
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
pub struct ApiPreferences {
    /// Additional CORS origins (comma-separated string or TOML array).
    /// Loopback origins are always included.
    /// Overridden by `LAB_CORS_ORIGINS` env var.
    #[serde(default)]
    pub cors_origins: Vec<String>,
}

/// Web UI preferences.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct WebPreferences {
    /// Path to the exported Labby assets directory served by `labby serve`.
    #[serde(default)]
    pub assets_dir: Option<PathBuf>,
    /// Disable `/v1/*` auth for the hosted web UI. Intended only for trusted reverse-proxy setups.
    #[serde(default)]
    pub disable_auth: Option<bool>,
}

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
pub struct WorkspacePreferences {
    /// Root directory used by fs browsing and stash-backed writable workspaces.
    /// Defaults to `~/.lab/stash`.
    #[serde(default)]
    pub root: Option<PathBuf>,
}

/// MCP Registry upstream preferences.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpRegistryPreferences {
    /// Upstream MCP Registry base URL.
    #[serde(default = "default_mcpregistry_url_option")]
    pub url: Option<String>,
}

impl Default for McpRegistryPreferences {
    fn default() -> Self {
        Self {
            url: default_mcpregistry_url_option(),
        }
    }
}

fn default_mcpregistry_url_option() -> Option<String> {
    Some(DEFAULT_MCPREGISTRY_URL.to_string())
}

/// OAuth local relay preferences.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct OauthPreferences {
    /// Named callback relay targets.
    #[serde(default)]
    pub machines: BTreeMap<String, OauthMachineConfig>,
}

/// A named OAuth callback relay target.
#[derive(Debug, Clone, Serialize, Deserialize)]
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
pub struct AdminPreferences {
    /// Enable the `lab_admin` MCP tool. Default: `false`.
    /// Overridden by `LAB_ADMIN_ENABLED=1` env var.
    #[serde(default)]
    pub enabled: bool,
}

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

/// Tailscale non-secret preferences.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct TailscalePreferences {
    /// Tailnet name. Overridden by `TAILSCALE_TAILNET` env var.
    /// Default: `"-"` (auto-detect).
    #[serde(default)]
    pub tailnet: Option<String>,
}

/// Load `config.toml` only — no `.env`, no side effects beyond file reads.
///
/// Called early in `main()` before tracing is initialized so that `[log]`
/// preferences can feed into `init_tracing()`. Safe to call before any
/// other subsystem.
///
/// Config TOML resolution (first found wins):
///   1. `./config.toml` (repo/CWD override)
///   2. `~/.lab/config.toml` (user-level, colocated with `.env`)
///   3. `~/.config/lab/config.toml` (XDG-style fallback)
pub fn load_toml(candidates: &[PathBuf]) -> Result<LabConfig> {
    for path in candidates {
        match std::fs::read_to_string(path) {
            Ok(raw) => {
                let mut cfg = toml::from_str::<LabConfig>(&raw)
                    .with_context(|| format!("failed to parse {}", path.display()))?;
                cfg.normalize_legacy_tool_search(root_tool_search_present(&raw));
                cfg.normalize_protected_mcp_routes()
                    .with_context(|| format!("invalid config {}", path.display()))?;
                // Validate all upstream configs eagerly at startup so that
                // invalid configuration (conflicting auth, bad URL scheme, etc.)
                // is discovered immediately rather than at first OAuth attempt.
                cfg.validate()
                    .with_context(|| format!("invalid config {}", path.display()))?;
                return Ok(cfg);
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
            Err(e) => {
                return Err(
                    anyhow::Error::new(e).context(format!("failed to read {}", path.display()))
                );
            }
        }
    }
    Ok(LabConfig::default())
}

/// Patch the non-secret built-in upstream API preference without rewriting
/// unrelated TOML content.
///
/// This intentionally edits only `[services].built_in_upstream_apis_enabled`.
/// It preserves comments, unknown keys, and plugin-owned sections that the
/// full typed `LabConfig` serializer cannot round-trip.
pub fn patch_built_in_upstream_apis_enabled(path: &Path, enabled: bool) -> Result<LabConfig> {
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
        Err(e) => {
            return Err(anyhow::Error::new(e).context(format!("failed to read {}", path.display())));
        }
    };
    let mut document = raw
        .parse::<toml_edit::DocumentMut>()
        .with_context(|| format!("failed to parse {}", path.display()))?;

    document["services"]["built_in_upstream_apis_enabled"] = toml_edit::value(enabled);
    let patched = document.to_string();
    let mut cfg = toml::from_str::<LabConfig>(&patched)
        .with_context(|| format!("failed to parse patched {}", path.display()))?;
    cfg.normalize_legacy_tool_search(root_tool_search_present(&patched));
    cfg.validate()
        .with_context(|| format!("invalid patched config {}", path.display()))?;

    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    let mut tmp = NamedTempFile::new_in(parent)
        .with_context(|| format!("failed to create temp file in {}", parent.display()))?;
    tmp.write_all(patched.as_bytes())
        .context("failed to write temp config")?;
    tmp.as_file()
        .sync_all()
        .context("failed to sync temp config")?;
    tmp.persist(path)
        .map_err(|e| anyhow::Error::new(e.error))
        .with_context(|| format!("failed to persist {}", path.display()))?;

    Ok(cfg)
}

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
    // Load ~/.lab/.env first (user-level secrets).
    if let Some(env_path) = dotenv_path()
        && env_path.exists()
    {
        dotenvy::from_path(&env_path)
            .with_context(|| format!("failed to load {}", env_path.display()))?;
    }

    // Also load .env from the current working directory (dev convenience).
    // Does not override vars already set by the user-level file.
    let cwd_env = Path::new(".env");
    if cwd_env.exists()
        && let Err(e) = dotenvy::from_path(cwd_env)
    {
        tracing::debug!(path = ".env", error = %e, "failed to load local .env (skipping)");
    }

    Ok(())
}

/// Load `.env` + `config.toml` in a single call (convenience for tests).
#[allow(dead_code)]
pub fn load() -> Result<LabConfig> {
    let cfg = load_toml(&toml_candidates())?;
    load_dotenv()?;
    Ok(cfg)
}

/// Candidate paths for `config.toml`, ordered by priority (highest first).
pub fn toml_candidates() -> Vec<PathBuf> {
    let mut paths = vec![PathBuf::from("config.toml")];
    if let Some(home) = home_dir() {
        paths.push(home.join(".lab").join("config.toml"));
        paths.push(home.join(".config").join("lab").join("config.toml"));
    }
    paths
}

/// Cross-platform home directory.
///
/// Checks `HOME` (Unix) then `USERPROFILE` (Windows). No external crate needed.
pub(crate) fn home_dir() -> Option<PathBuf> {
    std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from)
}

#[must_use]
pub fn mcpregistry_url(config: &LabConfig) -> &str {
    config
        .mcpregistry
        .url
        .as_deref()
        .map(str::trim)
        .filter(|url| !url.is_empty())
        .unwrap_or(DEFAULT_MCPREGISTRY_URL)
}

#[must_use]
pub fn workspace_root_for_home(config: &LabConfig, home: &Path) -> PathBuf {
    config
        .workspace
        .root
        .as_deref()
        .map(|root| expand_home_path(root, home))
        .unwrap_or_else(|| home.join(".lab").join("stash"))
}

pub fn workspace_root_path(config: &LabConfig) -> Result<PathBuf> {
    let home = home_dir().ok_or_else(|| anyhow::anyhow!("HOME env var not set"))?;
    Ok(workspace_root_for_home(config, &home))
}

fn expand_home_path(path: &Path, home: &Path) -> PathBuf {
    let raw = path.as_os_str().to_string_lossy();
    if raw == "~" {
        return home.to_path_buf();
    }
    if let Some(rest) = raw.strip_prefix("~/") {
        return home.join(rest);
    }
    path.to_path_buf()
}

/// Standard location for the `.env` file: `~/.lab/.env`.
pub fn dotenv_path() -> Option<PathBuf> {
    home_dir().map(|home| home.join(".lab").join(".env"))
}

pub fn config_toml_path() -> Option<PathBuf> {
    #[cfg(test)]
    if let Some(path) = TEST_CONFIG_TOML_PATH
        .get_or_init(|| Mutex::new(None))
        .lock()
        .expect("test config path lock")
        .clone()
    {
        return Some(path);
    }

    toml_candidates()
        .into_iter()
        .find(|path| path.exists())
        .or_else(|| home_dir().map(|home| home.join(".config").join("lab").join("config.toml")))
}

/// Path to the SQLite registry database: `~/.lab/registry.db`.
///
/// Creates no files — callers are responsible for opening/creating the store.
pub fn registry_db_path() -> PathBuf {
    home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".lab")
        .join("registry.db")
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
/// Always constructed programmatically via [`scan_instances_from`]; never
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

// ─── .env writer (used by `labby extract --apply`) ─────────────────────────────

/// Merge `creds` into the `.env` file at `path` via the canonical
/// [`env_merge::merge`] primitive. Preferred over [`write_env`] /
/// [`write_env_pairs`] for new code: handles backup, atomic write,
/// mtime-skew detection, retention pruning, and 0600 perms in one call.
///
/// Returns the underlying merge outcome (skipped conflicts, backup path,
/// prune stats).
///
/// # Errors
/// Returns the typed [`env_merge::MergeError`] on any merge failure.
#[allow(dead_code)]
pub fn write_service_creds(
    path: &Path,
    creds: &[ServiceCreds],
    force: bool,
) -> Result<env_merge::MergeOutcome, env_merge::MergeError> {
    let mut entries: Vec<env_merge::EnvEntry> = Vec::new();
    for cred in creds {
        let svc_upper = cred.service.to_uppercase();
        if let Some(url) = &cred.url {
            entries.push(env_merge::EnvEntry::new(
                format!("{svc_upper}_URL"),
                url.clone(),
            ));
        }
        if let Some(secret) = &cred.secret {
            entries.push(env_merge::EnvEntry::new(
                cred.env_field.clone(),
                secret.clone(),
            ));
        }
    }
    env_merge::merge(
        path,
        env_merge::MergeRequest {
            entries,
            force,
            expected_mtime: None,
        },
    )
}

/// Copy `path` to `path.bak.<unix-seconds>`. No-op if `path` does not exist.
///
/// Returns the backup path (useful for messaging the user).
///
/// # Errors
/// Returns an error if the copy fails.
pub fn backup_env(path: &Path) -> Result<PathBuf> {
    if !path.exists() {
        // Nothing to back up; return a synthetic path for display only.
        let ts = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_secs());
        return Ok(path.with_extension(format!("bak.{ts}")));
    }
    let ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    let backup = PathBuf::from(format!("{}.bak.{ts}", path.display()));
    std::fs::copy(path, &backup)
        .with_context(|| format!("backup {} → {}", path.display(), backup.display()))?;
    Ok(backup)
}

/// Merge `new_creds` into the `.env` file at `path` following the 8-rule algorithm.
///
/// Rule summary (full spec in `crates/lab-apis/src/extract/CLAUDE.md`):
/// 1. Backup is the caller's responsibility — call [`backup_env`] before this.
/// 2. Atomic write: `path.tmp` → rename.
/// 3. Existing key order and comments are preserved.
/// 4. Comments (`#`) and blank lines pass through unchanged.
/// 5. Dedupe: one entry per key.
/// 6. Conflicts (key exists, different value): skip-and-warn unless `force=true`.
/// 7. Values containing whitespace or shell metacharacters are double-quoted.
/// 8. Idempotence: caller must check before invoking (this fn always writes).
///
/// Returns a `Vec<String>` of warnings for skipped conflicts.
///
/// # Errors
/// Returns an error if the tmp file cannot be written or renamed.
pub fn write_env(path: &Path, new_creds: &[ServiceCreds], force: bool) -> Result<Vec<String>> {
    // Read the existing file (empty if absent).
    let existing_raw = if path.exists() {
        std::fs::read_to_string(path).with_context(|| format!("read {}", path.display()))?
    } else {
        String::new()
    };
    let existing_lines: Vec<&str> = existing_raw.lines().collect();

    // Build map of existing key → value from non-comment lines.
    let mut existing: HashMap<String, String> = HashMap::new();
    for line in &existing_lines {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        if let Some((k, v)) = trimmed.split_once('=') {
            existing.insert(k.trim().to_owned(), v.trim().to_owned());
        }
    }

    // Collect all (key, value) pairs to write from new_creds.
    let mut to_write: Vec<(String, String)> = Vec::new();
    for cred in new_creds {
        let svc_upper = cred.service.to_uppercase();
        if let Some(url) = &cred.url {
            to_write.push((format!("{svc_upper}_URL"), url.clone()));
        }
        if let Some(secret) = &cred.secret {
            to_write.push((cred.env_field.clone(), secret.clone()));
        }
    }

    // Process each pair: classify as NEW, SAME, or CONFLICT.
    let mut conflicts: Vec<String> = Vec::new();
    // Track keys that are overrides (force=true conflicts).
    let mut override_keys: HashMap<String, String> = HashMap::new();
    // Track keys that are genuinely new.
    let mut new_keys: Vec<(String, String)> = Vec::new();

    for (key, value) in &to_write {
        match existing.get(key) {
            None => new_keys.push((key.clone(), value.clone())),
            Some(existing_val) if existing_val == value => {
                // Idempotent — already present with same value, skip.
            }
            Some(existing_val) => {
                if force {
                    override_keys.insert(key.clone(), value.clone());
                } else {
                    conflicts.push(format!(
                        "CONFLICT: {key} already set to {existing_val:?}; skipping (use --force to overwrite)"
                    ));
                }
            }
        }
    }

    // Build the new file: start with existing lines, applying overrides in-place.
    let mut out_lines: Vec<String> = Vec::new();
    for line in &existing_lines {
        let trimmed = line.trim();
        if !trimmed.is_empty()
            && !trimmed.starts_with('#')
            && let Some((k, _)) = trimmed.split_once('=')
        {
            let key = k.trim();
            if let Some(new_val) = override_keys.get(key) {
                out_lines.push(format!("{}={}", key, quote_env_value(new_val)));
                continue;
            }
        }
        out_lines.push((*line).to_owned());
    }

    // Append new keys at the end.
    if !new_keys.is_empty() {
        if !out_lines.last().is_none_or(|l| l.trim().is_empty()) {
            out_lines.push(String::new()); // blank separator
        }
        for (key, value) in &new_keys {
            out_lines.push(format!("{}={}", key, quote_env_value(value)));
        }
    }

    // Atomic write: write to .tmp, sync, rename.
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("create dir {}", parent.display()))?;
    }

    let tmp_path = PathBuf::from(format!("{}.tmp", path.display()));
    {
        let mut file = std::fs::File::create(&tmp_path)
            .with_context(|| format!("create {}", tmp_path.display()))?;
        for line in &out_lines {
            writeln!(file, "{line}").with_context(|| format!("write {}", tmp_path.display()))?;
        }
        file.sync_all()
            .with_context(|| format!("sync {}", tmp_path.display()))?;
    }
    std::fs::rename(&tmp_path, path)
        .with_context(|| format!("rename {} → {}", tmp_path.display(), path.display()))?;

    Ok(conflicts)
}

/// Write raw `(key, value)` pairs into the `.env` file at `path`.
///
/// Identical merge semantics to [`write_env`]: atomic write, existing order preserved,
/// conflicts skipped unless `force=true`, idempotent on same values.
/// Returns a `Vec<String>` of conflict warnings.
///
/// Prefer this over [`write_env`] when the pairs are not derived from [`ServiceCreds`].
///
/// # Errors
/// Returns an error if the tmp file cannot be written or renamed.
pub fn write_env_pairs(
    path: &Path,
    pairs: &[(String, String)],
    force: bool,
) -> Result<Vec<String>> {
    let existing_raw = if path.exists() {
        std::fs::read_to_string(path).with_context(|| format!("read {}", path.display()))?
    } else {
        String::new()
    };
    let existing_lines: Vec<&str> = existing_raw.lines().collect();

    let mut existing: HashMap<String, String> = HashMap::new();
    for line in &existing_lines {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        if let Some((k, v)) = trimmed.split_once('=') {
            existing.insert(k.trim().to_owned(), v.trim().to_owned());
        }
    }

    let mut conflicts: Vec<String> = Vec::new();
    let mut override_keys: HashMap<String, String> = HashMap::new();
    let mut new_keys: Vec<(String, String)> = Vec::new();

    for (key, value) in pairs {
        match existing.get(key) {
            None => new_keys.push((key.clone(), value.clone())),
            Some(existing_val) if existing_val == value => {}
            Some(existing_val) => {
                if force {
                    override_keys.insert(key.clone(), value.clone());
                } else {
                    conflicts.push(format!(
                        "CONFLICT: {key} already set to {existing_val:?}; skipping (use --force to overwrite)"
                    ));
                }
            }
        }
    }

    let mut out_lines: Vec<String> = Vec::new();
    for line in &existing_lines {
        let trimmed = line.trim();
        if !trimmed.is_empty()
            && !trimmed.starts_with('#')
            && let Some((k, _)) = trimmed.split_once('=')
        {
            let key = k.trim();
            if let Some(new_val) = override_keys.get(key) {
                out_lines.push(format!("{}={}", key, quote_env_value(new_val)));
                continue;
            }
        }
        out_lines.push((*line).to_owned());
    }

    if !new_keys.is_empty() {
        if !out_lines.last().is_none_or(|l| l.trim().is_empty()) {
            out_lines.push(String::new());
        }
        for (key, value) in &new_keys {
            out_lines.push(format!("{}={}", key, quote_env_value(value)));
        }
    }

    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("create dir {}", parent.display()))?;
    }

    let tmp_path = PathBuf::from(format!("{}.tmp", path.display()));
    {
        let mut file = std::fs::File::create(&tmp_path)
            .with_context(|| format!("create {}", tmp_path.display()))?;
        for line in &out_lines {
            writeln!(file, "{line}").with_context(|| format!("write {}", tmp_path.display()))?;
        }
        file.sync_all()
            .with_context(|| format!("sync {}", tmp_path.display()))?;
    }
    std::fs::rename(&tmp_path, path)
        .with_context(|| format!("rename {} → {}", tmp_path.display(), path.display()))?;

    Ok(conflicts)
}

/// Returns true if all (key, value) pairs that would be written by `write_env`
/// are already present in `path` with matching values. Used to implement
/// idempotence: if this returns true, skip backup and write entirely.
pub fn env_is_up_to_date(path: &Path, new_creds: &[ServiceCreds]) -> bool {
    let Ok(raw) = std::fs::read_to_string(path) else {
        return false;
    };
    let existing: HashMap<String, String> = raw
        .lines()
        .filter(|l| !l.trim().is_empty() && !l.trim().starts_with('#'))
        .filter_map(|l| {
            l.split_once('=').map(|(k, v)| {
                let trimmed = v.trim();
                // Strip surrounding double quotes so that quoted values
                // written by write_env() compare equal to the raw secret.
                let unquoted = trimmed
                    .strip_prefix('"')
                    .and_then(|s| s.strip_suffix('"'))
                    .map_or_else(
                        || trimmed.to_owned(),
                        // Unescape sequences that write_env() would have escaped.
                        |inner| inner.replace(r#"\""#, "\"").replace(r"\\", r"\"),
                    );
                (k.trim().to_owned(), unquoted)
            })
        })
        .collect();

    for cred in new_creds {
        let svc_upper = cred.service.to_uppercase();
        if let Some(url) = &cred.url {
            let key = format!("{svc_upper}_URL");
            if existing.get(&key).map(String::as_str) != Some(url.as_str()) {
                return false;
            }
        }
        if let Some(secret) = &cred.secret
            && existing.get(&cred.env_field).map(String::as_str) != Some(secret.as_str())
        {
            return false;
        }
    }
    true
}

/// Quote a value that contains shell-significant characters.
fn quote_env_value(v: &str) -> String {
    let needs_quotes = v
        .chars()
        .any(|c| matches!(c, ' ' | '\t' | '#' | '$' | '\\' | '"' | '\'' | '`'));
    if needs_quotes {
        let escaped = v.replace('\\', r"\\").replace('"', r#"\""#);
        format!("\"{escaped}\"")
    } else {
        v.to_owned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
    fn resolve_auth_reads_ttls_from_config_toml_fields() {
        let cfg = AuthFileConfig {
            mode: Some("oauth".to_string()),
            public_url: Some("https://lab.example.com".to_string()),
            sqlite_path: None,
            key_path: None,
            bootstrap_secret: Some("bootstrap".to_string()),
            allowed_client_redirect_uris: Some(vec![
                "https://callback.tootie.tv/callback/*".to_string(),
            ]),
            google_client_id: Some("client-id".to_string()),
            google_client_secret: Some("client-secret".to_string()),
            google_callback_path: Some("/auth/google/callback".to_string()),
            google_scopes: Some(vec!["openid".to_string(), "email".to_string()]),
            access_token_ttl_secs: Some(120),
            refresh_token_ttl_secs: Some(3600),
            auth_code_ttl_secs: Some(45),
            admin_email: Some("admin@example.com".to_string()),
        };

        let resolved = resolve_auth(Some(&cfg)).expect("auth config should resolve");
        assert_eq!(resolved.access_token_ttl.as_secs(), 120);
        assert_eq!(resolved.refresh_token_ttl.as_secs(), 3600);
        assert_eq!(resolved.auth_code_ttl.as_secs(), 45);
        assert_eq!(
            resolved.allowed_client_redirect_uris,
            vec!["https://callback.tootie.tv/callback/*".to_string()]
        );
    }

    #[test]
    fn oauth_machine_config_deserializes() {
        let cfg = toml::from_str::<LabConfig>(
            r#"
[oauth.machines.dookie]
target_url = "http://100.88.16.79:38935/callback/dookie"
description = "Dookie Claude callback target"
default_port = 38935
"#,
        )
        .expect("oauth machine config should parse");

        assert_eq!(
            cfg.oauth.machines["dookie"].target_url,
            "http://100.88.16.79:38935/callback/dookie"
        );
        assert_eq!(
            cfg.oauth.machines["dookie"].description.as_deref(),
            Some("Dookie Claude callback target")
        );
        assert_eq!(cfg.oauth.machines["dookie"].default_port, Some(38935));
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
    fn mcpregistry_url_defaults_to_official_registry() {
        let cfg = toml::from_str::<LabConfig>("").expect("empty config should parse");

        assert_eq!(
            cfg.mcpregistry.url.as_deref(),
            Some(DEFAULT_MCPREGISTRY_URL)
        );
    }

    #[test]
    fn quarantined_virtual_servers_round_trip_through_toml() {
        let raw = r#"
[[quarantined_virtual_servers]]
id = "stale-registry"
service = "mcpregistry"
enabled = true

[quarantined_virtual_servers.surfaces]
mcp = true
"#;
        let cfg = toml::from_str::<LabConfig>(raw).expect("quarantine config should parse");
        assert_eq!(cfg.quarantined_virtual_servers.len(), 1);
        assert_eq!(cfg.quarantined_virtual_servers[0].id, "stale-registry");
        assert_eq!(cfg.quarantined_virtual_servers[0].service, "mcpregistry");
        assert!(cfg.quarantined_virtual_servers[0].surfaces.mcp);

        let serialized = toml::to_string(&cfg).expect("config should serialize");
        let reparsed =
            toml::from_str::<LabConfig>(&serialized).expect("serialized config should parse");
        assert_eq!(reparsed.quarantined_virtual_servers.len(), 1);
        assert_eq!(reparsed.quarantined_virtual_servers[0].id, "stale-registry");
    }

    #[test]
    fn workspace_root_defaults_to_lab_stash_under_home() {
        let cfg = toml::from_str::<LabConfig>("").expect("empty config should parse");
        let home = Path::new("/tmp/lab-home");

        assert_eq!(
            workspace_root_for_home(&cfg, home),
            home.join(".lab").join("stash")
        );
    }

    #[test]
    fn workspace_root_reads_config_toml_value() {
        let cfg = toml::from_str::<LabConfig>(
            r#"
[workspace]
root = "/srv/lab-stash"
"#,
        )
        .expect("workspace config should parse");

        assert_eq!(
            workspace_root_for_home(&cfg, Path::new("/tmp/ignored")),
            PathBuf::from("/srv/lab-stash")
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
                .contains("invalid LAB_WEB_UI_AUTH_DISABLED value")
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

    // ─── write_env / backup_env tests ───────────────────────────────────────

    fn radarr_cred() -> ServiceCreds {
        ServiceCreds {
            service: "radarr".to_owned(),
            url: Some("http://localhost:7878".to_owned()),
            secret: Some("abc123".to_owned()),
            env_field: "RADARR_API_KEY".to_owned(),
            source_host: None,
            probe_host: None,
            runtime: None,
            url_verified: false,
        }
    }

    #[test]
    fn write_env_adds_new_keys() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(".env");
        let warnings = write_env(&path, &[radarr_cred()], false).unwrap();
        assert!(warnings.is_empty());
        let content = std::fs::read_to_string(&path).unwrap();
        assert!(content.contains("RADARR_URL=http://localhost:7878"));
        assert!(content.contains("RADARR_API_KEY=abc123"));
    }

    #[test]
    fn write_env_preserves_comments_and_blanks() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(".env");
        std::fs::write(&path, "# my comment\nOTHER=val\n").unwrap();
        write_env(&path, &[radarr_cred()], false).unwrap();
        let content = std::fs::read_to_string(&path).unwrap();
        assert!(content.contains("# my comment"));
        assert!(content.contains("OTHER=val"));
    }

    #[test]
    fn write_env_conflict_skip_without_force() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(".env");
        std::fs::write(&path, "RADARR_API_KEY=oldvalue\n").unwrap();
        let warnings = write_env(&path, &[radarr_cred()], false).unwrap();
        assert!(!warnings.is_empty());
        let content = std::fs::read_to_string(&path).unwrap();
        assert!(content.contains("oldvalue"));
        assert!(!content.contains("abc123"));
    }

    #[test]
    fn write_env_conflict_overwrite_with_force() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(".env");
        std::fs::write(&path, "RADARR_API_KEY=oldvalue\n").unwrap();
        let warnings = write_env(&path, &[radarr_cred()], true).unwrap();
        assert!(warnings.is_empty());
        let content = std::fs::read_to_string(&path).unwrap();
        assert!(content.contains("abc123"));
        assert!(!content.contains("oldvalue"));
    }

    #[test]
    fn env_is_up_to_date_returns_true_when_matching() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(".env");
        std::fs::write(
            &path,
            "RADARR_URL=http://localhost:7878\nRADARR_API_KEY=abc123\n",
        )
        .unwrap();
        assert!(env_is_up_to_date(&path, &[radarr_cred()]));
    }

    #[test]
    fn env_is_up_to_date_returns_false_when_missing() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(".env");
        std::fs::write(&path, "RADARR_URL=http://localhost:7878\n").unwrap();
        assert!(!env_is_up_to_date(&path, &[radarr_cred()]));
    }

    #[test]
    fn write_env_quotes_value_with_special_chars() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(".env");
        let cred = ServiceCreds {
            service: "svc".to_owned(),
            url: None,
            secret: Some("has space".to_owned()),
            env_field: "SVC_KEY".to_owned(),
            source_host: None,
            probe_host: None,
            runtime: None,
            url_verified: false,
        };
        write_env(&path, &[cred], false).unwrap();
        let content = std::fs::read_to_string(&path).unwrap();
        assert!(content.contains("SVC_KEY=\"has space\""));
    }

    #[test]
    fn env_is_up_to_date_handles_quoted_values() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(".env");
        let cred = ServiceCreds {
            service: "svc".to_owned(),
            url: None,
            secret: Some("has space".to_owned()),
            env_field: "SVC_KEY".to_owned(),
            source_host: None,
            probe_host: None,
            runtime: None,
            url_verified: false,
        };
        // write_env quotes values with spaces
        write_env(&path, &[cred.clone()], false).unwrap();
        // env_is_up_to_date must strip quotes before comparing
        assert!(
            env_is_up_to_date(&path, &[cred]),
            "quoted value in .env should match raw secret"
        );
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
    fn tool_search_is_root_level_config() {
        let cfg = toml::from_str::<LabConfig>(
            r#"
[tool_search]
enabled = true
top_k_default = 20
max_tools = 8000

[[upstream]]
name = "acme"
url = "https://acme.example.com/mcp"
"#,
        )
        .expect("root tool_search parses");

        assert!(cfg.tool_search.enabled);
        assert_eq!(cfg.tool_search.top_k_default, 20);
        assert_eq!(cfg.tool_search.max_tools, 8000);
        cfg.validate().expect("root tool_search validates");
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
name = "syslog"
enabled = true
public_host = "mcp.example.com"
public_path = "/syslog"
upstream = " syslog "
"#,
        )
        .expect("protected route parses");

        cfg.normalize_protected_mcp_routes()
            .expect("upstream route normalizes");

        assert_eq!(
            cfg.protected_mcp_routes[0].upstream.as_deref(),
            Some("syslog")
        );
        assert_eq!(cfg.protected_mcp_routes[0].backend_url, "");
        assert_eq!(cfg.protected_mcp_routes[0].backend_mcp_path, "/mcp");
    }

    #[test]
    fn legacy_upstream_tool_search_migrates_to_root() {
        let mut cfg = toml::from_str::<LabConfig>(
            r#"
[[upstream]]
name = "acme"
url = "https://acme.example.com/mcp"

[upstream.tool_search]
enabled = true
top_k_default = 15
max_tools = 750
"#,
        )
        .expect("legacy upstream tool_search parses");

        cfg.normalize_legacy_tool_search(false);

        assert!(cfg.tool_search.enabled);
        assert_eq!(cfg.tool_search.top_k_default, 15);
        assert_eq!(cfg.tool_search.max_tools, 750);
    }

    #[test]
    fn explicit_root_tool_search_disable_blocks_legacy_migration() {
        let raw = r#"
[tool_search]
enabled = false

[[upstream]]
name = "acme"
url = "https://acme.example.com/mcp"

[upstream.tool_search]
enabled = true
top_k_default = 15
max_tools = 750
"#;
        let mut cfg = toml::from_str::<LabConfig>(raw)
            .expect("explicit root and legacy upstream tool_search parse");

        cfg.normalize_legacy_tool_search(root_tool_search_present(raw));

        assert!(!cfg.tool_search.enabled);
        assert_eq!(cfg.tool_search.top_k_default, 10);
        assert_eq!(cfg.tool_search.max_tools, 5000);
    }

    #[test]
    fn load_toml_preserves_explicit_root_tool_search_disable_with_legacy_upstream() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(
            &path,
            r#"
[tool_search]
enabled = false

[[upstream]]
name = "acme"
url = "https://acme.example.com/mcp"

[upstream.tool_search]
enabled = true
top_k_default = 15
max_tools = 750
"#,
        )
        .unwrap();

        let cfg = load_toml(&[path]).expect("config loads");

        assert!(!cfg.tool_search.enabled);
        assert_eq!(cfg.tool_search.top_k_default, 10);
        assert_eq!(cfg.tool_search.max_tools, 5000);
    }

    #[test]
    fn tool_search_validation_is_gateway_wide() {
        let cfg = toml::from_str::<LabConfig>(
            r"
[tool_search]
top_k_default = 0
",
        )
        .expect("config parses; validation is a separate step");

        let err = cfg.validate().expect_err("invalid top_k_default");
        assert!(matches!(
            err,
            ConfigError::InvalidToolSearchTopKDefault { value: 0 }
        ));
    }

    #[test]
    fn parses_deploy_defaults_and_host_overrides() {
        let raw = r#"
[deploy.defaults]
remote_path = "/usr/local/bin/labby"
service = "labby"
service_scope = "system"
max_parallel = 4
canary_hosts = ["mini1"]

[deploy.hosts.mini2]
remote_path = "/opt/lab/bin/labby"
service = "lab-worker"
service_scope = "user"
"#;
        let parsed: LabConfig = toml::from_str(raw).unwrap();
        let d = parsed.deploy.expect("deploy present");
        let defaults = d.defaults.expect("defaults present");
        assert_eq!(
            defaults.remote_path.as_deref(),
            Some("/usr/local/bin/labby")
        );
        assert_eq!(defaults.service.as_deref(), Some("labby"));
        assert_eq!(defaults.service_scope, Some(ServiceScope::System));
        assert_eq!(defaults.max_parallel, Some(4));
        assert_eq!(defaults.canary_hosts, vec!["mini1".to_string()]);
        let mini2 = d.hosts.get("mini2").expect("mini2 override");
        assert_eq!(mini2.remote_path.as_deref(), Some("/opt/lab/bin/labby"));
        assert_eq!(mini2.service_scope, Some(ServiceScope::User));
    }

    #[test]
    fn deploy_config_absent_is_none_not_error() {
        let raw = "[output]\n";
        let parsed: LabConfig = toml::from_str(raw).unwrap();
        assert!(parsed.deploy.is_none());
    }

    #[test]
    fn deploy_max_parallel_defaults_to_one_for_safety_at_read_time() {
        let raw = "[deploy.defaults]\nremote_path = \"/usr/local/bin/labby\"\n";
        let parsed: LabConfig = toml::from_str(raw).unwrap();
        let d = parsed.deploy.unwrap().defaults.unwrap();
        // unset remains None; safe default applied at orchestrator entry
        assert!(d.max_parallel.is_none());
    }
}
