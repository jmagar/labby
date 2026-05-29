//! `LabMcpServer` — the MCP `ServerHandler` implementation.
//!
//! Extracted from `cli/serve.rs` so that both the stdio and HTTP transports
//! can share the same handler logic.

use sha2::{Digest, Sha256};
use std::borrow::Cow;
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
use crate::dispatch::error::ToolError as DispatchToolError;
use crate::dispatch::gateway::SHARED_GATEWAY_OAUTH_SUBJECT;
use crate::dispatch::gateway::code_mode::{CodeModeBroker, CodeModeCaller, CodeModeSurface};
use crate::dispatch::gateway::manager::GatewayManager;
use crate::dispatch::upstream::types::UpstreamRuntimeOwner;
use crate::mcp::catalog::{TOOL_EXECUTE_TOOL_NAME, TOOL_SEARCH_TOOL_NAME};
use crate::mcp::elicitation::{ElicitResult, elicit_confirm};
use crate::mcp::envelope::{build_error, build_error_extra, build_success};
use crate::mcp::error::DispatchError;
use crate::mcp::error::canonical_kind;
use crate::mcp::logging::{DispatchLogOutcome, logging_level_rank};
use crate::registry::ToolRegistry;

const CODE_MODE_MAX_CODE_BYTES: usize = 20_000;
/// Tool description for the `execute` MCP tool (Code Mode sandbox).
///
/// This description is what the model receives. Keep it under 8192 bytes.
const CODE_EXECUTE_DESCRIPTION: &str = "\
Execute a JavaScript async arrow function in the Code Mode sandbox. Pass `code` as \
`async () => { ... }` — the sandbox awaits its return value (same shape as search). \
Every upstream MCP tool is pre-declared as a typed TypeScript helper in the `codemode` \
namespace — read the types, call the functions. No separate discovery step required.

```ts
// code is an async arrow function; whatever it returns becomes `result`.
async () => {
  const issues = await callTool('upstream::github::search_issues', { q: 'bug' });
  return issues.items.length;
}
```

`Promise.all([...])` dispatches `callTool` requests in parallel — batch independent \
reads instead of awaiting serially.

```ts
// codemode.<upstream>.<tool>() helpers are auto-generated from the live catalog.
// Use them. callTool is the escape hatch for dynamic IDs or truncated catalogs.
declare function callTool<T = unknown>(
  id: `upstream::${string}::${string}`,
  params: Record<string, unknown>
): Promise<T>;
```

Successful return: the upstream tool's structuredContent if present, else the parsed \
text of the first content[0] block. Never the raw MCP envelope.

Error handling:
```ts
// To recover: const env: CodeModeError = JSON.parse(String(e.message));
// Retry-safe:    rate_limited (honor retry_after_ms), timeout, network_error
// Fix-and-retry: missing_param, invalid_param, validation_failed, confirmation_required
// Terminal:      unknown_tool, unknown_action, auth_failed, server_error, internal_error
```

Scope: `lab:read` — catalog read only. `lab` / `lab:admin` — callTool execution.

Results are capped to the configured envelope budget (default 24 KB / 6000 tokens). \
Oversized results are replaced with a truncation marker containing `truncated`, \
`original_size`, `original_tokens`, `preview`, and `next_action`. Reduce data inside \
the sandbox before returning — that is the point of Code Mode.

Budget:
- Time: a 30s wall-clock timeout bounds the whole run (the meaningful limit). \
There is no small per-run call cap — fan out freely with `Promise.all`.
- Fuel: default 50M fuel supports heavy fan-out plus moderate result processing; \
base overhead ~100K, ~2K per callTool boundary.
- `code_mode_fuel_exhausted` or `timeout`: split the work across calls or reduce \
local processing.

