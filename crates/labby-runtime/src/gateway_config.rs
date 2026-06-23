//! Gateway configuration DTOs shared across the Lab gateway-extraction crates.
//!
//! These types are the surface-neutral data model for the MCP gateway:
//! upstream definitions, Code Mode limits, protected MCP routes, outbound
//! OAuth, and virtual-server persistence. They are deserialized from
//! `config.toml` and serialized back out, so their serde shape (defaults,
//! renames, skip rules) is a stability contract — changing it silently
//! corrupts operator config.
//!
//! This module is intentionally free of file/env IO. Loading lives in the
//! `lab` binary's `config` module, which re-exports everything here so existing
//! call sites keep compiling unchanged.

use std::collections::BTreeMap;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

// ─── serde default helpers ───────────────────────────────────────────────────

/// Serde default for boolean fields that default to `true`.
pub fn default_true() -> bool {
    true
}

fn default_code_mode_trace_params() -> bool {
    true
}

fn default_code_mode_timeout_ms() -> u64 {
    30_000
}

fn default_code_mode_max_response_bytes() -> usize {
    24 * 1024
}

fn default_code_mode_max_response_tokens() -> usize {
    6_000
}

fn default_token_estimate_divisor() -> u32 {
    4
}

fn default_max_log_entries() -> usize {
    1000
}

fn default_max_log_bytes() -> usize {
    65536
}

fn default_upstream_priority() -> f32 {
    1.0
}

/// Default MCP path used by protected routes (`/mcp`).
pub fn default_mcp_path() -> String {
    "/mcp".to_string()
}

fn is_default_mcp_path(path: &str) -> bool {
    path == "/mcp"
}

fn default_mcp_scopes() -> Vec<String> {
    vec!["mcp:read".to_string(), "mcp:write".to_string()]
}

// ─── Code Mode ───────────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CodeModeConfig {
    /// Whether the MCP gateway advertises `codemode`.
    #[serde(default)]
    pub enabled: bool,
    /// Whether Code Mode call traces include redacted/capped tool params.
    #[serde(default = "default_code_mode_trace_params")]
    pub trace_params: bool,
    /// Maximum wall-clock time for one Code Mode execution.
    #[serde(default = "default_code_mode_timeout_ms")]
    pub timeout_ms: u64,
    /// Maximum serialized response envelope size returned by codemode.
    #[serde(default = "default_code_mode_max_response_bytes")]
    pub max_response_bytes: usize,
    /// Approximate maximum response tokens returned by codemode.
    #[serde(default = "default_code_mode_max_response_tokens")]
    pub max_response_tokens: usize,
    /// Token estimation divisor. bytes/4 is intentionally conservative (real
    /// tokenization ≈ 1 token/3 bytes for JSON). Lower = more conservative =
    /// fewer tools per execution.
    #[serde(default = "default_token_estimate_divisor")]
    pub token_estimate_divisor: u32,
    /// Maximum number of console log lines captured per execution.
    /// Excess lines are dropped and a sentinel appended.
    #[serde(default = "default_max_log_entries")]
    pub max_log_entries: usize,
    /// Maximum total bytes of console log output captured per execution.
    /// Excess bytes are dropped and a sentinel appended.
    #[serde(default = "default_max_log_bytes")]
    pub max_log_bytes: usize,
}

impl Default for CodeModeConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            trace_params: default_code_mode_trace_params(),
            timeout_ms: default_code_mode_timeout_ms(),
            max_response_bytes: default_code_mode_max_response_bytes(),
            max_response_tokens: default_code_mode_max_response_tokens(),
            token_estimate_divisor: default_token_estimate_divisor(),
            max_log_entries: default_max_log_entries(),
            max_log_bytes: default_max_log_bytes(),
        }
    }
}

