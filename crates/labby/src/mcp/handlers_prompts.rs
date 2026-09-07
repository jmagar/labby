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

use std::sync::Arc;
use std::time::Instant;
#[cfg(any(feature = "gateway", test))]
use std::time::SystemTime;

use rmcp::ErrorData;
use rmcp::RoleServer;
use rmcp::model::{
    GetPromptRequestParams, GetPromptResponse, ListPromptsResult, PaginatedRequestParams,
};
use rmcp::service::RequestContext;
use serde_json::Value;

use labby_runtime::agent_error::{AgentErrorContext, AgentErrorOrigin, AgentSideEffectRisk};

use crate::mcp::agent_error::{
    internal as internal_agent_error, invalid_params as invalid_params_agent_error,
};

#[cfg(feature = "gateway")]
use crate::mcp::bound_access::{
    ProjectDiscoveryShadow, ProjectExecutionBinding, project_discovery_shadow,
    project_execution_binding,
};
use crate::mcp::context::auth_context_from_extensions;
#[cfg(feature = "gateway")]
use crate::mcp::context::{forwardable_client_capabilities, oauth_upstream_subject_for_request};
use crate::mcp::logging::{DispatchLogOutcome, LoggingLevel};
use crate::mcp::pagination::{
    CatalogSnapshotCollector, PageCollector, error_kind as pagination_error_kind, invalid_cursor,
    next_catalog_snapshot_revision,
};
#[cfg(any(feature = "gateway", test))]
use crate::mcp::runtime::PromptProvenance;
use crate::mcp::runtime::catalog_snapshot_audience;
use crate::mcp::server::LabMcpServer;

#[cfg(feature = "gateway")]
fn classify_regular_upstream_prompts(
    shadow: &ProjectDiscoveryShadow<'_>,
    provenance: &[PromptProvenance],
    now: SystemTime,
) -> (usize, usize) {
    classify_regular_upstream_prompts_with(provenance, |upstream, native_name| {
        shadow.allows_upstream_prompt(upstream, native_name, now)
    })
}

#[cfg(any(feature = "gateway", test))]
fn classify_regular_upstream_prompts_with(
    provenance: &[PromptProvenance],
    mut allows: impl FnMut(&str, &str) -> Option<bool>,
) -> (usize, usize) {
    provenance.iter().fold((0, 0), |(checked, denied), row| {
        match allows(&row.upstream, &row.native_name) {
            Some(allowed) => (checked + 1, denied + usize::from(!allowed)),
            None => (checked, denied),
        }
    })
}

fn prompt_error_context(
    prompt: &str,
    upstream: Option<&str>,
    cause: Option<&str>,
) -> AgentErrorContext {
    let mut context = AgentErrorContext::for_service_action("labby", "get_prompt");
    context.prompt = Some(prompt.to_string());
    context.upstream = upstream.map(ToOwned::to_owned);
    context.cause = cause.map(|cause| labby_runtime::agent_error::sanitize_error_text(cause, 4096));
    if upstream.is_some() {
        context.origin = Some(AgentErrorOrigin::UpstreamTransport);
        context.side_effects = Some(AgentSideEffectRisk::NoneExpected);
    }
    context
}

