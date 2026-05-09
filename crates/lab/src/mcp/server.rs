//! `LabMcpServer` — the MCP `ServerHandler` implementation.
//!
//! Extracted from `cli/serve.rs` so that both the stdio and HTTP transports
//! can share the same handler logic.

use sha2::{Digest, Sha256};
use std::sync::Arc;
use std::sync::atomic::{AtomicU8, Ordering};
use std::time::Instant;

use axum::http::{self, request::Parts};
use rmcp::model::{
    AnnotateAble, CallToolRequestParams, CallToolResult, CompleteRequestParams, CompleteResult,
    CompletionInfo, Content, GetPromptRequestParams, GetPromptResult, ListPromptsResult,
    ListResourcesResult, ListToolsResult, LoggingLevel, PaginatedRequestParams, RawResource,
    ReadResourceRequestParams, ReadResourceResult, ResourceContents, ServerCapabilities,
    ServerInfo, SetLevelRequestParams, Tool,
};
use rmcp::service::{NotificationContext, Peer, RequestContext};
use rmcp::{ErrorData, RoleServer, ServerHandler};
use serde_json::Value;
use tokio::sync::RwLock;

use crate::config::NodeRole;
use crate::dispatch::gateway::manager::GatewayManager;
use crate::mcp::elicitation::{ElicitResult, elicit_confirm};
use crate::mcp::envelope::{build_error, build_error_extra, build_success};
use crate::mcp::error::DispatchError;
use crate::mcp::error::canonical_kind;
use crate::mcp::logging::{DispatchLogOutcome, logging_level_rank};
use crate::registry::ToolRegistry;

#[cfg(test)]
use crate::mcp::peers::PeerNotifier;

/// JSON Schema for every service tool's input: `action` (required) + `params` (optional object).
#[allow(clippy::expect_used)]
fn action_schema() -> serde_json::Map<String, Value> {
    serde_json::json!({
        "type": "object",
        "properties": {
            "action": {
                "type": "string",
                "description": "Action to perform (e.g. \"movie.search\"). Use \"help\" to list all actions."
            },
            "params": {
                "type": "object",
                "description": "Action-specific parameters (varies per action)"
            }
        },
        "required": ["action"]
    })
    .as_object()
    .cloned()
    .expect("schema literal is always an object")
}

fn completion_info(values: Vec<String>) -> CompletionInfo {
    CompletionInfo {
        total: Some(values.len() as u32),
        has_more: Some(false),
        values,
    }
}

fn complete_prompt_arg(
    registry: &ToolRegistry,
    prompt_name: &str,
    argument_name: &str,
    prefix: &str,
) -> CompletionInfo {
    match (prompt_name, argument_name) {
        ("run-action", "action") => completion_info(registry.action_name_completions(prefix)),
        ("run-action" | "service-discover", "service") => {
            completion_info(service_name_completions(registry, prefix))
        }
        _ => completion_info(Vec::new()),
    }
}

fn service_name_completions(registry: &ToolRegistry, prefix: &str) -> Vec<String> {
    registry
        .services()
        .iter()
        .map(|service| service.name)
        .filter(|name| name.starts_with(prefix))
        .map(str::to_string)
        .collect()
}

/// MCP server handler — one tool per registered service.
pub struct LabMcpServer {
    pub registry: Arc<ToolRegistry>,
    /// Shared gateway manager used to resolve the current live upstream pool.
    pub gateway_manager: Option<Arc<GatewayManager>>,
    /// Resolved role for the current device.
    pub node_role: Option<NodeRole>,
    /// Connected peers for list-changed notifications.
    pub peers: Arc<RwLock<Vec<Peer<RoleServer>>>>,
    /// Negotiated RMCP logging threshold for this server/session.
    pub logging_level: Arc<AtomicU8>,
}

pub fn verify_upstream_subject_resolution_support() -> anyhow::Result<()> {
    let (parts, _) = http::Request::new(()).into_parts();
    let auth = crate::api::oauth::AuthContext {
        sub: "startup-self-test".to_string(),
        actor_key: None,
        scopes: Vec::new(),
        issuer: "https://lab.example.com".to_string(),
        via_session: false,
        csrf_token: None,
        email: None,
    };

    let mut extensions = rmcp::model::Extensions::new();
    let mut parts = parts;
    parts.extensions.insert(auth);
    extensions.insert(parts);

    if subject_from_extensions(&extensions) == Some("startup-self-test") {
        return Ok(());
    }

    anyhow::bail!(
        "rmcp subject extraction self-test failed: RequestContext.extensions did not yield \
         http::request::Parts/AuthContext. The current runtime expects rmcp 1.4 request \
         extension propagation (Plan A). Wire the tokio::task_local fallback (Plan B) or pin \
         a compatible rmcp version before starting."
    );
}

impl ServerHandler for LabMcpServer {
    fn get_info(&self) -> ServerInfo {
        tracing::info!(
            surface = "mcp",
            service = "labby",
            action = "server.info",
            subsystem = "mcp_server",
            phase = "server.info",
            builtin_service_count = self.registry.services().len(),
            gateway_manager_configured = self.gateway_manager.is_some(),
            node_role = ?self.node_role,
            "advertising MCP server capabilities"
        );
        ServerInfo::new(
            ServerCapabilities::builder()
                .enable_tools()
                .enable_tool_list_changed()
                .enable_resources()
                .enable_resources_list_changed()
                .enable_prompts()
                .enable_prompts_list_changed()
                .enable_logging()
                .enable_completions()
                .build(),
        )
    }

    async fn set_level(
        &self,
        request: SetLevelRequestParams,
        _context: RequestContext<RoleServer>,
    ) -> Result<(), ErrorData> {
        self.logging_level
            .store(logging_level_rank(request.level), Ordering::Release);
        tracing::info!(
            surface = "mcp",
            service = "labby",
            action = "logging.setLevel",
            level = ?request.level,
            "rmcp logging level updated"
        );
        Ok(())
    }

    async fn on_initialized(&self, context: NotificationContext<RoleServer>) {
        let mut peers = self.peers.write().await;
        peers.push(context.peer);
        tracing::info!(
            surface = "mcp",
            service = "peers",
            action = "peer.connect",
            subsystem = "mcp_server",
            phase = "session.initialized",
            peer_count = peers.len(),
            "mcp session connected"
        );
    }

    async fn complete(
        &self,
        request: CompleteRequestParams,
        context: RequestContext<RoleServer>,
    ) -> Result<CompleteResult, ErrorData> {
        let start = Instant::now();
        let subject = self.request_subject_log_tag(&context);
        let reference_type = request.r#ref.reference_type();
        let prompt = request.r#ref.as_prompt_name().map(str::to_string);
        tracing::info!(
            surface = "mcp",
            service = "labby",
            action = "completion.complete",
            subject,
            reference_type,
            prompt = prompt.as_deref().unwrap_or(""),
            argument = %request.argument.name,
            "dispatch start"
        );

        let completion = match prompt.as_deref() {
            Some(prompt_name) => complete_prompt_arg(
                &self.registry,
                prompt_name,
                &request.argument.name,
                &request.argument.value,
            ),
            None => completion_info(Vec::new()),
        };

        let elapsed_ms = start.elapsed().as_millis();
        tracing::info!(
            surface = "mcp",
            service = "labby",
            action = "completion.complete",
            subject,
            reference_type,
            prompt = prompt.as_deref().unwrap_or(""),
            argument = %request.argument.name,
            result_count = completion.values.len(),
            elapsed_ms,
            "completion ok"
        );
        self.emit_dispatch_notification(
            &context,
            "lab",
            "completion.complete",
            elapsed_ms,
            DispatchLogOutcome::Success,
        )
        .await;

        Ok(CompleteResult::new(completion))
    }

    async fn list_prompts(
        &self,
        _request: Option<PaginatedRequestParams>,
        context: RequestContext<RoleServer>,
    ) -> Result<ListPromptsResult, ErrorData> {
        let start = Instant::now();
        let subject = self.request_subject_log_tag(&context);
        tracing::info!(
            surface = "mcp",
            service = "labby",
            action = "list_prompts",
            subject,
            "dispatch start"
        );
        let mut prompts = crate::mcp::prompts::list_all().prompts;

        if let Some(pool) = self.current_upstream_pool().await {
            let builtin_names: Vec<String> = prompts
                .iter()
                .map(|prompt| prompt.name.to_string())
                .collect();
            let builtin_name_refs: Vec<&str> = builtin_names.iter().map(String::as_str).collect();
            let upstream_prompts = pool.list_upstream_prompts(&builtin_name_refs).await;
            prompts.extend(upstream_prompts);
            if let Some(subject) = self.request_subject(&context) {
                let scoped_prompts = pool
                    .subject_scoped_prompts(
                        &self.oauth_upstream_configs().await,
                        subject,
                        &builtin_name_refs,
                    )
                    .await;
                prompts.extend(scoped_prompts);
            }
        }

        let elapsed_ms = start.elapsed().as_millis();
        tracing::info!(
            surface = "mcp",
            service = "labby",
            action = "list_prompts",
            subject,
            elapsed_ms,
            "prompt list ok"
        );
        self.emit_dispatch_notification(
            &context,
            "lab",
            "list_prompts",
            elapsed_ms,
            DispatchLogOutcome::Success,
        )
        .await;

        Ok(ListPromptsResult::with_all_items(prompts))
    }

