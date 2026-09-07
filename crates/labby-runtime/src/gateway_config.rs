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

/// Namespace prefix owned by the synthetic in-process service peers that
/// expose builtin Labby services to the Code Mode catalog (issue #210 FU-1).
///
/// Reserved in config validation, not merely by convention: an operator-named
/// upstream that collides with `__in_process__<service>` would shadow the
/// builtin's catalog entry, and a protected route that lists one in its
/// `target.upstreams` would receive builtin tools the route is supposed to
/// exclude. Both are rejected at load (bead lab-eyeuv).
pub const IN_PROCESS_UPSTREAM_PREFIX: &str = "__in_process__";

/// Maximum characters retained from a Code Mode discovery hint.
pub const CODE_MODE_HINT_MAX_CHARS: usize = 240;
/// Maximum words retained from a Code Mode discovery hint.
pub const CODE_MODE_HINT_MAX_WORDS: usize = 24;
/// Version tag for the Code Mode hint sanitization contract.
pub const CODE_MODE_HINT_SANITIZER_VERSION: &str = "code_mode_hint_v1";

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

fn default_code_mode_max_source_bytes() -> usize {
    128 * 1024
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

fn default_semantic_search_blend_weight() -> f32 {
    0.5
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

// ─── Lab-owned MCP Apps ──────────────────────────────────────────────────────

/// Visibility switches for Labby-owned MCP App UI surfaces. The `mcp_app`
/// control tool stays available when its manager UI is disabled. Code Mode keeps
/// its existing `CodeModeConfig::mcp_ui_enabled` field for backward-compatible
/// config.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct McpAppsConfig {
    /// Attach MCP App metadata to the always-available `mcp_app` control tool and advertise its UI resources.
    /// The control tool itself remains available when this is false.
    #[serde(default)]
    pub manager: bool,
    /// Advertise the synthetic Add Server app tool and its UI resources.
    /// Labby-owned app surfaces are opt-in by default.
    #[serde(default)]
    pub add_server: bool,
    /// Attach the Server Logs app metadata and advertise its UI resources.
    /// Labby-owned app surfaces are opt-in by default.
    #[serde(default)]
    pub server_logs: bool,
    /// Advertise the synthetic Gateway Status app tool and its UI resources.
    /// Labby-owned app surfaces are opt-in by default.
    #[serde(default)]
    pub gateway_status: bool,
    /// Advertise the schema-backed Settings app tool and its UI resources.
    /// Labby-owned app surfaces are opt-in by default.
    #[serde(default)]
    pub settings: bool,
}

// ─── Code Mode ───────────────────────────────────────────────────────────────

/// Model-facing policy applied to oversized final Code Mode results.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum CodeModeResultShapePolicy {
    /// Return the normal result envelope without additional shaping.
    #[default]
    Off,
    /// Truncate oversized model-facing results to the configured limits.
    Truncate,
}

/// Optional embedding-based semantic search blend for `codemode.search()`.
///
/// Disabled by default (`tei_url = None`). When `tei_url` is unset or empty,
/// `codemode.search()` runs its existing pure-lexical algorithm unchanged;
/// this struct's fields are never read on that path.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SemanticSearchConfig {
    /// Base URL of the TEI (Text Embeddings Inference) server, e.g.
    /// `http://localhost:52000`. `None` or empty (the default) means
    /// semantic search stays off — this is the sole enable signal, there is
    /// no separate `enabled` flag.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tei_url: Option<String>,
    /// Weight applied to normalized semantic similarity when blending with
    /// normalized lexical score. See `preamble.rs` `codemode.search` blend
    /// comment for the exact formula.
    #[serde(default = "default_semantic_search_blend_weight")]
    pub blend_weight: f32,
}

impl Default for SemanticSearchConfig {
    fn default() -> Self {
        Self {
            tei_url: None,
            blend_weight: default_semantic_search_blend_weight(),
        }
    }
}

impl SemanticSearchConfig {
    /// True only when `tei_url` is set to a non-empty string. Every call
    /// site should gate on this rather than re-checking the field directly.
    #[must_use]
    pub fn is_configured(&self) -> bool {
        self.tei_url
            .as_deref()
            .is_some_and(|url| !url.trim().is_empty())
    }
}

// `Eq` intentionally omitted: `SemanticSearchConfig.blend_weight` is an `f32`.
/// Runtime limits, visibility, trust, and result-shaping settings for Code Mode.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CodeModeConfig {
    /// Whether the MCP gateway advertises `codemode`.
    #[serde(default)]
    pub enabled: bool,
    /// Retired compatibility field, retained so existing configs still parse.
    ///
    /// It has no production reader: both catalog admission and the execution
    /// gate gate on the upstream's own `readOnlyHint` annotation. Setting it
    /// grants nothing, so it must never be presented as a security control.
    /// `gateway.code_mode.set` warns and drops a non-empty value, and the field
    /// is never serialized back out, so no surface can present it as live.
    #[serde(default, skip_serializing)]
    pub trusted_read_only_tools: Vec<String>,
    /// Whether the explicit `codemode_ui` MCP App tool and resources are advertised.
    /// The text-only `codemode` executor remains available when this is false.
    /// Labby-owned MCP Apps are opt-in, so this defaults to false.
    #[serde(default)]
    pub mcp_ui_enabled: bool,
    /// Whether Code Mode call traces include redacted/capped tool params.
    #[serde(default = "default_code_mode_trace_params")]
    pub trace_params: bool,
    /// Optional model-facing final-result shaping policy.
    /// This never affects sandbox-visible callTool results.
    #[serde(default)]
    pub result_shape_policy: CodeModeResultShapePolicy,
    /// Maximum wall-clock time for one Code Mode execution.
    #[serde(default = "default_code_mode_timeout_ms")]
    pub timeout_ms: u64,
    /// Maximum accepted Code Mode source size in bytes.
    #[serde(default = "default_code_mode_max_source_bytes")]
    pub max_source_bytes: usize,
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
    /// Optional embedding-based semantic search blend for `codemode.search()`.
    #[serde(default)]
    pub semantic_search: SemanticSearchConfig,
    /// Legacy bypass: let a rendered mcp-ui widget's callback reach the
    /// upstream proxy by tool name even while the Code Mode synthetic
    /// surface hides raw tools from `list_tools`. Default: off.
    /// Overridden by `LABBY_CODE_MODE_WIDGET_CALLBACKS=1` env var.
    #[serde(default)]
    pub widget_callbacks: Option<bool>,
    /// Per-run artifact directory retention count. `0` disables count pruning.
    /// Overridden by `LABBY_CODE_MODE_ARTIFACT_RETENTION_RUNS` env var. Default: 200.
    #[serde(default)]
    pub artifact_retention_runs: Option<usize>,
    /// Per-artifact content cap in MiB. Overridden by
    /// `LABBY_CODE_MODE_ARTIFACT_MAX_MIB` env var. Default: 8.
    #[serde(default)]
    pub artifact_max_mib: Option<usize>,
    /// Total artifact store byte budget in MiB. `0` disables byte pruning.
    /// Overridden by `LABBY_CODE_MODE_ARTIFACT_MAX_STORE_MIB` env var. Default: 4096.
    #[serde(default)]
    pub artifact_max_store_mib: Option<u64>,
    /// Per-run `callTool` budget. Overridden by
    /// `LABBY_CODE_MODE_MAX_CALLS_PER_RUN` env var.
    #[serde(default)]
    pub max_calls_per_run: Option<u64>,
    /// Max `callTool` result size in MiB before truncation. Overridden by
    /// `LABBY_CODE_MODE_CALLTOOL_RESULT_MAX_MIB` env var.
    #[serde(default)]
    pub calltool_result_max_mib: Option<usize>,
}

impl Default for CodeModeConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            trusted_read_only_tools: Vec::new(),
            mcp_ui_enabled: false,
            trace_params: default_code_mode_trace_params(),
            result_shape_policy: CodeModeResultShapePolicy::Off,
            timeout_ms: default_code_mode_timeout_ms(),
            max_source_bytes: default_code_mode_max_source_bytes(),
            max_response_bytes: default_code_mode_max_response_bytes(),
            max_response_tokens: default_code_mode_max_response_tokens(),
            token_estimate_divisor: default_token_estimate_divisor(),
            max_log_entries: default_max_log_entries(),
            max_log_bytes: default_max_log_bytes(),
            semantic_search: SemanticSearchConfig::default(),
            widget_callbacks: None,
            artifact_retention_runs: None,
            artifact_max_mib: None,
            artifact_max_store_mib: None,
            max_calls_per_run: None,
            calltool_result_max_mib: None,
        }
    }
}

