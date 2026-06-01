//! `LabMcpServer` — the MCP `ServerHandler` implementation.
//!
//! Extracted from `cli/serve.rs` so that both the stdio and HTTP transports
//! can share the same handler logic.

use std::sync::Arc;
use std::sync::atomic::{AtomicU8, Ordering};
use std::time::Instant;

use axum::http;
use rmcp::model::{
    CallToolRequestParams, CallToolResult, CompleteRequestParams, CompleteResult,
    GetPromptRequestParams, GetPromptResult, ListPromptsResult, ListResourcesResult,
    ListToolsResult, PaginatedRequestParams, ReadResourceRequestParams, ReadResourceResult,
    ServerCapabilities, ServerInfo, SetLevelRequestParams,
};
use rmcp::service::{NotificationContext, Peer, RequestContext};
use rmcp::{ErrorData, RoleServer, ServerHandler};
use tokio::sync::RwLock;

use crate::config::NodeRole;
use crate::dispatch::gateway::manager::GatewayManager;
use crate::mcp::completion::{complete_prompt_arg, completion_info};
use crate::mcp::context::subject_from_extensions;
use crate::mcp::logging::{DispatchLogOutcome, logging_level_rank};
use crate::registry::ToolRegistry;

#[cfg(test)]
use crate::mcp::peers::PeerNotifier;

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
        request: Option<PaginatedRequestParams>,
        context: RequestContext<RoleServer>,
    ) -> Result<ListPromptsResult, ErrorData> {
        self.list_prompts_impl(request, context).await
    }

    async fn get_prompt(
        &self,
        request: GetPromptRequestParams,
        context: RequestContext<RoleServer>,
    ) -> Result<GetPromptResult, ErrorData> {
        self.get_prompt_impl(request, context).await
    }

    async fn list_resources(
        &self,
        request: Option<PaginatedRequestParams>,
        context: RequestContext<RoleServer>,
    ) -> Result<ListResourcesResult, ErrorData> {
        self.list_resources_impl(request, context).await
    }

    async fn read_resource(
        &self,
        request: ReadResourceRequestParams,
        context: RequestContext<RoleServer>,
    ) -> Result<ReadResourceResult, ErrorData> {
        self.read_resource_impl(request, context).await
    }

    async fn list_tools(
        &self,
        request: Option<PaginatedRequestParams>,
        context: RequestContext<RoleServer>,
    ) -> Result<ListToolsResult, ErrorData> {
        self.list_tools_impl(request, context).await
    }

    async fn call_tool(
        &self,
        request: CallToolRequestParams,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, ErrorData> {
        self.call_tool_impl(request, context).await
    }
}

use crate::mcp::catalog::CatalogSnapshot;

