//! Project-bound exact regular Resource read authorization and execution seam.

use std::time::SystemTime;

use labby_auth::VerifiedIdentity;
use labby_gateway::gateway::manager::{GatewayManager, PublishedResourceReadError};
use rmcp::model::{ReadResourceRequestParams, ReadResourceResult};
use thiserror::Error;

use crate::access::{AccessRuntime, Permission};
use crate::mcp::bound_access::{
    BoundAccessContext, TransportBoundAccessContext, bind_asset_use_access_context,
};

/// Server-owned inputs for one exact regular non-OAuth Resource read.
///
/// Deliberately non-`Clone`, non-`Debug`, and non-serializable. The identity
/// and protected-route facts must be trusted server inputs. This inner seam
/// does not itself prove a transport token instance or expiry; the mounted
/// handler reaches it only through the transport-bound wrapper.
pub(crate) struct ResourceReadResolutionInput {
    identity: VerifiedIdentity,
    route_name: String,
    resource: String,
    project_id: String,
    request: ReadResourceRequestParams,
}

impl ResourceReadResolutionInput {
    pub(crate) fn new(
        identity: VerifiedIdentity,
        route_name: impl Into<String>,
        resource: impl Into<String>,
        project_id: impl Into<String>,
        request: ReadResourceRequestParams,
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
pub(crate) enum ResourceReadResolutionError {
    #[error("resource read target is unavailable")]
    Unavailable,
    #[error("resource read queue is unavailable")]
    QueueUnavailable,
    #[error("resource read failed")]
    Upstream,
    #[error("resource read timed out")]
    Timeout,
    #[error("resource read was cancelled")]
    Cancelled,
    #[error("resource response is too large")]
    TooLarge,
}

struct ExactResourceTarget<'a> {
    upstream: &'a str,
    native_uri: &'a str,
    pool_generation: labby_gateway::gateway::manager::PoolPublicationGeneration,
    resource_generation: labby_gateway::upstream::pool::ResourceCatalogGeneration,
}

fn resolve_exact_target<'a>(
    context: &'a BoundAccessContext,
    wire_uri: &str,
) -> Option<ExactResourceTarget<'a>> {
    if context.catalog().access().permission != Permission::AssetUse {
        return None;
    }
    let resources = context.catalog().catalog().resources();
    let published = resources.unique_route_for_wire_uri(wire_uri)?;
    context
        .allows_upstream_resource_pair(
            published.upstream_name.as_ref(),
            published.native_uri.as_ref(),
        )
        .then_some(ExactResourceTarget {
            upstream: published.upstream_name.as_ref(),
            native_uri: published.native_uri.as_ref(),
            pool_generation: resources.pool_publication_generation(),
            resource_generation: resources.resource_catalog_generation(),
        })
}