impl CodeModeConfig {
    /// Validate Code Mode limits and semantic-search settings.
    pub fn validate(&self) -> Result<(), ConfigError> {
        if !(1..=60_000).contains(&self.timeout_ms) {
            return Err(ConfigError::InvalidCodeModeTimeout {
                value: self.timeout_ms,
            });
        }
        if !(1024..=1024 * 1024).contains(&self.max_source_bytes) {
            return Err(ConfigError::InvalidCodeModeMaxSourceBytes {
                value: self.max_source_bytes,
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
        if !(0.0..=1.0).contains(&self.semantic_search.blend_weight) {
            return Err(ConfigError::InvalidSemanticSearchBlendWeight {
                value: self.semantic_search.blend_weight,
            });
        }
        if let Some(tei_url) = self.semantic_search.tei_url.as_deref() {
            let trimmed = tei_url.trim();
            if !trimmed.is_empty() {
                let parsed = url::Url::parse(trimmed).map_err(|_| {
                    ConfigError::InvalidSemanticSearchTeiUrl {
                        value: tei_url.to_string(),
                    }
                })?;
                if parsed.scheme() != "http" && parsed.scheme() != "https" {
                    return Err(ConfigError::InvalidSemanticSearchTeiUrl {
                        value: tei_url.to_string(),
                    });
                }
            }
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
    /// Construct an import provenance record from client, path, and timestamp.
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

    /// Attach the normalized discovered server name to this provenance record.
    #[must_use]
    pub fn with_server_name(mut self, server_name: impl Into<String>) -> Self {
        self.server_name = Some(server_name.into());
        self
    }

    /// Attach the stable discovered transport fingerprint.
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
    /// Create a tombstone stamped with the current time.
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

/// Explicit transport for an upstream MCP server. Omitted configurations
/// retain the legacy URL/command inference behavior.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UpstreamTransport {
    /// Streamable HTTP transport over TCP/TLS.
    Http,
    /// WebSocket MCP transport.
    Websocket,
    /// Child-process stdio MCP transport.
    Stdio,
    /// HTTP MCP transport carried over a Unix-domain socket.
    UnixSocket,
}

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
    /// For Unix sockets this is still an HTTP(S) URI and supplies the request
    /// target plus Host authority; the connection itself uses `socket_path`.
    #[serde(default)]
    pub url: Option<String>,
    /// Explicit transport. When omitted, legacy URL/command inference is used.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub transport: Option<UpstreamTransport>,
    /// Filesystem Unix-domain socket path, or Linux abstract `@name` notation.
    /// Valid only with `transport = "unix_socket"`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub socket_path: Option<String>,
    /// Custom HTTP headers sent with every request for HTTP and Unix-socket
    /// transports. Use `bearer_token_env` for Authorization rather than storing
    /// credentials directly in this map.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub headers: BTreeMap<String, String>,
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
    /// Whether to proxy Agent Skills (SEP-2640) from this upstream.
    ///
    /// Defaults to **false**, unlike `proxy_resources`/`proxy_prompts`. Skills
    /// carry instructions an agent will act on, so aggregating them from an
    /// upstream is a trust decision an operator should make deliberately —
    /// and the extension remains explicitly opt-in per upstream.
    #[serde(default)]
    pub proxy_skills: bool,
    /// Optional allowlist of skill names/patterns to expose from this upstream.
    #[serde(default)]
    pub expose_skills: Option<Vec<String>>,
    /// Optional short model-visible capability hint for this upstream in Code Mode.
    ///
    /// This is operator-approved display metadata only. It must not affect
    /// routing, auth, enablement, exposure policy, or tool execution.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub code_mode_hint: Option<String>,
    /// Optional outbound OAuth configuration. Mutually exclusive with
    /// `bearer_token_env` — setting both is a config error.
    #[serde(default)]
    pub oauth: Option<UpstreamOauthConfig>,
    /// Import provenance — present when this upstream was discovered from an
    /// external MCP config rather than added manually. Omitted for manual entries.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub imported_from: Option<ImportSource>,
}

/// Normalize an operator-approved Code Mode upstream capability hint.
///
/// Hints are model-visible metadata, so this is intentionally a positive
/// policy: short ASCII-ish single-line capability summaries with simple
/// punctuation only. Anything that looks like instructions, local endpoints,
/// paths, markup, secrets, control text, or second-person language is omitted.
#[must_use]
pub fn normalize_code_mode_hint(raw: &str) -> Option<String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() || trimmed.chars().count() > CODE_MODE_HINT_MAX_CHARS {
        return None;
    }
    let mut previous_was_space = false;
    let mut normalized = String::with_capacity(trimmed.len());
    for ch in trimmed.chars() {
        if ch.is_control()
            || matches!(
                ch,
                '`'
                    | '<'
                    | '>'
                    | '['
                    | ']'
                    | '{'
                    | '}'
                    | '\\'
                    | '|'
                    | '#'
                    | '$'
                    | '@'
                    | '^'
                    | '*'
                    | '~'
                    | '"'
                    | '\''
                    | '\u{202A}'..='\u{202E}'
                    | '\u{2066}'..='\u{2069}'
            )
        {
            return None;
        }
        if ch.is_whitespace() {
            if !previous_was_space {
                normalized.push(' ');
                previous_was_space = true;
            }
            continue;
        }
        previous_was_space = false;
        if !(ch.is_ascii_alphanumeric()
            || matches!(
                ch,
                ' ' | ',' | '.' | ':' | ';' | '&' | '/' | '-' | '_' | '(' | ')'
            ))
        {
            return None;
        }
        normalized.push(ch);
    }
    let normalized = normalized.trim();
    if normalized.is_empty() {
        return None;
    }
    let words = normalized
        .split(|ch: char| !(ch.is_ascii_alphanumeric() || ch == '_' || ch == '-'))
        .filter(|word| !word.is_empty())
        .collect::<Vec<_>>();
    if words.is_empty() || words.len() > CODE_MODE_HINT_MAX_WORDS {
        return None;
    }
    let lower = normalized.to_ascii_lowercase();
    if looks_like_path_or_endpoint(normalized, &lower) {
        return None;
    }
    let blocked_substrings = [
        "ignore",
        "must",
        "execute",
        "run ",
        "system",
        "developer",
        "prompt",
        "instruction",
        "secret",
        "token",
        "password",
        "authorization",
        "http://",
        "https://",
        "localhost",
        "127.0.0.1",
        "0.0.0.0",
        ".env",
        "/home/",
        "/users/",
        "c:\\",
        "\\\\",
        "../",
        "./",
        "read ",
        "write ",
        "delete ",
        "install ",
        "upload ",
        "download ",
        "call ",
        "tool ",
    ];
    if blocked_substrings
        .iter()
        .any(|needle| lower.contains(needle))
    {
        return None;
    }
    let blocked_words = [
        "you", "your", "yours", "should", "shall", "will", "never", "always", "please", "use",
        "return", "respond", "act", "obey", "override", "bypass", "admin", "root",
    ];
    if words
        .iter()
        .any(|word| blocked_words.contains(&word.to_ascii_lowercase().as_str()))
    {
        return None;
    }
    Some(normalized.to_string())
}

fn looks_like_path_or_endpoint(normalized: &str, lower: &str) -> bool {
    if lower.contains("://") || lower.contains("www.") || normalized.contains('/') {
        return true;
    }
    normalized.split_whitespace().any(|raw| {
        let token = raw.trim_matches(|ch: char| matches!(ch, ',' | '.' | ':' | ';' | '(' | ')'));
        let lower = token.to_ascii_lowercase();
        let bytes = lower.as_bytes();
        if bytes.len() >= 2 && bytes[1] == b':' && bytes[0].is_ascii_alphabetic() {
            return true;
        }
        let host = lower
            .split_once(':')
            .map_or(lower.as_str(), |(host, port)| {
                if !host.is_empty() && port.chars().all(|ch| ch.is_ascii_digit()) {
                    host
                } else {
                    lower.as_str()
                }
            });
        let parts = host.split('.').collect::<Vec<_>>();
        if parts.len() == 4 && parts.iter().all(|part| part.parse::<u8>().is_ok()) {
            return true;
        }
        parts.len() >= 2
            && parts.iter().all(|part| {
                !part.is_empty()
                    && part
                        .chars()
                        .all(|ch| ch.is_ascii_alphanumeric() || ch == '-')
            })
            && parts
                .last()
                .is_some_and(|last| (2..=63).contains(&last.len()))
    })
}

fn is_http_token_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric()
        || matches!(
            byte,
            b'!' | b'#'
                | b'$'
                | b'%'
                | b'&'
                | b'\''
                | b'*'
                | b'+'
                | b'-'
                | b'.'
                | b'^'
                | b'_'
                | b'`'
                | b'|'
                | b'~'
        )
}

impl UpstreamConfig {
    /// Validate the upstream name and mutually-exclusive auth shapes.
    /// `bearer_token_env` and `oauth` both configured is a config error.
    pub fn validate(&self) -> Result<(), ConfigError> {
        // Reserved: `code_mode_host` decides whether to attach the caller's
        // OAuth identity to an upstream call by testing this prefix. A
        // configured upstream allowed to claim it would be handed the caller's
        // `sub` and scopes and sent them over the network — the caller identity
        // leak that guard exists to prevent. Names permit `_`, so nothing else
        // stops it.
        if self.name.starts_with(IN_PROCESS_UPSTREAM_PREFIX) {
            return Err(ConfigError::InvalidName {
                name: self.name.clone(),
                reason: format!(
                    "`{IN_PROCESS_UPSTREAM_PREFIX}` is reserved for Labby's in-process \
                     services and cannot prefix an upstream name"
                ),
            });
        }

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
        // When this upstream proxies skills, its name additionally becomes the
        // origin label in every `skill://` URI minted for it, so it must satisfy
        // the tighter origin-label grammar. Enforced only under `proxy_skills`
        // on purpose: the general name rules above already accept uppercase,
        // `_`, and `.`, and existing deployments must keep loading. Upstreams
        // that do not proxy skills are untouched.
        if self.proxy_skills {
            if self.name == crate::skills::FIRST_PARTY_ORIGIN {
                return Err(ConfigError::InvalidName {
                    name: self.name.clone(),
                    reason: format!(
                        "`{}` is reserved for Labby's own skills and cannot be used by an \
                         upstream with `proxy_skills` enabled",
                        crate::skills::FIRST_PARTY_ORIGIN
                    ),
                });
            }
            if !crate::skills::is_valid_origin_label(&self.name) {
                return Err(ConfigError::InvalidName {
                    name: self.name.clone(),
                    reason: "must be lowercase ASCII letters, digits, and interior hyphens to be \
                             usable as a skill origin label with `proxy_skills` enabled"
                        .to_string(),
                });
            }
        }
        // Reserved namespace: `__in_process__*` belongs to the synthetic
        // builtin-service peers. A configured upstream using it would shadow
        // a builtin's catalog entry and capture its Code Mode tool calls.
        if self.name.starts_with(IN_PROCESS_UPSTREAM_PREFIX) {
            return Err(ConfigError::InvalidName {
                name: self.name.clone(),
                reason: format!(
                    "must not start with `{IN_PROCESS_UPSTREAM_PREFIX}` — that prefix is \
                     reserved for Labby's built-in service peers"
                ),
            });
        }
        if self.bearer_token_env.is_some() && self.oauth.is_some() {
            return Err(ConfigError::ConflictingAuth {
                name: self.name.clone(),
            });
        }
        self.validate_transport()?;
        if self.oauth.is_some() && self.url.is_none() {
            return Err(ConfigError::MissingOauthUrl {
                name: self.name.clone(),
            });
        }
        if let Some(oauth) = self.oauth.as_ref()
            && oauth.credential.is_google_provider()
        {
            let invalid_oauth = |reason: &str| ConfigError::InvalidOauth {
                name: self.name.clone(),
                reason: reason.to_string(),
            };
            if !self
                .url
                .as_deref()
                .is_some_and(|url| url.starts_with("https://"))
            {
                return Err(invalid_oauth(
                    "google_provider requires an https:// Google MCP endpoint",
                ));
            }
            let UpstreamOauthRegistration::Preregistered {
                client_id,
                client_secret_env,
            } = &oauth.registration
            else {
                return Err(invalid_oauth(
                    "google_provider requires registration.strategy = preregistered",
                ));
            };
            if client_id.trim().is_empty() {
                return Err(invalid_oauth(
                    "google_provider requires a non-empty preregistered client_id",
                ));
            }
            if client_secret_env
                .as_deref()
                .map(str::trim)
                .is_none_or(str::is_empty)
            {
                return Err(invalid_oauth(
                    "google_provider requires client_secret_env for the Google OAuth client",
                ));
            }
            if oauth
                .scopes
                .as_ref()
                .is_none_or(|scopes| scopes.iter().all(|scope| scope.trim().is_empty()))
            {
                return Err(invalid_oauth(
                    "google_provider requires at least one Google Workspace application scope",
                ));
            }
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

    /// Resolve an explicit transport or preserve legacy URL/command inference.
    #[must_use]
    pub fn effective_transport(&self) -> Option<UpstreamTransport> {
        self.transport.or_else(|| {
            if self
                .url
                .as_deref()
                .is_some_and(|url| url.starts_with("ws://") || url.starts_with("wss://"))
            {
                Some(UpstreamTransport::Websocket)
            } else if self.url.is_some() {
                Some(UpstreamTransport::Http)
            } else if self.command.is_some() {
                Some(UpstreamTransport::Stdio)
            } else {
                None
            }
        })
    }

    fn validate_transport(&self) -> Result<(), ConfigError> {
        let invalid = |reason: &str| ConfigError::InvalidTransport {
            name: self.name.clone(),
            reason: reason.to_string(),
        };
        for (name, value) in &self.headers {
            if name.is_empty() || !name.bytes().all(is_http_token_byte) {
                return Err(invalid(
                    "custom header names must be valid HTTP token values",
                ));
            }
            if name.eq_ignore_ascii_case("authorization") {
                return Err(invalid(
                    "custom Authorization headers are forbidden; use bearer_token_env or OAuth",
                ));
            }
            if value.bytes().any(|byte| {
                byte == b'\r'
                    || byte == b'\n'
                    || byte == 0
                    || (byte < 0x20 && byte != b'\t')
                    || byte == 0x7f
            }) {
                return Err(invalid(
                    "custom header values contain invalid control bytes",
                ));
            }
        }
        if self.transport.is_none() && self.socket_path.is_some() {
            return Err(invalid("socket_path requires transport = \"unix_socket\""));
        }
        match self.effective_transport() {
            None => return Err(invalid("upstream requires a url or command")),
            Some(UpstreamTransport::UnixSocket) => {
                if !cfg!(unix) {
                    return Err(invalid("unix_socket is unsupported on this platform"));
                }
                let path = self
                    .socket_path
                    .as_deref()
                    .map(str::trim)
                    .filter(|path| !path.is_empty())
                    .ok_or_else(|| invalid("unix_socket requires a non-empty socket_path"))?;
                if path == "@" {
                    return Err(invalid(
                        "abstract socket_path must include a name after '@'",
                    ));
                }
                if path.starts_with('@') && !cfg!(target_os = "linux") {
                    return Err(invalid(
                        "abstract @name sockets are supported only on Linux",
                    ));
                }
                let Some(url) = self.url.as_deref() else {
                    return Err(invalid(
                        "unix_socket requires an HTTP(S) url for request URI and Host authority",
                    ));
                };
                if !(url.starts_with("http://") || url.starts_with("https://")) {
                    return Err(invalid("unix_socket url must use http:// or https://"));
                }
                if self.command.is_some() {
                    return Err(invalid("unix_socket cannot also configure command"));
                }
            }
            Some(UpstreamTransport::Http) => {
                if self.socket_path.is_some() || self.command.is_some() {
                    return Err(invalid("http cannot configure socket_path or command"));
                }
                if !self
                    .url
                    .as_deref()
                    .is_some_and(|url| url.starts_with("http://") || url.starts_with("https://"))
                {
                    return Err(invalid("http requires an http:// or https:// url"));
                }
            }
            Some(UpstreamTransport::Websocket) => {
                if self.socket_path.is_some() || self.command.is_some() {
                    return Err(invalid("websocket cannot configure socket_path or command"));
                }
                if !self.headers.is_empty() {
                    return Err(invalid("websocket custom headers are not supported"));
                }
                if !self
                    .url
                    .as_deref()
                    .is_some_and(|url| url.starts_with("ws://") || url.starts_with("wss://"))
                {
                    return Err(invalid("websocket requires a ws:// or wss:// url"));
                }
                if self.oauth.is_some() {
                    return Err(invalid("websocket does not support outbound OAuth"));
                }
            }
            Some(UpstreamTransport::Stdio) => {
                if self.url.is_some() || self.socket_path.is_some() {
                    return Err(invalid("stdio cannot configure url or socket_path"));
                }
                if self
                    .command
                    .as_deref()
                    .map(str::trim)
                    .is_none_or(str::is_empty)
                {
                    return Err(invalid("stdio requires a non-empty command"));
                }
                if self.oauth.is_some() || !self.headers.is_empty() {
                    return Err(invalid(
                        "stdio cannot configure OAuth or custom HTTP headers",
                    ));
                }
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

// ─── Gateway loadouts + protected MCP routes ────────────────────────────────

/// Named reusable gateway capability projection.
///
/// Loadouts select the upstream/service world a protected gateway route can see
/// and gate whole MCP capability categories. Per-upstream exposure policies are
/// still enforced underneath this layer, so a loadout can only narrow access.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GatewayLoadoutConfig {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// Selected upstream MCP servers.
    ///
    /// Always serialized, including when empty. Agents and HTTP clients read
    /// this as a required array on every `gateway.loadout.*` response, so
    /// omitting it for a services-only Loadout breaks them. The cost is an
    /// explicit `upstreams = []` in TOML, which is the honest representation.
    #[serde(default)]
    pub upstreams: Vec<String>,
    /// Selected built-in Labby services.
    ///
    /// Always serialized, including when empty, for the same reason as
    /// [`GatewayLoadoutConfig::upstreams`].
    #[serde(default)]
    pub services: Vec<String>,
    /// Redacted references to host-custodied credentials. A loadout may select
    /// a binding generation, but never contains or returns secret material.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub credential_bindings: Vec<GatewayCredentialBindingRef>,
    #[serde(default)]
    pub expose_code_mode: bool,
    #[serde(default = "default_true")]
    pub expose_tools: bool,
    #[serde(default = "default_true")]
    pub expose_resources: bool,
    #[serde(default = "default_true")]
    pub expose_prompts: bool,
    #[serde(default = "default_true")]
    pub expose_skills: bool,
}

impl Default for GatewayLoadoutConfig {
    fn default() -> Self {
        Self {
            name: String::new(),
            description: None,
            upstreams: Vec::new(),
            services: Vec::new(),
            credential_bindings: Vec::new(),
            expose_code_mode: false,
            expose_tools: true,
            expose_resources: true,
            expose_prompts: true,
            expose_skills: true,
        }
    }
}

impl GatewayLoadoutConfig {
    #[allow(clippy::result_unit_err)] // The caller intentionally maps every mismatch to one redacted boundary.
    pub fn intersect_gateway_subset(
        &self,
        target: &ProtectedGatewaySubsetTarget,
    ) -> Result<Self, ()> {
        if let Some(name) = target.loadout.as_deref() {
            return (name == self.name).then(|| self.clone()).ok_or(());
        }
        let upstreams = target
            .upstreams
            .iter()
            .map(String::as_str)
            .collect::<std::collections::BTreeSet<_>>();
        let services = target
            .services
            .iter()
            .map(String::as_str)
            .collect::<std::collections::BTreeSet<_>>();
        Ok(Self {
            name: self.name.clone(),
            description: self.description.clone(),
            upstreams: self
                .upstreams
                .iter()
                .filter(|name| upstreams.contains(name.as_str()))
                .cloned()
                .collect(),
            services: self
                .services
                .iter()
                .filter(|name| services.contains(name.as_str()))
                .cloned()
                .collect(),
            credential_bindings: self
                .credential_bindings
                .iter()
                .filter(|binding| upstreams.contains(binding.upstream_name.as_str()))
                .cloned()
                .collect(),
            expose_code_mode: self.expose_code_mode && target.expose_code_mode,
            expose_tools: self.expose_tools,
            expose_resources: self.expose_resources,
            expose_prompts: self.expose_prompts,
            expose_skills: self.expose_skills,
        })
    }
}

/// Secret-free reference from a Team-owned loadout to an installation-owned
/// credential. Rotation creates a new generation, which changes runtime cache
/// identity and makes retained work carrying the old generation fail closed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GatewayCredentialBindingRef {
    pub upstream_name: String,
    pub binding_id: String,
    pub generation: u64,
}

/// Explicit target kind for an OAuth-protected public MCP route.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ProtectedMcpRouteTarget {
    /// Publish a filtered subset of Labby gateway upstreams and built-in services.
    GatewaySubset(ProtectedGatewaySubsetTarget),
}

/// Gateway subset exposed by a protected MCP route.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct ProtectedGatewaySubsetTarget {
    /// Authoritative Project bound to this explicit gateway-subset route.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project_id: Option<String>,
    /// Optional named loadout. When set, inline subset fields must stay empty
    /// so the effective policy has one authoritative source.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub loadout: Option<String>,
    /// Named upstream MCP servers visible on the route.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub upstreams: Vec<String>,
    /// Built-in Labby services visible on the route.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub services: Vec<String>,
    /// Whether the route exposes the Code Mode surface over its filtered catalog.
    #[serde(default)]
    pub expose_code_mode: bool,
}

impl ProtectedGatewaySubsetTarget {
    pub fn canonical_project_id(&self) -> Option<&str> {
        let project_id = self.project_id.as_deref()?;
        is_canonical_project_id(project_id).then_some(project_id)
    }
}

pub const MAX_PROJECT_ID_LEN: usize = 128;

pub fn is_canonical_project_id(project_id: &str) -> bool {
    !project_id.is_empty()
        && project_id.trim() == project_id
        && project_id.len() <= MAX_PROJECT_ID_LEN
        && !project_id.chars().any(char::is_control)
}

/// Resolved runtime target after legacy protected-route fields are normalized.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProtectedMcpRouteEffectiveTarget {
    /// Proxy directly to a backend MCP URL.
    BackendUrl {
        /// Canonical backend MCP URL.
        url: String,
    },
    /// Publish one named configured upstream.
    Upstream {
        /// Configured upstream name.
        name: String,
    },
    /// Publish a filtered subset of the Labby gateway.
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
    /// Public host that reaches Lab through the edge proxy, e.g. `mcp.example.com`.
    pub public_host: String,
    /// Public path prefix on that host, e.g. `/syslog`.
    pub public_path: String,
    /// Optional named Gateway upstream to publish at this protected route.
    /// When set, Lab uses the upstream registry and its configured upstream
    /// auth instead of proxying directly to `backend_url`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub upstream: Option<String>,
    /// Full backend MCP endpoint URL, e.g. `http://100.64.0.10:3100/mcp`.
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
    /// Return the canonical public OAuth resource URL for this route.
    #[must_use]
    pub fn public_resource(&self) -> String {
        format!("https://{}{}", self.public_host, self.public_path)
    }

    /// Resolve the configured route target, including legacy upstream/backend fields.
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

    /// Return whether this route explicitly targets a gateway subset.
    #[must_use]
    pub fn is_gateway_subset(&self) -> bool {
        matches!(self.target, Some(ProtectedMcpRouteTarget::GatewaySubset(_)))
    }

    /// Borrow the explicit gateway-subset target when configured.
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
    /// An upstream name violates gateway naming rules.
    InvalidName {
        /// Invalid upstream name.
        name: String,
        /// Explanation of the naming violation.
        reason: String,
    },
    #[error("upstream '{name}' has both bearer_token_env and oauth configured — pick one")]
    /// Bearer-token and OAuth authentication were configured together.
    ConflictingAuth {
        /// Upstream with conflicting authentication configuration.
        name: String,
    },
    #[error("upstream '{name}' has invalid url: {url}")]
    /// An upstream URL is malformed or unsupported.
    InvalidUrl {
        /// Upstream name.
        name: String,
        /// Rejected URL.
        url: String,
    },
    #[error("upstream '{name}' has oauth configured but no url — oauth requires an HTTP url")]
    /// OAuth was configured for an upstream without an HTTP URL.
    MissingOauthUrl {
        /// Upstream missing the required URL.
        name: String,
    },
    #[error("upstream '{name}' has invalid oauth configuration: {reason}")]
    /// Outbound OAuth settings failed validation.
    InvalidOauth {
        /// Upstream name.
        name: String,
        /// Explanation of the OAuth validation failure.
        reason: String,
    },
    #[error("upstream '{name}' has invalid transport configuration: {reason}")]
    /// Upstream transport fields failed cross-field validation.
    InvalidTransport {
        /// Upstream name.
        name: String,
        /// Explanation of the transport validation failure.
        reason: String,
    },
    #[error("gateway code_mode.timeout_ms={value} is invalid — expected 1..=60000")]
    /// Code Mode timeout falls outside the supported range.
    InvalidCodeModeTimeout {
        /// Rejected timeout in milliseconds.
        value: u64,
    },
    #[error("gateway code_mode.max_source_bytes={value} is invalid — expected 1024..=1048576")]
    /// Code Mode source byte cap falls outside the supported range.
    InvalidCodeModeMaxSourceBytes {
        /// Rejected byte limit.
        value: usize,
    },
    #[error("gateway code_mode.max_response_bytes={value} is invalid — expected 1024..=1048576")]
    /// Code Mode response byte cap falls outside the supported range.
    InvalidCodeModeMaxResponseBytes {
        /// Rejected byte limit.
        value: usize,
    },
    #[error("gateway code_mode.max_response_tokens={value} is invalid — expected 256..=256000")]
    /// Code Mode response token cap falls outside the supported range.
    InvalidCodeModeMaxResponseTokens {
        /// Rejected approximate token limit.
        value: usize,
    },
    #[error("gateway code_mode.token_estimate_divisor={value} is invalid — expected 1..=64")]
    /// Code Mode token-estimation divisor falls outside the supported range.
    InvalidCodeModeTokenEstimateDivisor {
        /// Rejected bytes-per-token divisor.
        value: u32,
    },
    #[error("gateway code_mode.max_log_entries={value} is invalid — expected 1..=100000")]
    /// Code Mode log-entry cap falls outside the supported range.
    InvalidCodeModeMaxLogEntries {
        /// Rejected log-entry limit.
        value: usize,
    },
    #[error("gateway code_mode.max_log_bytes={value} is invalid — expected 1..=104857600")]
    /// Code Mode log-byte cap falls outside the supported range.
    InvalidCodeModeMaxLogBytes {
        /// Rejected log-byte limit.
        value: usize,
    },
    #[error(
        "gateway code_mode.semantic_search.blend_weight={value} is invalid — expected 0.0..=1.0"
    )]
    /// Semantic-search blend weight is outside the inclusive 0.0–1.0 range.
    InvalidSemanticSearchBlendWeight {
        /// Rejected semantic blend weight.
        value: f32,
    },
    #[error(
        "gateway code_mode.semantic_search.tei_url={value:?} is invalid — expected a well-formed http:// or https:// URL"
    )]
    /// Semantic-search TEI endpoint is not a valid HTTP(S) URL.
    InvalidSemanticSearchTeiUrl {
        /// Rejected TEI URL.
        value: String,
    },
    #[error("gateway upstream_request_timeout_ms={value} is invalid — expected 1..=300000")]
    /// Upstream request timeout falls outside the supported range.
    InvalidUpstreamRequestTimeout {
        /// Rejected request timeout in milliseconds.
        value: u64,
    },
    #[error("gateway upstream_relay_timeout_ms={value} is invalid — expected 1..=1800000")]
    /// Upstream relay timeout falls outside the supported range.
    InvalidUpstreamRelayTimeout {
        /// Rejected relay timeout in milliseconds.
        value: u64,
    },
    #[error("gateway mcp.catalog_notification_timeout_ms={value} is invalid — expected 1..=60000")]
    /// Catalog-notification timeout falls outside the supported range.
    InvalidCatalogNotificationTimeout {
        /// Rejected notification timeout in milliseconds.
        value: u64,
    },
    #[error("invalid proxy configuration: {reason}")]
    /// Proxy preferences failed cross-field validation.
    InvalidProxyConfig {
        /// Explanation of the proxy validation failure.
        reason: String,
    },
    #[error("protected MCP route '{name}' has invalid {field}: {value}")]
    /// A protected MCP route contains an invalid field value.
    InvalidProtectedRoute {
        /// Route name.
        name: String,
        /// Invalid field name.
        field: &'static str,
        /// Rejected field value.
        value: String,
    },
    #[error(
        "openapi spec label '{label}' is reserved — pick another (reserved: state, git, openapi)"
    )]
    /// An OpenAPI provider label uses a reserved Labby namespace.
    ReservedLabel {
        /// Rejected reserved label.
        label: String,
    },
    #[error(
        "openapi spec label '{label}' is invalid — labels must be non-empty and use only \
         ASCII letters, digits, '_' or '-' (no '.', ':', or whitespace, which would break \
         the openapi::<label>.<operationId> dispatch key)"
    )]
    /// An OpenAPI provider label violates identifier rules.
    InvalidLabel {
        /// Rejected provider label.
        label: String,
    },
    #[error("openapi spec label '{label}' is configured more than once")]
    /// An OpenAPI provider label is configured more than once.
    DuplicateLabel {
        /// Duplicate provider label.
        label: String,
    },
    #[error("openapi spec '{label}' is missing the mandatory base_url")]
    /// An OpenAPI provider is missing its mandatory base URL.
    MissingBaseUrl {
        /// Provider label missing the base URL.
        label: String,
    },
    #[error("openapi spec '{label}' has an invalid base_url")]
    /// An OpenAPI provider base URL is invalid.
    InvalidBaseUrl {
        /// Provider label with the invalid base URL.
        label: String,
    },
    #[error("openapi spec '{label}' has an invalid spec_url")]
    /// An OpenAPI provider spec URL is invalid.
    InvalidSpecUrl {
        /// Provider label with the invalid spec URL.
        label: String,
    },
    #[error("openapi spec '{label}' must set exactly one of spec_url or spec_path")]
    /// An OpenAPI provider sets zero or multiple spec sources.
    SpecSourceAmbiguous {
        /// Provider label with ambiguous spec source configuration.
        label: String,
    },
}