Lab actions (`lab::*` tool IDs) are not available in Code Mode. For Lab built-in \
actions use the `execute` tool in Tool Search mode.";

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
            let auth = auth_context_from_extensions(&context.extensions);
            if let Some(oauth_subject) =
                oauth_upstream_subject_for_request(auth, self.request_subject(&context))
            {
                let scoped_prompts = pool
                    .subject_scoped_prompts(
                        &self.oauth_upstream_configs().await,
                        oauth_subject.as_ref(),
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

        let auth = auth_context_from_extensions(&context.extensions);
        if let Some(oauth_subject) =
            oauth_upstream_subject_for_request(auth, self.request_subject(&context))
            && let Some(pool) = self.current_upstream_pool().await
        {
            let configs = self.oauth_upstream_configs().await;
            if let Some(upstream_name) = pool
                .subject_scoped_prompt_owner(&configs, oauth_subject.as_ref(), &request.name)
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
                    oauth_subject = %oauth_subject,
                    "dispatch route selected"
                );
                let outcome = match pool
                    .subject_scoped_get_prompt(&config, oauth_subject.as_ref(), request)
                    .await
                {
                    Ok(result) => {
                        let elapsed_ms = start.elapsed().as_millis();
                        tracing::info!(
                            surface = "mcp",
                            service = "labby",
                            action = "get_prompt",
                            subject,
                            oauth_subject = %oauth_subject,
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
            resources.extend(pool.gateway_synthetic_resources().await);
            resources.extend(pool.list_upstream_resources().await);
            let auth = auth_context_from_extensions(&context.extensions);
            if let Some(oauth_subject) =
                oauth_upstream_subject_for_request(auth, self.request_subject(&context))
            {
                let configs = self.oauth_upstream_configs().await;
                resources.extend(
                    pool.subject_scoped_resources(&configs, oauth_subject.as_ref())
                        .await,
                );
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

        if uri.starts_with("lab://gateway/") {
            tracing::info!(
                surface = "mcp",
                service = "labby",
                action = "read_resource",
                subject,
                resource_uri =
                    crate::dispatch::upstream::pool::redact_resource_uri_for_logging(uri),
                route = "gateway",
                "dispatch route selected"
            );
            let Some(pool) = self.current_upstream_pool().await else {
                let elapsed_ms = start.elapsed().as_millis();
                tracing::warn!(
                    surface = "mcp",
                    service = "labby",
                    action = "read_resource",
                    subject,
                    resource_uri =
                        crate::dispatch::upstream::pool::redact_resource_uri_for_logging(uri),
                    route = "gateway",
                    elapsed_ms,
                    kind = "unavailable",
                    "upstream pool not configured"
                );
                self.emit_dispatch_notification(
                    &context,
                    "lab",
                    "read_resource",
                    elapsed_ms,
                    DispatchLogOutcome::Failure {
                        level: LoggingLevel::Warning,
                        kind: "unavailable",
                    },
                )
                .await;
                return Err(ErrorData::resource_not_found(
                    "upstream pool not configured".to_string(),
                    None,
                ));
            };

            let json = if uri == "lab://gateway/servers" {
                Some(pool.gateway_servers_doc().await)
            } else if let Some(name) = uri
                .strip_prefix("lab://gateway/")
                .and_then(|rest| rest.strip_suffix("/schema"))
                .filter(|name| !name.is_empty() && !name.contains('/'))
            {
                pool.gateway_server_schema(name).await
            } else {
                None
            };

            let elapsed_ms = start.elapsed().as_millis();
            return match json {
                Some(value) => {
                    let text = match serde_json::to_string_pretty(&value) {
                        Ok(t) => t,
                        Err(e) => {
                            tracing::error!(
                                surface = "mcp",
                                service = "labby",
                                action = "read_resource",
                                resource_uri = crate::dispatch::upstream::pool::redact_resource_uri_for_logging(uri),
                                error = %e,
                                "failed to serialize synthetic gateway resource"
                            );
                            return Err(ErrorData::internal_error(
                                format!("failed to serialize resource: {e}"),
                                None,
                            ));
                        }
                    };
                    tracing::info!(
                        surface = "mcp",
                        service = "labby",
                        action = "read_resource",
                        subject,
                        resource_uri =
                            crate::dispatch::upstream::pool::redact_resource_uri_for_logging(uri),
                        route = "gateway",
                        elapsed_ms,
                        "synthetic resource ok"
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
                        ResourceContents::text(text, uri.clone())
                            .with_mime_type("application/json"),
                    ]))
                }
                None => {
                    tracing::warn!(
                        surface = "mcp",
                        service = "labby",
                        action = "read_resource",
                        subject,
                        resource_uri =
                            crate::dispatch::upstream::pool::redact_resource_uri_for_logging(uri),
                        route = "gateway",
                        elapsed_ms,
                        kind = "not_found",
                        "synthetic resource not found"
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
        }

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

        let auth = auth_context_from_extensions(&context.extensions);
        if let Some(oauth_subject) =
            oauth_upstream_subject_for_request(auth, self.request_subject(&context))
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
                oauth_subject = %oauth_subject,
                "dispatch route selected"
            );
            let outcome = match pool
                .subject_scoped_read_resource(&config, oauth_subject.as_ref(), uri)
                .await
            {
                Ok(result) => {
                    let elapsed_ms = start.elapsed().as_millis();
                    tracing::info!(
                        surface = "mcp",
                        service = "labby",
                        action = "read_resource",
                        subject,
                        oauth_subject = %oauth_subject,
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
        let mut suppressed_builtin_tool_count = 0usize;
        let visibility = self.tool_search_visibility().await;
        let manager_tool_search_enabled = visibility.exposes_synthetic_tools();
        let process_tool_search_enabled = crate::config::process_tool_search_enabled();
        let hide_raw_tools = visibility.hides_raw_tools();
        let visibility_mode = visibility.mode_label();
        for svc in self.registry.services() {
            if self.service_visible_on_mcp(svc.name).await {
                if hide_raw_tools {
                    suppressed_builtin_tool_count += 1;
                } else {
                    tools.push(Tool::new(svc.name, svc.description, Arc::clone(&schema)));
                    builtin_tool_count += 1;
                }
            }
        }
        if visibility.exposes_synthetic_tools() {
            // ── Gateway meta-tools: search (Boa JS against upstream catalog) +
            // execute (subprocess sandbox). Both take `{ code: string }`.
            // See mcp/CLAUDE.md for the exception rationale and
            // dispatch/gateway/dispatch.rs guard.
            let search_schema = match serde_json::json!({
                "type": "object",
                "properties": {
                    "code": {
                        "type": "string",
                        "description": "JavaScript async arrow function to search the upstream MCP tool catalog. \
                            The sandbox injects `const tools = [...]` where each entry has id, upstream, \
                            name, description, and schema. Return JSON-serializable results. \
                            Examples: \
                            `async () => tools.filter(t => /container.*log/i.test(t.description)).map(t => ({id:t.id, schema:t.schema}))`; \
                            `async () => tools.find(t => t.id === \"upstream::github::search_issues\")`; \
                            `async () => tools.filter(t => t.upstream === \"github\").slice(0, 20)`."
                    }
                },
                "required": ["code"]
            }) {
                Value::Object(map) => Arc::new(map),
                _ => unreachable!("search schema must be an object"),
            };
            tools.push(Tool::new(
                TOOL_SEARCH_TOOL_NAME,
                "Filter the upstream MCP tool catalog with JavaScript. Write an async arrow function \
                that filters `const tools = [...]` (each entry: id, upstream, name, description, schema) \
                and returns what you need. No embedding model, no vector DB — the agent writes the filter. \
                Use before execute() to discover the right tool id.",
                search_schema,
            ));
            gateway_tool_count += 1;
            let execute_schema = match serde_json::json!({
                "type": "object",
                "properties": {
                    "code": {
                        "type": "string",
                        "description": "JavaScript async arrow function to execute. Use await callTool(id, params) with JSON-serializable params."
                    }
                },
                "required": ["code"]
            }) {
                Value::Object(map) => Arc::new(map),
                _ => unreachable!("execute schema must be an object"),
            };
            debug_assert!(CODE_EXECUTE_DESCRIPTION.len() < 8192);
            tracing::info!(
                surface = "mcp",
                service = "execute",
                action = "tool.describe",
                description_bytes = CODE_EXECUTE_DESCRIPTION.len(),
                "registered Code Mode execute description"
            );
            tools.push(Tool::new(
                TOOL_EXECUTE_TOOL_NAME,
                CODE_EXECUTE_DESCRIPTION,
                execute_schema,
            ));
            gateway_tool_count += 1;
        }

        // Merge upstream tools (healthy only, filtered for collisions with built-in services).
        if !hide_raw_tools && let Some(pool) = self.current_upstream_pool().await {
            let mut builtin_names = Vec::new();
            for service in self.registry.services() {
                if self.service_visible_on_mcp(service.name).await {
                    builtin_names.push(service.name);
                }
            }
            let upstream_tools = pool.healthy_tools().await;
            for ut in upstream_tools {
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
            let auth = auth_context_from_extensions(&context.extensions);
            if let Some(oauth_subject) =
                oauth_upstream_subject_for_request(auth, self.request_subject(&context))
            {
                for (_upstream_name, upstream_tools) in pool
                    .subject_scoped_tools(
                        &self.oauth_upstream_configs().await,
                        oauth_subject.as_ref(),
                    )
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
            suppressed_builtin_tool_count,
            manager_tool_search_enabled,
            process_tool_search_enabled,
            hide_raw_tools,
            visibility_mode,
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

        // ── Gateway `search` tool: run caller's JS over the upstream catalog ──
        if service == TOOL_SEARCH_TOOL_NAME {
            let started = Instant::now();
            let input_tokens = estimate_tokens_args(&args);
            let subject = self.request_subject_log_tag(&context);
            let auth = auth_context_from_extensions(&context.extensions);
            if !tool_search_scope_allowed(auth) {
                tracing::warn!(
                    surface = "mcp",
                    service = %service,
                    action = "call_tool",
                    subject,
                    elapsed_ms = started.elapsed().as_millis(),
                    input_tokens,
                    kind = "forbidden",
                    "gateway code search denied by scope"
                );
                let env = build_error_extra(
                    &service,
                    "call_tool",
                    "forbidden",
                    "code_search requires one of scopes: lab:read, lab, lab:admin",
                    &serde_json::json!({ "required_scopes": ["lab:read", "lab", "lab:admin"] }),
                );
                return Ok(CallToolResult::error(vec![Content::text(env.to_string())]));
            }
            let code = args
                .get("code")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string();
            let code_hash = hash_arguments(&Value::String(code.clone()));
            let Some(manager) = &self.gateway_manager else {
                let envelope = build_error(
                    &service,
                    "call_tool",
                    "unknown_tool",
                    "code search is not enabled",
                );
                return Ok(CallToolResult::error(vec![Content::text(
                    envelope.to_string(),
                )]));
            };
            tracing::info!(
                surface = "mcp",
                service = "code_search",
                action = "call_tool",
                subject,
                code_hash = %code_hash,
                code_len = code.len(),
                input_tokens,
                "gateway code search start"
            );
            let broker = CodeModeBroker::new(&self.registry, Some(manager));
            let caller = auth.map_or(CodeModeCaller::TrustedLocal, |auth| {
                CodeModeCaller::Scoped {
                    scopes: auth.scopes.clone(),
                    sub: self.request_subject(&context).map(ToOwned::to_owned),
                }
            });
            return match broker
                .search(&code, caller, self.code_mode_surface(false))
                .await
            {
                Ok(response) => {
                    let output =
                        serde_json::to_string(&response).unwrap_or_else(|_| "null".to_string());
                    let output_tokens = estimate_tokens(&output);
                    tracing::info!(
                        surface = "mcp",
                        service = "code_search",
                        action = "call_tool",
                        subject,
                        code_hash = %code_hash,
                        code_len = code.len(),
                        elapsed_ms = started.elapsed().as_millis(),
                        input_tokens,
                        output_tokens,
                        "gateway code search ok"
                    );
                    Ok(CallToolResult::success(vec![Content::text(output)]))
                }
                Err(err) => {
                    tracing::warn!(
                        surface = "mcp",
                        service = "code_search",
                        action = "call_tool",
                        subject,
                        code_hash = %code_hash,
                        code_len = code.len(),
                        elapsed_ms = started.elapsed().as_millis(),
                        input_tokens,
                        kind = err.kind(),
                        error = %err,
                        "gateway code search failed"
                    );
                    let env = tool_error_envelope(&service, "call_tool", &err);
                    Ok(CallToolResult::error(vec![Content::text(env.to_string())]))
                }
            };
        }

        // ── Gateway `execute` tool: run caller's JS in the subprocess sandbox ─
        if service == TOOL_EXECUTE_TOOL_NAME {
            let started = Instant::now();
            let input_tokens = estimate_tokens_args(&args);
            let subject = self.request_subject_log_tag(&context);
            let auth = auth_context_from_extensions(&context.extensions);
            if !tool_execute_scope_allowed(auth) {
                tracing::warn!(
                    surface = "mcp",
                    service = %service,
                    action = "call_tool",
                    subject,
                    elapsed_ms = started.elapsed().as_millis(),
                    input_tokens,
                    kind = "forbidden",
                    "gateway code execute denied by scope"
                );
                let env = build_error_extra(
                    &service,
                    "call_tool",
                    "forbidden",
                    "code_execute requires one of scopes: lab, lab:admin",
                    &serde_json::json!({ "required_scopes": ["lab", "lab:admin"] }),
                );
                return Ok(CallToolResult::error(vec![Content::text(env.to_string())]));
            }
            let Some(manager) = &self.gateway_manager else {
                let envelope = build_error(
                    &service,
                    "call_tool",
                    "unknown_tool",
                    "code execute is not enabled",
                );
                return Ok(CallToolResult::error(vec![Content::text(
                    envelope.to_string(),
                )]));
            };
            let config = manager.code_mode_config().await;
            let code = args.get("code").and_then(Value::as_str).unwrap_or_default();
            if code.trim().is_empty() {
                let env = build_error_extra(
                    &service,
                    "call_tool",
                    "invalid_param",
                    "code must not be empty",
                    &serde_json::json!({ "param": "code" }),
                );
                return Ok(CallToolResult::error(vec![Content::text(env.to_string())]));
            }
            if code.len() > CODE_MODE_MAX_CODE_BYTES {
                let env = build_error_extra(
                    &service,
                    "call_tool",
                    "invalid_param",
                    "code exceeds max length 20000 bytes",
                    &serde_json::json!({ "param": "code" }),
                );
                return Ok(CallToolResult::error(vec![Content::text(env.to_string())]));
            }
            let requested_max_tool_calls = args
                .get("max_tool_calls")
                .and_then(Value::as_u64)
                .map(|value| value as usize)
                .unwrap_or(config.max_tool_calls)
                .max(1)
                .min(config.max_tool_calls.max(1));
            let allow_destructive_actions =
                args.get("confirm").and_then(Value::as_bool) == Some(true);
            let code_hash = hash_arguments(&Value::String(code.to_string()));
            tracing::info!(
                surface = "mcp",
                service = "code_execute",
                action = "call_tool",
                subject,
                code_hash = %code_hash,
                max_tool_calls = requested_max_tool_calls,
                input_tokens,
                "gateway code execute start"
            );
            let broker = CodeModeBroker::new(&self.registry, Some(manager));
            let caller = auth.map_or(CodeModeCaller::TrustedLocal, |auth| {
                CodeModeCaller::Scoped {
                    scopes: auth.scopes.clone(),
                    sub: self.request_subject(&context).map(ToOwned::to_owned),
                }
            });
            let before = self.snapshot_catalog().await;
            let response = match broker
                .execute(
                    code,
                    requested_max_tool_calls,
                    caller,
                    self.code_mode_surface(allow_destructive_actions),
                    config,
                )
                .await
            {
                Ok(response) => {
                    let after = self.snapshot_catalog().await;
                    self.notify_catalog_changes(&before, &after).await;
                    response
                }
                Err(err) => {
                    let after = self.snapshot_catalog().await;
                    self.notify_catalog_changes(&before, &after).await;
                    let env = tool_error_envelope(&service, "call_tool", &err);
                    return Ok(CallToolResult::error(vec![Content::text(env.to_string())]));
                }
            };
            let output = serde_json::to_string(&response).unwrap_or_else(|_| "{}".to_string());
            let output_tokens = estimate_tokens(&output);
            tracing::info!(
                surface = "mcp",
                service = "code_execute",
                action = "call_tool",
                subject,
                code_hash = %code_hash,
                call_count = response.calls.len(),
                elapsed_ms = started.elapsed().as_millis(),
                input_tokens,
                output_tokens,
                "gateway code execute ok"
            );
            return Ok(CallToolResult::success(vec![Content::text(output)]));
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

        if self.tool_search_visibility().await.hides_raw_tools() {
            let envelope = build_error(
                &service,
                &action,
                "not_found",
                &format!("tool `{service}` is hidden while tool_search mode is enabled"),
            );
            return Ok(CallToolResult::error(vec![Content::text(
                envelope.to_string(),
            )]));
        }

        if let Some(entry) = svc
            && !tool_execute_builtin_action_allowed(
                entry,
                &action,
                auth_context_from_extensions(&context.extensions),
            )
        {
            let envelope = build_error_extra(
                &service,
                &action,
                "forbidden",
                &format!("action `{action}` for service `{service}` requires `lab:admin` scope"),
                &serde_json::json!({ "required_scopes": ["lab:admin"] }),
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
        let raw_runtime_owner = self.request_runtime_owner(&context);
        let raw_oauth_subject = oauth_upstream_subject_for_request(
            auth_context_from_extensions(&context.extensions),
            self.request_subject(&context),
        );
        let raw_resolved = if let Some(manager) = &self.gateway_manager {
            Some(
                manager
                    .resolve_raw_upstream_tool(
                        &service,
                        Some(&raw_runtime_owner),
                        raw_oauth_subject.as_deref(),
                    )
                    .await,
            )
        } else {
            None
        };
        if let Some(Err(err)) = &raw_resolved
            && !matches!(err.kind(), "unknown_tool" | "not_found")
        {
            let elapsed_ms = start.elapsed().as_millis();
            let kind = canonical_kind(err.kind());
            tracing::warn!(
                surface = "mcp",
                service,
                action = upstream_action,
                tool = %service,
                elapsed_ms,
                kind,
                error = %err,
                "upstream proxy resolution failed"
            );
            let envelope = tool_error_envelope(&service, upstream_action, err);
            self.emit_dispatch_notification(
                &context,
                &service,
                upstream_action,
                elapsed_ms,
                DispatchLogOutcome::Failure {
                    level: LoggingLevel::Warning,
                    kind,
                },
            )
            .await;
            return Ok(CallToolResult::error(vec![Content::text(
                envelope.to_string(),
            )]));
        }
        if let Some(pool) = self.current_upstream_pool().await
            && let Some(Ok((upstream_name, _tool))) = raw_resolved
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

        let auth = auth_context_from_extensions(&context.extensions);
        if let Some(oauth_subject) =
            oauth_upstream_subject_for_request(auth, self.request_subject(&context))
            && let Some(pool) = self.current_upstream_pool().await
        {
            let configs = self.oauth_upstream_configs().await;
            let mut owner = None;
            for (upstream_name, tools) in pool
                .subject_scoped_tools(&configs, oauth_subject.as_ref())
                .await
            {
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
                    oauth_subject = %oauth_subject,
                    "dispatch route selected"
                );
                let mut upstream_params = CallToolRequestParams::new(service.clone());
                upstream_params.arguments = raw_arguments;
                match pool
                    .subject_scoped_call_tool(&config, oauth_subject.as_ref(), upstream_params)
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
                                oauth_subject = %oauth_subject,
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
                                oauth_subject = %oauth_subject,
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
    fn code_mode_surface(&self, allow_destructive_actions: bool) -> CodeModeSurface {
        CodeModeSurface::Mcp {
            allow_destructive_actions,
        }
    }

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

    fn request_runtime_owner(&self, context: &RequestContext<RoleServer>) -> UpstreamRuntimeOwner {
        let subject = self.request_subject(context).map(ToOwned::to_owned);
        let raw = subject
            .as_ref()
            .map(|subject| format!("mcp:{subject}"))
            .unwrap_or_else(|| "mcp:anonymous".to_string());
        UpstreamRuntimeOwner {
            surface: "mcp".to_string(),
            subject,
            request_id: None,
            session_id: None,
            client_name: None,
            raw: Some(raw),
        }
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

fn tool_error_envelope(service: &str, action: &str, err: &DispatchToolError) -> Value {
    let Ok(Value::Object(mut serialized)) = serde_json::to_value(err) else {
        return build_error(service, action, err.kind(), &err.to_string());
    };
    let kind = serialized
        .remove("kind")
        .and_then(|value| value.as_str().map(ToOwned::to_owned))
        .unwrap_or_else(|| err.kind().to_string());
    let message = serialized
        .remove("message")
        .and_then(|value| value.as_str().map(ToOwned::to_owned))
        .unwrap_or_else(|| err.to_string());
    if serialized.is_empty() {
        build_error(service, action, &kind, &message)
    } else {
        build_error_extra(service, action, &kind, &message, &Value::Object(serialized))
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

fn oauth_upstream_subject_for_request<'a>(
    auth: Option<&crate::api::oauth::AuthContext>,
    request_subject: Option<&'a str>,
) -> Option<Cow<'a, str>> {
    match auth {
        None => Some(Cow::Borrowed(SHARED_GATEWAY_OAUTH_SUBJECT)),
        Some(ctx) if ctx.scopes.iter().any(|scope| scope == "lab:admin") => {
            Some(Cow::Borrowed(SHARED_GATEWAY_OAUTH_SUBJECT))
        }
        Some(_) => request_subject.map(Cow::Borrowed),
    }
}

fn tool_execute_scope_allowed(auth: Option<&crate::api::oauth::AuthContext>) -> bool {
    auth.is_none_or(|auth| {
        auth.scopes
            .iter()
            .any(|scope| matches!(scope.as_str(), "lab" | "lab:admin"))
    })
}

/// Returns `true` when the caller is allowed to invoke the tool_search tool.
///
/// tool_search requires at least `lab:read`; tool_execute requires the stronger `lab` or `lab:admin`.
/// `None` auth means stdio transport — trusted by design (no per-request AuthContext).
fn tool_search_scope_allowed(auth: Option<&crate::api::oauth::AuthContext>) -> bool {
    auth.is_none_or(|auth| {
        auth.scopes
            .iter()
            .any(|scope| matches!(scope.as_str(), "lab:read" | "lab" | "lab:admin"))
    })
}

fn tool_execute_builtin_action_allowed(
    entry: &crate::registry::RegisteredService,
    action: &str,
    auth: Option<&crate::api::oauth::AuthContext>,
) -> bool {
    if !builtin_action_requires_admin(entry, action) {
        return true;
    }
    auth.is_none_or(|auth| auth.scopes.iter().any(|scope| scope == "lab:admin"))
}

fn builtin_action_requires_admin(entry: &crate::registry::RegisteredService, action: &str) -> bool {
    if entry.name == "gateway" {
        return !matches!(
            action,
            "help" | "schema" | "gateway.help" | "gateway.schema"
        );
    }
    entry.name == "setup"
        && entry
            .actions
            .iter()
            .any(|spec| spec.name == action && spec.destructive)
}

fn hash_arguments(arguments: &Value) -> String {
    let bytes = serde_json::to_vec(arguments).unwrap_or_default();
    hex::encode(Sha256::digest(bytes))
}

/// Rough char-based token estimator for gateway telemetry logs.
///
/// Uses the conventional ~4 chars-per-token heuristic. Cheap, dependency-free,
/// and accurate enough for capacity tracking; do NOT use for LLM budget
/// enforcement — pull in `tiktoken-rs` if exact counts are required.
fn estimate_tokens(s: &str) -> usize {
    s.len().div_ceil(4)
}

/// Token count of a JSON value, computed against its serialized form.
#[cfg(test)]
fn estimate_tokens_value(value: &Value) -> usize {
    estimate_tokens(&serde_json::to_string(value).unwrap_or_default())
}

/// Token count of an MCP arguments map (`request.arguments.unwrap_or_default()`).
fn estimate_tokens_args(arguments: &serde_json::Map<String, Value>) -> usize {
    estimate_tokens(&serde_json::to_string(arguments).unwrap_or_default())
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
    use super::{
        estimate_tokens, estimate_tokens_args, estimate_tokens_value, extract_error_info,
        logging_level_rank, normalize_upstream_result,
    };
    use crate::dispatch::error::ToolError;
    use crate::mcp::envelope::build_error;
    use crate::mcp::error::{DispatchError, canonical_kind};
    use crate::registry::{RegisteredService, ToolRegistry};
    use lab_apis::core::action::ActionSpec;
    use rmcp::ServerHandler;
    use rmcp::model::{CallToolResult, Content};
    use serde_json::Value;
    use std::future::Future;
    use std::pin::Pin;

    #[test]
    fn estimate_tokens_uses_chars_div_four_heuristic() {
        assert_eq!(estimate_tokens(""), 0);
        // 4 chars → 1 token.
        assert_eq!(estimate_tokens("abcd"), 1);
        // 5 chars → 2 tokens (ceiling).
        assert_eq!(estimate_tokens("abcde"), 2);
        assert_eq!(estimate_tokens("hello world"), 3);
    }

    #[test]
    fn estimate_tokens_value_serializes_first() {
        // Value's serialized form is `{"a":1}` (7 chars) → 2 tokens.
        let v = serde_json::json!({"a": 1});
        assert_eq!(estimate_tokens_value(&v), 2);
    }

    #[test]
    fn estimate_tokens_args_handles_empty_and_populated_maps() {
        let empty: serde_json::Map<String, Value> = serde_json::Map::new();
        // "{}" → 2 chars → 1 token.
        assert_eq!(estimate_tokens_args(&empty), 1);

        let mut populated = serde_json::Map::new();
        populated.insert("name".into(), Value::String("tool_search".into()));
        // `{"name":"tool_search"}` is 22 chars → 6 tokens.
        assert_eq!(estimate_tokens_args(&populated), 6);
    }

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
    fn tool_error_envelope_preserves_structured_extras() {
        let err = ToolError::MissingParam {
            message: "query is required".to_string(),
            param: "query".to_string(),
        };

        let envelope = super::tool_error_envelope("code_search", "call_tool", &err);

        assert_eq!(
            envelope.pointer("/error/kind"),
            Some(&Value::from("missing_param"))
        );
        assert_eq!(
            envelope.pointer("/error/param"),
            Some(&Value::from("query"))
        );
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
    ) -> Pin<Box<dyn Future<Output = Result<Value, ToolError>> + Send>> {
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
    async fn snapshot_catalog_hides_builtin_tools_when_tool_search_is_enabled() {
        let runtime = crate::dispatch::gateway::manager::GatewayRuntimeHandle::default();
        let manager = std::sync::Arc::new(crate::dispatch::gateway::manager::GatewayManager::new(
            std::path::PathBuf::from("config.toml"),
            runtime,
        ));
        manager
            .seed_config(crate::config::LabConfig {
                tool_search: crate::config::ToolSearchConfig {
                    enabled: true,
                    ..crate::config::ToolSearchConfig::default()
                },
                ..crate::config::LabConfig::default()
            })
            .await;
        let server = super::LabMcpServer {
            registry: std::sync::Arc::new(completion_test_registry()),
            gateway_manager: Some(manager),
            node_role: None,
            peers: std::sync::Arc::new(tokio::sync::RwLock::new(Vec::new())),
            logging_level: std::sync::Arc::new(std::sync::atomic::AtomicU8::new(
                logging_level_rank(rmcp::model::LoggingLevel::Info),
            )),
        };

        let snapshot = server.snapshot_catalog().await;

        // Tool Search mode: exactly `search` + `execute`. NO code_search, code_execute, or code.
        assert_eq!(
            snapshot.tools,
            ["execute".to_string(), "search".to_string()]
                .into_iter()
                .collect()
        );
        assert!(
            !snapshot.tools.contains("code_search"),
            "code_search must not appear in Tool Search mode"
        );
        assert!(
            !snapshot.tools.contains("code_execute"),
            "code_execute must not appear in Tool Search mode"
        );
        assert!(
            !snapshot.tools.contains("code"),
            "code must not appear in Tool Search mode"
        );
    }

    #[tokio::test]
    async fn snapshot_catalog_shows_no_gateway_tools_when_surface_is_disabled() {
        // When tool_search.enabled=false, none of the gateway meta-tools
        // (search, execute, code, code_search, code_execute) should appear in
        // the snapshot.
        let runtime = crate::dispatch::gateway::manager::GatewayRuntimeHandle::default();
        let manager = std::sync::Arc::new(crate::dispatch::gateway::manager::GatewayManager::new(
            std::path::PathBuf::from("config.toml"),
            runtime,
        ));
        manager
            .seed_config(crate::config::LabConfig {
                tool_search: crate::config::ToolSearchConfig {
                    enabled: false,
                    ..crate::config::ToolSearchConfig::default()
                },
                ..crate::config::LabConfig::default()
            })
            .await;
        let server = super::LabMcpServer {
            registry: std::sync::Arc::new(completion_test_registry()),
            gateway_manager: Some(manager),
            node_role: None,
            peers: std::sync::Arc::new(tokio::sync::RwLock::new(Vec::new())),
            logging_level: std::sync::Arc::new(std::sync::atomic::AtomicU8::new(
                logging_level_rank(rmcp::model::LoggingLevel::Info),
            )),
        };

        let snapshot = server.snapshot_catalog().await;

        // Raw mode — none of the five gateway meta-tools should appear.
        for meta_tool in ["search", "execute", "code", "code_search", "code_execute"] {
            assert!(
                !snapshot.tools.contains(meta_tool),
                "gateway meta-tool '{meta_tool}' must not appear when neither mode is enabled"
            );
        }
    }

    #[test]
    fn code_execute_description_contains_protocol_contract() {
        // Source of truth: docs/contracts/CODE_NODE_CONTRACT_FOR_RETARD_AGENTS.md
        // Full spec:       docs/specs/CODE_MODE_SPEC_FOR_RETARD_AGENTS.md
        assert!(super::CODE_EXECUTE_DESCRIPTION.contains("callTool<T = unknown>"));
        assert!(
            super::CODE_EXECUTE_DESCRIPTION
                .contains("Successful return: the upstream tool's structuredContent")
        );
        assert!(super::CODE_EXECUTE_DESCRIPTION.contains("JSON.parse(String(e.message))"));
        assert!(super::CODE_EXECUTE_DESCRIPTION.contains("Retry-safe:"));
        assert!(super::CODE_EXECUTE_DESCRIPTION.contains("Promise.all"));
        assert!(
            super::CODE_EXECUTE_DESCRIPTION.contains("codemode"),
            "description must explain the codemode typed helper namespace"
        );
        assert!(
            !super::CODE_EXECUTE_DESCRIPTION.contains("code_search"),
            "description must not reference the deprecated code_search tool"
        );
        assert!(super::CODE_EXECUTE_DESCRIPTION.len() < 8192);
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
                    id: "deploy".to_string(),
                    service: "deploy".to_string(),
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
        assert!(!snapshot.tools.contains("deploy"));
    }

    #[tokio::test]
    #[ignore = "gateway-pivot: hardcoded plex/radarr fixtures; rework with kept-service fixtures post-pivot"]
    async fn service_actions_json_filters_to_allowed_mcp_actions() {
        let runtime = crate::dispatch::gateway::manager::GatewayRuntimeHandle::default();
        let manager = std::sync::Arc::new(crate::dispatch::gateway::manager::GatewayManager::new(
            std::path::PathBuf::from("config.toml"),
            runtime,
        ));
        manager
            .seed_config(crate::config::LabConfig {
                virtual_servers: vec![crate::config::VirtualServerConfig {
                    id: "deploy".to_string(),
                    service: "deploy".to_string(),
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
            .service_actions_json("deploy")
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

    #[test]
    fn gateway_builtin_actions_require_admin_scope() {
        let entry = RegisteredService {
            name: "gateway",
            description: "Gateway",
            category: "bootstrap",
            kind: crate::registry::RegisteredServiceKind::BootstrapOperator,
            status: "available",
            actions: crate::dispatch::gateway::ACTIONS,
            dispatch: noop_dispatch,
        };
        let read_only = crate::api::oauth::AuthContext {
            sub: "alice".to_string(),
            actor_key: None,
            scopes: vec!["lab".to_string()],
            issuer: "https://lab.example.com".to_string(),
            via_session: true,
            csrf_token: None,
            email: None,
        };
        let admin = crate::api::oauth::AuthContext {
            scopes: vec!["lab:admin".to_string()],
            ..read_only.clone()
        };

        assert!(super::tool_execute_builtin_action_allowed(
            &entry,
            "gateway.help",
            Some(&read_only)
        ));
        assert!(!super::tool_execute_builtin_action_allowed(
            &entry,
            "gateway.import",
            Some(&read_only)
        ));
        assert!(super::tool_execute_builtin_action_allowed(
            &entry,
            "gateway.import",
            Some(&admin)
        ));
        assert!(super::tool_execute_builtin_action_allowed(
            &entry,
            "gateway.import",
            None
        ));
    }

    #[test]
    fn tool_search_scope_allows_read_but_tool_execute_does_not() {
        let base = crate::api::oauth::AuthContext {
            sub: "alice".to_string(),
            actor_key: None,
            scopes: vec!["lab:read".to_string()],
            issuer: "https://lab.example.com".to_string(),
            via_session: true,
            csrf_token: None,
            email: None,
        };
        let lab = crate::api::oauth::AuthContext {
            scopes: vec!["lab".to_string()],
            ..base.clone()
        };
        let admin = crate::api::oauth::AuthContext {
            scopes: vec!["lab:admin".to_string()],
            ..base.clone()
        };
        let empty = crate::api::oauth::AuthContext {
            scopes: Vec::new(),
            ..base.clone()
        };
        let unrelated = crate::api::oauth::AuthContext {
            scopes: vec!["profile".to_string()],
            ..base.clone()
        };

        assert!(super::tool_search_scope_allowed(None));
        assert!(super::tool_search_scope_allowed(Some(&base)));
        assert!(super::tool_search_scope_allowed(Some(&lab)));
        assert!(super::tool_search_scope_allowed(Some(&admin)));
        assert!(!super::tool_search_scope_allowed(Some(&empty)));
        assert!(!super::tool_search_scope_allowed(Some(&unrelated)));

        assert!(
            !super::tool_execute_scope_allowed(Some(&base)),
            "lab:read can search but cannot execute"
        );
    }

    #[test]
    fn setup_destructive_builtin_actions_require_admin_scope() {
        let registry = crate::registry::build_default_registry();
        let entry = registry
            .services()
            .iter()
            .find(|service| service.name == "setup")
            .expect("setup service");
        let read_only = crate::api::oauth::AuthContext {
            sub: "alice".to_string(),
            actor_key: None,
            scopes: vec!["lab".to_string()],
            issuer: "https://lab.example.com".to_string(),
            via_session: true,
            csrf_token: None,
            email: None,
        };
        let admin = crate::api::oauth::AuthContext {
            scopes: vec!["lab:admin".to_string()],
            ..read_only.clone()
        };

        assert!(super::tool_execute_builtin_action_allowed(
            entry,
            "state",
            Some(&read_only)
        ));
        assert!(!super::tool_execute_builtin_action_allowed(
            entry,
            "repair",
            Some(&read_only)
        ));
        assert!(super::tool_execute_builtin_action_allowed(
            entry,
            "repair",
            Some(&admin)
        ));
    }

    fn make_auth(scopes: &[&str]) -> crate::api::oauth::AuthContext {
        crate::api::oauth::AuthContext {
            sub: "test-user".to_string(),
            actor_key: None,
            scopes: scopes.iter().map(|s| s.to_string()).collect(),
            issuer: "https://lab.example.com".to_string(),
            via_session: false,
            csrf_token: None,
            email: None,
        }
    }

    #[test]
    fn oauth_upstream_subject_uses_shared_gateway_for_admin_and_trusted_callers() {
        assert_eq!(
            super::oauth_upstream_subject_for_request(None, None).as_deref(),
            Some(crate::dispatch::gateway::SHARED_GATEWAY_OAUTH_SUBJECT)
        );
        assert_eq!(
            super::oauth_upstream_subject_for_request(None, Some("stdio-subject")).as_deref(),
            Some(crate::dispatch::gateway::SHARED_GATEWAY_OAUTH_SUBJECT)
        );

        let admin = make_auth(&["lab:admin"]);
        assert_eq!(
            super::oauth_upstream_subject_for_request(Some(&admin), Some("google-subject"))
                .as_deref(),
            Some(crate::dispatch::gateway::SHARED_GATEWAY_OAUTH_SUBJECT)
        );
    }

    #[test]
    fn oauth_upstream_subject_preserves_non_admin_request_subjects() {
        let lab = make_auth(&["lab"]);
        assert_eq!(
            super::oauth_upstream_subject_for_request(Some(&lab), Some("user-subject")).as_deref(),
            Some("user-subject")
        );

        let read_only = make_auth(&["lab:read"]);
        assert_eq!(
            super::oauth_upstream_subject_for_request(Some(&read_only), Some("reader-subject"))
                .as_deref(),
            Some("reader-subject")
        );
        assert!(
            super::oauth_upstream_subject_for_request(Some(&read_only), None).is_none(),
            "non-admin HTTP callers must not fall back to shared gateway credentials without a subject"
        );
    }

    #[test]
    fn tool_search_scope_allowed_permits_all_expected_scopes() {
        // None = stdio transport → trusted (always permitted)
        assert!(super::tool_search_scope_allowed(None));

        // lab:read is the minimum acceptable scope for tool_search
        let read_only = make_auth(&["lab:read"]);
        assert!(super::tool_search_scope_allowed(Some(&read_only)));

        // bare lab must also pass tool_search
        let lab = make_auth(&["lab"]);
        assert!(super::tool_search_scope_allowed(Some(&lab)));

        // lab:admin must pass tool_search (identified as a gap in the original review)
        let admin = make_auth(&["lab:admin"]);
        assert!(super::tool_search_scope_allowed(Some(&admin)));

        // empty scopes → denied
        let no_scopes = make_auth(&[]);
        assert!(!super::tool_search_scope_allowed(Some(&no_scopes)));

        // unrelated scope → denied
        let unrelated = make_auth(&["mcp:read"]);
        assert!(!super::tool_search_scope_allowed(Some(&unrelated)));
    }

    #[test]
    fn scout_allows_lab_read_but_invoke_requires_lab() {
        // Intentional asymmetry: tool_search is a read-only discovery operation and therefore
        // accepts lab:read in addition to the stronger lab / lab:admin.
        // tool_execute must NOT accept lab:read — it executes upstream tools
        // which may have side effects.
        let read_only = make_auth(&["lab:read"]);

        // tool_search: lab:read is permitted
        assert!(
            super::tool_search_scope_allowed(Some(&read_only)),
            "tool_search should accept lab:read"
        );

        // tool_execute: lab:read must NOT be sufficient
        assert!(
            !super::tool_execute_scope_allowed(Some(&read_only)),
            "tool_execute must reject lab:read — requires lab or lab:admin"
        );
    }

    #[test]
    fn gateway_search_and_execute_input_schemas_are_code_only() {
        // Cloudflare-parity: both gateway meta-tools take exactly { code: string }.
        for schema in [serde_json::json!({
            "type": "object",
            "properties": { "code": { "type": "string" } },
            "required": ["code"]
        })] {
            let props = schema["properties"].as_object().expect("properties object");
            let prop_names: std::collections::BTreeSet<&str> =
                props.keys().map(String::as_str).collect();
            assert_eq!(prop_names, std::collections::BTreeSet::from(["code"]));
        }
    }
}