impl CodeModeConfig {
    pub fn validate(&self) -> Result<(), ConfigError> {
        if !(1..=60_000).contains(&self.timeout_ms) {
            return Err(ConfigError::InvalidCodeModeTimeout {
                value: self.timeout_ms,
            });
        }
        if !(1024..=1024 * 1024).contains(&self.max_response_bytes) {
            return Err(ConfigError::InvalidCodeModeMaxResponseBytes {
                value: self.max_response_bytes,
            });
        }
        if !(256..=256_000).contains(&self.max_response_tokens) {
            return Err(ConfigError::InvalidCodeModeMaxResponseTokens {
                value: self.max_response_tokens,
            });
        }
        if !(1..=64).contains(&self.token_estimate_divisor) {
            return Err(ConfigError::InvalidCodeModeTokenEstimateDivisor {
                value: self.token_estimate_divisor,
            });
        }
        if !(1..=100_000).contains(&self.max_log_entries) {
            return Err(ConfigError::InvalidCodeModeMaxLogEntries {
                value: self.max_log_entries,
            });
        }
        if !(1..=100 * 1024 * 1024).contains(&self.max_log_bytes) {
            return Err(ConfigError::InvalidCodeModeMaxLogBytes {
                value: self.max_log_bytes,
            });
        }
        Ok(())
    }
}

// ─── Import provenance ───────────────────────────────────────────────────────

/// Provenance record for an upstream imported from an external MCP config.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ImportSource {
    /// Which client config type this was discovered in (e.g. "cursor", "claude-code", "vscode").
    pub client: String,
    /// Absolute path to the config file the server was read from.
    pub path: String,
    /// Normalized server name as it appeared when discovered. This lets delete
    /// tombstones survive an operator renaming the imported gateway in Lab.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub server_name: Option<String>,
    /// Stable hash of the discovered transport target. Used to avoid suppressing
    /// a different server that later reuses the same client/path/name slot.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub transport_fingerprint: Option<String>,
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
            server_name: None,
            transport_fingerprint: None,
            imported_at: imported_at.into(),
        }
    }

    #[must_use]
    pub fn with_server_name(mut self, server_name: impl Into<String>) -> Self {
        self.server_name = Some(server_name.into());
        self
    }

    #[must_use]
    pub fn with_transport_fingerprint(mut self, fingerprint: impl Into<String>) -> Self {
        self.transport_fingerprint = Some(fingerprint.into());
        self
    }
}

/// Suppresses automatic re-import of an operator-deleted imported upstream.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UpstreamImportTombstone {
    /// Name of the removed upstream.
    pub name: String,
    /// Original import provenance for the removed upstream.
    pub imported_from: ImportSource,
    /// ISO 8601 timestamp of when the deletion was recorded.
    pub removed_at: String,
}

impl UpstreamImportTombstone {
    pub fn now(name: impl Into<String>, imported_from: ImportSource) -> Self {
        Self {
            name: name.into(),
            imported_from,
            removed_at: jiff::Timestamp::now().to_string(),
        }
    }
}

/// Controls how external MCP config discovery behaves on gateway startup.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GatewayImportMode {
    /// Discovery disabled. No external configs are scanned or imported (default).
    #[default]
    Off,
    /// Scan on startup; queue discovered servers under `upstream_pending` for
    /// operator approval via `gateway.import_pending.approve`. Never auto-applies.
    Pending,
    /// Auto-import everything not tombstoned (legacy behavior).
    Auto,
}

// ─── Upstreams ───────────────────────────────────────────────────────────────

/// Configuration for a single upstream MCP server.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UpstreamConfig {
    /// Human-readable name for this upstream (used as tool-name prefix).
    pub name: String,
    /// Whether this upstream is enabled for discovery and proxying. Defaults to true.
    #[serde(default = "default_true")]
    pub enabled: bool,
    /// Search result priority multiplier for tools from this upstream (default 1.0).
    /// Values above 1.0 boost this upstream's tools; below 1.0 suppress them.
    /// Applied before the score-floor cut, so it affects which tools survive.
    #[serde(default = "default_upstream_priority")]
    pub priority: f32,
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