/// Authorize and read one exact regular non-OAuth Resource against a bounded
/// Access/manager common interval. MCP handlers reach this primitive only
/// through the transport-bound wrapper below.
pub(crate) async fn read_exact_project_resource(
    runtime: &AccessRuntime,
    manager: &GatewayManager,
    input: ResourceReadResolutionInput,
) -> Result<ReadResourceResult, ResourceReadResolutionError> {
    let wire_uri = input.request.uri.clone();
    let first = bind_asset_use_access_context(
        runtime,
        manager,
        input.identity.clone(),
        &input.route_name,
        &input.resource,
        &input.project_id,
    )
    .await
    .map_err(|_| ResourceReadResolutionError::Unavailable)?;
    let target =
        resolve_exact_target(&first, &wire_uri).ok_or(ResourceReadResolutionError::Unavailable)?;
    let upstream = target.upstream.to_string();
    let native_uri = target.native_uri.to_string();
    let pool_generation = target.pool_generation;
    let resource_generation = target.resource_generation;
    let mut outbound = input.request;
    outbound.uri.clone_from(&native_uri);
    let result = manager
        .execute_published_resource_exact(
            pool_generation,
            resource_generation,
            &upstream,
            &native_uri,
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
    .map_err(|_| ResourceReadResolutionError::Unavailable)?;
    let Some(second_target) = resolve_exact_target(&second, &wire_uri) else {
        return Err(ResourceReadResolutionError::Unavailable);
    };
    if !first.same_publication_as(&second)
        || second_target.upstream != upstream
        || second_target.native_uri != native_uri
        || second_target.pool_generation != pool_generation
        || second_target.resource_generation != resource_generation
    {
        return Err(ResourceReadResolutionError::Unavailable);
    }
    result.map_err(map_published_resource_error)
}

fn map_published_resource_error(error: PublishedResourceReadError) -> ResourceReadResolutionError {
    match error {
        PublishedResourceReadError::Unavailable => ResourceReadResolutionError::Unavailable,
        PublishedResourceReadError::QueueUnavailable => {
            ResourceReadResolutionError::QueueUnavailable
        }
        PublishedResourceReadError::Upstream => ResourceReadResolutionError::Upstream,
        PublishedResourceReadError::Timeout => ResourceReadResolutionError::Timeout,
        PublishedResourceReadError::Cancelled => ResourceReadResolutionError::Cancelled,
        PublishedResourceReadError::TooLarge => ResourceReadResolutionError::TooLarge,
    }
}

#[cfg(test)]
mod error_mapping_tests {
    use super::*;

    #[test]
    fn published_cancellation_remains_cancelled() {
        assert_eq!(
            map_published_resource_error(PublishedResourceReadError::Cancelled),
            ResourceReadResolutionError::Cancelled
        );
    }
}

/// Execute from one middleware-owned protected-transport binding.
///
/// The immutable request-owned transport binding carries the exact token
/// instance. Its expiry and independently derived `VerifiedIdentity` binding
/// are checked on both sides of the Wave 46 resolver.
pub(crate) async fn read_transport_bound_project_resource(
    runtime: &AccessRuntime,
    manager: &GatewayManager,
    transport: &TransportBoundAccessContext,
    identity: &VerifiedIdentity,
    request: ReadResourceRequestParams,
) -> Result<ReadResourceResult, ResourceReadResolutionError> {
    read_transport_bound_project_resource_with_clock(
        runtime,
        manager,
        transport,
        identity,
        request,
        SystemTime::now,
    )
    .await
}

async fn read_transport_bound_project_resource_with_clock(
    runtime: &AccessRuntime,
    manager: &GatewayManager,
    transport: &TransportBoundAccessContext,
    identity: &VerifiedIdentity,
    request: ReadResourceRequestParams,
    mut now: impl FnMut() -> SystemTime,
) -> Result<ReadResourceResult, ResourceReadResolutionError> {
    transport
        .validate_not_expired(now())
        .map_err(|_| ResourceReadResolutionError::Unavailable)?;
    if !transport.matches_identity(identity) {
        return Err(ResourceReadResolutionError::Unavailable);
    }
    let route = transport.core().route();
    let result = read_exact_project_resource(
        runtime,
        manager,
        ResourceReadResolutionInput::new(
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
        .map_err(|_| ResourceReadResolutionError::Unavailable)?;
    if !transport.matches_identity(identity) {
        return Err(ResourceReadResolutionError::Unavailable);
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
        ErrorData, ListResourcesResult, PaginatedRequestParams, ReadResourceRequestParams,
        ReadResourceResponse, ReadResourceResult, RequestMetaObject, Resource, ResourceContents,
    };
    use rmcp::service::RequestContext;
    use rmcp::{RoleServer, ServerHandler};
    use tokio::sync::{Mutex, Notify};

    use super::{
        ResourceReadResolutionError, ResourceReadResolutionInput, read_exact_project_resource,
        read_transport_bound_project_resource_with_clock,
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
    struct EchoResourceServer {
        calls: Arc<AtomicUsize>,
        received: Arc<Mutex<Vec<(ReadResourceRequestParams, RequestMetaObject)>>>,
    }

    impl ServerHandler for EchoResourceServer {
        async fn list_resources(
            &self,
            _: Option<PaginatedRequestParams>,
            _: RequestContext<RoleServer>,
        ) -> Result<ListResourcesResult, ErrorData> {
            Ok(ListResourcesResult::with_all_items(vec![Resource::new(
                "lab://upstream/inner/file:///nested/value",
                "exact",
            )]))
        }

        async fn read_resource(
            &self,
            request: ReadResourceRequestParams,
            context: RequestContext<RoleServer>,
        ) -> Result<ReadResourceResponse, ErrorData> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            self.received
                .lock()
                .await
                .push((request.clone(), context.meta.clone()));
            Ok(ReadResourceResult::new(vec![
                ResourceContents::text("exact", "https://wrong.invalid/text"),
                ResourceContents::blob("YWJj", "https://wrong.invalid/blob"),
            ])
            .into())
        }
    }

    #[derive(Clone)]
    struct DelayedResourceServer {
        started: Arc<Notify>,
        release: Arc<Notify>,
        fail: bool,
    }

    impl ServerHandler for DelayedResourceServer {
        async fn read_resource(
            &self,
            request: ReadResourceRequestParams,
            _: RequestContext<RoleServer>,
        ) -> Result<ReadResourceResponse, ErrorData> {
            self.started.notify_one();
            self.release.notified().await;
            if self.fail {
                return Err(ErrorData::internal_error(
                    "private delayed resource failure",
                    None,
                ));
            }
            Ok(
                ReadResourceResult::new(vec![ResourceContents::text("delayed", request.uri)])
                    .into(),
            )
        }
    }

    #[derive(Clone)]
    struct FailingResourceServer;

    impl ServerHandler for FailingResourceServer {
        async fn read_resource(
            &self,
            _: ReadResourceRequestParams,
            _: RequestContext<RoleServer>,
        ) -> Result<ReadResourceResponse, ErrorData> {
            Err(ErrorData::invalid_params(
                "private stable resource failure",
                None,
            ))
        }
    }

    #[derive(Clone)]
    struct OversizedResourceServer;

    impl ServerHandler for OversizedResourceServer {
        async fn read_resource(
            &self,
            request: ReadResourceRequestParams,
            _: RequestContext<RoleServer>,
        ) -> Result<ReadResourceResponse, ErrorData> {
            Ok(ReadResourceResult::new(vec![ResourceContents::text(
                "x".repeat(12 * 1024 * 1024),
                request.uri,
            )])
            .into())
        }
    }

    struct Fixture {
        _directory: tempfile::TempDir,
        runtime: Arc<AccessRuntime>,
        manager: Arc<GatewayManager>,
        gateway_runtime: GatewayRuntimeHandle,
        pool: Arc<UpstreamPool>,
        identity: VerifiedIdentity,
        calls: Arc<AtomicUsize>,
        received: Arc<Mutex<Vec<(ReadResourceRequestParams, RequestMetaObject)>>>,
    }

    fn gateway_config(expose_resources: bool) -> GatewayConfig {
        GatewayConfig {
            upstream: vec![UpstreamConfig {
                enabled: true,
                name: "alpha".into(),
                url: None,
                transport: None,
                socket_path: None,
                headers: Default::default(),
                bearer_token_env: None,
                command: Some("node".into()),
                args: Vec::new(),
                env: Default::default(),
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
            loadouts: vec![GatewayLoadoutConfig {
                name: "production".into(),
                upstreams: vec!["alpha".into()],
                expose_resources,
                expose_skills: false,
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
        let received = Arc::new(Mutex::new(Vec::new()));
        let pool = Arc::new(UpstreamPool::new());
        pool.install_prompt_server_for_tests(
            "alpha",
            EchoResourceServer {
                calls: Arc::clone(&calls),
                received: Arc::clone(&received),
            },
        )
        .await;
        pool.insert_resource_routes_for_tests(
            "alpha",
            vec![Resource::new(
                "lab://upstream/inner/file:///nested/value",
                "exact",
            )],
        )
        .await;
        let gateway_runtime = GatewayRuntimeHandle::default();
        gateway_runtime.swap(Some(Arc::clone(&pool))).await;
        let gateway_path = directory.path().join("resource-execution.toml");
        let manager = Arc::new(GatewayManager::with_store(
            gateway_path.clone(),
            gateway_runtime.clone(),
            Arc::new(FsGatewayConfigStore::new(gateway_path)),
        ));
        manager.try_seed_config(gateway_config(true)).await.unwrap();
        Fixture {
            _directory: directory,
            runtime,
            manager,
            gateway_runtime,
            pool,
            identity,
            calls,
            received,
        }
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

    fn input(identity: VerifiedIdentity) -> ResourceReadResolutionInput {
        let mut meta = RequestMetaObject::new();
        meta.insert("trace".into(), serde_json::json!("opaque"));
        ResourceReadResolutionInput::new(
            identity,
            "project-route",
            "https://mcp.example.com/project",
            "bootstrap-default",
            ReadResourceRequestParams::new(
                "lab://upstream/alpha/lab://upstream/inner/file:///nested/value",
            )
            .with_meta(meta),
        )
    }

    #[tokio::test]
    async fn exact_asset_use_resource_read_rewrites_native_and_normalizes_every_content() {
        let fixture = fixture().await;
        let result = read_exact_project_resource(
            &fixture.runtime,
            &fixture.manager,
            input(fixture.identity.clone()),
        )
        .await
        .expect("owner AssetUse read");

        assert_eq!(fixture.calls.load(Ordering::SeqCst), 1);
        let received = fixture.received.lock().await;
        assert_eq!(
            received[0].0.uri,
            "lab://upstream/inner/file:///nested/value"
        );
        assert_eq!(
            received[0].1.get("trace"),
            Some(&serde_json::json!("opaque"))
        );
        let value = serde_json::to_value(result).unwrap();
        assert_eq!(
            value["contents"][0]["uri"],
            "lab://upstream/alpha/lab://upstream/inner/file:///nested/value"
        );
        assert_eq!(value["contents"][1]["uri"], value["contents"][0]["uri"]);
    }

    #[tokio::test]
    async fn transport_binding_expiry_after_resource_rpc_discards_result_deterministically() {
        let fixture = fixture().await;
        let before = UNIX_EPOCH + Duration::from_secs(100);
        let after = UNIX_EPOCH + Duration::from_secs(102);
        let transport = transport_binding(&fixture, 101, before).await;
        let ResourceReadResolutionInput { request, .. } = input(fixture.identity.clone());
        let mut times = [before, after].into_iter();

        assert_eq!(
            read_transport_bound_project_resource_with_clock(
                &fixture.runtime,
                &fixture.manager,
                &transport,
                &fixture.identity,
                request,
                || times.next().unwrap(),
            )
            .await,
            Err(ResourceReadResolutionError::Unavailable)
        );
        assert_eq!(fixture.calls.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn handler_bound_regular_reads_and_nonregular_families_never_fall_back() {
        let fixture = fixture().await;
        let now = UNIX_EPOCH + Duration::from_secs(100);
        let (transport, _client_transport) = tokio::io::duplex(64 * 1024);
        let running = rmcp::service::serve_directly::<RoleServer, _, _, std::io::Error, _>(
            handler_server(&fixture),
            transport,
            None,
        );
        let bound = transport_binding(&fixture, usize::MAX, now).await;
        let ResourceReadResolutionInput { request, .. } = input(fixture.identity.clone());
        let response = running
            .service()
            .read_resource_impl(
                request,
                handler_context(
                    running.peer().clone(),
                    Some(fixture.identity.clone()),
                    Ok(bound),
                ),
            )
            .await
            .expect("bound exact regular Resource");
        assert_eq!(fixture.calls.load(Ordering::SeqCst), 1);
        let ReadResourceResponse::Complete(response) = response else {
            panic!("unexpected incomplete Resource response")
        };
        let value = serde_json::to_value(response).unwrap();
        assert_eq!(
            value["contents"][0]["uri"],
            "lab://upstream/alpha/lab://upstream/inner/file:///nested/value"
        );

        for uri in [
            "lab://catalog",
            "ui://lab/code-mode/app.html",
            "lab://gateway/actions",
            "lab://gateway/servers",
            "skill://labby/using-labby/SKILL.md",
            "lab://upstream/oauth/file:///subject",
            "lab://upstream/alpha/missing",
        ] {
            let bound = transport_binding(&fixture, usize::MAX, now).await;
            let error = running
                .service()
                .read_resource_impl(
                    ReadResourceRequestParams::new(uri),
                    handler_context(
                        running.peer().clone(),
                        Some(fixture.identity.clone()),
                        Ok(bound),
                    ),
                )
                .await
                .expect_err("Project-bound nonregular Resource is terminal");
            assert_eq!(error.data.as_ref().unwrap()["kind"], "not_found");
        }
        assert_eq!(fixture.calls.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn handler_unavailable_expired_and_identity_mismatch_are_terminal_without_rpc() {
        let fixture = fixture().await;
        let (transport, _client_transport) = tokio::io::duplex(64 * 1024);
        let running = rmcp::service::serve_directly::<RoleServer, _, _, std::io::Error, _>(
            handler_server(&fixture),
            transport,
            None,
        );
        let now = UNIX_EPOCH + Duration::from_secs(100);
        let expired = transport_binding(&fixture, 101, now).await;
        let current = transport_binding(&fixture, usize::MAX, now).await;
        let other_identity = VerifiedIdentity::local_credential_with_issuer(
            Authenticator::StaticBearer,
            "other-issuer",
            "other-credential",
        )
        .unwrap();
        let contexts = [
            handler_context(
                running.peer().clone(),
                Some(fixture.identity.clone()),
                Err(crate::mcp::bound_access::BoundAccessContextError::Unavailable),
            ),
            handler_context(
                running.peer().clone(),
                Some(fixture.identity.clone()),
                Ok(expired),
            ),
            handler_context(running.peer().clone(), None, Ok(current)),
            handler_context(
                running.peer().clone(),
                Some(other_identity),
                Ok(transport_binding(&fixture, usize::MAX, now).await),
            ),
        ];
        for context in contexts {
            let error = running
                .service()
                .read_resource_impl(ReadResourceRequestParams::new("lab://catalog"), context)
                .await
                .expect_err("unavailable Project binding is terminal");
            assert_eq!(error.data.as_ref().unwrap()["kind"], "not_found");
        }
        assert_eq!(fixture.calls.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn handler_legacy_local_path_preserves_existing_output() {
        let fixture = fixture().await;
        let (transport, _client_transport) = tokio::io::duplex(64 * 1024);
        let running = rmcp::service::serve_directly::<RoleServer, _, _, std::io::Error, _>(
            handler_server(&fixture),
            transport,
            None,
        );
        let local = running
            .service()
            .read_resource_impl(
                ReadResourceRequestParams::new("lab://catalog"),
                RequestContext::new(
                    rmcp::model::NumberOrString::Number(1),
                    running.peer().clone(),
                ),
            )
            .await
            .expect("legacy local Resource remains mounted");
        let ReadResourceResponse::Complete(local) = local else {
            panic!("unexpected incomplete local response")
        };
        let ResourceContents::TextResourceContents { uri, .. } = &local.contents[0] else {
            panic!("legacy catalog must remain text")
        };
        assert_eq!(uri, "lab://catalog");
    }

    #[tokio::test]
    async fn handler_bound_oauth_uri_is_terminal_while_legacy_reaches_oauth_branch() {
        let fixture = fixture().await;
        let (config, oauth) = oauth_gateway_config();
        fixture.manager.try_seed_config(config).await.unwrap();
        fixture
            .pool
            .install_test_subject_server_for_upstream(
                &oauth,
                "reader",
                EchoResourceServer {
                    calls: Arc::clone(&fixture.calls),
                    received: Arc::clone(&fixture.received),
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
        let uri = "lab://upstream/oauth/file:///subject";
        let bound_error = running
            .service()
            .read_resource_impl(
                ReadResourceRequestParams::new(uri),
                handler_context(running.peer().clone(), Some(fixture.identity), Ok(bound)),
            )
            .await
            .expect_err("Bound OAuth-shaped URI must not fall through");
        assert_eq!(bound_error.data.as_ref().unwrap()["kind"], "not_found");

        let legacy_error = running
            .service()
            .read_resource_impl(
                ReadResourceRequestParams::new(uri),
                legacy_oauth_context(running.peer().clone()),
            )
            .await
            .expect_err("legacy OAuth fixture intentionally cannot connect");
        assert_eq!(legacy_error.code, rmcp::model::ErrorCode::INVALID_PARAMS);
        assert!(legacy_error.message.contains("oauth"));
        assert_eq!(fixture.calls.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn handler_operational_failures_are_redacted_upstream_errors() {
        for oversized in [false, true] {
            let fixture = fixture().await;
            if oversized {
                fixture
                    .pool
                    .install_prompt_server_for_tests("alpha", OversizedResourceServer)
                    .await;
            } else {
                fixture
                    .pool
                    .install_prompt_server_for_tests("alpha", FailingResourceServer)
                    .await;
            }
            fixture
                .pool
                .insert_resource_routes_for_tests(
                    "alpha",
                    vec![Resource::new(
                        "lab://upstream/inner/file:///nested/value",
                        "exact",
                    )],
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
            let ResourceReadResolutionInput { request, .. } = input(fixture.identity.clone());
            let error = running
                .service()
                .read_resource_impl(
                    request,
                    handler_context(
                        running.peer().clone(),
                        Some(fixture.identity.clone()),
                        Ok(bound),
                    ),
                )
                .await
                .expect_err("operational Resource read failure");
            // Each cause keeps its own stable kind rather than flattening to
            // `upstream_error`: an oversized response tells the caller to
            // reduce work, an upstream fault does not. Redaction is about the
            // private failure text, not about hiding the documented kind.
            let expected_kind = if oversized {
                "response_too_large"
            } else {
                "upstream_error"
            };
            assert_eq!(error.data.as_ref().unwrap()["kind"], expected_kind);
            assert!(!error.message.contains("private"));
            assert!(
                !error
                    .to_string()
                    .contains("private stable resource failure")
            );
        }
    }

    #[tokio::test]
    async fn exact_resource_read_rejects_non_asset_use_and_hidden_route_without_rpc() {
        let viewer = fixture().await;
        viewer
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
            read_exact_project_resource(
                &viewer.runtime,
                &viewer.manager,
                input(viewer.identity.clone()),
            )
            .await,
            Err(ResourceReadResolutionError::Unavailable)
        );
        assert_eq!(viewer.calls.load(Ordering::SeqCst), 0);

        let hidden = fixture().await;
        hidden
            .manager
            .try_seed_config(gateway_config(false))
            .await
            .unwrap();
        assert_eq!(
            read_exact_project_resource(&hidden.runtime, &hidden.manager, input(hidden.identity),)
                .await,
            Err(ResourceReadResolutionError::Unavailable)
        );
        assert_eq!(hidden.calls.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn exact_resource_read_rejects_unknown_and_ui_targets_without_rpc() {
        let fixture = fixture().await;
        for wire_uri in [
            "lab://upstream/alpha/file:///missing",
            "lab://upstream/alpha/UI://widget",
        ] {
            let mut request = input(fixture.identity.clone());
            request.request.uri = wire_uri.to_string();
            assert_eq!(
                read_exact_project_resource(&fixture.runtime, &fixture.manager, request).await,
                Err(ResourceReadResolutionError::Unavailable)
            );
        }
        assert_eq!(fixture.calls.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn exact_resource_read_discards_pool_publication_aba_success_and_failure() {
        for fail in [false, true] {
            let fixture = fixture().await;
            let started = Arc::new(Notify::new());
            let release = Arc::new(Notify::new());
            fixture
                .pool
                .install_prompt_server_for_tests(
                    "alpha",
                    DelayedResourceServer {
                        started: Arc::clone(&started),
                        release: Arc::clone(&release),
                        fail,
                    },
                )
                .await;
            fixture
                .pool
                .insert_resource_routes_for_tests(
                    "alpha",
                    vec![Resource::new(
                        "lab://upstream/inner/file:///nested/value",
                        "exact",
                    )],
                )
                .await;
            fixture
                .pool
                .set_resource_last_error_for_tests("alpha", Some("sentinel".into()))
                .await;
            let runtime = Arc::clone(&fixture.runtime);
            let manager = Arc::clone(&fixture.manager);
            let request = input(fixture.identity.clone());
            let task = tokio::spawn(async move {
                read_exact_project_resource(&runtime, &manager, request).await
            });
            started.notified().await;
            fixture.gateway_runtime.swap(None).await;
            fixture
                .gateway_runtime
                .swap(Some(Arc::clone(&fixture.pool)))
                .await;
            release.notify_one();
            assert_eq!(
                task.await.unwrap(),
                Err(ResourceReadResolutionError::Unavailable)
            );
            assert_eq!(
                fixture
                    .pool
                    .resource_last_error_for_tests("alpha")
                    .await
                    .as_deref(),
                Some("sentinel")
            );
        }
    }

    #[tokio::test]
    async fn exact_resource_read_rejects_access_route_and_resource_generation_aba() {
        for mutation in ["access", "route", "resource"] {
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
            let initial_runtime_generation = fixture
                .manager
                .published_runtime_loadout_snapshot("production")
                .await
                .generation();
            let started = Arc::new(Notify::new());
            let release = Arc::new(Notify::new());
            fixture
                .pool
                .install_prompt_server_for_tests(
                    "alpha",
                    DelayedResourceServer {
                        started: Arc::clone(&started),
                        release: Arc::clone(&release),
                        fail: false,
                    },
                )
                .await;
            fixture
                .pool
                .insert_resource_routes_for_tests(
                    "alpha",
                    vec![Resource::new(
                        "lab://upstream/inner/file:///nested/value",
                        "exact",
                    )],
                )
                .await;
            let initial_resource_generation = fixture
                .pool
                .published_resource_catalog()
                .await
                .unwrap()
                .generation();
            let runtime = Arc::clone(&fixture.runtime);
            let manager = Arc::clone(&fixture.manager);
            let request = input(fixture.identity.clone());
            let task = tokio::spawn(async move {
                read_exact_project_resource(&runtime, &manager, request).await
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
                    assert_ne!(
                        fixture
                            .manager
                            .published_runtime_loadout_snapshot("production")
                            .await
                            .generation(),
                        initial_runtime_generation
                    );
                }
                "resource" => {
                    fixture
                        .pool
                        .insert_resource_routes_for_tests(
                            "alpha",
                            vec![Resource::new("file:///other", "other")],
                        )
                        .await;
                    assert_ne!(
                        fixture
                            .pool
                            .published_resource_catalog()
                            .await
                            .unwrap()
                            .generation(),
                        initial_resource_generation
                    );
                    fixture
                        .pool
                        .insert_resource_routes_for_tests(
                            "alpha",
                            vec![Resource::new(
                                "lab://upstream/inner/file:///nested/value",
                                "exact",
                            )],
                        )
                        .await;
                    assert_ne!(
                        fixture
                            .pool
                            .published_resource_catalog()
                            .await
                            .unwrap()
                            .generation(),
                        initial_resource_generation
                    );
                }
                _ => unreachable!(),
            }
            release.notify_one();
            assert_eq!(
                task.await.unwrap(),
                Err(ResourceReadResolutionError::Unavailable),
                "{mutation} ABA must be detected"
            );
        }
    }

    #[tokio::test]
    async fn exact_resource_read_cancellation_never_applies_prepared_outcome() {
        let fixture = fixture().await;
        let started = Arc::new(Notify::new());
        let release = Arc::new(Notify::new());
        fixture
            .pool
            .install_prompt_server_for_tests(
                "alpha",
                DelayedResourceServer {
                    started: Arc::clone(&started),
                    release: Arc::clone(&release),
                    fail: false,
                },
            )
            .await;
        fixture
            .pool
            .insert_resource_routes_for_tests(
                "alpha",
                vec![Resource::new(
                    "lab://upstream/inner/file:///nested/value",
                    "exact",
                )],
            )
            .await;
        fixture
            .pool
            .set_resource_last_error_for_tests("alpha", Some("sentinel".into()))
            .await;
        let runtime = Arc::clone(&fixture.runtime);
        let manager = Arc::clone(&fixture.manager);
        let request = input(fixture.identity);
        let task =
            tokio::spawn(
                async move { read_exact_project_resource(&runtime, &manager, request).await },
            );
        started.notified().await;
        task.abort();
        release.notify_one();
        assert!(task.await.unwrap_err().is_cancelled());
        fixture.gateway_runtime.swap(None).await;
        fixture
            .gateway_runtime
            .swap(Some(Arc::clone(&fixture.pool)))
            .await;
        assert_eq!(
            fixture
                .pool
                .resource_last_error_for_tests("alpha")
                .await
                .as_deref(),
            Some("sentinel")
        );
    }

    #[tokio::test]
    async fn exact_resource_read_maps_stable_upstream_error_without_private_detail() {
        let fixture = fixture().await;
        fixture
            .pool
            .install_prompt_server_for_tests("alpha", FailingResourceServer)
            .await;
        fixture
            .pool
            .insert_resource_routes_for_tests(
                "alpha",
                vec![Resource::new(
                    "lab://upstream/inner/file:///nested/value",
                    "exact",
                )],
            )
            .await;
        let error = read_exact_project_resource(
            &fixture.runtime,
            &fixture.manager,
            input(fixture.identity),
        )
        .await
        .expect_err("stable application error");
        assert_eq!(error, ResourceReadResolutionError::Upstream);
        assert!(
            !error
                .to_string()
                .contains("private stable resource failure")
        );
    }

    #[tokio::test]
    async fn exact_resource_read_maps_oversized_result() {
        let fixture = fixture().await;
        fixture
            .pool
            .install_prompt_server_for_tests("alpha", OversizedResourceServer)
            .await;
        fixture
            .pool
            .insert_resource_routes_for_tests(
                "alpha",
                vec![Resource::new(
                    "lab://upstream/inner/file:///nested/value",
                    "exact",
                )],
            )
            .await;

        assert_eq!(
            read_exact_project_resource(
                &fixture.runtime,
                &fixture.manager,
                input(fixture.identity),
            )
            .await,
            Err(ResourceReadResolutionError::TooLarge)
        );
    }
}
