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
//! No behavior change — relocation only.

use std::time::Instant;

use rmcp::ErrorData;
use rmcp::RoleServer;
use rmcp::model::{
    AnnotateAble, ListResourcesResult, LoggingLevel, Meta, PaginatedRequestParams, RawResource,
    ReadResourceRequestParams, ReadResourceResult, ResourceContents,
};
use rmcp::service::RequestContext;
use serde_json::{Value, json};

use crate::mcp::catalog::{CODE_MODE_SEARCH_TOOL_NAME, TOOL_EXECUTE_TOOL_NAME};
#[cfg(feature = "gateway")]
use crate::mcp::context::oauth_upstream_subject_for_request;
use crate::mcp::context::{auth_context_from_extensions, code_mode_search_scope_allowed};
use crate::mcp::logging::DispatchLogOutcome;
use crate::mcp::server::LabMcpServer;

/// MCP Apps (Claude / SEP-1724) MIME — bound via the tool's `_meta.ui.resourceUri`.
pub(crate) const CODE_MODE_APP_MIME: &str = "text/html;profile=mcp-app";
/// OpenAI Apps (ChatGPT / Codex) MIME — bound via the tool's `openai/outputTemplate`.
/// Same HTML body; a distinct URI + MIME so the Claude resource stays untouched.
pub(crate) const CODE_MODE_APP_SKYBRIDGE_MIME: &str = "text/html+skybridge";
/// URI namespace reserved for Lab's own Code Mode app resources, served locally.
/// Any other `ui://` is an upstream mcp-ui widget resource routed to its peer.
pub(crate) const CODE_MODE_APP_URI_PREFIX: &str = "ui://lab/code-mode/";
pub(crate) const CODE_MODE_SEARCH_APP_URI: &str = "ui://lab/code-mode/search";
pub(crate) const CODE_MODE_EXECUTE_APP_URI: &str = "ui://lab/code-mode/execute";
pub(crate) const CODE_MODE_HISTORY_APP_URI: &str = "ui://lab/code-mode/history";
/// OpenAI Apps skybridge variants — same HTML, served under the skybridge MIME.
pub(crate) const CODE_MODE_SEARCH_APP_SKYBRIDGE_URI: &str = "ui://lab/code-mode/search.skybridge";
pub(crate) const CODE_MODE_EXECUTE_APP_SKYBRIDGE_URI: &str = "ui://lab/code-mode/execute.skybridge";

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

pub(crate) struct CodeModeAppResourceDescriptor {
    pub(crate) uri: &'static str,
    pub(crate) name: &'static str,
    pub(crate) runtime: CodeModeRuntime,
    /// Tool this widget binds to, or `None` for the history widget (not tool-
    /// bound). `runtime` selects which `_meta` key the URI is exposed under.
    pub(crate) tool_name: Option<&'static str>,
}

pub(crate) const CODE_MODE_APP_RESOURCE_DESCRIPTORS: &[CodeModeAppResourceDescriptor] = &[
    CodeModeAppResourceDescriptor {
        uri: CODE_MODE_SEARCH_APP_URI,
        name: "code-mode/search",
        runtime: CodeModeRuntime::McpApp,
        tool_name: Some(CODE_MODE_SEARCH_TOOL_NAME),
    },
    CodeModeAppResourceDescriptor {
        uri: CODE_MODE_EXECUTE_APP_URI,
        name: "code-mode/execute",
        runtime: CodeModeRuntime::McpApp,
        tool_name: Some(TOOL_EXECUTE_TOOL_NAME),
    },
    CodeModeAppResourceDescriptor {
        uri: CODE_MODE_HISTORY_APP_URI,
        name: "code-mode/history",
        runtime: CodeModeRuntime::McpApp,
        tool_name: None,
    },
    CodeModeAppResourceDescriptor {
        uri: CODE_MODE_SEARCH_APP_SKYBRIDGE_URI,
        name: "code-mode/search.skybridge",
        runtime: CodeModeRuntime::Skybridge,
        tool_name: Some(CODE_MODE_SEARCH_TOOL_NAME),
    },
    CodeModeAppResourceDescriptor {
        uri: CODE_MODE_EXECUTE_APP_SKYBRIDGE_URI,
        name: "code-mode/execute.skybridge",
        runtime: CodeModeRuntime::Skybridge,
        tool_name: Some(TOOL_EXECUTE_TOOL_NAME),
    },
];