// ─── Protected MCP routes ────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ProtectedMcpRouteTarget {
    GatewaySubset(ProtectedGatewaySubsetTarget),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct ProtectedGatewaySubsetTarget {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub upstreams: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub services: Vec<String>,
    #[serde(default)]
    pub expose_code_mode: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProtectedMcpRouteEffectiveTarget {
    BackendUrl { url: String },
    Upstream { name: String },
    GatewaySubset(ProtectedGatewaySubsetTarget),
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
    /// Explicit route target. Omitted for legacy proxy routes that use
    /// `backend_url` or `upstream`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target: Option<ProtectedMcpRouteTarget>,
}

impl ProtectedMcpRouteConfig {
    #[must_use]
    pub fn public_resource(&self) -> String {
        format!("https://{}{}", self.public_host, self.public_path)
    }

    #[must_use]
    pub fn effective_target(&self) -> ProtectedMcpRouteEffectiveTarget {
        if let Some(ProtectedMcpRouteTarget::GatewaySubset(target)) = &self.target {
            return ProtectedMcpRouteEffectiveTarget::GatewaySubset(target.clone());
        }
        if let Some(name) = self.upstream.as_ref() {
            return ProtectedMcpRouteEffectiveTarget::Upstream { name: name.clone() };
        }
        ProtectedMcpRouteEffectiveTarget::BackendUrl {
            url: self.backend_url.clone(),
        }
    }

    #[must_use]
    pub fn is_gateway_subset(&self) -> bool {
        matches!(self.target, Some(ProtectedMcpRouteTarget::GatewaySubset(_)))
    }

    #[must_use]
    pub fn gateway_subset_target(&self) -> Option<&ProtectedGatewaySubsetTarget> {
        match &self.target {
            Some(ProtectedMcpRouteTarget::GatewaySubset(target)) => Some(target),
            None => None,
        }
    }
}

/// Normalize a protected route backend URL, folding a legacy path into the URL
/// when the URL itself carries no path.
pub fn normalize_protected_backend_url(
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

// ─── Config-layer errors ─────────────────────────────────────────────────────

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
    #[error("gateway code_mode.timeout_ms={value} is invalid — expected 1..=60000")]
    InvalidCodeModeTimeout { value: u64 },
    #[error("gateway code_mode.max_response_bytes={value} is invalid — expected 1024..=1048576")]
    InvalidCodeModeMaxResponseBytes { value: usize },
    #[error("gateway code_mode.max_response_tokens={value} is invalid — expected 256..=256000")]
    InvalidCodeModeMaxResponseTokens { value: usize },
    #[error("gateway code_mode.token_estimate_divisor={value} is invalid — expected 1..=64")]
    InvalidCodeModeTokenEstimateDivisor { value: u32 },
    #[error("gateway code_mode.max_log_entries={value} is invalid — expected 1..=100000")]
    InvalidCodeModeMaxLogEntries { value: usize },
    #[error("gateway code_mode.max_log_bytes={value} is invalid — expected 1..=104857600")]
    InvalidCodeModeMaxLogBytes { value: usize },
    #[error("gateway upstream_request_timeout_ms={value} is invalid — expected 1..=300000")]
    InvalidUpstreamRequestTimeout { value: u64 },
    #[error("gateway upstream_relay_timeout_ms={value} is invalid — expected 1..=1800000")]
    InvalidUpstreamRelayTimeout { value: u64 },
    #[error("protected MCP route '{name}' has invalid {field}: {value}")]
    InvalidProtectedRoute {
        name: String,
        field: &'static str,
        value: String,
    },
}

// ─── Outbound OAuth ──────────────────────────────────────────────────────────

/// Outbound OAuth configuration for an upstream MCP server.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UpstreamOauthConfig {
    pub mode: UpstreamOauthMode,
    pub registration: UpstreamOauthRegistration,
    #[serde(default)]
    pub scopes: Option<Vec<String>>,
    /// When `true`, always use the Client ID Metadata Document (CIMD) strategy
    /// regardless of whether the upstream advertises a `registration_endpoint`.
    /// When `false`, always use dynamic registration (RFC 7591) when the upstream
    /// advertises a `registration_endpoint`.
    /// When absent (`None`), the legacy default applies: upstreams named `"swag"`
    /// default to CIMD; all others default to dynamic registration when available.
    ///
    /// Set this field explicitly to remove the deployment-specific `"swag"` name
    /// check. New upstreams should set this field rather than relying on the legacy
    /// name-based default.
    #[serde(default)]
    pub prefer_client_metadata_document: Option<bool>,
}