    async fn get_prompt(
        &self,
        request: GetPromptRequestParams,
        context: RequestContext<RoleServer>,
    ) -> Result<GetPromptResult, ErrorData> {
        let start = Instant::now();
        let subject = self.request_subject_log_tag(&context);
        tracing::info!(
            surface = "mcp",
            service = "labby",
            action = "get_prompt",
            subject,
            prompt = %request.name,
            "dispatch start"
        );
        let args = request
            .arguments
            .clone()
            .unwrap_or_default()
            .into_iter()
            .map(|(key, value)| {
                let string = match value {
                    Value::String(text) => text,
                    other => other.to_string(),
                };
                (key, string)
            })
            .collect();

        if let Some(prompt) = crate::mcp::prompts::get(&self.registry, &request.name, &args) {
            let elapsed_ms = start.elapsed().as_millis();
            tracing::info!(
                surface = "mcp",
                service = "labby",
                action = "get_prompt",
                subject,
                elapsed_ms,
                "prompt resolved"
            );
            self.emit_dispatch_notification(
                &context,
                "lab",
                "get_prompt",
                elapsed_ms,
                DispatchLogOutcome::Success,
            )
            .await;
            return Ok(prompt);
        }

        if let Some(pool) = self.current_upstream_pool().await
            && let Some(upstream_name) = pool.find_prompt_owner(&request.name).await
        {
            let prompt_name = request.name.clone();
            tracing::info!(
                surface = "mcp",
                service = "labby",
                action = "get_prompt",
                prompt = %prompt_name,
                upstream = %upstream_name,
                route = "upstream",
                "dispatch route selected"
            );
            let outcome = match pool.get_prompt(&upstream_name, request).await {
                Some(Ok(result)) => {
                    let elapsed_ms = start.elapsed().as_millis();
                    tracing::info!(
                        surface = "mcp",
                        service = "labby",
                        action = "get_prompt",
                        subject,
                        prompt = %prompt_name,
                        upstream = %upstream_name,
                        elapsed_ms,
                        "prompt proxy ok"
                    );
                    self.emit_dispatch_notification(
                        &context,
                        "lab",
                        "get_prompt",
                        elapsed_ms,
                        DispatchLogOutcome::Success,
                    )
                    .await;
                    Ok(result)
                }
                Some(Err(message)) => {
                    let elapsed_ms = start.elapsed().as_millis();
                    tracing::warn!(
                        surface = "mcp",
                        service = "labby",
                        action = "get_prompt",
                        prompt = %prompt_name,
                        upstream = %upstream_name,
                        elapsed_ms,
                        kind = "internal_error",
                        error = %message,
                        "prompt proxy failed"
                    );
                    self.emit_dispatch_notification(
                        &context,
                        "lab",
                        "get_prompt",
                        elapsed_ms,
                        DispatchLogOutcome::Failure {
                            level: LoggingLevel::Error,
                            kind: "internal_error",
                        },
                    )
                    .await;
                    Err(ErrorData::internal_error(message, None))
                }
                None => {
                    let elapsed_ms = start.elapsed().as_millis();
                    tracing::warn!(
                        surface = "mcp",
                        service = "labby",
                        action = "get_prompt",
                        prompt = %prompt_name,
                        upstream = %upstream_name,
                        elapsed_ms,
                        kind = "not_found",
                        "upstream not connected for prompt"
                    );
                    self.emit_dispatch_notification(
                        &context,
                        "lab",
                        "get_prompt",
                        elapsed_ms,
                        DispatchLogOutcome::Failure {
                            level: LoggingLevel::Warning,
                            kind: "not_found",
                        },
                    )
                    .await;
                    Err(ErrorData::invalid_params(
                        format!("unknown prompt: {prompt_name}"),
                        None,
                    ))
                }
            };
            return outcome;
        }

        if let Some(subject) = self.request_subject(&context)
            && let Some(pool) = self.current_upstream_pool().await
        {
            let configs = self.oauth_upstream_configs().await;
            if let Some(upstream_name) = pool
                .subject_scoped_prompt_owner(&configs, subject, &request.name)
                .await
                && let Some(config) = configs
                    .into_iter()
                    .find(|config| config.name == upstream_name)
            {
                let prompt_name = request.name.clone();
                tracing::info!(
                    surface = "mcp",
                    service = "labby",
                    action = "get_prompt",
                    prompt = %prompt_name,
                    upstream = %config.name,
                    route = "subject_scoped",
                    "dispatch route selected"
                );
                let outcome = match pool
                    .subject_scoped_get_prompt(&config, subject, request)
                    .await
                {
                    Ok(result) => {
                        let elapsed_ms = start.elapsed().as_millis();
                        tracing::info!(
                            surface = "mcp",
                            service = "labby",
                            action = "get_prompt",
                            subject,
                            prompt = %prompt_name,
                            upstream = %config.name,
                            elapsed_ms,
                            "subject-scoped prompt proxy ok"
                        );
                        self.emit_dispatch_notification(
                            &context,
                            "lab",
                            "get_prompt",
                            elapsed_ms,
                            DispatchLogOutcome::Success,
                        )
                        .await;
                        Ok(result)
                    }
                    Err(message) => {
                        let elapsed_ms = start.elapsed().as_millis();
                        tracing::warn!(
                            surface = "mcp",
                            service = "labby",
                            action = "get_prompt",
                            prompt = %prompt_name,
                            upstream = %config.name,
                            elapsed_ms,
                            kind = "upstream_error",
                            error = %message,
                            "subject-scoped prompt proxy failed"
                        );
                        self.emit_dispatch_notification(
                            &context,
                            "lab",
                            "get_prompt",
                            elapsed_ms,
                            DispatchLogOutcome::Failure {
                                level: LoggingLevel::Warning,
                                kind: "upstream_error",
                            },
                        )
                        .await;
                        Err(ErrorData::invalid_params(
                            format!("upstream prompt `{prompt_name}` failed: {message}"),
                            None,
                        ))
                    }
                };
                return outcome;
            }
        }

        let elapsed_ms = start.elapsed().as_millis();
        tracing::warn!(
            surface = "mcp",
            service = "labby",
            action = "get_prompt",
            subject,
            elapsed_ms,
            kind = "not_found",
            "unknown prompt"
        );
        self.emit_dispatch_notification(
            &context,
            "lab",
            "get_prompt",
            elapsed_ms,
            DispatchLogOutcome::Failure {
                level: LoggingLevel::Warning,
                kind: "not_found",
            },
        )
        .await;
        Err(ErrorData::invalid_params(
            format!("unknown prompt: {}", request.name),
            None,
        ))
    }

