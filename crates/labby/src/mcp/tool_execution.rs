//! Project-bound nondestructive regular Tool execution authorization seam.

use std::time::SystemTime;

use labby_auth::VerifiedIdentity;
use labby_gateway::gateway::manager::{GatewayManager, PublishedToolCallError};
use labby_gateway::upstream::tool_error::mcp_error_data_kind;
use rmcp::model::{CallToolRequestParams, CallToolResponse, CallToolResult};
use thiserror::Error;

use crate::access::{AccessRuntime, Permission};
use crate::mcp::bound_access::{
    BoundAccessContext, TransportBoundAccessContext, bind_asset_use_access_context,
};

/// Server-owned inputs for one exact regular non-OAuth Tool execution.
///
/// Deliberately non-`Clone`, non-`Debug`, and non-serializable. The identity
/// and protected-route facts must be trusted server inputs. This inner seam
/// does not prove a transport token instance or expiry; the mounted handler
/// reaches it only through the transport-bound Complete-only adapter.
pub(crate) struct ToolExecutionResolutionInput {
    identity: VerifiedIdentity,
    route_name: String,
    resource: String,
    project_id: String,
    request: CallToolRequestParams,
}

impl ToolExecutionResolutionInput {
    pub(crate) fn new(
        identity: VerifiedIdentity,
        route_name: impl Into<String>,
        resource: impl Into<String>,
        project_id: impl Into<String>,
        request: CallToolRequestParams,
    ) -> Self {
        Self {
            identity,
            route_name: route_name.into(),
            resource: resource.into(),
            project_id: project_id.into(),
            request,
        }
    }
}

/// Redacted result classes exposed by the server-owned freshness seam.
#[derive(Debug, Error, PartialEq, Eq)]
pub(crate) enum ToolExecutionResolutionError {
    #[error("tool execution target is unavailable")]
    Unavailable,
    #[error("tool execution queue is unavailable")]
    QueueUnavailable,
    #[error("upstream tool returned an MCP {kind} error (code {code})")]
    Mcp { kind: &'static str, code: i32 },
    #[error("upstream tool transport failed")]
    Transport,
    #[error("upstream tool protocol failed")]
    Protocol,
    #[error("tool execution timed out")]
    Timeout,
    #[error("tool execution was cancelled")]
    Cancelled,
    #[error("upstream tool input-required rounds were exceeded")]
    InputRequiredRoundsExceeded,
    #[error("tool execution failed")]
    Other,
    #[error("tool response is too large")]
    TooLarge,
    #[error("upstream returned a response unsupported by this execution path")]
    UnsupportedTerminalResponse,
}

#[derive(Clone, PartialEq, Eq)]
struct ExactToolTarget {
    upstream: String,
    native_name: String,
    pool_generation: labby_gateway::gateway::manager::PoolPublicationGeneration,
    tool_generation: labby_gateway::upstream::pool::ToolCatalogGeneration,
    destructive: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg(all(test, feature = "proxy-testkit"))]
pub(crate) enum ProjectToolOwnership {
    OwnedLabby,
    Regular,
}

#[cfg(all(test, feature = "proxy-testkit"))]
pub(crate) fn transport_bound_tool_ownership(
    transport: &TransportBoundAccessContext,
    wire_name: &str,
) -> ProjectToolOwnership {
    if crate::mcp::permanent_tools::is_reserved_non_upstream_tool_name(wire_name)
        || transport
            .core()
            .catalog()
            .catalog()
            .services()
            .services()
            .iter()
            .any(|service| service.name() == wire_name)
    {
        ProjectToolOwnership::OwnedLabby
    } else {
        ProjectToolOwnership::Regular
    }
}

fn resolve_exact_target(context: &BoundAccessContext, wire_name: &str) -> Option<ExactToolTarget> {
    if context.catalog().access().permission != Permission::AssetUse {
        return None;
    }
    if crate::mcp::permanent_tools::is_reserved_non_upstream_tool_name(wire_name)
        || context
            .catalog()
            .catalog()
            .services()
            .services()
            .iter()
            .any(|service| service.name() == wire_name)
    {
        return None;
    }
    let tools = context.catalog().catalog().tools();
    let published = tools.unique_route_for_wire_name(wire_name)?;
    if published.tool.destructive
        || !context.allows_upstream_tool_pair(
            published.upstream_name.as_ref(),
            published.tool_name.as_ref(),
        )
    {
        return None;
    }
    Some(ExactToolTarget {
        upstream: published.upstream_name.to_string(),
        native_name: published.tool_name.to_string(),
        pool_generation: tools.pool_publication_generation(),
        tool_generation: tools.tool_catalog_generation(),
        destructive: published.tool.destructive,
    })
}

/// Authorize and execute one exact nondestructive regular non-OAuth Tool over
/// a bounded Access/manager common interval. MCP handlers reach this primitive
/// only through the transport-bound Complete-only wrapper below.
pub(crate) async fn execute_exact_project_tool(
    runtime: &AccessRuntime,
    manager: &GatewayManager,
    input: ToolExecutionResolutionInput,
) -> Result<CallToolResponse, ToolExecutionResolutionError> {
    let wire_name = input.request.name.to_string();
    let first = bind_asset_use_access_context(
        runtime,
        manager,
        input.identity.clone(),
        &input.route_name,
        &input.resource,
        &input.project_id,
    )
    .await
    .map_err(|_| ToolExecutionResolutionError::Unavailable)?;
    let target = resolve_exact_target(&first, &wire_name)
        .ok_or(ToolExecutionResolutionError::Unavailable)?;
    let mut outbound = input.request;
    outbound.name = target.native_name.clone().into();
    let result = manager
        .execute_published_tool_exact(
            target.pool_generation,
            target.tool_generation,
            &target.upstream,
            &target.native_name,
            outbound,
        )
        .await;
    let second = bind_asset_use_access_context(
        runtime,
        manager,
        input.identity,
        &input.route_name,
        &input.resource,
        &input.project_id,
    )
    .await
    .map_err(|_| ToolExecutionResolutionError::Unavailable)?;
    let second_target = resolve_exact_target(&second, &wire_name)
        .ok_or(ToolExecutionResolutionError::Unavailable)?;
    if !first.same_publication_as(&second) || target != second_target {
        return Err(ToolExecutionResolutionError::Unavailable);
    }
    map_manager_result(result)
}

async fn execute_transport_bound_project_tool_with_clock(
    runtime: &AccessRuntime,
    manager: &GatewayManager,
    transport: &TransportBoundAccessContext,
    identity: &VerifiedIdentity,
    request: CallToolRequestParams,
    mut now: impl FnMut() -> SystemTime,
) -> Result<CallToolResponse, ToolExecutionResolutionError> {
    transport
        .validate_not_expired(now())
        .map_err(|_| ToolExecutionResolutionError::Unavailable)?;
    if !transport.matches_identity(identity) {
        return Err(ToolExecutionResolutionError::Unavailable);
    }
    let route = transport.core().route();
    let result = execute_exact_project_tool(
        runtime,
        manager,
        ToolExecutionResolutionInput::new(
            identity.clone(),
            route.route_name(),
            route.resource(),
            route.project_id(),
            request,
        ),
    )
    .await;
    finish_transport_bound_tool_result(transport, identity, now(), result)
}

fn finish_transport_bound_tool_result(
    transport: &TransportBoundAccessContext,
    identity: &VerifiedIdentity,
    now: SystemTime,
    result: Result<CallToolResponse, ToolExecutionResolutionError>,
) -> Result<CallToolResponse, ToolExecutionResolutionError> {
    transport
        .validate_not_expired(now)
        .map_err(|_| ToolExecutionResolutionError::Unavailable)?;
    if !transport.matches_identity(identity) {
        return Err(ToolExecutionResolutionError::Unavailable);
    }
    result
}

/// Execute the protected Tool path under the handler's explicit Complete-only
/// terminal contract.
///
/// The regular exact pool truthfully negotiates no Task or InputRequired
/// capabilities. A nonconforming upstream response is dropped immediately and
/// mapped to one static class; its payload is never retained in the error,
/// retried, relayed, or registered as a task.
pub(crate) async fn execute_transport_bound_project_complete_tool(
    runtime: &AccessRuntime,
    manager: &GatewayManager,
    transport: &TransportBoundAccessContext,
    identity: &VerifiedIdentity,
    request: CallToolRequestParams,
) -> Result<CallToolResult, ToolExecutionResolutionError> {
    execute_transport_bound_project_complete_tool_with_clock(
        runtime,
        manager,
        transport,
        identity,
        request,
        SystemTime::now,
    )
    .await
}

async fn execute_transport_bound_project_complete_tool_with_clock(
    runtime: &AccessRuntime,
    manager: &GatewayManager,
    transport: &TransportBoundAccessContext,
    identity: &VerifiedIdentity,
    request: CallToolRequestParams,
    now: impl FnMut() -> SystemTime,
) -> Result<CallToolResult, ToolExecutionResolutionError> {
    let response = execute_transport_bound_project_tool_with_clock(
        runtime, manager, transport, identity, request, now,
    )
    .await;
    match response {
        Err(error) => Err(error),
        Ok(response) => finish_complete_tool_response(response),
    }
}

fn finish_complete_tool_response(
    response: CallToolResponse,
) -> Result<CallToolResult, ToolExecutionResolutionError> {
    match response {
        CallToolResponse::Complete(result) => Ok(result),
        _ => Err(ToolExecutionResolutionError::UnsupportedTerminalResponse),
    }
}

fn map_manager_result(
    result: Result<CallToolResponse, PublishedToolCallError>,
) -> Result<CallToolResponse, ToolExecutionResolutionError> {
    result.map_err(map_manager_error)
}

fn map_manager_error(error: PublishedToolCallError) -> ToolExecutionResolutionError {
    match error {
        PublishedToolCallError::Unavailable => ToolExecutionResolutionError::Unavailable,
        PublishedToolCallError::QueueUnavailable => ToolExecutionResolutionError::QueueUnavailable,
        PublishedToolCallError::Mcp(data) => ToolExecutionResolutionError::Mcp {
            kind: mcp_error_data_kind(&data),
            code: data.code.0,
        },
        PublishedToolCallError::Transport => ToolExecutionResolutionError::Transport,
        PublishedToolCallError::Protocol => ToolExecutionResolutionError::Protocol,
        PublishedToolCallError::Timeout => ToolExecutionResolutionError::Timeout,
        PublishedToolCallError::Cancelled => ToolExecutionResolutionError::Cancelled,
        PublishedToolCallError::InputRequiredRoundsExceeded => {
            ToolExecutionResolutionError::InputRequiredRoundsExceeded
        }
        PublishedToolCallError::Other => ToolExecutionResolutionError::Other,
        PublishedToolCallError::TooLarge => ToolExecutionResolutionError::TooLarge,
    }
}

#[cfg(all(test, feature = "proxy-testkit"))]
#[allow(clippy::disallowed_methods)] // Test fixture constructs upstream-owned descriptors directly.
mod tests {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::{Duration, SystemTime, UNIX_EPOCH};

