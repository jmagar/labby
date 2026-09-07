//! Resource handler bodies (`list_resources`, `read_resource`).
//!
//! Extracted from `server.rs` (bead `lab-kvji.24.1.3`) as inherent
//! `impl LabMcpServer` methods. The `ServerHandler` trait impl in
//! `server.rs` keeps one-line delegators.
//!
//! `read_resource_impl` keeps the prefix-dispatch skeleton + the local
//! `lab://catalog` / `lab://<svc>/actions` branch; the three proxy
//! branches live in `resource_proxy.rs` and are reached via the same
//! guard ordering as the original (gateway → upstream → subject-scoped).
//!
//! Skill resources are converted here into their exact MCP text/blob wire
//! representation after the shared registry and digest checks succeed.

use std::sync::Arc;
use std::time::Instant;
#[cfg(feature = "gateway")]
use std::time::SystemTime;

use rmcp::ErrorData;
use rmcp::RoleServer;
use rmcp::model::{
    ListResourceTemplatesResult, ListResourcesResult, MetaObject, PaginatedRequestParams,
    ReadResourceRequestParams, ReadResourceResponse, ReadResourceResult, Resource,
    ResourceContents,
};
use rmcp::service::RequestContext;
use serde_json::{Value, json};

#[cfg(feature = "gateway")]
use crate::mcp::resource_errors::fetch_classified as resource_fetch_classified;
use crate::mcp::resource_errors::{
    forbidden as forbidden_resource_error, render as resource_render_error,
    route_scope as route_scope_resource_error, unknown as unknown_resource_error,
};

#[cfg(feature = "gateway")]
pub(crate) use crate::app_assets::{
    ADD_SERVER_APP_SKYBRIDGE_URI, ADD_SERVER_APP_URI, GATEWAY_STATUS_APP_SKYBRIDGE_URI,
    GATEWAY_STATUS_APP_URI, MCP_APPS_APP_SKYBRIDGE_URI, MCP_APPS_APP_URI,
    SETTINGS_APP_SKYBRIDGE_URI, SETTINGS_APP_URI,
};
pub(crate) use crate::app_assets::{
    SERVER_LOGS_APP_SKYBRIDGE_URI, SERVER_LOGS_APP_URI, SERVER_LOGS_APP_URI_PREFIX,
};
#[cfg(feature = "skills")]
pub(crate) use crate::app_assets::{
    SKILL_LIBRARY_APP_SKYBRIDGE_URI, SKILL_LIBRARY_APP_URI, SKILL_LIBRARY_APP_URI_PREFIX,
};
#[cfg(feature = "gateway")]
use crate::mcp::bound_access::{
    ProjectDiscoveryShadow, ProjectExecutionBinding, project_discovery_shadow,
    project_execution_binding,
};
#[cfg(feature = "gateway")]
use crate::mcp::catalog::{
    ADD_SERVER_TOOL_NAME, GATEWAY_STATUS_TOOL_NAME, MCP_APP_TOOL_NAME, SETTINGS_TOOL_NAME,
};
use crate::mcp::catalog::{CODE_MODE_UI_TOOL_NAME, SERVER_LOGS_TOOL_NAME};
#[cfg(feature = "gateway")]
use crate::mcp::context::oauth_upstream_subject_for_request;
use crate::mcp::context::{
    auth_context_from_extensions, code_mode_read_scope_allowed, tool_execute_scope_allowed,
};
use crate::mcp::logging::{DispatchLogOutcome, LoggingLevel};
use crate::mcp::pagination::{
    CatalogSnapshotCollector, PageCollector, error_kind as pagination_error_kind, invalid_cursor,
    next_catalog_snapshot_revision,
};
use crate::mcp::runtime::{
    ResourceProvenance, ResourceTemplateProvenance, catalog_snapshot_audience,
};
use crate::mcp::server::LabMcpServer;

#[cfg(feature = "skills")]
fn skill_library_resource_error_correlation(context: &RequestContext<RoleServer>) -> String {
    static REQUESTS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
    let supplied = context
        .extensions
        .get::<axum::http::request::Parts>()
        .and_then(|parts| parts.headers.get("x-request-id"))
        .and_then(|value| value.to_str().ok());
    if let Some(value) = supplied
        && crate::dispatch::skill_library::audit::SkillLibraryCorrelationId::parse(value).is_ok()
    {
        return value.to_owned();
    }
    format!(
        "mcp-skill-resource-{}",
        REQUESTS.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    )
}

#[cfg(feature = "skills")]
fn with_skill_library_resource_correlation(
    mut error: ErrorData,
    context: &RequestContext<RoleServer>,
) -> ErrorData {
    let correlation_id = skill_library_resource_error_correlation(context);
    let mut data = error
        .data
        .take()
        .and_then(|value| value.as_object().cloned())
        .unwrap_or_default();
    data.insert("correlation_id".to_owned(), Value::String(correlation_id));
    error.data = Some(Value::Object(data));
    error
}

/// In-band error-contract discovery: the published JSON Schema for the
/// versioned agent-error contract every error envelope carries
/// (`docs/contracts/agent-error-contract.md`).
pub(crate) const AGENT_ERROR_CONTRACT_URI: &str = "lab://contracts/agent-error";
/// In-band error-contract discovery: the published JSON Schema for Code Mode
/// `callTool` rejection payloads (`docs/contracts/code-mode-tool-errors.md`).
pub(crate) const CODE_MODE_CALL_ERROR_CONTRACT_URI: &str = "lab://contracts/code-mode-call-error";
const AGENT_ERROR_CONTRACT_SCHEMA: &str =
    include_str!("../../../../docs/contracts/schemas/agent-error.schema.json");
const CODE_MODE_CALL_ERROR_CONTRACT_SCHEMA: &str =
    include_str!("../../../../docs/contracts/schemas/code-mode-call-error.schema.json");
const CONTRACT_SCHEMA_MIME: &str = "application/schema+json";

/// Parse only the canonical built-in action-resource URI family.
///
/// Parsing identifies provenance, not authority; callers must still apply the
/// bound Project route publication.
#[cfg(any(feature = "gateway", test))]
pub(crate) fn builtin_action_resource_service(uri: &str) -> Option<&str> {
    let service = uri.strip_prefix("lab://")?.strip_suffix("/actions")?;
    (!service.is_empty() && !service.contains('/')).then_some(service)
}

#[cfg(feature = "gateway")]
fn classify_builtin_action_resources<'a>(
    shadow: &ProjectDiscoveryShadow<'_>,
    resources: impl IntoIterator<Item = &'a Resource>,
) -> (usize, usize) {
    classify_builtin_action_resources_with(resources, |service| {
        shadow.allows_builtin_action_resource(service, SystemTime::now())
    })
}

#[cfg(any(feature = "gateway", test))]
fn classify_builtin_action_resources_with<'a>(
    resources: impl IntoIterator<Item = &'a Resource>,
    mut allows: impl FnMut(&str) -> Option<bool>,
) -> (usize, usize) {
    let mut checked = 0usize;
    let mut would_suppress = 0usize;
    for resource in resources {
        let Some(service) = builtin_action_resource_service(&resource.uri) else {
            continue;
        };
        if let Some(allowed) = allows(service) {
            checked += 1;
            would_suppress += usize::from(!allowed);
        }
    }
    (checked, would_suppress)
}

#[cfg(feature = "gateway")]
fn classify_regular_upstream_resources(
    shadow: &ProjectDiscoveryShadow<'_>,
    provenance: &[ResourceProvenance],
) -> (usize, usize) {
    classify_regular_upstream_resources_with(provenance, |upstream, native_uri| {
        shadow.allows_upstream_resource(upstream, native_uri, SystemTime::now())
    })
}

#[cfg(any(feature = "gateway", test))]
fn classify_regular_upstream_resources_with(
    provenance: &[ResourceProvenance],
    mut allows: impl FnMut(&str, &str) -> Option<bool>,
) -> (usize, usize) {
    let mut checked = 0;
    let mut would_suppress = 0;
    for candidate in provenance {
        if let Some(allowed) = allows(&candidate.upstream, &candidate.native_uri) {
            checked += 1;
            would_suppress += usize::from(!allowed);
        }
    }
    (checked, would_suppress)
}

#[cfg(feature = "gateway")]
fn classify_regular_upstream_resource_templates(
    shadow: &ProjectDiscoveryShadow<'_>,
    provenance: &[ResourceTemplateProvenance],
) -> (usize, usize) {
    let now = SystemTime::now();
    classify_regular_upstream_resource_templates_with(provenance, |upstream, template| {
        shadow.allows_upstream_resource_template(upstream, template, now)
    })
}

#[cfg(any(feature = "gateway", test))]
fn classify_regular_upstream_resource_templates_with(
    provenance: &[ResourceTemplateProvenance],
    mut allows: impl FnMut(&str, &str) -> Option<bool>,
) -> (usize, usize) {
    provenance.iter().fold((0, 0), |(checked, denied), row| {
        if is_ui_resource_uri(&row.native_uri_template) {
            return (checked, denied);
        }
        match allows(&row.upstream, &row.native_uri_template) {
            Some(allowed) => (checked + 1, denied + usize::from(!allowed)),
            None => (checked, denied),
        }
    })
}

#[cfg(any(feature = "gateway", test))]
fn is_ui_resource_uri(uri: &str) -> bool {
    uri.get(..5)
        .is_some_and(|prefix| prefix.eq_ignore_ascii_case("ui://"))
}
/// In-band discovery for the Skills extension (SEP-2640).
///
/// Published so a client that does not speak the extension can still discover
/// that skills exist, what the URI grammar is, and which methods to call — via
/// `resources/list`, which every client already reads. This is protocol
/// metadata, not skill content, and lists no `skill://` URI.
#[cfg(feature = "skills")]
pub(crate) const SKILLS_EXTENSION_CONTRACT_URI: &str = "lab://contracts/skills-extension";
#[cfg(feature = "skills")]
const SKILLS_EXTENSION_CONTRACT: &str =
    include_str!("../../../../docs/contracts/skills-extension.md");

/// MCP Apps (Claude / SEP-1724) MIME — bound via the tool's `_meta.ui.resourceUri`.
pub(crate) const CODE_MODE_APP_MIME: &str = "text/html;profile=mcp-app";
/// OpenAI Apps (ChatGPT / Codex) MIME — bound via the tool's `openai/outputTemplate`.
/// Same HTML body; a distinct URI + MIME so the Claude resource stays untouched.
pub(crate) const CODE_MODE_APP_SKYBRIDGE_MIME: &str = "text/html+skybridge";
/// URI namespace reserved for Lab's own Code Mode app resources, served locally.
/// Any other `ui://` is an upstream mcp-ui widget resource routed to its peer.
pub(crate) const CODE_MODE_APP_URI_PREFIX: &str = "ui://lab/code-mode/";
pub(crate) const CODE_MODE_APP_URI: &str = "ui://lab/code-mode/codemode";
pub(crate) const CODE_MODE_HISTORY_APP_URI: &str = "ui://lab/code-mode/history";
/// OpenAI Apps skybridge variants — same HTML, served under the skybridge MIME.
pub(crate) const CODE_MODE_APP_SKYBRIDGE_URI: &str = "ui://lab/code-mode/codemode.skybridge";
/// Host runtime a Code Mode widget resource targets. The runtime is the single
/// discriminant: it derives the served MIME, whether the resource is listed, and
/// which tool `_meta` key the resource URI is exposed under — so those
/// projections can't drift apart.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum CodeModeRuntime {
    /// Anthropic MCP Apps (Claude): `text/html;profile=mcp-app`, listed in
    /// `resources/list`, bound via the tool's `_meta.ui.resourceUri`.
    McpApp,
    /// OpenAI Apps (ChatGPT / Codex): `text/html+skybridge`, unlisted — reached
    /// directly via the tool's `openai/outputTemplate`.
    Skybridge,
}

impl CodeModeRuntime {
    const fn mime(self) -> &'static str {
        match self {
            Self::McpApp => CODE_MODE_APP_MIME,
            Self::Skybridge => CODE_MODE_APP_SKYBRIDGE_MIME,
        }
    }

    /// Only MCP Apps resources appear in `resources/list`; skybridge variants are
    /// discovered via the tool's `openai/outputTemplate`, keeping the Claude
    /// surface unchanged.
    const fn listed(self) -> bool {
        matches!(self, Self::McpApp)
    }
}

pub(crate) struct AppResourceDescriptor {
    pub(crate) uri: &'static str,
    pub(crate) name: &'static str,
    pub(crate) runtime: CodeModeRuntime,
    /// Tool this widget binds to, or `None` for the history widget (not tool-
    /// bound). `runtime` selects which `_meta` key the URI is exposed under.
    pub(crate) tool_name: Option<&'static str>,
    pub(crate) resource_description: &'static str,
    pub(crate) skybridge_widget_description: Option<&'static str>,
}

pub(crate) const CODE_MODE_APP_RESOURCE_DESCRIPTORS: &[AppResourceDescriptor] = &[
    AppResourceDescriptor {
        uri: CODE_MODE_APP_URI,
        name: "code-mode/codemode",
        runtime: CodeModeRuntime::McpApp,
        tool_name: Some(CODE_MODE_UI_TOOL_NAME),
        resource_description: "Read-only MCP App for Code Mode call traces",
        skybridge_widget_description: None,
    },
    AppResourceDescriptor {
        uri: CODE_MODE_HISTORY_APP_URI,
        name: "code-mode/history",
        runtime: CodeModeRuntime::McpApp,
        tool_name: None,
        resource_description: "Read-only MCP App for Code Mode call traces",
        skybridge_widget_description: None,
    },
    AppResourceDescriptor {
        uri: CODE_MODE_APP_SKYBRIDGE_URI,
        name: "code-mode/codemode.skybridge",
        runtime: CodeModeRuntime::Skybridge,
        tool_name: Some(CODE_MODE_UI_TOOL_NAME),
        resource_description: "Read-only MCP App for Code Mode call traces",
        skybridge_widget_description: Some(
            "Live Code Mode call trace — upstream tool calls, catalog search matches, and recent gateway history.",
        ),
    },
];

const CODE_MODE_APP_FALLBACK_HTML: &str = crate::app_assets::CODE_MODE_APP_HTML;
const SERVER_LOGS_APP_FALLBACK_HTML: &str = crate::app_assets::SERVER_LOGS_APP_HTML;
#[cfg(feature = "skills")]
const SKILL_LIBRARY_APP_FALLBACK_HTML: &str = crate::app_assets::SKILL_LIBRARY_APP_HTML;
#[cfg(feature = "gateway")]
const ADD_SERVER_APP_FALLBACK_HTML: &str = crate::app_assets::ADD_SERVER_APP_HTML;
#[cfg(feature = "gateway")]
const GATEWAY_STATUS_APP_FALLBACK_HTML: &str = crate::app_assets::GATEWAY_STATUS_APP_HTML;
#[cfg(feature = "gateway")]
const SETTINGS_APP_FALLBACK_HTML: &str = crate::app_assets::SETTINGS_APP_HTML;
#[cfg(feature = "gateway")]
const MCP_APPS_APP_FALLBACK_HTML: &str = crate::app_assets::MCP_APPS_APP_HTML;

pub(crate) const SERVER_LOGS_APP_RESOURCE_DESCRIPTORS: &[AppResourceDescriptor] = &[
    AppResourceDescriptor {
        uri: SERVER_LOGS_APP_URI,
        name: "server-logs/viewer",
        runtime: CodeModeRuntime::McpApp,
        tool_name: Some(SERVER_LOGS_TOOL_NAME),
        resource_description: "Admin MCP App for Labby server process logs",
        skybridge_widget_description: None,
    },
    AppResourceDescriptor {
        uri: SERVER_LOGS_APP_SKYBRIDGE_URI,
        name: "server-logs/viewer.skybridge",
        runtime: CodeModeRuntime::Skybridge,
        tool_name: Some(SERVER_LOGS_TOOL_NAME),
        resource_description: "Admin MCP App for Labby server process logs",
        skybridge_widget_description: Some(
            "Admin viewer for Labby's rolling server process logs with level, service, action, kind, and text filters.",
        ),
    },
];

#[cfg(feature = "skills")]
pub(crate) const SKILL_LIBRARY_APP_RESOURCE_DESCRIPTORS: &[AppResourceDescriptor] = &[
    AppResourceDescriptor {
        uri: SKILL_LIBRARY_APP_URI,
        name: "skill-library/app",
        runtime: CodeModeRuntime::McpApp,
        tool_name: Some("artifacts"),
        resource_description: "MCP App shell for the authenticated Labby Skill Library",
        skybridge_widget_description: None,
    },
    AppResourceDescriptor {
        uri: SKILL_LIBRARY_APP_SKYBRIDGE_URI,
        name: "skill-library/app.skybridge",
        runtime: CodeModeRuntime::Skybridge,
        tool_name: Some("artifacts"),
        resource_description: "MCP App shell for the authenticated Labby Skill Library",
        skybridge_widget_description: Some(
            "Manage personal and shared Labby Skills through authenticated host callbacks.",
        ),
    },
];

#[cfg(feature = "gateway")]
pub(crate) const ADD_SERVER_APP_RESOURCE_DESCRIPTORS: &[AppResourceDescriptor] = &[
    AppResourceDescriptor {
        uri: ADD_SERVER_APP_URI,
        name: "gateway/add-server",
        runtime: CodeModeRuntime::McpApp,
        tool_name: Some(ADD_SERVER_TOOL_NAME),
        resource_description: "Admin MCP App for adding an upstream server to Labby",
        skybridge_widget_description: None,
    },
    AppResourceDescriptor {
        uri: ADD_SERVER_APP_SKYBRIDGE_URI,
        name: "gateway/add-server.skybridge",
        runtime: CodeModeRuntime::Skybridge,
        tool_name: Some(ADD_SERVER_TOOL_NAME),
        resource_description: "Admin MCP App for adding an upstream server to Labby",
        skybridge_widget_description: Some(
            "Connect and test a remote or local MCP server, then add it to the Labby gateway catalog.",
        ),
    },
];

#[cfg(feature = "gateway")]
pub(crate) const GATEWAY_STATUS_APP_RESOURCE_DESCRIPTORS: &[AppResourceDescriptor] = &[
    AppResourceDescriptor {
        uri: GATEWAY_STATUS_APP_URI,
        name: "gateway/status",
        runtime: CodeModeRuntime::McpApp,
        tool_name: Some(GATEWAY_STATUS_TOOL_NAME),
        resource_description: "Admin MCP App for live gateway upstream status",
        skybridge_widget_description: None,
    },
    AppResourceDescriptor {
        uri: GATEWAY_STATUS_APP_SKYBRIDGE_URI,
        name: "gateway/status.skybridge",
        runtime: CodeModeRuntime::Skybridge,
        tool_name: Some(GATEWAY_STATUS_TOOL_NAME),
        resource_description: "Admin MCP App for live gateway upstream status",
        skybridge_widget_description: Some(
            "Live connection status, capabilities, and warnings for Labby gateway upstream MCP servers.",
        ),
    },
];

#[cfg(feature = "gateway")]
pub(crate) const SETTINGS_APP_RESOURCE_DESCRIPTORS: &[AppResourceDescriptor] = &[
    AppResourceDescriptor {
        uri: SETTINGS_APP_URI,
        name: "settings/editor",
        runtime: CodeModeRuntime::McpApp,
        tool_name: Some(SETTINGS_TOOL_NAME),
        resource_description: "Admin MCP App for schema-backed Labby runtime settings",
        skybridge_widget_description: None,
    },
    AppResourceDescriptor {
        uri: SETTINGS_APP_SKYBRIDGE_URI,
        name: "settings/editor.skybridge",
        runtime: CodeModeRuntime::Skybridge,
        tool_name: Some(SETTINGS_TOOL_NAME),
        resource_description: "Admin MCP App for schema-backed Labby runtime settings",
        skybridge_widget_description: Some(
            "Manage Code Mode, proxy, surface, feature, and runtime settings using Labby's schema-backed configuration controls.",
        ),
    },
];

#[cfg(feature = "gateway")]
pub(crate) const MCP_APPS_APP_RESOURCE_DESCRIPTORS: &[AppResourceDescriptor] = &[
    AppResourceDescriptor {
        uri: MCP_APPS_APP_URI,
        name: "apps/manage",
        runtime: CodeModeRuntime::McpApp,
        tool_name: Some(MCP_APP_TOOL_NAME),
        resource_description: "MCP App for managing Labby-owned app visibility",
        skybridge_widget_description: None,
    },
    AppResourceDescriptor {
        uri: MCP_APPS_APP_SKYBRIDGE_URI,
        name: "apps/manage.skybridge",
        runtime: CodeModeRuntime::Skybridge,
        tool_name: Some(MCP_APP_TOOL_NAME),
        resource_description: "MCP App for managing Labby-owned app visibility",
        skybridge_widget_description: Some(
            "Enable or disable Labby-owned MCP Apps and their UI resources without disabling the manager itself.",
        ),
    },
];

#[cfg(feature = "skills")]
use crate::app_catalog::SKILL_LIBRARY_APP_VERSION;
#[cfg(feature = "gateway")]
use crate::app_catalog::{
    ADD_SERVER_APP_VERSION, GATEWAY_STATUS_APP_VERSION, MCP_APPS_APP_VERSION, SETTINGS_APP_VERSION,
};
/// FNV-1a over the bundled widget HTML, evaluated at compile time. Changes iff
/// the HTML bytes change, so it is a stable per-build cache-bust key.
use crate::app_catalog::{CODE_MODE_APP_VERSION, SERVER_LOGS_APP_VERSION};
#[cfg(test)]
use crate::app_catalog::{bridged_app_content_version, fnv1a_64};

#[derive(Clone, Copy)]
struct OwnedAppRegistration {
    descriptors: &'static [AppResourceDescriptor],
    html: &'static str,
    version: &'static std::sync::LazyLock<String>,
}

impl OwnedAppRegistration {
    /// Resolve either a canonical or cache-busted URI to its descriptor.
    fn descriptor(self, uri: &str) -> Option<&'static AppResourceDescriptor> {
        app_descriptor_for_uri(self.descriptors, uri)
    }

    /// Add the registration's content-derived cache-bust token to a base URI.
    fn versioned_uri(self, base: &str) -> String {
        format!("{base}?v={}", self.version.as_str())
    }

    /// Build the MCP resource metadata for one registered runtime variant.
    fn resource(self, descriptor: &AppResourceDescriptor) -> Resource {
        let uri = self.versioned_uri(descriptor.uri);
        Resource::new(uri.clone(), descriptor.name.to_string())
            .with_description(descriptor.resource_description)
            .with_mime_type(descriptor.runtime.mime())
            .with_meta(app_resource_meta_for_descriptor(&uri, descriptor))
    }

    /// Build the resource-list entries hosts are expected to discover.
    fn listed_resources(self) -> Vec<Resource> {
        self.descriptors
            .iter()
            .filter(|descriptor| descriptor.runtime.listed())
            .map(|descriptor| self.resource(descriptor))
            .collect()
    }

    /// Find a tool-bound URI for a particular host runtime.
    fn uri_for_tool(self, runtime: CodeModeRuntime, tool_name: &str) -> Option<String> {
        self.descriptors
            .iter()
            .find(|descriptor| {
                descriptor.runtime == runtime && descriptor.tool_name == Some(tool_name)
            })
            .map(|descriptor| self.versioned_uri(descriptor.uri))
    }

    /// Inline the shared host bridge into this registration's fallback HTML.
    fn inline_html(self, descriptor: &AppResourceDescriptor) -> Result<String, String> {
        inline_app_host_script(self.html, descriptor)
    }
}

/// Return the shared Code Mode app registration.
fn code_mode_app() -> OwnedAppRegistration {
    OwnedAppRegistration {
        descriptors: CODE_MODE_APP_RESOURCE_DESCRIPTORS,
        html: CODE_MODE_APP_FALLBACK_HTML,
        version: &CODE_MODE_APP_VERSION,
    }
}

/// Return the Server Logs app registration.
fn server_logs_app() -> OwnedAppRegistration {
    OwnedAppRegistration {
        descriptors: SERVER_LOGS_APP_RESOURCE_DESCRIPTORS,
        html: SERVER_LOGS_APP_FALLBACK_HTML,
        version: &SERVER_LOGS_APP_VERSION,
    }
}

#[cfg(feature = "skills")]
fn skill_library_app() -> OwnedAppRegistration {
    OwnedAppRegistration {
        descriptors: SKILL_LIBRARY_APP_RESOURCE_DESCRIPTORS,
        html: SKILL_LIBRARY_APP_FALLBACK_HTML,
        version: &SKILL_LIBRARY_APP_VERSION,
    }
}

#[cfg(feature = "gateway")]
/// Return the gateway Add Server app registration.
fn add_server_app() -> OwnedAppRegistration {
    OwnedAppRegistration {
        descriptors: ADD_SERVER_APP_RESOURCE_DESCRIPTORS,
        html: ADD_SERVER_APP_FALLBACK_HTML,
        version: &ADD_SERVER_APP_VERSION,
    }
}

#[cfg(feature = "gateway")]
/// Return the gateway upstream status app registration.
fn gateway_status_app() -> OwnedAppRegistration {
    OwnedAppRegistration {
        descriptors: GATEWAY_STATUS_APP_RESOURCE_DESCRIPTORS,
        html: GATEWAY_STATUS_APP_FALLBACK_HTML,
        version: &GATEWAY_STATUS_APP_VERSION,
    }
}

#[cfg(feature = "gateway")]
fn settings_app() -> OwnedAppRegistration {
    OwnedAppRegistration {
        descriptors: SETTINGS_APP_RESOURCE_DESCRIPTORS,
        html: SETTINGS_APP_FALLBACK_HTML,
        version: &SETTINGS_APP_VERSION,
    }
}

#[cfg(feature = "gateway")]
/// Return the always-on Labby MCP App manager registration.
fn mcp_apps_app() -> OwnedAppRegistration {
    OwnedAppRegistration {
        descriptors: MCP_APPS_APP_RESOURCE_DESCRIPTORS,
        html: MCP_APPS_APP_FALLBACK_HTML,
        version: &MCP_APPS_APP_VERSION,
    }
}

/// Strip the `?v=<hash>` cache-bust suffix so a versioned URI matches its base
/// descriptor. A base URI (no query) is returned unchanged.
fn strip_app_version(uri: &str) -> &str {
    uri.split_once('?').map_or(uri, |(base, _)| base)
}