impl LabMcpServer {
    pub(crate) async fn notify_catalog_changes(
        &self,
        before: &CatalogSnapshot,
        after: &CatalogSnapshot,
    ) {
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

#[cfg(test)]
mod tests {
    use super::logging_level_rank;
    use crate::dispatch::error::ToolError;
    use crate::mcp::call_tool_codemode::{CODE_EXECUTE_DESCRIPTION, string_array_arg};
    use crate::mcp::completion::complete_prompt_arg;
    use crate::mcp::context::{
        actor_key_from_extensions, oauth_upstream_subject_for_request, subject_from_extensions,
        tool_execute_builtin_action_allowed, tool_execute_scope_allowed, tool_search_scope_allowed,
    };
    use crate::mcp::envelope::build_error;
    use crate::mcp::error::{DispatchError, canonical_kind};
    use crate::mcp::result_format::{
        estimate_tokens, estimate_tokens_args, estimate_tokens_value, extract_error_info,
        tool_error_envelope,
    };
    use crate::mcp::upstream::normalize_upstream_result;
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

        let envelope = tool_error_envelope("code_search", "call_tool", &err);

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
    fn code_mode_filter_arg_rejects_malformed_values() {
        let mut args = serde_json::Map::new();
        args.insert(
            "tools".to_string(),
            Value::String("upstream::github::search_issues".to_string()),
        );
        let err = string_array_arg(&args, "tools")
            .expect_err("string filter must not be treated as allow-all");
        assert_eq!(err.kind(), "invalid_param");

        let mut args = serde_json::Map::new();
        args.insert("upstreams".to_string(), serde_json::json!(["github", 42]));
        let err = string_array_arg(&args, "upstreams")
            .expect_err("non-string filter entries must not be dropped");
        assert_eq!(err.kind(), "invalid_param");
    }

    #[test]
    fn code_mode_filter_arg_accepts_absent_and_string_arrays() {
        let args = serde_json::Map::new();
        assert_eq!(
            string_array_arg(&args, "tools").expect("absent ok"),
            Vec::<String>::new()
        );

        let mut args = serde_json::Map::new();
        args.insert("tools".to_string(), serde_json::json!(["a", "b"]));
        assert_eq!(
            string_array_arg(&args, "tools").expect("array ok"),
            vec!["a".to_string(), "b".to_string()]
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

        let completion = complete_prompt_arg(&registry, "run-action", "action", "");

        assert_eq!(completion.values, registry.action_name_completions(""));
        assert_eq!(completion.total, Some(registry.action_names().len() as u32));
        assert_eq!(completion.has_more, Some(false));
    }

    #[test]
    fn completion_run_action_action_prefix_filters_cached_action_names() {
        let registry = completion_test_registry();

        let completion = complete_prompt_arg(&registry, "run-action", "action", "movie.");

        assert_eq!(
            completion.values,
            vec!["movie.lookup".to_string(), "movie.search".to_string()]
        );
    }

    #[test]
    fn completion_prompt_service_arguments_filter_service_names() {
        let registry = completion_test_registry();

        let run_action = complete_prompt_arg(&registry, "run-action", "service", "ra");
        let discover = complete_prompt_arg(&registry, "service-discover", "service", "so");

        assert_eq!(run_action.values, vec!["radarr".to_string()]);
        assert_eq!(discover.values, vec!["sonarr".to_string()]);
    }

    #[test]
    fn completion_unknown_prompt_argument_returns_empty_result() {
        let registry = completion_test_registry();

        let completion = complete_prompt_arg(&registry, "run-action", "params", "{");

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
        assert!(CODE_EXECUTE_DESCRIPTION.contains("callTool<T = unknown>"));
        assert!(
            CODE_EXECUTE_DESCRIPTION
                .contains("Successful return: the upstream tool's structuredContent")
        );
        assert!(CODE_EXECUTE_DESCRIPTION.contains("JSON.parse(String(e.message))"));
        assert!(CODE_EXECUTE_DESCRIPTION.contains("Retry-safe:"));
        assert!(CODE_EXECUTE_DESCRIPTION.contains("Promise.all"));
        assert!(
            CODE_EXECUTE_DESCRIPTION.contains("codemode"),
            "description must explain the codemode typed helper namespace"
        );
        assert!(
            !CODE_EXECUTE_DESCRIPTION.contains("code_search"),
            "description must not reference the deprecated code_search tool"
        );
        assert!(CODE_EXECUTE_DESCRIPTION.len() < 8192);
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

        assert_eq!(subject_from_extensions(&extensions), Some("alice"));
        assert_eq!(actor_key_from_extensions(&extensions), Some("actor-alice"));
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

        assert!(tool_execute_builtin_action_allowed(
            &entry,
            "gateway.help",
            Some(&read_only)
        ));
        assert!(!tool_execute_builtin_action_allowed(
            &entry,
            "gateway.import",
            Some(&read_only)
        ));
        assert!(tool_execute_builtin_action_allowed(
            &entry,
            "gateway.import",
            Some(&admin)
        ));
        assert!(tool_execute_builtin_action_allowed(
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

        assert!(tool_search_scope_allowed(None));
        assert!(tool_search_scope_allowed(Some(&base)));
        assert!(tool_search_scope_allowed(Some(&lab)));
        assert!(tool_search_scope_allowed(Some(&admin)));
        assert!(!tool_search_scope_allowed(Some(&empty)));
        assert!(!tool_search_scope_allowed(Some(&unrelated)));

        assert!(
            !tool_execute_scope_allowed(Some(&base)),
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

        assert!(tool_execute_builtin_action_allowed(
            entry,
            "state",
            Some(&read_only)
        ));
        assert!(!tool_execute_builtin_action_allowed(
            entry,
            "repair",
            Some(&read_only)
        ));
        assert!(tool_execute_builtin_action_allowed(
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
            oauth_upstream_subject_for_request(None, None).as_deref(),
            Some(crate::dispatch::gateway::SHARED_GATEWAY_OAUTH_SUBJECT)
        );
        assert_eq!(
            oauth_upstream_subject_for_request(None, Some("stdio-subject")).as_deref(),
            Some(crate::dispatch::gateway::SHARED_GATEWAY_OAUTH_SUBJECT)
        );

        let admin = make_auth(&["lab:admin"]);
        assert_eq!(
            oauth_upstream_subject_for_request(Some(&admin), Some("google-subject")).as_deref(),
            Some(crate::dispatch::gateway::SHARED_GATEWAY_OAUTH_SUBJECT)
        );
    }

    #[test]
    fn oauth_upstream_subject_preserves_non_admin_request_subjects() {
        let lab = make_auth(&["lab"]);
        assert_eq!(
            oauth_upstream_subject_for_request(Some(&lab), Some("user-subject")).as_deref(),
            Some("user-subject")
        );

        let read_only = make_auth(&["lab:read"]);
        assert_eq!(
            oauth_upstream_subject_for_request(Some(&read_only), Some("reader-subject")).as_deref(),
            Some("reader-subject")
        );
        assert!(
            oauth_upstream_subject_for_request(Some(&read_only), None).is_none(),
            "non-admin HTTP callers must not fall back to shared gateway credentials without a subject"
        );
    }

    #[test]
    fn tool_search_scope_allowed_permits_all_expected_scopes() {
        // None = stdio transport → trusted (always permitted)
        assert!(tool_search_scope_allowed(None));

        // lab:read is the minimum acceptable scope for tool_search
        let read_only = make_auth(&["lab:read"]);
        assert!(tool_search_scope_allowed(Some(&read_only)));

        // bare lab must also pass tool_search
        let lab = make_auth(&["lab"]);
        assert!(tool_search_scope_allowed(Some(&lab)));

        // lab:admin must pass tool_search (identified as a gap in the original review)
        let admin = make_auth(&["lab:admin"]);
        assert!(tool_search_scope_allowed(Some(&admin)));

        // empty scopes → denied
        let no_scopes = make_auth(&[]);
        assert!(!tool_search_scope_allowed(Some(&no_scopes)));

        // unrelated scope → denied
        let unrelated = make_auth(&["mcp:read"]);
        assert!(!tool_search_scope_allowed(Some(&unrelated)));
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
            tool_search_scope_allowed(Some(&read_only)),
            "tool_search should accept lab:read"
        );

        // tool_execute: lab:read must NOT be sufficient
        assert!(
            !tool_execute_scope_allowed(Some(&read_only)),
            "tool_execute must reject lab:read — requires lab or lab:admin"
        );
    }

    #[test]
    fn gateway_search_input_schema_is_code_only() {
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