    async fn list_resources(
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
        let mut resources = vec![
            RawResource::new("lab://catalog", "catalog")
                .with_description("Full discovery document for all services")
                .with_mime_type("application/json")
                .no_annotation(),
        ];

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

        if let Some(pool) = self.current_upstream_pool().await {
            resources.extend(pool.list_upstream_resources().await);
            if let Some(subject) = self.request_subject(&context) {
                let configs = self.oauth_upstream_configs().await;
                resources.extend(pool.subject_scoped_resources(&configs, subject).await);
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

    async fn read_resource(
        &self,
        request: ReadResourceRequestParams,
        context: RequestContext<RoleServer>,
    ) -> Result<ReadResourceResult, ErrorData> {
        let start = Instant::now();
        let subject = self.request_subject_log_tag(&context);
        let uri = &request.uri;
        tracing::info!(
            surface = "mcp",
            service = "labby",
            action = "read_resource",
            subject,
            resource_uri = crate::dispatch::upstream::pool::redact_resource_uri_for_logging(uri),
            "dispatch start"
        );

        if let Some(pool) = self.current_upstream_pool().await
            && uri.starts_with("lab://upstream/")
        {
            tracing::info!(
                surface = "mcp",
                service = "labby",
                action = "read_resource",
                resource_uri =
                    crate::dispatch::upstream::pool::redact_resource_uri_for_logging(uri),
                route = "upstream",
                "dispatch route selected"
            );
            let outcome = match pool.read_upstream_resource(uri).await {
                Some(Ok(result)) => {
                    let elapsed_ms = start.elapsed().as_millis();
                    let upstream = uri
                        .strip_prefix("lab://upstream/")
                        .and_then(|rest| rest.split('/').next())
                        .unwrap_or("unknown");
                    tracing::info!(
                        surface = "mcp",
                        service = "labby",
                        action = "read_resource",
                        subject,
                        upstream,
                        resource_uri =
                            crate::dispatch::upstream::pool::redact_resource_uri_for_logging(uri),
                        elapsed_ms,
                        "resource proxy ok"
                    );
                    self.emit_dispatch_notification(
                        &context,
                        "lab",
                        "read_resource",
                        elapsed_ms,
                        DispatchLogOutcome::Success,
                    )
                    .await;
                    Ok(result)
                }
                Some(Err(message)) => {
                    let elapsed_ms = start.elapsed().as_millis();
                    let upstream = uri
                        .strip_prefix("lab://upstream/")
                        .and_then(|rest| rest.split('/').next())
                        .unwrap_or("unknown");
                    tracing::warn!(
                        surface = "mcp",
                        service = "labby",
                        action = "read_resource",
                        upstream,
                        resource_uri = crate::dispatch::upstream::pool::redact_resource_uri_for_logging(uri),
                        elapsed_ms,
                        kind = "internal_error",
                        error = %message,
                        "resource proxy failed"
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
                    Err(ErrorData::internal_error(message, None))
                }
                None => {
                    let elapsed_ms = start.elapsed().as_millis();
                    let upstream = uri
                        .strip_prefix("lab://upstream/")
                        .and_then(|rest| rest.split('/').next())
                        .unwrap_or("unknown");
                    tracing::warn!(
                        surface = "mcp",
                        service = "labby",
                        action = "read_resource",
                        upstream,
                        resource_uri =
                            crate::dispatch::upstream::pool::redact_resource_uri_for_logging(uri),
                        elapsed_ms,
                        kind = "not_found",
                        "upstream not connected for resource"
                    );
                    self.emit_dispatch_notification(
                        &context,
                        "lab",
                        "read_resource",
                        elapsed_ms,
                        DispatchLogOutcome::Failure {
                            level: LoggingLevel::Warning,
                            kind: "not_found",
                        },
                    )
                    .await;
                    Err(ErrorData::resource_not_found(
                        format!("unknown resource: {uri}"),
                        None,
                    ))
                }
            };
            return outcome;
        }

        if let Some(subject) = self.request_subject(&context)
            && let Some(pool) = self.current_upstream_pool().await
            && let Some(upstream_name) = uri
                .strip_prefix("lab://upstream/")
                .and_then(|rest| rest.split('/').next())
            && let Some(config) = self.oauth_upstream_config(upstream_name).await
        {
            tracing::info!(
                surface = "mcp",
                service = "labby",
                action = "read_resource",
                resource_uri = crate::dispatch::upstream::pool::redact_resource_uri_for_logging(uri),
                upstream = %config.name,
                route = "subject_scoped",
                "dispatch route selected"
            );
            let outcome = match pool
                .subject_scoped_read_resource(&config, subject, uri)
                .await
            {
                Ok(result) => {
                    let elapsed_ms = start.elapsed().as_millis();
                    tracing::info!(
                        surface = "mcp",
                        service = "labby",
                        action = "read_resource",
                        subject,
                        upstream = %config.name,
                        resource_uri = crate::dispatch::upstream::pool::redact_resource_uri_for_logging(uri),
                        elapsed_ms,
                        "subject-scoped resource proxy ok"
                    );
                    self.emit_dispatch_notification(
                        &context,
                        "lab",
                        "read_resource",
                        elapsed_ms,
                        DispatchLogOutcome::Success,
                    )
                    .await;
                    Ok(result)
                }
                Err(message) => {
                    let elapsed_ms = start.elapsed().as_millis();
                    tracing::warn!(
                        surface = "mcp",
                        service = "labby",
                        action = "read_resource",
                        upstream = %config.name,
                        resource_uri = crate::dispatch::upstream::pool::redact_resource_uri_for_logging(uri),
                        elapsed_ms,
                        kind = "upstream_error",
                        error = %message,
                        "subject-scoped resource proxy failed"
                    );
                    self.emit_dispatch_notification(
                        &context,
                        "lab",
                        "read_resource",
                        elapsed_ms,
                        DispatchLogOutcome::Failure {
                            level: LoggingLevel::Warning,
                            kind: "upstream_error",
                        },
                    )
                    .await;
                    Err(ErrorData::invalid_params(message, None))
                }
            };
            return outcome;
        }

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
                let text = serde_json::to_string_pretty(&value).unwrap_or_default();
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

    async fn list_tools(
        &self,
        _request: Option<PaginatedRequestParams>,
        context: RequestContext<RoleServer>,
    ) -> Result<ListToolsResult, ErrorData> {
        let start = Instant::now();
        let subject = self.request_subject_log_tag(&context);
        tracing::info!(
            surface = "mcp",
            service = "labby",
            action = "list_tools",
            subject,
            "dispatch start"
        );
        let schema = Arc::new(action_schema());
        let mut tools = Vec::new();
        let mut builtin_tool_count = 0usize;
        let mut upstream_tool_count = 0usize;
        let mut subject_scoped_tool_count = 0usize;
        let mut gateway_tool_count = 0usize;
        let (tool_search_enabled, enabled_tool_search_gateways) =
            if let Some(manager) = &self.gateway_manager {
                (
                    manager.tool_search_enabled().await,
                    manager.tool_search_enabled_gateways().await,
                )
            } else {
                (false, Vec::new())
            };
        for svc in self.registry.services() {
            if self.service_visible_on_mcp(svc.name).await {
                tools.push(Tool::new(svc.name, svc.description, Arc::clone(&schema)));
                builtin_tool_count += 1;
            }
        }
        if tool_search_enabled {
            let tool_search_schema = match serde_json::json!({
                "type": "object",
                "properties": {
                    "query": { "type": "string", "maxLength": 500 },
                    "top_k": { "type": "integer", "minimum": 1, "maximum": 50 },
                    "include_schema": { "type": "boolean", "default": false }
                },
                "required": ["query"]
            }) {
                Value::Object(map) => Arc::new(map),
                _ => unreachable!("tool_search schema must be an object"),
            };
            tools.push(Tool::new(
                "tool_search",
                "Search the gateway's proxied upstream tool catalog",
                tool_search_schema,
            ));
            gateway_tool_count += 1;
            let tool_invoke_schema = match serde_json::json!({
                "type": "object",
                "properties": {
                    "name": { "type": "string" },
                    "arguments": { "type": "object" }
                },
                "required": ["name", "arguments"]
            }) {
                Value::Object(map) => Arc::new(map),
                _ => unreachable!("tool_invoke schema must be an object"),
            };
            tools.push(Tool::new(
                "tool_invoke",
                "Invoke one upstream tool discovered through tool_search",
                tool_invoke_schema,
            ));
            gateway_tool_count += 1;
        }

        // Merge upstream tools (healthy only, filtered for collisions with built-in services).
        if let Some(pool) = self.current_upstream_pool().await {
            let mut builtin_names = Vec::new();
            for service in self.registry.services() {
                if self.service_visible_on_mcp(service.name).await {
                    builtin_names.push(service.name);
                }
            }
            let upstream_tools = pool.healthy_tools().await;
            for ut in upstream_tools {
                if tool_search_enabled
                    && enabled_tool_search_gateways
                        .iter()
                        .any(|gateway| gateway == ut.upstream_name.as_ref())
                {
                    continue;
                }
                let tool_name = ut.tool.name.as_ref();
                if builtin_names.contains(&tool_name) {
                    tracing::debug!(
                        surface = "mcp",
                        service = "labby",
                        action = "tool.register",
                        tool = tool_name,
                        "skipping upstream tool that collides with built-in service"
                    );
                    continue;
                }
                tools.push(ut.tool);
                upstream_tool_count += 1;
            }
            if let Some(subject) = self.request_subject(&context) {
                for (_upstream_name, upstream_tools) in pool
                    .subject_scoped_tools(&self.oauth_upstream_configs().await, subject)
                    .await
                {
                    for ut in upstream_tools {
                        let tool_name = ut.name.as_ref();
                        if builtin_names.contains(&tool_name)
                            || tools.iter().any(|tool| tool.name.as_ref() == tool_name)
                        {
                            continue;
                        }
                        tools.push(ut);
                        subject_scoped_tool_count += 1;
                    }
                }
            }
        }

        let elapsed_ms = start.elapsed().as_millis();
        tracing::info!(
            surface = "mcp",
            service = "labby",
            action = "list_tools",
            subject,
            elapsed_ms,
            builtin_tool_count,
            gateway_tool_count,
            upstream_tool_count,
            subject_scoped_tool_count,
            total_tool_count = tools.len(),
            "tool list ok"
        );
        self.emit_dispatch_notification(
            &context,
            "lab",
            "list_tools",
            elapsed_ms,
            DispatchLogOutcome::Success,
        )
        .await;

        Ok(ListToolsResult::with_all_items(tools))
    }

    #[allow(clippy::too_many_lines)]
    async fn call_tool(
        &self,
        request: CallToolRequestParams,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, ErrorData> {
        let service = request.name.as_ref().to_string();
        let raw_arguments = request.arguments.clone();
        let args = request.arguments.unwrap_or_default();
        let action = args
            .get("action")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        let params = args.get("params").cloned().unwrap_or(Value::Null);
        let instance = params
            .get("instance")
            .and_then(Value::as_str)
            .map(ToOwned::to_owned);
        let param_key_count = params.as_object().map_or(0, serde_json::Map::len);

        let svc = self.registry.services().iter().find(|s| s.name == service);
        if service == "tool_search" {
            let query = args
                .get("query")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string();
            let requested_top_k = args
                .get("top_k")
                .and_then(Value::as_u64)
                .map(|value| value as usize);
            let include_schema = args
                .get("include_schema")
                .and_then(Value::as_bool)
                .unwrap_or(false);
            let Some(manager) = &self.gateway_manager else {
                let envelope = build_error(
                    &service,
                    "call_tool",
                    "unknown_tool",
                    "tool search is not enabled",
                );
                return Ok(CallToolResult::error(vec![Content::text(
                    envelope.to_string(),
                )]));
            };
            let top_k = match requested_top_k {
                Some(value) => value,
                None => manager.tool_search_config().await.top_k_default,
            };
            return match manager.search_tools(&query, top_k, include_schema).await {
                Ok(results) => Ok(CallToolResult::success(vec![Content::text(
                    serde_json::to_string(&results).unwrap_or_else(|_| "[]".to_string()),
                )])),
                Err(err) => {
                    let kind = err.kind();
                    let mut extra = serde_json::Map::new();
                    if kind == "index_warming" {
                        extra.insert("retry_after_ms".to_string(), serde_json::json!(2000));
                    }
                    if kind == "invalid_param" {
                        extra.insert("param".to_string(), serde_json::json!("query"));
                    }
                    let env = build_error_extra(
                        &service,
                        "call_tool",
                        kind,
                        &err.to_string(),
                        &Value::Object(extra),
                    );
                    Ok(CallToolResult::error(vec![Content::text(env.to_string())]))
                }
            };
        }
        if service == "tool_invoke" {
            let tool_name = args
                .get("name")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string();
            let arguments = args
                .get("arguments")
                .cloned()
                .filter(|value| value.is_object())
                .unwrap_or_else(|| serde_json::json!({}));
            let arguments_hash = hash_arguments(&arguments);
            let subject = self.request_subject_log_tag(&context);
            if !tool_invoke_scope_allowed(auth_context_from_extensions(&context.extensions)) {
                tracing::warn!(
                    surface = "mcp",
                    service = "tool_invoke",
                    action = "call_tool",
                    subject,
                    upstream_tool = %tool_name,
                    arguments_hash = %arguments_hash,
                    kind = "forbidden",
                    "gateway tool invoke denied by scope"
                );
                let env = build_error_extra(
                    &service,
                    "call_tool",
                    "forbidden",
                    "tool_invoke requires one of scopes: lab, lab:admin",
                    &serde_json::json!({ "required_scopes": ["lab", "lab:admin"] }),
                );
                return Ok(CallToolResult::error(vec![Content::text(env.to_string())]));
            }
            let Some(manager) = &self.gateway_manager else {
                let envelope = build_error(
                    &service,
                    "call_tool",
                    "unknown_tool",
                    "tool invoke is not enabled",
                );
                return Ok(CallToolResult::error(vec![Content::text(
                    envelope.to_string(),
                )]));
            };
            let resolved = manager.resolve_tool_invoke(&tool_name).await;
            let (upstream_name, _) = match resolved {
                Ok(value) => value,
                Err(crate::dispatch::error::ToolError::AmbiguousTool { message, valid }) => {
                    let mut extra = serde_json::Map::new();
                    extra.insert("valid".to_string(), serde_json::json!(valid));
                    let env = build_error_extra(
                        &service,
                        "call_tool",
                        "ambiguous_tool",
                        &message,
                        &Value::Object(extra),
                    );
                    return Ok(CallToolResult::error(vec![Content::text(env.to_string())]));
                }
                Err(err) => {
                    let kind = err.kind();
                    let mut extra = serde_json::Map::new();
                    if kind == "unknown_tool" {
                        extra.insert(
                            "hint".to_string(),
                            serde_json::json!("Call tool_search to discover available tools"),
                        );
                    }
                    let env = build_error_extra(
                        &service,
                        "call_tool",
                        kind,
                        &err.to_string(),
                        &Value::Object(extra),
                    );
                    return Ok(CallToolResult::error(vec![Content::text(env.to_string())]));
                }
            };
            if let Some(pool) = self.current_upstream_pool().await {
                let started = Instant::now();
                tracing::info!(
                    surface = "mcp",
                    service = "tool_invoke",
                    action = "call_tool",
                    subject,
                    upstream = %upstream_name,
                    upstream_tool = %tool_name,
                    arguments_hash = %arguments_hash,
                    "gateway tool invoke start"
                );
                let mut upstream_params = CallToolRequestParams::new(tool_name.clone());
                upstream_params.arguments = Some(match arguments {
                    Value::Object(map) => map,
                    _ => serde_json::Map::new(),
                });
                match pool.call_tool(&upstream_name, upstream_params).await {
                    Some(Ok(result)) => {
                        tracing::info!(
                            surface = "mcp",
                            service = "tool_invoke",
                            action = "call_tool",
                            subject,
                            upstream = %upstream_name,
                            upstream_tool = %tool_name,
                            arguments_hash = %arguments_hash,
                            elapsed_ms = started.elapsed().as_millis(),
                            "gateway tool invoke ok"
                        );
                        return Ok(result);
                    }
                    Some(Err(e)) => {
                        tracing::warn!(
                            surface = "mcp",
                            service = "tool_invoke",
                            action = "call_tool",
                            subject,
                            upstream = %upstream_name,
                            upstream_tool = %tool_name,
                            arguments_hash = %arguments_hash,
                            elapsed_ms = started.elapsed().as_millis(),
                            kind = "upstream_error",
                            error = %e,
                            "gateway tool invoke failed"
                        );
                        let env = build_error(&service, "call_tool", "upstream_error", &e);
                        return Ok(CallToolResult::error(vec![Content::text(env.to_string())]));
                    }
                    None => {
                        tracing::warn!(
                            surface = "mcp",
                            service = "tool_invoke",
                            action = "call_tool",
                            subject,
                            upstream = %upstream_name,
                            upstream_tool = %tool_name,
                            arguments_hash = %arguments_hash,
                            elapsed_ms = started.elapsed().as_millis(),
                            kind = "upstream_error",
                            "gateway tool invoke upstream disconnected"
                        );
                        let env = build_error(
                            &service,
                            "call_tool",
                            "upstream_error",
                            &format!("upstream `{upstream_name}` is not connected"),
                        );
                        return Ok(CallToolResult::error(vec![Content::text(env.to_string())]));
                    }
                }
            }
            // resolve_tool_invoke succeeded but no upstream pool is wired
            // yet (e.g. gateway manager present but runtime handle hasn't
            // swapped in a pool). Without this branch, execution falls
            // through to the catch-all "no dispatcher wired" error below,
            // which is misleading — surface a structured upstream_error
            // envelope instead.
            tracing::warn!(
                surface = "mcp",
                service = "tool_invoke",
                action = "call_tool",
                subject,
                upstream = %upstream_name,
                upstream_tool = %tool_name,
                arguments_hash = %arguments_hash,
                kind = "upstream_error",
                "gateway tool invoke dispatched without upstream pool"
            );
            let env = build_error(
                &service,
                "call_tool",
                "upstream_error",
                "no upstream pool available to dispatch tool_invoke",
            );
            return Ok(CallToolResult::error(vec![Content::text(env.to_string())]));
        }
        if svc.is_some() && !self.service_visible_on_mcp(&service).await {
            let envelope = build_error(
                &service,
                &action,
                "not_found",
                &format!("service `{service}` is not enabled on the mcp surface"),
            );
            return Ok(CallToolResult::error(vec![Content::text(
                envelope.to_string(),
            )]));
        }

        if svc.is_some() && !self.action_allowed_on_mcp(&service, &action).await {
            let mut extra = serde_json::Map::new();
            if let Some(valid) = self.allowed_mcp_actions(&service).await {
                extra.insert(
                    "valid".to_string(),
                    serde_json::to_value(valid).unwrap_or(Value::Array(Vec::new())),
                );
            }
            let envelope = build_error_extra(
                &service,
                &action,
                "unknown_action",
                &format!("action `{action}` is not exposed for service `{service}`"),
                &Value::Object(extra),
            );
            return Ok(CallToolResult::error(vec![Content::text(
                envelope.to_string(),
            )]));
        }

        // Elicitation gate: if the action is destructive and the client supports
        // elicitation, ask for confirmation before dispatching.
        if let Some(entry) = svc {
            let is_destructive = entry
                .actions
                .iter()
                .any(|a| a.name == action && a.destructive);
            if is_destructive {
                match elicit_confirm(&context, &service, &action).await {
                    ElicitResult::Confirmed => {}
                    ElicitResult::Declined | ElicitResult::Cancelled => {
                        let envelope = build_error(
                            &service,
                            &action,
                            "confirmation_required",
                            &format!("action `{action}` is destructive — confirm to proceed"),
                        );
                        return Ok(CallToolResult::error(vec![Content::text(
                            envelope.to_string(),
                        )]));
                    }
                    ElicitResult::NotSupported => {
                        // Client does not support elicitation — allow params["confirm"] == true
                        // as a machine-to-machine bypass (mirrors HTTP's handle_action()).
                        if params.get("confirm").and_then(Value::as_bool) != Some(true) {
                            let envelope = build_error(
                                &service,
                                &action,
                                "confirmation_required",
                                &format!(
                                    "action `{action}` is destructive — pass \
                                     {{\"confirm\":true}} in params or use a client \
                                     that supports MCP elicitation"
                                ),
                            );
                            return Ok(CallToolResult::error(vec![Content::text(
                                envelope.to_string(),
                            )]));
                        }
                    }
                    ElicitResult::Failed => {
                        let envelope = build_error(
                            &service,
                            &action,
                            "confirmation_required",
                            &format!(
                                "action `{action}` is destructive — confirmation failed, retry with a client that supports MCP elicitation"
                            ),
                        );
                        return Ok(CallToolResult::error(vec![Content::text(
                            envelope.to_string(),
                        )]));
                    }
                }
            }
        }

        let start = Instant::now();
        let subject = self.request_subject_log_tag(&context);
        let actor_key = self.request_actor_key(&context);
        let dispatch_action = if svc.is_some() {
            action.as_str()
        } else {
            "call_tool"
        };
        tracing::info!(
            surface = "mcp",
            service,
            action = dispatch_action,
            subject,
            actor_key,
            tool = %service,
            instance = instance.as_deref(),
            param_key_count,
            "dispatch start"
        );

        // Try built-in dispatch first.
        if let Some(entry) = svc {
            tracing::info!(
                surface = "mcp",
                service,
                action = action.as_str(),
                tool = %service,
                route = "builtin",
                "dispatch route selected"
            );
            let params = if service == "gateway" {
                inject_gateway_origin_param(params, self.request_subject(&context))
            } else {
                params
            };
            let result = (entry.dispatch)(action.clone(), params)
                .await
                .map_err(|te| anyhow::Error::from(DispatchError::from(te)));
            let elapsed_ms = start.elapsed().as_millis();
            let (result, outcome) =
                format_dispatch_result(result, &service, &action, elapsed_ms, &subject, actor_key);
            self.emit_dispatch_notification(&context, &service, &action, elapsed_ms, outcome)
                .await;
            return Ok(result);
        }

        // Fall through to upstream proxy dispatch.
        // Upstream tools don't use lab's action/params wrapper — they receive
        // raw arguments. Use "call_tool" as the action label for logging/envelopes.
        let upstream_action = "call_tool";
        let upstream_capability = "tools";
        let upstream_operation = "tool.call";
        if let Some(pool) = self.current_upstream_pool().await
            && let Some((upstream_name, _tool)) = pool.find_tool(&service).await
        {
            let before = self.snapshot_catalog().await;
            tracing::info!(
                surface = "mcp",
                service,
                action = upstream_action,
                tool = %service,
                upstream = %upstream_name,
                route = "upstream",
                "dispatch route selected"
            );
            tracing::debug!(
                surface = "mcp",
                service,
                action = upstream_action,
                tool = %service,
                upstream = %upstream_name,
                capability = upstream_capability,
                operation = upstream_operation,
                subject_scoped = false,
                "proxying to upstream"
            );

            let mut upstream_params = CallToolRequestParams::new(service.clone());
            upstream_params.arguments = raw_arguments;

            match pool.call_tool(&upstream_name, upstream_params).await {
                Some(Ok(result)) => {
                    let elapsed_ms = start.elapsed().as_millis();
                    let (result, kind, counts_as_failure) =
                        normalize_upstream_result(&service, upstream_action, result);
                    let outcome = if counts_as_failure || kind != "ok" {
                        DispatchLogOutcome::Failure {
                            level: if counts_as_failure {
                                LoggingLevel::Error
                            } else {
                                LoggingLevel::Warning
                            },
                            kind,
                        }
                    } else {
                        DispatchLogOutcome::Success
                    };
                    if counts_as_failure {
                        pool.record_failure(
                            &upstream_name,
                            format!("upstream `{upstream_name}` returned `{kind}`"),
                        )
                        .await;
                        tracing::warn!(
                            surface = "mcp",
                            service,
                            action = upstream_action,
                            tool = %service,
                            upstream = %upstream_name,
                            capability = upstream_capability,
                            operation = upstream_operation,
                            subject_scoped = false,
                            elapsed_ms,
                            kind,
                            "upstream proxy failed"
                        );
                    } else {
                        pool.record_success(&upstream_name).await;
                        tracing::info!(
                            surface = "mcp",
                            service,
                            action = upstream_action,
                            subject,
                            tool = %service,
                            upstream = %upstream_name,
                            capability = upstream_capability,
                            operation = upstream_operation,
                            subject_scoped = false,
                            elapsed_ms,
                            "upstream proxy ok"
                        );
                    }
                    self.emit_dispatch_notification(
                        &context,
                        &service,
                        upstream_action,
                        elapsed_ms,
                        outcome,
                    )
                    .await;
                    let after = self.snapshot_catalog().await;
                    self.notify_catalog_changes(&before, &after).await;
                    return Ok(result);
                }
                Some(Err(e)) => {
                    pool.record_failure(&upstream_name, e.clone()).await;
                    let after = self.snapshot_catalog().await;
                    self.notify_catalog_changes(&before, &after).await;
                    let elapsed_ms = start.elapsed().as_millis();
                    tracing::warn!(
                        surface = "mcp",
                        service,
                        action = upstream_action,
                        tool = %service,
                        upstream = %upstream_name,
                        capability = upstream_capability,
                        operation = upstream_operation,
                        subject_scoped = false,
                        elapsed_ms,
                        kind = "upstream_error",
                        error = %e,
                        "upstream proxy failed"
                    );
                    let envelope = build_error(
                        &service,
                        upstream_action,
                        "upstream_error",
                        &format!("upstream `{upstream_name}` call failed: {e}"),
                    );
                    self.emit_dispatch_notification(
                        &context,
                        &service,
                        upstream_action,
                        elapsed_ms,
                        DispatchLogOutcome::Failure {
                            level: LoggingLevel::Error,
                            kind: "upstream_error",
                        },
                    )
                    .await;
                    return Ok(CallToolResult::error(vec![Content::text(
                        envelope.to_string(),
                    )]));
                }
                None => {
                    // Connection is gone — record failure so the circuit
                    // breaker can eventually exclude this upstream.
                    pool.record_failure(
                        &upstream_name,
                        format!("upstream `{upstream_name}` is not connected"),
                    )
                    .await;
                    let after = self.snapshot_catalog().await;
                    self.notify_catalog_changes(&before, &after).await;
                    let elapsed_ms = start.elapsed().as_millis();
                    tracing::warn!(
                        surface = "mcp",
                        service,
                        action = upstream_action,
                        tool = %service,
                        upstream = %upstream_name,
                        capability = upstream_capability,
                        operation = upstream_operation,
                        subject_scoped = false,
                        elapsed_ms,
                        kind = "upstream_error",
                        error = "upstream disconnected",
                        "upstream not connected"
                    );
                    let envelope = build_error(
                        &service,
                        upstream_action,
                        "upstream_error",
                        &format!("upstream `{upstream_name}` is not connected"),
                    );
                    self.emit_dispatch_notification(
                        &context,
                        &service,
                        upstream_action,
                        elapsed_ms,
                        DispatchLogOutcome::Failure {
                            level: LoggingLevel::Error,
                            kind: "upstream_error",
                        },
                    )
                    .await;
                    return Ok(CallToolResult::error(vec![Content::text(
                        envelope.to_string(),
                    )]));
                }
            }
        }

        if let Some(subject) = self.request_subject(&context)
            && let Some(pool) = self.current_upstream_pool().await
        {
            let configs = self.oauth_upstream_configs().await;
            let mut owner = None;
            for (upstream_name, tools) in pool.subject_scoped_tools(&configs, subject).await {
                if tools.iter().any(|tool| tool.name.as_ref() == service) {
                    owner = Some(upstream_name);
                    break;
                }
            }

            if let Some(upstream_name) = owner
                && let Some(config) = configs
                    .into_iter()
                    .find(|config| config.name == upstream_name)
            {
                tracing::info!(
                    surface = "mcp",
                    service,
                    action = upstream_action,
                    tool = %service,
                    upstream = %upstream_name,
                    route = "subject_scoped",
                    "dispatch route selected"
                );
                let mut upstream_params = CallToolRequestParams::new(service.clone());
                upstream_params.arguments = raw_arguments;
                match pool
                    .subject_scoped_call_tool(&config, subject, upstream_params)
                    .await
                {
                    Ok(result) => {
                        let elapsed_ms = start.elapsed().as_millis();
                        let (result, kind, counts_as_failure) =
                            normalize_upstream_result(&service, upstream_action, result);
                        let outcome = if counts_as_failure || kind != "ok" {
                            tracing::warn!(
                                surface = "mcp",
                                service,
                                action = upstream_action,
                                tool = %service,
                                upstream = %upstream_name,
                                capability = upstream_capability,
                                operation = upstream_operation,
                                subject_scoped = true,
                                subject,
                                elapsed_ms,
                                kind,
                                "upstream dispatch error"
                            );
                            DispatchLogOutcome::Failure {
                                level: if counts_as_failure {
                                    LoggingLevel::Error
                                } else {
                                    LoggingLevel::Warning
                                },
                                kind,
                            }
                        } else {
                            tracing::info!(
                                surface = "mcp",
                                service,
                                action = upstream_action,
                                tool = %service,
                                upstream = %upstream_name,
                                capability = upstream_capability,
                                operation = upstream_operation,
                                subject_scoped = true,
                                subject,
                                elapsed_ms,
                                "upstream dispatch ok"
                            );
                            DispatchLogOutcome::Success
                        };
                        self.emit_dispatch_notification(
                            &context,
                            &service,
                            upstream_action,
                            elapsed_ms,
                            outcome,
                        )
                        .await;
                        return Ok(result);
                    }
                    Err(e) => {
                        let elapsed_ms = start.elapsed().as_millis();
                        tracing::warn!(
                            surface = "mcp",
                            service,
                            action = upstream_action,
                            tool = %service,
                            upstream = %upstream_name,
                            capability = upstream_capability,
                            operation = upstream_operation,
                            subject_scoped = true,
                            subject,
                            elapsed_ms,
                            kind = "upstream_error",
                            error = %e,
                            "upstream dispatch error"
                        );
                        let envelope = build_error(
                            &service,
                            upstream_action,
                            "upstream_error",
                            &format!("upstream `{upstream_name}` call failed: {e}"),
                        );
                        self.emit_dispatch_notification(
                            &context,
                            &service,
                            upstream_action,
                            elapsed_ms,
                            DispatchLogOutcome::Failure {
                                level: LoggingLevel::Error,
                                kind: "upstream_error",
                            },
                        )
                        .await;
                        return Ok(CallToolResult::error(vec![Content::text(
                            envelope.to_string(),
                        )]));
                    }
                }
            }
        }

        // Neither built-in nor upstream.
        let elapsed_ms = start.elapsed().as_millis();
        let err = anyhow::anyhow!("service `{service}` has no dispatcher wired");
        let (result, outcome) =
            format_dispatch_result(Err(err), &service, &action, elapsed_ms, &subject, actor_key);
        self.emit_dispatch_notification(&context, &service, &action, elapsed_ms, outcome)
            .await;
        Ok(result)
    }
}

use crate::mcp::catalog::CatalogSnapshot;

fn inject_gateway_origin_param(params: Value, subject: Option<&str>) -> Value {
    let raw = subject
        .map(|value| format!("mcp:{value}"))
        .unwrap_or_else(|| "mcp:anonymous".to_string());
    let Some(mut object) = params.as_object().cloned() else {
        return params;
    };
    object.entry("owner".to_string()).or_insert_with(|| {
        serde_json::json!({
            "surface": "mcp",
            "subject": subject,
            "raw": raw,
        })
    });
    object
        .entry("origin".to_string())
        .or_insert_with(|| Value::String(raw));
    Value::Object(object)
}

fn redact_subject_for_logging(subject: &str) -> String {
    let digest = Sha256::digest(subject.as_bytes());
    format!("sub:{}", hex::encode(digest))[..16].to_string()
}

impl LabMcpServer {
    fn request_subject<'a>(&self, context: &'a RequestContext<RoleServer>) -> Option<&'a str> {
        subject_from_extensions(&context.extensions)
    }

    fn request_subject_log_tag(&self, context: &RequestContext<RoleServer>) -> String {
        self.request_subject(context)
            .map(redact_subject_for_logging)
            .unwrap_or_default()
    }

    fn request_actor_key<'a>(&self, context: &'a RequestContext<RoleServer>) -> Option<&'a str> {
        actor_key_from_extensions(&context.extensions)
    }

    async fn oauth_upstream_configs(&self) -> Vec<crate::config::UpstreamConfig> {
        match &self.gateway_manager {
            Some(manager) => manager.oauth_upstream_configs().await,
            None => Vec::new(),
        }
    }

    async fn oauth_upstream_config(
        &self,
        upstream_name: &str,
    ) -> Option<crate::config::UpstreamConfig> {
        match &self.gateway_manager {
            Some(manager) => manager.oauth_upstream_config(upstream_name).await,
            None => None,
        }
    }

    async fn notify_catalog_changes(&self, before: &CatalogSnapshot, after: &CatalogSnapshot) {
        if before == after {
            return;
        }

        let peers = self.peers.read().await.clone();
        let peer_count = peers.len();
        tracing::info!(
            surface = "mcp",
            service = "peers",
            action = "catalog.notify",
            subsystem = "mcp_server",
            phase = "catalog.notify",
            peer_count,
            tools_changed = before.tools != after.tools,
            resources_changed = before.resources != after.resources,
            prompts_changed = before.prompts != after.prompts,
            "notifying MCP peers about catalog change"
        );
        let mut alive = Vec::with_capacity(peers.len());
        for (peer_index, peer) in peers.into_iter().enumerate() {
            let mut ok = true;
            if before.tools != after.tools {
                if peer.notify_tool_list_changed().await.is_err() {
                    tracing::warn!(
                        surface = "mcp",
                        service = "peers",
                        action = "peer.disconnect",
                        peer_index,
                        phase = "tools",
                        "failed to notify peer about tool catalog change; pruning stale session"
                    );
                    ok = false;
                }
            }
            if ok && before.resources != after.resources {
                if peer.notify_resource_list_changed().await.is_err() {
                    tracing::warn!(
                        surface = "mcp",
                        service = "peers",
                        action = "peer.disconnect",
                        peer_index,
                        phase = "resources",
                        "failed to notify peer about resource catalog change; pruning stale session"
                    );
                    ok = false;
                }
            }
            if ok && before.prompts != after.prompts {
                if peer.notify_prompt_list_changed().await.is_err() {
                    tracing::warn!(
                        surface = "mcp",
                        service = "peers",
                        action = "peer.disconnect",
                        peer_index,
                        phase = "prompts",
                        "failed to notify peer about prompt catalog change; pruning stale session"
                    );
                    ok = false;
                }
            }
            if ok {
                alive.push(peer);
            }
        }
        let mut guard = self.peers.write().await;
        let added_since_snapshot = if guard.len() > peer_count {
            guard.split_off(peer_count)
        } else {
            Vec::new()
        };
        let alive_count = alive.len();
        *guard = alive;
        guard.extend(added_since_snapshot);
        let pruned = peer_count.saturating_sub(alive_count);
        tracing::info!(
            surface = "mcp",
            service = "peers",
            action = "peer.gc",
            pruned_count = pruned,
            active_count = guard.len(),
            "MCP peer catalog-change notification complete"
        );
    }
}

fn subject_from_extensions(extensions: &rmcp::model::Extensions) -> Option<&str> {
    auth_context_from_extensions(extensions).map(|auth| auth.sub.as_str())
}

pub(crate) fn actor_key_from_extensions(extensions: &rmcp::model::Extensions) -> Option<&str> {
    auth_context_from_extensions(extensions).and_then(|auth| auth.actor_key.as_deref())
}

fn auth_context_from_extensions(
    extensions: &rmcp::model::Extensions,
) -> Option<&crate::api::oauth::AuthContext> {
    let parts = extensions.get::<Parts>()?;
    parts.extensions.get::<crate::api::oauth::AuthContext>()
}

fn tool_invoke_scope_allowed(auth: Option<&crate::api::oauth::AuthContext>) -> bool {
    auth.is_none_or(|auth| {
        auth.scopes
            .iter()
            .any(|scope| matches!(scope.as_str(), "lab" | "lab:admin"))
    })
}

fn hash_arguments(arguments: &Value) -> String {
    let bytes = serde_json::to_vec(arguments).unwrap_or_default();
    hex::encode(Sha256::digest(bytes))
}

/// Format the result of a dispatch operation into an MCP `CallToolResult`.
fn format_dispatch_result(
    result: Result<Value, anyhow::Error>,
    service: &str,
    action: &str,
    elapsed_ms: u128,
    subject: &str,
    actor_key: Option<&str>,
) -> (CallToolResult, DispatchLogOutcome) {
    match result {
        Ok(v) => {
            tracing::info!(
                surface = "mcp",
                service,
                action,
                subject,
                actor_key,
                tool = %service,
                elapsed_ms,
                "dispatch ok"
            );
            let envelope = build_success(service, action, &v);
            (
                CallToolResult::success(vec![Content::text(envelope.to_string())]),
                DispatchLogOutcome::Success,
            )
        }
        Err(e) => {
            let (kind, message, extra) = extract_error_info(&e);
            let is_fatal = matches!(kind, "internal_error" | "server_error" | "decode_error");
            if is_fatal {
                tracing::error!(
                    surface = "mcp",
                    service,
                    action,
                    subject,
                    actor_key,
                    tool = %service,
                    elapsed_ms,
                    kind,
                    "dispatch error"
                );
            } else {
                tracing::warn!(
                    surface = "mcp",
                    service,
                    action,
                    subject,
                    actor_key,
                    tool = %service,
                    elapsed_ms,
                    kind,
                    "dispatch error"
                );
            }
            let envelope = extra.map_or_else(
                || build_error(service, action, kind, &message),
                |ref extra| build_error_extra(service, action, kind, &message, extra),
            );
            (
                CallToolResult::error(vec![Content::text(envelope.to_string())]),
                DispatchLogOutcome::Failure {
                    level: if is_fatal {
                        LoggingLevel::Error
                    } else {
                        LoggingLevel::Warning
                    },
                    kind,
                },
            )
        }
    }
}

fn normalize_upstream_result(
    service: &str,
    action: &str,
    result: CallToolResult,
) -> (CallToolResult, &'static str, bool) {
    if result.is_error != Some(true) {
        return (result, "ok", false);
    }

    let Some(text) = result
        .content
        .first()
        .and_then(|content| content.as_text())
        .map(|content| content.text.as_str())
    else {
        let envelope = build_error(
            service,
            action,
            "upstream_error",
            "upstream returned a non-text error payload",
        );
        return (
            CallToolResult::error(vec![Content::text(envelope.to_string())]),
            "upstream_error",
            true,
        );
    };

    let Ok(parsed) = serde_json::from_str::<Value>(text) else {
        let envelope = build_error(service, action, "upstream_error", text);
        return (
            CallToolResult::error(vec![Content::text(envelope.to_string())]),
            "upstream_error",
            true,
        );
    };

    let error_obj = parsed
        .get("error")
        .and_then(Value::as_object)
        .or_else(|| parsed.as_object());

    let Some(error_obj) = error_obj else {
        let envelope = build_error(service, action, "upstream_error", text);
        return (
            CallToolResult::error(vec![Content::text(envelope.to_string())]),
            "upstream_error",
            true,
        );
    };

    let kind = error_obj
        .get("kind")
        .and_then(Value::as_str)
        .map(canonical_kind)
        .unwrap_or("upstream_error");
    let message = error_obj
        .get("message")
        .and_then(Value::as_str)
        .unwrap_or(text);

    let extra = serde_json::Map::from_iter(
        error_obj
            .iter()
            .filter(|(key, _)| *key != "kind" && *key != "message")
            .map(|(key, value)| (key.clone(), value.clone())),
    );

    let envelope = if extra.is_empty() {
        build_error(service, action, kind, message)
    } else {
        build_error_extra(service, action, kind, message, &Value::Object(extra))
    };

    (
        CallToolResult::error(vec![Content::text(envelope.to_string())]),
        kind,
        matches!(
            kind,
            "upstream_error" | "network_error" | "server_error" | "decode_error" | "internal_error"
        ),
    )
}

/// Recover a stable kind tag and message from an `anyhow::Error`.
///
/// Priority:
/// 1. Downcast to [`DispatchError`] — gives structured kind + optional extras.
/// 2. Parse `e.to_string()` as JSON `{ "kind": "…" }` — covers `ToolError`
///    errors that were serialized to string before entering anyhow (radarr).
/// 3. Fall back to `"internal_error"`.
pub fn extract_error_info(e: &anyhow::Error) -> (&'static str, String, Option<Value>) {
    // 1. Structured DispatchError
    if let Some(de) = e.downcast_ref::<DispatchError>() {
        let extra = if de.valid.is_some() || de.param.is_some() || de.hint.is_some() {
            Some(serde_json::json!({
                "valid": de.valid,
                "param": de.param,
                "hint":  de.hint,
            }))
        } else {
            None
        };
        return (de.kind, de.message.clone(), extra);
    }
    // 2. ToolError serialized as JSON string (legacy radarr path)
    let msg = e.to_string();
    if let Ok(v) = serde_json::from_str::<Value>(&msg)
        && let Some(kind_str) = v.get("kind").and_then(|k| k.as_str())
    {
        let kind: &'static str = canonical_kind(kind_str);
        let message = v["message"].as_str().unwrap_or(&msg).to_string();
        // Preserve structured extras (valid list, param name, hint) if present.
        let has_valid = v.get("valid").is_some_and(|v| !v.is_null());
        let has_param = v.get("param").is_some_and(|v| !v.is_null());
        let has_hint = v.get("hint").is_some_and(|v| !v.is_null());
        let extra = if has_valid || has_param || has_hint {
            Some(serde_json::json!({
                "valid": v.get("valid"),
                "param": v.get("param"),
                "hint":  v.get("hint"),
            }))
        } else {
            None
        };
        return (kind, message, extra);
    }
    // 3. Generic fallback
    ("internal_error", msg, None)
}

#[cfg(test)]
mod tests {
    use super::{extract_error_info, logging_level_rank, normalize_upstream_result};
    use crate::dispatch::error::ToolError;
    use crate::mcp::envelope::build_error;
    use crate::mcp::error::{DispatchError, canonical_kind};
    use crate::registry::{RegisteredService, ToolRegistry};
    use lab_apis::core::action::ActionSpec;
    use rmcp::ServerHandler;
    use rmcp::model::{CallToolResult, Content};
    use serde_json::Value;
    use std::future::Future;

    #[tokio::test]
    async fn extract_error_info_preserves_unknown_action_from_real_dispatch_downcast() {
        let err = crate::dispatch::lab_admin::dispatch("definitely.unknown", serde_json::json!({}))
            .await
            .expect_err("unknown lab_admin action should fail");
        let dispatch_error = DispatchError::from(err);
        let anyhow_error = anyhow::Error::from(dispatch_error);

        let (kind, message, extra) = extract_error_info(&anyhow_error);

        assert_eq!(kind, "unknown_action");
        assert_eq!(message, "unknown action `lab_admin.definitely.unknown`");
        let extra = extra.expect("unknown_action should preserve valid action extras");
        assert_eq!(extra["valid"][0], "help");
        assert_eq!(extra["param"], Value::Null);
        assert_eq!(extra["hint"], Value::Null);
    }

    #[test]
    fn extract_error_info_preserves_unknown_action_from_json_fallback() {
        let serialized = serde_json::json!({
            "kind": "unknown_action",
            "message": "unknown action `movie.serch` for service `radarr`",
            "valid": ["movie.search", "movie.add"],
            "hint": "movie.search"
        })
        .to_string();
        let anyhow_error = anyhow::anyhow!(serialized);

        let (kind, message, extra) = extract_error_info(&anyhow_error);

        assert_eq!(kind, "unknown_action");
        assert_eq!(message, "unknown action `movie.serch` for service `radarr`");
        let extra = extra.expect("json fallback should preserve structured extras");
        assert_eq!(
            extra["valid"],
            serde_json::json!(["movie.search", "movie.add"])
        );
        assert_eq!(extra["param"], Value::Null);
        assert_eq!(extra["hint"], serde_json::json!("movie.search"));
    }

    /// Every kind that `ToolError::kind()` can return must have an explicit arm
    /// in `canonical_kind()`.  If a new variant or SDK kind is added to `ToolError`
    /// without a matching arm here, this test will catch the silent downgrade to
    /// `"internal_error"`.
    #[test]
    fn canonical_kind_round_trips_all_tool_error_kinds() {
        // Fixed-variant kinds — produced by the named ToolError variants.
        let fixed_variants: &[ToolError] = &[
            ToolError::UnknownAction {
                message: String::new(),
                valid: vec![],
                hint: None,
            },
            ToolError::MissingParam {
                message: String::new(),
                param: "p".into(),
            },
            ToolError::InvalidParam {
                message: String::new(),
                param: "p".into(),
            },
            ToolError::UnknownInstance {
                message: String::new(),
                valid: vec![],
            },
        ];

        for err in fixed_variants {
            let kind = err.kind();
            assert_eq!(
                canonical_kind(kind),
                kind,
                "canonical_kind({kind:?}) should round-trip but returns \"{}\"",
                canonical_kind(kind),
            );
        }

        // SDK-promoted kinds — every stable kind tag that `ApiError::kind()` can
        // return and that `ToolError::Sdk` promotes to the top-level `kind` field.
        let sdk_kinds: &[&str] = &[
            "unknown_action",
            "unknown_subaction",
            "missing_param",
            "invalid_param",
            "unknown_instance",
            "auth_failed",
            "not_found",
            "rate_limited",
            "validation_failed",
            "network_error",
            "server_error",
            "decode_error",
            "confirmation_required",
        ];

        for &sdk_kind in sdk_kinds {
            let err = ToolError::Sdk {
                sdk_kind: sdk_kind.to_string(),
                message: String::new(),
            };
            let kind = err.kind();
            assert_eq!(
                canonical_kind(kind),
                kind,
                "canonical_kind({kind:?}) should round-trip but returns \"{}\"",
                canonical_kind(kind),
            );
        }
    }

    #[test]
    fn normalize_upstream_result_preserves_user_errors_without_poisoning_health() {
        let upstream = CallToolResult::error(vec![Content::text(
            build_error("radarr", "movie.add", "missing_param", "need title").to_string(),
        )]);

        let (_, kind, counts_as_failure) =
            normalize_upstream_result("radarr", "call_tool", upstream);

        assert_eq!(kind, "missing_param");
        assert!(!counts_as_failure);
    }

    #[test]
    fn server_capabilities_advertise_list_changed_support() {
        let server = super::LabMcpServer {
            registry: std::sync::Arc::new(ToolRegistry::new()),
            gateway_manager: None,
            node_role: None,
            peers: std::sync::Arc::new(tokio::sync::RwLock::new(Vec::new())),
            logging_level: std::sync::Arc::new(std::sync::atomic::AtomicU8::new(
                logging_level_rank(rmcp::model::LoggingLevel::Info),
            )),
        };

        let info = server.get_info();
        assert_eq!(
            info.capabilities.tools.and_then(|c| c.list_changed),
            Some(true)
        );
        assert_eq!(
            info.capabilities.resources.and_then(|c| c.list_changed),
            Some(true)
        );
        assert_eq!(
            info.capabilities.prompts.and_then(|c| c.list_changed),
            Some(true)
        );
        assert!(
            info.capabilities.logging.is_some(),
            "RMCP logging capability must be advertised"
        );
        assert!(
            info.capabilities.completions.is_some(),
            "RMCP completion capability must be advertised"
        );
    }

    const TEST_ACTIONS_ONE: &[ActionSpec] = &[
        ActionSpec {
            name: "queue.list",
            description: "List queue",
            destructive: false,
            params: &[],
            returns: "object",
        },
        ActionSpec {
            name: "movie.search",
            description: "Search movies",
            destructive: false,
            params: &[],
            returns: "object",
        },
    ];

    const TEST_ACTIONS_TWO: &[ActionSpec] = &[
        ActionSpec {
            name: "calendar.list",
            description: "List calendar",
            destructive: false,
            params: &[],
            returns: "object",
        },
        ActionSpec {
            name: "movie.lookup",
            description: "Look up movie",
            destructive: false,
            params: &[],
            returns: "object",
        },
    ];

    fn noop_dispatch(
        _action: String,
        _params: Value,
    ) -> std::pin::Pin<Box<dyn Future<Output = Result<Value, ToolError>> + Send>> {
        Box::pin(async { Ok(Value::Null) })
    }

    fn completion_test_registry() -> ToolRegistry {
        let mut registry = ToolRegistry::new();
        registry.register(RegisteredService {
            name: "radarr",
            description: "Movies",
            category: "media",
            kind: crate::registry::RegisteredServiceKind::BuiltInUpstreamApi,
            status: "available",
            actions: TEST_ACTIONS_ONE,
            dispatch: noop_dispatch,
        });
        registry.register(RegisteredService {
            name: "sonarr",
            description: "Shows",
            category: "media",
            kind: crate::registry::RegisteredServiceKind::BuiltInUpstreamApi,
            status: "available",
            actions: TEST_ACTIONS_TWO,
            dispatch: noop_dispatch,
        });
        registry
    }

    #[test]
    fn completion_run_action_empty_action_prefix_uses_cached_action_names() {
        let registry = completion_test_registry();

        let completion = super::complete_prompt_arg(&registry, "run-action", "action", "");

        assert_eq!(completion.values, registry.action_name_completions(""));
        assert_eq!(completion.total, Some(registry.action_names().len() as u32));
        assert_eq!(completion.has_more, Some(false));
    }

    #[test]
    fn completion_run_action_action_prefix_filters_cached_action_names() {
        let registry = completion_test_registry();

        let completion = super::complete_prompt_arg(&registry, "run-action", "action", "movie.");

        assert_eq!(
            completion.values,
            vec!["movie.lookup".to_string(), "movie.search".to_string()]
        );
    }

    #[test]
    fn completion_prompt_service_arguments_filter_service_names() {
        let registry = completion_test_registry();

        let run_action = super::complete_prompt_arg(&registry, "run-action", "service", "ra");
        let discover = super::complete_prompt_arg(&registry, "service-discover", "service", "so");

        assert_eq!(run_action.values, vec!["radarr".to_string()]);
        assert_eq!(discover.values, vec!["sonarr".to_string()]);
    }

    #[test]
    fn completion_unknown_prompt_argument_returns_empty_result() {
        let registry = completion_test_registry();

        let completion = super::complete_prompt_arg(&registry, "run-action", "params", "{");

        assert!(completion.values.is_empty());
        assert_eq!(completion.total, Some(0));
        assert_eq!(completion.has_more, Some(false));
    }

    #[tokio::test]
    async fn server_reads_current_pool_from_gateway_manager() {
        let runtime = crate::dispatch::gateway::manager::GatewayRuntimeHandle::default();
        let manager = std::sync::Arc::new(crate::dispatch::gateway::manager::GatewayManager::new(
            std::path::PathBuf::from("config.toml"),
            runtime.clone(),
        ));
        let notifier = super::PeerNotifier::default();
        let server = super::LabMcpServer {
            registry: std::sync::Arc::new(ToolRegistry::new()),
            gateway_manager: Some(std::sync::Arc::clone(&manager)),
            node_role: None,
            peers: std::sync::Arc::clone(&notifier.peers),
            logging_level: std::sync::Arc::new(std::sync::atomic::AtomicU8::new(
                logging_level_rank(rmcp::model::LoggingLevel::Info),
            )),
        };

        assert!(server.current_upstream_pool().await.is_none());

        let pool = std::sync::Arc::new(crate::dispatch::upstream::pool::UpstreamPool::new());
        runtime.swap(Some(std::sync::Arc::clone(&pool))).await;

        let current = server.current_upstream_pool().await.expect("pool");
        assert!(std::sync::Arc::ptr_eq(&current, &pool));
    }

    #[tokio::test]
    async fn snapshot_catalog_hides_mcp_disabled_virtual_services() {
        let runtime = crate::dispatch::gateway::manager::GatewayRuntimeHandle::default();
        let manager = std::sync::Arc::new(crate::dispatch::gateway::manager::GatewayManager::new(
            std::path::PathBuf::from("config.toml"),
            runtime,
        ));
        manager
            .seed_config(crate::config::LabConfig {
                virtual_servers: vec![crate::config::VirtualServerConfig {
                    id: "plex".to_string(),
                    service: "plex".to_string(),
                    enabled: true,
                    surfaces: crate::config::VirtualServerSurfacesConfig {
                        cli: false,
                        api: false,
                        mcp: false,
                        webui: false,
                    },
                    mcp_policy: None,
                }],
                ..crate::config::LabConfig::default()
            })
            .await;

        let server = super::LabMcpServer {
            registry: std::sync::Arc::new(crate::registry::build_default_registry()),
            gateway_manager: Some(manager),
            node_role: None,
            peers: std::sync::Arc::new(tokio::sync::RwLock::new(Vec::new())),
            logging_level: std::sync::Arc::new(std::sync::atomic::AtomicU8::new(
                logging_level_rank(rmcp::model::LoggingLevel::Info),
            )),
        };

        let snapshot = server.snapshot_catalog().await;
        assert!(!snapshot.tools.contains("plex"));
    }

    #[tokio::test]
    async fn service_actions_json_filters_to_allowed_mcp_actions() {
        let runtime = crate::dispatch::gateway::manager::GatewayRuntimeHandle::default();
        let manager = std::sync::Arc::new(crate::dispatch::gateway::manager::GatewayManager::new(
            std::path::PathBuf::from("config.toml"),
            runtime,
        ));
        manager
            .seed_config(crate::config::LabConfig {
                virtual_servers: vec![crate::config::VirtualServerConfig {
                    id: "plex".to_string(),
                    service: "plex".to_string(),
                    enabled: true,
                    surfaces: crate::config::VirtualServerSurfacesConfig {
                        cli: false,
                        api: false,
                        mcp: true,
                        webui: false,
                    },
                    mcp_policy: Some(crate::config::VirtualServerMcpPolicyConfig {
                        allowed_actions: vec!["server.info".to_string()],
                    }),
                }],
                ..crate::config::LabConfig::default()
            })
            .await;

        let server = super::LabMcpServer {
            registry: std::sync::Arc::new(crate::registry::build_default_registry()),
            gateway_manager: Some(manager),
            node_role: None,
            peers: std::sync::Arc::new(tokio::sync::RwLock::new(Vec::new())),
            logging_level: std::sync::Arc::new(std::sync::atomic::AtomicU8::new(
                logging_level_rank(rmcp::model::LoggingLevel::Info),
            )),
        };

        let value = server
            .service_actions_json("plex")
            .await
            .expect("service actions");
        let actions = value.as_array().expect("array");
        assert!(actions.iter().any(|action| action["name"] == "help"));
        assert!(actions.iter().any(|action| action["name"] == "schema"));
        assert!(actions.iter().any(|action| action["name"] == "server.info"));
        assert!(
            !actions
                .iter()
                .any(|action| action["name"] == "session.list")
        );
    }

    #[test]
    fn server_reads_subject_scoped_upstream_pool_from_request_extensions() {
        let mut parts = axum::http::Request::new(()).into_parts().0;
        parts.extensions.insert(crate::api::oauth::AuthContext {
            sub: "alice".to_string(),
            actor_key: Some(std::sync::Arc::<str>::from("actor-alice")),
            scopes: vec!["lab".to_string()],
            issuer: "https://lab.example.com".to_string(),
            via_session: true,
            csrf_token: None,
            email: Some("alice@example.com".to_string()),
        });

        let mut extensions = rmcp::model::Extensions::new();
        extensions.insert(parts);

        assert_eq!(super::subject_from_extensions(&extensions), Some("alice"));
        assert_eq!(
            super::actor_key_from_extensions(&extensions),
            Some("actor-alice")
        );
    }

    #[test]
    fn upstream_subject_resolution_self_test_passes_for_plan_a() {
        super::verify_upstream_subject_resolution_support().expect("self-test");
    }
}