impl LabMcpServer {
    pub(crate) async fn list_resources_impl(
        &self,
        request: Option<PaginatedRequestParams>,
        context: RequestContext<RoleServer>,
    ) -> Result<ListResourcesResult, ErrorData> {
        let start = Instant::now();
        let subject = self.request_subject_log_tag(&context);
        tracing::info!(
            surface = "mcp",
            service = "labby",
            action = "list_resources",
            subject,
            "dispatch start"
        );
        let auth = auth_context_from_extensions(&context.extensions);
        #[cfg(feature = "gateway")]
        let project_shadow = project_discovery_shadow(&context.extensions, SystemTime::now());
        if !self.route_scope.exposes_resources() {
            let elapsed_ms = start.elapsed().as_millis();
            tracing::info!(
                surface = "mcp",
                service = "labby",
                action = "list_resources",
                subject,
                route_scope = %self.route_scope.label(),
                elapsed_ms,
                "resource catalog hidden by loadout"
            );
            self.emit_dispatch_notification(
                &context,
                "lab",
                "list_resources",
                elapsed_ms,
                DispatchLogOutcome::Success,
            )
            .await;
            return Ok(ListResourcesResult::with_all_items(Vec::new())
                .with_ttl_ms(0)
                .with_cache_scope(rmcp::model::CacheScope::Private));
        }
        #[cfg(feature = "gateway")]
        let (code_mode_app_enabled, mcp_apps_config) =
            crate::mcp::peer_contract::mcp_app_visibility_snapshot(
                self.gateway_manager.as_deref(),
                &self.code_mode_app_state,
            )
            .await;
        #[cfg(not(feature = "gateway"))]
        let code_mode_app_enabled = self.code_mode_app_state.is_enabled();
        #[cfg(feature = "gateway")]
        let server_logs_app_enabled = mcp_apps_config.server_logs;
        #[cfg(not(feature = "gateway"))]
        let server_logs_app_enabled = true;
        let mut page_collector = match PageCollector::new(request) {
            Ok(collector) => collector,
            Err(error) => {
                let elapsed_ms = start.elapsed().as_millis();
                let kind = pagination_error_kind(&error);
                tracing::warn!(
                    surface = "mcp",
                    service = "labby",
                    action = "list_resources",
                    subject,
                    elapsed_ms,
                    kind,
                    "resource list failed"
                );
                self.emit_dispatch_notification(
                    &context,
                    "lab",
                    "list_resources",
                    elapsed_ms,
                    DispatchLogOutcome::Failure {
                        level: LoggingLevel::Warning,
                        kind,
                    },
                )
                .await;
                return Err(error);
            }
        };
        let snapshot_audience = catalog_snapshot_audience(auth);

        // Cursor pages must resume the exact catalog captured by page one. In
        // particular, do not turn every offset into another fleet-wide
        // resources/list fan-out.
        if let Some(revision) = page_collector.expected_revision().map(str::to_owned) {
            let snapshot = self
                .route_runtime
                .resource_snapshot(&snapshot_audience, &revision)
                .await;
            let Some((snapshot, provenance, stored_shadow_key)) = snapshot else {
                let error = invalid_cursor(
                    "resource-list snapshot expired or is unavailable; restart from the first page",
                );
                let elapsed_ms = start.elapsed().as_millis();
                tracing::warn!(
                    surface = "mcp",
                    service = "labby",
                    action = "list_resources",
                    subject,
                    elapsed_ms,
                    kind = "invalid_cursor",
                    catalog_source = "snapshot_miss",
                    "resource list failed"
                );
                self.emit_dispatch_notification(
                    &context,
                    "lab",
                    "list_resources",
                    elapsed_ms,
                    DispatchLogOutcome::Failure {
                        level: LoggingLevel::Warning,
                        kind: "invalid_cursor",
                    },
                )
                .await;
                return Err(error);
            };
            page_collector.bind_revision(&revision)?;
            for resource in snapshot.iter().cloned() {
                page_collector.accept(resource);
                if page_collector.finished() {
                    break;
                }
            }
            let (resources, next_cursor) = page_collector.finish()?;
            #[cfg(feature = "gateway")]
            let (
                mut project_shadow_checked_resource_count,
                mut project_shadow_would_suppress_resource_count,
            ) = classify_builtin_action_resources(&project_shadow, snapshot.iter());
            #[cfg(feature = "gateway")]
            {
                let regular = classify_regular_upstream_resources(&project_shadow, &provenance);
                project_shadow_checked_resource_count += regular.0;
                project_shadow_would_suppress_resource_count += regular.1;
            }
            #[cfg(feature = "gateway")]
            let project_shadow_state = project_shadow.state_label_at(SystemTime::now());
            #[cfg(feature = "gateway")]
            let shadow_key_matches = stored_shadow_key
                .as_ref()
                .zip(project_shadow.snapshot_key(SystemTime::now()).as_ref())
                .is_some_and(|(stored, current)| stored == current);
            #[cfg(feature = "gateway")]
            let project_shadow_state = if project_shadow_state == "bound" && !shadow_key_matches {
                "unavailable"
            } else {
                project_shadow_state
            };
            #[cfg(feature = "gateway")]
            if project_shadow_state != "bound" {
                project_shadow_checked_resource_count = 0;
                project_shadow_would_suppress_resource_count = 0;
            }
            #[cfg(not(feature = "gateway"))]
            let project_shadow_state = "legacy";
            #[cfg(not(feature = "gateway"))]
            let (
                project_shadow_checked_resource_count,
                project_shadow_would_suppress_resource_count,
            ) = (0usize, 0usize);
            let elapsed_ms = start.elapsed().as_millis();
            tracing::info!(
                surface = "mcp",
                service = "labby",
                action = "list_resources",
                subject,
                elapsed_ms,
                catalog_source = "snapshot",
                catalog_resource_count = snapshot.len(),
                page_resource_count = resources.len(),
                has_next_cursor = next_cursor.is_some(),
                project_shadow_state,
                project_shadow_checked_resource_count,
                project_shadow_would_suppress_resource_count,
                "resource list ok"
            );
            self.emit_dispatch_notification(
                &context,
                "lab",
                "list_resources",
                elapsed_ms,
                DispatchLogOutcome::Success,
            )
            .await;
            let mut result = ListResourcesResult::with_all_items(resources)
                .with_ttl_ms(0)
                .with_cache_scope(rmcp::model::CacheScope::Private);
            result.next_cursor = next_cursor;
            return Ok(result);
        }

        // Bare numeric cursors predate revision-bound resource snapshots. They
        // cannot safely resume a result set, and rejecting before discovery
        // prevents one stale cursor from triggering a new fleet refresh.
        if page_collector.start_offset() > 0 {
            return Err(invalid_cursor(
                "resource-list cursor must include the result-set revision; restart from the first page",
            ));
        }

        let mut resources = CatalogSnapshotCollector::new(page_collector);
        let mut regular_resource_provenance = Vec::new();

        for resource in self.file_stash_resources(&context).await? {
            resources.accept(resource);
            if resources.finished() {
                break;
            }
        }

        if !resources.finished() {
            resources.accept(
                Resource::new("lab://catalog", "catalog")
                    .with_description("Full discovery document for all services")
                    .with_mime_type("application/json"),
            );
        }

        // Error-contract schemas: always listed so agents can discover the
        // envelope contract in-band instead of relying on out-of-band docs.
        #[cfg(feature = "skills")]
        if !resources.finished() {
            resources.accept(
                Resource::new(SKILLS_EXTENSION_CONTRACT_URI, "contracts/skills-extension")
                    .with_description(
                        "The MCP Skills extension (SEP-2640) contract this server implements: \
                         pinned draft revision, URI grammar, and verification requirements",
                    )
                    .with_mime_type("text/markdown"),
            );
        }
        if !resources.finished() {
            resources.accept(
                Resource::new(AGENT_ERROR_CONTRACT_URI, "contracts/agent-error")
                    .with_description(
                        "JSON Schema for the versioned agent-error contract carried by every \
                         Labby error envelope (kind, origin, recovery, side_effects)",
                    )
                    .with_mime_type(CONTRACT_SCHEMA_MIME),
            );
        }
        if !resources.finished() {
            resources.accept(
                Resource::new(
                    CODE_MODE_CALL_ERROR_CONTRACT_URI,
                    "contracts/code-mode-call-error",
                )
                .with_description(
                    "JSON Schema for the structured error object a failed Code Mode \
                     callTool rejects with",
                )
                .with_mime_type(CONTRACT_SCHEMA_MIME),
            );
        }

        #[cfg(feature = "skills")]
        if !resources.finished()
            && self.route_scope.exposes_skills()
            && code_mode_read_scope_allowed(auth)
            && self.route_scope.allows_service("artifacts")
            && self.service_visible_on_mcp("skills").await
        {
            for resource in skill_library_app_resources() {
                resources.accept(resource);
                if resources.finished() {
                    break;
                }
            }
        }

        #[cfg(feature = "gateway")]
        if !resources.finished()
            && mcp_apps_config.manager
            && self.route_scope.is_root()
            && tool_execute_scope_allowed(auth)
        {
            for resource in mcp_apps_app_resources() {
                resources.accept(resource);
                if resources.finished() {
                    break;
                }
            }
        }

        if !resources.finished()
            && code_mode_app_enabled
            && code_mode_app_resources_visible(
                self.code_mode_visibility().await.exposes_synthetic_tools(),
                auth,
            )
        {
            for resource in code_mode_app_resources() {
                resources.accept(resource);
                if resources.finished() {
                    break;
                }
            }
        }

        #[cfg(feature = "gateway")]
        if !resources.finished()
            && admin_app_resources_visible(auth)
            && self
                .gateway_status_app_available_on_mcp_with(mcp_apps_config)
                .await
        {
            for resource in gateway_status_app_resources() {
                resources.accept(resource);
                if resources.finished() {
                    break;
                }
            }
        }

        #[cfg(feature = "gateway")]
        if !resources.finished()
            && mcp_apps_config.settings
            && admin_app_resources_visible(auth)
            && self.route_scope.allows_service("setup")
            && self.service_visible_on_mcp("setup").await
        {
            for resource in settings_app().listed_resources() {
                resources.accept(resource);
                if resources.finished() {
                    break;
                }
            }
        }

        if !resources.finished()
            && server_logs_app_enabled
            && admin_app_resources_visible(auth)
            && self.route_scope.allows_service(SERVER_LOGS_TOOL_NAME)
            && self.service_visible_on_mcp(SERVER_LOGS_TOOL_NAME).await
        {
            for resource in server_logs_app_resources() {
                resources.accept(resource);
                if resources.finished() {
                    break;
                }
            }
        }

        #[cfg(feature = "gateway")]
        if !resources.finished()
            && admin_app_resources_visible(auth)
            && self
                .add_server_app_available_on_mcp_with(mcp_apps_config)
                .await
        {
            for resource in add_server_app_resources() {
                resources.accept(resource);
                if resources.finished() {
                    break;
                }
            }
        }

        if !resources.finished() {
            for svc in self.registry.services() {
                if self.route_scope.allows_service(svc.name)
                    && self.service_visible_on_mcp(svc.name).await
                {
                    let uri = format!("lab://{}/actions", svc.name);
                    let name = format!("{}/actions", svc.name);
                    resources.accept(
                        Resource::new(uri, name)
                            .with_description(format!("Action list for {}", svc.name))
                            .with_mime_type("application/json"),
                    );
                    if resources.finished() {
                        break;
                    }
                }
            }
        }

        #[cfg(feature = "gateway")]
        if !resources.finished()
            && let Some(pool) = self.current_upstream_pool().await
        {
            for resource in pool
                .gateway_synthetic_resources_allowed(self.route_scope.allowed_upstreams())
                .await
            {
                resources.accept(resource);
                if resources.finished() {
                    break;
                }
            }
            if !resources.finished() {
                for listed in pool
                    .list_upstream_resources_with_provenance_allowed(
                        self.route_scope.allowed_upstreams(),
                    )
                    .await
                {
                    if !is_ui_resource_uri(&listed.native_uri) {
                        regular_resource_provenance.push(ResourceProvenance {
                            upstream: listed.upstream_name.clone(),
                            native_uri: listed.native_uri.clone(),
                        });
                    }
                    resources.accept(listed.resource);
                    if resources.finished() {
                        break;
                    }
                }
            }
            if !resources.finished()
                && let Some(oauth_subject) =
                    oauth_upstream_subject_for_request(auth, self.request_subject(&context))
            {
                let configs = self.route_scoped_oauth_upstream_configs().await;
                let mut scoped_resources = pool
                    .subject_scoped_resources(&configs, oauth_subject.as_ref())
                    .await;
                scoped_resources.retain(|resource| {
                    resource
                        .uri
                        .strip_prefix("lab://upstream/")
                        .and_then(|rest| rest.split('/').next())
                        .is_none_or(|upstream| self.route_scope.allows_upstream(upstream))
                });
                for resource in scoped_resources {
                    resources.accept(resource);
                    if resources.finished() {
                        break;
                    }
                }
            }
        }

        let revision = next_catalog_snapshot_revision();
        if let Err(error) = resources.bind_revision(&revision) {
            let elapsed_ms = start.elapsed().as_millis();
            let kind = pagination_error_kind(&error);
            tracing::warn!(
                surface = "mcp",
                service = "labby",
                action = "list_resources",
                subject,
                elapsed_ms,
                kind,
                "resource list failed"
            );
            self.emit_dispatch_notification(
                &context,
                "lab",
                "list_resources",
                elapsed_ms,
                DispatchLogOutcome::Failure {
                    level: LoggingLevel::Warning,
                    kind,
                },
            )
            .await;
            return Err(error);
        }
        let (resources, next_cursor, complete_catalog) = match resources.finish() {
            Ok(page) => page,
            Err(error) => {
                let elapsed_ms = start.elapsed().as_millis();
                let kind = pagination_error_kind(&error);
                tracing::warn!(
                    surface = "mcp",
                    service = "labby",
                    action = "list_resources",
                    subject,
                    elapsed_ms,
                    kind,
                    "resource list failed"
                );
                self.emit_dispatch_notification(
                    &context,
                    "lab",
                    "list_resources",
                    elapsed_ms,
                    DispatchLogOutcome::Failure {
                        level: LoggingLevel::Warning,
                        kind,
                    },
                )
                .await;
                return Err(error);
            }
        };
        let catalog_resource_count = complete_catalog.len();
        #[cfg(feature = "gateway")]
        let (
            mut project_shadow_checked_resource_count,
            mut project_shadow_would_suppress_resource_count,
        ) = classify_builtin_action_resources(&project_shadow, complete_catalog.iter());
        #[cfg(feature = "gateway")]
        {
            let regular =
                classify_regular_upstream_resources(&project_shadow, &regular_resource_provenance);
            project_shadow_checked_resource_count += regular.0;
            project_shadow_would_suppress_resource_count += regular.1;
        }
        #[cfg(feature = "gateway")]
        let project_shadow_state = project_shadow.state_label_at(SystemTime::now());
        #[cfg(feature = "gateway")]
        if project_shadow_state != "bound" {
            project_shadow_checked_resource_count = 0;
            project_shadow_would_suppress_resource_count = 0;
        }
        #[cfg(not(feature = "gateway"))]
        let project_shadow_state = "legacy";
        #[cfg(not(feature = "gateway"))]
        let (project_shadow_checked_resource_count, project_shadow_would_suppress_resource_count) =
            (0usize, 0usize);
        if next_cursor.is_some() {
            #[cfg(feature = "gateway")]
            let stored_project_shadow_key = project_shadow.snapshot_key(SystemTime::now());
            #[cfg(not(feature = "gateway"))]
            let stored_project_shadow_key = None;
            self.route_runtime
                .store_resource_snapshot(
                    snapshot_audience,
                    revision,
                    Arc::from(complete_catalog),
                    Arc::from(regular_resource_provenance),
                    stored_project_shadow_key,
                )
                .await;
        }

        let elapsed_ms = start.elapsed().as_millis();
        tracing::info!(
            surface = "mcp",
            service = "labby",
            action = "list_resources",
            subject,
            elapsed_ms,
            catalog_source = "live_snapshot",
            catalog_resource_count,
            page_resource_count = resources.len(),
            has_next_cursor = next_cursor.is_some(),
            project_shadow_state,
            project_shadow_checked_resource_count,
            project_shadow_would_suppress_resource_count,
            "resource list ok"
        );
        self.emit_dispatch_notification(
            &context,
            "lab",
            "list_resources",
            elapsed_ms,
            DispatchLogOutcome::Success,
        )
        .await;

        let mut result = ListResourcesResult::with_all_items(resources)
            .with_ttl_ms(0)
            .with_cache_scope(rmcp::model::CacheScope::Private);
        result.next_cursor = next_cursor;
        Ok(result)
    }

    pub(crate) async fn list_resource_templates_impl(
        &self,
        request: Option<PaginatedRequestParams>,
        context: RequestContext<RoleServer>,
    ) -> Result<ListResourceTemplatesResult, ErrorData> {
        let start = Instant::now();
        let subject = self.request_subject_log_tag(&context);
        if !self.route_scope.exposes_resources() {
            let elapsed_ms = start.elapsed().as_millis();
            tracing::info!(
                surface = "mcp",
                service = "labby",
                action = "list_resource_templates",
                subject,
                route_scope = %self.route_scope.label(),
                elapsed_ms,
                "resource template catalog hidden by loadout"
            );
            return Ok(ListResourceTemplatesResult::with_all_items(Vec::new())
                .with_ttl_ms(0)
                .with_cache_scope(rmcp::model::CacheScope::Private));
        }
        let auth = auth_context_from_extensions(&context.extensions);
        let snapshot_audience = catalog_snapshot_audience(auth);
        #[cfg(feature = "gateway")]
        let project_shadow = project_discovery_shadow(&context.extensions, SystemTime::now());
        let mut page_collector = PageCollector::new(request)?;

        if let Some(revision) = page_collector.expected_revision().map(str::to_owned) {
            let snapshot = self
                .route_runtime
                .resource_template_snapshot(&snapshot_audience, &revision)
                .await;
            let Some((snapshot, provenance, stored_shadow_key)) = snapshot else {
                return Err(invalid_cursor(
                    "resource-template snapshot expired or is unavailable; restart from the first page",
                ));
            };
            page_collector.bind_revision(&revision)?;
            for template in snapshot.iter().cloned() {
                page_collector.accept(template);
                if page_collector.finished() {
                    break;
                }
            }
            let (templates, next_cursor) = page_collector.finish()?;
            #[cfg(feature = "gateway")]
            let (mut shadow_checked, mut shadow_would_suppress) =
                classify_regular_upstream_resource_templates(&project_shadow, &provenance);
            #[cfg(feature = "gateway")]
            let shadow_key_matches = stored_shadow_key
                .as_ref()
                .zip(project_shadow.snapshot_key(SystemTime::now()).as_ref())
                .is_some_and(|(stored, current)| stored == current);
            #[cfg(feature = "gateway")]
            let current_shadow_state = project_shadow.state_label_at(SystemTime::now());
            #[cfg(feature = "gateway")]
            let project_shadow_state = if current_shadow_state == "bound" && !shadow_key_matches {
                "unavailable"
            } else {
                current_shadow_state
            };
            #[cfg(feature = "gateway")]
            if project_shadow_state != "bound" {
                shadow_checked = 0;
                shadow_would_suppress = 0;
            }
            #[cfg(not(feature = "gateway"))]
            let (project_shadow_state, shadow_checked, shadow_would_suppress) = ("legacy", 0, 0);
            let elapsed_ms = start.elapsed().as_millis();
            tracing::info!(
                surface = "mcp",
                service = "labby",
                action = "list_resource_templates",
                subject,
                template_count = templates.len(),
                catalog_template_count = snapshot.len(),
                catalog_source = "snapshot",
                project_shadow_state,
                project_shadow_checked_template_count = shadow_checked,
                project_shadow_would_suppress_template_count = shadow_would_suppress,
                has_next_cursor = next_cursor.is_some(),
                elapsed_ms,
                "resource template list ok"
            );
            self.emit_dispatch_notification(
                &context,
                "lab",
                "list_resource_templates",
                elapsed_ms,
                DispatchLogOutcome::Success,
            )
            .await;
            let mut result = ListResourceTemplatesResult::with_all_items(templates)
                .with_ttl_ms(0)
                .with_cache_scope(rmcp::model::CacheScope::Private);
            result.next_cursor = next_cursor;
            return Ok(result);
        }

        if page_collector.start_offset() > 0 {
            return Err(invalid_cursor(
                "resource-template cursor must include the result-set revision; restart from the first page",
            ));
        }

        let mut templates = CatalogSnapshotCollector::new(page_collector);
        if self.file_stash_caller_bound()
            && self.route_scope.allows_service("stash")
            && self
                .file_stash_principal(&context, Some(&context.meta))
                .await
                .is_ok()
        {
            templates.accept(crate::mcp::file_stash::template());
        }
        #[cfg(feature = "gateway")]
        let mut regular_template_provenance = Vec::new();
        #[cfg(feature = "gateway")]
        if let Some(pool) = self.current_upstream_pool().await {
            for listed in pool
                .list_upstream_resource_templates_with_provenance_allowed(
                    self.route_scope.allowed_upstreams(),
                )
                .await
            {
                regular_template_provenance.push(ResourceTemplateProvenance {
                    upstream: listed.upstream_name,
                    native_uri_template: listed.native_uri_template,
                });
                templates.accept(listed.template);
            }
        }

        let revision = next_catalog_snapshot_revision();
        templates.bind_revision(&revision)?;
        let (templates, next_cursor, complete_catalog) = templates.finish()?;
        let catalog_template_count = complete_catalog.len();
        #[cfg(feature = "gateway")]
        let (mut shadow_checked, mut shadow_would_suppress) =
            classify_regular_upstream_resource_templates(
                &project_shadow,
                &regular_template_provenance,
            );
        #[cfg(feature = "gateway")]
        let project_shadow_state = project_shadow.state_label_at(SystemTime::now());
        #[cfg(feature = "gateway")]
        if project_shadow_state != "bound" {
            shadow_checked = 0;
            shadow_would_suppress = 0;
        }
        #[cfg(not(feature = "gateway"))]
        let (project_shadow_state, shadow_checked, shadow_would_suppress) = ("legacy", 0, 0);
        #[cfg(not(feature = "gateway"))]
        let regular_template_provenance = Vec::new();
        if next_cursor.is_some() {
            #[cfg(feature = "gateway")]
            let stored_shadow_key = project_shadow.snapshot_key(SystemTime::now());
            #[cfg(not(feature = "gateway"))]
            let stored_shadow_key = None;
            self.route_runtime
                .store_resource_template_snapshot(
                    snapshot_audience,
                    revision,
                    Arc::from(complete_catalog),
                    Arc::from(regular_template_provenance),
                    stored_shadow_key,
                )
                .await;
        }

        let elapsed_ms = start.elapsed().as_millis();
        tracing::info!(
            surface = "mcp",
            service = "labby",
            action = "list_resource_templates",
            subject,
            template_count = templates.len(),
            catalog_template_count,
            catalog_source = "live_snapshot",
            project_shadow_state,
            project_shadow_checked_template_count = shadow_checked,
            project_shadow_would_suppress_template_count = shadow_would_suppress,
            has_next_cursor = next_cursor.is_some(),
            elapsed_ms,
            "resource template list ok"
        );
        self.emit_dispatch_notification(
            &context,
            "lab",
            "list_resource_templates",
            elapsed_ms,
            DispatchLogOutcome::Success,
        )
        .await;

        let mut result = ListResourceTemplatesResult::with_all_items(templates)
            .with_ttl_ms(0)
            .with_cache_scope(rmcp::model::CacheScope::Private);
        result.next_cursor = next_cursor;
        Ok(result)
    }