impl LabMcpServer {
    pub(crate) async fn list_prompts_impl(
        &self,
        request: Option<PaginatedRequestParams>,
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
        if !self.route_scope.exposes_prompts() {
            let elapsed_ms = start.elapsed().as_millis();
            tracing::info!(
                surface = "mcp",
                service = "labby",
                action = "list_prompts",
                subject,
                route_scope = %self.route_scope.label(),
                elapsed_ms,
                "prompt catalog hidden by loadout"
            );
            self.emit_dispatch_notification(
                &context,
                "lab",
                "list_prompts",
                elapsed_ms,
                DispatchLogOutcome::Success,
            )
            .await;
            return Ok(ListPromptsResult::with_all_items(Vec::new())
                .with_ttl_ms(0)
                .with_cache_scope(rmcp::model::CacheScope::Private));
        }
        let auth = auth_context_from_extensions(&context.extensions);
        let snapshot_audience = catalog_snapshot_audience(auth);
        let mut page_collector = match PageCollector::new(request) {
            Ok(collector) => collector,
            Err(error) => {
                let elapsed_ms = start.elapsed().as_millis();
                let kind = pagination_error_kind(&error);
                tracing::warn!(
                    surface = "mcp",
                    service = "labby",
                    action = "list_prompts",
                    subject,
                    elapsed_ms,
                    kind,
                    "prompt list failed"
                );
                self.emit_dispatch_notification(
                    &context,
                    "lab",
                    "list_prompts",
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

        if let Some(revision) = page_collector.expected_revision().map(str::to_owned) {
            let snapshot = self
                .route_runtime
                .prompt_snapshot(&snapshot_audience, &revision)
                .await;
            let Some((snapshot, provenance, stored_key)) = snapshot else {
                return Err(invalid_cursor(
                    "prompt-list snapshot expired or is unavailable; restart from the first page",
                ));
            };
            page_collector.bind_revision(&revision)?;
            for prompt in snapshot.iter().cloned() {
                page_collector.accept(prompt);
                if page_collector.finished() {
                    break;
                }
            }
            let (prompts, next_cursor) = page_collector.finish()?;
            #[cfg(feature = "gateway")]
            let (project_shadow_state, project_shadow_checked, project_shadow_would_suppress) = {
                let now = SystemTime::now();
                let shadow = project_discovery_shadow(&context.extensions, now);
                let mut state = shadow.state_label_at(now);
                let key_matches = shadow.snapshot_key(now).as_ref() == stored_key.as_ref();
                if state == "bound" && !key_matches {
                    state = "unavailable";
                }
                let (checked, denied) = if state == "bound" {
                    classify_regular_upstream_prompts(&shadow, &provenance, now)
                } else {
                    (0, 0)
                };
                (state, checked, denied)
            };
            #[cfg(not(feature = "gateway"))]
            let _ = (&provenance, &stored_key);
            #[cfg(not(feature = "gateway"))]
            let (project_shadow_state, project_shadow_checked, project_shadow_would_suppress) =
                ("legacy", 0usize, 0usize);
            let elapsed_ms = start.elapsed().as_millis();
            tracing::info!(
                surface = "mcp",
                service = "labby",
                action = "list_prompts",
                subject,
                page_prompt_count = prompts.len(),
                catalog_prompt_count = snapshot.len(),
                catalog_source = "snapshot",
                has_next_cursor = next_cursor.is_some(),
                elapsed_ms,
                project_shadow_state,
                project_shadow_checked_prompt_count = project_shadow_checked,
                project_shadow_would_suppress_prompt_count = project_shadow_would_suppress,
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
            let mut result = ListPromptsResult::with_all_items(prompts)
                .with_ttl_ms(0)
                .with_cache_scope(rmcp::model::CacheScope::Private);
            result.next_cursor = next_cursor;
            return Ok(result);
        }

        if page_collector.start_offset() > 0 {
            return Err(invalid_cursor(
                "prompt-list cursor must include the result-set revision; restart from the first page",
            ));
        }

        let mut prompts = CatalogSnapshotCollector::new(page_collector);
        #[cfg(feature = "gateway")]
        let project_shadow = project_discovery_shadow(&context.extensions, SystemTime::now());
        #[cfg(feature = "gateway")]
        let mut regular_prompt_provenance = Vec::new();
        let builtin_prompts = crate::mcp::prompts::list_all().prompts;
        let builtin_names: Vec<String> = builtin_prompts
            .iter()
            .map(|prompt| prompt.name.to_string())
            .collect();
        for prompt in builtin_prompts {
            prompts.accept(prompt);
        }

        #[cfg(feature = "gateway")]
        if let Some(pool) = self.current_upstream_pool().await {
            let catalog_deadline = tokio::time::Instant::now() + pool.request_timeout();
            let builtin_name_refs: Vec<&str> = builtin_names.iter().map(String::as_str).collect();
            let upstream_prompts = pool
                .list_upstream_prompts_with_provenance_allowed_until(
                    &builtin_name_refs,
                    self.route_scope.allowed_upstreams(),
                    catalog_deadline,
                )
                .await;
            for listed in upstream_prompts {
                regular_prompt_provenance.push(PromptProvenance {
                    upstream: listed.upstream_name,
                    native_name: listed.native_name,
                });
                prompts.accept(listed.prompt);
            }
            if let Some(oauth_subject) =
                oauth_upstream_subject_for_request(auth, self.request_subject(&context))
            {
                let configs = self.route_scoped_oauth_upstream_configs().await;
                let scoped_prompts = pool
                    .subject_scoped_prompts_until(
                        &configs,
                        oauth_subject.as_ref(),
                        &builtin_name_refs,
                        catalog_deadline,
                    )
                    .await;
                for prompt in scoped_prompts.into_iter().filter(|prompt| {
                    prompt
                        .name
                        .split_once('/')
                        .is_none_or(|(upstream, _)| self.route_scope.allows_upstream(upstream))
                }) {
                    prompts.accept(prompt);
                }
            }
        }

        let revision = next_catalog_snapshot_revision();
        prompts.bind_revision(&revision)?;
        let (prompts, next_cursor, complete_catalog) = match prompts.finish() {
            Ok(page) => page,
            Err(error) => {
                let elapsed_ms = start.elapsed().as_millis();
                let kind = pagination_error_kind(&error);
                tracing::warn!(
                    surface = "mcp",
                    service = "labby",
                    action = "list_prompts",
                    subject,
                    elapsed_ms,
                    kind,
                    "prompt list failed"
                );
                self.emit_dispatch_notification(
                    &context,
                    "lab",
                    "list_prompts",
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
        let catalog_prompt_count = complete_catalog.len();
        #[cfg(feature = "gateway")]
        let now = SystemTime::now();
        #[cfg(feature = "gateway")]
        let project_shadow_state = project_shadow.state_label_at(now);
        #[cfg(feature = "gateway")]
        let (project_shadow_checked, project_shadow_would_suppress) =
            if project_shadow_state == "bound" {
                classify_regular_upstream_prompts(&project_shadow, &regular_prompt_provenance, now)
            } else {
                (0, 0)
            };
        #[cfg(feature = "gateway")]
        let project_shadow_snapshot_key = project_shadow.snapshot_key(now);
        #[cfg(not(feature = "gateway"))]
        let (project_shadow_state, project_shadow_checked, project_shadow_would_suppress) =
            ("legacy", 0usize, 0usize);
        if next_cursor.is_some() {
            #[cfg(feature = "gateway")]
            let stored_provenance = Arc::from(regular_prompt_provenance);
            #[cfg(not(feature = "gateway"))]
            let stored_provenance = Arc::from([]);
            #[cfg(feature = "gateway")]
            let stored_key = project_shadow_snapshot_key;
            #[cfg(not(feature = "gateway"))]
            let stored_key = None;
            self.route_runtime
                .store_prompt_snapshot(
                    snapshot_audience,
                    revision,
                    Arc::from(complete_catalog),
                    stored_provenance,
                    stored_key,
                )
                .await;
        }

        let elapsed_ms = start.elapsed().as_millis();
        tracing::info!(
            surface = "mcp",
            service = "labby",
            action = "list_prompts",
            subject,
            page_prompt_count = prompts.len(),
            catalog_prompt_count,
            catalog_source = "live_snapshot",
            has_next_cursor = next_cursor.is_some(),
            elapsed_ms,
            project_shadow_state,
            project_shadow_checked_prompt_count = project_shadow_checked,
            project_shadow_would_suppress_prompt_count = project_shadow_would_suppress,
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

        let mut result = ListPromptsResult::with_all_items(prompts)
            .with_ttl_ms(0)
            .with_cache_scope(rmcp::model::CacheScope::Private);
        result.next_cursor = next_cursor;
        Ok(result)
    }

    pub(crate) async fn get_prompt_impl(
        &self,
        request: GetPromptRequestParams,
        context: RequestContext<RoleServer>,
    ) -> Result<GetPromptResponse, ErrorData> {
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
        #[cfg(feature = "gateway")]
        match project_execution_binding(&context.extensions, SystemTime::now()) {
            ProjectExecutionBinding::Legacy => {}
            ProjectExecutionBinding::Unavailable => {
                let elapsed_ms = start.elapsed().as_millis();
                self.emit_dispatch_notification(
                    &context,
                    "lab",
                    "get_prompt",
                    elapsed_ms,
                    DispatchLogOutcome::Failure {
                        level: LoggingLevel::Warning,
                        kind: "access_context_unavailable",
                    },
                )
                .await;
                let error_context = prompt_error_context(&request.name, None, None);
                return Err(invalid_params_agent_error(
                    "upstream_error",
                    format!("Prompt `{}` is unavailable.", request.name),
                    None,
                    &error_context,
                ));
            }
            ProjectExecutionBinding::Bound {
                transport,
                identity,
            } => {
                let prompt_name = request.name.clone();
                let result = match self.gateway_manager.as_deref() {
                    Some(manager) => {
                        crate::mcp::prompt_execution::execute_transport_bound_project_prompt(
                            self.access_runtime.as_ref(),
                            manager,
                            transport,
                            identity,
                            request,
                        )
                        .await
                    }
                    None => Err(
                        crate::mcp::prompt_execution::PromptExecutionResolutionError::Unavailable,
                    ),
                };
                let elapsed_ms = start.elapsed().as_millis();
                return match result {
                    Ok(response) => {
                        self.emit_dispatch_notification(
                            &context,
                            "lab",
                            "get_prompt",
                            elapsed_ms,
                            DispatchLogOutcome::Success,
                        )
                        .await;
                        Ok(response.into())
                    }
                    Err(error) => {
                        use crate::mcp::prompt_execution::PromptExecutionResolutionError;
                        let unavailable =
                            matches!(&error, PromptExecutionResolutionError::Unavailable);
                        // Map each cause to its stable kind rather than folding
                        // them into `internal_error`. `cancelled` and `timeout`
                        // are documented kinds with different recovery advice
                        // (docs/dev/ERRORS.md) — `cancelled` is explicitly not
                        // automatically retryable, so an agent told only
                        // "could not be fetched" retries work the pool already
                        // classified as withdrawn. Cancellation is also
                        // caller-driven, so it must not log at ERROR and page an
                        // operator for a healthy upstream.
                        let (level, error_kind) = match &error {
                            PromptExecutionResolutionError::Unavailable => {
                                (LoggingLevel::Warning, "upstream_error")
                            }
                            PromptExecutionResolutionError::Cancelled => {
                                (LoggingLevel::Warning, "cancelled")
                            }
                            PromptExecutionResolutionError::Timeout => {
                                (LoggingLevel::Error, "timeout")
                            }
                            PromptExecutionResolutionError::QueueUnavailable
                            | PromptExecutionResolutionError::Upstream => {
                                (LoggingLevel::Error, "upstream_error")
                            }
                        };
                        self.emit_dispatch_notification(
                            &context,
                            "lab",
                            "get_prompt",
                            elapsed_ms,
                            DispatchLogOutcome::Failure {
                                level,
                                kind: error_kind,
                            },
                        )
                        .await;
                        let error_context = prompt_error_context(&prompt_name, None, None);
                        if unavailable {
                            Err(invalid_params_agent_error(
                                "upstream_error",
                                format!("Prompt `{prompt_name}` is unavailable."),
                                None,
                                &error_context,
                            ))
                        } else {
                            let message = match &error {
                                PromptExecutionResolutionError::Cancelled => {
                                    format!("Prompt `{prompt_name}` was cancelled.")
                                }
                                PromptExecutionResolutionError::Timeout => {
                                    format!("Prompt `{prompt_name}` timed out.")
                                }
                                _ => format!("Prompt `{prompt_name}` could not be fetched."),
                            };
                            Err(internal_agent_error(
                                error_kind,
                                message,
                                None,
                                &error_context,
                            ))
                        }
                    }
                };
            }
        }
        if !self.route_scope.exposes_prompts() {
            let elapsed_ms = start.elapsed().as_millis();
            let message = "MCP Prompts are disabled by this loadout; ask the operator to enable Prompts for this loadout";
            self.log_route_scope_denial(&context, "prompts", "get_prompt", message, elapsed_ms);
            return Err(ErrorData::new(
                rmcp::model::ErrorCode::INVALID_REQUEST,
                message.to_string(),
                None,
            ));
        }
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
            let mut error_context = prompt_error_context(&request.name, None, None);
            error_context.origin = Some(AgentErrorOrigin::Policy);
            error_context.side_effects = Some(AgentSideEffectRisk::NoneExpected);
            // `denied_service`, not `service`: the error context's `service`
            // stays "labby"; this key names the service the prompt targeted.
            let extra = serde_json::json!({ "denied_service": service_name });
            return Err(invalid_params_agent_error(
                "route_scope_denied",
                format!("Service `{service_name}` is not exposed on this MCP route."),
                Some(&extra),
                &error_context,
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
            return Ok(prompt.into());
        }

        #[cfg(feature = "gateway")]
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
            let relay_capabilities = forwardable_client_capabilities(request.meta.as_ref());
            let relay_config = if relay_capabilities.is_some() {
                match &self.gateway_manager {
                    Some(manager) => manager
                        .upstream_config(&upstream_name)
                        .await
                        .filter(crate::mcp::context::upstream_uses_capability_relay),
                    None => None,
                }
            } else {
                None
            };
            let upstream_outcome = match (relay_config, relay_capabilities) {
                (Some(config), Some(capabilities)) => {
                    pool.get_prompt_relayed(
                        &config,
                        None,
                        request,
                        context.peer.clone(),
                        context.id.clone(),
                        context.ct.clone(),
                        self.relay_session_id,
                        capabilities,
                    )
                    .await
                }
                _ => pool
                    .get_prompt(&upstream_name, request)
                    .await
                    .map(|outcome| outcome.map(Into::into)),
            };
            let outcome = match upstream_outcome {
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
                    let error_context =
                        prompt_error_context(&prompt_name, Some(&upstream_name), Some(&message));
                    Err(internal_agent_error(
                        "upstream_error",
                        format!(
                            "Upstream `{upstream_name}` failed while fetching prompt `{prompt_name}`."
                        ),
                        None,
                        &error_context,
                    ))
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
                    let error_context = prompt_error_context(
                        &prompt_name,
                        Some(&upstream_name),
                        Some("upstream is not connected"),
                    );
                    Err(invalid_params_agent_error(
                        "upstream_error",
                        format!(
                            "Upstream `{upstream_name}` is not connected, so prompt `{prompt_name}` could not be fetched."
                        ),
                        None,
                        &error_context,
                    ))
                }
            };
            return outcome;
        }

        #[cfg(feature = "gateway")]
        let auth = auth_context_from_extensions(&context.extensions);
        #[cfg(feature = "gateway")]
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
                let relay_capabilities = forwardable_client_capabilities(request.meta.as_ref());
                let upstream_outcome = if let Some(capabilities) = relay_capabilities {
                    pool.get_prompt_relayed(
                        &config,
                        Some(oauth_subject.as_ref()),
                        request,
                        context.peer.clone(),
                        context.id.clone(),
                        context.ct.clone(),
                        self.relay_session_id,
                        capabilities,
                    )
                    .await
                    .unwrap_or_else(|| {
                        Err(format!("relayed upstream `{}` connect failed", config.name))
                    })
                } else {
                    pool.subject_scoped_get_prompt(&config, oauth_subject.as_ref(), request)
                        .await
                        .map(Into::into)
                };
                let outcome = match upstream_outcome {
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
                        let error_context =
                            prompt_error_context(&prompt_name, Some(&config.name), Some(&message));
                        Err(invalid_params_agent_error(
                            "upstream_error",
                            format!(
                                "Upstream `{}` failed while fetching prompt `{prompt_name}`.",
                                config.name
                            ),
                            None,
                            &error_context,
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
        let error_context = prompt_error_context(&request.name, None, None);
        Err(invalid_params_agent_error(
            "not_found",
            format!(
                "Unknown prompt `{}`. Call `prompts/list` and retry with an advertised prompt name.",
                request.name
            ),
            None,
            &error_context,
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

    use rmcp::model::{GetPromptRequestParams, NumberOrString, PaginatedRequestParams};
    use rmcp::service::RequestContext;

    use super::*;
    use crate::mcp::logging::logging_level_rank;
    use crate::mcp::route_scope::McpRouteScope;
    use crate::registry::build_default_registry;

    fn complete_prompt(response: GetPromptResponse) -> rmcp::model::GetPromptResult {
        match response {
            GetPromptResponse::Complete(result) => result,
            GetPromptResponse::InputRequired(_) => {
                panic!("local prompt unexpectedly required input")
            }
            _ => panic!("unexpected prompt response variant"),
        }
    }

    fn prompt_test_server(route_scope: McpRouteScope) -> LabMcpServer {
        LabMcpServer {
            registry: Arc::new(build_default_registry()),
            access_runtime: Arc::new(crate::access::AccessRuntime::blocked_unavailable()),
            file_stash_runtime: Arc::new(crate::file_stash::FileStashRuntime::blocked()),
            #[cfg(feature = "gateway")]
            gateway_manager: None,
            peers: Default::default(),
            code_mode_app_state: Default::default(),
            last_listed_tool_contract: Default::default(),
            route_runtime: Default::default(),
            #[cfg(feature = "gateway")]
            client_registry: Default::default(),
            transport_label: "test",
            logging_level: Arc::new(AtomicU8::new(logging_level_rank(LoggingLevel::Emergency))),
            route_scope,
            relay_session_id: 0,
            code_mode_widget_callbacks_enabled_for_test: false,
        }
    }

    fn request_context(peer: rmcp::service::Peer<RoleServer>) -> RequestContext<RoleServer> {
        RequestContext::new(NumberOrString::Number(1), peer)
    }

    #[test]
    fn regular_prompt_shadow_uses_exact_upstream_and_native_name() {
        let rows = [
            PromptProvenance {
                upstream: "alpha".into(),
                native_name: "same".into(),
            },
            PromptProvenance {
                upstream: "bravo".into(),
                native_name: "same".into(),
            },
            PromptProvenance {
                upstream: "alpha".into(),
                native_name: "other".into(),
            },
        ];
        assert_eq!(
            classify_regular_upstream_prompts_with(&rows, |upstream, name| {
                Some(upstream == "alpha" && name == "same")
            }),
            (3, 2)
        );
    }

    #[tokio::test]
    async fn protected_scope_denies_builtin_prompt_for_disallowed_service() {
        let server = prompt_test_server(McpRouteScope::protected_subset(
            "ops",
            ["gateway-alpha"],
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
    async fn list_prompts_rejects_invalid_cursor() {
        let server = prompt_test_server(McpRouteScope::Root);
        let (transport, _client_transport) = tokio::io::duplex(64);
        let running = rmcp::service::serve_directly::<RoleServer, _, _, std::io::Error, _>(
            server, transport, None,
        );
        let request = PaginatedRequestParams::default().with_cursor(Some("bad".to_string()));

        let err = running
            .service()
            .list_prompts_impl(Some(request), request_context(running.peer().clone()))
            .await
            .expect_err("invalid cursor");

        assert_eq!(
            err.data.as_ref().expect("error data")["kind"],
            serde_json::json!("invalid_cursor")
        );
    }

    #[tokio::test]
    async fn list_prompts_includes_required_private_cache_metadata() {
        let server = prompt_test_server(McpRouteScope::Root);
        let (transport, _client_transport) = tokio::io::duplex(64);
        let running = rmcp::service::serve_directly::<RoleServer, _, _, std::io::Error, _>(
            server, transport, None,
        );

        let result = running
            .service()
            .list_prompts_impl(None, request_context(running.peer().clone()))
            .await
            .expect("prompt list");

        assert_eq!(result.ttl_ms, Some(0));
        assert_eq!(result.cache_scope, Some(rmcp::model::CacheScope::Private));
        let wire = serde_json::to_value(result).expect("serialize prompt list");
        assert_eq!(wire["resultType"], "complete");
        assert_eq!(wire["ttlMs"], 0);
        assert_eq!(wire["cacheScope"], "private");
    }

    #[tokio::test]
    async fn protected_scope_allows_builtin_prompt_for_allowed_service() {
        let server = prompt_test_server(McpRouteScope::protected_subset(
            "ops",
            ["gateway-alpha"],
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

        let prompt = complete_prompt(
            running
                .service()
                .get_prompt_impl(request, request_context(running.peer().clone()))
                .await
                .expect("allowed built-in prompt service"),
        );

        assert!(
            prompt
                .description
                .as_deref()
                .is_some_and(|description| description.contains("gateway"))
        );
    }
}