const CODE_MODE_APP_FALLBACK_HTML: &str = include_str!("assets/code_mode_app.html");

impl LabMcpServer {
    pub(crate) async fn list_resources_impl(
        &self,
        _request: Option<PaginatedRequestParams>,
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
        let mut resources = vec![
            RawResource::new("lab://catalog", "catalog")
                .with_description("Full discovery document for all services")
                .with_mime_type("application/json")
                .no_annotation(),
        ];
        if code_mode_app_resources_visible(
            self.code_mode_visibility().await.exposes_synthetic_tools(),
            auth,
        ) {
            resources.extend(code_mode_app_resources());
        }

        for svc in self.registry.services() {
            if self.service_visible_on_mcp(svc.name).await {
                let uri = format!("lab://{}/actions", svc.name);
                let name = format!("{}/actions", svc.name);
                resources.push(
                    RawResource::new(uri, name)
                        .with_description(format!("Action list for {}", svc.name))
                        .with_mime_type("application/json")
                        .no_annotation(),
                );
            }
        }

        #[cfg(feature = "gateway")]
        if let Some(pool) = self.current_upstream_pool().await {
            resources.extend(
                pool.gateway_synthetic_resources_allowed(self.route_scope.allowed_upstreams())
                    .await,
            );
            resources.extend(
                pool.list_upstream_resources_allowed(self.route_scope.allowed_upstreams())
                    .await,
            );
            if let Some(oauth_subject) =
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
                resources.extend(scoped_resources);
            }
        }

        let elapsed_ms = start.elapsed().as_millis();
        tracing::info!(
            surface = "mcp",
            service = "labby",
            action = "list_resources",
            subject,
            elapsed_ms,
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

        Ok(ListResourcesResult::with_all_items(resources))
    }