    pub(crate) async fn read_resource_impl(
        &self,
        request: ReadResourceRequestParams,
        context: RequestContext<RoleServer>,
    ) -> Result<ReadResourceResponse, ErrorData> {
        let start = Instant::now();
        let subject = self.request_subject_log_tag(&context);
        let uri = request.uri.clone();
        #[cfg(feature = "gateway")]
        let resource_uri_log =
            crate::dispatch::upstream::pool::redact_resource_uri_for_logging(&uri);
        #[cfg(not(feature = "gateway"))]
        let resource_uri_log = uri.to_string();
        tracing::info!(
            surface = "mcp",
            service = "labby",
            action = "read_resource",
            subject,
            resource_uri = %resource_uri_log,
            "dispatch start"
        );
        if uri.starts_with("stash://") {
            if !self.file_stash_caller_bound()
                || !self.route_scope.exposes_resources()
                || !self.route_scope.allows_service("stash")
            {
                return Err(unknown_resource_error(&uri, false));
            }
            return self.read_file_stash_resource(&uri, &context).await;
        }
        #[cfg(feature = "gateway")]
        match project_execution_binding(&context.extensions, SystemTime::now()) {
            ProjectExecutionBinding::Legacy => {}
            ProjectExecutionBinding::Unavailable => {
                let elapsed_ms = start.elapsed().as_millis();
                self.emit_dispatch_notification(
                    &context,
                    "lab",
                    "read_resource",
                    elapsed_ms,
                    DispatchLogOutcome::Failure {
                        level: LoggingLevel::Warning,
                        kind: "access_context_unavailable",
                    },
                )
                .await;
                return Err(unknown_resource_error(&uri, false));
            }
            ProjectExecutionBinding::Bound {
                transport,
                identity,
            } => {
                let result = match self.gateway_manager.as_deref() {
                    Some(manager) => {
                        crate::mcp::resource_execution::read_transport_bound_project_resource(
                            self.access_runtime.as_ref(),
                            manager,
                            transport,
                            identity,
                            request,
                        )
                        .await
                    }
                    None => Err(
                        crate::mcp::resource_execution::ResourceReadResolutionError::Unavailable,
                    ),
                };
                let elapsed_ms = start.elapsed().as_millis();
                return match result {
                    Ok(response) => {
                        self.emit_dispatch_notification(
                            &context,
                            "lab",
                            "read_resource",
                            elapsed_ms,
                            DispatchLogOutcome::Success,
                        )
                        .await;
                        Ok(response.into())
                    }
                    Err(error) => {
                        use crate::mcp::resource_execution::ResourceReadResolutionError;
                        let unavailable =
                            matches!(&error, ResourceReadResolutionError::Unavailable);
                        // Preserve the distinct causes instead of collapsing
                        // them: `cancelled` is not automatically retryable and
                        // `response_too_large` tells the caller to reduce work,
                        // while a bare `internal_error` invites a blind retry.
                        // Cancellation is caller-driven, so it must not log at
                        // ERROR and page an operator for a healthy upstream.
                        let (level, kind, summary) = match &error {
                            ResourceReadResolutionError::Unavailable => {
                                (LoggingLevel::Warning, "not_found", "is unavailable")
                            }
                            ResourceReadResolutionError::Cancelled => {
                                (LoggingLevel::Warning, "cancelled", "read was cancelled")
                            }
                            ResourceReadResolutionError::Timeout => {
                                (LoggingLevel::Error, "timeout", "read timed out")
                            }
                            ResourceReadResolutionError::TooLarge => (
                                LoggingLevel::Warning,
                                "response_too_large",
                                "response exceeded the gateway cap",
                            ),
                            ResourceReadResolutionError::QueueUnavailable
                            | ResourceReadResolutionError::Upstream => (
                                LoggingLevel::Error,
                                "upstream_error",
                                "could not be fetched",
                            ),
                        };
                        self.emit_dispatch_notification(
                            &context,
                            "lab",
                            "read_resource",
                            elapsed_ms,
                            DispatchLogOutcome::Failure { level, kind },
                        )
                        .await;
                        if unavailable {
                            Err(unknown_resource_error(&uri, false))
                        } else {
                            Err(resource_fetch_classified(&uri, kind, summary))
                        }
                    }
                };
            }
        }
        if !self.route_scope.exposes_resources() {
            let elapsed_ms = start.elapsed().as_millis();
            let message = "MCP Resources are disabled by this loadout; ask the operator to enable Resources for this loadout (Agent Skills also require Resources)";
            self.log_route_scope_denial(
                &context,
                "resources",
                "read_resource",
                message,
                elapsed_ms,
            );
            return Err(ErrorData::new(
                rmcp::model::ErrorCode::INVALID_REQUEST,
                message.to_string(),
                None,
            ));
        }

        // Branch -1: canonical skill files. The skill namespace is exact-match
        // and disjoint from the lab:// resource handlers below.
        #[cfg(feature = "skills")]
        if crate::mcp::skills::is_skill_uri(&uri) {
            if !self.route_scope.exposes_skills() {
                let message = "Agent Skills are disabled by this loadout; ask the operator to enable Skills (and Resources) for this loadout";
                self.log_route_scope_denial(
                    &context,
                    "skills",
                    "read_resource",
                    message,
                    start.elapsed().as_millis(),
                );
                return Err(ErrorData::new(
                    rmcp::model::ErrorCode::INVALID_REQUEST,
                    message.to_string(),
                    None,
                ));
            }
            // Same scope `skills/list` and `skills/get` require. Gating only
            // the enumerating methods would leave every skill file fetchable by
            // URI, and skill URIs are not secret — they appear in listings,
            // docs, and any prior authorized session. That is a bypass, not a
            // restriction, and it is the rule this repo already applies to
            // `expose_skills` on the listing-and-access pair.
            if !code_mode_read_scope_allowed(auth_context_from_extensions(&context.extensions)) {
                return Err(forbidden_resource_error(
                    &uri,
                    "skill resources require one of scopes: lab:read, lab, lab:admin",
                    &["lab:read", "lab", "lab:admin"],
                ));
            }
            let registry = self
                .skill_registry_context(&context)
                .await
                .map_err(crate::mcp::skills::skill_read_error)?;
            tracing::debug!(
                surface = "mcp",
                service = "labby",
                action = "read_resource",
                skill_generation = registry.generation_id(),
                skill_generation_digest = registry.generation_digest(),
                "captured Skill generation"
            );
            let file = read_skill_resource_with_registry(&registry, &uri)
                .await
                .map_err(crate::mcp::skills::skill_read_error)?;
            tracing::info!(
                surface = "mcp",
                service = "labby",
                action = "read_resource",
                subject,
                resource_uri = %resource_uri_log,
                skill_origin = %file.origin,
                elapsed_ms = start.elapsed().as_millis(),
                "dispatch finish"
            );
            let contents = visible_skill_resource_contents(file, &uri);
            return Ok(ReadResourceResult::new(vec![contents]).into());
        }

        // Branch 0: MCP Apps UI resources. This must precede all lab://
        // fallbacks so ui:// has its own exact lookup semantics.
        //
        // The `mcp_app` control tool is always locally available, but its own
        // Labby-owned UI is opt-in like every other Labby-owned MCP App.
        #[cfg(feature = "gateway")]
        if uri.starts_with(MCP_APPS_APP_URI) {
            if !self.mcp_apps_config().await.manager {
                return Err(unknown_resource_error(&uri, true));
            }
            return self
                .read_mcp_apps_app_resource_impl(&uri, &subject, start, &context)
                .await
                .map(Into::into);
        }

        #[cfg(feature = "skills")]
        if uri.starts_with(SKILL_LIBRARY_APP_URI_PREFIX) {
            return self
                .read_skill_library_app_resource_impl(&uri, &subject, start, &context)
                .await
                .map_err(|error| with_skill_library_resource_correlation(error, &context))
                .map(Into::into);
        }

        // Local Code Mode app resources own the `ui://lab/code-mode/*` namespace
        // and are served from the bundled HTML.
        if uri.starts_with(CODE_MODE_APP_URI_PREFIX) {
            return self
                .read_code_mode_app_resource_impl(&uri, &subject, start, &context)
                .await
                .map(Into::into);
        }
        if uri.starts_with(SERVER_LOGS_APP_URI_PREFIX) {
            return self
                .read_server_logs_app_resource_impl(&uri, &subject, start, &context)
                .await
                .map(Into::into);
        }
        #[cfg(feature = "gateway")]
        if uri.starts_with(ADD_SERVER_APP_URI) {
            return self
                .read_add_server_app_resource_impl(
                    &uri,
                    &resource_uri_log,
                    &subject,
                    start,
                    &context,
                )
                .await
                .map(Into::into);
        }
        #[cfg(feature = "gateway")]
        if uri.starts_with(GATEWAY_STATUS_APP_URI) {
            return self
                .read_gateway_status_app_resource_impl(
                    &uri,
                    &resource_uri_log,
                    &subject,
                    start,
                    &context,
                )
                .await
                .map(Into::into);
        }
        #[cfg(feature = "gateway")]
        if uri.starts_with(SETTINGS_APP_URI) {
            return self
                .read_settings_app_resource_impl(&uri, &resource_uri_log, &subject, start, &context)
                .await
                .map(Into::into);
        }
        // Any other `ui://` is an upstream MCP Apps (mcp-ui) widget resource
        // (referenced by a tool result's `_meta.ui.resourceUri`): reverse-look-up
        // the owning upstream peer via the pool and forward the read under the
        // native `ui://` URI. These widgets are surfaced through the Code Mode
        // synthetic surface, so gate them behind the same read scope as Lab's own
        // Code Mode app resources rather than leaving them ungated.
        #[cfg(feature = "gateway")]
        if uri.starts_with("ui://") {
            let auth = auth_context_from_extensions(&context.extensions);
            if !code_mode_read_scope_allowed(auth) {
                return Err(forbidden_resource_error(
                    &uri,
                    "UI resources require one of scopes: lab:read, lab, lab:admin",
                    &["lab:read", "lab", "lab:admin"],
                ));
            }
            if let Some(pool) = self.current_upstream_pool().await {
                return self
                    .read_upstream_ui_resource_impl(&pool, request, &subject, start, &context)
                    .await;
            }
            return Err(unknown_resource_error(&uri, true));
        }

        // Error-contract schema resources: in-band discovery of the published
        // agent-error / Code Mode call-error contracts, served from the
        // embedded schemas. Read-only, no scope requirement — the contract is
        // documentation, exactly like `lab://catalog`.
        #[cfg(feature = "skills")]
        if uri == SKILLS_EXTENSION_CONTRACT_URI {
            tracing::info!(
                surface = "mcp",
                service = "labby",
                action = "read_resource",
                subject,
                resource_uri = %resource_uri_log,
                elapsed_ms = start.elapsed().as_millis(),
                "dispatch finish"
            );
            return Ok(ReadResourceResult::new(vec![
                ResourceContents::text(SKILLS_EXTENSION_CONTRACT, uri.to_string())
                    .with_mime_type("text/markdown"),
            ])
            .into());
        }
        if uri == AGENT_ERROR_CONTRACT_URI || uri == CODE_MODE_CALL_ERROR_CONTRACT_URI {
            return self
                .read_contract_schema_resource(&uri, &subject, start, &context)
                .await
                .map(Into::into);
        }

        // Branch 1: local per-service action resources. This must precede the
        // `lab://gateway/*` proxy branch so `lab://gateway/actions` remains the
        // built-in gateway service catalog resource, not a gateway synthetic
        // resource lookup.
        if let Some(service) = uri
            .strip_prefix("lab://")
            .and_then(|value| value.strip_suffix("/actions"))
        {
            if !self.route_scope.allows_service(service) {
                let elapsed_ms = start.elapsed().as_millis();
                let message = format!("service `{service}` is not exposed on this MCP route");
                tracing::warn!(
                    surface = "mcp",
                    service,
                    action = "read_resource",
                    subject,
                    route_scope = %self.route_scope.label(),
                    resource_uri = %resource_uri_log,
                    elapsed_ms,
                    kind = "route_scope_denied",
                    error = %message,
                    "MCP resource read denied by protected route scope"
                );
                self.emit_dispatch_notification(
                    &context,
                    "lab",
                    "read_resource",
                    elapsed_ms,
                    DispatchLogOutcome::Failure {
                        level: LoggingLevel::Warning,
                        kind: "route_scope_denied",
                    },
                )
                .await;
                return Err(route_scope_resource_error(&uri, service, &message));
            }

            let json = self.service_actions_json(service).await;
            return self
                .read_local_json_resource(json, &uri, &subject, start, &context)
                .await
                .map(Into::into);
        }

        // Branch 2: gateway-synthetic resources.
        #[cfg(feature = "gateway")]
        if uri.starts_with("lab://gateway/") {
            return self
                .read_gateway_resource_impl(&uri, &subject, start, &context)
                .await
                .map(Into::into);
        }

        // Branch 3: subject-scoped OAuth upstream resource proxy. OAuth
        // ownership must be resolved before the raw pool path, otherwise the
        // unconditional raw return makes this route unreachable.
        #[cfg(feature = "gateway")]
        let auth = auth_context_from_extensions(&context.extensions);
        #[cfg(feature = "gateway")]
        if let Some(oauth_subject) =
            oauth_upstream_subject_for_request(auth, self.request_subject(&context))
            && let Some(pool) = self.current_upstream_pool().await
            && let Some(upstream_name) = uri
                .strip_prefix("lab://upstream/")
                .and_then(|rest| rest.split('/').next())
            && self.route_scope.allows_upstream(upstream_name)
            && let Some(config) = self.oauth_upstream_config(upstream_name).await
        {
            return self
                .read_subject_scoped_resource_impl(
                    &pool,
                    &config,
                    oauth_subject.as_ref(),
                    request.clone(),
                    &subject,
                    start,
                    &context,
                )
                .await;
        }

        // Branch 4: raw upstream resource proxy.
        #[cfg(feature = "gateway")]
        if let Some(pool) = self.current_upstream_pool().await
            && uri.starts_with("lab://upstream/")
        {
            return self
                .read_upstream_resource_impl(&pool, request, &subject, start, &context)
                .await;
        }

        // Local branch: lab://catalog + lab://<svc>/actions.
        let json = if uri == "lab://catalog" {
            self.catalog_json().await
        } else {
            return Err(unknown_resource_error(uri.as_ref(), false));
        };

        self.read_local_json_resource(json, &uri, &subject, start, &context)
            .await
            .map(Into::into)
    }

    /// Serve one of the embedded error-contract JSON Schemas.
    async fn read_contract_schema_resource(
        &self,
        uri: &str,
        subject: &str,
        start: Instant,
        context: &RequestContext<RoleServer>,
    ) -> Result<ReadResourceResult, ErrorData> {
        let schema = match uri {
            AGENT_ERROR_CONTRACT_URI => AGENT_ERROR_CONTRACT_SCHEMA,
            CODE_MODE_CALL_ERROR_CONTRACT_URI => CODE_MODE_CALL_ERROR_CONTRACT_SCHEMA,
            _ => return Err(unknown_resource_error(uri, false)),
        };
        let elapsed_ms = start.elapsed().as_millis();
        tracing::info!(
            surface = "mcp",
            service = "labby",
            action = "read_resource",
            subject,
            elapsed_ms,
            resource_uri = uri,
            "contract schema resource read ok"
        );
        self.emit_dispatch_notification(
            context,
            "lab",
            "read_resource",
            elapsed_ms,
            DispatchLogOutcome::Success,
        )
        .await;
        Ok(ReadResourceResult::new(vec![
            ResourceContents::text(schema, uri.to_string()).with_mime_type(CONTRACT_SCHEMA_MIME),
        ]))
    }

    async fn read_local_json_resource(
        &self,
        json: anyhow::Result<Value>,
        uri: &str,
        subject: &str,
        start: Instant,
        context: &RequestContext<RoleServer>,
    ) -> Result<ReadResourceResult, ErrorData> {
        match json {
            Ok(value) => {
                let text = match serde_json::to_string_pretty(&value) {
                    Ok(t) => t,
                    Err(e) => {
                        tracing::error!(
                            surface = "mcp",
                            service = "labby",
                            action = "read_resource",
                            subject,
                            error = %e,
                            "failed to serialize resource"
                        );
                        return Err(resource_render_error(
                            uri,
                            format!("failed to serialize resource: {e}"),
                        ));
                    }
                };
                let elapsed_ms = start.elapsed().as_millis();
                tracing::info!(
                    surface = "mcp",
                    service = "labby",
                    action = "read_resource",
                    subject,
                    elapsed_ms,
                    "resource read ok"
                );
                self.emit_dispatch_notification(
                    &context,
                    "lab",
                    "read_resource",
                    elapsed_ms,
                    DispatchLogOutcome::Success,
                )
                .await;
                Ok(ReadResourceResult::new(vec![
                    ResourceContents::text(text, uri.to_string())
                        .with_mime_type("application/json"),
                ]))
            }
            Err(e) => {
                let elapsed_ms = start.elapsed().as_millis();
                tracing::error!(
                    surface = "mcp",
                    service = "labby",
                    action = "read_resource",
                    elapsed_ms,
                    kind = "internal_error",
                    "resource read failed"
                );
                self.emit_dispatch_notification(
                    &context,
                    "lab",
                    "read_resource",
                    elapsed_ms,
                    DispatchLogOutcome::Failure {
                        level: LoggingLevel::Error,
                        kind: "internal_error",
                    },
                )
                .await;
                Err(resource_render_error(uri, e.to_string()))
            }
        }
    }

    #[cfg(feature = "skills")]
    async fn read_skill_library_app_resource_impl(
        &self,
        uri: &str,
        subject: &str,
        start: Instant,
        context: &RequestContext<RoleServer>,
    ) -> Result<ReadResourceResult, ErrorData> {
        if !self.route_scope.exposes_skills()
            || !self.route_scope.allows_service("artifacts")
            || !self.service_visible_on_mcp("skills").await
        {
            return Err(unknown_resource_error(uri, true));
        }
        if !code_mode_read_scope_allowed(auth_context_from_extensions(&context.extensions)) {
            return Err(forbidden_resource_error(
                uri,
                "Skill Library app resources require one of scopes: lab:read, lab, lab:admin",
                &["lab:read", "lab", "lab:admin"],
            ));
        }
        let app = skill_library_app();
        let descriptor = app
            .descriptor(uri)
            .ok_or_else(|| unknown_resource_error(uri, true))?;
        let html = app
            .inline_html(descriptor)
            .map_err(|message| resource_render_error(uri, message))?;
        let mime_type = descriptor.runtime.mime();
        let elapsed_ms = start.elapsed().as_millis();
        tracing::info!(
            surface = "mcp",
            service = "labby",
            action = "read_resource",
            subject,
            elapsed_ms,
            resource_uri = uri,
            mime_type,
            html_bytes = html.len(),
            "Skill Library app resource read ok"
        );
        self.emit_dispatch_notification(
            context,
            "lab",
            "read_resource",
            elapsed_ms,
            DispatchLogOutcome::Success,
        )
        .await;
        Ok(ReadResourceResult::new(vec![
            ResourceContents::text(html, uri.to_string())
                .with_mime_type(mime_type)
                .with_meta(app_resource_meta_for_descriptor(uri, descriptor)),
        ]))
    }

    #[cfg(feature = "gateway")]
    async fn read_mcp_apps_app_resource_impl(
        &self,
        uri: &str,
        subject: &str,
        start: Instant,
        context: &RequestContext<RoleServer>,
    ) -> Result<ReadResourceResult, ErrorData> {
        if !self.route_scope.is_root() {
            return Err(unknown_resource_error(uri, true));
        }
        let auth = auth_context_from_extensions(&context.extensions);
        if !tool_execute_scope_allowed(auth) {
            return Err(forbidden_resource_error(
                uri,
                "MCP App manager resources require one of scopes: lab, lab:admin",
                &["lab", "lab:admin"],
            ));
        }
        let app = mcp_apps_app();
        let descriptor = app
            .descriptor(uri)
            .ok_or_else(|| unknown_resource_error(uri, true))?;
        let html = app
            .inline_html(descriptor)
            .map_err(|message| resource_render_error(uri, message))?;
        let mime_type = descriptor.runtime.mime();
        let elapsed_ms = start.elapsed().as_millis();
        tracing::info!(
            surface = "mcp",
            service = "labby",
            action = "read_resource",
            subject,
            elapsed_ms,
            resource_uri = uri,
            mime_type,
            html_bytes = html.len(),
            "MCP App manager resource read ok"
        );
        self.emit_dispatch_notification(
            context,
            "lab",
            "read_resource",
            elapsed_ms,
            DispatchLogOutcome::Success,
        )
        .await;
        Ok(ReadResourceResult::new(vec![
            ResourceContents::text(html, uri.to_string())
                .with_mime_type(mime_type)
                .with_meta(app_resource_meta_for_descriptor(uri, descriptor)),
        ]))
    }

    #[cfg(feature = "gateway")]
    async fn read_settings_app_resource_impl(
        &self,
        uri: &str,
        resource_uri_log: &str,
        subject: &str,
        start: Instant,
        context: &RequestContext<RoleServer>,
    ) -> Result<ReadResourceResult, ErrorData> {
        if !self.mcp_apps_config().await.settings
            || !self.route_scope.allows_service("setup")
            || !self.service_visible_on_mcp("setup").await
        {
            return Err(unknown_resource_error(uri, true));
        }
        if !admin_app_resources_visible(auth_context_from_extensions(&context.extensions)) {
            return Err(forbidden_resource_error(
                uri,
                "Settings app resources require scope: lab:admin",
                &["lab:admin"],
            ));
        }
        let app = settings_app();
        let descriptor = app
            .descriptor(uri)
            .ok_or_else(|| unknown_resource_error(uri, true))?;
        let html = app
            .inline_html(descriptor)
            .map_err(|message| resource_render_error(uri, message))?;
        let mime_type = descriptor.runtime.mime();
        let elapsed_ms = start.elapsed().as_millis();
        tracing::info!(
            surface = "mcp",
            service = "labby",
            action = "read_resource",
            subject,
            elapsed_ms,
            resource_uri = resource_uri_log,
            mime_type,
            html_bytes = html.len(),
            "settings app resource read ok"
        );
        self.emit_dispatch_notification(
            context,
            "lab",
            "read_resource",
            elapsed_ms,
            DispatchLogOutcome::Success,
        )
        .await;
        Ok(ReadResourceResult::new(vec![
            ResourceContents::text(html, uri.to_string())
                .with_mime_type(mime_type)
                .with_meta(app_resource_meta_for_descriptor(uri, descriptor)),
        ]))
    }

    async fn read_code_mode_app_resource_impl(
        &self,
        uri: &str,
        subject: &str,
        start: Instant,
        context: &RequestContext<RoleServer>,
    ) -> Result<ReadResourceResult, ErrorData> {
        if !self.code_mode_visibility().await.exposes_synthetic_tools()
            || !self.code_mode_app_enabled_on_mcp().await
        {
            return Err(unknown_resource_error(uri, true));
        }
        let auth = auth_context_from_extensions(&context.extensions);
        if !code_mode_read_scope_allowed(auth) {
            let elapsed_ms = start.elapsed().as_millis();
            tracing::warn!(
                surface = "mcp",
                service = "labby",
                action = "read_resource",
                subject,
                elapsed_ms,
                kind = "forbidden",
                resource_uri = uri,
                "code mode app resource denied by scope"
            );
            self.emit_dispatch_notification(
                context,
                "lab",
                "read_resource",
                elapsed_ms,
                DispatchLogOutcome::Failure {
                    level: LoggingLevel::Warning,
                    kind: "forbidden",
                },
            )
            .await;
            return Err(forbidden_resource_error(
                uri,
                "Code Mode app resources require one of scopes: lab:read, lab, lab:admin",
                &["lab:read", "lab", "lab:admin"],
            ));
        }
        let history = if strip_app_version(uri) == CODE_MODE_HISTORY_APP_URI {
            #[cfg(feature = "gateway")]
            match &self.gateway_manager {
                Some(manager) if self.route_scope.protected_history_label().is_some() => {
                    let label = self.route_scope.protected_history_label();
                    Some(json!({
                        "kind": "code_mode_history",
                        "entries": manager.code_mode_history_snapshot_for_route_scope(label.as_deref()).await,
                    }))
                }
                Some(manager) => Some(json!({
                    "kind": "code_mode_history",
                    "entries": manager.code_mode_history_snapshot().await,
                })),
                None => Some(json!({ "kind": "code_mode_history", "entries": [] })),
            }
            #[cfg(not(feature = "gateway"))]
            {
                Some(json!({ "kind": "code_mode_history", "entries": [] }))
            }
        } else {
            None
        };
        let descriptor = app_descriptor_for_uri(CODE_MODE_APP_RESOURCE_DESCRIPTORS, uri)
            .ok_or_else(|| unknown_resource_error(uri, true))?;
        let html = code_mode_app_html_for_descriptor(history.as_ref());
        let runtime = descriptor.runtime;
        let mime_type = runtime.mime();
        let elapsed_ms = start.elapsed().as_millis();
        tracing::info!(
            surface = "mcp",
            service = "labby",
            action = "read_resource",
            subject,
            elapsed_ms,
            resource_uri = uri,
            mime_type,
            html_bytes = html.len(),
            versioned = uri.contains("?v="),
            "code mode app resource read ok"
        );
        self.emit_dispatch_notification(
            context,
            "lab",
            "read_resource",
            elapsed_ms,
            DispatchLogOutcome::Success,
        )
        .await;

        Ok(ReadResourceResult::new(vec![
            ResourceContents::text(html, uri.to_string())
                .with_mime_type(mime_type)
                .with_meta(app_resource_meta_for_descriptor(uri, descriptor)),
        ]))
    }

    async fn read_server_logs_app_resource_impl(
        &self,
        uri: &str,
        subject: &str,
        start: Instant,
        context: &RequestContext<RoleServer>,
    ) -> Result<ReadResourceResult, ErrorData> {
        #[cfg(feature = "gateway")]
        if !self.mcp_apps_config().await.server_logs {
            return Err(unknown_resource_error(uri, true));
        }
        if !self.route_scope.allows_service(SERVER_LOGS_TOOL_NAME)
            || !self.service_visible_on_mcp(SERVER_LOGS_TOOL_NAME).await
        {
            return Err(unknown_resource_error(uri, true));
        }
        let auth = auth_context_from_extensions(&context.extensions);
        if !admin_app_resources_visible(auth) {
            let elapsed_ms = start.elapsed().as_millis();
            tracing::warn!(
                surface = "mcp",
                service = "labby",
                action = "read_resource",
                subject,
                elapsed_ms,
                kind = "forbidden",
                resource_uri = uri,
                "server logs app resource denied by scope"
            );
            self.emit_dispatch_notification(
                context,
                "lab",
                "read_resource",
                elapsed_ms,
                DispatchLogOutcome::Failure {
                    level: LoggingLevel::Warning,
                    kind: "forbidden",
                },
            )
            .await;
            return Err(forbidden_resource_error(
                uri,
                "Server log app resources require scope: lab:admin",
                &["lab:admin"],
            ));
        }

        let app = server_logs_app();
        let descriptor = app
            .descriptor(uri)
            .ok_or_else(|| unknown_resource_error(uri, true))?;
        let html = app
            .inline_html(descriptor)
            .map_err(|message| resource_render_error(uri, message))?;
        let runtime = descriptor.runtime;
        let mime_type = runtime.mime();
        let elapsed_ms = start.elapsed().as_millis();
        tracing::info!(
            surface = "mcp",
            service = "labby",
            action = "read_resource",
            subject,
            elapsed_ms,
            resource_uri = uri,
            mime_type,
            html_bytes = html.len(),
            versioned = uri.contains("?v="),
            "server logs app resource read ok"
        );
        self.emit_dispatch_notification(
            context,
            "lab",
            "read_resource",
            elapsed_ms,
            DispatchLogOutcome::Success,
        )
        .await;

        Ok(ReadResourceResult::new(vec![
            ResourceContents::text(html, uri.to_string())
                .with_mime_type(mime_type)
                .with_meta(app_resource_meta_for_descriptor(uri, descriptor)),
        ]))
    }

    #[cfg(feature = "gateway")]
    async fn read_add_server_app_resource_impl(
        &self,
        uri: &str,
        resource_uri_log: &str,
        subject: &str,
        start: Instant,
        context: &RequestContext<RoleServer>,
    ) -> Result<ReadResourceResult, ErrorData> {
        if !self.add_server_app_available_on_mcp().await {
            let elapsed_ms = start.elapsed().as_millis();
            tracing::warn!(
                surface = "mcp",
                service = "labby",
                action = "read_resource",
                subject,
                elapsed_ms,
                kind = "not_found",
                resource_uri = resource_uri_log,
                "add server app resource unavailable"
            );
            self.emit_dispatch_notification(
                context,
                "lab",
                "read_resource",
                elapsed_ms,
                DispatchLogOutcome::Failure {
                    level: LoggingLevel::Warning,
                    kind: "not_found",
                },
            )
            .await;
            return Err(unknown_resource_error(uri, true));
        }
        let auth = auth_context_from_extensions(&context.extensions);
        if !admin_app_resources_visible(auth) {
            let elapsed_ms = start.elapsed().as_millis();
            tracing::warn!(
                surface = "mcp",
                service = "labby",
                action = "read_resource",
                subject,
                elapsed_ms,
                kind = "forbidden",
                resource_uri = resource_uri_log,
                "add server app resource denied by scope"
            );
            self.emit_dispatch_notification(
                context,
                "lab",
                "read_resource",
                elapsed_ms,
                DispatchLogOutcome::Failure {
                    level: LoggingLevel::Warning,
                    kind: "forbidden",
                },
            )
            .await;
            return Err(forbidden_resource_error(
                uri,
                "Add Server app resources require scope: lab:admin",
                &["lab:admin"],
            ));
        }
        let app = add_server_app();
        let descriptor = app
            .descriptor(uri)
            .ok_or_else(|| unknown_resource_error(uri, true))?;
        let html = app
            .inline_html(descriptor)
            .map_err(|message| resource_render_error(uri, message))?;
        let mime_type = descriptor.runtime.mime();
        let elapsed_ms = start.elapsed().as_millis();
        tracing::info!(
            surface = "mcp",
            service = "labby",
            action = "read_resource",
            subject,
            elapsed_ms,
            resource_uri = resource_uri_log,
            mime_type,
            html_bytes = html.len(),
            "add server app resource read ok"
        );
        self.emit_dispatch_notification(
            context,
            "lab",
            "read_resource",
            elapsed_ms,
            DispatchLogOutcome::Success,
        )
        .await;
        Ok(ReadResourceResult::new(vec![
            ResourceContents::text(html, uri.to_string())
                .with_mime_type(mime_type)
                .with_meta(app_resource_meta_for_descriptor(uri, descriptor)),
        ]))
    }

    #[cfg(feature = "gateway")]
    async fn read_gateway_status_app_resource_impl(
        &self,
        uri: &str,
        resource_uri_log: &str,
        subject: &str,
        start: Instant,
        context: &RequestContext<RoleServer>,
    ) -> Result<ReadResourceResult, ErrorData> {
        if !self.gateway_status_app_available_on_mcp().await {
            let elapsed_ms = start.elapsed().as_millis();
            tracing::warn!(
                surface = "mcp",
                service = "labby",
                action = "read_resource",
                subject,
                elapsed_ms,
                kind = "not_found",
                resource_uri = resource_uri_log,
                "gateway status app resource unavailable"
            );
            self.emit_dispatch_notification(
                context,
                "lab",
                "read_resource",
                elapsed_ms,
                DispatchLogOutcome::Failure {
                    level: LoggingLevel::Warning,
                    kind: "not_found",
                },
            )
            .await;
            return Err(unknown_resource_error(uri, true));
        }
        if !admin_app_resources_visible(auth_context_from_extensions(&context.extensions)) {
            let elapsed_ms = start.elapsed().as_millis();
            tracing::warn!(
                surface = "mcp",
                service = "labby",
                action = "read_resource",
                subject,
                elapsed_ms,
                kind = "forbidden",
                resource_uri = resource_uri_log,
                "gateway status app resource denied by scope"
            );
            self.emit_dispatch_notification(
                context,
                "lab",
                "read_resource",
                elapsed_ms,
                DispatchLogOutcome::Failure {
                    level: LoggingLevel::Warning,
                    kind: "forbidden",
                },
            )
            .await;
            return Err(forbidden_resource_error(
                uri,
                "Gateway Status app resources require scope: lab:admin",
                &["lab:admin"],
            ));
        }
        let app = gateway_status_app();
        let descriptor = app
            .descriptor(uri)
            .ok_or_else(|| unknown_resource_error(uri, true))?;
        let html = app
            .inline_html(descriptor)
            .map_err(|message| resource_render_error(uri, message))?;
        let mime_type = descriptor.runtime.mime();
        let elapsed_ms = start.elapsed().as_millis();
        tracing::info!(
            surface = "mcp",
            service = "labby",
            action = "read_resource",
            subject,
            elapsed_ms,
            resource_uri = resource_uri_log,
            mime_type,
            html_bytes = html.len(),
            "gateway status app resource read ok"
        );
        self.emit_dispatch_notification(
            context,
            "lab",
            "read_resource",
            elapsed_ms,
            DispatchLogOutcome::Success,
        )
        .await;
        Ok(ReadResourceResult::new(vec![
            ResourceContents::text(html, uri.to_string())
                .with_mime_type(mime_type)
                .with_meta(app_resource_meta_for_descriptor(uri, descriptor)),
        ]))
    }
}

