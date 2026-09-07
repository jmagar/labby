//! Completion + per-service action-schema helpers for the MCP server.
//!
//! Pure free functions extracted from `server.rs` (bead `lab-kvji.24.1.1`).
//! No behavior change — relocation + `pub(crate)` visibility only.

use std::time::Instant;

use rmcp::model::{CompleteRequestParams, CompleteResult, CompletionInfo};
use rmcp::service::RequestContext;
use rmcp::{ErrorData, RoleServer};
use serde_json::Value;

use crate::registry::ToolRegistry;

/// JSON Schema for every service tool's input: `action` (required) + `params` (optional object).
#[allow(clippy::expect_used)]
pub(crate) fn action_schema() -> serde_json::Map<String, Value> {
    serde_json::json!({
        "type": "object",
        "properties": {
            "action": {
                "type": "string",
                "description": "Action to perform (e.g. \"status.get\"). Use \"help\" to list all actions."
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

pub(crate) fn completion_info(values: Vec<String>) -> CompletionInfo {
    let total = values.len() as u32;
    let has_more = values.len() > CompletionInfo::MAX_VALUES;
    let values = values
        .into_iter()
        .take(CompletionInfo::MAX_VALUES)
        .collect();
    CompletionInfo::with_pagination(values, Some(total), has_more)
        .expect("completion values are capped at rmcp's maximum")
}

pub(crate) fn complete_prompt_arg(
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

pub(crate) fn service_name_completions(registry: &ToolRegistry, prefix: &str) -> Vec<String> {
    registry
        .services()
        .iter()
        .map(|service| service.name)
        .filter(|name| name.starts_with(prefix))
        .map(str::to_string)
        .collect()
}

#[cfg(feature = "gateway")]
use crate::mcp::context::{auth_context_from_extensions, oauth_upstream_subject_for_request};
use crate::mcp::logging::{DispatchLogOutcome, LoggingLevel};
use crate::mcp::server::LabMcpServer;

impl LabMcpServer {
    pub(crate) async fn complete_impl(
        &self,
        request: CompleteRequestParams,
        context: RequestContext<RoleServer>,
    ) -> Result<CompleteResult, ErrorData> {
        let start = Instant::now();
        let subject = self.request_subject_log_tag(&context);
        let reference_type = request.r#ref.reference_type();
        let prompt_name = request.r#ref.as_prompt_name().map(str::to_string);

        tracing::info!(
            surface = "mcp",
            service = "labby",
            action = "completion.complete",
            subject,
            reference_type,
            prompt = prompt_name.as_deref().unwrap_or(""),
            argument = %request.argument.name,
            "dispatch start"
        );

        let local_prompt = prompt_name
            .as_deref()
            .is_some_and(|name| matches!(name, "run-action" | "service-discover"));
        if local_prompt {
            let completion = complete_prompt_arg(
                &self.registry,
                prompt_name.as_deref().expect("known local prompt"),
                &request.argument.name,
                &request.argument.value,
            );
            let elapsed_ms = start.elapsed().as_millis();
            self.emit_dispatch_notification(
                &context,
                "lab",
                "completion.complete",
                elapsed_ms,
                DispatchLogOutcome::Success,
            )
            .await;
            return Ok(CompleteResult::new(completion));
        }

        #[cfg(feature = "gateway")]
        if let Some(pool) = self.current_upstream_pool().await {
            let auth = auth_context_from_extensions(&context.extensions);
            let oauth_subject = self.route_oauth_subject(oauth_upstream_subject_for_request(
                auth,
                self.request_subject(&context),
            ));

            if let Some(oauth_subject) = oauth_subject.as_deref() {
                let configs = self.route_scoped_oauth_upstream_configs().await;
                let oauth_owner = if let Some(resource_uri) = request.r#ref.as_resource_uri() {
                    resource_uri
                        .strip_prefix("lab://upstream/")
                        .and_then(|rest| rest.split('/').next())
                        .and_then(|owner| {
                            configs
                                .iter()
                                .find(|config| config.name == owner && config.oauth.is_some())
                        })
                        .cloned()
                } else if let Some(prompt_name) = request.r#ref.as_prompt_name() {
                    let mut owner = None;
                    for config in &configs {
                        if config.oauth.is_some()
                            && pool
                                .subject_scoped_prompt_owner(
                                    std::slice::from_ref(config),
                                    oauth_subject,
                                    prompt_name,
                                )
                                .await
                                .as_deref()
                                == Some(config.name.as_str())
                        {
                            owner = Some(config.clone());
                            break;
                        }
                    }
                    owner
                } else {
                    None
                };

                if let Some(config) = oauth_owner {
                    return pool
                        .subject_scoped_complete_reference(&config, oauth_subject, request)
                        .await
                        .map_err(|message| ErrorData::internal_error(message, None));
                }
            }

            let raw_owner = if let Some(resource_uri) = request.r#ref.as_resource_uri() {
                resource_uri
                    .strip_prefix("lab://upstream/")
                    .and_then(|rest| rest.split('/').next())
                    .filter(|owner| self.route_scope.allows_upstream(owner))
                    .map(str::to_string)
            } else if let Some(prompt_name) = request.r#ref.as_prompt_name() {
                pool.find_prompt_owner_allowed(prompt_name, self.route_scope.allowed_upstreams())
                    .await
            } else {
                None
            };

            if let Some(owner) = raw_owner {
                match pool.complete_reference(&owner, request).await {
                    Some(Ok(result)) => return Ok(result),
                    Some(Err(message)) => {
                        let elapsed_ms = start.elapsed().as_millis();
                        self.emit_dispatch_notification(
                            &context,
                            "lab",
                            "completion.complete",
                            elapsed_ms,
                            DispatchLogOutcome::Failure {
                                level: LoggingLevel::Error,
                                kind: "upstream_error",
                            },
                        )
                        .await;
                        return Err(ErrorData::internal_error(message, None));
                    }
                    None => {}
                }
            }
        }

        let completion = completion_info(Vec::new());
        let elapsed_ms = start.elapsed().as_millis();
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
}

#[cfg(test)]
mod tests;