// ─── Outbound OAuth ──────────────────────────────────────────────────────────

/// Outbound OAuth configuration for an upstream MCP server.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UpstreamOauthConfig {
    /// OAuth authorization flow used by this upstream.
    pub mode: UpstreamOauthMode,
    /// Client registration strategy used for the upstream authorization server.
    pub registration: UpstreamOauthRegistration,
    /// Optional OAuth scopes requested during authorization.
    #[serde(default)]
    pub scopes: Option<Vec<String>>,
    /// Selects where OAuth credentials for this upstream are persisted.
    ///
    /// `dedicated` preserves the original per-`(upstream, subject)` token store.
    /// `google_provider` reuses Labby's encrypted, subject-scoped Google provider
    /// credential and never copies its refresh token into an upstream row.
    #[serde(default)]
    pub credential: UpstreamOauthCredentialSource,
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
    /// OAuth authorization-code flow with PKCE.
    AuthorizationCodePkce,
}

/// Persistence source for an upstream OAuth credential.
#[derive(Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "source", rename_all = "snake_case")]
pub enum UpstreamOauthCredentialSource {
    /// Keep an encrypted token bundle per `(upstream, subject)`.
    #[default]
    Dedicated,
    /// Reuse the central Google provider credential.
    ///
    /// `account` accepts either a Google `sub` or a verified email address. When
    /// omitted, resolution succeeds only when exactly one provider credential
    /// exists.
    GoogleProvider {
        /// Optional Google subject or verified email used to select a shared credential.
        #[serde(default)]
        account: Option<String>,
    },
}