    use labby_auth::{Authenticator, VerifiedIdentity};
    use labby_gateway::gateway::config_store::FsGatewayConfigStore;
    use labby_gateway::gateway::manager::{GatewayManager, GatewayRuntimeHandle};
    use labby_gateway::upstream::pool::UpstreamPool;
    use labby_gateway::upstream::types::UpstreamTool;
    use labby_runtime::gateway_config::{
        GatewayConfig, GatewayLoadoutConfig, ProtectedGatewaySubsetTarget, ProtectedMcpRouteConfig,
        ProtectedMcpRouteTarget, UpstreamConfig, UpstreamOauthConfig, UpstreamOauthMode,
        UpstreamOauthRegistration, VirtualServerConfig, VirtualServerSurfacesConfig,
    };
    use rmcp::model::{
        BooleanSchema, CallToolRequestParams, CallToolResponse, CallToolResult, ContentBlock,
        CreateTaskResult, ElicitRequest, ElicitRequestParams, ElicitationSchema, InputRequest,
        InputRequests, InputRequiredResult, PrimitiveSchemaDefinition, ServerCapabilities,
        ServerInfo, Task, TaskStatus, Tool,
    };
    use rmcp::model::{ErrorCode, ErrorData};
    use rmcp::service::RequestContext;
    use rmcp::{RoleServer, ServerHandler};
    use tokio::sync::{Mutex, Notify};

    use super::{
        ProjectToolOwnership, ToolExecutionResolutionError, ToolExecutionResolutionInput,
        execute_exact_project_tool, execute_transport_bound_project_complete_tool_with_clock,
        execute_transport_bound_project_tool_with_clock, finish_complete_tool_response,
        finish_transport_bound_tool_result, map_manager_error, map_manager_result,
        transport_bound_tool_ownership,
    };
    use crate::access::{AccessRuntime, AssignProjectLoadoutInput, BootstrapOwnerInput};
    use crate::mcp::bound_access::{
        TransportBoundAccessContext, attach_project_access_observation, bind_access_context,
        validate_transport_credential_binding,
    };
    use crate::mcp::catalog::{
        ADD_SERVER_TOOL_NAME, CODE_MODE_READ_TOOL_NAME, CODE_MODE_TOOL_NAME,
        CODE_MODE_UI_TOOL_NAME, GATEWAY_STATUS_TOOL_NAME, MCP_APP_TOOL_NAME, SETTINGS_TOOL_NAME,
    };
    use crate::mcp::logging::{LoggingLevel, logging_level_rank};
    use crate::mcp::route_scope::McpRouteScope;
    use crate::mcp::server::LabMcpServer;
    use labby_gateway::gateway::manager::PublishedToolCallError;

    #[test]
    fn mcp_error_mapping_keeps_only_stable_kind_and_code() {
        let mapped = map_manager_error(PublishedToolCallError::Mcp(ErrorData::new(
            ErrorCode(-32_602),
            "private tenant secret",
            Some(serde_json::json!({"kind": "invalid_params", "secret": "hidden"})),
        )));

        assert_eq!(
            mapped,
            ToolExecutionResolutionError::Mcp {
                kind: "invalid_param",
                code: -32_602,
            }
        );
        let rendered = mapped.to_string();
        assert!(!rendered.contains("private tenant secret"));
        assert!(!rendered.contains("hidden"));

        for (published, expected) in [
            (
                PublishedToolCallError::Unavailable,
                ToolExecutionResolutionError::Unavailable,
            ),
            (
                PublishedToolCallError::QueueUnavailable,
                ToolExecutionResolutionError::QueueUnavailable,
            ),
            (
                PublishedToolCallError::Transport,
                ToolExecutionResolutionError::Transport,
            ),
            (
                PublishedToolCallError::Protocol,
                ToolExecutionResolutionError::Protocol,
            ),
            (
                PublishedToolCallError::Timeout,
                ToolExecutionResolutionError::Timeout,
            ),
            (
                PublishedToolCallError::Cancelled,
                ToolExecutionResolutionError::Cancelled,
            ),
            (
                PublishedToolCallError::InputRequiredRoundsExceeded,
                ToolExecutionResolutionError::InputRequiredRoundsExceeded,
            ),
            (
                PublishedToolCallError::Other,
                ToolExecutionResolutionError::Other,
            ),
            (
                PublishedToolCallError::TooLarge,
                ToolExecutionResolutionError::TooLarge,
            ),
        ] {
            assert_eq!(map_manager_error(published), expected);
        }
    }

    #[test]
    fn wrapper_boundary_preserves_complete_task_and_input_required_responses() {
        let schema = ElicitationSchema::builder()
            .required_property(
                "confirm",
                PrimitiveSchemaDefinition::Boolean(BooleanSchema::default()),
            )
            .build()
            .unwrap();
        let expected = [
            CallToolResponse::Complete(CallToolResult::success(vec![ContentBlock::text(
                "complete",
            )])),
            CallToolResponse::Task(CreateTaskResult::new(Task::new(
                "task-7",
                TaskStatus::Working,
                "2026-08-24T00:00:00Z",
                "2026-08-24T00:00:00Z",
            ))),
            CallToolResponse::InputRequired(InputRequiredResult::from_input_requests(
                InputRequests::from([(
                    "confirmation".into(),
                    InputRequest::Elicitation(ElicitRequest::new(
                        ElicitRequestParams::FormElicitationParams {
                            meta: None,
                            message: "confirm?".into(),
                            requested_schema: schema,
                        },
                    )),
                )]),
            )),
        ];
        for expected in expected {
            let actual = map_manager_result(Ok(expected.clone())).unwrap();
            match (actual, expected) {
                (CallToolResponse::Complete(actual), CallToolResponse::Complete(expected)) => {
                    assert_eq!(
                        serde_json::to_value(actual).unwrap(),
                        serde_json::to_value(expected).unwrap()
                    );
                }
                (CallToolResponse::Task(actual), CallToolResponse::Task(expected)) => {
                    assert_eq!(
                        serde_json::to_value(actual).unwrap(),
                        serde_json::to_value(expected).unwrap()
                    );
                }
                (
                    CallToolResponse::InputRequired(actual),
                    CallToolResponse::InputRequired(expected),
                ) => {
                    assert_eq!(
                        serde_json::to_value(actual).unwrap(),
                        serde_json::to_value(expected).unwrap()
                    );
                }
                _ => panic!("wrapper boundary changed response variant"),
            }
        }
    }

    #[derive(Clone)]
    struct EchoToolServer {
        calls: Arc<AtomicUsize>,
        received_meta: Arc<Mutex<Vec<rmcp::model::RequestMetaObject>>>,
    }

    impl ServerHandler for EchoToolServer {
        fn get_info(&self) -> ServerInfo {
            ServerInfo::new(ServerCapabilities::builder().enable_tools().build())
        }

        async fn call_tool(
            &self,
            request: CallToolRequestParams,
            context: RequestContext<RoleServer>,
        ) -> Result<CallToolResponse, ErrorData> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            self.received_meta.lock().await.push(context.meta);
            let value = request
                .arguments
                .as_ref()
                .and_then(|args| args.get("value"))
                .cloned();
            Ok(CallToolResult::success(vec![ContentBlock::text(format!(
                "{}:{}",
                request.name,
                value.unwrap_or_default()
            ))])
            .into())
        }
    }

    #[derive(Clone)]
    struct DelayedToolServer {
        calls: Arc<AtomicUsize>,
        started: Arc<Notify>,
        release: Arc<Notify>,
        fail: bool,
    }

    impl ServerHandler for DelayedToolServer {
        fn get_info(&self) -> ServerInfo {
            ServerInfo::new(ServerCapabilities::builder().enable_tools().build())
        }

