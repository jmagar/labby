//! Prompt handler bodies (`list_prompts`, `get_prompt`).
//!
//! Extracted from `server.rs` (bead `lab-kvji.24.1.2`) as inherent
//! `impl LabMcpServer` methods. The `ServerHandler` trait impl in
//! `server.rs` keeps one-line delegators into these `*_impl` methods.
//!
//! Named `handlers_prompts` (not `prompts`) because `crate::mcp::prompts`
//! already owns the builtin prompt definitions this layer calls.
//!
//! No behavior change — relocation only.

use std::time::Instant;

use rmcp::ErrorData;
use rmcp::RoleServer;
use rmcp::model::{
    GetPromptRequestParams, GetPromptResult, ListPromptsResult, LoggingLevel,
    PaginatedRequestParams,
};
use rmcp::service::RequestContext;
use serde_json::Value;

use crate::mcp::context::{auth_context_from_extensions, oauth_upstream_subject_for_request};
use crate::mcp::logging::DispatchLogOutcome;
use crate::mcp::server::LabMcpServer;

impl LabMcpServer {
    pub(crate) async fn list_prompts_impl(
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
            let upstream_prompts = pool
                .list_upstream_prompts_allowed(
                    &builtin_name_refs,
                    self.route_scope.allowed_upstreams(),
                )
                .await;
            prompts.extend(upstream_prompts);
            let auth = auth_context_from_extensions(&context.extensions);
            if let Some(oauth_subject) =
                oauth_upstream_subject_for_request(auth, self.request_subject(&context))
            {
                let configs = self.route_scoped_oauth_upstream_configs().await;
                let scoped_prompts = pool
                    .subject_scoped_prompts(&configs, oauth_subject.as_ref(), &builtin_name_refs)
                    .await;
                prompts.extend(scoped_prompts.into_iter().filter(|prompt| {
                    prompt
                        .name
                        .split_once('/')
                        .is_none_or(|(upstream, _)| self.route_scope.allows_upstream(upstream))
                }));
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

    pub(crate) async fn get_prompt_impl(
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

        if let Some(service_name) = builtin_prompt_service_arg(&request.name, &args)
            && !self.route_scope.allows_service(service_name)
        {
            let elapsed_ms = start.elapsed().as_millis();
            tracing::warn!(
                surface = "mcp",
                service = "labby",
                action = "get_prompt",
                subject,
                prompt = %request.name,
                requested_service = %service_name,
                route_scope = %self.route_scope.label(),
                elapsed_ms,
                kind = "route_scope_denied",
                "built-in prompt denied by protected route scope"
            );
            self.emit_dispatch_notification(
                &context,
                "lab",
                "get_prompt",
                elapsed_ms,
                DispatchLogOutcome::Failure {
                    level: LoggingLevel::Warning,
                    kind: "route_scope_denied",
                },
            )
            .await;
            return Err(ErrorData::invalid_params(
                format!("service `{service_name}` is not exposed on this MCP route"),
                Some(serde_json::json!({
                    "kind": "route_scope_denied",
                    "service": service_name,
                    "prompt": request.name,
                })),
            ));
        }

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
            && let Some(upstream_name) = pool
                .find_prompt_owner_allowed(&request.name, self.route_scope.allowed_upstreams())
                .await
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
            let configs = self.route_scoped_oauth_upstream_configs().await;
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
}

fn builtin_prompt_service_arg<'a>(
    prompt_name: &str,
    args: &'a std::collections::HashMap<String, String>,
) -> Option<&'a str> {
    match prompt_name {
        "run-action" | "service-discover" => args.get("service").map(String::as_str),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::sync::atomic::AtomicU8;

    use rmcp::model::{GetPromptRequestParams, NumberOrString};
    use rmcp::service::RequestContext;

    use super::*;
    use crate::mcp::logging::logging_level_rank;
    use crate::mcp::route_scope::McpRouteScope;
    use crate::registry::build_default_registry;

    fn prompt_test_server(route_scope: McpRouteScope) -> LabMcpServer {
        LabMcpServer {
            registry: Arc::new(build_default_registry()),
            gateway_manager: None,
            node_role: None,
            peers: Arc::new(tokio::sync::RwLock::new(Vec::new())),
            logging_level: Arc::new(AtomicU8::new(logging_level_rank(LoggingLevel::Emergency))),
            route_scope,
        }
    }

    fn request_context(peer: rmcp::service::Peer<RoleServer>) -> RequestContext<RoleServer> {
        RequestContext::new(NumberOrString::Number(1), peer)
    }

    #[tokio::test]
    async fn protected_scope_denies_builtin_prompt_for_disallowed_service() {
        let server = prompt_test_server(McpRouteScope::protected_subset(
            "media",
            ["sonarr"],
            ["gateway"],
            false,
        ));
        let (transport, _client_transport) = tokio::io::duplex(64);
        let running = rmcp::service::serve_directly::<RoleServer, _, _, std::io::Error, _>(
            server, transport, None,
        );
        let mut request = GetPromptRequestParams::new("service-discover");
        request.arguments = Some(
            std::iter::once(("service".to_string(), Value::String("deploy".to_string()))).collect(),
        );

        let err = running
            .service()
            .get_prompt_impl(request, request_context(running.peer().clone()))
            .await
            .expect_err("disallowed built-in prompt service must be denied");

        assert_eq!(
            err.data.as_ref().expect("error data")["kind"],
            serde_json::json!("route_scope_denied")
        );
    }

    #[tokio::test]
    async fn protected_scope_allows_builtin_prompt_for_allowed_service() {
        let server = prompt_test_server(McpRouteScope::protected_subset(
            "media",
            ["sonarr"],
            ["gateway"],
            false,
        ));
        let (transport, _client_transport) = tokio::io::duplex(64);
        let running = rmcp::service::serve_directly::<RoleServer, _, _, std::io::Error, _>(
            server, transport, None,
        );
        let mut request = GetPromptRequestParams::new("service-discover");
        request.arguments = Some(
            std::iter::once(("service".to_string(), Value::String("gateway".to_string())))
                .collect(),
        );

        let prompt = running
            .service()
            .get_prompt_impl(request, request_context(running.peer().clone()))
            .await
            .expect("allowed built-in prompt service");

        assert!(
            prompt
                .description
                .as_deref()
                .is_some_and(|description| description.contains("gateway"))
        );
    }
}