    pub(crate) async fn read_resource_impl(
        &self,
        request: ReadResourceRequestParams,
        context: RequestContext<RoleServer>,
    ) -> Result<ReadResourceResult, ErrorData> {
        let start = Instant::now();
        let subject = self.request_subject_log_tag(&context);
        let uri = &request.uri;
        #[cfg(feature = "gateway")]
        let resource_uri_log =
            crate::dispatch::upstream::pool::redact_resource_uri_for_logging(uri);
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

        // Branch 0: MCP Apps UI resources. This must precede all lab://
        // fallbacks so ui:// has its own exact lookup semantics.
        //
        // Local Code Mode app resources own the `ui://lab/code-mode/*` namespace
        // and are served from the bundled HTML.
        if uri.starts_with(CODE_MODE_APP_URI_PREFIX) {
            return self
                .read_code_mode_app_resource_impl(uri, &subject, start, &context)
                .await;
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
            if !code_mode_search_scope_allowed(auth) {
                return Err(ErrorData::invalid_params(
                    "UI resources require one of scopes: lab:read, lab, lab:admin",
                    Some(json!({
                        "kind": "forbidden",
                        "required_scopes": ["lab:read", "lab", "lab:admin"],
                    })),
                ));
            }
            if let Some(pool) = self.current_upstream_pool().await {
                return self
                    .read_upstream_ui_resource_impl(&pool, uri, &subject, start, &context)
                    .await;
            }
            return Err(ErrorData::resource_not_found(
                format!("unknown UI resource: {uri}"),
                None,
            ));
        }

        // Branch 1: gateway-synthetic resources.
        #[cfg(feature = "gateway")]
        if uri.starts_with("lab://gateway/") {
            return self
                .read_gateway_resource_impl(uri, &subject, start, &context)
                .await;
        }

        // Branch 2: raw upstream resource proxy.
        #[cfg(feature = "gateway")]
        if let Some(pool) = self.current_upstream_pool().await
            && uri.starts_with("lab://upstream/")
        {
            return self
                .read_upstream_resource_impl(&pool, uri, &subject, start, &context)
                .await;
        }

        // Branch 3: subject-scoped upstream resource proxy.
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
                    uri,
                    &subject,
                    start,
                    &context,
                )
                .await;
        }

        // Local branch: lab://catalog + lab://<svc>/actions.
        let json = if uri == "lab://catalog" {
            self.catalog_json().await
        } else if let Some(service) = uri
            .strip_prefix("lab://")
            .and_then(|value| value.strip_suffix("/actions"))
        {
            self.service_actions_json(service).await
        } else {
            return Err(ErrorData::resource_not_found(
                format!("unknown resource: {uri}"),
                None,
            ));
        };

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
                        return Err(ErrorData::internal_error(
                            format!("failed to serialize resource: {e}"),
                            None,
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
                    ResourceContents::text(text, uri.clone()).with_mime_type("application/json"),
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
                Err(ErrorData::internal_error(e.to_string(), None))
            }
        }
    }

    async fn read_code_mode_app_resource_impl(
        &self,
        uri: &str,
        subject: &str,
        start: Instant,
        context: &RequestContext<RoleServer>,
    ) -> Result<ReadResourceResult, ErrorData> {
        if !self.code_mode_visibility().await.exposes_synthetic_tools() {
            return Err(ErrorData::resource_not_found(
                format!("unknown UI resource: {uri}"),
                None,
            ));
        }
        let auth = auth_context_from_extensions(&context.extensions);
        if !code_mode_search_scope_allowed(auth) {
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
            return Err(ErrorData::invalid_params(
                "Code Mode app resources require one of scopes: lab:read, lab, lab:admin",
                Some(json!({
                    "kind": "forbidden",
                    "required_scopes": ["lab:read", "lab", "lab:admin"],
                })),
            ));
        }
        let history = if uri == CODE_MODE_HISTORY_APP_URI {
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
        let html = code_mode_app_html(uri, history.as_ref())
            .map_err(|message| ErrorData::resource_not_found(message, None))?;
        let elapsed_ms = start.elapsed().as_millis();
        tracing::info!(
            surface = "mcp",
            service = "labby",
            action = "read_resource",
            subject,
            elapsed_ms,
            resource_uri = uri,
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
                .with_mime_type(code_mode_app_runtime_for_uri(uri).mime())
                .with_meta(code_mode_app_resource_meta(uri)),
        ]))
    }
}

fn code_mode_app_html(uri: &str, history: Option<&Value>) -> Result<String, String> {
    if !CODE_MODE_APP_RESOURCE_DESCRIPTORS
        .iter()
        .any(|descriptor| descriptor.uri == uri)
    {
        return Err(format!("unknown UI resource: {uri}"));
    }

    let mut html = CODE_MODE_APP_FALLBACK_HTML.to_string();
    if let Some(snapshot) = history {
        let injected = format!(
            "window.__LAB_CODE_MODE_INITIAL_TRACE__ = {};",
            snapshot.to_string().replace('<', "\\u003c")
        );
        html = html.replace("window.__LAB_CODE_MODE_INITIAL_TRACE__ = null;", &injected);
    }
    Ok(html)
}

fn code_mode_app_resource(descriptor: &CodeModeAppResourceDescriptor) -> rmcp::model::Resource {
    RawResource::new(descriptor.uri.to_string(), descriptor.name.to_string())
        .with_description("Read-only MCP App for Code Mode call traces")
        .with_mime_type(descriptor.runtime.mime())
        .with_meta(code_mode_app_resource_meta(descriptor.uri))
        .no_annotation()
}