        async fn call_tool(
            &self,
            _request: CallToolRequestParams,
            _context: RequestContext<RoleServer>,
        ) -> Result<CallToolResponse, ErrorData> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            self.started.notify_one();
            self.release.notified().await;
            if self.fail {
                return Err(ErrorData::internal_error("private delayed failure", None));
            }
            Ok(CallToolResult::success(vec![ContentBlock::text("delayed")]).into())
        }
    }

    #[derive(Clone)]
    struct TerminalVariantToolServer {
        calls: Arc<AtomicUsize>,
        response: CallToolResponse,
    }

    impl ServerHandler for TerminalVariantToolServer {
        fn get_info(&self) -> ServerInfo {
            ServerInfo::new(ServerCapabilities::builder().enable_tools().build())
        }

        async fn call_tool(
            &self,
            _request: CallToolRequestParams,
            _context: RequestContext<RoleServer>,
        ) -> Result<CallToolResponse, ErrorData> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            Ok(self.response.clone())
        }
    }

    #[derive(Clone)]
    struct McpErrorToolServer {
        calls: Arc<AtomicUsize>,
        code: i32,
        message: &'static str,
    }

    impl ServerHandler for McpErrorToolServer {
        fn get_info(&self) -> ServerInfo {
            ServerInfo::new(ServerCapabilities::builder().enable_tools().build())
        }

        async fn call_tool(
            &self,
            _request: CallToolRequestParams,
            _context: RequestContext<RoleServer>,
        ) -> Result<CallToolResponse, ErrorData> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            Err(ErrorData::new(
                ErrorCode(self.code),
                self.message,
                Some(serde_json::json!({"secret": "spoof-secret"})),
            ))
        }
    }

    fn config(expose_tools: bool) -> GatewayConfig {
        GatewayConfig {
            upstream: ["alpha", "bravo"]
                .into_iter()
                .map(|name| UpstreamConfig {
                    enabled: true,
                    name: name.into(),
                    url: None,
                    transport: None,
                    socket_path: None,
                    headers: Default::default(),
                    bearer_token_env: None,
                    command: Some("node".into()),
                    args: Vec::new(),
                    env: [("MCP_UPSTREAM_RELAY_MODE".into(), "pooled".into())]
                        .into_iter()
                        .collect(),
                    proxy_resources: false,
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
                })
                .collect(),
            loadouts: vec![GatewayLoadoutConfig {
                name: "production".into(),
                upstreams: vec!["alpha".into(), "bravo".into()],
                services: vec!["gateway".into()],
                expose_tools,
                ..GatewayLoadoutConfig::default()
            }],
            protected_mcp_routes: vec![ProtectedMcpRouteConfig {
                name: "project-route".into(),
                enabled: true,
                public_host: "mcp.example.com".into(),
                public_path: "/project".into(),
                upstream: None,
                backend_url: String::new(),
                backend_mcp_path: "/mcp".into(),
                scopes: Vec::new(),
                health_path: None,
                target: Some(ProtectedMcpRouteTarget::GatewaySubset(
                    ProtectedGatewaySubsetTarget {
                        project_id: Some("bootstrap-default".into()),
                        loadout: Some("production".into()),
                        ..ProtectedGatewaySubsetTarget::default()
                    },
                )),
            }],
            virtual_servers: vec![VirtualServerConfig {
                id: "gateway".into(),
                service: "gateway".into(),
                enabled: true,
                surfaces: VirtualServerSurfacesConfig {
                    mcp: true,
                    ..VirtualServerSurfacesConfig::default()
                },
                mcp_policy: None,
            }],
            ..GatewayConfig::default()
        }
    }

    fn upstream_tool(name: &str, destructive: bool) -> UpstreamTool {
        let tool = Tool::new(name.to_string(), "exact", Arc::new(serde_json::Map::new()))
            .with_annotations(
                rmcp::model::ToolAnnotations::new()
                    .read_only(!destructive)
                    .destructive(destructive),
            );
        UpstreamTool {
            input_schema: Some(serde_json::Value::Object((*tool.input_schema).clone())),
            output_schema: None,
            destructive,
            upstream_name: Arc::from("alpha"),
            tool,
        }
    }

    async fn transport_binding(
        runtime: &AccessRuntime,
        manager: &GatewayManager,
        identity: VerifiedIdentity,
        expires_at: usize,
        now: SystemTime,
    ) -> TransportBoundAccessContext {
        let core = bind_access_context(
            runtime,
            manager,
            identity,
            "project-route",
            "https://mcp.example.com/project",
            "bootstrap-default",
        )
        .await
        .unwrap();
        let credential =
            validate_transport_credential_binding("issuer", "request-jti", expires_at, now)
                .unwrap();
        TransportBoundAccessContext::new(core, credential, now).unwrap()
    }

    fn handler_server(runtime: Arc<AccessRuntime>, manager: Arc<GatewayManager>) -> LabMcpServer {
        LabMcpServer {
            registry: Arc::new(crate::registry::build_default_registry()),
            access_runtime: runtime,
            file_stash_runtime: Arc::new(crate::file_stash::FileStashRuntime::blocked()),
            gateway_manager: Some(manager),
            peers: Default::default(),
            code_mode_app_state: Default::default(),
            last_listed_tool_contract: Default::default(),
            route_runtime: Default::default(),
            client_registry: Default::default(),
            transport_label: "test",
            logging_level: Arc::new(std::sync::atomic::AtomicU8::new(logging_level_rank(
                LoggingLevel::Emergency,
            ))),
            route_scope: McpRouteScope::protected_subset(
                "project-route",
                ["alpha"],
                ["gateway"],
                false,
            ),
            relay_session_id: 0,
            code_mode_widget_callbacks_enabled_for_test: false,
        }
    }

    static DESTRUCTIVE_DISPATCH_CALLS: AtomicUsize = AtomicUsize::new(0);
    static DESTRUCTIVE_ACTIONS: &[labby_primitives::action::ActionSpec] =
        &[labby_primitives::action::ActionSpec {
            name: "danger.delete",
            description: "test destructive action",
            destructive: true,
            requires_admin: false,
            params: &[],
            returns: "object",
        }];

    fn destructive_dispatch(
        _action: String,
        _params: serde_json::Value,
    ) -> std::pin::Pin<
        Box<
            dyn Future<Output = Result<serde_json::Value, crate::dispatch::error::ToolError>>
                + Send,
        >,
    > {
        Box::pin(async {
            DESTRUCTIVE_DISPATCH_CALLS.fetch_add(1, Ordering::SeqCst);
            Ok(serde_json::json!({"dispatched": true}))
        })
    }

    #[tokio::test]
    async fn call_tool_response_refuses_unsupported_destructive_call_before_dispatch() {
        DESTRUCTIVE_DISPATCH_CALLS.store(0, Ordering::SeqCst);
        let mut registry = crate::registry::ToolRegistry::new();
        registry.register(crate::registry::RegisteredService::bootstrap_operator(
            "danger",
            "test",
            "test",
            DESTRUCTIVE_ACTIONS,
            destructive_dispatch,
        ));
        let server = LabMcpServer {
            registry: Arc::new(registry),
            access_runtime: Arc::new(AccessRuntime::blocked_unavailable()),
            file_stash_runtime: Arc::new(crate::file_stash::FileStashRuntime::blocked()),
            gateway_manager: None,
            peers: Default::default(),
            code_mode_app_state: Default::default(),
            last_listed_tool_contract: Default::default(),
            route_runtime: Default::default(),
            client_registry: Default::default(),
            transport_label: "test",
            logging_level: Arc::new(std::sync::atomic::AtomicU8::new(logging_level_rank(
                LoggingLevel::Emergency,
            ))),
            route_scope: McpRouteScope::Root,
            relay_session_id: 0,
            code_mode_widget_callbacks_enabled_for_test: false,
        };
        let (transport, _client) = tokio::io::duplex(16 * 1024);
        let running = rmcp::service::serve_directly::<RoleServer, _, _, std::io::Error, _>(
            server, transport, None,
        );
        let response = running
            .service()
            .call_tool_response_impl(
                CallToolRequestParams::new("danger").with_arguments(serde_json::Map::from_iter([
                    ("action".into(), serde_json::json!("danger.delete")),
                ])),
                legacy_handler_context(running.peer().clone()),
            )
            .await
            .unwrap();
        let CallToolResponse::Complete(response) = response else {
            panic!("unsupported elicitation must return a terminal refusal")
        };
        assert!(
            serde_json::to_string(&response)
                .unwrap()
                .contains("confirmation_required")
        );
        assert_eq!(DESTRUCTIVE_DISPATCH_CALLS.load(Ordering::SeqCst), 0);
    }

    fn handler_context(
        peer: rmcp::service::Peer<RoleServer>,
        identity: Option<VerifiedIdentity>,
        binding: Result<
            TransportBoundAccessContext,
            crate::mcp::bound_access::BoundAccessContextError,
        >,
    ) -> RequestContext<RoleServer> {
        let mut context = RequestContext::new(rmcp::model::NumberOrString::Number(1), peer);
        let mut parts = axum::http::Request::new(()).into_parts().0;
        if let Some(identity) = identity {
            parts.extensions.insert(identity);
        }
        parts
            .extensions
            .insert(labby_auth::auth_context::AuthContext {
                sub: "reader".into(),
                actor_key: None,
                scopes: vec!["lab".into()],
                issuer: "https://issuer.example".into(),
                via_session: true,
                csrf_token: None,
                email: None,
            });
        attach_project_access_observation(&mut parts.extensions, binding);
        context.extensions.insert(parts);
        context
    }

    fn assert_complete_not_found(response: CallToolResponse) {
        let CallToolResponse::Complete(response) = response else {
            panic!("terminal project handler response must be Complete")
        };
        assert!(
            serde_json::to_string(&response)
                .unwrap()
                .contains("not_found")
        );
    }

    fn legacy_handler_context(peer: rmcp::service::Peer<RoleServer>) -> RequestContext<RoleServer> {
        RequestContext::new(rmcp::model::NumberOrString::Number(1), peer)
    }

    fn legacy_oauth_handler_context(
        peer: rmcp::service::Peer<RoleServer>,
    ) -> RequestContext<RoleServer> {
        let mut context = legacy_handler_context(peer);
        let mut parts = axum::http::Request::new(()).into_parts().0;
        parts
            .extensions
            .insert(labby_auth::auth_context::AuthContext {
                sub: "reader".into(),
                actor_key: None,
                scopes: vec!["lab".into()],
                issuer: "https://issuer.example".into(),
                via_session: true,
                csrf_token: None,
                email: None,
            });
        context.extensions.insert(parts);
        context
    }

    async fn start_delayed_call(
        runtime: Arc<AccessRuntime>,
        manager: Arc<GatewayManager>,
        pool: &Arc<UpstreamPool>,
        identity: VerifiedIdentity,
        calls: Arc<AtomicUsize>,
        fail: bool,
    ) -> (
        tokio::task::JoinHandle<Result<CallToolResponse, ToolExecutionResolutionError>>,
        Arc<Notify>,
        Arc<Notify>,
    ) {
        let started = Arc::new(Notify::new());
        let release = Arc::new(Notify::new());
        pool.install_tool_server_for_tests(
            "alpha",
            DelayedToolServer {
                calls,
                started: Arc::clone(&started),
                release: Arc::clone(&release),
                fail,
            },
        )
        .await;
        pool.insert_tool_routes_for_tests("alpha", vec![upstream_tool("nested/tool", false)])
            .await;
        let task = tokio::spawn(async move {
            execute_exact_project_tool(
                &runtime,
                &manager,
                ToolExecutionResolutionInput::new(
                    identity,
                    "project-route",
                    "https://mcp.example.com/project",
                    "bootstrap-default",
                    CallToolRequestParams::new("nested/tool"),
                ),
            )
            .await
        });
        (task, started, release)
    }

    #[tokio::test]
    async fn exact_asset_use_tool_rewrites_raw_name_and_rejects_owned_or_destructive_names() {
        let directory = tempfile::tempdir().unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            std::fs::set_permissions(directory.path(), std::fs::Permissions::from_mode(0o700))
                .unwrap();
        }
        let runtime = Arc::new(AccessRuntime::initialize(directory.path().join("access.db")).await);
        let identity = VerifiedIdentity::local_credential_with_issuer(
            Authenticator::StaticBearer,
            "server-static-issuer",
            "server-credential",
        )
        .unwrap();
        runtime
            .bootstrap_owner(
                BootstrapOwnerInput::new(identity.clone(), "Local", "Default").unwrap(),
            )
            .await
            .unwrap();
        runtime
            .store()
            .await
            .unwrap()
            .assign_project_loadout(
                AssignProjectLoadoutInput::new(identity.clone(), "bootstrap-default", "production")
                    .unwrap(),
            )
            .await
            .unwrap();
        let usage_store = Arc::new(
            labby_gateway::usage::UsageStore::open(directory.path().join("usage.db"))
                .await
                .unwrap(),
        );
        let calls = Arc::new(AtomicUsize::new(0));
        let received_meta = Arc::new(Mutex::new(Vec::new()));
        let pool = Arc::new(UpstreamPool::new().with_usage_store(Some(usage_store)));
        pool.install_tool_server_for_tests(
            "alpha",
            EchoToolServer {
                calls: Arc::clone(&calls),
                received_meta: Arc::clone(&received_meta),
            },
        )
        .await;
        pool.insert_tool_routes_for_tests("alpha", vec![upstream_tool("nested/tool", false)])
            .await;
        let gateway_runtime = GatewayRuntimeHandle::default();
        gateway_runtime.swap(Some(Arc::clone(&pool))).await;
        let path = directory.path().join("tool-execution.toml");
        let manager = Arc::new(
            GatewayManager::with_store(
                path.clone(),
                gateway_runtime.clone(),
                Arc::new(FsGatewayConfigStore::new(path)),
            )
            .with_builtin_service_registry(Arc::new(crate::registry::build_default_registry())),
        );
        manager.try_seed_config(config(true)).await.unwrap();
        let make_input = |name: &str| {
            ToolExecutionResolutionInput::new(
                identity.clone(),
                "project-route",
                "https://mcp.example.com/project",
                "bootstrap-default",
                {
                    let mut request = CallToolRequestParams::new(name.to_string()).with_arguments(
                        serde_json::Map::from_iter([("value".into(), serde_json::json!("kept"))]),
                    );
                    let mut meta = rmcp::model::RequestMetaObject::new();
                    meta.insert("trace-id".into(), serde_json::json!("opaque-meta"));
                    request.meta = Some(meta);
                    request
                },
            )
        };

        let response = execute_exact_project_tool(&runtime, &manager, make_input("nested/tool"))
            .await
            .unwrap();
        let CallToolResponse::Complete(result) = response else {
            panic!("regular fixture must complete")
        };
        let serialized = serde_json::to_value(result).unwrap();
        assert_eq!(serialized["content"][0]["text"], "nested/tool:\"kept\"");
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        assert_eq!(received_meta.lock().await[0]["trace-id"], "opaque-meta");
        for _ in 0..100 {
            if pool.usage_row_count_for_tests().await == 1 {
                break;
            }
            tokio::task::yield_now().await;
        }
        assert_eq!(pool.usage_row_count_for_tests().await, 1);

        assert!(matches!(
            execute_exact_project_tool(&runtime, &manager, make_input("unknown")).await,
            Err(ToolExecutionResolutionError::Unavailable)
        ));
        assert_eq!(calls.load(Ordering::SeqCst), 1);

        let bravo_calls = Arc::new(AtomicUsize::new(0));
        pool.install_tool_server_for_tests(
            "bravo",
            EchoToolServer {
                calls: Arc::clone(&bravo_calls),
                received_meta: Arc::new(Mutex::new(Vec::new())),
            },
        )
        .await;
        let mut duplicate = upstream_tool("nested/tool", false);
        duplicate.upstream_name = Arc::from("bravo");
        pool.insert_tool_routes_for_tests("bravo", vec![duplicate])
            .await;
        assert!(matches!(
            execute_exact_project_tool(&runtime, &manager, make_input("nested/tool")).await,
            Err(ToolExecutionResolutionError::Unavailable)
        ));
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        assert_eq!(bravo_calls.load(Ordering::SeqCst), 0);

        let mut reverse_order = config(true);
        reverse_order.upstream.reverse();
        reverse_order.loadouts[0].upstreams.reverse();
        manager.try_seed_config(reverse_order).await.unwrap();
        assert!(matches!(
            execute_exact_project_tool(&runtime, &manager, make_input("nested/tool")).await,
            Err(ToolExecutionResolutionError::Unavailable)
        ));
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        assert_eq!(bravo_calls.load(Ordering::SeqCst), 0);
        pool.insert_tool_routes_for_tests("bravo", Vec::new()).await;

        let excluded_calls = Arc::new(AtomicUsize::new(0));
        pool.install_tool_server_for_tests(
            "charlie",
            EchoToolServer {
                calls: Arc::clone(&excluded_calls),
                received_meta: Arc::new(Mutex::new(Vec::new())),
            },
        )
        .await;
        let mut excluded = upstream_tool("excluded", false);
        excluded.upstream_name = Arc::from("charlie");
        pool.insert_tool_routes_for_tests("charlie", vec![excluded])
            .await;
        assert!(matches!(
            execute_exact_project_tool(&runtime, &manager, make_input("excluded")).await,
            Err(ToolExecutionResolutionError::Unavailable)
        ));
        assert_eq!(excluded_calls.load(Ordering::SeqCst), 0);

        for (name, destructive) in [
            (CODE_MODE_TOOL_NAME, false),
            (CODE_MODE_READ_TOOL_NAME, false),
            (CODE_MODE_UI_TOOL_NAME, false),
            (MCP_APP_TOOL_NAME, false),
            (ADD_SERVER_TOOL_NAME, false),
            (GATEWAY_STATUS_TOOL_NAME, false),
            (SETTINGS_TOOL_NAME, false),
            ("gateway", false),
            ("danger", true),
        ] {
            pool.insert_tool_routes_for_tests("alpha", vec![upstream_tool(name, destructive)])
                .await;
            let actual = execute_exact_project_tool(&runtime, &manager, make_input(name)).await;
            assert!(
                matches!(&actual, Err(ToolExecutionResolutionError::Unavailable)),
                "{name}: {actual:?}"
            );
            assert_eq!(calls.load(Ordering::SeqCst), 1, "{name}");
        }

        let delayed_calls = Arc::new(AtomicUsize::new(0));
        let started = Arc::new(Notify::new());
        let release = Arc::new(Notify::new());
        pool.install_tool_server_for_tests(
            "alpha",
            DelayedToolServer {
                calls: Arc::clone(&delayed_calls),
                started: Arc::clone(&started),
                release: Arc::clone(&release),
                fail: false,
            },
        )
        .await;
        pool.insert_tool_routes_for_tests("alpha", vec![upstream_tool("nested/tool", false)])
            .await;
        let task_runtime = Arc::clone(&runtime);
        let task_manager = Arc::clone(&manager);
        let task_identity = identity.clone();
        let task = tokio::spawn(async move {
            execute_exact_project_tool(
                &task_runtime,
                &task_manager,
                ToolExecutionResolutionInput::new(
                    task_identity,
                    "project-route",
                    "https://mcp.example.com/project",
                    "bootstrap-default",
                    CallToolRequestParams::new("nested/tool"),
                ),
            )
            .await
        });
        started.notified().await;
        manager.try_seed_config(config(false)).await.unwrap();
        manager.try_seed_config(config(true)).await.unwrap();
        release.notify_one();
        assert!(matches!(
            task.await.unwrap(),
            Err(ToolExecutionResolutionError::Unavailable)
        ));
        assert_eq!(delayed_calls.load(Ordering::SeqCst), 1);

        let access_started = Arc::new(Notify::new());
        let access_release = Arc::new(Notify::new());
        pool.install_tool_server_for_tests(
            "alpha",
            DelayedToolServer {
                calls: Arc::clone(&delayed_calls),
                started: Arc::clone(&access_started),
                release: Arc::clone(&access_release),
                fail: false,
            },
        )
        .await;
        pool.insert_tool_routes_for_tests("alpha", vec![upstream_tool("nested/tool", false)])
            .await;
        let task_runtime = Arc::clone(&runtime);
        let task_manager = Arc::clone(&manager);
        let task_identity = identity.clone();
        let access_task = tokio::spawn(async move {
            execute_exact_project_tool(
                &task_runtime,
                &task_manager,
                ToolExecutionResolutionInput::new(
                    task_identity,
                    "project-route",
                    "https://mcp.example.com/project",
                    "bootstrap-default",
                    CallToolRequestParams::new("nested/tool"),
                ),
            )
            .await
        });
        access_started.notified().await;
        let store = runtime.store().await.unwrap();
        store
            .execute_test_statement(
                "UPDATE project_memberships SET role='viewer' WHERE project_id='bootstrap-default';
             UPDATE access_metadata SET global_revision=global_revision+1 WHERE singleton=1",
            )
            .await
            .unwrap();
        store
            .execute_test_statement(
                "UPDATE project_memberships SET role='owner' WHERE project_id='bootstrap-default';
             UPDATE access_metadata SET global_revision=global_revision+1 WHERE singleton=1",
            )
            .await
            .unwrap();
        access_release.notify_one();
        assert!(matches!(
            access_task.await.unwrap(),
            Err(ToolExecutionResolutionError::Unavailable)
        ));
        assert_eq!(delayed_calls.load(Ordering::SeqCst), 2);

        let (tool_task, tool_started, tool_release) = start_delayed_call(
            Arc::clone(&runtime),
            Arc::clone(&manager),
            &pool,
            identity.clone(),
            Arc::clone(&delayed_calls),
            false,
        )
        .await;
        tool_started.notified().await;
        let before_tool_generation = pool.published_tool_catalog().await.unwrap().generation();
        pool.insert_tool_routes_for_tests("alpha", vec![upstream_tool("replacement", false)])
            .await;
        pool.insert_tool_routes_for_tests("alpha", vec![upstream_tool("nested/tool", false)])
            .await;
        assert_ne!(
            pool.published_tool_catalog().await.unwrap().generation(),
            before_tool_generation
        );
        tool_release.notify_one();
        assert!(matches!(
            tool_task.await.unwrap(),
            Err(ToolExecutionResolutionError::Unavailable)
        ));

        let (safety_task, safety_started, safety_release) = start_delayed_call(
            Arc::clone(&runtime),
            Arc::clone(&manager),
            &pool,
            identity.clone(),
            Arc::clone(&delayed_calls),
            false,
        )
        .await;
        safety_started.notified().await;
        pool.insert_tool_routes_for_tests("alpha", vec![upstream_tool("nested/tool", true)])
            .await;
        pool.insert_tool_routes_for_tests("alpha", vec![upstream_tool("nested/tool", false)])
            .await;
        safety_release.notify_one();
        assert!(matches!(
            safety_task.await.unwrap(),
            Err(ToolExecutionResolutionError::Unavailable)
        ));

        let (service_task, service_started, service_release) = start_delayed_call(
            Arc::clone(&runtime),
            Arc::clone(&manager),
            &pool,
            identity.clone(),
            Arc::clone(&delayed_calls),
            false,
        )
        .await;
        service_started.notified().await;
        manager.set_builtin_service_registry(Arc::new(crate::registry::build_default_registry()));
        service_release.notify_one();
        assert!(matches!(
            service_task.await.unwrap(),
            Err(ToolExecutionResolutionError::Unavailable)
        ));
        assert_eq!(delayed_calls.load(Ordering::SeqCst), 5);
        for _ in 0..100 {
            if pool.usage_row_count_for_tests().await >= 5 {
                break;
            }
            tokio::task::yield_now().await;
        }
        let usage_before_pool_aba = pool.usage_row_count_for_tests().await;

        let (pool_task, pool_started, pool_release) = start_delayed_call(
            Arc::clone(&runtime),
            Arc::clone(&manager),
            &pool,
            identity.clone(),
            Arc::clone(&delayed_calls),
            false,
        )
        .await;
        pool_started.notified().await;
        pool.set_tool_last_error_for_tests("alpha", Some("sentinel".into()))
            .await;
        let replacement = Arc::new(UpstreamPool::new());
        replacement
            .install_tool_server_for_tests(
                "alpha",
                EchoToolServer {
                    calls: Arc::new(AtomicUsize::new(0)),
                    received_meta: Arc::new(Mutex::new(Vec::new())),
                },
            )
            .await;
        replacement
            .insert_tool_routes_for_tests("alpha", vec![upstream_tool("nested/tool", false)])
            .await;
        gateway_runtime.swap(Some(replacement)).await;
        gateway_runtime.swap(Some(Arc::clone(&pool))).await;
        pool_release.notify_one();
        assert!(matches!(
            pool_task.await.unwrap(),
            Err(ToolExecutionResolutionError::Unavailable)
        ));
        assert_eq!(
            pool.upstream_tool_last_error("alpha").await.as_deref(),
            Some("sentinel")
        );
        assert!(pool.header_recovery_is_empty_for_tests("alpha"));
        assert_eq!(
            pool.usage_row_count_for_tests().await,
            usage_before_pool_aba
        );

        let (pool_error_task, pool_error_started, pool_error_release) = start_delayed_call(
            Arc::clone(&runtime),
            Arc::clone(&manager),
            &pool,
            identity.clone(),
            Arc::clone(&delayed_calls),
            true,
        )
        .await;
        pool_error_started.notified().await;
        pool.set_tool_last_error_for_tests("alpha", Some("error-sentinel".into()))
            .await;
        gateway_runtime
            .swap(Some(Arc::new(UpstreamPool::new())))
            .await;
        gateway_runtime.swap(Some(Arc::clone(&pool))).await;
        pool_error_release.notify_one();
        assert!(matches!(
            pool_error_task.await.unwrap(),
            Err(ToolExecutionResolutionError::Unavailable)
        ));
        assert_eq!(
            pool.upstream_tool_last_error("alpha").await.as_deref(),
            Some("error-sentinel")
        );
        assert!(pool.header_recovery_is_empty_for_tests("alpha"));
        assert_eq!(
            pool.usage_row_count_for_tests().await,
            usage_before_pool_aba
        );

        let (cancel_task, cancel_started, cancel_release) = start_delayed_call(
            Arc::clone(&runtime),
            Arc::clone(&manager),
            &pool,
            identity.clone(),
            Arc::clone(&delayed_calls),
            false,
        )
        .await;
        cancel_started.notified().await;
        cancel_task.abort();
        assert!(cancel_task.await.unwrap_err().is_cancelled());
        cancel_release.notify_one();
        pool.install_tool_server_for_tests(
            "alpha",
            EchoToolServer {
                calls: Arc::clone(&calls),
                received_meta: Arc::clone(&received_meta),
            },
        )
        .await;
        pool.insert_tool_routes_for_tests("alpha", vec![upstream_tool("nested/tool", false)])
            .await;
        assert!(
            execute_exact_project_tool(&runtime, &manager, make_input("nested/tool"))
                .await
                .is_ok()
        );
        assert_eq!(calls.load(Ordering::SeqCst), 2);

        pool.insert_tool_routes_for_tests("alpha", vec![upstream_tool("nested/tool", false)])
            .await;
        manager.try_seed_config(config(false)).await.unwrap();
        assert!(matches!(
            execute_exact_project_tool(&runtime, &manager, make_input("nested/tool")).await,
            Err(ToolExecutionResolutionError::Unavailable)
        ));
        assert_eq!(calls.load(Ordering::SeqCst), 2);

        manager.try_seed_config(config(true)).await.unwrap();
        runtime
            .store()
            .await
            .unwrap()
            .execute_test_statement(
                "UPDATE project_memberships SET role='viewer' WHERE project_id='bootstrap-default'",
            )
            .await
            .unwrap();
        assert!(matches!(
            execute_exact_project_tool(&runtime, &manager, make_input("nested/tool")).await,
            Err(ToolExecutionResolutionError::Unavailable)
        ));
        assert_eq!(calls.load(Ordering::SeqCst), 2);

        runtime
            .store()
            .await
            .unwrap()
            .execute_test_statement(
                "UPDATE project_memberships SET role='owner' WHERE project_id='bootstrap-default';
                 UPDATE access_metadata SET global_revision=global_revision+1 WHERE singleton=1",
            )
            .await
            .unwrap();
        let before = UNIX_EPOCH + Duration::from_secs(100);
        let after = UNIX_EPOCH + Duration::from_secs(102);
        let transport =
            transport_binding(&runtime, &manager, identity.clone(), usize::MAX, before).await;
        let mut transport_request = CallToolRequestParams::new("nested/tool");
        transport_request.arguments = Some(serde_json::Map::from_iter([(
            "value".into(),
            serde_json::json!("transport-kept"),
        )]));
        let mut untrusted_meta = rmcp::model::RequestMetaObject::new();
        untrusted_meta.insert("route_name".into(), serde_json::json!("attacker-route"));
        untrusted_meta.insert("project_id".into(), serde_json::json!("attacker-project"));
        untrusted_meta.insert(
            "resource".into(),
            serde_json::json!("https://attacker.invalid"),
        );
        transport_request.meta = Some(untrusted_meta);
        let transport_response = execute_transport_bound_project_tool_with_clock(
            &runtime,
            &manager,
            &transport,
            &identity,
            transport_request,
            || before,
        )
        .await
        .unwrap();
        let CallToolResponse::Complete(transport_response) = transport_response else {
            panic!("transport fixture must complete")
        };
        assert_eq!(
            serde_json::to_value(transport_response).unwrap()["content"][0]["text"],
            "nested/tool:\"transport-kept\""
        );
        let received = received_meta.lock().await;
        assert_eq!(received.last().unwrap()["route_name"], "attacker-route");
        assert_eq!(received.last().unwrap()["project_id"], "attacker-project");
        drop(received);
        assert_eq!(calls.load(Ordering::SeqCst), 3);

        let expiring = transport_binding(&runtime, &manager, identity.clone(), 101, before).await;
        assert!(matches!(
            execute_transport_bound_project_tool_with_clock(
                &runtime,
                &manager,
                &expiring,
                &identity,
                CallToolRequestParams::new("nested/tool"),
                || after,
            )
            .await,
            Err(ToolExecutionResolutionError::Unavailable)
        ));
        assert_eq!(calls.load(Ordering::SeqCst), 3);

        let other_identity = VerifiedIdentity::local_credential_with_issuer(
            Authenticator::StaticBearer,
            "other-issuer",
            "other-credential",
        )
        .unwrap();
        assert!(matches!(
            execute_transport_bound_project_tool_with_clock(
                &runtime,
                &manager,
                &transport,
                &other_identity,
                CallToolRequestParams::new("nested/tool"),
                || before,
            )
            .await,
            Err(ToolExecutionResolutionError::Unavailable)
        ));
        assert_eq!(calls.load(Ordering::SeqCst), 3);

        let expiring = transport_binding(&runtime, &manager, identity.clone(), 101, before).await;
        let mut times = [before, after].into_iter();
        assert!(matches!(
            execute_transport_bound_project_tool_with_clock(
                &runtime,
                &manager,
                &expiring,
                &identity,
                CallToolRequestParams::new("nested/tool"),
                || times.next().unwrap(),
            )
            .await,
            Err(ToolExecutionResolutionError::Unavailable)
        ));
        assert_eq!(calls.load(Ordering::SeqCst), 4);

        let valid_transport =
            transport_binding(&runtime, &manager, identity.clone(), usize::MAX, before).await;
        let variant_schema = ElicitationSchema::builder()
            .required_property(
                "confirm",
                PrimitiveSchemaDefinition::Boolean(BooleanSchema::default()),
            )
            .build()
            .unwrap();
        for response in [
            CallToolResponse::Task(CreateTaskResult::new(Task::new(
                "transport-task",
                TaskStatus::Working,
                "2026-08-24T00:00:00Z",
                "2026-08-24T00:00:00Z",
            ))),
            CallToolResponse::InputRequired(InputRequiredResult::from_input_requests(
                InputRequests::from([(
                    "confirmation".into(),
                    InputRequest::Elicitation(ElicitRequest::new(
                        ElicitRequestParams::FormElicitationParams {
                            meta: None,
                            message: "confirm?".into(),
                            requested_schema: variant_schema.clone(),
                        },
                    )),
                )]),
            )),
        ] {
            let preserved = finish_transport_bound_tool_result(
                &valid_transport,
                &identity,
                before,
                Ok(response.clone()),
            )
            .unwrap();
            match (preserved, response) {
                (CallToolResponse::Task(actual), CallToolResponse::Task(expected)) => assert_eq!(
                    serde_json::to_value(actual).unwrap(),
                    serde_json::to_value(expected).unwrap()
                ),
                (
                    CallToolResponse::InputRequired(actual),
                    CallToolResponse::InputRequired(expected),
                ) => assert_eq!(
                    serde_json::to_value(actual).unwrap(),
                    serde_json::to_value(expected).unwrap()
                ),
                _ => panic!("transport finish gate changed response variant"),
            }
        }
        assert!(matches!(
            finish_transport_bound_tool_result(
                &valid_transport,
                &identity,
                before,
                Err(ToolExecutionResolutionError::Mcp {
                    kind: "internal_error",
                    code: -32_603,
                }),
            ),
            Err(ToolExecutionResolutionError::Mcp {
                kind: "internal_error",
                code: -32_603,
            })
        ));
        assert!(matches!(
            finish_transport_bound_tool_result(
                &valid_transport,
                &identity,
                before,
                Err(ToolExecutionResolutionError::Transport),
            ),
            Err(ToolExecutionResolutionError::Transport)
        ));
        for error in [
            ToolExecutionResolutionError::Mcp {
                kind: "internal_error",
                code: -32_603,
            },
            ToolExecutionResolutionError::Transport,
        ] {
            assert!(matches!(
                finish_transport_bound_tool_result(&expiring, &identity, after, Err(error),),
                Err(ToolExecutionResolutionError::Unavailable)
            ));
        }

        let complete_transport =
            transport_binding(&runtime, &manager, identity.clone(), usize::MAX, before).await;
        let complete = execute_transport_bound_project_complete_tool_with_clock(
            &runtime,
            &manager,
            &complete_transport,
            &identity,
            CallToolRequestParams::new("nested/tool").with_arguments(serde_json::Map::from_iter([
                ("value".into(), serde_json::json!("complete-only")),
            ])),
            || before,
        )
        .await
        .unwrap();
        assert_eq!(
            serde_json::to_value(complete).unwrap()["content"][0]["text"],
            "nested/tool:\"complete-only\""
        );
        assert_eq!(calls.load(Ordering::SeqCst), 5);

        let secret_schema = ElicitationSchema::builder()
            .required_property(
                "secret-schema-field",
                PrimitiveSchemaDefinition::Boolean(BooleanSchema::default()),
            )
            .build()
            .unwrap();
        let unsupported = [
            (
                CallToolResponse::Task(CreateTaskResult::new(Task::new(
                    "secret-native-task-id",
                    TaskStatus::Working,
                    "2026-08-24T00:00:00Z",
                    "2026-08-24T00:00:00Z",
                ))),
                [
                    "secret-native-task-id",
                    "secret-schema-field",
                    "secret-message",
                ],
                ToolExecutionResolutionError::Mcp {
                    kind: "validation_failed",
                    code: -32_021,
                },
            ),
            (
                CallToolResponse::InputRequired(InputRequiredResult::from_input_requests(
                    InputRequests::from([(
                        "secret-input-key".into(),
                        InputRequest::Elicitation(ElicitRequest::new(
                            ElicitRequestParams::FormElicitationParams {
                                meta: None,
                                message: "secret-message".into(),
                                requested_schema: secret_schema,
                            },
                        )),
                    )]),
                )),
                ["secret-input-key", "secret-schema-field", "secret-message"],
                ToolExecutionResolutionError::Mcp {
                    kind: "upstream_error",
                    code: -32_600,
                },
            ),
        ];
        for (response, secrets, expected_wire_error) in unsupported {
            let injected_error = finish_complete_tool_response(response.clone()).unwrap_err();
            assert_eq!(
                injected_error,
                ToolExecutionResolutionError::UnsupportedTerminalResponse
            );
            let injected_rendered = injected_error.to_string();
            for secret in secrets {
                assert!(!injected_rendered.contains(secret), "leaked {secret}");
            }
            let variant_calls = Arc::new(AtomicUsize::new(0));
            pool.install_tool_server_for_tests(
                "alpha",
                TerminalVariantToolServer {
                    calls: Arc::clone(&variant_calls),
                    response,
                },
            )
            .await;
            pool.insert_tool_routes_for_tests("alpha", vec![upstream_tool("nested/tool", false)])
                .await;
            let variant_transport =
                transport_binding(&runtime, &manager, identity.clone(), usize::MAX, before).await;
            let error = execute_transport_bound_project_complete_tool_with_clock(
                &runtime,
                &manager,
                &variant_transport,
                &identity,
                CallToolRequestParams::new("nested/tool"),
                || before,
            )
            .await
            .unwrap_err();
            assert_eq!(error, expected_wire_error);
            let rendered = error.to_string();
            for secret in secrets {
                assert!(!rendered.contains(secret), "leaked {secret}");
            }
            assert_eq!(variant_calls.load(Ordering::SeqCst), 1);
            tokio::task::yield_now().await;
            assert_eq!(variant_calls.load(Ordering::SeqCst), 1);
        }

        for (code, message, expected_kind) in [
            (
                -32_021,
                "Missing required client capability",
                "validation_failed",
            ),
            (
                -32_600,
                "InputRequiredResult requires negotiated protocol version 2026-07-28 or newer",
                "upstream_error",
            ),
        ] {
            let spoof_calls = Arc::new(AtomicUsize::new(0));
            pool.install_tool_server_for_tests(
                "alpha",
                McpErrorToolServer {
                    calls: Arc::clone(&spoof_calls),
                    code,
                    message,
                },
            )
            .await;
            pool.insert_tool_routes_for_tests("alpha", vec![upstream_tool("nested/tool", false)])
                .await;
            let spoof_transport =
                transport_binding(&runtime, &manager, identity.clone(), usize::MAX, before).await;
            let error = execute_transport_bound_project_complete_tool_with_clock(
                &runtime,
                &manager,
                &spoof_transport,
                &identity,
                CallToolRequestParams::new("nested/tool"),
                || before,
            )
            .await
            .unwrap_err();
            assert_eq!(
                error,
                ToolExecutionResolutionError::Mcp {
                    kind: expected_kind,
                    code,
                }
            );
            assert!(!error.to_string().contains("spoof-secret"));
            assert_eq!(spoof_calls.load(Ordering::SeqCst), 1);
        }

        let expired_variant_calls = Arc::new(AtomicUsize::new(0));
        pool.install_tool_server_for_tests(
            "alpha",
            TerminalVariantToolServer {
                calls: Arc::clone(&expired_variant_calls),
                response: CallToolResponse::Task(CreateTaskResult::new(Task::new(
                    "post-expiry-secret-task",
                    TaskStatus::Working,
                    "2026-08-24T00:00:00Z",
                    "2026-08-24T00:00:00Z",
                ))),
            },
        )
        .await;
        pool.insert_tool_routes_for_tests("alpha", vec![upstream_tool("nested/tool", false)])
            .await;
        let expiring = transport_binding(&runtime, &manager, identity.clone(), 101, before).await;
        let mut variant_times = [before, after].into_iter();
        assert!(matches!(
            execute_transport_bound_project_complete_tool_with_clock(
                &runtime,
                &manager,
                &expiring,
                &identity,
                CallToolRequestParams::new("nested/tool"),
                || variant_times.next().unwrap(),
            )
            .await,
            Err(ToolExecutionResolutionError::Unavailable)
        ));
        assert_eq!(expired_variant_calls.load(Ordering::SeqCst), 1);

        let transport_cancel_calls = Arc::new(AtomicUsize::new(0));
        let transport_cancel_started = Arc::new(Notify::new());
        let transport_cancel_release = Arc::new(Notify::new());
        pool.install_tool_server_for_tests(
            "alpha",
            DelayedToolServer {
                calls: Arc::clone(&transport_cancel_calls),
                started: Arc::clone(&transport_cancel_started),
                release: Arc::clone(&transport_cancel_release),
                fail: false,
            },
        )
        .await;
        pool.insert_tool_routes_for_tests("alpha", vec![upstream_tool("nested/tool", false)])
            .await;
        let transport =
            transport_binding(&runtime, &manager, identity.clone(), usize::MAX, before).await;
        let cancel_runtime = Arc::clone(&runtime);
        let cancel_manager = Arc::clone(&manager);
        let cancel_identity = identity.clone();
        let transport_cancel_task = tokio::spawn(async move {
            execute_transport_bound_project_complete_tool_with_clock(
                &cancel_runtime,
                &cancel_manager,
                &transport,
                &cancel_identity,
                CallToolRequestParams::new("nested/tool"),
                || before,
            )
            .await
        });
        tokio::time::timeout(Duration::from_secs(5), transport_cancel_started.notified())
            .await
            .expect("transport-bound exact call must reach upstream before cancellation");
        transport_cancel_task.abort();
        assert!(transport_cancel_task.await.unwrap_err().is_cancelled());
        transport_cancel_release.notify_one();
        assert_eq!(transport_cancel_calls.load(Ordering::SeqCst), 1);

        let handler_calls = Arc::new(AtomicUsize::new(0));
        pool.install_tool_server_for_tests(
            "alpha",
            EchoToolServer {
                calls: Arc::clone(&handler_calls),
                received_meta: Arc::new(Mutex::new(Vec::new())),
            },
        )
        .await;
        pool.insert_tool_routes_for_tests("alpha", vec![upstream_tool("nested/tool", false)])
            .await;
        let mut handler_config = config(true);
        handler_config
            .upstream
            .retain(|upstream| upstream.name == "alpha");
        handler_config.loadouts[0].upstreams = vec!["alpha".into()];
        manager.try_seed_config(handler_config).await.unwrap();
        let (handler_transport, mut handler_client) = tokio::io::duplex(64 * 1024);
        let _handler_client_drain = tokio::spawn(async move {
            let mut sink = tokio::io::sink();
            let _copy_result = tokio::io::copy(&mut handler_client, &mut sink).await;
        });
        let running = rmcp::service::serve_directly::<RoleServer, _, _, std::io::Error, _>(
            handler_server(Arc::clone(&runtime), Arc::clone(&manager)),
            handler_transport,
            None,
        );
        let ownership =
            transport_binding(&runtime, &manager, identity.clone(), usize::MAX, before).await;
        assert_eq!(
            transport_bound_tool_ownership(&ownership, "gateway"),
            ProjectToolOwnership::OwnedLabby
        );
        assert_eq!(
            transport_bound_tool_ownership(&ownership, CODE_MODE_TOOL_NAME),
            ProjectToolOwnership::OwnedLabby
        );
        assert_eq!(
            transport_bound_tool_ownership(&ownership, "doctor"),
            ProjectToolOwnership::Regular,
            "global registration cannot grant ownership when absent from the bound publication"
        );
        assert_eq!(
            transport_bound_tool_ownership(&ownership, "nested/tool"),
            ProjectToolOwnership::Regular
        );
        let bound =
            transport_binding(&runtime, &manager, identity.clone(), usize::MAX, before).await;
        let response = running
            .service()
            .call_tool_response_impl(
                CallToolRequestParams::new("nested/tool").with_arguments(
                    serde_json::Map::from_iter([("value".into(), serde_json::json!("handler"))]),
                ),
                handler_context(running.peer().clone(), Some(identity.clone()), Ok(bound)),
            )
            .await
            .unwrap();
        let CallToolResponse::Complete(response) = response else {
            panic!("Project regular handler must be Complete-only")
        };
        assert_eq!(
            serde_json::to_value(response).unwrap()["content"][0]["text"],
            "nested/tool:\"handler\""
        );
        assert_eq!(handler_calls.load(Ordering::SeqCst), 1);

        let (legacy_upstream, legacy_tool) = manager
            .resolve_raw_upstream_tool("nested/tool", None, None)
            .await
            .unwrap();
        assert_eq!(legacy_upstream, "alpha");
        assert!(!legacy_tool.destructive);
        assert_eq!(
            manager
                .upstream_config("alpha")
                .await
                .unwrap()
                .env
                .get("MCP_UPSTREAM_RELAY_MODE")
                .map(String::as_str),
            Some("pooled")
        );
        assert!(
            !tokio::time::timeout(
                Duration::from_secs(5),
                running.service().tool_request_is_destructive(
                    &CallToolRequestParams::new("nested/tool"),
                    &legacy_handler_context(running.peer().clone()),
                ),
            )
            .await
            .expect("Legacy destructive classification must be bounded")
        );
        let legacy_response = tokio::time::timeout(
            Duration::from_secs(5),
            running.service().call_tool_response_impl(
                CallToolRequestParams::new("nested/tool").with_arguments(
                    serde_json::Map::from_iter([("value".into(), serde_json::json!("legacy"))]),
                ),
                legacy_handler_context(running.peer().clone()),
            ),
        )
        .await
        .expect("Legacy pooled raw proxy must not enter relay")
        .unwrap();
        let CallToolResponse::Complete(legacy_response) = legacy_response else {
            panic!("Legacy pooled raw proxy path changed response variant")
        };
        assert_eq!(
            serde_json::to_value(legacy_response).unwrap()["content"][0]["text"],
            "nested/tool:\"legacy\""
        );
        assert_eq!(handler_calls.load(Ordering::SeqCst), 2);

        pool.insert_tool_routes_for_tests("alpha", vec![upstream_tool("nested/tool", true)])
            .await;
        let refused = running
            .service()
            .call_tool_response_impl(
                CallToolRequestParams::new("nested/tool"),
                legacy_handler_context(running.peer().clone()),
            )
            .await
            .unwrap();
        let CallToolResponse::Complete(refused) = refused else {
            panic!("unsupported elicitation must return a terminal refusal")
        };
        assert!(
            serde_json::to_string(&refused)
                .unwrap()
                .contains("confirmation_required")
        );
        assert_eq!(
            handler_calls.load(Ordering::SeqCst),
            2,
            "destructive dispatch must not run when form elicitation is unsupported"
        );
        pool.insert_tool_routes_for_tests("alpha", vec![upstream_tool("nested/tool", false)])
            .await;

        let owned =
            transport_binding(&runtime, &manager, identity.clone(), usize::MAX, before).await;
        let owned_response = running
            .service()
            .call_tool_response_impl(
                CallToolRequestParams::new("gateway").with_arguments(serde_json::Map::from_iter([
                    ("action".into(), serde_json::json!("help")),
                ])),
                handler_context(running.peer().clone(), Some(identity.clone()), Ok(owned)),
            )
            .await
            .unwrap();
        assert!(matches!(owned_response, CallToolResponse::Complete(_)));
        assert_eq!(handler_calls.load(Ordering::SeqCst), 2);

        let (owned_guard_transport, _owned_guard_client) = tokio::io::duplex(64 * 1024);
        let mut owned_guard_server = handler_server(Arc::clone(&runtime), Arc::clone(&manager));
        owned_guard_server.registry = Arc::new(crate::registry::ToolRegistry::default());
        let owned_guard_running =
            rmcp::service::serve_directly::<RoleServer, _, _, std::io::Error, _>(
                owned_guard_server,
                owned_guard_transport,
                None,
            );
        let owned_guard =
            transport_binding(&runtime, &manager, identity.clone(), usize::MAX, before).await;
        let owned_guard_response = owned_guard_running
            .service()
            .call_tool_response_impl(
                CallToolRequestParams::new("gateway"),
                handler_context(
                    owned_guard_running.peer().clone(),
                    Some(identity.clone()),
                    Ok(owned_guard),
                ),
            )
            .await
            .unwrap();
        let CallToolResponse::Complete(owned_guard_response) = owned_guard_response else {
            panic!("Owned but undispatchable Tool must remain a Complete error")
        };
        assert!(
            serde_json::to_string(&owned_guard_response)
                .unwrap()
                .contains("not_found")
        );
        assert_eq!(handler_calls.load(Ordering::SeqCst), 2);

        let unknown =
            transport_binding(&runtime, &manager, identity.clone(), usize::MAX, before).await;
        let unknown_response = running
            .service()
            .call_tool_response_impl(
                CallToolRequestParams::new("unknown-project-tool"),
                handler_context(running.peer().clone(), Some(identity.clone()), Ok(unknown)),
            )
            .await
            .unwrap();
        let CallToolResponse::Complete(unknown_response) = unknown_response else {
            panic!("Project unknown handler response must remain Complete-only")
        };
        assert!(
            serde_json::to_string(&unknown_response)
                .unwrap()
                .contains("not_found")
        );
        assert_eq!(handler_calls.load(Ordering::SeqCst), 2);

        let unavailable_response = running
            .service()
            .call_tool_response_impl(
                CallToolRequestParams::new("nested/tool"),
                handler_context(
                    running.peer().clone(),
                    Some(identity.clone()),
                    Err(crate::mcp::bound_access::BoundAccessContextError::Unavailable),
                ),
            )
            .await
            .unwrap();
        let CallToolResponse::Complete(unavailable_response) = unavailable_response else {
            panic!("Unavailable Project observation must be terminal Complete error")
        };
        assert!(
            serde_json::to_string(&unavailable_response)
                .unwrap()
                .contains("not_found")
        );
        assert_eq!(handler_calls.load(Ordering::SeqCst), 2);

        let missing_identity =
            transport_binding(&runtime, &manager, identity.clone(), usize::MAX, before).await;
        let missing_identity_response = running
            .service()
            .call_tool_response_impl(
                CallToolRequestParams::new("nested/tool"),
                handler_context(running.peer().clone(), None, Ok(missing_identity)),
            )
            .await
            .unwrap();
        assert_complete_not_found(missing_identity_response);
        assert_eq!(handler_calls.load(Ordering::SeqCst), 2);

        let mismatched =
            transport_binding(&runtime, &manager, identity.clone(), usize::MAX, before).await;
        let mismatched_identity = VerifiedIdentity::local_credential_with_issuer(
            Authenticator::StaticBearer,
            "server-static-issuer",
            "other-credential",
        )
        .unwrap();
        let mismatched_response = running
            .service()
            .call_tool_response_impl(
                CallToolRequestParams::new("nested/tool"),
                handler_context(
                    running.peer().clone(),
                    Some(mismatched_identity),
                    Ok(mismatched),
                ),
            )
            .await
            .unwrap();
        assert_complete_not_found(mismatched_response);
        assert_eq!(handler_calls.load(Ordering::SeqCst), 2);

        let expired = transport_binding(&runtime, &manager, identity.clone(), 101, before).await;
        let expired_response = running
            .service()
            .call_tool_response_impl(
                CallToolRequestParams::new("nested/tool"),
                handler_context(running.peer().clone(), Some(identity.clone()), Ok(expired)),
            )
            .await
            .unwrap();
        assert_complete_not_found(expired_response);
        assert_eq!(handler_calls.load(Ordering::SeqCst), 2);

        let mut oauth_config = config(true);
        oauth_config
            .upstream
            .retain(|upstream| upstream.name == "alpha");
        let mut oauth = oauth_config.upstream[0].clone();
        oauth.name = "oauth".into();
        oauth.command = None;
        oauth.url = Some("http://127.0.0.1:9/mcp".into());
        oauth.oauth = Some(UpstreamOauthConfig {
            mode: UpstreamOauthMode::AuthorizationCodePkce,
            registration: UpstreamOauthRegistration::Dynamic,
            scopes: None,
            credential: Default::default(),
            prefer_client_metadata_document: None,
        });
        oauth_config.upstream.push(oauth.clone());
        oauth_config.loadouts[0].upstreams = vec!["alpha".into(), "oauth".into()];
        manager.try_seed_config(oauth_config).await.unwrap();
        let oauth_calls = Arc::new(AtomicUsize::new(0));
        pool.install_test_subject_server_for_upstream(
            &oauth,
            "reader",
            McpErrorToolServer {
                calls: Arc::clone(&oauth_calls),
                code: -32_602,
                message: "private oauth subject failure",
            },
        )
        .await;
        pool.install_test_subject_tools_for_upstream(
            &oauth,
            "reader",
            vec![
                Tool::new("oauth-tool", "subject", Arc::new(serde_json::Map::new()))
                    .with_annotations(
                        rmcp::model::ToolAnnotations::new()
                            .read_only(true)
                            .destructive(false),
                    ),
            ],
        )
        .await;
        let (oauth_transport, mut oauth_client) = tokio::io::duplex(64 * 1024);
        let _oauth_client_drain = tokio::spawn(async move {
            let mut sink = tokio::io::sink();
            let _copy_result = tokio::io::copy(&mut oauth_client, &mut sink).await;
        });
        let mut oauth_server = handler_server(Arc::clone(&runtime), Arc::clone(&manager));
        oauth_server.route_scope = McpRouteScope::protected_subset(
            "project-route",
            ["alpha", "oauth"],
            ["gateway"],
            false,
        );
        let oauth_running = rmcp::service::serve_directly::<RoleServer, _, _, std::io::Error, _>(
            oauth_server,
            oauth_transport,
            None,
        );
        let oauth_bound =
            transport_binding(&runtime, &manager, identity.clone(), usize::MAX, before).await;
        let oauth_response = oauth_running
            .service()
            .call_tool_response_impl(
                CallToolRequestParams::new("oauth-tool"),
                handler_context(
                    oauth_running.peer().clone(),
                    Some(identity.clone()),
                    Ok(oauth_bound),
                ),
            )
            .await
            .unwrap();
        assert_complete_not_found(oauth_response);
        assert_eq!(oauth_calls.load(Ordering::SeqCst), 0);
        assert_eq!(handler_calls.load(Ordering::SeqCst), 2);

        let legacy_oauth_response = tokio::time::timeout(
            Duration::from_secs(5),
            oauth_running.service().call_tool_response_impl(
                CallToolRequestParams::new("oauth-tool"),
                legacy_oauth_handler_context(oauth_running.peer().clone()),
            ),
        )
        .await
        .expect("Legacy OAuth subject Tool must reach its existing branch")
        .unwrap();
        let CallToolResponse::Complete(legacy_oauth_response) = legacy_oauth_response else {
            panic!("Legacy OAuth error must remain Complete")
        };
        let legacy_oauth_json = serde_json::to_string(&legacy_oauth_response).unwrap();
        assert!(
            legacy_oauth_json.contains("\"kind\":\"upstream_error\"")
                && legacy_oauth_json.contains("\"upstream\":\"oauth\"")
                && legacy_oauth_json.contains("upstream is not connected"),
            "{legacy_oauth_json}"
        );
        assert!(!legacy_oauth_json.contains("private oauth subject failure"));
        assert_eq!(oauth_calls.load(Ordering::SeqCst), 0);

        let mut handler_config = config(true);
        handler_config
            .upstream
            .retain(|upstream| upstream.name == "alpha");
        handler_config.loadouts[0].upstreams = vec!["alpha".into()];
        manager.try_seed_config(handler_config).await.unwrap();

        pool.insert_tool_routes_for_tests("alpha", vec![upstream_tool("danger", true)])
            .await;
        let destructive =
            transport_binding(&runtime, &manager, identity.clone(), usize::MAX, before).await;
        let destructive_response = running
            .service()
            .call_tool_response_impl(
                CallToolRequestParams::new("danger"),
                handler_context(
                    running.peer().clone(),
                    Some(identity.clone()),
                    Ok(destructive),
                ),
            )
            .await
            .unwrap();
        let CallToolResponse::Complete(destructive_response) = destructive_response else {
            panic!("Project destructive regular Tool must not elicit")
        };
        assert!(
            serde_json::to_string(&destructive_response)
                .unwrap()
                .contains("not_found")
        );
        assert_eq!(handler_calls.load(Ordering::SeqCst), 2);

        pool.insert_tool_routes_for_tests("alpha", vec![upstream_tool("nested/tool", false)])
            .await;
        let viewer =
            transport_binding(&runtime, &manager, identity.clone(), usize::MAX, before).await;
        let store = runtime.store().await.unwrap();
        store
            .execute_test_statement(
                "UPDATE project_memberships SET role='viewer' WHERE project_id='bootstrap-default';
                 UPDATE access_metadata SET global_revision=global_revision+1 WHERE singleton=1",
            )
            .await
            .unwrap();
        let viewer_response = running
            .service()
            .call_tool_response_impl(
                CallToolRequestParams::new("nested/tool"),
                handler_context(running.peer().clone(), Some(identity), Ok(viewer)),
            )
            .await
            .unwrap();
        assert_complete_not_found(viewer_response);
        assert_eq!(handler_calls.load(Ordering::SeqCst), 2);
    }
}
