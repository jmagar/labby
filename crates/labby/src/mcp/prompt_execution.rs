//! Project-bound exact regular Prompt execution authorization seam.

use labby_auth::VerifiedIdentity;
use labby_gateway::gateway::manager::{GatewayManager, PublishedPromptCallError};
use rmcp::model::{GetPromptRequestParams, GetPromptResult};
use std::time::SystemTime;
use thiserror::Error;

use crate::access::{AccessRuntime, Permission};
use crate::mcp::bound_access::{
    BoundAccessContext, TransportBoundAccessContext, bind_asset_use_access_context,
};

/// Server-owned inputs for one exact regular non-OAuth Prompt execution.
///
/// Deliberately non-`Clone`, non-`Debug`, and non-serializable. Callers must
/// construct it from authenticated identity and protected-route facts, never
/// from MCP params or `_meta` beyond the Prompt request itself.
pub(crate) struct PromptExecutionResolutionInput {
    identity: VerifiedIdentity,
    route_name: String,
    resource: String,
    project_id: String,
    request: GetPromptRequestParams,
}

impl PromptExecutionResolutionInput {
    pub(crate) fn new(
        identity: VerifiedIdentity,
        route_name: impl Into<String>,
        resource: impl Into<String>,
        project_id: impl Into<String>,
        request: GetPromptRequestParams,
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

#[derive(Debug, Error, PartialEq, Eq)]
pub(crate) enum PromptExecutionResolutionError {
    #[error("prompt execution target is unavailable")]
    Unavailable,
    #[error("prompt execution queue is unavailable")]
    QueueUnavailable,
    #[error("prompt execution failed")]
    Upstream,
    #[error("prompt execution timed out")]
    Timeout,
    #[error("prompt execution was cancelled")]
    Cancelled,
}

struct ExactPromptTarget<'a> {
    upstream: &'a str,
    native_name: &'a str,
    pool_generation: labby_gateway::gateway::manager::PoolPublicationGeneration,
    prompt_generation: labby_gateway::upstream::pool::PromptCatalogGeneration,
}

fn resolve_exact_target<'a>(
    context: &'a BoundAccessContext,
    wire_name: &str,
) -> Option<ExactPromptTarget<'a>> {
    let access = context.catalog().access();
    if access.permission != Permission::AssetUse {
        return None;
    }
    let prompts = context.catalog().catalog().prompts();
    let published = prompts.unique_route_for_wire_name(wire_name)?;
    context
        .allows_upstream_prompt_pair(
            published.upstream_name.as_ref(),
            published.native_name.as_ref(),
        )
        .then_some(ExactPromptTarget {
            upstream: published.upstream_name.as_ref(),
            native_name: published.native_name.as_ref(),
            pool_generation: prompts.pool_publication_generation(),
            prompt_generation: prompts.prompt_catalog_generation(),
        })
}