impl std::fmt::Debug for UpstreamOauthCredentialSource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Dedicated => f.write_str("Dedicated"),
            Self::GoogleProvider { account } => f
                .debug_struct("GoogleProvider")
                .field("account", &account.as_ref().map(|_| "<redacted>"))
                .finish(),
        }
    }
}

impl UpstreamOauthCredentialSource {
    /// Return whether this upstream reuses the central Google provider credential.
    #[must_use]
    pub const fn is_google_provider(&self) -> bool {
        matches!(self, Self::GoogleProvider { .. })
    }

    /// Return the configured Google account selector, when present.
    #[must_use]
    pub fn account(&self) -> Option<&str> {
        match self {
            Self::GoogleProvider { account } => account.as_deref(),
            Self::Dedicated => None,
        }
    }
}

/// Outbound OAuth client-registration strategy.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "strategy", rename_all = "snake_case")]
pub enum UpstreamOauthRegistration {
    /// Use a Client ID Metadata Document published at a fixed URL.
    ClientMetadataDocument {
        /// URL of the client metadata document.
        url: String,
    },
    /// Use operator-provided client credentials registered out of band.
    Preregistered {
        /// OAuth client identifier.
        client_id: String,
        /// Optional environment variable containing the client secret.
        #[serde(default)]
        client_secret_env: Option<String>,
    },
    /// Register a client dynamically with the upstream authorization server.
    Dynamic,
}