/// Host runtime a Code Mode app URI targets. Callers must pass a table URI; an
/// un-tabled URI is a programming error (the runtime selects MIME/listed-ness
/// and binding, so a silent wrong default would mis-bind the widget) — assert in
/// debug, warn and fall back to MCP Apps in release rather than serving nothing.
fn code_mode_app_runtime_for_uri(uri: &str) -> CodeModeRuntime {
    CODE_MODE_APP_RESOURCE_DESCRIPTORS
        .iter()
        .find(|descriptor| descriptor.uri == uri)
        .map_or_else(
            || {
                debug_assert!(
                    false,
                    "code_mode_app_runtime_for_uri called with un-tabled URI: {uri}"
                );
                tracing::warn!(
                    resource_uri = uri,
                    "unknown Code Mode URI; defaulting to MCP Apps runtime"
                );
                CodeModeRuntime::McpApp
            },
            |descriptor| descriptor.runtime,
        )
}

fn code_mode_app_resources_visible(
    exposes_synthetic_tools: bool,
    auth: Option<&crate::api::oauth::AuthContext>,
) -> bool {
    exposes_synthetic_tools && code_mode_search_scope_allowed(auth)
}

fn code_mode_app_resources() -> Vec<rmcp::model::Resource> {
    CODE_MODE_APP_RESOURCE_DESCRIPTORS
        .iter()
        .filter(|descriptor| descriptor.runtime.listed())
        .map(code_mode_app_resource)
        .collect()
}

/// MCP Apps (Claude) widget URI for a tool — backs `_meta.ui.resourceUri`.
pub(crate) fn code_mode_app_resource_uri_for_tool(tool_name: &str) -> Option<&'static str> {
    code_mode_app_uri_for_tool(CodeModeRuntime::McpApp, tool_name)
}

/// OpenAI Apps (ChatGPT / Codex) widget URI for a tool — backs `openai/outputTemplate`.
pub(crate) fn code_mode_app_skybridge_uri_for_tool(tool_name: &str) -> Option<&'static str> {
    code_mode_app_uri_for_tool(CodeModeRuntime::Skybridge, tool_name)
}

fn code_mode_app_uri_for_tool(runtime: CodeModeRuntime, tool_name: &str) -> Option<&'static str> {
    CODE_MODE_APP_RESOURCE_DESCRIPTORS
        .iter()
        .find(|descriptor| descriptor.runtime == runtime && descriptor.tool_name == Some(tool_name))
        .map(|descriptor| descriptor.uri)
}

pub(crate) fn code_mode_app_resource_meta(uri: &str) -> Meta {
    let runtime = code_mode_app_runtime_for_uri(uri);
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
    // OpenAI Apps exposes a model-facing description of the widget. Skybridge-
    // only, so the Claude (`text/html;profile=mcp-app`) resource `_meta` stays
    // byte-identical.
    if runtime == CodeModeRuntime::Skybridge {
        meta.insert(
            "openai/widgetDescription".to_string(),
            json!(
                "Live Code Mode call trace — upstream tool calls, catalog search matches, and recent gateway history."
            ),
        );
    }
    Meta(meta)
}

#[cfg(all(test, feature = "gateway"))]
#[allow(clippy::panic)]
mod tests {
    use super::*;
    use rmcp::service::{Peer, RequestContext};

    async fn code_mode_server() -> LabMcpServer {
        code_mode_server_with_scope(crate::mcp::route_scope::McpRouteScope::Root).await
    }