/// Authorize and execute one exact regular non-OAuth Prompt against a bounded
/// common interval. MCP handlers reach this primitive only through the
/// transport-bound wrapper below.
pub(crate) async fn execute_exact_project_prompt(
    runtime: &AccessRuntime,
    manager: &GatewayManager,
    input: PromptExecutionResolutionInput,
) -> Result<GetPromptResult, PromptExecutionResolutionError> {
    let wire_name = input.request.name.clone();
    let first = bind_asset_use_access_context(
        runtime,
        manager,
        input.identity.clone(),
        &input.route_name,
        &input.resource,
        &input.project_id,
    )
    .await
    .map_err(|_| PromptExecutionResolutionError::Unavailable)?;
    let target = resolve_exact_target(&first, &wire_name)
        .ok_or(PromptExecutionResolutionError::Unavailable)?;
    let upstream = target.upstream.to_string();
    let native_name = target.native_name.to_string();
    let pool_generation = target.pool_generation;
    let prompt_generation = target.prompt_generation;
    let mut outbound = input.request;
    outbound.name.clone_from(&native_name);
    let result = manager
        .execute_published_prompt_exact(
            pool_generation,
            prompt_generation,
            &upstream,
            &native_name,
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
    .map_err(|_| PromptExecutionResolutionError::Unavailable)?;
    if !first.same_publication_as(&second) || resolve_exact_target(&second, &wire_name).is_none() {
        return Err(PromptExecutionResolutionError::Unavailable);
    }
    result.map_err(map_published_prompt_error)
}

fn map_published_prompt_error(error: PublishedPromptCallError) -> PromptExecutionResolutionError {
    match error {
        PublishedPromptCallError::Unavailable => PromptExecutionResolutionError::Unavailable,
        PublishedPromptCallError::QueueUnavailable => {
            PromptExecutionResolutionError::QueueUnavailable
        }
        PublishedPromptCallError::Upstream => PromptExecutionResolutionError::Upstream,
        PublishedPromptCallError::Timeout => PromptExecutionResolutionError::Timeout,
        PublishedPromptCallError::Cancelled => PromptExecutionResolutionError::Cancelled,
    }
}

#[cfg(test)]
mod error_mapping_tests {
    use super::*;

    #[test]
    fn published_cancellation_remains_cancelled() {
        assert_eq!(
            map_published_prompt_error(PublishedPromptCallError::Cancelled),
            PromptExecutionResolutionError::Cancelled
        );
    }
}

/// Execute from one middleware-owned protected-transport binding.
///
/// The immutable request-owned transport binding carries the exact token
/// instance. Its expiry and independently derived `VerifiedIdentity` binding
/// are checked on both sides of the Wave 43 resolver. Route, resource, and
/// Project facts come only from that middleware binding; no MCP parameter can
/// select them.
pub(crate) async fn execute_transport_bound_project_prompt(
    runtime: &AccessRuntime,
    manager: &GatewayManager,
    transport: &TransportBoundAccessContext,
    identity: &VerifiedIdentity,
    request: GetPromptRequestParams,
) -> Result<GetPromptResult, PromptExecutionResolutionError> {
    execute_transport_bound_project_prompt_with_clock(
        runtime,
        manager,
        transport,
        identity,
        request,
        SystemTime::now,
    )
    .await
}

async fn execute_transport_bound_project_prompt_with_clock(
    runtime: &AccessRuntime,
    manager: &GatewayManager,
    transport: &TransportBoundAccessContext,
    identity: &VerifiedIdentity,
    request: GetPromptRequestParams,
    mut now: impl FnMut() -> SystemTime,
) -> Result<GetPromptResult, PromptExecutionResolutionError> {
    transport
        .validate_not_expired(now())
        .map_err(|_| PromptExecutionResolutionError::Unavailable)?;
    if !transport.matches_identity(identity) {
        return Err(PromptExecutionResolutionError::Unavailable);
    }
    let route = transport.core().route();
    let result = execute_exact_project_prompt(
        runtime,
        manager,
        PromptExecutionResolutionInput::new(
            identity.clone(),
            route.route_name(),
            route.resource(),
            route.project_id(),
            request,
        ),
    )
    .await;
    transport
        .validate_not_expired(now())
        .map_err(|_| PromptExecutionResolutionError::Unavailable)?;
    if !transport.matches_identity(identity) {
        return Err(PromptExecutionResolutionError::Unavailable);
    }
    result
}

#[cfg(all(test, feature = "proxy-testkit"))]
mod tests {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::{Duration, UNIX_EPOCH};

    use labby_auth::{Authenticator, VerifiedIdentity};
    use labby_gateway::gateway::config_store::FsGatewayConfigStore;
    use labby_gateway::gateway::manager::{GatewayManager, GatewayRuntimeHandle};
    use labby_gateway::upstream::pool::UpstreamPool;
    use labby_runtime::gateway_config::{
        GatewayConfig, GatewayLoadoutConfig, ProtectedGatewaySubsetTarget, ProtectedMcpRouteConfig,
        ProtectedMcpRouteTarget, UpstreamConfig, UpstreamOauthConfig, UpstreamOauthMode,
        UpstreamOauthRegistration,
    };
    use rmcp::model::{
        ClientCapabilities, ErrorData, GetPromptRequestParams, GetPromptResponse, GetPromptResult,
        Implementation, ListPromptsResult, Prompt, PromptMessage, ProtocolVersion,
        RequestMetaObject, Role,
    };
    use rmcp::service::RequestContext;
    use rmcp::{RoleServer, ServerHandler};
    use tokio::sync::Notify;

    use super::{
        PromptExecutionResolutionError, PromptExecutionResolutionInput,
        execute_exact_project_prompt, execute_transport_bound_project_prompt_with_clock,
    };
    use crate::access::{
        AccessRuntime, AssignProjectLoadoutInput, BootstrapOwnerInput, Permission,
        project_runtime_mcp_catalog_context,
    };
    use crate::mcp::bound_access::{
        TransportBoundAccessContext, attach_project_access_observation, bind_access_context,
        validate_transport_credential_binding,
    };
    use crate::mcp::logging::{LoggingLevel, logging_level_rank};
    use crate::mcp::route_scope::McpRouteScope;
    use crate::mcp::server::LabMcpServer;

    #[derive(Clone)]
    struct EchoPromptServer {
        calls: Arc<AtomicUsize>,
    }

    impl ServerHandler for EchoPromptServer {
        async fn list_prompts(
            &self,
            _request: Option<rmcp::model::PaginatedRequestParams>,
            _context: RequestContext<RoleServer>,
        ) -> Result<ListPromptsResult, ErrorData> {
            Ok(ListPromptsResult::with_all_items(vec![Prompt::new(
                "owner/nested/name",
                Some("exact"),
                None,
            )]))
        }

        async fn get_prompt(
            &self,
            request: GetPromptRequestParams,
            _context: RequestContext<RoleServer>,
        ) -> Result<GetPromptResponse, ErrorData> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            let argument = request
                .arguments
                .as_ref()
                .and_then(|arguments| arguments.get("target"))
                .and_then(serde_json::Value::as_str)
                .unwrap_or("missing");
            Ok(GetPromptResult::new(vec![PromptMessage::new_text(
                Role::User,
                format!("{}:{argument}", request.name),
            )])
            .into())
        }
    }

    #[derive(Clone)]
    struct DelayedPromptServer {
        started: Arc<Notify>,
        release: Arc<Notify>,
        fail: bool,
    }

    #[derive(Clone)]
    struct FailingPromptServer;

    impl ServerHandler for FailingPromptServer {
        async fn get_prompt(
            &self,
            _request: GetPromptRequestParams,
            _context: RequestContext<RoleServer>,
        ) -> Result<GetPromptResponse, ErrorData> {
            Err(ErrorData::internal_error(
                "private upstream prompt failure",
                None,
            ))
        }
    }

    impl ServerHandler for DelayedPromptServer {
        async fn get_prompt(
            &self,
            request: GetPromptRequestParams,
            _context: RequestContext<RoleServer>,
        ) -> Result<GetPromptResponse, ErrorData> {
            self.started.notify_one();
            self.release.notified().await;
            if self.fail {
                return Err(ErrorData::internal_error("private delayed failure", None));
            }
            Ok(
                GetPromptResult::new(vec![PromptMessage::new_text(Role::User, request.name)])
                    .into(),
            )
        }
    }

    struct Fixture {
        _directory: tempfile::TempDir,
        runtime: Arc<AccessRuntime>,
        gateway_runtime: GatewayRuntimeHandle,
        manager: Arc<GatewayManager>,
        identity: VerifiedIdentity,
        pool: Arc<UpstreamPool>,
        calls: Arc<AtomicUsize>,
    }

    fn gateway_config(expose_prompts: bool) -> GatewayConfig {
        GatewayConfig {
            upstream: ["alpha"]
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
                    env: Default::default(),
                    proxy_resources: false,
                    proxy_prompts: true,
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
                upstreams: vec!["alpha".into()],
                expose_prompts,
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
            ..GatewayConfig::default()
        }
    }

    fn pooled_gateway_config() -> GatewayConfig {
        let mut config = gateway_config(true);
        config.upstream[0]
            .env
            .insert("MCP_UPSTREAM_RELAY_MODE".into(), "pooled".into());
        config
    }

    fn oauth_gateway_config() -> (GatewayConfig, UpstreamConfig) {
        let mut config = gateway_config(true);
        let mut oauth = config.upstream[0].clone();
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
        config.upstream.push(oauth.clone());
        config.loadouts[0].upstreams.push("oauth".into());
        (config, oauth)
    }

    async fn fixture() -> Fixture {
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

        let calls = Arc::new(AtomicUsize::new(0));
        let pool = Arc::new(UpstreamPool::new());
        pool.install_prompt_server_for_tests(
            "alpha",
            EchoPromptServer {
                calls: Arc::clone(&calls),
            },
        )
        .await;
        pool.insert_prompt_routes_for_tests(
            "alpha",
            vec![Prompt::new("owner/nested/name", Some("exact"), None)],
        )
        .await;
        let gateway_runtime = GatewayRuntimeHandle::default();
        gateway_runtime.swap(Some(Arc::clone(&pool))).await;
        let gateway_path = directory.path().join("prompt-execution.toml");
        let manager = Arc::new(GatewayManager::with_store(
            gateway_path.clone(),
            gateway_runtime.clone(),
            Arc::new(FsGatewayConfigStore::new(gateway_path)),
        ));
        manager.try_seed_config(gateway_config(true)).await.unwrap();
        Fixture {
            _directory: directory,
            runtime,
            gateway_runtime,
            manager,
            identity,
            pool,
            calls,
        }
    }

    fn input(identity: VerifiedIdentity) -> PromptExecutionResolutionInput {
        let arguments = serde_json::Map::from_iter([(
            "target".to_string(),
            serde_json::Value::String("exact-value".to_string()),
        )]);
        PromptExecutionResolutionInput::new(
            identity,
            "project-route",
            "https://mcp.example.com/project",
            "bootstrap-default",
            GetPromptRequestParams::new("alpha/owner/nested/name").with_arguments(arguments),
        )
    }

    async fn transport_binding(
        fixture: &Fixture,
        expires_at: usize,
        now: std::time::SystemTime,
    ) -> TransportBoundAccessContext {
        let core = bind_access_context(
            &fixture.runtime,
            &fixture.manager,
            fixture.identity.clone(),
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

    fn handler_server(fixture: &Fixture) -> LabMcpServer {
        LabMcpServer {
            registry: Arc::new(crate::registry::build_default_registry()),
            access_runtime: Arc::clone(&fixture.runtime),
            file_stash_runtime: Arc::new(crate::file_stash::FileStashRuntime::blocked()),
            gateway_manager: Some(Arc::clone(&fixture.manager)),
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
        }
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
        attach_project_access_observation(&mut parts.extensions, binding);
        context.extensions.insert(parts);
        context
    }

    fn legacy_oauth_context(peer: rmcp::service::Peer<RoleServer>) -> RequestContext<RoleServer> {
        let mut context = RequestContext::new(rmcp::model::NumberOrString::Number(1), peer);
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

    #[tokio::test]
    async fn transport_binding_expiry_after_rpc_discards_result_deterministically() {
        let fixture = fixture().await;
        let before = UNIX_EPOCH + Duration::from_secs(100);
        let after = UNIX_EPOCH + Duration::from_secs(102);
        let transport = transport_binding(&fixture, 101, before).await;
        let mut times = [before, after].into_iter();
        let PromptExecutionResolutionInput { request, .. } = input(fixture.identity.clone());

        assert_eq!(
            execute_transport_bound_project_prompt_with_clock(
                &fixture.runtime,
                &fixture.manager,
                &transport,
                &fixture.identity,
                request,
                || times.next().unwrap(),
            )
            .await,
            Err(PromptExecutionResolutionError::Unavailable)
        );
        assert_eq!(fixture.calls.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn handler_bound_regular_executes_and_bound_nonregular_never_falls_back() {
        let fixture = fixture().await;
        let now = UNIX_EPOCH + Duration::from_secs(100);
        let (transport, _client_transport) = tokio::io::duplex(64 * 1024);
        let running = rmcp::service::serve_directly::<RoleServer, _, _, std::io::Error, _>(
            handler_server(&fixture),
            transport,
            None,
        );
        let bound = transport_binding(&fixture, usize::MAX, now).await;
        let mut request = GetPromptRequestParams::new("alpha/owner/nested/name").with_arguments(
            serde_json::Map::from_iter([(
                "target".into(),
                serde_json::Value::String("handler".into()),
            )]),
        );
        request.meta = Some(RequestMetaObject::with_client_context(
            ProtocolVersion::V_2026_07_28,
            Implementation::new("relay-capable-client", "1.0.0"),
            ClientCapabilities::default(),
        ));
        let response = running
            .service()
            .get_prompt_impl(
                request,
                handler_context(
                    running.peer().clone(),
                    Some(fixture.identity.clone()),
                    Ok(bound),
                ),
            )
            .await
            .expect("bound exact regular prompt");
        let GetPromptResponse::Complete(response) = response else {
            panic!("unexpected prompt response")
        };
        assert_eq!(
            response.messages,
            vec![PromptMessage::new_text(
                Role::User,
                "owner/nested/name:handler"
            )]
        );
        assert_eq!(fixture.calls.load(Ordering::SeqCst), 1);

        for name in ["service-discover", "alpha/missing"] {
            let bound = transport_binding(&fixture, usize::MAX, now).await;
            let error = running
                .service()
                .get_prompt_impl(
                    GetPromptRequestParams::new(name),
                    handler_context(
                        running.peer().clone(),
                        Some(fixture.identity.clone()),
                        Ok(bound),
                    ),
                )
                .await
                .expect_err("Project-bound nonregular prompt is terminal");
            assert_eq!(error.data.as_ref().unwrap()["kind"], "upstream_error");
        }
        assert_eq!(fixture.calls.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn handler_legacy_regular_path_preserves_existing_output() {
        let fixture = fixture().await;
        fixture
            .manager
            .try_seed_config(pooled_gateway_config())
            .await
            .unwrap();
        fixture.pool.list_upstream_prompts_allowed(&[], None).await;
        let (transport, _client_transport) = tokio::io::duplex(64 * 1024);
        let running = rmcp::service::serve_directly::<RoleServer, _, _, std::io::Error, _>(
            handler_server(&fixture),
            transport,
            None,
        );
        let context = RequestContext::new(
            rmcp::model::NumberOrString::Number(1),
            running.peer().clone(),
        );
        let response = running
            .service()
            .get_prompt_impl(
                GetPromptRequestParams::new("alpha/owner/nested/name").with_arguments(
                    serde_json::Map::from_iter([(
                        "target".into(),
                        serde_json::Value::String("legacy".into()),
                    )]),
                ),
                context,
            )
            .await
            .expect("legacy regular path remains mounted");
        let GetPromptResponse::Complete(response) = response else {
            panic!("unexpected prompt response")
        };
        assert_eq!(
            response.messages,
            vec![PromptMessage::new_text(
                Role::User,
                "owner/nested/name:legacy"
            )]
        );
        assert_eq!(fixture.calls.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn handler_bound_oauth_name_is_terminal_while_legacy_reaches_oauth_branch() {
        let fixture = fixture().await;
        let (config, oauth) = oauth_gateway_config();
        fixture.manager.try_seed_config(config).await.unwrap();
        fixture
            .pool
            .install_test_subject_server_for_upstream(
                &oauth,
                "reader",
                EchoPromptServer {
                    calls: Arc::clone(&fixture.calls),
                },
            )
            .await;
        let (transport, _client_transport) = tokio::io::duplex(64 * 1024);
        let running = rmcp::service::serve_directly::<RoleServer, _, _, std::io::Error, _>(
            handler_server(&fixture),
            transport,
            None,
        );
        let now = UNIX_EPOCH + Duration::from_secs(100);
        let bound = transport_binding(&fixture, usize::MAX, now).await;
        let bound_error = running
            .service()
            .get_prompt_impl(
                GetPromptRequestParams::new("oauth/owner/nested/name"),
                handler_context(running.peer().clone(), Some(fixture.identity), Ok(bound)),
            )
            .await
            .expect_err("Bound OAuth-shaped name must not fall through");
        assert_eq!(
            bound_error.data.as_ref().unwrap()["message"],
            "Prompt `oauth/owner/nested/name` is unavailable."
        );

        let legacy_error = running
            .service()
            .get_prompt_impl(
                GetPromptRequestParams::new("oauth/owner/nested/name"),
                legacy_oauth_context(running.peer().clone()),
            )
            .await
            .expect_err("legacy OAuth relay fixture intentionally cannot connect");
        assert!(
            legacy_error.data.as_ref().unwrap()["message"]
                .as_str()
                .unwrap()
                .contains("Upstream `oauth` failed")
        );
        assert_eq!(fixture.calls.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn handler_rejects_unavailable_missing_or_mismatched_identity_before_rpc() {
        let fixture = fixture().await;
        let now = UNIX_EPOCH + Duration::from_secs(100);
        let (transport, _client_transport) = tokio::io::duplex(64 * 1024);
        let running = rmcp::service::serve_directly::<RoleServer, _, _, std::io::Error, _>(
            handler_server(&fixture),
            transport,
            None,
        );
        let other = VerifiedIdentity::local_credential_with_issuer(
            Authenticator::StaticBearer,
            "other-issuer",
            "other-credential",
        )
        .unwrap();
        for identity in [None, Some(other)] {
            let bound = transport_binding(&fixture, usize::MAX, now).await;
            let error = running
                .service()
                .get_prompt_impl(
                    GetPromptRequestParams::new("alpha/owner/nested/name"),
                    handler_context(running.peer().clone(), identity, Ok(bound)),
                )
                .await
                .expect_err("invalid transport binding must fail closed");
            assert_eq!(error.data.as_ref().unwrap()["kind"], "upstream_error");
        }
        let expired = transport_binding(&fixture, 101, now).await;
        let error = running
            .service()
            .get_prompt_impl(
                GetPromptRequestParams::new("alpha/owner/nested/name"),
                handler_context(
                    running.peer().clone(),
                    Some(fixture.identity.clone()),
                    Ok(expired),
                ),
            )
            .await
            .expect_err("expired transport binding must fail before RPC");
        assert_eq!(error.data.as_ref().unwrap()["kind"], "upstream_error");
        let error = running
            .service()
            .get_prompt_impl(
                GetPromptRequestParams::new("service-discover"),
                handler_context(
                    running.peer().clone(),
                    Some(fixture.identity),
                    Err(crate::mcp::bound_access::BoundAccessContextError::Unavailable),
                ),
            )
            .await
            .expect_err("explicit unavailable observation is terminal before builtin");
        assert_eq!(error.data.as_ref().unwrap()["kind"], "upstream_error");
        assert_eq!(fixture.calls.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn handler_bound_upstream_failure_is_redacted_and_existing_shaped() {
        let fixture = fixture().await;
        fixture
            .pool
            .install_prompt_server_for_tests("alpha", FailingPromptServer)
            .await;
        fixture
            .pool
            .insert_prompt_routes_for_tests(
                "alpha",
                vec![Prompt::new("owner/nested/name", Some("exact"), None)],
            )
            .await;
        let now = UNIX_EPOCH + Duration::from_secs(100);
        let bound = transport_binding(&fixture, usize::MAX, now).await;
        let (transport, _client_transport) = tokio::io::duplex(64 * 1024);
        let running = rmcp::service::serve_directly::<RoleServer, _, _, std::io::Error, _>(
            handler_server(&fixture),
            transport,
            None,
        );
        let error = running
            .service()
            .get_prompt_impl(
                GetPromptRequestParams::new("alpha/owner/nested/name"),
                handler_context(running.peer().clone(), Some(fixture.identity), Ok(bound)),
            )
            .await
            .expect_err("upstream application error");
        let serialized = serde_json::to_string(&error).unwrap();
        assert_eq!(error.data.as_ref().unwrap()["kind"], "upstream_error");
        assert!(!serialized.contains("private upstream prompt failure"));
    }

    #[tokio::test]
    async fn asset_use_executes_exact_native_prompt_and_preserves_arguments() {
        let fixture = fixture().await;
        let result = execute_exact_project_prompt(
            &fixture.runtime,
            &fixture.manager,
            input(fixture.identity.clone()),
        )
        .await
        .expect("owner has AssetUse");

        assert_eq!(fixture.calls.load(Ordering::SeqCst), 1);
        assert_eq!(
            result.messages,
            vec![PromptMessage::new_text(
                Role::User,
                "owner/nested/name:exact-value"
            )]
        );
    }

    #[tokio::test]
    async fn viewer_and_unknown_wire_target_fail_before_rpc() {
        let fixture = fixture().await;
        fixture
            .runtime
            .store()
            .await
            .unwrap()
            .execute_test_statement(
                "UPDATE project_memberships SET role='viewer' WHERE project_id='bootstrap-default'",
            )
            .await
            .unwrap();
        assert_eq!(
            execute_exact_project_prompt(
                &fixture.runtime,
                &fixture.manager,
                input(fixture.identity.clone()),
            )
            .await,
            Err(PromptExecutionResolutionError::Unavailable)
        );
        assert_eq!(fixture.calls.load(Ordering::SeqCst), 0);

        fixture
            .runtime
            .store()
            .await
            .unwrap()
            .execute_test_statement(
                "UPDATE project_memberships SET role='owner' WHERE project_id='bootstrap-default'",
            )
            .await
            .unwrap();
        let unknown = PromptExecutionResolutionInput::new(
            fixture.identity,
            "project-route",
            "https://mcp.example.com/project",
            "bootstrap-default",
            GetPromptRequestParams::new("alpha/unknown"),
        );
        assert_eq!(
            execute_exact_project_prompt(&fixture.runtime, &fixture.manager, unknown).await,
            Err(PromptExecutionResolutionError::Unavailable)
        );
        assert_eq!(fixture.calls.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn route_exclusion_fails_before_rpc() {
        let fixture = fixture().await;
        fixture
            .manager
            .try_seed_config(gateway_config(false))
            .await
            .unwrap();

        assert_eq!(
            execute_exact_project_prompt(
                &fixture.runtime,
                &fixture.manager,
                input(fixture.identity),
            )
            .await,
            Err(PromptExecutionResolutionError::Unavailable)
        );
        assert_eq!(fixture.calls.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn pool_publication_aba_discards_delayed_success_and_failure() {
        for fail in [false, true] {
            let fixture = fixture().await;
            let started = Arc::new(Notify::new());
            let release = Arc::new(Notify::new());
            fixture
                .pool
                .install_prompt_server_for_tests(
                    "alpha",
                    DelayedPromptServer {
                        started: Arc::clone(&started),
                        release: Arc::clone(&release),
                        fail,
                    },
                )
                .await;
            fixture
                .pool
                .insert_prompt_routes_for_tests(
                    "alpha",
                    vec![Prompt::new("owner/nested/name", None::<String>, None)],
                )
                .await;
            fixture
                .pool
                .set_prompt_last_error_for_tests("alpha", Some("replacement sentinel".into()))
                .await;
            let runtime = Arc::clone(&fixture.runtime);
            let manager = Arc::clone(&fixture.manager);
            let request = input(fixture.identity.clone());
            let task = tokio::spawn(async move {
                execute_exact_project_prompt(&runtime, &manager, request).await
            });
            started.notified().await;
            fixture
                .gateway_runtime
                .swap(Some(Arc::new(UpstreamPool::new())))
                .await;
            fixture
                .gateway_runtime
                .swap(Some(Arc::clone(&fixture.pool)))
                .await;
            release.notify_one();
            assert_eq!(
                task.await.unwrap(),
                Err(PromptExecutionResolutionError::Unavailable)
            );
            assert_eq!(
                fixture
                    .pool
                    .prompt_last_error_for_tests("alpha")
                    .await
                    .as_deref(),
                Some("replacement sentinel")
            );
        }
    }

    #[tokio::test]
    async fn access_revocation_during_rpc_discards_result_and_cancellation_returns_no_result() {
        let fixture = fixture().await;
        let started = Arc::new(Notify::new());
        let release = Arc::new(Notify::new());
        fixture
            .pool
            .install_prompt_server_for_tests(
                "alpha",
                DelayedPromptServer {
                    started: Arc::clone(&started),
                    release: Arc::clone(&release),
                    fail: false,
                },
            )
            .await;
        fixture
            .pool
            .insert_prompt_routes_for_tests(
                "alpha",
                vec![Prompt::new("owner/nested/name", None::<String>, None)],
            )
            .await;
        let runtime = Arc::clone(&fixture.runtime);
        let manager = Arc::clone(&fixture.manager);
        let request = input(fixture.identity.clone());
        let task =
            tokio::spawn(
                async move { execute_exact_project_prompt(&runtime, &manager, request).await },
            );
        started.notified().await;
        fixture
            .runtime
            .store()
            .await
            .unwrap()
            .execute_test_statement(
                "UPDATE project_memberships SET role='viewer' WHERE project_id='bootstrap-default'",
            )
            .await
            .unwrap();
        release.notify_one();
        assert_eq!(
            task.await.unwrap(),
            Err(PromptExecutionResolutionError::Unavailable)
        );

        fixture
            .runtime
            .store()
            .await
            .unwrap()
            .execute_test_statement(
                "UPDATE project_memberships SET role='owner' WHERE project_id='bootstrap-default'",
            )
            .await
            .unwrap();
        let started = Arc::new(Notify::new());
        let release = Arc::new(Notify::new());
        fixture
            .pool
            .install_prompt_server_for_tests(
                "alpha",
                DelayedPromptServer {
                    started: Arc::clone(&started),
                    release: Arc::clone(&release),
                    fail: false,
                },
            )
            .await;
        fixture
            .pool
            .insert_prompt_routes_for_tests(
                "alpha",
                vec![Prompt::new("owner/nested/name", None::<String>, None)],
            )
            .await;
        fixture
            .pool
            .set_prompt_last_error_for_tests("alpha", Some("cancellation sentinel".into()))
            .await;
        let runtime = Arc::clone(&fixture.runtime);
        let manager = Arc::clone(&fixture.manager);
        let task = tokio::spawn(async move {
            execute_exact_project_prompt(&runtime, &manager, input(fixture.identity)).await
        });
        started.notified().await;
        let gateway_runtime = fixture.gateway_runtime.clone();
        let swapping = tokio::spawn(async move {
            gateway_runtime
                .swap(Some(Arc::new(UpstreamPool::new())))
                .await;
        });
        swapping.await.unwrap();
        task.abort();
        release.notify_one();
        assert!(task.await.unwrap_err().is_cancelled());
        assert_eq!(
            fixture
                .pool
                .prompt_last_error_for_tests("alpha")
                .await
                .as_deref(),
            Some("cancellation sentinel")
        );
    }

    #[tokio::test]
    async fn access_route_and_prompt_aba_each_discard_delayed_result() {
        for mutation in ["access", "route", "prompt"] {
            let fixture = fixture().await;
            let initial_access_revision = project_runtime_mcp_catalog_context(
                &fixture.runtime,
                &fixture.manager,
                fixture.identity.clone(),
                "bootstrap-default",
                Permission::AssetUse,
            )
            .await
            .unwrap()
            .access()
            .global_revision;
            let started = Arc::new(Notify::new());
            let release = Arc::new(Notify::new());
            fixture
                .pool
                .install_prompt_server_for_tests(
                    "alpha",
                    DelayedPromptServer {
                        started: Arc::clone(&started),
                        release: Arc::clone(&release),
                        fail: false,
                    },
                )
                .await;
            fixture
                .pool
                .insert_prompt_routes_for_tests(
                    "alpha",
                    vec![Prompt::new("owner/nested/name", None::<String>, None)],
                )
                .await;
            let runtime = Arc::clone(&fixture.runtime);
            let manager = Arc::clone(&fixture.manager);
            let request = input(fixture.identity.clone());
            let task = tokio::spawn(async move {
                execute_exact_project_prompt(&runtime, &manager, request).await
            });
            started.notified().await;
            match mutation {
                "access" => {
                    let store = fixture.runtime.store().await.unwrap();
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
                    let current_revision = project_runtime_mcp_catalog_context(
                        &fixture.runtime,
                        &fixture.manager,
                        fixture.identity.clone(),
                        "bootstrap-default",
                        Permission::AssetUse,
                    )
                    .await
                    .unwrap()
                    .access()
                    .global_revision;
                    assert_ne!(current_revision, initial_access_revision);
                }
                "route" => {
                    fixture
                        .manager
                        .try_seed_config(gateway_config(false))
                        .await
                        .unwrap();
                    fixture
                        .manager
                        .try_seed_config(gateway_config(true))
                        .await
                        .unwrap();
                }
                "prompt" => {
                    fixture
                        .pool
                        .insert_prompt_routes_for_tests(
                            "alpha",
                            vec![Prompt::new("other", None::<String>, None)],
                        )
                        .await;
                    fixture
                        .pool
                        .insert_prompt_routes_for_tests(
                            "alpha",
                            vec![Prompt::new("owner/nested/name", None::<String>, None)],
                        )
                        .await;
                }
                _ => unreachable!(),
            }
            release.notify_one();
            assert_eq!(
                task.await.unwrap(),
                Err(PromptExecutionResolutionError::Unavailable),
                "{mutation} ABA must not expose the delayed result"
            );
        }
    }
}