// ─── Virtual servers ─────────────────────────────────────────────────────────

/// Persisted state for a Labby-backed virtual server shown in the gateway.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct VirtualServerConfig {
    /// Stable virtual-server identifier.
    pub id: String,
    /// Built-in Labby service projected by this virtual server.
    pub service: String,
    /// Whether the virtual server is enabled.
    #[serde(default)]
    pub enabled: bool,
    /// Product surfaces on which the virtual server is exposed.
    #[serde(default)]
    pub surfaces: VirtualServerSurfacesConfig,
    /// Optional action-level MCP allowlist policy.
    #[serde(default)]
    pub mcp_policy: Option<VirtualServerMcpPolicyConfig>,
}

/// Per-surface exposure flags for a virtual server.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct VirtualServerSurfacesConfig {
    /// Expose the service through the CLI.
    #[serde(default)]
    pub cli: bool,
    /// Expose the service through the HTTP API.
    #[serde(default)]
    pub api: bool,
    /// Expose the service through MCP.
    #[serde(default)]
    pub mcp: bool,
    /// Expose the service through the operator web UI.
    #[serde(default)]
    pub webui: bool,
}

/// Action-level policy for Labby-backed single-tool MCP services.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct VirtualServerMcpPolicyConfig {
    /// Service actions exposed by the virtual server's MCP tool.
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
    /// Max concurrent upstream discovery/reprobe connections.
    /// Overridden by `LABBY_UPSTREAM_DISCOVERY_CONCURRENCY` env var. Default: 3.
    #[serde(default)]
    pub upstream_discovery_concurrency: Option<usize>,
    /// Max accepted upstream response size in bytes.
    /// Overridden by `LABBY_UPSTREAM_MAX_RESPONSE_BYTES` env var.
    #[serde(default)]
    pub upstream_max_response_bytes: Option<usize>,
    /// Timeout in milliseconds for the MCP runtime catalog warm-cache path.
    /// Overridden by `LABBY_GATEWAY_MCP_LIST_WARM_TIMEOUT_MS` env var. Default: 5000.
    #[serde(default)]
    pub mcp_list_warm_timeout_ms: Option<u64>,
    /// Log level for forwarded stdio upstream stderr output: `"trace"`,
    /// `"debug"` (default), `"info"`, `"warn"`, `"error"`, or `"off"`/`"null"`
    /// to discard. Overridden by `LABBY_GW_UPSTREAM_STDERR` env var.
    #[serde(default)]
    pub upstream_stderr_level: Option<String>,
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
/// host through the `GatewayConfigStore` seam in `labby-gateway`. There is
/// intentionally **no** `#[serde(flatten)]` bag here — preservation of unrelated
/// `config.toml` keys stays the host's job, because the host keeps `LabConfig`.
///
/// [`GatewayManager`]: ../../labby_gateway/gateway/struct.GatewayManager.html
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct GatewayConfig {
    /// Gateway-wide Code Mode exposure and execution settings.
    #[serde(default)]
    pub code_mode: CodeModeConfig,
    /// Visibility of Labby-owned MCP App surfaces other than Code Mode.
    #[serde(default)]
    pub mcp_apps: McpAppsConfig,
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
    /// Named reusable gateway capability projections.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub loadouts: Vec<GatewayLoadoutConfig>,
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
                if let Some(project_id) = target.project_id.take() {
                    let project_id = project_id.trim().to_string();
                    if !is_canonical_project_id(&project_id) {
                        return Err(ConfigError::InvalidProtectedRoute {
                            name: route.name.clone(),
                            field: "target.project_id",
                            value: "project binding must not be empty".to_string(),
                        });
                    }
                    target.project_id = Some(project_id);
                }
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
                // A protected route must never name a synthetic in-process
                // peer: builtins reach the Code Mode catalog only on the root
                // scope, and listing one here would hand the route builtin
                // tools it is meant to exclude (bead lab-eyeuv).
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

#[cfg(test)]
mod tests {
    #[test]
    fn gateway_subset_routes_may_share_a_path_on_different_hosts() {
        let mut config: GatewayConfig = toml::from_str(
            r#"
[[protected_mcp_routes]]
name = "alpha"
public_host = "alpha.example.com"
public_path = "/mcp"

[protected_mcp_routes.target]
kind = "gateway_subset"
expose_code_mode = true

[[protected_mcp_routes]]
name = "beta"
public_host = "beta.example.com"
public_path = "/mcp"

[protected_mcp_routes.target]
kind = "gateway_subset"
expose_code_mode = true
"#,
        )
        .expect("parse gateway config");

        config
            .normalize_protected_mcp_routes()
            .expect("host-specific routes can share a path");
    }

    #[test]
    fn code_mode_max_source_bytes_validation_is_bounded() {
        let mut config = CodeModeConfig {
            max_source_bytes: 1023,
            ..CodeModeConfig::default()
        };
        assert!(matches!(
            config.validate(),
            Err(ConfigError::InvalidCodeModeMaxSourceBytes { value: 1023 })
        ));

        config.max_source_bytes = 1024;
        config.validate().expect("minimum source budget validates");

        config.max_source_bytes = 1024 * 1024;
        config.validate().expect("hard source ceiling validates");

        config.max_source_bytes = 1024 * 1024 + 1;
        assert!(matches!(
            config.validate(),
            Err(ConfigError::InvalidCodeModeMaxSourceBytes { value }) if value == 1024 * 1024 + 1
        ));
    }

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
    fn skills_proxying_is_opt_in_and_does_not_disturb_existing_configs() {
        let cfg: UpstreamConfig = toml::from_str("name=\"axon\"\nurl=\"https://x/mcp\"\n").unwrap();
        // Deliberately unlike proxy_resources/proxy_prompts, which default true.
        assert!(!cfg.proxy_skills);
        assert!(cfg.expose_skills.is_none());

        // An upstream name the general rules allow but the origin-label grammar
        // does not must keep loading and validating while skills are off — the
        // whole point of gating the tighter rule on proxy_skills.
        let legacy: UpstreamConfig =
            toml::from_str("name=\"My_Server.v2\"\nurl=\"https://x/mcp\"\n").unwrap();
        legacy
            .validate()
            .expect("an existing config must not start failing because skills exist");
    }

    #[test]
    fn skills_proxying_requires_a_usable_origin_label() {
        // With proxy_skills on, the name becomes the origin label in every
        // minted skill:// URI, so it must satisfy the tighter grammar.
        for name in ["My_Server.v2", "UPPER", "trailing-", "has space"] {
            let cfg: UpstreamConfig = toml::from_str(&format!(
                "name=\"{name}\"\nurl=\"https://x/mcp\"\nproxy_skills=true\n"
            ))
            .unwrap();
            assert!(
                cfg.validate().is_err(),
                "`{name}` must be rejected as an origin label"
            );
        }

        let ok: UpstreamConfig =
            toml::from_str("name=\"upstream-2\"\nurl=\"https://x/mcp\"\nproxy_skills=true\n")
                .unwrap();
        ok.validate().expect("a conforming label is accepted");
    }

    #[test]
    fn the_first_party_origin_label_is_reserved_from_upstreams() {
        let cfg: UpstreamConfig =
            toml::from_str("name=\"labby\"\nurl=\"https://x/mcp\"\nproxy_skills=true\n").unwrap();
        let error = cfg
            .validate()
            .expect_err("an upstream must not claim Labby's own skill namespace");
        assert!(format!("{error}").contains("reserved"));

        // ...but only when it actually proxies skills; the name is otherwise
        // unremarkable and must not break an existing deployment.
        let without: UpstreamConfig =
            toml::from_str("name=\"labby\"\nurl=\"https://x/mcp\"\n").unwrap();
        without.validate().expect("no skills, no reservation");
    }

    #[test]
    fn omitted_transport_preserves_legacy_inference() {
        let http: UpstreamConfig =
            toml::from_str("name=\"http\"\nurl=\"https://example.com/mcp\"\n").unwrap();
        assert_eq!(http.effective_transport(), Some(UpstreamTransport::Http));

        let websocket: UpstreamConfig =
            toml::from_str("name=\"ws\"\nurl=\"wss://example.com/mcp\"\n").unwrap();
        assert_eq!(
            websocket.effective_transport(),
            Some(UpstreamTransport::Websocket)
        );

        let stdio: UpstreamConfig = toml::from_str("name=\"stdio\"\ncommand=\"server\"\n").unwrap();
        assert_eq!(stdio.effective_transport(), Some(UpstreamTransport::Stdio));
    }

    #[cfg(unix)]
    #[test]
    fn filesystem_unix_socket_transport_parses_and_validates() {
        let cfg: UpstreamConfig = toml::from_str(
            "name=\"local\"\ntransport=\"unix_socket\"\nsocket_path=\"/tmp/local-mcp.sock\"\nurl=\"http://local.internal/mcp\"\n",
        )
        .unwrap();

        assert_eq!(
            cfg.effective_transport(),
            Some(UpstreamTransport::UnixSocket)
        );
        assert_eq!(cfg.socket_path.as_deref(), Some("/tmp/local-mcp.sock"));
        assert!(cfg.validate().is_ok());
    }

    #[test]
    fn socket_path_without_unix_transport_is_rejected() {
        let cfg: UpstreamConfig = toml::from_str(
            "name=\"bad\"\nsocket_path=\"/tmp/local-mcp.sock\"\nurl=\"http://local.internal/mcp\"\n",
        )
        .unwrap();

        let error = cfg.validate().unwrap_err();
        assert!(matches!(error, ConfigError::InvalidTransport { .. }));
        assert!(error.to_string().contains("socket_path requires transport"));
    }

    #[cfg(unix)]
    #[test]
    fn unix_socket_requires_url_and_valid_path() {
        let missing_url: UpstreamConfig = toml::from_str(
            "name=\"bad\"\ntransport=\"unix_socket\"\nsocket_path=\"/tmp/local-mcp.sock\"\n",
        )
        .unwrap();
        assert!(missing_url.validate().is_err());

        let empty_abstract_socket: UpstreamConfig = toml::from_str(
            "name=\"bad\"\ntransport=\"unix_socket\"\nsocket_path=\"@\"\nurl=\"http://local.internal/mcp\"\n",
        )
        .unwrap();
        assert!(empty_abstract_socket.validate().is_err());

        #[cfg(target_os = "linux")]
        {
            let abstract_socket: UpstreamConfig = toml::from_str(
                "name=\"local\"\ntransport=\"unix_socket\"\nsocket_path=\"@local-mcp\"\nurl=\"http://local.internal/mcp\"\n",
            )
            .unwrap();
            assert!(abstract_socket.validate().is_ok());
        }
    }

    #[test]
    fn custom_headers_validate_before_pool_publication() {
        let valid: UpstreamConfig = toml::from_str(
            "name=\"local\"\nurl=\"http://local.internal/mcp\"\n[headers]\nx-labby-test=\"present\"\n",
        )
        .unwrap();
        assert!(valid.validate().is_ok());

        for invalid_toml in [
            "name=\"bad\"\nurl=\"http://local.internal/mcp\"\n[headers]\nauthorization=\"Bearer secret\"\n",
            "name=\"bad\"\nurl=\"http://local.internal/mcp\"\n[headers]\n\"bad header\"=\"value\"\n",
            "name=\"bad\"\nurl=\"http://local.internal/mcp\"\n[headers]\nx-test=\"bad\\nvalue\"\n",
        ] {
            let cfg: UpstreamConfig = toml::from_str(invalid_toml).unwrap();
            assert!(cfg.validate().is_err());
        }
    }

    #[test]
    fn inferred_transports_enforce_their_field_contracts() {
        for invalid_toml in [
            "name=\"bad\"\ncommand=\"server\"\n[headers]\nx-test=\"value\"\n",
            "name=\"bad\"\nurl=\"ws://local.internal/mcp\"\n[headers]\nx-test=\"value\"\n",
            "name=\"bad\"\nurl=\"http://local.internal/mcp\"\ncommand=\"server\"\n",
            "name=\"bad\"\n",
        ] {
            let cfg: UpstreamConfig = toml::from_str(invalid_toml).unwrap();
            assert!(
                cfg.validate().is_err(),
                "config should fail: {invalid_toml}"
            );
        }
    }

    #[test]
    fn stdio_preserves_named_bearer_environment_injection() {
        let cfg: UpstreamConfig = toml::from_str(
            "name=\"stdio\"\ncommand=\"server\"\nbearer_token_env=\"SERVER_TOKEN\"\n",
        )
        .unwrap();

        assert_eq!(cfg.effective_transport(), Some(UpstreamTransport::Stdio));
        assert!(cfg.validate().is_ok());
    }

    #[test]
    fn explicit_transport_rejects_conflicting_fields() {
        let cfg: UpstreamConfig = toml::from_str(
            "name=\"bad\"\ntransport=\"stdio\"\ncommand=\"server\"\nurl=\"http://local.internal/mcp\"\n",
        )
        .unwrap();
        assert!(cfg.validate().is_err());
    }

    #[test]
    fn google_provider_oauth_requires_https_preregistered_secret_and_scopes() {
        let valid: UpstreamConfig = toml::from_str(
            r#"
name = "google-calendar"
url = "https://calendarmcp.googleapis.com/mcp/v1"
[oauth]
mode = "authorization_code_pkce"
scopes = ["https://www.googleapis.com/auth/calendar.events.readonly"]
[oauth.credential]
source = "google_provider"
account = "admin@example.com"
[oauth.registration]
strategy = "preregistered"
client_id = "google-client"
client_secret_env = "LABBY_GOOGLE_CLIENT_SECRET"
"#,
        )
        .unwrap();
        assert!(valid.validate().is_ok());

        for invalid_toml in [
            r#"
name = "google-calendar"
url = "http://calendarmcp.googleapis.com/mcp/v1"
[oauth]
mode = "authorization_code_pkce"
scopes = ["calendar"]
[oauth.credential]
source = "google_provider"
[oauth.registration]
strategy = "preregistered"
client_id = "google-client"
client_secret_env = "SECRET"
"#,
            r#"
name = "google-calendar"
url = "https://calendarmcp.googleapis.com/mcp/v1"
[oauth]
mode = "authorization_code_pkce"
scopes = ["calendar"]
[oauth.credential]
source = "google_provider"
[oauth.registration]
strategy = "dynamic"
"#,
            r#"
name = "google-calendar"
url = "https://calendarmcp.googleapis.com/mcp/v1"
[oauth]
mode = "authorization_code_pkce"
scopes = ["calendar"]
[oauth.credential]
source = "google_provider"
[oauth.registration]
strategy = "preregistered"
client_id = "google-client"
"#,
            r#"
name = "google-calendar"
url = "https://calendarmcp.googleapis.com/mcp/v1"
[oauth]
mode = "authorization_code_pkce"
[oauth.credential]
source = "google_provider"
[oauth.registration]
strategy = "preregistered"
client_id = "google-client"
client_secret_env = "SECRET"
"#,
        ] {
            let config: UpstreamConfig = toml::from_str(invalid_toml).unwrap();
            assert!(matches!(
                config.validate(),
                Err(ConfigError::InvalidOauth { .. })
            ));
        }
    }

    #[test]
    fn google_provider_credential_debug_redacts_account_selector() {
        let source = UpstreamOauthCredentialSource::GoogleProvider {
            account: Some("admin@example.com".to_string()),
        };
        let debug = format!("{source:?}");
        assert!(!debug.contains("admin@example.com"));
        assert!(debug.contains("<redacted>"));
    }

    #[test]
    fn code_mode_config_defaults_roundtrip() {
        let cfg: CodeModeConfig = toml::from_str("").unwrap();
        let expected = CodeModeConfig::default();
        assert_eq!(cfg, expected);
        assert!(!cfg.enabled);
        assert!(cfg.trusted_read_only_tools.is_empty());
        assert!(!cfg.mcp_ui_enabled);
        assert!(cfg.trace_params);
        assert_eq!(cfg.timeout_ms, 30_000);
        assert_eq!(cfg.token_estimate_divisor, 4);
    }

    #[test]
    fn a_legacy_trusted_read_only_tools_list_still_parses_but_is_never_echoed_back() {
        // The field is retired: it exists only so an existing config file keeps
        // parsing. It must not round-trip, or an operator reading the config back
        // would see a populated list that looks like a live security control.
        let cfg: CodeModeConfig =
            toml::from_str("trusted_read_only_tools = [\"dookie::read_file\"]\n").unwrap();
        assert_eq!(cfg.trusted_read_only_tools, vec!["dookie::read_file"]);

        let round_tripped = serde_json::to_value(&cfg).unwrap();
        assert!(
            round_tripped.get("trusted_read_only_tools").is_none(),
            "the retired field must never be serialized back to any surface"
        );
    }

    #[test]
    fn code_mode_mcp_ui_can_be_enabled_in_toml() {
        let cfg: CodeModeConfig = toml::from_str(
            "mcp_ui_enabled = true
",
        )
        .unwrap();
        assert!(cfg.mcp_ui_enabled);
        assert!(!cfg.enabled);
    }

    #[test]
    fn mcp_apps_config_defaults_all_managed_apps_disabled() {
        let cfg: McpAppsConfig = toml::from_str("").unwrap();
        assert_eq!(cfg, McpAppsConfig::default());
        assert!(!cfg.manager);
        assert!(!cfg.add_server);
        assert!(!cfg.server_logs);
        assert!(!cfg.gateway_status);
        assert!(!cfg.settings);
    }

    #[test]
    fn mcp_apps_config_supports_independent_visibility_switches() {
        let cfg: McpAppsConfig = toml::from_str(
            "manager = true\nadd_server = false\nserver_logs = true\ngateway_status = false\nsettings = false\n",
        )
        .unwrap();
        assert!(cfg.manager);
        assert!(!cfg.add_server);
        assert!(cfg.server_logs);
        assert!(!cfg.gateway_status);
        assert!(!cfg.settings);
    }

    #[test]
    fn semantic_search_defaults_to_unconfigured() {
        let cfg = CodeModeConfig::default();
        assert!(cfg.semantic_search.tei_url.is_none());
        assert!(!cfg.semantic_search.is_configured());
        assert!(cfg.validate().is_ok());
    }

    #[test]
    fn semantic_search_with_valid_http_url_is_configured_and_valid() {
        let mut cfg = CodeModeConfig::default();
        cfg.semantic_search.tei_url = Some("http://localhost:52000".to_string());
        assert!(cfg.semantic_search.is_configured());
        assert!(cfg.validate().is_ok());
    }

    #[test]
    fn semantic_search_with_https_url_is_valid() {
        let mut cfg = CodeModeConfig::default();
        cfg.semantic_search.tei_url = Some("https://tei.internal.example:8443".to_string());
        assert!(cfg.validate().is_ok());
    }

    #[test]
    fn semantic_search_with_non_http_scheme_fails_validation() {
        let mut cfg = CodeModeConfig::default();
        cfg.semantic_search.tei_url = Some("ftp://example.com".to_string());
        let err = cfg.validate().unwrap_err();
        assert!(matches!(
            err,
            ConfigError::InvalidSemanticSearchTeiUrl { .. }
        ));
    }

    #[test]
    fn semantic_search_with_malformed_url_fails_validation() {
        let mut cfg = CodeModeConfig::default();
        cfg.semantic_search.tei_url = Some("not a url at all".to_string());
        let err = cfg.validate().unwrap_err();
        assert!(matches!(
            err,
            ConfigError::InvalidSemanticSearchTeiUrl { .. }
        ));
    }

    #[test]
    fn semantic_search_blend_weight_out_of_range_fails_validation() {
        let mut cfg = CodeModeConfig::default();
        cfg.semantic_search.blend_weight = 1.5;
        let err = cfg.validate().unwrap_err();
        assert!(matches!(
            err,
            ConfigError::InvalidSemanticSearchBlendWeight { .. }
        ));
    }

    #[test]
    fn semantic_search_toml_round_trips_with_defaults_when_omitted() {
        // An existing config.toml with a `[code_mode]` section but no
        // `semantic_search` subsection must still deserialize (backward
        // compatibility with every config.toml written before this feature).
        let toml_str = "enabled = true\ntimeout_ms = 30000\n";
        let cfg: CodeModeConfig = toml::from_str(toml_str).unwrap();
        assert!(cfg.semantic_search.tei_url.is_none());
        assert!(!cfg.semantic_search.is_configured());
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

    /// Bead lab-eyeuv: `__in_process__*` is the namespace of the synthetic
    /// builtin-service peers (issue #210 FU-1). A configured upstream using
    /// that prefix would shadow a builtin's catalog entry and capture its
    /// Code Mode tool calls, so it is rejected at load rather than merely
    /// discouraged by comment.
    #[test]
    fn upstream_name_rejects_the_reserved_in_process_prefix() {
        let cfg: UpstreamConfig =
            toml::from_str("name=\"__in_process__gateway\"\nurl=\"https://example.com/mcp\"\n")
                .unwrap();

        let error = cfg
            .validate()
            .expect_err("reserved prefix must be rejected");
        let rendered = error.to_string();
        assert!(rendered.contains("__in_process__"), "{rendered}");

        // Ordinary names that merely contain the token are fine — only the
        // prefix is reserved.
        let ok: UpstreamConfig =
            toml::from_str("name=\"my__in_process__thing\"\nurl=\"https://example.com/mcp\"\n")
                .unwrap();
        assert!(ok.validate().is_ok());
    }

    /// A protected route must not be able to opt itself into builtin tools by
    /// naming a synthetic peer in its upstream allowlist — the `fail closed`
    /// property FU-1 documents is now enforced, not assumed.
    #[test]
    fn protected_route_rejects_reserved_in_process_upstreams() {
        let mut route: ProtectedMcpRouteConfig = toml::from_str(
            "name=\"scoped\"\npublic_host=\"mcp.example.com\"\npublic_path=\"/svc\"\n",
        )
        .unwrap();
        route.backend_url = String::new();
        route.target = Some(ProtectedMcpRouteTarget::GatewaySubset(
            ProtectedGatewaySubsetTarget {
                project_id: None,
                loadout: None,
                upstreams: vec![format!("{IN_PROCESS_UPSTREAM_PREFIX}setup")],
                services: Vec::new(),
                expose_code_mode: false,
            },
        ));
        let mut cfg = GatewayConfig {
            protected_mcp_routes: vec![route],
            ..GatewayConfig::default()
        };

        let error = cfg
            .normalize_protected_mcp_routes()
            .expect_err("a protected route must not name an in-process peer");
        let rendered = error.to_string();
        assert!(rendered.contains("__in_process__setup"), "{rendered}");
        assert!(rendered.contains("target.services"), "{rendered}");
    }

    /// The reserved prefix and the name builder in `labby-gateway` must stay
    /// in lockstep; the builder formats with this constant.
    #[test]
    fn in_process_prefix_constant_is_stable() {
        assert_eq!(IN_PROCESS_UPSTREAM_PREFIX, "__in_process__");
    }
    /// `gateway.loadout.*` responses must always carry `upstreams` and
    /// `services` as arrays. Omitting an empty selection made every JSON
    /// consumer that treats them as required arrays fail on a Loadout that
    /// picked only upstreams or only services.
    #[test]
    fn loadout_json_always_carries_both_selection_arrays() {
        let upstreams_only = GatewayLoadoutConfig {
            name: "sd".to_string(),
            upstreams: vec!["chrome-devtools".to_string()],
            ..GatewayLoadoutConfig::default()
        };
        let services_only = GatewayLoadoutConfig {
            name: "ops".to_string(),
            services: vec!["gateway".to_string()],
            ..GatewayLoadoutConfig::default()
        };

        for loadout in [&upstreams_only, &services_only] {
            let value = serde_json::to_value(loadout).expect("loadout serializes to JSON");
            let object = value.as_object().expect("loadout serializes as an object");
            assert!(
                object
                    .get("upstreams")
                    .is_some_and(serde_json::Value::is_array),
                "upstreams must always be an array: {value}"
            );
            assert!(
                object
                    .get("services")
                    .is_some_and(serde_json::Value::is_array),
                "services must always be an array: {value}"
            );
        }

        assert_eq!(
            serde_json::to_value(&services_only)
                .expect("loadout serializes to JSON")
                .get("upstreams"),
            Some(&serde_json::json!([])),
            "a services-only Loadout still reports an empty upstream selection"
        );
    }

    /// Always-serialized selections must survive a TOML round trip, including
    /// the explicit empty arrays now written into `config.toml`.
    #[test]
    fn loadout_toml_round_trips_empty_selection_arrays() {
        let services_only = GatewayLoadoutConfig {
            name: "ops".to_string(),
            services: vec!["gateway".to_string()],
            ..GatewayLoadoutConfig::default()
        };

        let rendered = toml::to_string(&services_only).expect("loadout serializes to TOML");
        assert!(rendered.contains("upstreams = []"), "{rendered}");

        let parsed: GatewayLoadoutConfig =
            toml::from_str(&rendered).expect("loadout parses back from TOML");
        assert_eq!(parsed, services_only);
    }
    #[test]
    fn protected_route_project_id_normalization_is_bounded_and_compatible() {
        fn config(project_id: Option<String>) -> GatewayConfig {
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
            GatewayConfig {
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

        let expected = "x".repeat(MAX_PROJECT_ID_LEN);
        let mut trimmed = config(Some(format!("  {expected}  ")));
        trimmed.normalize_protected_mcp_routes().expect("128 bytes");
        let Some(ProtectedMcpRouteTarget::GatewaySubset(target)) =
            trimmed.protected_mcp_routes[0].target.as_ref()
        else {
            unreachable!()
        };
        assert_eq!(target.project_id.as_deref(), Some(expected.as_str()));

        for invalid in ["   ".to_string(), "x".repeat(MAX_PROJECT_ID_LEN + 1)] {
            assert!(
                config(Some(invalid))
                    .normalize_protected_mcp_routes()
                    .is_err()
            );
        }
    }
}