    async fn code_mode_server_with_scope(
        route_scope: crate::mcp::route_scope::McpRouteScope,
    ) -> LabMcpServer {
        let runtime = crate::dispatch::gateway::manager::GatewayRuntimeHandle::default();
        let manager = std::sync::Arc::new(crate::dispatch::gateway::manager::GatewayManager::new(
            std::path::PathBuf::from("config.toml"),
            runtime,
        ));
        manager
            .seed_config(crate::config::LabConfig {
                code_mode: crate::config::CodeModeConfig {
                    enabled: true,
                    ..crate::config::CodeModeConfig::default()
                },
                ..crate::config::LabConfig::default()
            })
            .await;
        LabMcpServer {
            registry: std::sync::Arc::new(crate::registry::ToolRegistry::new()),
            gateway_manager: Some(manager),
            node_role: None,
            peers: std::sync::Arc::new(tokio::sync::RwLock::new(Vec::new())),
            logging_level: std::sync::Arc::new(std::sync::atomic::AtomicU8::new(
                crate::mcp::logging::logging_level_rank(LoggingLevel::Emergency),
            )),
            route_scope,
            code_mode_widget_callbacks_enabled_for_test: false,
        }
    }

    fn scoped_context(peer: Peer<RoleServer>, scopes: &[&str]) -> RequestContext<RoleServer> {
        let mut context = RequestContext::new(rmcp::model::NumberOrString::Number(1), peer);
        let mut parts = axum::http::Request::new(()).into_parts().0;
        parts.extensions.insert(crate::api::oauth::AuthContext {
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
        let meta = code_mode_app_resource_meta(CODE_MODE_SEARCH_APP_URI);
        assert_eq!(
            meta.0["ui"]["resourceUri"].as_str(),
            Some(CODE_MODE_SEARCH_APP_URI)
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
            .map(|resource| resource.uri.as_str())
            .collect::<Vec<_>>();
        assert_eq!(
            code_mode_uris,
            vec![
                CODE_MODE_SEARCH_APP_URI,
                CODE_MODE_EXECUTE_APP_URI,
                CODE_MODE_HISTORY_APP_URI
            ]
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
            .read_resource_impl(
                ReadResourceRequestParams::new(CODE_MODE_HISTORY_APP_URI),
                scoped_context(running.peer().clone(), &["lab:read"]),
            )
            .await
            .expect("read history resource");
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
        }
    }

    #[tokio::test]
    async fn protected_scope_history_resource_hides_unscoped_entries() {
        let server =
            code_mode_server_with_scope(crate::mcp::route_scope::McpRouteScope::protected_subset(
                "media",
                ["sonarr"],
                ["gateway"],
                true,
            ))
            .await;
        let manager = server.gateway_manager.as_ref().expect("manager").clone();
        manager
            .record_code_mode_history(crate::dispatch::gateway::code_mode::CodeModeHistoryEntry {
                seq: 0,
                route_scope: "root".to_string(),
                kind: crate::dispatch::gateway::code_mode::CodeModeHistoryKind::Search,
                ok: true,
                elapsed_ms: 7,
                error_kind: None,
                calls: Vec::new(),
                match_count: Some(7),
            })
            .await;
        let (transport, _client_transport) = tokio::io::duplex(64);
        let running = rmcp::service::serve_directly::<RoleServer, _, _, std::io::Error, _>(
            server, transport, None,
        );

        let allowed = running
            .service()
            .read_resource_impl(
                ReadResourceRequestParams::new(CODE_MODE_HISTORY_APP_URI),
                scoped_context(running.peer().clone(), &["lab:read"]),
            )
            .await
            .expect("read history resource");

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
        let allowed = running
            .service()
            .read_resource_impl(
                ReadResourceRequestParams::new(CODE_MODE_EXECUTE_APP_SKYBRIDGE_URI),
                scoped_context(running.peer().clone(), &["lab:read"]),
            )
            .await
            .expect("read skybridge resource");
        let ResourceContents::TextResourceContents {
            uri,
            mime_type,
            text,
            meta,
        } = &allowed.contents[0]
        else {
            panic!("expected text resource");
        };
        assert_eq!(uri, CODE_MODE_EXECUTE_APP_SKYBRIDGE_URI);
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
                ReadResourceRequestParams::new(CODE_MODE_EXECUTE_APP_SKYBRIDGE_URI),
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
        for tool in [CODE_MODE_SEARCH_TOOL_NAME, TOOL_EXECUTE_TOOL_NAME] {
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
    fn code_mode_app_html_accepts_known_ui_resources_and_rejects_unknown() {
        let html = code_mode_app_html(CODE_MODE_EXECUTE_APP_URI, None).expect("known resource");
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
        let skybridge = code_mode_app_html(CODE_MODE_EXECUTE_APP_SKYBRIDGE_URI, None)
            .expect("skybridge resource");
        assert!(skybridge.contains("Lab Code Mode Inspector"));

        let err = code_mode_app_html("ui://lab/code-mode/nope", None).expect_err("unknown");
        assert!(err.contains("unknown UI resource"));
    }

    #[test]
    fn skybridge_and_mcp_app_resource_meta_diverge_by_runtime() {
        // OpenAI skybridge resource: skybridge MIME + model-facing description.
        let skybridge = code_mode_app_resource_meta(CODE_MODE_EXECUTE_APP_SKYBRIDGE_URI);
        assert_eq!(
            skybridge.0["ui"]["mimeTypes"][0].as_str(),
            Some(CODE_MODE_APP_SKYBRIDGE_MIME)
        );
        assert!(
            skybridge.0.contains_key("openai/widgetDescription"),
            "skybridge resource must carry an OpenAI widget description"
        );

        // Claude resource: MCP Apps MIME, and byte-identical (no openai/* keys).
        let mcp_app = code_mode_app_resource_meta(CODE_MODE_EXECUTE_APP_URI);
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
        let read_auth = crate::api::oauth::AuthContext {
            sub: "reader".to_string(),
            actor_key: None,
            scopes: vec!["lab:read".to_string()],
            issuer: "https://lab.example.com".to_string(),
            via_session: true,
            csrf_token: None,
            email: None,
        };
        let denied_auth = crate::api::oauth::AuthContext {
            scopes: vec!["profile".to_string()],
            ..read_auth.clone()
        };
        assert!(
            code_mode_app_resources_visible(true, Some(&read_auth)),
            "Code Mode app resources should be listed with synthetic search/execute tools"
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
            .map(|resource| resource.uri.as_str())
            .collect::<Vec<_>>();
        assert_eq!(
            uris,
            vec![
                CODE_MODE_SEARCH_APP_URI,
                CODE_MODE_EXECUTE_APP_URI,
                CODE_MODE_HISTORY_APP_URI
            ]
        );
        assert_eq!(
            code_mode_app_resource_uri_for_tool(CODE_MODE_SEARCH_TOOL_NAME),
            Some(CODE_MODE_SEARCH_APP_URI)
        );
        assert_eq!(
            code_mode_app_resource_uri_for_tool(TOOL_EXECUTE_TOOL_NAME),
            Some(CODE_MODE_EXECUTE_APP_URI)
        );
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
            CODE_MODE_EXECUTE_APP_URI,
            Some(&json!({
                "kind": "code_mode_execute_trace",
                "call_count": 1,
                "calls": [{
                    "id": "github::search_issues",
                    "upstream": "github",
                    "tool": "search_issues",
                    "ok": true,
                    "elapsed_ms": 12,
                    "result_shape": {"type": "array", "length": 3},
                }],
            })),
        )
        .expect("execute resource");

        assert!(html.contains("statusLabel"));
        assert!(html.contains("call.ok"));
        assert!(html.contains("s.length"));
        assert!(
            !html.contains("call.status"),
            "inline app must use the emitted ok boolean, not stale status fields"
        );
        assert!(
            !html.contains("array_len"),
            "inline app must use result_shape.length"
        );
    }
}