/// Outbound OAuth mode. Currently only `authorization_code_pkce` is supported.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UpstreamOauthMode {
    AuthorizationCodePkce,
}

/// Outbound OAuth client-registration strategy.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
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

// ─── Virtual servers ─────────────────────────────────────────────────────────

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

// ─── Web preferences ─────────────────────────────────────────────────────────

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

// ─── Gateway spawn-guard preferences ─────────────────────────────────────────

/// Controls the stdio spawn-guard that validates upstream MCP server commands.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct GatewayPreferences {
    /// Extra commands allowed as stdio upstream programs beyond the built-in list
    /// (npx, uvx, docker, node, python, python3, deno, pipx, dnx).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub extra_stdio_commands: Vec<String>,
    /// Disable all stdio spawn-guard command validation.
    /// Any command may be used as a stdio upstream when true.
    /// Only set this when you control all gateway write access.
    #[serde(default)]
    pub disable_spawn_guard: bool,
}

// ─── Resolved public URLs ────────────────────────────────────────────────────

/// Canonical public URL pair after env-over-config merge.
///
/// Produced by the host's config layer (which owns env precedence and the
/// legacy `[auth].public_url` fallback) and handed to the gateway runtime.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResolvedPublicUrls {
    /// Public app URL. May be `None` when the operator has not configured one.
    pub app: Option<String>,
    /// Public MCP gateway URL. Falls back to `app` when not separately configured.
    pub mcp_gateway: Option<String>,
}

impl ResolvedPublicUrls {
    /// Return the effective MCP gateway URL, preferring a separately configured
    /// gateway URL over the app URL.
    #[must_use]
    pub fn effective_mcp_gateway(&self) -> Option<&str> {
        self.mcp_gateway.as_deref().or(self.app.as_deref())
    }
}

// ─── Gateway config DTO ──────────────────────────────────────────────────────

/// Default request timeout for one proxied upstream MCP response (30s).
pub const DEFAULT_UPSTREAM_REQUEST_TIMEOUT_MS: u64 = 30_000;
/// Default deadline for a single *relayed* upstream tool call (5 minutes).
pub const DEFAULT_UPSTREAM_RELAY_TIMEOUT_MS: u64 = 300_000;

/// Surface-neutral gateway configuration the [`GatewayManager`] reads and
/// mutates.
///
/// This is the gateway-relevant slice of the host's full `LabConfig`. It is the
/// **in-memory** model only: persistence (TOML render with foreign-key
/// preservation, atomic write, env-credential side effects) is owned by the
/// host through the `GatewayConfigStore` seam in `lab-gateway`. There is
/// intentionally **no** `#[serde(flatten)]` bag here — preservation of unrelated
/// `config.toml` keys stays the host's job, because the host keeps `LabConfig`.
///
/// [`GatewayManager`]: ../../labby_gateway/gateway/struct.GatewayManager.html
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct GatewayConfig {
    /// Gateway-wide Code Mode exposure and execution settings.
    #[serde(default)]
    pub code_mode: CodeModeConfig,
    /// Maximum time to wait for one proxied upstream MCP response.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub upstream_request_timeout_ms: Option<u64>,
    /// Maximum time to wait for one *relayed* upstream tool call.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub upstream_relay_timeout_ms: Option<u64>,
    /// Upstream MCP servers to proxy through the gateway.
    #[serde(default)]
    pub upstream: Vec<UpstreamConfig>,
    /// Imported upstreams removed by an operator.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub upstream_import_tombstones: Vec<UpstreamImportTombstone>,
    /// Discovered upstreams waiting for operator approval.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub upstream_pending: Vec<UpstreamConfig>,
    /// Public HTTP MCP routes protected by Lab OAuth and proxied by Lab.
    #[serde(default)]
    pub protected_mcp_routes: Vec<ProtectedMcpRouteConfig>,
    /// Virtual MCP servers backed by canonically configured Lab services.
    #[serde(default)]
    pub virtual_servers: Vec<VirtualServerConfig>,
    /// Virtual servers whose backing service is no longer registered.
    #[serde(default)]
    pub quarantined_virtual_servers: Vec<VirtualServerConfig>,
    /// Gateway spawn-guard and command-allowlist preferences.
    #[serde(default)]
    pub gateway: GatewayPreferences,
}

