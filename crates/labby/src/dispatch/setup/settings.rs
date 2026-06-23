use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};
use std::collections::{BTreeMap, BTreeSet};
use std::sync::OnceLock;

use crate::dispatch::error::ToolError;

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

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct SettingsOption {
    pub value: &'static str,
    pub label: &'static str,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
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

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct SettingsSectionSpec {
    pub id: &'static str,
    pub label: &'static str,
    pub description: &'static str,
    pub advanced: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct SettingsSchemaResponse {
    pub schema_version: u32,
    pub sections: Vec<SettingsSectionSpec>,
    pub fields: Vec<SettingsFieldSpec>,
}

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

#[derive(Debug, Clone, Serialize)]
pub struct SettingsUpdateEntry {
    pub key: String,
    pub value: Value,
    #[serde(default)]
    pub previous: Value,
    #[serde(default)]
    pub unset: bool,
    #[serde(skip)]
    pub previous_present: bool,
}

impl<'de> Deserialize<'de> for SettingsUpdateEntry {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        use serde::de::Error as _;

        let mut object = Map::<String, Value>::deserialize(deserializer)?;
        let key = object
            .remove("key")
            .and_then(|value| value.as_str().map(str::to_owned))
            .ok_or_else(|| D::Error::missing_field("key"))?;
        let value = object.remove("value").unwrap_or(Value::Null);
        let previous_present = object.contains_key("previous");
        let previous = object.remove("previous").unwrap_or(Value::Null);
        let unset = object
            .remove("unset")
            .map(|value| {
                value
                    .as_bool()
                    .ok_or_else(|| D::Error::custom("unset must be boolean"))
            })
            .transpose()?
            .unwrap_or(false);
        Ok(Self {
            key,
            value,
            previous,
            unset,
            previous_present,
        })
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SettingsMutationOutcome {
    pub state: SettingsStateResponse,
    pub backup_path: Option<String>,
}

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

pub const SETTINGS_SCHEMA_VERSION: u32 = 1;
const READONLY_MAX_DEPTH: usize = 6;
const READONLY_MAX_OBJECT_KEYS: usize = 80;
const READONLY_MAX_ARRAY_ITEMS: usize = 50;
const READONLY_MAX_STRING_BYTES: usize = 4096;

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

fn enum_editable_with_env(
    section: &'static str,
    key: &'static str,
    label: &'static str,
    description: &'static str,
    apply_mode: SettingsApplyMode,
    options: Vec<SettingsOption>,
    env_override: Option<&'static str>,
    example: Option<&'static str>,
) -> SettingsFieldSpec {
    let mut field = enum_editable(
        section,
        key,
        label,
        description,
        apply_mode,
        options,
        example,
    );
    field.env_override = env_override;
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

fn number_editable_with_env(
    section: &'static str,
    key: &'static str,
    label: &'static str,
    description: &'static str,
    apply_mode: SettingsApplyMode,
    min: i64,
    max: i64,
    env_override: Option<&'static str>,
    example: Option<&'static str>,
) -> SettingsFieldSpec {
    let mut field = number_editable(
        section,
        key,
        label,
        description,
        apply_mode,
        min,
        max,
        example,
    );
    field.env_override = env_override;
    field
}

pub fn schema_response() -> SettingsSchemaResponse {
    SettingsSchemaResponse {
        schema_version: SETTINGS_SCHEMA_VERSION,
        sections: vec![
            SettingsSectionSpec {
                id: "core",
                label: "Core",
                description: "Env-backed process defaults and low-risk operator paths.",
                advanced: false,
            },
            SettingsSectionSpec {
                id: "surfaces",
                label: "Surfaces",
                description: "Safe scalar HTTP, MCP, URL, and CORS settings.",
                advanced: false,
            },
            SettingsSectionSpec {
                id: "features",
                label: "Features",
                description: "Runtime feature gates with explicit apply semantics.",
                advanced: false,
            },
            SettingsSectionSpec {
                id: "services",
                label: "Services",
                description: "Service env vars and service preferences.",
                advanced: false,
            },
            SettingsSectionSpec {
                id: "advanced",
                label: "Advanced",
                description: "Redacted read-only complex config and env inventory.",
                advanced: true,
            },
        ],
        fields: settings_fields(),
    }
}

pub fn settings_fields() -> Vec<SettingsFieldSpec> {
    let mut fields = vec![
        editable(
            "core",
            "LAB_MCP_HTTP_HOST",
            "Bind host",
            "Environment override for HTTP MCP bind host.",
            SettingsBackend::Env,
            SettingsControl::Text,
            SettingsApplyMode::Restart,
            None,
            Some("127.0.0.1"),
        ),
        editable(
            "core",
            "LAB_MCP_HTTP_PORT",
            "Bind port",
            "Environment override for HTTP MCP bind port.",
            SettingsBackend::Env,
            SettingsControl::Number,
            SettingsApplyMode::Restart,
            None,
            Some("8765"),
        ),
        editable(
            "core",
            "LAB_LOG",
            "Log filter",
            "Tracing filter directive.",
            SettingsBackend::Env,
            SettingsControl::Text,
            SettingsApplyMode::Restart,
            None,
            Some("labby=info,labby_apis=warn"),
        ),
        enum_env(
            "core",
            "LAB_LOG_FORMAT",
            "Log format",
            "Set json for structured logs.",
            vec![
                SettingsOption {
                    value: "text",
                    label: "Text",
                },
                SettingsOption {
                    value: "json",
                    label: "JSON",
                },
            ],
            Some("json"),
        ),
        editable(
            "surfaces",
            "LAB_PUBLIC_URL",
            "Public app URL env",
            "Environment override for the public Lab UI and OAuth issuer URL.",
            SettingsBackend::Env,
            SettingsControl::Url,
            SettingsApplyMode::Restart,
            None,
            Some("https://lab.example.com"),
        ),
        editable(
            "surfaces",
            "LAB_MCP_GATEWAY_URL",
            "Public MCP gateway URL env",
            "Environment override for the public MCP gateway base URL.",
            SettingsBackend::Env,
            SettingsControl::Url,
            SettingsApplyMode::Restart,
            None,
            Some("https://mcp.example.com"),
        ),
        editable(
            "core",
            "log.filter",
            "Log filter default",
            "config.toml tracing filter directive; LAB_LOG overrides it.",
            SettingsBackend::ConfigToml,
            SettingsControl::Text,
            SettingsApplyMode::Restart,
            Some("LAB_LOG"),
            Some("labby=info,labby_apis=warn"),
        ),
        enum_editable_with_env(
            "core",
            "log.format",
            "Log format default",
            "config.toml log format; LAB_LOG_FORMAT overrides it.",
            SettingsApplyMode::Restart,
            vec![
                SettingsOption {
                    value: "text",
                    label: "Text",
                },
                SettingsOption {
                    value: "json",
                    label: "JSON",
                },
            ],
            Some("LAB_LOG_FORMAT"),
            Some("text"),
        ),
        editable(
            "core",
            "output.format",
            "CLI output format",
            "Default CLI output format when --json is not supplied.",
            SettingsBackend::ConfigToml,
            SettingsControl::Text,
            SettingsApplyMode::Restart,
            None,
            Some("human"),
        ),
        editable(
            "core",
            "workspace.root",
            "Workspace root",
            "Root directory used by fs browsing and stash workspaces.",
            SettingsBackend::ConfigToml,
            SettingsControl::Text,
            SettingsApplyMode::Restart,
            None,
            Some("~/.lab/stash"),
        ),
        editable(
            "core",
            "mcpregistry.url",
            "MCP Registry URL",
            "Upstream MCP Registry base URL.",
            SettingsBackend::ConfigToml,
            SettingsControl::Url,
            SettingsApplyMode::Restart,
            None,
            Some("https://registry.modelcontextprotocol.io"),
        ),
        enum_editable_with_env(
            "surfaces",
            "mcp.transport",
            "MCP transport",
            "Default MCP transport; LAB_MCP_TRANSPORT overrides it.",
            SettingsApplyMode::Restart,
            vec![
                SettingsOption {
                    value: "http",
                    label: "HTTP",
                },
                SettingsOption {
                    value: "stdio",
                    label: "stdio",
                },
            ],
            Some("LAB_MCP_TRANSPORT"),
            Some("http"),
        ),
        editable(
            "surfaces",
            "mcp.host",
            "MCP HTTP host",
            "TOML default for HTTP MCP host; LAB_MCP_HTTP_HOST overrides it.",
            SettingsBackend::ConfigToml,
            SettingsControl::Text,
            SettingsApplyMode::Restart,
            Some("LAB_MCP_HTTP_HOST"),
            Some("127.0.0.1"),
        ),
        number_editable_with_env(
            "surfaces",
            "mcp.port",
            "MCP HTTP port",
            "TOML default for HTTP MCP port; LAB_MCP_HTTP_PORT overrides it.",
            SettingsApplyMode::Restart,
            1,
            65535,
            Some("LAB_MCP_HTTP_PORT"),
            Some("8765"),
        ),
        number_editable_with_env(
            "surfaces",
            "mcp.session_ttl_secs",
            "MCP session TTL",
            "Default session keep-alive TTL in seconds; LAB_MCP_SESSION_TTL_SECS overrides it.",
            SettingsApplyMode::Restart,
            1,
            86_400,
            Some("LAB_MCP_SESSION_TTL_SECS"),
            Some("3600"),
        ),
        editable(
            "surfaces",
            "mcp.stateful",
            "Stateful MCP sessions",
            "Whether HTTP MCP uses stateful sessions by default; LAB_MCP_STATEFUL overrides it.",
            SettingsBackend::ConfigToml,
            SettingsControl::Bool,
            SettingsApplyMode::Restart,
            Some("LAB_MCP_STATEFUL"),
            Some("true"),
        ),
        editable(
            "surfaces",
            "mcp.allowed_hosts",
            "Allowed hosts",
            "Additional DNS rebinding allowed hosts; LAB_MCP_ALLOWED_HOSTS overrides it.",
            SettingsBackend::ConfigToml,
            SettingsControl::StringList,
            SettingsApplyMode::Restart,
            Some("LAB_MCP_ALLOWED_HOSTS"),
            Some("lab.tootie.tv"),
        ),
        editable(
            "surfaces",
            "api.cors_origins",
            "CORS origins",
            "Additional CORS origins. Loopback origins are always included; LAB_CORS_ORIGINS overrides this list.",
            SettingsBackend::ConfigToml,
            SettingsControl::StringList,
            SettingsApplyMode::Restart,
            Some("LAB_CORS_ORIGINS"),
            Some("https://lab.example.com"),
        ),
        editable(
            "surfaces",
            "web.assets_dir",
            "Web assets directory",
            "Path to exported Labby assets served by labby serve.",
            SettingsBackend::ConfigToml,
            SettingsControl::Text,
            SettingsApplyMode::Restart,
            None,
            Some("apps/gateway-admin/out"),
        ),
        editable(
            "surfaces",
            "public_urls.app",
            "Public app URL",
            "Public Lab UI and OAuth issuer URL.",
            SettingsBackend::ConfigToml,
            SettingsControl::Url,
            SettingsApplyMode::Restart,
            Some("LAB_PUBLIC_URL"),
            Some("https://lab.example.com"),
        ),
        editable(
            "surfaces",
            "public_urls.mcp_gateway",
            "Public MCP gateway URL",
            "Separate public MCP gateway base URL.",
            SettingsBackend::ConfigToml,
            SettingsControl::Url,
            SettingsApplyMode::Restart,
            Some("LAB_MCP_GATEWAY_URL"),
            Some("https://mcp.example.com"),
        ),
        editable(
            "features",
            "services.built_in_upstream_apis_enabled",
            "Built-in upstream API services",
            "Enable bundled external API integrations while keeping bootstrap tools online.",
            SettingsBackend::ConfigToml,
            SettingsControl::Bool,
            SettingsApplyMode::Immediate,
            None,
            Some("true"),
        ),
        editable(
            "features",
            "code_mode.trace_params",
            "Trace Code Mode params",
            "Include redacted/capped tool params in Code Mode traces.",
            SettingsBackend::ConfigToml,
            SettingsControl::Bool,
            SettingsApplyMode::Partial,
            None,
            Some("false"),
        ),
        editable(
            "services",
            "services.tailscale.tailnet",
            "Tailscale tailnet",
            "Tailnet name. TAILSCALE_TAILNET overrides this.",
            SettingsBackend::ConfigToml,
            SettingsControl::Text,
            SettingsApplyMode::Restart,
            Some("TAILSCALE_TAILNET"),
            Some("-"),
        ),
        number_editable(
            "advanced",
            "upstream_request_timeout_ms",
            "Upstream request timeout",
            "Maximum time for one proxied upstream MCP response.",
            SettingsApplyMode::Restart,
            1,
            300_000,
            Some("30000"),
        ),
        number_editable(
            "advanced",
            "upstream_relay_timeout_ms",
            "Upstream relay (elicitation) timeout",
            "Maximum time for one relayed upstream call that waits on a human \
             answering an elicitation. Only used on the opt-in relay path.",
            SettingsApplyMode::Restart,
            1,
            1_800_000,
            Some("300000"),
        ),
        number_editable(
            "advanced",
            "local_logs.retention_days",
            "Log retention days",
            "Local log retention window.",
            SettingsApplyMode::Partial,
            1,
            3650,
            Some("30"),
        ),
        number_editable(
            "advanced",
            "local_logs.max_bytes",
            "Max log bytes",
            "Maximum retained logical bytes.",
            SettingsApplyMode::Partial,
            1,
            1_099_511_627_776,
            Some("1073741824"),
        ),
        number_editable(
            "advanced",
            "local_logs.queue_capacity",
            "Log queue capacity",
            "Bounded ingest queue size.",
            SettingsApplyMode::Restart,
            1,
            1_000_000,
            Some("4096"),
        ),
        number_editable(
            "advanced",
            "local_logs.subscriber_capacity",
            "Subscriber capacity",
            "Bounded live-subscriber ring size.",
            SettingsApplyMode::Restart,
            1,
            1_000_000,
            Some("1024"),
        ),
        editable(
            "advanced",
            "node.controller",
            "Node controller",
            "Controller host for node runtime.",
            SettingsBackend::ConfigToml,
            SettingsControl::Text,
            SettingsApplyMode::Restart,
            None,
            Some("dookie"),
        ),
        number_editable(
            "advanced",
            "node.log_retention_days",
            "Node log retention days",
            "How many days of node logs to retain.",
            SettingsApplyMode::Partial,
            1,
            3650,
            Some("30"),
        ),
        enum_editable(
            "advanced",
            "node.role",
            "Node role",
            "Explicit runtime role for this device.",
            SettingsApplyMode::Restart,
            vec![
                SettingsOption {
                    value: "controller",
                    label: "Controller",
                },
                SettingsOption {
                    value: "node",
                    label: "Node",
                },
            ],
            Some("controller"),
        ),
        editable(
            "advanced",
            "device.master",
            "Legacy device master",
            "Legacy master host for device runtime.",
            SettingsBackend::ConfigToml,
            SettingsControl::Text,
            SettingsApplyMode::Restart,
            None,
            Some("dookie"),
        ),
        number_editable(
            "advanced",
            "code_mode.timeout_ms",
            "Code Mode timeout",
            "Maximum wall-clock time for one Code Mode execution.",
            SettingsApplyMode::Partial,
            1,
            60_000,
            Some("30000"),
        ),
        number_editable(
            "advanced",
            "code_mode.max_response_bytes",
            "Code Mode max response bytes",
            "Maximum serialized response envelope size.",
            SettingsApplyMode::Partial,
            1024,
            1_048_576,
            Some("1048576"),
        ),
        number_editable(
            "advanced",
            "code_mode.max_response_tokens",
            "Code Mode max response tokens",
            "Approximate maximum response tokens.",
            SettingsApplyMode::Partial,
            256,
            256_000,
            Some("64000"),
        ),
        number_editable(
            "advanced",
            "code_mode.token_estimate_divisor",
            "Token estimate divisor",
            "Lower values are more conservative.",
            SettingsApplyMode::Partial,
            1,
            64,
            Some("4"),
        ),
        number_editable(
            "advanced",
            "code_mode.max_log_entries",
            "Code Mode max log entries",
            "Maximum console log lines captured per execution.",
            SettingsApplyMode::Partial,
            1,
            100_000,
            Some("1000"),
        ),
        number_editable(
            "advanced",
            "code_mode.max_log_bytes",
            "Code Mode max log bytes",
            "Maximum console log bytes captured per execution.",
            SettingsApplyMode::Partial,
            1,
            104_857_600,
            Some("1048576"),
        ),
    ];
    fields.extend(readonly_fields());
    fields
}

fn enum_env(
    section: &'static str,
    key: &'static str,
    label: &'static str,
    description: &'static str,
    options: Vec<SettingsOption>,
    example: Option<&'static str>,
) -> SettingsFieldSpec {
    let mut field = editable(
        section,
        key,
        label,
        description,
        SettingsBackend::Env,
        SettingsControl::Enum,
        SettingsApplyMode::Restart,
        None,
        example,
    );
    field.options = options;
    field
}

fn readonly_fields() -> Vec<SettingsFieldSpec> {
    vec![
        readonly(
            "surfaces",
            "web.disable_auth",
            "Disable web auth",
            "Auth bypass is visible here but requires a dedicated dangerous settings flow.",
            SettingsRisk::Dangerous,
            SettingsWritePolicy::DangerousFlowRequired,
        ),
        readonly(
            "surfaces",
            "auth",
            "Auth config",
            "OAuth and bearer auth settings are redacted and read-only in this epic.",
            SettingsRisk::SecuritySensitive,
            SettingsWritePolicy::SecretWriteOnlyFuture,
        ),
        readonly(
            "features",
            "gateway_import_mode",
            "Gateway import mode",
            "External MCP config discovery can expose new upstreams and requires a dedicated dangerous settings flow.",
            SettingsRisk::Dangerous,
            SettingsWritePolicy::DangerousFlowRequired,
        ),
        readonly(
            "features",
            "admin.enabled",
            "Admin tool enabled",
            "Enabling the lab_admin MCP tool requires a dedicated dangerous settings flow.",
            SettingsRisk::Dangerous,
            SettingsWritePolicy::DangerousFlowRequired,
        ),
        readonly(
            "features",
            "gateway.extra_stdio_commands",
            "Extra stdio commands",
            "Additional stdio upstream commands require a dedicated dangerous settings flow.",
            SettingsRisk::Dangerous,
            SettingsWritePolicy::DangerousFlowRequired,
        ),
        readonly(
            "features",
            "code_mode.enabled",
            "Code Mode enabled",
            "Enabling the synthetic Code Mode surface requires dedicated runtime exposure tests.",
            SettingsRisk::SecuritySensitive,
            SettingsWritePolicy::DangerousFlowRequired,
        ),
        readonly(
            "features",
            "gateway.disable_spawn_guard",
            "Disable spawn guard",
            "Disabling stdio command validation requires typed confirmation and rollback instructions.",
            SettingsRisk::Dangerous,
            SettingsWritePolicy::DangerousFlowRequired,
        ),
        readonly(
            "advanced",
            "oauth.machines",
            "OAuth relay machines",
            "Named OAuth callback relay targets.",
            SettingsRisk::SecuritySensitive,
            SettingsWritePolicy::ReadOnly,
        ),
        readonly(
            "advanced",
            "deploy",
            "Deploy preferences",
            "Deploy defaults and per-host overrides.",
            SettingsRisk::SecuritySensitive,
            SettingsWritePolicy::ReadOnly,
        ),
        readonly(
            "advanced",
            "upstream",
            "Gateway upstreams",
            "Upstream MCP servers proxied through Lab.",
            SettingsRisk::SecuritySensitive,
            SettingsWritePolicy::ReadOnly,
        ),
        readonly(
            "advanced",
            "upstream_pending",
            "Pending upstream imports",
            "Discovered upstreams waiting for approval.",
            SettingsRisk::SecuritySensitive,
            SettingsWritePolicy::ReadOnly,
        ),
        readonly(
            "advanced",
            "upstream_import_tombstones",
            "Import tombstones",
            "Deleted imports that should not return automatically.",
            SettingsRisk::Restart,
            SettingsWritePolicy::ReadOnly,
        ),
        readonly(
            "advanced",
            "protected_mcp_routes",
            "Protected MCP routes",
            "OAuth-protected public MCP route definitions.",
            SettingsRisk::Dangerous,
            SettingsWritePolicy::ReadOnly,
        ),
        readonly(
            "advanced",
            "virtual_servers",
            "Virtual servers",
            "Virtual MCP servers backed by Lab services.",
            SettingsRisk::Restart,
            SettingsWritePolicy::ReadOnly,
        ),
        readonly(
            "advanced",
            "quarantined_virtual_servers",
            "Quarantined virtual servers",
            "Virtual servers whose backing service is no longer registered.",
            SettingsRisk::Restart,
            SettingsWritePolicy::ReadOnly,
        ),
    ]
}

pub fn state_response(
    cfg: &crate::config::LabConfig,
    config_path: String,
    env_path: String,
    section: &str,
) -> SettingsStateResponse {
    let explicit_config_paths = explicit_config_paths(&config_path);
    let env_path_ref = std::path::Path::new(&env_path);
    let mut values = BTreeMap::new();
    let mut sources = BTreeMap::new();
    for field in settings_fields()
        .into_iter()
        .filter(|field| field.section == section)
    {
        let (value, source) = value_for_field(cfg, &field, &explicit_config_paths, env_path_ref);
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

fn value_for_field(
    cfg: &crate::config::LabConfig,
    field: &SettingsFieldSpec,
    explicit_config_paths: &BTreeSet<String>,
    env_path: &std::path::Path,
) -> (Value, SettingsValueSource) {
    if field.backend == SettingsBackend::Env {
        let value = env_current_value(env_path, field).unwrap_or(Value::Null);
        return (
            value.clone(),
            SettingsValueSource {
                source: if value.is_null() {
                    SettingsSourceKind::Default
                } else {
                    SettingsSourceKind::Env
                },
                overridden_by_env: None,
            },
        );
    }
    let override_source = env_override_source(env_path, field);
    let mut value = override_source.as_ref().map_or_else(
        || crate::config::config_json_value_for_path(cfg, field.key),
        |(_, value)| env_override_value(field, value),
    );
    if field.control == SettingsControl::ReadOnly {
        value = redact_value(value);
        value = cap_readonly_value(value, 0);
    }
    let source = if let Some((name, _)) = override_source.clone() {
        SettingsValueSource {
            source: SettingsSourceKind::Env,
            overridden_by_env: Some(name.to_string()),
        }
    } else if !explicit_config_paths.contains(field.key) {
        SettingsValueSource {
            source: SettingsSourceKind::Default,
            overridden_by_env: None,
        }
    } else {
        SettingsValueSource {
            source: SettingsSourceKind::ConfigToml,
            overridden_by_env: None,
        }
    };
    (value, source)
}

fn explicit_config_paths(config_path: &str) -> BTreeSet<String> {
    let Ok(raw) = std::fs::read_to_string(config_path) else {
        return BTreeSet::new();
    };
    let Ok(document) = raw.parse::<toml_edit::DocumentMut>() else {
        return BTreeSet::new();
    };
    let mut paths = BTreeSet::new();
    collect_toml_paths(document.as_item(), "", &mut paths);
    paths
}

fn collect_toml_paths(item: &toml_edit::Item, prefix: &str, paths: &mut BTreeSet<String>) {
    if let Some(table) = item.as_table() {
        for (key, value) in table {
            let next = if prefix.is_empty() {
                key.to_string()
            } else {
                format!("{prefix}.{key}")
            };
            paths.insert(next.clone());
            collect_toml_paths(value, &next, paths);
        }
    } else if let Some(inline) = item.as_value().and_then(toml_edit::Value::as_inline_table) {
        for (key, value) in inline {
            let next = if prefix.is_empty() {
                key.to_string()
            } else {
                format!("{prefix}.{key}")
            };
            paths.insert(next.clone());
            collect_toml_value_paths(value, &next, paths);
        }
    }
}

fn collect_toml_value_paths(value: &toml_edit::Value, prefix: &str, paths: &mut BTreeSet<String>) {
    if let Some(inline) = value.as_inline_table() {
        for (key, child) in inline {
            let next = format!("{prefix}.{key}");
            paths.insert(next.clone());
            collect_toml_value_paths(child, &next, paths);
        }
    }
}

fn env_process_value(field: &SettingsFieldSpec) -> Value {
    match crate::dispatch::helpers::env_non_empty(field.key) {
        Some(value) if field.control == SettingsControl::Number => value
            .parse::<i64>()
            .map_or_else(|_| json!(value), |parsed| json!(parsed)),
        Some(value) => json!(value),
        None => Value::Null,
    }
}

fn env_current_value(path: &std::path::Path, field: &SettingsFieldSpec) -> Option<Value> {
    env_file_value(path, field).or_else(|| {
        let value = env_process_value(field);
        (!value.is_null()).then_some(value)
    })
}

fn env_override_source(
    path: &std::path::Path,
    field: &SettingsFieldSpec,
) -> Option<(&'static str, String)> {
    let name = field.env_override?;
    env_file_value_by_name(path, name)
        .or_else(|| crate::dispatch::helpers::env_non_empty(name))
        .map(|value| (name, value))
}

fn env_override_value(field: &SettingsFieldSpec, value: &str) -> Value {
    match field.control {
        SettingsControl::Number => value
            .parse::<i64>()
            .map_or_else(|_| json!(value), |parsed| json!(parsed)),
        SettingsControl::StringList => Value::Array(
            value
                .split(',')
                .map(str::trim)
                .filter(|entry| !entry.is_empty())
                .map(|entry| json!(entry))
                .collect(),
        ),
        SettingsControl::Bool => value
            .parse::<bool>()
            .map_or_else(|_| json!(value), |parsed| json!(parsed)),
        _ => json!(value),
    }
}

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

fn cap_readonly_value(value: Value, depth: usize) -> Value {
    if depth >= READONLY_MAX_DEPTH {
        return json!({
            "truncated": true,
            "reason": "max_depth",
        });
    }

    match value {
        Value::String(text) if text.len() > READONLY_MAX_STRING_BYTES => {
            let preview = truncate_utf8_preview(text, READONLY_MAX_STRING_BYTES);
            json!({
                "truncated": true,
                "kind": "string",
                "bytes": preview.len(),
                "preview": preview,
            })
        }
        Value::Array(values) => {
            let original_len = values.len();
            let preview: Vec<Value> = values
                .into_iter()
                .take(READONLY_MAX_ARRAY_ITEMS)
                .map(|value| cap_readonly_value(value, depth + 1))
                .collect();
            if original_len > READONLY_MAX_ARRAY_ITEMS {
                json!({
                    "truncated": true,
                    "kind": "array",
                    "total_items": original_len,
                    "preview": preview,
                })
            } else {
                Value::Array(preview)
            }
        }
        Value::Object(map) => {
            let original_len = map.len();
            let mut preview = Map::new();
            for (key, value) in map.into_iter().take(READONLY_MAX_OBJECT_KEYS) {
                preview.insert(key, cap_readonly_value(value, depth + 1));
            }
            if original_len > READONLY_MAX_OBJECT_KEYS {
                json!({
                    "truncated": true,
                    "kind": "object",
                    "total_keys": original_len,
                    "preview": Value::Object(preview),
                })
            } else {
                Value::Object(preview)
            }
        }
        other => other,
    }
}

fn truncate_utf8_preview(mut text: String, max_bytes: usize) -> String {
    if text.len() <= max_bytes {
        return text;
    }
    let end = text
        .char_indices()
        .map(|(index, _)| index)
        .take_while(|index| *index <= max_bytes)
        .last()
        .unwrap_or(0);
    text.truncate(end);
    text
}

pub fn config_patches_from_entries(
    entries: &[SettingsUpdateEntry],
) -> Result<Vec<crate::config::ConfigScalarPatch>, ToolError> {
    let fields = settings_fields_by_key();
    let mut patches = Vec::new();
    for entry in entries {
        let Some(field) = fields.get(entry.key.as_str()) else {
            return Err(ToolError::InvalidParam {
                message: format!("unknown setting `{}`", entry.key),
                param: entry.key.clone(),
            });
        };
        if field.backend != SettingsBackend::ConfigToml
            || field.write_policy != SettingsWritePolicy::Editable
        {
            return Err(ToolError::InvalidParam {
                message: format!(
                    "setting `{}` is not editable through settings.config.update",
                    entry.key
                ),
                param: entry.key.clone(),
            });
        }
        if field.secret {
            return Err(ToolError::InvalidParam {
                message: "secret config writes are not supported by this settings slice".into(),
                param: entry.key.clone(),
            });
        }
        if let Some((name, _)) = env_override_source(&super::client::env_path(), field) {
            return Err(ToolError::InvalidParam {
                message: format!(
                    "setting `{}` is overridden by env var `{name}` and cannot be edited here",
                    entry.key
                ),
                param: entry.key.clone(),
            });
        }
        require_previous(entry)?;
        patches.push(config_patch_for_field(field, entry)?);
    }
    Ok(patches)
}

pub fn expected_config_scalars(
    entries: &[SettingsUpdateEntry],
) -> Result<Vec<crate::config::ExpectedConfigScalar>, ToolError> {
    let fields = settings_fields_by_key();
    let mut expected = Vec::new();
    for entry in entries {
        let Some(field) = fields.get(entry.key.as_str()) else {
            continue;
        };
        require_previous(entry)?;
        expected.push(crate::config::ExpectedConfigScalar::new(
            field.key,
            entry.previous.clone(),
        ));
    }
    Ok(expected)
}

fn config_patch_for_field(
    field: &SettingsFieldSpec,
    entry: &SettingsUpdateEntry,
) -> Result<crate::config::ConfigScalarPatch, ToolError> {
    use crate::config::{ConfigScalarPatch, ConfigScalarValue};
    if entry.unset {
        return Ok(ConfigScalarPatch::new(
            field.key,
            ConfigScalarValue::UnsetOptional,
        ));
    }
    let value = match field.control {
        SettingsControl::Bool => ConfigScalarValue::Bool(
            entry
                .value
                .as_bool()
                .ok_or_else(|| invalid_field(field, "must be boolean"))?,
        ),
        SettingsControl::Number => {
            let raw = entry
                .value
                .as_i64()
                .ok_or_else(|| invalid_field(field, "must be an integer"))?;
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
            let raw = entry
                .value
                .as_str()
                .ok_or_else(|| invalid_field(field, "must be a string"))?
                .trim()
                .to_string();
            validate_string_field(field, &raw)?;
            ConfigScalarValue::String(raw)
        }
        SettingsControl::StringList => {
            let values = entry
                .value
                .as_array()
                .ok_or_else(|| invalid_field(field, "must be an array"))?
                .iter()
                .map(|value| {
                    value
                        .as_str()
                        .map(str::trim)
                        .filter(|s| !s.is_empty())
                        .map(str::to_string)
                })
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

pub fn env_entries_from_updates(
    entries: &[SettingsUpdateEntry],
) -> Result<Vec<labby_apis::setup::DraftEntry>, ToolError> {
    let fields = settings_fields_by_key();
    let mut out = Vec::new();
    for entry in entries {
        let Some(field) = fields.get(entry.key.as_str()) else {
            return Err(ToolError::InvalidParam {
                message: format!("unknown setting `{}`", entry.key),
                param: entry.key.clone(),
            });
        };
        if field.backend != SettingsBackend::Env
            || field.write_policy != SettingsWritePolicy::Editable
        {
            return Err(ToolError::InvalidParam {
                message: format!(
                    "setting `{}` is not editable through settings.env.update",
                    entry.key
                ),
                param: entry.key.clone(),
            });
        }
        require_previous(entry)?;
        let value = match field.control {
            SettingsControl::Number => entry
                .value
                .as_i64()
                .ok_or_else(|| invalid_field(field, "must be an integer"))?
                .to_string(),
            SettingsControl::Enum | SettingsControl::Text | SettingsControl::Url => {
                let raw = entry
                    .value
                    .as_str()
                    .ok_or_else(|| invalid_field(field, "must be a string"))?
                    .trim()
                    .to_string();
                validate_string_field(field, &raw)?;
                raw
            }
            _ => return Err(invalid_field(field, "has unsupported env control")),
        };
        out.push(labby_apis::setup::DraftEntry {
            key: entry.key.clone(),
            value,
        });
    }
    Ok(out)
}

pub fn validate_env_previous(
    entries: &[SettingsUpdateEntry],
    env_path: &std::path::Path,
) -> Result<(), ToolError> {
    let fields = settings_fields_by_key();
    for entry in entries {
        let Some(field) = fields.get(entry.key.as_str()) else {
            continue;
        };
        require_previous(entry)?;
        let current = env_current_value(env_path, field).unwrap_or(Value::Null);
        if current != entry.previous {
            return Err(ToolError::InvalidParam {
                message: format!("setting `{}` changed since it was loaded", entry.key),
                param: entry.key.clone(),
            });
        }
    }
    Ok(())
}

fn require_previous(entry: &SettingsUpdateEntry) -> Result<(), ToolError> {
    if entry.previous_present {
        return Ok(());
    }
    Err(ToolError::InvalidParam {
        message: format!(
            "setting `{}` requires previous for stale-write protection",
            entry.key
        ),
        param: entry.key.clone(),
    })
}

fn settings_fields_by_key() -> BTreeMap<&'static str, SettingsFieldSpec> {
    settings_fields()
        .into_iter()
        .map(|field| (field.key, field))
        .collect()
}

fn env_file_value(path: &std::path::Path, field: &SettingsFieldSpec) -> Option<Value> {
    let raw = env_file_value_by_name(path, field.key)?;
    if field.control == SettingsControl::Number {
        return raw
            .parse::<i64>()
            .map_or_else(|_| Some(json!(raw)), |parsed| Some(json!(parsed)));
    }
    Some(json!(raw))
}

fn env_file_value_by_name(path: &std::path::Path, name: &str) -> Option<String> {
    let iter = dotenvy::from_path_iter(path).ok()?;
    iter.filter_map(Result::ok)
        .find_map(|(key, value)| (key == name).then_some(value))
}

pub fn env_schema() -> Vec<EnvSettingSpec> {
    static ENV_SCHEMA: OnceLock<Vec<EnvSettingSpec>> = OnceLock::new();
    ENV_SCHEMA.get_or_init(build_env_schema).clone()
}

fn build_env_schema() -> Vec<EnvSettingSpec> {
    let mut by_key: BTreeMap<String, EnvSettingSpec> = BTreeMap::new();
    let generated: Value = serde_json::from_str(include_str!(
        "../../../../../docs/generated/env-reference.json"
    ))
    .unwrap_or_else(|_| Value::Array(Vec::new()));
    if let Value::Array(entries) = generated {
        for entry in entries {
            let Some(key) = entry.get("env_var").and_then(Value::as_str) else {
                continue;
            };
            by_key.insert(
                key.to_string(),
                EnvSettingSpec {
                    service: entry
                        .get("service")
                        .and_then(Value::as_str)
                        .unwrap_or("lab")
                        .to_string(),
                    key: key.to_string(),
                    required: entry
                        .get("required")
                        .and_then(Value::as_bool)
                        .unwrap_or(false),
                    secret: entry
                        .get("secret")
                        .and_then(Value::as_bool)
                        .unwrap_or(false),
                    description: entry
                        .get("description")
                        .and_then(Value::as_str)
                        .unwrap_or("")
                        .to_string(),
                    example: entry
                        .get("example")
                        .and_then(Value::as_str)
                        .unwrap_or("")
                        .to_string(),
                    editable: is_editable_core_env(key),
                },
            );
        }
    }
    for (key, description, example, secret) in [
        (
            "LAB_MCP_HTTP_HOST",
            "HTTP MCP bind host.",
            "127.0.0.1",
            false,
        ),
        ("LAB_MCP_HTTP_PORT", "HTTP MCP bind port.", "8765", false),
        (
            "LAB_LOG",
            "Tracing filter directive.",
            "labby=info,labby_apis=warn",
            false,
        ),
        ("LAB_LOG_FORMAT", "Log format.", "json", false),
        (
            "LAB_PUBLIC_URL",
            "Public Lab app URL.",
            "https://lab.example.com",
            false,
        ),
        (
            "LAB_MCP_GATEWAY_URL",
            "Public MCP gateway URL.",
            "https://mcp.example.com",
            false,
        ),
        (
            "LAB_MCP_HTTP_TOKEN",
            "Bearer token for the HTTP MCP/API surface.",
            "<token>",
            true,
        ),
    ] {
        by_key
            .entry(key.to_string())
            .or_insert_with(|| EnvSettingSpec {
                service: "lab".to_string(),
                key: key.to_string(),
                required: false,
                secret,
                description: description.to_string(),
                example: example.to_string(),
                editable: is_editable_core_env(key),
            });
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
                            existing.editable = is_editable_core_env(var.name);
                        })
                        .or_insert_with(|| EnvSettingSpec {
                            service: entry.name.to_string(),
                            key: var.name.to_string(),
                            required,
                            secret: var.secret,
                            description: var.description.to_string(),
                            example: var.example.to_string(),
                            editable: is_editable_core_env(var.name),
                        });
                }
            }
        }
    }
    by_key.into_values().collect()
}

fn is_editable_core_env(key: &str) -> bool {
    matches!(
        key,
        "LAB_MCP_HTTP_HOST"
            | "LAB_MCP_HTTP_PORT"
            | "LAB_LOG"
            | "LAB_LOG_FORMAT"
            | "LAB_PUBLIC_URL"
            | "LAB_MCP_GATEWAY_URL"
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::{BTreeSet, HashMap};

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
            assert_ne!(
                field.write_policy,
                SettingsWritePolicy::Editable,
                "{key} must not be scalar-editable"
            );
        }
    }

    #[test]
    fn env_override_metadata_is_present_for_shadowed_toml_fields() {
        let fields = settings_fields();
        for (key, env) in [
            ("log.filter", "LAB_LOG"),
            ("log.format", "LAB_LOG_FORMAT"),
            ("mcp.transport", "LAB_MCP_TRANSPORT"),
            ("mcp.host", "LAB_MCP_HTTP_HOST"),
            ("mcp.port", "LAB_MCP_HTTP_PORT"),
            ("mcp.session_ttl_secs", "LAB_MCP_SESSION_TTL_SECS"),
            ("mcp.stateful", "LAB_MCP_STATEFUL"),
            ("mcp.allowed_hosts", "LAB_MCP_ALLOWED_HOSTS"),
            ("api.cors_origins", "LAB_CORS_ORIGINS"),
            ("public_urls.app", "LAB_PUBLIC_URL"),
            ("public_urls.mcp_gateway", "LAB_MCP_GATEWAY_URL"),
        ] {
            assert_eq!(
                fields
                    .iter()
                    .find(|field| field.key == key)
                    .unwrap()
                    .env_override,
                Some(env),
                "{key} must advertise {env}"
            );
        }
    }

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
        let entries = vec![SettingsUpdateEntry {
            key: "auth".into(),
            value: json!("********"),
            previous: json!(null),
            unset: false,
            previous_present: true,
        }];
        let err = config_patches_from_entries(&entries).unwrap_err();
        assert_eq!(err.kind(), "invalid_param");
    }

    #[test]
    fn env_update_accepts_only_allowlisted_core_env_keys() {
        let entries = vec![SettingsUpdateEntry {
            key: "LAB_MCP_HTTP_PORT".into(),
            value: json!(8766),
            previous: json!(8765),
            unset: false,
            previous_present: true,
        }];
        let parsed = env_entries_from_updates(&entries).unwrap();
        assert_eq!(parsed[0].key, "LAB_MCP_HTTP_PORT");
        assert_eq!(parsed[0].value, "8766");

        let rejected = vec![SettingsUpdateEntry {
            key: "LAB_MCP_HTTP_TOKEN".into(),
            value: json!("secret"),
            previous: json!(null),
            unset: false,
            previous_present: true,
        }];
        assert!(env_entries_from_updates(&rejected).is_err());
    }

    #[test]
    fn env_previous_validation_accepts_matching_process_value_when_file_missing() {
        let temp = tempfile::tempdir().expect("tempdir");
        let env_path = temp.path().join(".env");
        let entries = vec![SettingsUpdateEntry {
            key: "LAB_LOG".into(),
            value: json!("labby=debug"),
            previous: json!("labby=info"),
            unset: false,
            previous_present: true,
        }];

        crate::dispatch::helpers::with_env_override(
            HashMap::from([("LAB_LOG".to_string(), "labby=info".to_string())]),
            || validate_env_previous(&entries, &env_path),
        )
        .expect("process value should satisfy previous");
    }

    #[test]
    fn config_update_requires_previous_for_stale_protection() {
        let entries = vec![SettingsUpdateEntry {
            key: "mcp.port".into(),
            value: json!(8766),
            previous: json!(null),
            unset: false,
            previous_present: false,
        }];
        let err = config_patches_from_entries(&entries).unwrap_err();
        assert_eq!(err.kind(), "invalid_param");
    }

    #[test]
    fn dangerous_exposure_fields_are_not_normal_editable_settings() {
        let fields = settings_fields();
        for key in [
            "gateway_import_mode",
            "admin.enabled",
            "gateway.extra_stdio_commands",
        ] {
            let field = fields.iter().find(|field| field.key == key).unwrap();
            assert_eq!(field.control, SettingsControl::ReadOnly);
            assert_eq!(
                field.write_policy,
                SettingsWritePolicy::DangerousFlowRequired
            );
        }
    }

    #[test]
    fn readonly_values_are_capped_for_advanced_state() {
        let value = json!((0..90).collect::<Vec<i32>>());
        let capped = cap_readonly_value(value, 0);
        assert_eq!(capped["truncated"], true);
        assert_eq!(capped["kind"], "array");
        assert_eq!(capped["total_items"], 90);
    }

    #[test]
    fn readonly_string_capping_is_utf8_boundary_safe() {
        let value = json!(format!("{}é", "a".repeat(READONLY_MAX_STRING_BYTES - 1)));
        let capped = cap_readonly_value(value, 0);
        assert_eq!(capped["truncated"], true);
        assert_eq!(capped["kind"], "string");
        assert_eq!(
            capped["preview"].as_str().unwrap().len(),
            READONLY_MAX_STRING_BYTES - 1
        );
    }

    #[test]
    fn env_override_values_are_coerced_to_field_control() {
        let field = settings_fields()
            .into_iter()
            .find(|field| field.key == "mcp.port")
            .unwrap();
        assert_eq!(env_override_value(&field, "8766"), json!(8766));
    }

    #[test]
    fn env_schema_merges_generated_reference_and_plugin_meta() {
        let specs = env_schema();
        for key in ["LAB_ACP_DB", "LAB_PUBLIC_URL", "LAB_MCP_HTTP_TOKEN"] {
            assert!(specs.iter().any(|spec| spec.key == key), "missing {key}");
        }
        let token = specs
            .iter()
            .find(|spec| spec.key == "LAB_MCP_HTTP_TOKEN")
            .unwrap();
        assert!(token.secret, "token must be secret");
    }

    #[test]
    fn env_schema_only_marks_low_risk_core_env_editable() {
        let specs = env_schema();
        for key in ["LAB_LOG", "LAB_PUBLIC_URL", "LAB_MCP_GATEWAY_URL"] {
            assert!(
                specs.iter().find(|spec| spec.key == key).unwrap().editable,
                "{key} should be editable"
            );
        }
        assert!(
            !specs
                .iter()
                .find(|spec| spec.key == "LAB_MCP_HTTP_TOKEN")
                .unwrap()
                .editable
        );
    }
}