#[cfg(feature = "skills")]
fn visible_skill_resource_contents(
    file: crate::skills::facade::VisibleSkillFile,
    uri: &str,
) -> ResourceContents {
    let mut contents = match file.content {
        crate::skills::facade::VisibleSkillContent::Text(text) => {
            ResourceContents::text(text, uri.to_string())
        }
        crate::skills::facade::VisibleSkillContent::Blob(bytes) => {
            use base64::Engine as _;
            ResourceContents::blob(
                base64::engine::general_purpose::STANDARD.encode(bytes),
                uri.to_string(),
            )
        }
    };
    if let Some(mime_type) = file.mime_type {
        contents = contents.with_mime_type(mime_type);
    }
    contents
}

#[cfg(feature = "skills")]
pub(crate) async fn read_skill_resource_with_registry(
    registry: &crate::skills::facade::SkillRegistryContext,
    uri: &str,
) -> Result<crate::skills::facade::VisibleSkillFile, labby_runtime::error::ToolError> {
    crate::skills::facade::read_visible_skill_file(registry, uri).await
}

#[cfg(test)]
fn code_mode_app_html(uri: &str, history: Option<&Value>) -> Result<String, String> {
    if app_descriptor_for_uri(CODE_MODE_APP_RESOURCE_DESCRIPTORS, uri).is_none() {
        return Err(format!("unknown UI resource: {uri}"));
    }
    Ok(code_mode_app_html_for_descriptor(history))
}

fn code_mode_app_html_for_descriptor(history: Option<&Value>) -> String {
    let mut html = CODE_MODE_APP_FALLBACK_HTML.to_string();
    if let Some(snapshot) = history {
        let injected = format!(
            "window.__LAB_CODE_MODE_INITIAL_TRACE__ = {};",
            snapshot.to_string().replace('<', "\\u003c")
        );
        html = html.replace("window.__LAB_CODE_MODE_INITIAL_TRACE__ = null;", &injected);
    }
    html
}

#[cfg(test)]
fn server_logs_app_html(uri: &str) -> Result<String, String> {
    let app = server_logs_app();
    let Some(descriptor) = app.descriptor(uri) else {
        return Err(format!("unknown UI resource: {uri}"));
    };
    app.inline_html(descriptor)
}

/// Replace the external host-script marker with the embedded bridge runtime.
fn inline_app_host_script(
    html: &str,
    descriptor: &AppResourceDescriptor,
) -> Result<String, String> {
    const HOST_SCRIPT_MARKER: &str = r#"<script src="/apps/assets/labby-app-host.js"></script>"#;
    if !html.contains(HOST_SCRIPT_MARKER) {
        return Err("missing Labby app host script marker".to_string());
    }
    let mcp_resource_flag = if descriptor.runtime == CodeModeRuntime::McpApp {
        "window.__LABBY_MCP_RESOURCE=true;"
    } else {
        ""
    };
    Ok(html.replace(
        HOST_SCRIPT_MARKER,
        &format!(
            "<script>{mcp_resource_flag}{}</script>",
            crate::app_assets::LABBY_APP_HOST_JS
        ),
    ))
}

/// Resolve a canonical or cache-busted URI within a descriptor table.
fn app_descriptor_for_uri<'a>(
    descriptors: &'a [AppResourceDescriptor],
    uri: &str,
) -> Option<&'a AppResourceDescriptor> {
    let base = strip_app_version(uri);
    descriptors.iter().find(|descriptor| descriptor.uri == base)
}

#[cfg(test)]
fn versioned_app_uri(base: &str) -> String {
    code_mode_app().versioned_uri(base)
}

/// Host runtime a Code Mode app URI targets. Callers must pass a table URI; an
/// un-tabled URI is a programming error because runtime selects MIME,
/// listed-ness, and tool binding.
#[cfg(test)]
fn code_mode_app_runtime_for_uri(uri: &str) -> CodeModeRuntime {
    app_runtime_for_uri(uri, CODE_MODE_APP_RESOURCE_DESCRIPTORS, "Code Mode")
}

#[cfg(test)]
fn app_runtime_for_uri(
    uri: &str,
    descriptors: &[AppResourceDescriptor],
    label: &'static str,
) -> CodeModeRuntime {
    app_descriptor_for_uri(descriptors, uri)
        .unwrap_or_else(|| panic!("{label} app runtime lookup called with un-tabled URI: {uri}"))
        .runtime
}

/// Whether Code Mode app resources are readable by the current caller.
fn code_mode_app_resources_visible(
    exposes_synthetic_tools: bool,
    auth: Option<&labby_auth::auth_context::AuthContext>,
) -> bool {
    exposes_synthetic_tools && code_mode_read_scope_allowed(auth)
}

/// Whether admin-only Lab-owned app resources are readable by this caller.
pub(crate) fn admin_app_resources_visible(
    auth: Option<&labby_auth::auth_context::AuthContext>,
) -> bool {
    auth.is_none_or(|auth| auth.scopes.iter().any(|scope| scope == "lab:admin"))
}

/// Build the discoverable Code Mode app resources.
fn code_mode_app_resources() -> Vec<Resource> {
    code_mode_app().listed_resources()
}

/// Build the discoverable Server Logs app resources.
fn server_logs_app_resources() -> Vec<Resource> {
    server_logs_app().listed_resources()
}

#[cfg(feature = "skills")]
fn skill_library_app_resources() -> Vec<Resource> {
    skill_library_app().listed_resources()
}

#[cfg(feature = "gateway")]
/// Build the discoverable Add Server app resources.
fn add_server_app_resources() -> Vec<Resource> {
    add_server_app().listed_resources()
}

#[cfg(feature = "gateway")]
/// Build the discoverable Gateway Status app resources.
fn gateway_status_app_resources() -> Vec<Resource> {
    gateway_status_app().listed_resources()
}

#[cfg(feature = "gateway")]
/// Build the discoverable opt-in MCP App manager UI resources.
fn mcp_apps_app_resources() -> Vec<Resource> {
    mcp_apps_app().listed_resources()
}

/// MCP Apps (Claude) widget URI for a tool — backs `_meta.ui.resourceUri`.
///
/// Carries the `?v=<hash>` cache-bust suffix so a rebuilt widget forces the host
/// to refetch instead of rendering its cached copy of the previous build.
pub(crate) fn code_mode_app_resource_uri_for_tool(tool_name: &str) -> Option<String> {
    code_mode_app().uri_for_tool(CodeModeRuntime::McpApp, tool_name)
}

/// OpenAI Apps (ChatGPT / Codex) widget URI for a tool — backs `openai/outputTemplate`.
///
/// Carries the same `?v=<hash>` cache-bust suffix as the MCP Apps URI.
pub(crate) fn code_mode_app_skybridge_uri_for_tool(tool_name: &str) -> Option<String> {
    code_mode_app().uri_for_tool(CodeModeRuntime::Skybridge, tool_name)
}

/// MCP Apps widget URI for the server log viewer tool.
pub(crate) fn server_logs_app_resource_uri_for_tool(tool_name: &str) -> Option<String> {
    server_logs_app().uri_for_tool(CodeModeRuntime::McpApp, tool_name)
}

/// OpenAI Apps skybridge widget URI for the server log viewer tool.
pub(crate) fn server_logs_app_skybridge_uri_for_tool(tool_name: &str) -> Option<String> {
    server_logs_app().uri_for_tool(CodeModeRuntime::Skybridge, tool_name)
}

/// MCP Apps resource URI for the Skill Library tool descriptor.
#[cfg(feature = "skills")]
pub(crate) fn skill_library_app_resource_uri_for_tool(tool_name: &str) -> Option<String> {
    skill_library_app().uri_for_tool(CodeModeRuntime::McpApp, tool_name)
}

/// OpenAI skybridge URI for the Skill Library tool descriptor.
#[cfg(feature = "skills")]
pub(crate) fn skill_library_app_skybridge_uri_for_tool(tool_name: &str) -> Option<String> {
    skill_library_app().uri_for_tool(CodeModeRuntime::Skybridge, tool_name)
}

#[cfg(feature = "gateway")]
pub(crate) fn add_server_app_resource_uri_for_tool(tool_name: &str) -> Option<String> {
    add_server_app().uri_for_tool(CodeModeRuntime::McpApp, tool_name)
}

#[cfg(feature = "gateway")]
pub(crate) fn add_server_app_skybridge_uri_for_tool(tool_name: &str) -> Option<String> {
    add_server_app().uri_for_tool(CodeModeRuntime::Skybridge, tool_name)
}

#[cfg(feature = "gateway")]
pub(crate) fn gateway_status_app_resource_uri_for_tool(tool_name: &str) -> Option<String> {
    gateway_status_app().uri_for_tool(CodeModeRuntime::McpApp, tool_name)
}

#[cfg(feature = "gateway")]
pub(crate) fn gateway_status_app_skybridge_uri_for_tool(tool_name: &str) -> Option<String> {
    gateway_status_app().uri_for_tool(CodeModeRuntime::Skybridge, tool_name)
}

#[cfg(feature = "gateway")]
pub(crate) fn settings_app_resource_uri_for_tool(tool_name: &str) -> Option<String> {
    settings_app().uri_for_tool(CodeModeRuntime::McpApp, tool_name)
}

#[cfg(feature = "gateway")]
pub(crate) fn settings_app_skybridge_uri_for_tool(tool_name: &str) -> Option<String> {
    settings_app().uri_for_tool(CodeModeRuntime::Skybridge, tool_name)
}

#[cfg(feature = "gateway")]
pub(crate) fn mcp_apps_app_resource_uri_for_tool(tool_name: &str) -> Option<String> {
    mcp_apps_app().uri_for_tool(CodeModeRuntime::McpApp, tool_name)
}

#[cfg(feature = "gateway")]
pub(crate) fn mcp_apps_app_skybridge_uri_for_tool(tool_name: &str) -> Option<String> {
    mcp_apps_app().uri_for_tool(CodeModeRuntime::Skybridge, tool_name)
}

#[cfg(test)]
pub(crate) fn code_mode_app_resource_meta(uri: &str) -> MetaObject {
    app_resource_meta(uri, CODE_MODE_APP_RESOURCE_DESCRIPTORS)
}

#[cfg(test)]
fn app_resource_meta(uri: &str, descriptors: &[AppResourceDescriptor]) -> MetaObject {
    let descriptor = app_descriptor_for_uri(descriptors, uri)
        .unwrap_or_else(|| panic!("app resource meta lookup called with un-tabled URI: {uri}"));
    app_resource_meta_for_descriptor(uri, descriptor)
}

fn app_resource_meta_for_descriptor(uri: &str, descriptor: &AppResourceDescriptor) -> MetaObject {
    build_app_resource_meta(
        uri,
        descriptor.runtime,
        descriptor.skybridge_widget_description,
    )
}

fn build_app_resource_meta(
    uri: &str,
    runtime: CodeModeRuntime,
    skybridge_widget_description: Option<&'static str>,
) -> MetaObject {
    let mut meta = serde_json::Map::new();
    meta.insert(
        "ui".to_string(),
        json!({
            "resourceUri": uri,
            "mimeTypes": [runtime.mime()],
            "csp": {
                "connectDomains": [],
                "resourceDomains": [],
                "frameDomains": [],
            },
            "prefersBorder": false,
        }),
    );
    if runtime == CodeModeRuntime::Skybridge
        && let Some(description) = skybridge_widget_description
    {
        meta.insert("openai/widgetDescription".to_string(), json!(description));
    }
    MetaObject(meta)
}

#[cfg(all(test, feature = "gateway"))]
#[allow(clippy::panic)]
#[allow(clippy::disallowed_methods)] // test fixtures construct upstream Tool values directly
mod tests {
    use super::*;

    #[cfg(feature = "skills")]
    #[test]
    fn skill_resource_read_emits_one_binary_content_with_mime_type() {
        use crate::skills::facade::{VisibleSkillContent, VisibleSkillFile};

        let uri = "skill://up/demo/asset.png";
        let content = visible_skill_resource_contents(
            VisibleSkillFile {
                uri: uri.into(),
                skill_uri: "skill://up/demo/SKILL.md".into(),
                origin: "up".into(),
                digest: "sha256:test".into(),
                mime_type: Some("image/png".into()),
                content: VisibleSkillContent::Blob(vec![0, 1, 2, 3]),
            },
            uri,
        );
        let result = ReadResourceResult::new(vec![content]);
        let wire = serde_json::to_value(result).unwrap();
        assert_eq!(wire["contents"].as_array().unwrap().len(), 1);
        assert_eq!(wire["contents"][0]["blob"], "AAECAw==");
        assert_eq!(wire["contents"][0]["mimeType"], "image/png");
        assert!(wire["contents"][0].get("text").is_none());
    }
    use crate::dispatch::upstream::pool::{
        InProcessConnector, InProcessRegistration, UpstreamConnection, UpstreamPool,
    };
    use crate::dispatch::upstream::types::UpstreamRuntimeMetadata;
    use futures::future::BoxFuture;
    use rmcp::model::{
        ArgumentInfo, CompleteRequestParams, CompleteResult, CompletionInfo,
        ListResourceTemplatesResult, ListResourcesResult, Reference, ResourceTemplate,
        ServerCapabilities, ServerInfo, Tool,
    };
    use rmcp::service::{Peer, RequestContext};
    use rmcp::{RoleClient, ServerHandler, ServiceExt};
    use serde_json::Value;
    use std::collections::BTreeMap;
    use std::future::Future;
    use std::pin::Pin;
    use std::process::Command;
    use std::sync::Arc;

    const UPSTREAM_UI_URI: &str = "ui://quick-shell/app.html";
    const UPSTREAM_UI_TOOL_NAME: &str = "quick_shell_ui";

    #[test]
    fn builtin_action_resource_parser_accepts_only_exact_canonical_family() {
        assert_eq!(
            builtin_action_resource_service("lab://gateway/actions"),
            Some("gateway")
        );
        for uri in [
            "lab:///actions",
            "lab://gateway/foo/actions",
            "lab://gateway/actions/",
            "lab://gateway/actions?cursor=1",
            "ui://gateway/actions",
        ] {
            assert_eq!(builtin_action_resource_service(uri), None, "{uri}");
        }

        let resources = [
            Resource::new("lab://fs/actions", "fs/actions"),
            Resource::new("lab://setup/actions", "setup/actions"),
            Resource::new("lab://setup/nested/actions", "nested"),
            Resource::new("lab://catalog", "catalog"),
        ];
        assert_eq!(
            classify_builtin_action_resources_with(resources.iter(), |service| {
                Some(service == "fs")
            }),
            (2, 1),
            "only exact built-in action resources are classified"
        );
    }

    #[test]
    fn regular_template_shadow_uses_exact_provenance_and_skips_ui() {
        let candidates = [
            ResourceTemplateProvenance {
                upstream: "alpha".into(),
                native_uri_template: "file:///{id}".into(),
            },
            ResourceTemplateProvenance {
                upstream: "bravo".into(),
                native_uri_template: "file:///{id}".into(),
            },
            ResourceTemplateProvenance {
                upstream: "alpha".into(),
                native_uri_template: "UI://widget/{id}".into(),
            },
        ];
        assert_eq!(
            classify_regular_upstream_resource_templates_with(&candidates, |upstream, template| {
                Some(upstream == "alpha" && template == "file:///{id}")
            }),
            (2, 1)
        );
    }

    #[test]
    fn regular_resource_shadow_uses_exact_upstream_and_native_uri_provenance() {
        let candidates = vec![
            ResourceProvenance {
                upstream: "alpha".into(),
                native_uri: "file:///same".into(),
            },
            ResourceProvenance {
                upstream: "beta".into(),
                native_uri: "file:///same".into(),
            },
            ResourceProvenance {
                upstream: "alpha".into(),
                native_uri: "file:///other".into(),
            },
        ];
        let classified = classify_regular_upstream_resources_with(&candidates, |upstream, uri| {
            match (upstream, uri) {
                ("alpha", "file:///same") => Some(true),
                ("beta", "file:///same") => Some(false),
                _ => None,
            }
        });
        assert_eq!(classified, (2, 1));
        assert!(is_ui_resource_uri("ui://widget/app"));
        assert!(is_ui_resource_uri("UI://widget/app"));
        assert!(!is_ui_resource_uri("file:///widget"));
    }

    fn complete_resource(response: ReadResourceResponse) -> ReadResourceResult {
        match response {
            ReadResourceResponse::Complete(result) => result,
            ReadResourceResponse::InputRequired(_) => {
                panic!("local resource unexpectedly required input")
            }
            _ => panic!("unexpected resource response variant"),
        }
    }

    fn final_inline_script(html: &str) -> &str {
        html.rsplit_once("<script>")
            .and_then(|(_, tail)| tail.split_once("</script>"))
            .map(|(script, _)| script)
            .expect("final inline app script")
    }