impl GatewayConfig {
    /// Resolved request timeout for one proxied upstream MCP response.
    #[must_use]
    pub fn upstream_request_timeout(&self) -> std::time::Duration {
        std::time::Duration::from_millis(
            self.upstream_request_timeout_ms
                .unwrap_or(DEFAULT_UPSTREAM_REQUEST_TIMEOUT_MS),
        )
    }

    /// Resolved deadline for a single *relayed* upstream tool call.
    #[must_use]
    pub fn upstream_relay_timeout(&self) -> std::time::Duration {
        std::time::Duration::from_millis(
            self.upstream_relay_timeout_ms
                .unwrap_or(DEFAULT_UPSTREAM_RELAY_TIMEOUT_MS),
        )
    }

    /// Normalize protected MCP route targets, trim whitespace, and validate.
    ///
    /// Ported verbatim from the host's `LabConfig::normalize_protected_mcp_routes`
    /// for the gateway-owned slice so the standalone (FS-store) load path matches
    /// the host's load path byte-for-byte.
    pub fn normalize_protected_mcp_routes(&mut self) -> Result<(), ConfigError> {
        for route in &mut self.protected_mcp_routes {
            route.upstream = route
                .upstream
                .take()
                .map(|name| name.trim().to_string())
                .filter(|name| !name.is_empty());
            if let Some(ProtectedMcpRouteTarget::GatewaySubset(target)) = &mut route.target {
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
        validate_gateway_subset_paths_are_unique(&self.protected_mcp_routes)?;
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

fn validate_gateway_subset_paths_are_unique(
    routes: &[ProtectedMcpRouteConfig],
) -> Result<(), ConfigError> {
    let mut paths = std::collections::HashSet::new();
    for route in routes
        .iter()
        .filter(|route| route.enabled && route.is_gateway_subset())
    {
        if !paths.insert(route.public_path.clone()) {
            return Err(ConfigError::InvalidProtectedRoute {
                name: route.name.clone(),
                field: "public_path",
                value: format!(
                    "gateway_subset routes must use unique public_path values; `{}` is already mounted",
                    route.public_path
                ),
            });
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn omitted_proxy_flags_default_to_true() {
        let cfg: UpstreamConfig = toml::from_str("name=\"axon\"\nurl=\"https://x/mcp\"\n").unwrap();
        assert!(cfg.proxy_resources);
        assert!(cfg.proxy_prompts);
        // Other serde defaults along the same path.
        assert!(cfg.enabled);
        assert!((cfg.priority - default_upstream_priority()).abs() < f32::EPSILON);
        assert!(cfg.oauth.is_none());
    }

    #[test]
    fn code_mode_config_defaults_roundtrip() {
        let cfg: CodeModeConfig = toml::from_str("").unwrap();
        let expected = CodeModeConfig::default();
        assert_eq!(cfg, expected);
        assert!(!cfg.enabled);
        assert!(cfg.trace_params);
        assert_eq!(cfg.timeout_ms, 30_000);
        assert_eq!(cfg.token_estimate_divisor, 4);
    }

    #[test]
    fn protected_route_backend_mcp_path_defaults_to_mcp() {
        let route: ProtectedMcpRouteConfig = toml::from_str(
            "name=\"r\"\npublic_host=\"mcp.example.com\"\npublic_path=\"/svc\"\nbackend_url=\"http://10.0.0.1:3100/mcp\"\n",
        )
        .unwrap();
        assert_eq!(route.backend_mcp_path, "/mcp");
        assert!(route.enabled);
        assert_eq!(route.scopes, vec!["mcp:read", "mcp:write"]);
    }
}