    fn function_source<'a>(html: &'a str, start: &str, next: &str) -> &'a str {
        let tail = html.split_once(start).expect("function start").1;
        let body = tail.split_once(next).expect("next function").0;
        let start_offset = html.len() - tail.len() - start.len();
        let end_offset = start_offset + start.len() + body.len();
        &html[start_offset..end_offset]
    }

    fn run_node(script: &str) {
        // A file keeps the full browser fixture independent of Windows' much
        // smaller process command-line limit. Explicit CommonJS matches `-e`.
        let source = tempfile::Builder::new()
            .suffix(".cjs")
            .tempfile()
            .expect("create MCP App behavior test script");
        std::fs::write(source.path(), script).expect("write MCP App behavior test script");
        let output = Command::new("node")
            .arg(source.path())
            .output()
            .expect("node must be available for MCP App behavior tests");
        assert!(
            output.status.success(),
            "node behavior test failed:\nstdout: {}\nstderr: {}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }

    fn dom_harness(host_body: &str) -> String {
        format!(
            r#"
const all = new Map();
function makeElement(id) {{
  const attrs = new Map();
  const listeners = new Map();
  const element = {{
    id, value: "", textContent: "", className: "", innerHTML: "", disabled: false,
    scrollHeight: id === "shell" ? 700 : 600,
    getAttribute(name) {{ return attrs.has(name) ? attrs.get(name) : null; }},
    setAttribute(name, value) {{ attrs.set(name, String(value)); }},
    addEventListener(type, listener) {{
      if (!listeners.has(type)) listeners.set(type, new Set());
      listeners.get(type).add(listener);
    }},
    removeEventListener(type, listener) {{ listeners.get(type)?.delete(listener); }},
    dispatch(type, event = {{}}) {{
      event.preventDefault ||= () => {{}};
      for (const listener of [...(listeners.get(type) || [])]) listener(event);
    }},
    focus() {{}},
    getBoundingClientRect() {{ return {{ width: 800, height: 600 }}; }}
  }};
  all.set(id, element);
  return element;
}}
for (const id of ["form","name","target","resources","prompts","status","test","create","cancel","close","shell","list","refresh","filter","connected","enabled","attention"]) makeElement(id);
all.get("resources").setAttribute("aria-checked", "true");
all.get("prompts").setAttribute("aria-checked", "true");
const dialog = makeElement("dialog");
const windowListeners = new Map();
const parentWindow = {{}};
const window = {{
  parent: parentWindow,
  openai: null,
  addEventListener(type, listener) {{
    if (!windowListeners.has(type)) windowListeners.set(type, new Set());
    windowListeners.get(type).add(listener);
  }},
  removeEventListener(type, listener) {{ windowListeners.get(type)?.delete(listener); }},
  dispatch(type, event = {{}}) {{
    for (const listener of [...(windowListeners.get(type) || [])]) listener(event);
  }}
}};
const document = {{
  documentElement: {{ scrollHeight: 777 }},
  getElementById(id) {{ return all.get(id); }},
  querySelector(selector) {{ return selector === ".dialog" ? dialog : null; }}
}};
const history = {{ length: 1, back() {{}} }};
let frameId = 0;
const frames = new Map();
function requestAnimationFrame(callback) {{ const id = ++frameId; frames.set(id, callback); return id; }}
function cancelAnimationFrame(id) {{ frames.delete(id); }}
function flushFrames() {{ const queued = [...frames.values()]; frames.clear(); for (const callback of queued) callback(); }}
class ResizeObserver {{ constructor(callback) {{ this.callback = callback; }} observe() {{}} disconnect() {{}} }}
const host = {{ {host_body} }};
window.LabbyAppHost = host;
Object.assign(globalThis, {{ window, document, history, requestAnimationFrame, cancelAnimationFrame, ResizeObserver }});
"#
        )
    }

    #[cfg(feature = "skills")]
    fn instrumented_skill_library_script() -> String {
        let script = final_inline_script(SKILL_LIBRARY_APP_FALLBACK_HTML);
        script.replacen(
            "})();",
            "globalThis.__skillTest={state,recovery,requestKey,deterministicIdempotencyKey,canonicalIntent,intentDigest,mutationAlreadySatisfied,authoredMutationAlreadySatisfied,matchesCommittedCreate,findCommittedCreate,loadList,selectSkill,loadHistory,loadFile,edit,mutate,mutationParams,openWorkspace,closeWorkspace,renderList,renderDetail,renderEditor,validate,save,command,resize,deriveViewModel,receiveContract,newDraft,validationFeedback};})();",
            1,
        )
    }

    #[cfg(feature = "skills")]
    fn skill_library_node_harness(test_body: &str) -> String {
        let instrumented = instrumented_skill_library_script();
        format!(
            r#"
const elements = new Map();
function element(id) {{
  if (elements.has(id)) return elements.get(id);
  const attributes = new Map();
  const listeners = new Map();
  const node = {{
    id, hidden: false, disabled: false, value: "", textContent: "", innerHTML: "",
    className: "", dataset: {{}},
    classList: {{ add() {{}}, remove() {{}}, toggle() {{}} }},
    setAttribute(name, value) {{ attributes.set(name, String(value)); }},
    getAttribute(name) {{ return attributes.get(name) ?? null; }},
    addEventListener(type, fn) {{ listeners.set(type, fn); }},
    removeEventListener(type) {{ listeners.delete(type); }},
    focus() {{ globalThis.__focused = id; }},
    matches(selector) {{ return selector.split(",").some(x => x.trim().slice(1) === id); }},
    closest() {{ return null; }},
    getBoundingClientRect() {{ return {{ width: 760, height: 480 }}; }}
  }};
  elements.set(id, node);
  return node;
}}
for (const id of ["app","workspace","main","skillList","status","expand","summary","quickCreate","browse","search","prevPage","nextPage","newSkill"]) element(id);
element("workspace").hidden = true;
const document = {{
  getElementById: element,
  querySelector() {{ return null; }}
}};
globalThis.__host ||= {{ hasBridge:()=>false, requestResize:()=>{{}}, requestTeardown:()=>{{}}, callAction:async()=>({{}}) }};
const windowListeners = new Map();
const parentWindow = {{ postMessage(message, origin) {{ globalThis.__parentMessages ||= []; globalThis.__parentMessages.push({{message,origin}}); }} }};
const window = {{
  parent: parentWindow,
  LabbyAppHost: globalThis.__host,
  addEventListener(type, fn) {{ windowListeners.set(type, fn); }},
  removeEventListener(type) {{ windowListeners.delete(type); }}
}};
let raf = 0;
function requestAnimationFrame(fn) {{ raf += 1; fn(); return raf; }}
const confirm = () => true;
Object.assign(globalThis, {{ document, window, requestAnimationFrame, confirm }});
{instrumented}
(async () => {{
{test_body}
}})().catch(error => {{ console.error(error); process.exitCode = 1; }});
"#
        )
    }

    struct UpstreamUiResourceServer;

    impl ServerHandler for UpstreamUiResourceServer {
        fn get_info(&self) -> ServerInfo {
            ServerInfo::new(
                ServerCapabilities::builder()
                    .enable_resources()
                    .enable_completions()
                    .build(),
            )
        }

        async fn list_resources(
            &self,
            _request: Option<PaginatedRequestParams>,
            _context: RequestContext<RoleServer>,
        ) -> Result<ListResourcesResult, ErrorData> {
            Ok(ListResourcesResult::with_all_items(vec![
                Resource::new(UPSTREAM_UI_URI, "quick-shell/app")
                    .with_mime_type("text/html;profile=mcp-app"),
            ]))
        }

        async fn list_resource_templates(
            &self,
            _request: Option<PaginatedRequestParams>,
            _context: RequestContext<RoleServer>,
        ) -> Result<ListResourceTemplatesResult, ErrorData> {
            Ok(ListResourceTemplatesResult::with_all_items(vec![
                ResourceTemplate::new("file:///{path}", "widget"),
            ]))
        }

        async fn complete(
            &self,
            request: CompleteRequestParams,
            _context: RequestContext<RoleServer>,
        ) -> Result<CompleteResult, ErrorData> {
            assert_eq!(request.r#ref.as_resource_uri(), Some("file:///{path}"));
            Ok(CompleteResult::new(
                CompletionInfo::with_pagination(
                    vec![format!("{}-completion", request.argument.value)],
                    Some(1),
                    false,
                )
                .expect("valid completion fixture"),
            ))
        }

        async fn read_resource(
            &self,
            params: ReadResourceRequestParams,
            _context: RequestContext<RoleServer>,
        ) -> Result<ReadResourceResponse, ErrorData> {
            if params.uri != UPSTREAM_UI_URI {
                return Err(ErrorData::resource_not_found(
                    format!("unknown upstream UI resource: {}", params.uri),
                    None,
                ));
            }

            let mut result = ReadResourceResult::new(vec![
                ResourceContents::text("<main>quick shell widget</main>", params.uri)
                    .with_mime_type("text/html;profile=mcp-app"),
            ]);
            result.result_type = None;
            Ok(result.into())
        }
    }

    /// Turn the Code Mode MCP App off through the authority a manager-backed
    /// server actually reads.
    async fn disable_code_mode_ui(server: &LabMcpServer) {
        let manager = server
            .gateway_manager
            .as_ref()
            .expect("code mode test server is manager-backed");
        let mut config = manager.current_config().await;
        config.code_mode.mcp_ui_enabled = false;
        manager.seed_config_unchecked_for_tests(config).await;
    }

    async fn code_mode_server() -> LabMcpServer {
        code_mode_server_with_scope(crate::mcp::route_scope::McpRouteScope::Root).await
    }

    async fn code_mode_server_with_scope(
        route_scope: crate::mcp::route_scope::McpRouteScope,
    ) -> LabMcpServer {
        let runtime = crate::dispatch::gateway::manager::GatewayRuntimeHandle::default();
        let manager = Arc::new(
            crate::dispatch::gateway::config_store::test_gateway_manager(
                std::path::PathBuf::from("config.toml"),
                runtime,
            ),
        );
        manager
            .seed_config_unchecked_for_tests(
                crate::config::LabConfig {
                    code_mode: crate::config::CodeModeConfig {
                        enabled: true,
                        mcp_ui_enabled: true,
                        ..crate::config::CodeModeConfig::default()
                    },
                    mcp_apps: crate::config::McpAppsConfig {
                        manager: true,
                        add_server: true,
                        server_logs: true,
                        gateway_status: true,
                        settings: true,
                    },
                    ..crate::config::LabConfig::default()
                }
                .to_gateway_config(),
            )
            .await;
        let code_mode_app_state = manager.code_mode_app_state();
        LabMcpServer {
            registry: Arc::new(crate::registry::ToolRegistry::new()),
            access_runtime: Arc::new(crate::access::AccessRuntime::blocked_unavailable()),
            file_stash_runtime: Arc::new(crate::file_stash::FileStashRuntime::blocked()),
            gateway_manager: Some(manager),
            peers: Default::default(),
            code_mode_app_state,
            last_listed_tool_contract: Default::default(),
            route_runtime: Default::default(),
            client_registry: Default::default(),
            transport_label: "test",
            logging_level: Arc::new(std::sync::atomic::AtomicU8::new(
                crate::mcp::logging::logging_level_rank(LoggingLevel::Emergency),
            )),
            route_scope,
            relay_session_id: 0,
            code_mode_widget_callbacks_enabled_for_test: false,
        }
    }

    async fn resource_scope_server(
        route_scope: crate::mcp::route_scope::McpRouteScope,
    ) -> LabMcpServer {
        let mut server = code_mode_server_with_scope(route_scope).await;
        server.registry = Arc::new(crate::registry::build_default_registry());
        server
    }

    async fn code_mode_server_with_upstream_ui_resource() -> LabMcpServer {
        static ACTIONS: &[labby_primitives::action::ActionSpec] =
            &[labby_primitives::action::ActionSpec {
                name: "terminal.open",
                description: "Open terminal",
                destructive: false,
                requires_admin: false,
                params: &[],
                returns: "object",
            }];

        let mut registry = crate::registry::ToolRegistry::new();
        registry.register(crate::registry::RegisteredService {
            name: "quick_shell",
            description: "Quick shell",
            category: "test",
            kind: crate::registry::RegisteredServiceKind::BootstrapOperator,
            status: "available",
            actions: ACTIONS,
            dispatch: noop_dispatch,
        });

        let connector: InProcessConnector = Arc::new(|service| {
            let future: BoxFuture<'static, anyhow::Result<InProcessRegistration>> =
                Box::pin(async move {
                    let upstream_name: Arc<str> = Arc::from(service.service_name());
                    let mut tool = Tool::new(
                        UPSTREAM_UI_TOOL_NAME.to_string(),
                        "Quick shell UI",
                        Arc::new(serde_json::Map::new()),
                    );
                    tool.meta = Some(MetaObject(serde_json::Map::from_iter([(
                        "ui".to_string(),
                        json!({ "resourceUri": UPSTREAM_UI_URI }),
                    )])));
                    Ok(InProcessRegistration {
                        connection: Some(upstream_ui_connection().await),
                        tools: vec![tool],
                        entry_name: Arc::clone(&upstream_name),
                        upstream_name: upstream_name.to_string(),
                    })
                });
            future
        });

        let pool = Arc::new(UpstreamPool::new().with_in_process_connector(connector));
        pool.register_in_process_service_peers(&registry).await;
        pool.list_upstream_resources().await;

        let runtime = crate::dispatch::gateway::manager::GatewayRuntimeHandle::default();
        runtime.swap(Some(pool)).await;
        let manager = Arc::new(
            crate::dispatch::gateway::config_store::test_gateway_manager(
                std::path::PathBuf::from("config.toml"),
                runtime,
            ),
        );
        manager
            .seed_config_unchecked_for_tests(
                crate::config::LabConfig {
                    code_mode: crate::config::CodeModeConfig {
                        enabled: true,
                        ..crate::config::CodeModeConfig::default()
                    },
                    upstream: vec![crate::config::UpstreamConfig {
                        enabled: true,
                        name: "quick_shell".to_string(),
                        url: None,
                        transport: None,
                        socket_path: None,
                        headers: Default::default(),
                        bearer_token_env: None,
                        command: Some("in-process".to_string()),
                        args: Vec::new(),
                        env: BTreeMap::new(),
                        proxy_resources: true,
                        proxy_prompts: false,
                        expose_tools: None,
                        expose_resources: None,
                        expose_prompts: None,
                        proxy_skills: false,
                        expose_skills: None,
                        code_mode_hint: None,
                        oauth: None,
                        imported_from: None,
                        priority: 1.0,
                    }],
                    ..crate::config::LabConfig::default()
                }
                .to_gateway_config(),
            )
            .await;

        LabMcpServer {
            registry: Arc::new(registry),
            access_runtime: Arc::new(crate::access::AccessRuntime::blocked_unavailable()),
            file_stash_runtime: Arc::new(crate::file_stash::FileStashRuntime::blocked()),
            gateway_manager: Some(manager),
            peers: Default::default(),
            code_mode_app_state: Default::default(),
            last_listed_tool_contract: Default::default(),
            route_runtime: Default::default(),
            client_registry: Default::default(),
            transport_label: "test",
            logging_level: Arc::new(std::sync::atomic::AtomicU8::new(
                crate::mcp::logging::logging_level_rank(LoggingLevel::Emergency),
            )),
            route_scope: crate::mcp::route_scope::McpRouteScope::Root,
            relay_session_id: 0,
            code_mode_widget_callbacks_enabled_for_test: false,
        }
    }

    async fn upstream_ui_connection() -> UpstreamConnection {
        let (server_transport, client_transport) = tokio::io::duplex(256 * 1024);
        let server_task = tokio::spawn(async move {
            let running = UpstreamUiResourceServer
                .serve(server_transport)
                .await
                .expect("upstream UI server starts");
            running.waiting().await.expect("upstream UI server runs");
        });
        let client_service: rmcp::service::RunningService<RoleClient, ()> = ()
            .serve(client_transport)
            .await
            .expect("upstream UI client starts");
        let peer = client_service.peer().clone();
        UpstreamConnection::new(
            client_service,
            Some(server_task),
            peer,
            UpstreamRuntimeMetadata::default(),
        )
    }

    fn noop_dispatch(
        _action: String,
        _params: Value,
    ) -> Pin<Box<dyn Future<Output = Result<Value, crate::dispatch::error::ToolError>> + Send>>
    {
        Box::pin(async { Ok(Value::Null) })
    }

    fn large_resource_server(service_count: usize) -> LabMcpServer {
        let mut registry = crate::registry::ToolRegistry::new();
        // Keep pagination offsets independent of process-wide Code Mode state.
        let code_mode_app_state = crate::mcp::catalog::CodeModeAppState::default();
        code_mode_app_state.set_enabled(false);
        static ACTIONS: &[labby_primitives::action::ActionSpec] =
            &[labby_primitives::action::ActionSpec {
                name: "thing.list",
                description: "List things",
                destructive: false,
                requires_admin: false,
                params: &[],
                returns: "object",
            }];
        for index in 0..service_count {
            let name = Box::leak(format!("resource_service_{index:03}").into_boxed_str());
            registry.register(crate::registry::RegisteredService {
                name,
                description: "Synthetic service",
                category: "test",
                kind: crate::registry::RegisteredServiceKind::BootstrapOperator,
                status: "available",
                actions: ACTIONS,
                dispatch: noop_dispatch,
            });
        }
        LabMcpServer {
            registry: Arc::new(registry),
            access_runtime: Arc::new(crate::access::AccessRuntime::blocked_unavailable()),
            file_stash_runtime: Arc::new(crate::file_stash::FileStashRuntime::blocked()),
            gateway_manager: None,
            peers: Default::default(),
            code_mode_app_state,
            last_listed_tool_contract: Default::default(),
            route_runtime: Default::default(),
            client_registry: Default::default(),
            transport_label: "test",
            logging_level: Arc::new(std::sync::atomic::AtomicU8::new(
                crate::mcp::logging::logging_level_rank(LoggingLevel::Emergency),
            )),
            route_scope: crate::mcp::route_scope::McpRouteScope::Root,
            relay_session_id: 0,
            code_mode_widget_callbacks_enabled_for_test: false,
        }
    }

    fn scoped_context(peer: Peer<RoleServer>, scopes: &[&str]) -> RequestContext<RoleServer> {
        let mut context = RequestContext::new(rmcp::model::NumberOrString::Number(1), peer);
        let mut parts = axum::http::Request::new(()).into_parts().0;
        parts
            .extensions
            .insert(labby_auth::auth_context::AuthContext {
                sub: "reader".to_string(),
                actor_key: None,
                scopes: scopes.iter().map(|scope| scope.to_string()).collect(),
                issuer: "https://lab.example.com".to_string(),
                via_session: true,
                csrf_token: None,
                email: None,
            });
        context.extensions.insert(parts);
        context
    }

    #[test]
    fn code_mode_app_resource_meta_uses_mcp_app_mime_and_csp() {
        let meta = code_mode_app_resource_meta(CODE_MODE_APP_URI);
        assert_eq!(
            meta.0["ui"]["resourceUri"].as_str(),
            Some(CODE_MODE_APP_URI)
        );
        assert_eq!(
            meta.0["ui"]["mimeTypes"][0].as_str(),
            Some(CODE_MODE_APP_MIME)
        );
        assert_eq!(meta.0["ui"]["prefersBorder"].as_bool(), Some(false));
        assert!(meta.0.get("csp").is_none(), "CSP belongs under _meta.ui");
        assert!(
            meta.0.get("prefersBorder").is_none(),
            "border preference belongs under _meta.ui"
        );
        assert_eq!(meta.0["ui"]["csp"]["connectDomains"], json!([]));
        assert_eq!(meta.0["ui"]["csp"]["resourceDomains"], json!([]));
        assert_eq!(meta.0["ui"]["csp"]["frameDomains"], json!([]));
    }

    #[tokio::test]
    async fn contract_schema_resources_are_listed_and_readable() {
        let (transport, _client_transport) = tokio::io::duplex(64);
        let running = rmcp::service::serve_directly::<RoleServer, _, _, std::io::Error, _>(
            code_mode_server().await,
            transport,
            None,
        );

        // Listed for in-band discovery — the contract is documentation, so it
        // is advertised at the same visibility as `lab://catalog`.
        let listed = running
            .service()
            .list_resources_impl(None, scoped_context(running.peer().clone(), &["lab:read"]))
            .await
            .expect("list resources");
        for uri in [AGENT_ERROR_CONTRACT_URI, CODE_MODE_CALL_ERROR_CONTRACT_URI] {
            let resource = listed
                .resources
                .iter()
                .find(|resource| resource.uri == uri)
                .unwrap_or_else(|| panic!("contract resource {uri} must be listed"));
            assert_eq!(resource.mime_type.as_deref(), Some(CONTRACT_SCHEMA_MIME));
        }

        // Each reads back as the embedded, parseable JSON Schema.
        for (uri, expected_id) in [
            (AGENT_ERROR_CONTRACT_URI, "agent-error-v1.json"),
            (
                CODE_MODE_CALL_ERROR_CONTRACT_URI,
                "code-mode-call-error-v1.json",
            ),
        ] {
            let response = running
                .service()
                .read_resource_impl(
                    ReadResourceRequestParams::new(uri),
                    scoped_context(running.peer().clone(), &["lab:read"]),
                )
                .await
                .unwrap_or_else(|e| panic!("read {uri}: {e:?}"));
            let result = complete_resource(response);
            let ResourceContents::TextResourceContents { text, .. } = &result.contents[0] else {
                panic!("contract schema must be text content");
            };
            let schema: Value = serde_json::from_str(text).expect("schema must be valid JSON");
            assert!(
                schema["$id"]
                    .as_str()
                    .is_some_and(|id| id.ends_with(expected_id)),
                "unexpected $id for {uri}: {}",
                schema["$id"]
            );
        }
    }

    #[tokio::test]
    async fn list_resources_only_lists_code_mode_apps_for_read_scope() {
        let (transport, _client_transport) = tokio::io::duplex(64);
        let running = rmcp::service::serve_directly::<RoleServer, _, _, std::io::Error, _>(
            code_mode_server().await,
            transport,
            None,
        );

        let denied = running
            .service()
            .list_resources_impl(None, scoped_context(running.peer().clone(), &["profile"]))
            .await
            .expect("list resources without scope");
        assert!(
            denied
                .resources
                .iter()
                .all(|resource| !resource.uri.starts_with("ui://lab/code-mode/")),
            "listed Code Mode UI resources without read scope"
        );

        let allowed = running
            .service()
            .list_resources_impl(None, scoped_context(running.peer().clone(), &["lab:read"]))
            .await
            .expect("list resources with scope");
        let code_mode_uris = allowed
            .resources
            .iter()
            .filter(|resource| resource.uri.starts_with("ui://lab/code-mode/"))
            .map(|resource| resource.uri.clone())
            .collect::<Vec<_>>();
        // Advertised URIs carry the `?v=<hash>` cache-bust suffix; compare bases.
        assert_eq!(
            code_mode_uris
                .iter()
                .map(|uri| strip_app_version(uri))
                .collect::<Vec<_>>(),
            vec![CODE_MODE_APP_URI, CODE_MODE_HISTORY_APP_URI]
        );
        assert!(
            code_mode_uris.iter().all(|uri| uri.contains("?v=")),
            "advertised Code Mode URIs must carry a cache-bust token: {code_mode_uris:?}"
        );
    }

    #[tokio::test]
    async fn code_mode_passes_through_upstream_mcp_app_tool_and_resource() {
        let (transport, _client_transport) = tokio::io::duplex(64);
        let running = rmcp::service::serve_directly::<RoleServer, _, _, std::io::Error, _>(
            code_mode_server_with_upstream_ui_resource().await,
            transport,
            None,
        );
        let tool_context = scoped_context(running.peer().clone(), &["lab"]);
        let resource_context = scoped_context(running.peer().clone(), &["lab:read"]);

        let tools = running
            .service()
            .list_tools_impl(None, tool_context)
            .await
            .expect("list tools");
        assert!(
            tools
                .tools
                .iter()
                .all(|tool| tool.name.as_ref() != CODE_MODE_UI_TOOL_NAME),
            "Labby-owned Code Mode UI must stay opt-in by default"
        );
        assert!(
            tools
                .tools
                .iter()
                .any(|tool| tool.name.as_ref() == UPSTREAM_UI_TOOL_NAME),
            "upstream MCP App tools must pass through synthetic Code Mode"
        );

        let resources = running
            .service()
            .list_resources_impl(None, resource_context.clone())
            .await
            .expect("list resources");
        let uris = resources
            .resources
            .iter()
            .map(|resource| resource.uri.as_str())
            .collect::<Vec<_>>();
        assert!(
            uris.iter()
                .all(|uri| strip_app_version(uri) != CODE_MODE_APP_URI),
            "Labby-owned Code Mode app resource must stay hidden by default: {uris:?}"
        );
        assert!(
            uris.contains(&UPSTREAM_UI_URI),
            "upstream MCP UI resource should remain listed with its native URI: {uris:?}"
        );

        let templates = running
            .service()
            .list_resource_templates(None, resource_context.clone())
            .await
            .expect("list resource templates");
        assert_eq!(templates.ttl_ms, Some(0));
        assert_eq!(
            templates.cache_scope,
            Some(rmcp::model::CacheScope::Private)
        );
        assert_eq!(templates.resource_templates.len(), 1);
        assert_eq!(templates.resource_templates[0].name, "quick_shell/widget");
        assert_eq!(
            templates.resource_templates[0].uri_template,
            "lab://upstream/quick_shell/file:///{path}"
        );

        let completion = running
            .service()
            .complete(
                CompleteRequestParams::new(
                    Reference::for_resource("lab://upstream/quick_shell/file:///{path}"),
                    ArgumentInfo::new("path", "sys"),
                ),
                resource_context.clone(),
            )
            .await
            .expect("complete upstream resource template");
        assert_eq!(completion.completion.values, vec!["sys-completion"]);

        running
            .service()
            .read_resource_impl(
                ReadResourceRequestParams::new(CODE_MODE_APP_URI),
                resource_context.clone(),
            )
            .await
            .expect_err("disabled Labby-owned Code Mode UI resource must not be readable");

        let upstream_read = complete_resource(
            running
                .service()
                .read_resource_impl(
                    ReadResourceRequestParams::new(UPSTREAM_UI_URI),
                    resource_context,
                )
                .await
                .expect("read upstream UI resource"),
        );
        let ResourceContents::TextResourceContents {
            uri,
            text: upstream_html,
            ..
        } = &upstream_read.contents[0]
        else {
            panic!("expected upstream text resource");
        };
        assert_eq!(uri, UPSTREAM_UI_URI);
        assert!(upstream_html.contains("quick shell widget"));
        assert_eq!(
            upstream_read.result_type,
            Some(rmcp::model::ResultType::COMPLETE),
            "Labby must restore the current-protocol discriminator omitted by an older upstream"
        );
    }

    #[tokio::test]
    async fn list_resources_only_lists_server_logs_app_for_admin_scope() {
        let (transport, _client_transport) = tokio::io::duplex(64);
        let running = rmcp::service::serve_directly::<RoleServer, _, _, std::io::Error, _>(
            code_mode_server().await,
            transport,
            None,
        );

        let denied = running
            .service()
            .list_resources_impl(None, scoped_context(running.peer().clone(), &["lab:read"]))
            .await
            .expect("list resources without admin scope");
        assert!(
            denied
                .resources
                .iter()
                .all(|resource| !resource.uri.starts_with(SERVER_LOGS_APP_URI_PREFIX)),
            "listed server logs UI resources without admin scope"
        );

        let allowed = running
            .service()
            .list_resources_impl(None, scoped_context(running.peer().clone(), &["lab:admin"]))
            .await
            .expect("list resources with admin scope");
        let server_logs_uris = allowed
            .resources
            .iter()
            .filter(|resource| resource.uri.starts_with(SERVER_LOGS_APP_URI_PREFIX))
            .map(|resource| resource.uri.clone())
            .collect::<Vec<_>>();
        assert_eq!(
            server_logs_uris
                .iter()
                .map(|uri| strip_app_version(uri))
                .collect::<Vec<_>>(),
            vec![SERVER_LOGS_APP_URI]
        );
        assert!(
            server_logs_uris.iter().all(|uri| uri.contains("?v=")),
            "advertised server logs URI must carry a cache-bust token: {server_logs_uris:?}"
        );
    }

    #[cfg(feature = "skills")]
    #[tokio::test]
    async fn skill_library_app_list_and_read_follow_skill_visibility_and_scope() {
        let server = resource_scope_server(crate::mcp::route_scope::McpRouteScope::Root).await;
        let (transport, _client_transport) = tokio::io::duplex(64);
        let running = rmcp::service::serve_directly::<RoleServer, _, _, std::io::Error, _>(
            server, transport, None,
        );

        let denied = running
            .service()
            .list_resources_impl(None, scoped_context(running.peer().clone(), &["profile"]))
            .await
            .expect("list resources without read scope");
        assert!(
            denied
                .resources
                .iter()
                .all(|resource| { !resource.uri.starts_with(SKILL_LIBRARY_APP_URI_PREFIX) })
        );

        let allowed = running
            .service()
            .list_resources_impl(None, scoped_context(running.peer().clone(), &["lab:read"]))
            .await
            .expect("list resources with read scope");
        let listed = allowed
            .resources
            .iter()
            .filter(|resource| resource.uri.starts_with(SKILL_LIBRARY_APP_URI_PREFIX))
            .collect::<Vec<_>>();
        assert_eq!(listed.len(), 1, "only the MCP Apps projection is listed");
        assert_eq!(strip_app_version(&listed[0].uri), SKILL_LIBRARY_APP_URI);
        assert!(listed[0].uri.contains("?v="));

        let err = running
            .service()
            .read_resource_impl(
                ReadResourceRequestParams::new(SKILL_LIBRARY_APP_URI),
                scoped_context(running.peer().clone(), &["profile"]),
            )
            .await
            .expect_err("read without scope must be denied");
        assert_eq!(
            err.data.as_ref().expect("error data")["kind"],
            json!("forbidden")
        );

        let result = complete_resource(
            running
                .service()
                .read_resource_impl(
                    ReadResourceRequestParams::new(listed[0].uri.clone()),
                    scoped_context(running.peer().clone(), &["lab:read"]),
                )
                .await
                .expect("authorized versioned app read"),
        );
        let ResourceContents::TextResourceContents {
            text,
            mime_type,
            meta,
            ..
        } = &result.contents[0]
        else {
            panic!("expected text app resource");
        };
        assert_eq!(mime_type.as_deref(), Some(CODE_MODE_APP_MIME));
        assert!(text.contains("MCP_PROTOCOL_VERSION = \"2026-01-26\""));
        assert!(text.contains("window.__LABBY_MCP_RESOURCE=true;"));
        let ui = &meta.as_ref().expect("app metadata").0["ui"];
        assert_eq!(ui["resourceUri"], listed[0].uri);
        assert_eq!(ui["csp"]["connectDomains"], json!([]));
        assert_eq!(ui["csp"]["resourceDomains"], json!([]));
        assert_eq!(ui["csp"]["frameDomains"], json!([]));

        let skybridge_uri = skill_library_app_skybridge_uri_for_tool("artifacts")
            .expect("versioned Skill Library Skybridge URI");
        let skybridge = complete_resource(
            running
                .service()
                .read_resource_impl(
                    ReadResourceRequestParams::new(skybridge_uri.clone()),
                    scoped_context(running.peer().clone(), &["lab:read"]),
                )
                .await
                .expect("authorized Skybridge app read"),
        );
        let ResourceContents::TextResourceContents {
            text,
            mime_type,
            meta,
            ..
        } = &skybridge.contents[0]
        else {
            panic!("expected text Skybridge resource");
        };
        assert_eq!(mime_type.as_deref(), Some(CODE_MODE_APP_SKYBRIDGE_MIME));
        assert!(text.contains("window.openai"));
        let meta = &meta.as_ref().expect("Skybridge metadata").0;
        assert_eq!(meta["ui"]["resourceUri"], skybridge_uri);
        assert_eq!(
            meta["ui"]["mimeTypes"],
            json!([CODE_MODE_APP_SKYBRIDGE_MIME])
        );
        assert!(meta["openai/widgetDescription"].is_string());
    }

    #[cfg(feature = "skills")]
    #[tokio::test]
    async fn skill_library_resource_errors_expose_only_safe_correlation() {
        let server = resource_scope_server(crate::mcp::route_scope::McpRouteScope::Root).await;
        let (transport, _client_transport) = tokio::io::duplex(64);
        let running = rmcp::service::serve_directly::<RoleServer, _, _, std::io::Error, _>(
            server, transport, None,
        );

        let mut accepted = scoped_context(running.peer().clone(), &["profile"]);
        accepted
            .extensions
            .get_mut::<axum::http::request::Parts>()
            .expect("request parts")
            .headers
            .insert("x-request-id", "client-safe-42".parse().expect("header"));
        let accepted_error = running
            .service()
            .read_resource_impl(
                ReadResourceRequestParams::new(SKILL_LIBRARY_APP_URI),
                accepted,
            )
            .await
            .expect_err("missing read scope");
        let accepted_data = accepted_error.data.expect("structured resource error");
        assert_eq!(accepted_data["kind"], "forbidden");
        assert_eq!(accepted_data["correlation_id"], "client-safe-42");

        let hostile = "../../secret-authorization-value";
        let mut rejected = scoped_context(running.peer().clone(), &["profile"]);
        rejected
            .extensions
            .get_mut::<axum::http::request::Parts>()
            .expect("request parts")
            .headers
            .insert("x-request-id", hostile.parse().expect("header"));
        let rejected_error = running
            .service()
            .read_resource_impl(
                ReadResourceRequestParams::new(SKILL_LIBRARY_APP_URI),
                rejected,
            )
            .await
            .expect_err("unsafe correlation is replaced");
        let rendered = format!("{rejected_error:?}");
        let rejected_data = rejected_error.data.expect("structured resource error");
        assert_eq!(rejected_data["kind"], "forbidden");
        assert!(
            rejected_data["correlation_id"]
                .as_str()
                .is_some_and(|value| value.starts_with("mcp-skill-resource-"))
        );
        assert!(!rendered.contains(hostile));
        assert!(!rejected_data.to_string().contains(hostile));
    }

    #[cfg(feature = "skills")]
    #[tokio::test]
    async fn protected_scope_hides_skill_library_app_when_skills_service_is_not_allowed() {
        let server =
            resource_scope_server(crate::mcp::route_scope::McpRouteScope::protected_subset(
                "ops",
                std::iter::empty::<&str>(),
                ["gateway"],
                false,
            ))
            .await;
        let (transport, _client_transport) = tokio::io::duplex(64);
        let running = rmcp::service::serve_directly::<RoleServer, _, _, std::io::Error, _>(
            server, transport, None,
        );
        let context = scoped_context(running.peer().clone(), &["lab:read"]);

        let resources = running
            .service()
            .list_resources_impl(None, context.clone())
            .await
            .expect("list protected resources");
        assert!(
            resources
                .resources
                .iter()
                .all(|resource| { !resource.uri.starts_with(SKILL_LIBRARY_APP_URI_PREFIX) })
        );
        let err = running
            .service()
            .read_resource_impl(
                ReadResourceRequestParams::new(SKILL_LIBRARY_APP_URI),
                context,
            )
            .await
            .expect_err("cached read must not bypass protected service selection");
        assert!(err.message.contains("unknown UI resource"));
    }

    #[cfg(feature = "skills")]
    #[test]
    fn skill_library_registration_is_versioned_bounded_and_runtime_specific() {
        let app = skill_library_app();
        let mcp_uri = skill_library_app_resource_uri_for_tool("artifacts")
            .expect("MCP Apps Artifact Library URI");
        let skybridge_uri = skill_library_app_skybridge_uri_for_tool("artifacts")
            .expect("Skybridge Skill Library URI");
        assert!(mcp_uri.starts_with(SKILL_LIBRARY_APP_URI));
        assert!(skybridge_uri.starts_with(SKILL_LIBRARY_APP_SKYBRIDGE_URI));
        assert!(mcp_uri.contains("?v="));
        assert!(skybridge_uri.contains("?v="));
        assert!(skill_library_app_resource_uri_for_tool("other").is_none());

        let mcp = app.descriptor(&mcp_uri).expect("versioned MCP descriptor");
        let skybridge = app
            .descriptor(&skybridge_uri)
            .expect("versioned Skybridge descriptor");
        assert_eq!(mcp.runtime.mime(), CODE_MODE_APP_MIME);
        assert_eq!(skybridge.runtime.mime(), CODE_MODE_APP_SKYBRIDGE_MIME);
        assert!(mcp.runtime != skybridge.runtime);

        let changed = bridged_app_content_version(&format!(
            "{SKILL_LIBRARY_APP_FALLBACK_HTML}<!-- changed -->"
        ));
        assert_ne!(changed, *SKILL_LIBRARY_APP_VERSION);

        let shell = SKILL_LIBRARY_APP_FALLBACK_HTML;
        assert!(shell.contains("/apps/assets/labby-app-host.js"));
        assert!(shell.contains("const host=window.LabbyAppHost"));
        assert!(shell.contains("host.hasBridge()"));
        for forbidden in ["fetch(", "Authorization", "localStorage", "document.cookie"] {
            assert!(!shell.contains(forbidden), "shell contains `{forbidden}`");
        }
    }

    #[cfg(feature = "skills")]
    #[test]
    fn skill_library_app_projects_truthful_lifecycle_and_action_recovery() {
        run_node(&skill_library_node_harness(
            r#"
const t=globalThis.__skillTest;
const assert=(value,message)=>{if(!value)throw new Error(message)};
const summary={active_revision_id:"r2",latest_revision_id:"r3",visibility:"shared"};
assert(t.deriveViewModel(summary).lifecycle==="Active","active server truth");
assert(t.deriveViewModel({...summary,archived:true}).lifecycle==="Archived","archive wins");
assert(t.deriveViewModel({...summary,active_revision_id:null}).lifecycle==="Deactivated","deactivated truth");
assert(t.recovery("stale_version").toLowerCase().includes("truth"),"stale recovery reloads truth");
assert(t.recovery("validation_failed").includes("validate again"),"validation recovery");
assert(t.recovery("forbidden").includes("owner"),"authorization recovery");
assert(t.recovery("publish_failed").includes("publication"),"publication recovery");
let resized=0;globalThis.__host.requestResize=()=>resized++;
t.openWorkspace();assert(globalThis.__focused==="main","expanded view receives focus");
t.closeWorkspace();assert(globalThis.__focused==="quickCreate","collapsed card restores focus");
const retryKey=t.requestKey("artifacts.save","new");
windowListeners.get("message")({source:parentWindow,data:{method:"ui/resource-teardown",id:41}});
assert(t.state.disposed,"teardown aborts in-flight rendering");
assert(t.state.pending.get("artifacts.save:new")===retryKey,"teardown retains ambiguous retry identity");
assert(resized>0,"host resize was invoked");
assert(globalThis.__parentMessages[0].message.id===41&&globalThis.__parentMessages[0].origin==="*","teardown was acknowledged");
"#,
        ));
    }

    #[cfg(feature = "skills")]
    #[test]
    fn skill_library_app_pages_bounded_catalog_and_history_to_cap_plus_one() {
        let body = r##"
const t=globalThis.__skillTest;
for(let offset=0;offset<=10000;offset+=50)await t.loadList(String(offset));
const listCalls=globalThis.__calls.filter(x=>x.action==="artifacts.list");
if(listCalls.length!==201)throw new Error(`expected 200 pages plus cap probe, got ${listCalls.length}`);
if(listCalls.some(x=>x.service!=="artifacts"||x.params.limit!==50))throw new Error("unbounded or wrong host wiring");
if(t.state.items[0].artifact_id!=="item-10000"||t.state.next!==null)throw new Error("cap+1 page truth lost");
t.state.detail={artifact_id:"skill-1"};
await t.loadHistory("skill-1",null);await t.loadHistory("skill-1","50");
const historyCalls=globalThis.__calls.filter(x=>x.action==="artifacts.history");
if(historyCalls.length!==2||historyCalls.some(x=>x.params.limit!==50))throw new Error("history pagination not bounded");
if(t.state.history.length!==2)throw new Error("history continuation replaced prior revisions");
"##;
        let harness = skill_library_node_harness(body);
        let host = r##"
globalThis.__calls=[];
globalThis.__host={hasBridge:()=>false,requestResize:()=>{},requestTeardown:()=>{},callAction:async(service,action,params)=>{
  globalThis.__calls.push({service,action,params});
  if(action==="artifacts.list")return {items:[{artifact_id:`item-${params.cursor}`,name:"skill"}],next_cursor:params.cursor==="10000"?null:String(Number(params.cursor)+50),library_version:91};
  if(action==="artifacts.history")return {items:[{revision_id:`r-${params.cursor??0}`}],next_cursor:params.cursor==="10000"?null:String(Number(params.cursor??0)+50)};
  throw new Error(action);
}};
"##;
        run_node(&format!("{host}\n{harness}"));
    }

    #[cfg(feature = "skills")]
    #[test]
    fn skill_library_app_reuses_idempotency_and_discards_stale_responses() {
        let body = r#"
const t=globalThis.__skillTest;t.state.selected={artifact_id:"skill-1"};t.state.detail={artifact_id:"skill-1",revision_id:"r2"};
 t.state.libraryVersion=12;
const first=t.requestKey("artifacts.activate","skill-1"),again=t.requestKey("artifacts.activate","skill-1");
if(first!==again)throw new Error("retry changed idempotency key");
globalThis.__failMutation=true;
await t.mutate("artifacts.activate",t.mutationParams("artifacts.activate","r2"),"active");
if(t.requestKey("artifacts.activate","skill-1")!==first)throw new Error("ambiguous failure discarded idempotency key");
globalThis.__failMutation=false;
await t.mutate("artifacts.activate",t.mutationParams("artifacts.activate","r2"),"active");
if(t.requestKey("artifacts.activate","skill-1")!==first)throw new Error("deterministic fallback changed after host state cleared");
globalThis.__deferLists=true;
const old=t.loadList("old"),fresh=t.loadList("fresh");
globalThis.__resolvers[1]({items:[{artifact_id:"fresh",name:"fresh"}],next_cursor:null,library_version:14});await fresh;
globalThis.__resolvers[0]({items:[{artifact_id:"stale",name:"stale"}],next_cursor:null,library_version:13});await old;
if(t.state.items[0].artifact_id!=="fresh"||t.state.libraryVersion!==14)throw new Error("stale response overwrote server truth");
"#;
        let harness = skill_library_node_harness(body);
        let host = r#"
globalThis.__calls=[];globalThis.__resolvers=[];
globalThis.__host={hasBridge:()=>false,requestResize:()=>{},requestTeardown:()=>{},callAction:(service,action,params)=>{
  globalThis.__calls.push({service,action,params});
  if(action==="artifacts.activate"&&globalThis.__failMutation)return Promise.reject(Object.assign(new Error("ambiguous transport result"),{kind:"conflict"}));
  if(action==="artifacts.list"&&globalThis.__deferLists)return new Promise(resolve=>globalThis.__resolvers.push(resolve));
  return Promise.resolve({items:[],next_cursor:null,library_version:12,committed_library_version:12,published_library_version:12});
}};
"#;
        run_node(&format!("{host}\n{harness}"));
    }

    #[cfg(feature = "skills")]
    #[test]
    fn skill_library_app_accessibility_resize_teardown_and_secret_boundary_are_explicit() {
        let html = SKILL_LIBRARY_APP_FALLBACK_HTML;
        for marker in [
            "aria-live=\"polite\"",
            "role=\"listbox\"",
            "role=\"option\"",
            "aria-current=",
            "aria-selected=",
            "main.focus()",
            "$(\"quickCreate\").focus()",
            "min-height:44px",
            "@media(max-width:620px)",
            "@media(prefers-reduced-motion:no-preference)",
            "host.requestResize",
            "ui/resource-teardown",
            "state.disposed=true",
            "Object.keys(state.seq).forEach",
            "postMessage({jsonrpc:\"2.0\",id:data.id,result:{}},\"*\")",
            "host.callAction(\"artifacts\",action,params)",
            "relist_required",
            "published_library_version",
        ] {
            assert!(
                html.contains(marker),
                "Skill Library app missing `{marker}`"
            );
        }
        for forbidden in [
            "fetch(",
            "XMLHttpRequest",
            "WebSocket",
            "localStorage",
            "sessionStorage",
            "indexedDB",
            "document.cookie",
            "Authorization",
            "Bearer ",
            "/home/",
            "file://",
            "process.env",
            "SECRET_CANARY_DO_NOT_RENDER",
        ] {
            assert!(
                !html.contains(forbidden),
                "app crossed boundary with `{forbidden}`"
            );
        }
    }

    #[cfg(feature = "skills")]
    #[test]
    fn skill_library_selection_races_are_bound_to_selection_and_revision() {
        let body = r#"
const t=globalThis.__skillTest;
t.state.items=[
  {artifact_id:"skill-a",name:"A",can_mutate:true},
  {artifact_id:"skill-b",name:"B",can_mutate:true}
];
const a=t.selectSkill("skill-a");
const b=t.selectSkill("skill-b");
globalThis.__gets.get("skill-b")({item:{artifact_id:"skill-b",name:"B",latest_revision_id:"b2",can_mutate:true},library_version:8});
await b;
globalThis.__gets.get("skill-a")({item:{artifact_id:"skill-a",name:"A",latest_revision_id:"a9",can_mutate:true},library_version:7});
await a;
if(t.state.selected?.artifact_id!=="skill-b"||t.state.detail?.artifact_id!=="skill-b")throw new Error("selection A overwrote selection B");
if(t.state.libraryVersion!==8)throw new Error("stale selection lowered authoritative version");
await t.command({dataset:{command:"activate"}});
const activation=globalThis.__calls.find(x=>x.action==="artifacts.activate");
if(activation?.params.expected_revision_id!=="b2")throw new Error("activation did not target selected latest revision");
if(activation?.params.artifact_id!=="skill-b")throw new Error("activation crossed the selected artifact guard");
"#;
        let harness = skill_library_node_harness(body);
        let host = r#"
globalThis.__calls=[];globalThis.__gets=new Map();
globalThis.__host={hasBridge:()=>false,requestResize:()=>{},requestTeardown:()=>{},callAction:(service,action,params)=>{
  globalThis.__calls.push({service,action,params});
  if(action==="artifacts.get")return new Promise(resolve=>globalThis.__gets.set(params.artifact_id,resolve));
  if(action==="artifacts.activate")return Promise.resolve({committed_library_version:9,published_library_version:9,new_generation:3});
  if(action==="artifacts.list")return Promise.resolve({items:[{artifact_id:"skill-b",name:"B",latest_revision_id:"b2",can_mutate:true}],next_cursor:null,library_version:9});
  if(action==="artifacts.history")return Promise.resolve({items:[],next_cursor:null,library_version:9});
  throw new Error(action);
}};
"#;
        run_node(&format!("{host}\n{harness}"));
    }

    #[cfg(feature = "skills")]
    #[test]
    fn skill_library_editor_preserves_manifest_and_lazy_loads_each_file() {
        let body = r#"
const t=globalThis.__skillTest;
t.state.selected={artifact_id:"multi",name:"Multi",can_mutate:true};
t.state.detail={artifact_id:"multi",name:"Multi",latest_revision_id:"r7",can_mutate:true,files:[{path:"SKILL.md"},{path:"references/guide.md"},{path:"scripts/check.sh"}]};
await t.edit();
if(t.state.files.map(x=>x.path).join(",")!=="SKILL.md,references/guide.md,scripts/check.sh")throw new Error("revision manifest was not preserved");
if(globalThis.__reads.length!==1||globalThis.__reads[0].path!=="SKILL.md")throw new Error("editor did not lazily read only the initial file");
t.state.file=1;
await t.loadFile(1,"multi","r7");
if(globalThis.__reads.length!==2||globalThis.__reads[1].path!=="references/guide.md")throw new Error("supporting file was not lazily loaded");
if(t.state.files[2].content!==undefined)throw new Error("unselected file was eagerly populated");
"#;
        let harness = skill_library_node_harness(body);
        let host = r#"
globalThis.__reads=[];
globalThis.__host={hasBridge:()=>false,requestResize:()=>{},requestTeardown:()=>{},callAction:async(service,action,params)=>{
  if(action==="artifacts.read"){globalThis.__reads.push(params);return {path:params.path,text:`body:${params.path}`,library_version:21};}
  throw new Error(action);
}};
"#;
        run_node(&format!("{host}\n{harness}"));
    }

    #[cfg(feature = "skills")]
    #[test]
    fn skill_library_server_capabilities_drive_personal_and_shared_controls() {
        let body = r#"
const t=globalThis.__skillTest;
await t.loadList();
if(!t.state.capabilities.can_create_personal||!t.state.capabilities.can_create_shared)throw new Error("server creation capabilities were not retained");
if(!t.state.contract.canCreate)throw new Error("server creation capability was not projected");
if(!t.state.contract.visibilities.has("private")||!t.state.contract.visibilities.has("shared"))throw new Error("server personal/shared visibility contract drifted");
t.openWorkspace(true);
if(!globalThis.__lastHtml.includes("Personal library")||!globalThis.__lastHtml.includes("Shared Labby namespace"))throw new Error("personal/shared authoring choice is not rendered");
"#;
        let harness = skill_library_node_harness(body);
        let host = r#"
globalThis.__host={hasBridge:()=>false,requestResize:()=>{},requestTeardown:()=>{},callAction:async(service,action)=>{
  if(action==="artifacts.list")return {items:[],next_cursor:null,library_version:4,capabilities:{can_create_personal:true,can_create_shared:true,can_import:true,default_visibility:"personal",create_visibilities:["personal","shared"],actions:["artifacts.create"]}};
  throw new Error(action);
}};
"#;
        // The DOM shim records the generated editor markup through the main node.
        let harness = harness.replace(
            "id, hidden: false, disabled: false, value: \"\", textContent: \"\", innerHTML: \"\",",
            "id, hidden: false, disabled: false, value: \"\", textContent: \"\", _html: \"\", set innerHTML(value){this._html=value;if(id===\"main\")globalThis.__lastHtml=value}, get innerHTML(){return this._html},",
        );
        run_node(&format!("{host}\n{harness}"));
    }

    #[cfg(feature = "skills")]
    #[test]
    fn skill_library_versions_never_regress_and_definitive_errors_release_retry_keys() {
        let body = r#"
const t=globalThis.__skillTest;
t.state.libraryVersion=100;
await t.loadList();
if(t.state.libraryVersion!==100)throw new Error("older list response regressed the monotonic library version");
t.state.selected={artifact_id:"one"};t.state.detail={artifact_id:"one",latest_revision_id:"r2",can_mutate:true};
const params=t.mutationParams("artifacts.activate","r2");
const key=params.idempotency_key;
await t.mutate("artifacts.activate",params,"active");
if(t.requestKey("artifacts.activate","one")===key)throw new Error("definitive validation failure retained a consumed retry key");
"#;
        let harness = skill_library_node_harness(body);
        let host = r#"
globalThis.__host={hasBridge:()=>false,requestResize:()=>{},requestTeardown:()=>{},callAction:async(service,action)=>{
  if(action==="artifacts.list")return {items:[],next_cursor:null,library_version:90};
  if(action==="artifacts.activate")throw Object.assign(new Error("invalid revision"),{kind:"validation_failed",definitive:true});
  throw new Error(action);
}};
"#;
        run_node(&format!("{host}\n{harness}"));
    }

    #[cfg(feature = "skills")]
    #[test]
    fn skill_library_teardown_is_idempotent_and_keeps_ambiguous_reconciliation() {
        run_node(&skill_library_node_harness(
            r#"
const t=globalThis.__skillTest;
t.state.selected={artifact_id:"one"};
const key=t.requestKey("artifacts.save","one");
const teardown=windowListeners.get("message");
teardown({source:parentWindow,data:{method:"ui/resource-teardown",id:51}});
teardown({source:parentWindow,data:{method:"ui/resource-teardown",id:51}});
if(!t.state.disposed)throw new Error("teardown did not dispose the app");
if(t.state.pending.get("artifacts.save:one")!==key)throw new Error("ambiguous retry identity was discarded during teardown");
if(globalThis.__parentMessages.length!==2||globalThis.__parentMessages.some(x=>x.message.id!==51))throw new Error("repeated teardown was not safely acknowledged");
"#,
        ));
    }

    #[cfg(feature = "skills")]
    #[test]
    fn skill_library_keyboard_focus_touch_theme_and_motion_contract_is_explicit() {
        let html = SKILL_LIBRARY_APP_FALLBACK_HTML;
        for marker in [
            "ArrowDown",
            "ArrowUp",
            "Home",
            "End",
            "tabindex=\"${state.selected?.artifact_id===x.artifact_id?0:-1}\"",
            "focus()",
            "min-height:44px",
            "min-width:44px",
            "@media(max-width:620px)",
            "color-scheme:light dark",
            "prefers-color-scheme:dark",
            "prefers-reduced-motion:no-preference",
        ] {
            assert!(
                html.contains(marker),
                "Skill Library app missing `{marker}`"
            );
        }
    }

    #[cfg(feature = "skills")]
    #[test]
    fn skill_library_history_walks_ten_thousand_and_one_in_order_with_bounded_ui_state() {
        let body = r#"
const t=globalThis.__skillTest;
t.state.detail={artifact_id:"huge",latest_revision_id:"r10000",can_mutate:true};
let cursor=null;
do { await t.loadHistory("huge",cursor); cursor=t.state.historyNext; } while(cursor!==null);
if(globalThis.__historyCalls.length!==201)throw new Error(`expected 200 bounded pages plus cap+1, got ${globalThis.__historyCalls.length}`);
if(globalThis.__historyCalls.some((x,i)=>x.limit!==50||x.cursor!==(i===0?null:String(i*50))))throw new Error("history request order or bound drifted");
if(globalThis.__seen.length!==10001||globalThis.__seen[0]!=="r0"||globalThis.__seen[10000]!=="r10000")throw new Error("10,001 revision traversal lost endpoint order");
if(new Set(globalThis.__seen).size!==10001)throw new Error("revision traversal duplicated entries");
if(t.state.history.length>250)throw new Error("retained history is unbounded: "+t.state.history.length);
const rendered=(elements.get("history")?.innerHTML.match(/class="revision"/g)||[]).length;
if(rendered>250)throw new Error("rendered revision DOM is unbounded: "+rendered);
if(t.state.history.at(-1)?.revision_id!=="r10000"||t.state.historyNext!==null)throw new Error("cap+1 terminal page truth was lost");
"#;
        let harness = skill_library_node_harness(body);
        let host = r#"
globalThis.__historyCalls=[];globalThis.__seen=[];
globalThis.__host={hasBridge:()=>false,requestResize:()=>{},requestTeardown:()=>{},callAction:async(service,action,params)=>{
  if(action!=="artifacts.history")throw new Error(action);
  globalThis.__historyCalls.push(params);
  const start=params.cursor===null?0:Number(params.cursor),count=start===10000?1:50;
  const items=Array.from(new Array(count),(_,i)=>({revision_id:`r${start+i}`,created_at:`t${start+i}`}));
  globalThis.__seen.push(...items.map(x=>x.revision_id));
  return {items,next_cursor:start===10000?null:String(start+50),library_version:300};
}};
"#;
        run_node(&format!("{host}\n{harness}"));
    }

    #[cfg(feature = "skills")]
    #[test]
    fn skill_library_complete_host_flow_and_real_dto_controls_are_rendered() {
        let body = r##"
const t=globalThis.__skillTest;
await t.loadList();t.openWorkspace(true);
document.getElementById("skillName").value="host-flow";
document.getElementById("filePath").value="SKILL.md";document.getElementById("fileBody").value="# Host flow";
await t.command({dataset:{command:"add-file"}});
document.getElementById("filePath").value="references/guide.md";document.getElementById("fileBody").value="support";
await t.validate();await t.save({preventDefault(){}});
const create=globalThis.__calls.find(x=>x.action==="artifacts.create");
if(create.params.files.length!==2||create.params.files[1].path!=="references/guide.md")throw new Error("create lost supporting file");
if(!globalThis.__calls.some(x=>x.action==="artifacts.validate"))throw new Error("validate was not called");
t.state.selected=globalThis.__summary;t.state.detail=globalThis.__summary;t.state.version=3;
await t.command({dataset:{command:"activate"}});
if(!globalThis.__calls.some(x=>x.action==="artifacts.activate"&&x.params.expected_revision_id==="rev-1"))throw new Error("latest revision was not activated");
for(const [kind,phrase] of [["stale_version","truth"],["collision","unique"],["forbidden","owner"],["source_unavailable","optional"],["refresh_failed","publication"]]){
  globalThis.__failure=kind;
  await t.mutate("artifacts.archive",{artifact_id:"artifact-1",expected_library_version:t.state.version,idempotency_key:t.requestKey("artifacts.archive","artifact-1")},"archived","artifact-1");
  if(!elements.get("notice").innerHTML.toLowerCase().includes(phrase))throw new Error(`${kind} recovery was not rendered`);
}
for(const dto of [
  {...globalThis.__summary,active_revision_id:"rev-0",owner:{relationship:"owner"},can_mutate:true,allowed_actions:["artifacts.read","artifacts.activate","artifacts.deactivate","artifacts.archive","artifacts.history"]},
  {...globalThis.__summary,active_revision_id:"rev-0",owner:{relationship:"administrator"},can_mutate:true,allowed_actions:["artifacts.read","artifacts.activate","artifacts.deactivate","artifacts.archive","artifacts.history"]},
  {...globalThis.__summary,active_revision_id:"rev-0",owner:{relationship:"member"},can_mutate:false,allowed_actions:["artifacts.history"]}
]){
  t.state.detail=dto;t.state.selected=dto;t.renderDetail();const html=elements.get("main").innerHTML;
  if(!html.includes(dto.owner.relationship))throw new Error("owner relationship DTO was not rendered");
  const disabled=(html.match(/ disabled/g)||[]).length;
  if(dto.owner.relationship==="member"&&disabled<4)throw new Error("member mutation controls were not disabled");
  if(dto.owner.relationship!=="member"&&disabled!==0)throw new Error(`${dto.owner.relationship} controls were unexpectedly disabled`);
}
"##;
        let harness = skill_library_node_harness(body);
        let host = r##"
globalThis.__calls=[];
globalThis.__summary={artifact_id:"artifact-1",name:"Host Flow",visibility:"shared",latest_revision_id:"rev-1",active_revision_id:null,current_generation:3,published_library_version:3,latest_revision_files:[{path:"SKILL.md"},{path:"references/guide.md"}],can_mutate:true,allowed_actions:["artifacts.read","artifacts.activate","artifacts.deactivate","artifacts.archive","artifacts.history"]};
globalThis.__host={hasBridge:()=>false,requestResize:()=>{},requestTeardown:()=>{},readState:()=>null,writeState:()=>{},callAction:async(service,action,params)=>{
  globalThis.__calls.push({service,action,params});
  if(globalThis.__failure&&action==="artifacts.archive"){const kind=globalThis.__failure;globalThis.__failure=null;const messages={stale_version:"stale version",collision:"collision choose unique",forbidden:"forbidden owner",source_unavailable:"source unavailable",refresh_failed:"refresh publication failed"};throw Object.assign(new Error(messages[kind]),{kind,definitive:true});}
  if(action==="artifacts.list")return {items:[globalThis.__summary],next_cursor:null,library_version:3,capabilities:{can_create_personal:true,can_create_shared:true,default_visibility:"personal",allowed_actions:["artifacts.create","artifacts.validate"]}};
  if(action==="artifacts.validate")return {valid:true,revision_id:"candidate"};
  if(action==="artifacts.create")return {artifact_id:"artifact-1",committed_library_version:3,published_library_version:3,new_generation:2};
  if(action==="artifacts.activate")return {artifact_id:"artifact-1",committed_library_version:4,published_library_version:4,new_generation:4};
  if(action==="artifacts.get")return {item:globalThis.__summary,library_version:3};
  if(action==="artifacts.history")return {items:[],next_cursor:null,library_version:3};
  if(action==="artifacts.read")return {path:params.path,text:params.path==="SKILL.md"?"# Host flow":"support",library_version:3};
  throw new Error(action);
}};
"##;
        run_node(&format!("{host}\n{harness}"));
    }

    #[cfg(feature = "skills")]
    #[test]
    fn skill_library_fresh_instance_restores_retry_and_reconciliation_from_host_state() {
        let source = serde_json::to_string(&instrumented_skill_library_script()).expect("script");
        let script = format!(
            r#"
const vm=require("node:vm"),saved=new Map();
function boot(){{
  const nodes=new Map(),element=id=>{{if(nodes.has(id))return nodes.get(id);const n={{id,hidden:false,disabled:false,value:"",textContent:"",innerHTML:"",dataset:{{}},className:"",classList:{{add(){{}},remove(){{}},toggle(){{}}}},setAttribute(){{}},getAttribute(){{return null}},addEventListener(){{}},removeEventListener(){{}},focus(){{}},matches(){{return false}},closest(){{return null}},getBoundingClientRect(){{return{{width:760,height:480}}}}}};nodes.set(id,n);return n;}};
  for(const id of ["app","workspace","main","skillList","status","expand","summary","quickCreate","browse","search","prevPage","nextPage","newSkill"])element(id);
  const parent={{postMessage(){{}}}},listeners=new Map(),host={{hasBridge:()=>false,requestResize(){{}},requestTeardown(){{}},readState:key=>saved.get(key),writeState:(key,value)=>saved.set(key,structuredClone(value)),callAction:async()=>{{throw Object.assign(new Error("network timeout"),{{kind:"network_timeout"}})}}}};
  const window={{parent,LabbyAppHost:host,addEventListener:(k,v)=>listeners.set(k,v),removeEventListener:()=>{{}}}},document={{getElementById:element,querySelector:()=>null}},context={{window,document,confirm:()=>true,requestAnimationFrame:fn=>{{fn();return 1}},crypto,structuredClone,console,setTimeout,clearTimeout}};context.globalThis=context;vm.createContext(context);vm.runInContext({source},context);return{{context,listeners}};
}}
const first=boot(),t1=first.context.__skillTest;t1.state.selected={{artifact_id:"persist"}};t1.state.detail={{artifact_id:"persist",latest_revision_id:"r1"}};t1.state.version=1;
const key=t1.requestKey("artifacts.save","persist");
t1.mutate("artifacts.save",{{artifact_id:"persist",expected_library_version:1,idempotency_key:key}},"saved","persist").then(()=>{{
  first.listeners.get("message")({{source:first.context.window.parent,data:{{method:"ui/resource-teardown",id:9}}}});
  const second=boot(),t2=second.context.__skillTest;
  if(t2.state.pending.get("artifacts.save:persist")!==key)throw new Error("fresh instance did not restore idempotency key");
  if(t2.state.reconciliation?.idempotency_key!==key||t2.state.reconciliation?.artifact_id!=="persist")throw new Error("fresh instance did not restore reconciliation");
}}).catch(e=>{{console.error(e);process.exitCode=1}});
"#
        );
        run_node(&script);
    }

    #[cfg(feature = "skills")]
    #[test]
    fn skill_library_fresh_instance_rederives_retry_key_without_persistent_host_state() {
        run_node(&skill_library_node_harness(
            r##"
const t=globalThis.__skillTest;
const intent={artifact_id:"persist",expected_revision_id:"r1",expected_library_version:7,files:[{path:"SKILL.md",content:"# Stable"},{path:"notes.md",content:"same intent"}]};
const first=t.deterministicIdempotencyKey("artifacts.save","persist",intent);
const reordered=t.deterministicIdempotencyKey("artifacts.save","persist",{files:intent.files,expected_library_version:7,expected_revision_id:"r1",artifact_id:"persist"});
if(first!==reordered)throw new Error("canonical field order changed retry key");
if(first!==t.requestKey("artifacts.save","persist",intent))throw new Error("request did not use deterministic key");
t.state.pending.clear();
const fresh=t.requestKey("artifacts.save","persist",structuredClone(intent));
if(fresh!==first)throw new Error("fresh instance cannot rederive retry key");
const changed=t.deterministicIdempotencyKey("artifacts.save","persist",{...intent,files:[{path:"SKILL.md",content:"# Changed"}]});
if(changed===first)throw new Error("changed intent reused retry key");
"##,
        ));
    }

    #[cfg(feature = "skills")]
    #[test]
    fn skill_library_fresh_instance_reconciles_committed_create_and_save_before_retry() {
        run_node(&skill_library_node_harness(
            r#"
const t=globalThis.__skillTest;
t.state.version=8;
t.state.detail={artifact_id:"saved",name:"stable",latest_revision_id:"r-new"};
if(!t.authoredMutationAlreadySatisfied("artifacts.save","saved","r-new"))throw new Error("committed save was not reconciled");
if(t.authoredMutationAlreadySatisfied("artifacts.save","saved","r-other"))throw new Error("different save was incorrectly reconciled");
let calls=0;
globalThis.__host.callAction=async(_service,action,params)=>{if(action!=="artifacts.get"||params.artifact_id!=="created")throw new Error(action);calls++;return {item:{artifact_id:"created",name:"stable",latest_revision_id:"r-new",visibility:"shared"},library_version:8};};
const found=await t.findCommittedCreate("created","stable","r-new","shared");
if(found?.artifact_id!=="created"||calls!==1)throw new Error("committed create was not reconciled by exact identity");
calls=0;
const absent=await t.findCommittedCreate("created","stable","r-new","private");
if(absent!==null||calls!==1)throw new Error("visibility mismatch falsely reconciled create");
"#,
        ));
    }

    #[cfg(feature = "skills")]
    #[test]
    fn skill_library_derive_view_model_orthogonal_truth_table_is_exhaustive() {
        run_node(&skill_library_node_harness(
            r#"
const derive=globalThis.__skillTest.deriveViewModel,booleans=[false,true];let rows=0;
for(const draft of booleans)for(const archived of booleans)for(const active of booleans)for(const mismatch of booleans)for(const shared of booleans){
  const input={archived,active_revision_id:active?"r1":null,latest_revision_id:"r2",visibility:shared?"shared":"private",current_generation:7,published_library_version:mismatch?6:7};
  const vm=derive(input,{draft});rows++;
  const expected=draft?"Draft":archived?"Archived":active?(mismatch?"Publishing":"Active"):"Deactivated";
  if(vm.lifecycle!==expected)throw new Error(`row ${rows}: expected ${expected}, got ${vm.lifecycle}`);
  if(vm.latestRevision!=="r2"||vm.activeRevision!==(active?"r1":null)||vm.visibility.value!==input.visibility)throw new Error(`row ${rows}: orthogonal projection drifted`);
  if(vm.publication.synchronized!==!mismatch)throw new Error(`row ${rows}: publication truth drifted`);
}
if(rows!==32)throw new Error(`expected exhaustive 32 rows, got ${rows}`);
"#,
        ));
    }

    /// FR-2a (issue #210, lab-41e7m.5): the consolidated availability gate is
    /// audience-free, so the admin-scope denial MUST come from this call
    /// site's own `admin_app_resources_visible` check. A regression that
    /// folded the audience into the shared gate would flip the non-admin
    /// branch from `forbidden` to success (catalog default audience is
    /// admin-visible) — this test pins the denial at the RESOURCE path.
    #[tokio::test]
    async fn read_add_server_app_resource_denies_non_admin_scope() {
        let mut server = code_mode_server().await;
        // The availability gate requires a registered `gateway` service.
        server.registry = Arc::new(crate::registry::build_default_registry());
        let (transport, _client_transport) = tokio::io::duplex(64);
        let running = rmcp::service::serve_directly::<RoleServer, _, _, std::io::Error, _>(
            server, transport, None,
        );
        let uri =
            add_server_app_resource_uri_for_tool(ADD_SERVER_TOOL_NAME).expect("add server app uri");

        let err = running
            .service()
            .read_resource_impl(
                ReadResourceRequestParams::new(uri.clone()),
                scoped_context(running.peer().clone(), &["lab:read"]),
            )
            .await
            .expect_err("non-admin read must be denied");
        assert_eq!(
            err.data.as_ref().expect("error data")["kind"],
            json!("forbidden")
        );

        let ok = complete_resource(
            running
                .service()
                .read_resource_impl(
                    ReadResourceRequestParams::new(uri),
                    scoped_context(running.peer().clone(), &["lab:admin"]),
                )
                .await
                .expect("admin read succeeds"),
        );
        assert!(!ok.contents.is_empty());
    }

    #[tokio::test]
    async fn read_server_logs_app_resource_requires_admin_scope() {
        let (transport, _client_transport) = tokio::io::duplex(64);
        let running = rmcp::service::serve_directly::<RoleServer, _, _, std::io::Error, _>(
            code_mode_server().await,
            transport,
            None,
        );

        let err = running
            .service()
            .read_resource_impl(
                ReadResourceRequestParams::new(SERVER_LOGS_APP_URI),
                scoped_context(running.peer().clone(), &["lab:read"]),
            )
            .await
            .expect_err("server logs app resource must require admin");
        assert_eq!(
            err.data.as_ref().expect("error data")["kind"],
            json!("forbidden")
        );

        let ok = complete_resource(
            running
                .service()
                .read_resource_impl(
                    ReadResourceRequestParams::new(SERVER_LOGS_APP_URI),
                    scoped_context(running.peer().clone(), &["lab:admin"]),
                )
                .await
                .expect("server logs app resource with admin scope"),
        );
        let ResourceContents::TextResourceContents { text, .. } = &ok.contents[0] else {
            panic!("expected text resource");
        };
        assert!(text.contains("Server logs"));
        assert!(text.contains("server_logs.query"));
    }

    #[test]
    fn server_logs_app_html_exposes_log_viewer_affordances() {
        let html = server_logs_app_html(SERVER_LOGS_APP_URI).expect("server logs resource");

        for expected in [
            "LabbyServerLogs",
            "server_logs.query",
            "/v1/server-logs/query",
            "html.browser",
            "LabbyAppHost",
            "savedViews",
            "persistSavedViews",
            "requestSeq",
            "drillLinks",
            "Level",
            "Service",
            "Action",
            "Kind",
            "Search",
            "normalizeOutput",
            "value.ok===false&&value.error",
            "clearRows",
            "requestWidgetResize",
        ] {
            assert!(
                html.contains(expected),
                "server logs app must include marker `{expected}`"
            );
        }
        assert!(
            !html.contains(">read only</span>"),
            "read-only state should use the compact lock affordance, not a full badge"
        );
    }

    #[test]
    fn server_logs_host_script_injection_fails_without_marker() {
        let descriptor = SERVER_LOGS_APP_RESOURCE_DESCRIPTORS
            .iter()
            .find(|descriptor| descriptor.uri == SERVER_LOGS_APP_URI)
            .expect("server logs descriptor");

        let err = inline_app_host_script("<html></html>", descriptor)
            .expect_err("missing host script marker should fail");

        assert!(err.contains("missing Labby app host script marker"));
    }

    #[test]
    fn add_server_app_is_interactive_and_mobile_responsive() {
        let descriptor = ADD_SERVER_APP_RESOURCE_DESCRIPTORS
            .iter()
            .find(|descriptor| descriptor.uri == ADD_SERVER_APP_URI)
            .expect("Add Server descriptor");
        let html = add_server_app()
            .inline_html(descriptor)
            .expect("Add Server HTML");

        for expected in [
            "Add Server",
            "Test connection",
            "Create server",
            "host.callAction(\"add_server\",action",
            "proxy_resources",
            "proxy_prompts",
            "@media (max-width:620px)",
            "env(safe-area-inset-bottom)",
            "min-height:48px",
            "ui/notifications/request-teardown",
            "probeStatus(result)",
            "result&&result.last_error",
            "lifecycle=\"closing\"",
            "observer.disconnect()",
            "originalButtonMarkup",
            "nameInput,targetInput,resources,prompts",
            ".status.warn",
            "document.documentElement.scrollHeight",
            "removeEventListener",
        ] {
            assert!(
                html.contains(expected),
                "Add Server app must include marker `{expected}`"
            );
        }
        assert!(html.contains("window.__LABBY_MCP_RESOURCE=true;"));
        assert!(!html.contains("height+20"));
    }

    #[test]
    fn add_server_command_parser_preserves_argv_boundaries() {
        let source = function_source(
            ADD_SERVER_APP_FALLBACK_HTML,
            "function words(value)",
            "function spec()",
        );
        run_node(&format!(
            r#"
{source}
const cases = [
  ['cmd "" tail', ['cmd', '', 'tail']],
  ['tool --regex \\d+', ['tool', '--regex', '\\d+']],
  ['"C:\\Program Files\\server.exe" --flag', ['C:\\Program Files\\server.exe', '--flag']],
  ['tool escaped\\ space', ['tool', 'escaped space']],
  ['tool "quoted \\"value\\""', ['tool', 'quoted "value"']],
  ['tool trailing\\', ['tool', 'trailing\\']]
];
for (const [input, expected] of cases) {{
  const actual = words(input);
  if (JSON.stringify(actual) !== JSON.stringify(expected)) {{
    throw new Error(`${{input}} => ${{JSON.stringify(actual)}}, expected ${{JSON.stringify(expected)}}`);
  }}
}}
"#
        ));
    }

    #[test]
    fn add_server_resize_and_teardown_are_stable() {
        let mut script = dom_harness(
            r#"sizes: [], teardowns: 0,
requestResize(size) { this.sizes.push(size); },
requestTeardown() { this.teardowns += 1; },
async callAction() { return { ok: true, data: {} }; }"#,
        );
        script.push_str(final_inline_script(ADD_SERVER_APP_FALLBACK_HTML));
        script.push_str(
            r#"
flushFrames();
if (host.sizes.length !== 1) throw new Error(`expected one resize, got ${host.sizes.length}`);
if ('width' in host.sizes[0]) throw new Error(`app must not request width: ${JSON.stringify(host.sizes[0])}`);
if (host.sizes[0].height !== 777) throw new Error(`height must include document: ${JSON.stringify(host.sizes[0])}`);
const exposeResources = all.get('resources');
const before = exposeResources.getAttribute('aria-checked');
all.get('close').dispatch('click');
exposeResources.dispatch('click');
if (exposeResources.getAttribute('aria-checked') !== before) throw new Error('disposed switch handler remained active');
if (host.teardowns !== 1) throw new Error(`expected one teardown, got ${host.teardowns}`);
"#,
        );
        run_node(&script);
    }

    #[test]
    fn add_server_accepts_a_connected_zero_capability_handshake() {
        let source = function_source(
            ADD_SERVER_APP_FALLBACK_HTML,
            "function probeStatus(result)",
            "function setControlsDisabled(value)",
        );
        run_node(&format!(
            r#"
function nonEssentialCapabilityError() {{ return false; }}
{source}
const probe = probeStatus({{connected:true,tool_count:0,resource_count:0,prompt_count:0,last_error:null}});
if (!probe.connected || !probe.healthy || !probe.empty) {{
  throw new Error(`zero-capability handshake was rejected: ${{JSON.stringify(probe)}}`);
}}
"#
        ));
    }

    #[test]
    fn mcp_apps_manager_is_interactive_and_mobile_responsive() {
        let descriptor = MCP_APPS_APP_RESOURCE_DESCRIPTORS
            .iter()
            .find(|descriptor| descriptor.uri == MCP_APPS_APP_URI)
            .expect("MCP Apps manager descriptor");
        let html = mcp_apps_app()
            .inline_html(descriptor)
            .expect("MCP Apps manager HTML");

        for expected in [
            "MCP Apps",
            "id:\"manager\"",
            "Enable all",
            "Disable all",
            "role='switch'",
            "aria-live=\"polite\"",
            "host.callAction(\"mcp_app\",\"status\"",
            "host.callAction(\"mcp_app\",enabled?\"enable\":\"disable\"",
            "target===\"all\"",
            "ResizeObserver",
            "observer.disconnect()",
            "document.documentElement.scrollHeight",
            "env(safe-area-inset-bottom)",
            "@media(max-width:600px)",
        ] {
            assert!(
                html.contains(expected),
                "MCP Apps manager must include marker {expected:?}"
            );
        }
        for forbidden in ["height+20", "width:Math.ceil", "min-height:100dvh"] {
            assert!(
                !html.contains(forbidden),
                "MCP Apps manager must not include {forbidden:?}"
            );
        }
        assert!(html.contains("window.__LABBY_MCP_RESOURCE=true;"));
    }

    #[test]
    fn mcp_apps_manager_preserves_structured_errors() {
        let source = function_source(
            MCP_APPS_APP_FALLBACK_HTML,
            "function normalize(value)",
            "function apply(raw)",
        );
        run_node(&format!(
            r#"
{source}
for (const value of [
  {{ok:false,error:{{message:'backend exploded'}}}},
  {{structuredContent:{{ok:false,error:{{message:'nested exploded'}}}}}},
  {{isError:true,content:[{{type:'text',text:'text exploded'}}]}}
]) {{
  let message = '';
  try {{ normalize(value); }} catch (error) {{ message = error.message; }}
  if (!message.includes('exploded')) throw new Error('structured error was masked: ' + message);
}}
"#
        ));
    }

    #[test]
    fn gateway_status_app_handles_live_status_and_mobile_lifecycle() {
        let descriptor = GATEWAY_STATUS_APP_RESOURCE_DESCRIPTORS
            .iter()
            .find(|descriptor| descriptor.uri == GATEWAY_STATUS_APP_URI)
            .expect("Gateway Status descriptor");
        let html = gateway_status_app()
            .inline_html(descriptor)
            .expect("Gateway Status HTML");

        for expected in [
            "warning.message",
            "warnings.map",
            "exposed_tool_count??",
            "window.openai.toolOutput",
            "observer.disconnect()",
            ".badge.disabled",
            "min-height:44px",
            "visible ${plural",
            "showing data from",
            "value.ok===false",
            "document.documentElement.scrollHeight",
            "initialOutputTimer",
        ] {
            assert!(
                html.contains(expected),
                "Gateway Status app must include marker `{expected}`"
            );
        }
        for forbidden in [
            "min-height:100dvh",
            "height+20",
            "text(warnings[0])",
            "width:Math.ceil",
        ] {
            assert!(
                !html.contains(forbidden),
                "Gateway Status app must not include `{forbidden}`"
            );
        }
        assert!(html.contains("window.__LABBY_MCP_RESOURCE=true;"));
    }

    #[test]
    fn settings_app_is_schema_backed_mobile_and_parseable() {
        let descriptor = SETTINGS_APP_RESOURCE_DESCRIPTORS
            .iter()
            .find(|descriptor| descriptor.uri == SETTINGS_APP_URI)
            .expect("Settings descriptor");
        let html = settings_app()
            .inline_html(descriptor)
            .expect("Settings HTML");
        for expected in [
            "LabbySettings",
            "settings\",\"schema",
            "settings\",\"state",
            "config.update",
            "env.update",
            "@media(max-width:560px)",
            "min-height:44px",
            "prefers-reduced-motion",
            "window.__LABBY_MCP_RESOURCE=true;",
        ] {
            assert!(
                html.contains(expected),
                "Settings app must include `{expected}`"
            );
        }
        let script = final_inline_script(&html);
        let escaped = serde_json::to_string(script).expect("serialize Settings script");
        run_node(&format!("new Function({escaped});"));
    }

    #[test]
    fn gateway_status_keeps_the_newest_snapshot() {
        let mut script = dom_harness(
            r#"sizes: [], calls: 0, pending: [],
requestResize(size) { this.sizes.push(size); },
requestTeardown() {},
callAction() { this.calls += 1; return new Promise(resolve => this.pending.push(resolve)); }"#,
        );
        script.push_str(final_inline_script(GATEWAY_STATUS_APP_FALLBACK_HTML));
        script.push_str(
            r#"
(async () => {
  await new Promise(resolve => setTimeout(resolve, 100));
  if (host.calls !== 1) throw new Error(`expected one fallback refresh, got ${host.calls}`);
  host.pending.shift()({ok:true,data:[{id:'new',name:'new',enabled:true,connected:true,warnings:[],config_summary:{transport:'http'}}]});
  await new Promise(resolve => setImmediate(resolve));
  window.dispatch('message', {source: parentWindow, data:{method:'ui/notifications/tool-result',params:{ok:true,data:[{id:'old',name:'old',enabled:true,connected:true,warnings:[],config_summary:{transport:'http'}}]}}});
  if (!all.get('list').innerHTML.includes('new') || all.get('list').innerHTML.includes('old')) throw new Error(`stale launch output replaced refresh: ${all.get('list').innerHTML}`);
  flushFrames();
  if (host.sizes.some(size => 'width' in size)) throw new Error(`status app must not request width: ${JSON.stringify(host.sizes)}`);
  if (!host.sizes.some(size => size.height === 777)) throw new Error(`status height must include document: ${JSON.stringify(host.sizes)}`);
})().catch(error => { console.error(error); process.exitCode = 1; });
"#,
        );
        run_node(&script);
    }

    #[test]
    fn gateway_status_preserves_structured_errors() {
        let source = function_source(
            GATEWAY_STATUS_APP_FALLBACK_HTML,
            "function normalize(value)",
            "function text(value)",
        );
        run_node(&format!(
            r#"
{source}
for (const value of [
  {{ok:false,error:{{message:'backend exploded'}}}},
  {{structuredContent:{{ok:false,error:{{message:'nested exploded'}}}}}},
  {{isError:true,content:[{{type:'text',text:'text exploded'}}]}}
]) {{
  let message = '';
  try {{ normalize(value); }} catch (error) {{ message = error.message; }}
  if (!message.includes('exploded')) throw new Error(`structured error was masked: ${{message}}`);
}}
"#
        ));
    }

    #[test]
    fn gateway_status_discovery_contract_is_documented() {
        let mcp = include_str!("../../../../docs/surfaces/MCP.md");
        let gateway = include_str!("../../../../docs/services/GATEWAY.md");
        for expected in [
            "gateway_status",
            "ui://lab/gateway/status",
            "gateway.list",
            "lab:admin",
        ] {
            assert!(
                mcp.contains(expected) || gateway.contains(expected),
                "Gateway Status docs must include `{expected}`"
            );
        }
    }

    #[tokio::test]
    async fn gateway_status_resources_are_admin_only_and_use_runtime_mime() {
        let server = resource_scope_server(crate::mcp::route_scope::McpRouteScope::Root).await;
        let (transport, _client_transport) = tokio::io::duplex(64 * 1024);
        let running = rmcp::service::serve_directly::<RoleServer, _, _, std::io::Error, _>(
            server, transport, None,
        );

        let denied = running
            .service()
            .list_resources_impl(None, scoped_context(running.peer().clone(), &["lab:read"]))
            .await
            .expect("read-scope resources");
        assert!(
            denied
                .resources
                .iter()
                .all(|resource| !resource.uri.starts_with(GATEWAY_STATUS_APP_URI))
        );

        let allowed = running
            .service()
            .list_resources_impl(None, scoped_context(running.peer().clone(), &["lab:admin"]))
            .await
            .expect("admin resources");
        assert!(
            allowed
                .resources
                .iter()
                .any(|resource| strip_app_version(&resource.uri) == GATEWAY_STATUS_APP_URI)
        );

        let forbidden = running
            .service()
            .read_resource_impl(
                ReadResourceRequestParams::new(GATEWAY_STATUS_APP_URI),
                scoped_context(running.peer().clone(), &["lab:read"]),
            )
            .await
            .expect_err("status resource must require admin");
        assert_eq!(
            forbidden.data.as_ref().expect("error data")["kind"],
            json!("forbidden")
        );

        for (uri, expected_mime) in [
            (GATEWAY_STATUS_APP_URI, CODE_MODE_APP_MIME),
            (
                GATEWAY_STATUS_APP_SKYBRIDGE_URI,
                CODE_MODE_APP_SKYBRIDGE_MIME,
            ),
        ] {
            let read = complete_resource(
                running
                    .service()
                    .read_resource_impl(
                        ReadResourceRequestParams::new(uri),
                        scoped_context(running.peer().clone(), &["lab:admin"]),
                    )
                    .await
                    .expect("admin status resource"),
            );
            let ResourceContents::TextResourceContents {
                mime_type, text, ..
            } = &read.contents[0]
            else {
                panic!("expected text status resource");
            };
            assert_eq!(mime_type.as_deref(), Some(expected_mime));
            assert!(text.contains("Gateway Status"));
        }
    }

    #[tokio::test]
    async fn protected_scope_omits_disallowed_service_action_resources() {
        let server =
            resource_scope_server(crate::mcp::route_scope::McpRouteScope::protected_subset(
                "ops",
                ["gateway-alpha"],
                ["gateway"],
                false,
            ))
            .await;
        let (transport, _client_transport) = tokio::io::duplex(64);
        let running = rmcp::service::serve_directly::<RoleServer, _, _, std::io::Error, _>(
            server, transport, None,
        );

        let resources = running
            .service()
            .list_resources_impl(None, scoped_context(running.peer().clone(), &["lab:read"]))
            .await
            .expect("list resources");
        let uris = resources
            .resources
            .iter()
            .map(|resource| resource.uri.as_str())
            .collect::<Vec<_>>();

        assert!(
            uris.contains(&"lab://gateway/actions"),
            "allowed service action resource should be listed: {uris:?}"
        );
        assert!(
            !uris.contains(&"lab://deploy/actions"),
            "disallowed service action resource leaked into resources/list: {uris:?}"
        );
    }

    #[tokio::test]
    async fn list_resources_paginates_large_builtin_catalog() {
        let server = large_resource_server(250);
        let (transport, _client_transport) = tokio::io::duplex(64);
        let running = rmcp::service::serve_directly::<RoleServer, _, _, std::io::Error, _>(
            server, transport, None,
        );

        let first = running
            .service()
            .list_resources_impl(None, scoped_context(running.peer().clone(), &["lab:read"]))
            .await
            .expect("first page");

        assert_eq!(
            first.resources.len(),
            crate::mcp::pagination::MCP_LIST_PAGE_SIZE
        );
        assert_eq!(first.resources[0].uri, "lab://catalog");
        assert!(
            first
                .next_cursor
                .as_deref()
                .is_some_and(|cursor| cursor.starts_with("v1:100:")),
            "resource pagination must bind the cursor to the captured catalog: {:?}",
            first.next_cursor
        );
        assert_eq!(first.ttl_ms, Some(0));
        assert_eq!(first.cache_scope, Some(rmcp::model::CacheScope::Private));
        let wire = serde_json::to_value(&first).expect("serialize resource list");
        assert_eq!(wire["resultType"], "complete");
        assert_eq!(wire["ttlMs"], 0);
        assert_eq!(wire["cacheScope"], "private");
        let first_page_service_resources = first
            .resources
            .iter()
            .filter(|resource| resource.uri.starts_with("lab://resource_service_"))
            .count();
        assert!(
            first_page_service_resources > 0,
            "first page should include synthetic service resources"
        );

        // Streamable HTTP creates a fresh handler for the next POST. Prove the
        // cursor resumes only from the shared route runtime by giving the
        // second handler an empty registry: rebuilding would return no service
        // resources, while the retained snapshot still contains the original
        // 250-entry catalog.
        let shared_route_runtime = Arc::clone(&running.service().route_runtime);
        let mut second_server = large_resource_server(0);
        second_server.route_runtime = shared_route_runtime;
        let (second_transport, _second_client_transport) = tokio::io::duplex(64);
        let second_running = rmcp::service::serve_directly::<RoleServer, _, _, std::io::Error, _>(
            second_server,
            second_transport,
            None,
        );
        let second_request =
            PaginatedRequestParams::default().with_cursor(first.next_cursor.clone());
        let second = second_running
            .service()
            .list_resources_impl(
                Some(second_request),
                scoped_context(second_running.peer().clone(), &["lab:read"]),
            )
            .await
            .expect("second page from shared snapshot");

        let expected_first_service_on_second_page =
            format!("lab://resource_service_{first_page_service_resources:03}/actions");
        assert_eq!(
            second.resources[0].uri,
            expected_first_service_on_second_page
        );

        // An explicit Project-shadow failure on a continuation must neither
        // refan out nor alter the retained wire page/cursor.
        let mut unavailable_context = scoped_context(second_running.peer().clone(), &["lab:read"]);
        unavailable_context
            .extensions
            .get_mut::<axum::http::request::Parts>()
            .expect("scoped context HTTP parts")
            .extensions
            .insert(crate::mcp::bound_access::ProjectAccessObservation::Unavailable);
        let unavailable = second_running
            .service()
            .list_resources_impl(
                Some(PaginatedRequestParams::default().with_cursor(first.next_cursor.clone())),
                unavailable_context,
            )
            .await
            .expect("second page with unavailable Project shadow");
        assert_eq!(
            serde_json::to_vec(&second).unwrap(),
            serde_json::to_vec(&unavailable).unwrap(),
            "Project shadow state must not mutate a retained resources/list page"
        );
    }

    #[tokio::test]
    async fn list_resource_templates_resumes_provenance_snapshot_without_refanout() {
        let server = code_mode_server().await;
        let (transport, _client_transport) = tokio::io::duplex(64);
        let running = rmcp::service::serve_directly::<RoleServer, _, _, std::io::Error, _>(
            server, transport, None,
        );
        let context = scoped_context(running.peer().clone(), &["lab:read"]);
        let audience = catalog_snapshot_audience(auth_context_from_extensions(&context.extensions));
        let templates = (0..101)
            .map(|index| ResourceTemplate::new(format!("file:///{index}/{{id}}"), "row"))
            .collect::<Vec<_>>();
        let provenance = templates
            .iter()
            .map(|template| ResourceTemplateProvenance {
                upstream: "alpha".into(),
                native_uri_template: template.uri_template.clone(),
            })
            .collect::<Vec<_>>();
        running
            .service()
            .route_runtime
            .store_resource_template_snapshot(
                audience,
                "template-revision".into(),
                Arc::from(templates),
                Arc::from(provenance),
                None,
            )
            .await;
        let request =
            PaginatedRequestParams::default().with_cursor(Some("v1:100:template-revision".into()));
        let legacy = running
            .service()
            .list_resource_templates_impl(Some(request.clone()), context)
            .await
            .expect("retained template page");
        let mut unavailable_context = scoped_context(running.peer().clone(), &["lab:read"]);
        unavailable_context
            .extensions
            .get_mut::<axum::http::request::Parts>()
            .expect("HTTP parts")
            .extensions
            .insert(crate::mcp::bound_access::ProjectAccessObservation::Unavailable);
        let unavailable = running
            .service()
            .list_resource_templates_impl(Some(request), unavailable_context)
            .await
            .expect("unavailable shadow retains page");
        assert_eq!(legacy.resource_templates.len(), 1);
        assert_eq!(
            serde_json::to_vec(&legacy).unwrap(),
            serde_json::to_vec(&unavailable).unwrap()
        );
    }

    #[tokio::test]
    async fn disabled_code_mode_app_denies_cached_resource_reads() {
        let server = code_mode_server().await;
        // A manager-backed server reads the published config directly, so a
        // config mutation cannot race the mirrored session atomic. Disable
        // through that authority rather than the atomic, which such a server
        // does not consult.
        disable_code_mode_ui(&server).await;
        let (transport, _client_transport) = tokio::io::duplex(64);
        let running = rmcp::service::serve_directly::<RoleServer, _, _, std::io::Error, _>(
            server, transport, None,
        );
        let context = scoped_context(running.peer().clone(), &["lab:read"]);

        for uri in [
            CODE_MODE_APP_URI,
            CODE_MODE_HISTORY_APP_URI,
            CODE_MODE_APP_SKYBRIDGE_URI,
        ] {
            let err = running
                .service()
                .read_resource_impl(ReadResourceRequestParams::new(uri), context.clone())
                .await
                .expect_err("disabled Code Mode app resource must stay hidden");
            assert!(
                err.message.contains("unknown UI resource"),
                "{uri} should be hidden as an unknown UI resource, got {err:?}"
            );
        }

        let versioned = versioned_app_uri(CODE_MODE_APP_URI);
        let err = running
            .service()
            .read_resource_impl(ReadResourceRequestParams::new(versioned.clone()), context)
            .await
            .expect_err("cached versioned URI must not bypass the disabled state");
        assert!(
            err.message.contains("unknown UI resource"),
            "{versioned} should be hidden as an unknown UI resource, got {err:?}"
        );
    }

    #[tokio::test]
    async fn protected_scope_hides_code_mode_app_resources_when_disabled() {
        let server =
            resource_scope_server(crate::mcp::route_scope::McpRouteScope::protected_subset(
                "ops",
                ["gateway-alpha"],
                ["gateway"],
                false,
            ))
            .await;
        let (transport, _client_transport) = tokio::io::duplex(64);
        let running = rmcp::service::serve_directly::<RoleServer, _, _, std::io::Error, _>(
            server, transport, None,
        );

        let resources = running
            .service()
            .list_resources_impl(None, scoped_context(running.peer().clone(), &["lab:read"]))
            .await
            .expect("list resources");
        let uris = resources
            .resources
            .iter()
            .map(|resource| resource.uri.as_str())
            .collect::<Vec<_>>();

        assert!(
            uris.iter()
                .all(|uri| !uri.starts_with(CODE_MODE_APP_URI_PREFIX)),
            "Code Mode app resources leaked into resources/list with expose_code_mode=false: {uris:?}"
        );
    }

    #[tokio::test]
    async fn protected_scope_denies_code_mode_app_resource_read_when_disabled() {
        let server =
            resource_scope_server(crate::mcp::route_scope::McpRouteScope::protected_subset(
                "ops",
                ["gateway-alpha"],
                ["gateway"],
                false,
            ))
            .await;
        let (transport, _client_transport) = tokio::io::duplex(64);
        let running = rmcp::service::serve_directly::<RoleServer, _, _, std::io::Error, _>(
            server, transport, None,
        );

        for uri in [CODE_MODE_APP_URI, CODE_MODE_HISTORY_APP_URI] {
            let err = running
                .service()
                .read_resource_impl(
                    ReadResourceRequestParams::new(uri),
                    scoped_context(running.peer().clone(), &["lab:read"]),
                )
                .await
                .expect_err("Code Mode app resource must be hidden");

            assert!(
                err.message.contains("unknown UI resource"),
                "{uri} should be hidden as an unknown UI resource, got {err:?}"
            );
        }
    }

    #[tokio::test]
    async fn protected_scope_denies_disallowed_service_action_resource_read() {
        let server =
            resource_scope_server(crate::mcp::route_scope::McpRouteScope::protected_subset(
                "ops",
                ["gateway-alpha"],
                ["gateway"],
                false,
            ))
            .await;
        let (transport, _client_transport) = tokio::io::duplex(64);
        let running = rmcp::service::serve_directly::<RoleServer, _, _, std::io::Error, _>(
            server, transport, None,
        );

        let err = running
            .service()
            .read_resource_impl(
                ReadResourceRequestParams::new("lab://deploy/actions"),
                scoped_context(running.peer().clone(), &["lab:read"]),
            )
            .await
            .expect_err("disallowed service action resource must be denied");

        assert_eq!(
            err.data.as_ref().expect("error data")["kind"],
            json!("route_scope_denied")
        );
    }

    #[tokio::test]
    async fn protected_scope_allows_allowed_service_action_resource_read() {
        let server =
            resource_scope_server(crate::mcp::route_scope::McpRouteScope::protected_subset(
                "ops",
                ["gateway-alpha"],
                ["gateway"],
                false,
            ))
            .await;
        let (transport, _client_transport) = tokio::io::duplex(64);
        let running = rmcp::service::serve_directly::<RoleServer, _, _, std::io::Error, _>(
            server, transport, None,
        );

        let allowed = complete_resource(
            running
                .service()
                .read_resource_impl(
                    ReadResourceRequestParams::new("lab://gateway/actions"),
                    scoped_context(running.peer().clone(), &["lab:read"]),
                )
                .await
                .expect("allowed service action resource"),
        );

        let ResourceContents::TextResourceContents { text, .. } = &allowed.contents[0] else {
            panic!("expected text resource");
        };
        assert!(
            text.contains(r#""name": "help""#),
            "allowed action resource should render the service action catalog: {text}"
        );
    }

    #[tokio::test]
    async fn read_history_resource_requires_read_scope_and_returns_html_metadata() {
        let (transport, _client_transport) = tokio::io::duplex(64);
        let running = rmcp::service::serve_directly::<RoleServer, _, _, std::io::Error, _>(
            code_mode_server().await,
            transport,
            None,
        );

        let denied = running
            .service()
            .read_resource_impl(
                ReadResourceRequestParams::new(CODE_MODE_HISTORY_APP_URI),
                scoped_context(running.peer().clone(), &["profile"]),
            )
            .await
            .expect_err("scope must be denied");
        assert_eq!(
            denied.data.as_ref().expect("error data")["kind"],
            json!("forbidden")
        );

        let allowed = running
            .service()
            .read_resource(
                ReadResourceRequestParams::new(CODE_MODE_HISTORY_APP_URI),
                scoped_context(running.peer().clone(), &["lab:read"]),
            )
            .await
            .expect("read history resource");
        let ReadResourceResponse::Complete(allowed) = allowed else {
            panic!("history resource must complete in one round");
        };
        assert_eq!(allowed.ttl_ms, Some(0));
        assert_eq!(allowed.cache_scope, Some(rmcp::model::CacheScope::Private));
        let wire = serde_json::to_value(&allowed).expect("serialize resource read");
        assert_eq!(wire["resultType"], "complete");
        assert_eq!(wire["ttlMs"], 0);
        assert_eq!(wire["cacheScope"], "private");
        assert_eq!(allowed.contents.len(), 1);
        match &allowed.contents[0] {
            ResourceContents::TextResourceContents {
                uri,
                mime_type,
                text,
                meta,
            } => {
                assert_eq!(uri, CODE_MODE_HISTORY_APP_URI);
                assert_eq!(mime_type.as_deref(), Some(CODE_MODE_APP_MIME));
                assert!(text.contains("code_mode_history"));
                let meta = meta.as_ref().expect("resource metadata");
                assert_eq!(
                    meta.0["ui"]["resourceUri"],
                    json!(CODE_MODE_HISTORY_APP_URI)
                );
                assert_eq!(meta.0["ui"]["mimeTypes"], json!([CODE_MODE_APP_MIME]));
                assert_eq!(meta.0["ui"]["prefersBorder"], json!(false));
                assert_eq!(meta.0["ui"]["csp"]["connectDomains"], json!([]));
                assert!(meta.0.get("csp").is_none());
                assert!(meta.0.get("prefersBorder").is_none());
            }
            ResourceContents::BlobResourceContents { .. } => panic!("expected text resource"),
            _ => panic!("expected text resource"),
        }
    }

    #[tokio::test]
    async fn protected_scope_history_resource_hides_unscoped_entries() {
        let server =
            code_mode_server_with_scope(crate::mcp::route_scope::McpRouteScope::protected_subset(
                "ops",
                ["gateway-alpha"],
                ["gateway"],
                true,
            ))
            .await;
        let manager = server.gateway_manager.as_ref().expect("manager").clone();
        manager
            .record_code_mode_history(crate::dispatch::gateway::code_mode::CodeModeHistoryEntry {
                execution_id: None,
                seq: 0,
                route_scope: "root".to_string(),
                kind: crate::dispatch::gateway::code_mode::CodeModeHistoryKind::Execute,
                ok: true,
                elapsed_ms: 7,
                input_tokens: Some(3),
                output_tokens: Some(5),
                error_kind: None,
                calls: Vec::new(),
                match_count: None,
            })
            .await;
        let (transport, _client_transport) = tokio::io::duplex(64);
        let running = rmcp::service::serve_directly::<RoleServer, _, _, std::io::Error, _>(
            server, transport, None,
        );

        let allowed = complete_resource(
            running
                .service()
                .read_resource_impl(
                    ReadResourceRequestParams::new(CODE_MODE_HISTORY_APP_URI),
                    scoped_context(running.peer().clone(), &["lab:read"]),
                )
                .await
                .expect("read history resource"),
        );

        let ResourceContents::TextResourceContents { text, .. } = &allowed.contents[0] else {
            panic!("expected text resource");
        };
        assert!(
            text.contains(r#""entries":[]"#),
            "protected scope should not see global history: {text}"
        );
    }

    #[tokio::test]
    async fn skybridge_resource_is_readable_by_uri_despite_being_unlisted() {
        let (transport, _client_transport) = tokio::io::duplex(64);
        let running = rmcp::service::serve_directly::<RoleServer, _, _, std::io::Error, _>(
            code_mode_server().await,
            transport,
            None,
        );

        // OpenAI hosts never see this URI in resources/list (`listed: false`);
        // they reach it directly via the tool's `openai/outputTemplate`. Prove
        // the full read path serves it under the skybridge MIME with the
        // model-facing description attached.
        let allowed = complete_resource(
            running
                .service()
                .read_resource_impl(
                    ReadResourceRequestParams::new(CODE_MODE_APP_SKYBRIDGE_URI),
                    scoped_context(running.peer().clone(), &["lab:read"]),
                )
                .await
                .expect("read skybridge resource"),
        );
        let ResourceContents::TextResourceContents {
            uri,
            mime_type,
            text,
            meta,
        } = &allowed.contents[0]
        else {
            panic!("expected text resource");
        };
        assert_eq!(uri, CODE_MODE_APP_SKYBRIDGE_URI);
        assert_eq!(mime_type.as_deref(), Some(CODE_MODE_APP_SKYBRIDGE_MIME));
        assert!(text.contains("Lab Code Mode Inspector"));
        assert!(
            meta.as_ref()
                .expect("resource metadata")
                .0
                .contains_key("openai/widgetDescription")
        );

        // The unlisted resource still honors the read scope gate.
        let denied = running
            .service()
            .read_resource_impl(
                ReadResourceRequestParams::new(CODE_MODE_APP_SKYBRIDGE_URI),
                scoped_context(running.peer().clone(), &["profile"]),
            )
            .await
            .expect_err("scope must be denied");
        assert_eq!(
            denied.data.as_ref().expect("error data")["kind"],
            json!("forbidden")
        );
    }

    #[tokio::test]
    async fn unknown_code_mode_uri_is_rejected_by_the_read_path() {
        let (transport, _client_transport) = tokio::io::duplex(64);
        let running = rmcp::service::serve_directly::<RoleServer, _, _, std::io::Error, _>(
            code_mode_server().await,
            transport,
            None,
        );

        // The router admits any `ui://lab/code-mode/*` prefix; an un-tabled URI
        // under it must 404 through the full read path, not be served fallback HTML.
        let err = running
            .service()
            .read_resource_impl(
                ReadResourceRequestParams::new("ui://lab/code-mode/nope"),
                scoped_context(running.peer().clone(), &["lab:read"]),
            )
            .await
            .expect_err("un-tabled URI must be rejected");
        assert!(err.message.contains("unknown UI resource"), "{err:?}");
    }

    #[test]
    fn code_mode_app_descriptor_table_invariants_hold() {
        // MIME and listed-ness now derive from `runtime`, so the mime↔listed and
        // "both runtimes bound to one resource" failure modes are unrepresentable.
        // The one convention left is the tool↔descriptor mapping: every Code Mode
        // tool must have exactly one MCP (Claude) descriptor and exactly one
        // skybridge (OpenAI) descriptor, or it silently loses one runtime's binding.
        for tool in [CODE_MODE_UI_TOOL_NAME] {
            assert_eq!(
                CODE_MODE_APP_RESOURCE_DESCRIPTORS
                    .iter()
                    .filter(|descriptor| {
                        descriptor.runtime == CodeModeRuntime::McpApp
                            && descriptor.tool_name == Some(tool)
                    })
                    .count(),
                1,
                "tool {tool} must have exactly one MCP (Claude) descriptor"
            );
            assert_eq!(
                CODE_MODE_APP_RESOURCE_DESCRIPTORS
                    .iter()
                    .filter(|descriptor| {
                        descriptor.runtime == CodeModeRuntime::Skybridge
                            && descriptor.tool_name == Some(tool)
                    })
                    .count(),
                1,
                "tool {tool} is missing its skybridge (OpenAI) descriptor"
            );
        }

        // Skybridge resources must never appear in resources/list (Claude surface).
        assert!(
            CODE_MODE_APP_RESOURCE_DESCRIPTORS
                .iter()
                .filter(|descriptor| descriptor.runtime == CodeModeRuntime::Skybridge)
                .all(|descriptor| !descriptor.runtime.listed()),
            "skybridge resources must stay out of resources/list"
        );

        // The one illegal state the enum can't prevent: a descriptor's URI must
        // match its runtime (a `.skybridge` URI on an McpApp row would be served
        // under the wrong MIME and leak into the Claude listing). Pin URI↔runtime.
        for descriptor in CODE_MODE_APP_RESOURCE_DESCRIPTORS {
            assert_eq!(
                descriptor.uri.ends_with(".skybridge"),
                descriptor.runtime == CodeModeRuntime::Skybridge,
                "descriptor {} URI suffix disagrees with its runtime",
                descriptor.uri
            );
        }

        // Lookups return None for an unmapped tool (the skybridge binding is then
        // silently omitted; the MCP binding `.expect()`s at the call site).
        assert_eq!(code_mode_app_resource_uri_for_tool("not_a_tool"), None);
        assert_eq!(code_mode_app_skybridge_uri_for_tool("not_a_tool"), None);
    }

    #[test]
    fn versioned_widget_uri_round_trips_through_the_read_path() {
        // The host fetches the advertised (versioned) URI. It must resolve to the
        // same descriptor/HTML as the base URI so the cache-bust token is purely a
        // cache key, not a new resource the read path can't find.
        let versioned = versioned_app_uri(CODE_MODE_APP_URI);
        assert!(versioned.starts_with(CODE_MODE_APP_URI));
        assert!(versioned.contains("?v="));
        assert_eq!(strip_app_version(&versioned), CODE_MODE_APP_URI);

        let from_base = code_mode_app_html(CODE_MODE_APP_URI, None).expect("base resource");
        let from_versioned = code_mode_app_html(&versioned, None).expect("versioned resource");
        assert_eq!(from_base, from_versioned);

        // Runtime/MIME resolution must also ignore the suffix.
        assert_eq!(
            code_mode_app_runtime_for_uri(&versioned).mime(),
            CODE_MODE_APP_MIME
        );

        // A base URI with no query is returned unchanged.
        assert_eq!(strip_app_version(CODE_MODE_APP_URI), CODE_MODE_APP_URI);

        // An un-tabled URI is still rejected even with a cache-bust suffix.
        let bogus = versioned_app_uri("ui://lab/code-mode/nope");
        assert!(code_mode_app_html(&bogus, None).is_err());
    }

    #[test]
    fn bridged_app_version_includes_the_injected_host_runtime() {
        let html = "<html>fixture</html>";
        let html_only = format!("{:016x}", fnv1a_64(html.as_bytes()));
        let combined = format!("{html}\n{}", crate::app_assets::LABBY_APP_HOST_JS);

        assert_eq!(
            bridged_app_content_version(html),
            format!("{:016x}", fnv1a_64(combined.as_bytes()))
        );
        assert_ne!(bridged_app_content_version(html), html_only);
    }

    #[test]
    fn add_server_resource_log_uri_redacts_query_credentials() {
        let uri = format!("{ADD_SERVER_APP_URI}?token=super-secret#fragment");
        let resource_uri_log =
            crate::dispatch::upstream::pool::redact_resource_uri_for_logging(&uri);

        assert_eq!(resource_uri_log, ADD_SERVER_APP_URI);
        assert!(!resource_uri_log.contains("super-secret"));
    }

    #[test]
    fn code_mode_app_html_accepts_known_ui_resources_and_rejects_unknown() {
        let html = code_mode_app_html(CODE_MODE_APP_URI, None).expect("known resource");
        assert!(html.contains("Lab Code Mode Inspector"));
        // The bundle hydrates natively under the OpenAI Apps runtime
        // (ChatGPT / Codex) via window.openai.toolOutput + openai:set_globals.
        // The bundle is hand-maintained vanilla JS with no JS harness, so these
        // string guards catch the regression where the whole OpenAI branch or its
        // "waiting" gate is dropped and only the React copy (which IS tested)
        // stays correct.
        assert!(
            html.contains("openai:set_globals"),
            "bundle must carry the OpenAI Apps hydration bridge"
        );
        assert!(
            html.contains("window.openai"),
            "bundle must branch on the OpenAI Apps runtime global"
        );
        assert!(
            html.contains("\"waiting\""),
            "bundle must keep the 'waiting' state so an empty widget isn't shown as connected"
        );

        // The skybridge variant serves the same HTML under the OpenAI MIME.
        let skybridge =
            code_mode_app_html(CODE_MODE_APP_SKYBRIDGE_URI, None).expect("skybridge resource");
        assert!(skybridge.contains("Lab Code Mode Inspector"));

        let err = code_mode_app_html("ui://lab/code-mode/nope", None).expect_err("unknown");
        assert!(err.contains("unknown UI resource"));
    }

    #[test]
    fn skybridge_and_mcp_app_resource_meta_diverge_by_runtime() {
        // OpenAI skybridge resource: skybridge MIME + model-facing description.
        let skybridge = code_mode_app_resource_meta(CODE_MODE_APP_SKYBRIDGE_URI);
        assert_eq!(
            skybridge.0["ui"]["mimeTypes"][0].as_str(),
            Some(CODE_MODE_APP_SKYBRIDGE_MIME)
        );
        assert!(
            skybridge.0.contains_key("openai/widgetDescription"),
            "skybridge resource must carry an OpenAI widget description"
        );

        // Claude resource: MCP Apps MIME, and byte-identical (no openai/* keys).
        let mcp_app = code_mode_app_resource_meta(CODE_MODE_APP_URI);
        assert_eq!(
            mcp_app.0["ui"]["mimeTypes"][0].as_str(),
            Some(CODE_MODE_APP_MIME)
        );
        assert!(
            !mcp_app.0.contains_key("openai/widgetDescription"),
            "Claude resource _meta must stay free of OpenAI compatibility keys"
        );
    }

    #[test]
    fn code_mode_app_resources_follow_synthetic_tool_visibility() {
        let read_auth = labby_auth::auth_context::AuthContext {
            sub: "reader".to_string(),
            actor_key: None,
            scopes: vec!["lab:read".to_string()],
            issuer: "https://lab.example.com".to_string(),
            via_session: true,
            csrf_token: None,
            email: None,
        };
        let denied_auth = labby_auth::auth_context::AuthContext {
            scopes: vec!["profile".to_string()],
            ..read_auth.clone()
        };
        assert!(
            code_mode_app_resources_visible(true, Some(&read_auth)),
            "Code Mode app resources should be listed with the synthetic codemode tool"
        );
        assert!(
            !code_mode_app_resources_visible(true, Some(&denied_auth)),
            "Code Mode app resources should not be listed without Code Mode read scope"
        );
        assert!(
            !code_mode_app_resources_visible(false, Some(&read_auth)),
            "Code Mode app resources should not be listed when synthetic tools are disabled"
        );
        let resources = code_mode_app_resources();
        let uris = resources
            .iter()
            .map(|resource| strip_app_version(&resource.uri).to_string())
            .collect::<Vec<_>>();
        assert_eq!(uris, vec![CODE_MODE_APP_URI, CODE_MODE_HISTORY_APP_URI]);
        // The tool-binding URI carries the cache-bust token but resolves to the
        // canonical base after stripping it.
        let codemode_uri =
            code_mode_app_resource_uri_for_tool(CODE_MODE_UI_TOOL_NAME).expect("codemode UI uri");
        assert!(codemode_uri.contains("?v="));
        assert_eq!(strip_app_version(&codemode_uri), CODE_MODE_APP_URI);
    }

    #[test]
    fn code_mode_history_html_injects_escaped_snapshot() {
        let html = code_mode_app_html(
            CODE_MODE_HISTORY_APP_URI,
            Some(&json!({
                "kind": "code_mode_history",
                "entries": [{"seq": 1, "kind": "execute", "ok": true, "elapsed_ms": 1, "calls": [{"params": {"note": "</script>"}}]}],
            })),
        )
        .expect("history resource");

        assert!(html.contains("code_mode_history"));
        assert!(!html.contains("</script>\""));
        assert!(html.contains("\\u003c/script>"));
    }

    #[test]
    fn code_mode_app_html_uses_current_trace_field_names() {
        let html = code_mode_app_html(
            CODE_MODE_APP_URI,
            Some(&json!({
                "kind": "code_mode_execute_trace",
                "call_count": 1,
                "calls": [{
                    "id": "github::search_issues",
                    "upstream": "github",
                    "tool": "search_issues",
                    "ok": true,
                    "elapsed_ms": 12,
                    "ui": {"resourceUri": "ui://github/search.html"},
                    "result_shape": {"type": "array", "length": 3},
                }],
            })),
        )
        .expect("codemode resource");

        assert!(html.contains("call.ok"));
        assert!(html.contains("call.error_kind"));
        assert!(html.contains("call.ui"));
        assert!(html.contains("resourceUri"));
        assert!(html.contains("MCP UI"));
        assert!(html.contains("s.length"));
        assert!(
            html.contains("call.namespace"),
            "inline app must read the emitted namespace field (with an id-split fallback)"
        );
        assert!(
            !html.contains("call.status"),
            "inline app must use the emitted ok boolean, not stale status fields"
        );
        assert!(
            !html.contains("array_len"),
            "inline app must use result_shape.length"
        );
    }

    #[test]
    fn code_mode_app_html_unwraps_connector_result_envelopes() {
        let html = code_mode_app_html(CODE_MODE_APP_URI, None).expect("codemode resource");

        for expected in [
            "function tracePayload",
            "structuredContent",
            "structured_content",
            "JSON.parse(block.text)",
        ] {
            assert!(
                html.contains(expected),
                "inline app must unwrap connector envelope marker `{expected}`"
            );
        }
    }

    #[test]
    fn code_mode_app_html_gates_connected_state_on_bridge_handshake() {
        let html = code_mode_app_html(CODE_MODE_APP_URI, None).expect("codemode resource");
        // Status must not be claimed "connected" before the bridge resolves.
        assert!(
            html.contains("\"connecting\""),
            "MCP Apps branch must start from a 'connecting' state, not optimistic 'connected'"
        );
        assert!(
            html.contains("if (!hydrated) setState(\"connected\", true)"),
            "MCP Apps branch must gate 'connected' on the connect() handshake"
        );
        assert!(
            html.contains("if (!hydrated) setState(\"unavailable\", false)"),
            "MCP Apps branch must keep a rejected bridge handshake diagnostically visible"
        );
    }

    #[test]
    fn code_mode_app_html_preserves_expanded_rows_and_uses_available_width() {
        let html = code_mode_app_html(CODE_MODE_APP_URI, None).expect("codemode resource");

        assert!(
            html.contains("data-row-key"),
            "rows need stable keys so repaint can preserve expanded state"
        );
        assert!(
            html.contains("snapshotExpandedRows"),
            "paint must snapshot expanded rows before replacing the DOM"
        );
        assert!(
            html.contains("restoreExpandedRows"),
            "paint must restore expanded rows after replacing the DOM"
        );
        assert!(
            html.contains("max-width:none"),
            "the ChatGPT app should use the host-provided width"
        );
        assert!(
            !html.contains("max-width:680px"),
            "the old 680px cap leaves unused space around the inspector"
        );
    }

    #[test]
    fn code_mode_app_html_reports_content_sized_height() {
        let html = code_mode_app_html(CODE_MODE_APP_URI, None).expect("codemode resource");

        assert!(
            html.contains("function scheduleResize"),
            "inline app should explicitly measure its rendered widget height"
        );
        assert!(
            html.contains("sendSizeChanged"),
            "inline app should notify MCP Apps hosts when content height changes"
        );
        let reset_height = html
            .find("document.documentElement.style.height=\"auto\"")
            .expect("inline app resets the persisted root height");
        let measure_height = html
            .find("document.body.getBoundingClientRect()")
            .expect("inline app measures its content height");
        assert!(
            reset_height < measure_height,
            "persisted heights must be reset before measuring so the app can shrink"
        );
        assert!(
            html.contains("if(activeMcpUiUri!==uri)setMinimized(true)"),
            "repainting the same MCP UI must preserve a user's restored inspector state"
        );
        assert!(
            html.contains("autoResize: false"),
            "document-root auto-resize can over-report empty iframe space below the widget"
        );
    }

    #[test]
    fn code_mode_app_html_starts_minimized() {
        let html = code_mode_app_html(CODE_MODE_APP_URI, None).expect("codemode resource");

        for expected in [
            "<main class=\"widget minimized\">",
            "let minimized=true;",
            "id=\"minimizeToggle\" aria-label=\"Restore inspector\" aria-pressed=\"true\" title=\"Restore inspector\"",
            "class=\"minimize-icon\" width=\"12\" height=\"12\" viewBox=\"0 0 24 24\" fill=\"none\" stroke=\"currentColor\" stroke-width=\"1.75\" stroke-linecap=\"round\" stroke-linejoin=\"round\" hidden",
            "class=\"restore-icon\" width=\"12\" height=\"12\" viewBox=\"0 0 24 24\" fill=\"none\" stroke=\"currentColor\" stroke-width=\"1.75\" stroke-linecap=\"round\" stroke-linejoin=\"round\"><path",
        ] {
            assert!(
                html.contains(expected),
                "inline app must start minimized with marker `{expected}`"
            );
        }
    }

    #[test]
    fn code_mode_app_html_exposes_debugger_ui_affordances() {
        let html = code_mode_app_html(CODE_MODE_APP_URI, None).expect("codemode resource");

        for expected in [
            "paintHeadMeta",
            "section(\"Calls\"",
            "section(\"Request\"",
            "section(\"Response\"",
            "row-error",
            "viewTab(\"pretty\"",
            "viewTab(\"raw\"",
            "viewTab(\"shape\"",
            "rowcopy",
            "longest",
            "Run ",
            "border-radius:10px",
            "head-tools-panel",
            "aria-label=\"Read only\"",
        ] {
            assert!(
                html.contains(expected),
                "inline app must include debugger UI affordance marker `{expected}`"
            );
        }
    }

    #[test]
    fn code_mode_app_html_surfaces_action_dispatched_calls() {
        let html = code_mode_app_html(CODE_MODE_APP_URI, None).expect("codemode resource");

        assert!(
            html.contains("function callActionLabel"),
            "inline app must derive a readable action label from call params"
        );
        assert!(
            html.contains("params.action"),
            "action-dispatched one-tool servers should show params.action"
        );
        assert!(
            html.contains("action-label"),
            "call rows should render the derived action label separately from the tool id"
        );
    }

    #[test]
    fn code_mode_app_html_exposes_inspector_power_tools() {
        let html = code_mode_app_html(CODE_MODE_APP_URI, None).expect("codemode resource");

        for expected in [
            "copyReplaySnippet",
            "saveSnippet",
            "resultSearch",
            "setAllRows",
            "actionDescription",
            "callInvocationMode",
            "emptyReason",
            "truncationNotice",
            "historyDelta",
        ] {
            assert!(
                html.contains(expected),
                "inline app must include inspector power-tool marker `{expected}`"
            );
        }
    }
}
