//! MCP-owned in-process peer construction for built-in Lab services.

use std::sync::Arc;
use std::sync::atomic::AtomicU8;

use crate::mcp::logging::LoggingLevel;
use rmcp::service::{ClientLifecycleMode, ClientServiceExt};
use rmcp::{RoleClient, ServiceExt};

use labby_gateway::registry::InProcessService;

use crate::access::AccessRuntime;
use crate::dispatch::upstream::pool::{
    InProcessConnector, InProcessRegistration, UpstreamConnection, in_process_upstream_name,
};
use crate::dispatch::upstream::types::UpstreamRuntimeMetadata;
use crate::mcp::logging::logging_level_rank;
use crate::mcp::server::LabMcpServer;
use crate::registry::{RegisteredService, ToolRegistry};

const IN_PROCESS_PEER_BUFFER_BYTES: usize = 256 * 1024;

/// Transport label for the in-process service peers.
///
/// Load-bearing beyond logging: it is deliberately absent from
/// `mcp::server::TRANSPORTS_TRUSTING_ABSENT_AUTH`, so this transport does not
/// inherit the stdio trust model. Requests arriving over the duplex carry no
/// `AuthContext` because there is no HTTP layer to inject one — that means
/// "unauthenticated hop", not "trusted local operator" (bead lab-m01gl).
pub(crate) const IN_PROCESS_TRANSPORT_LABEL: &str = "in-process";

pub(crate) fn connector() -> InProcessConnector {
    Arc::new(|service: Box<dyn InProcessService>| {
        Box::pin(async move {
            // The gateway pool hands back the type-erased service it was given;
            // recover the concrete `RegisteredService` this crate registered.
            let service = service
                .as_any()
                .downcast::<RegisteredService>()
                .map_err(|_| {
                    anyhow::anyhow!(
                        "in-process connector received a non-RegisteredService peer descriptor"
                    )
                })?;
            connect_in_process_service_peer(*service).await
        })
    })
}

/// Build the mini server that fronts one builtin service.
///
/// Extracted so the trust-boundary test exercises the same construction
/// production uses — a test that hand-rolled an equivalent server could drift
/// from this one and keep passing.
pub(crate) fn build_peer_server(service: &RegisteredService) -> LabMcpServer {
    let mut registry = ToolRegistry::new();
    registry.register(service.clone());
    LabMcpServer {
        registry: Arc::new(registry),
        // Delegated built-in peers are protocol adapters, not access-policy
        // decision points or process lifecycle owners. Give them an explicit,
        // non-I/O blocked runtime so future enforcement cannot accidentally
        // treat this internal hop as an independently authoritative store.
        access_runtime: Arc::new(AccessRuntime::blocked_unavailable()),
        file_stash_runtime: Arc::new(crate::file_stash::FileStashRuntime::blocked()),
        // Each of these INDEPENDENTLY closes the re-entrancy path:
        // `expose_code_mode: false` (below) short-circuits
        // `code_mode_visibility` to `Raw` before the manager is consulted, and
        // `gateway_manager: None` makes `current_upstream_pool()` return
        // `None`. Keep BOTH — the redundancy is deliberate, and neither is
        // pinned by a test. Losing both would let this mini-server's
        // `list_tools` re-enter `code_mode_catalog_tools_allowed` and call
        // `ensure_in_process_service_peers` from inside a registration, which
        // is a runtime-only hang rather than a compile error.
        gateway_manager: None,
        peers: Default::default(),
        code_mode_app_state: Default::default(),
        last_listed_tool_contract: Default::default(),
        route_runtime: Default::default(),
        client_registry: Default::default(),
        transport_label: IN_PROCESS_TRANSPORT_LABEL,
        logging_level: Arc::new(AtomicU8::new(logging_level_rank(LoggingLevel::Emergency))),
        // FU-1 (issue #210, lab-48z4k): force Raw mode. Under `Root` the mini
        // server derives its visibility from the process-global code-mode
        // flag, so inside a serve process with Code Mode ENABLED the peer
        // suppressed the very service it exists to expose and registered zero
        // tools. `expose_code_mode: false` pins Raw regardless of the flag,
        // and the single-service allowlist scopes the peer to exactly its
        // own service.
        route_scope: crate::mcp::route_scope::McpRouteScope::protected_subset(
            format!("in-process-{}", service.name),
            std::iter::empty::<&str>(),
            [service.name],
            /* expose_code_mode */ false,
        ),
        relay_session_id: crate::mcp::server::next_relay_session_id(),
        #[cfg(test)]
        code_mode_widget_callbacks_enabled_for_test: false,
    }
}

async fn connect_in_process_service_peer(
    service: RegisteredService,
) -> anyhow::Result<InProcessRegistration> {
    tracing::info!(
        service = service.name,
        phase = "in_process.connect.start",
        "connecting in-process peer"
    );
    let upstream_name = in_process_upstream_name(service.name);
    let entry_name: Arc<str> = Arc::from(upstream_name.as_str());
    let (server_transport, client_transport) = tokio::io::duplex(IN_PROCESS_PEER_BUFFER_BYTES);
    let server = build_peer_server(&service);
    let service_name = service.name;
    let server_task = tokio::spawn(async move {
        tracing::info!(
            service = service_name,
            phase = "in_process.server.spawned",
            "starting in-process server task"
        );
        match server.serve(server_transport).await {
            Ok(running) => {
                tracing::info!(
                    service = service_name,
                    phase = "in_process.server.ready",
                    "in-process server transport ready"
                );
                if let Err(error) = running.waiting().await {
                    tracing::warn!(
                        service = service_name,
                        phase = "in_process.server.waiting.error",
                        error = %error,
                        "in-process server exited with error"
                    );
                }
            }
            Err(error) => {
                tracing::warn!(
                    service = service_name,
                    phase = "in_process.server.serve.error",
                    error = %error,
                    "failed to start in-process server"
                );
            }
        }
    });
    let client_service: rmcp::service::RunningService<RoleClient, ()> = ()
        .serve_with_lifecycle(
            client_transport,
            ClientLifecycleMode::Discover {
                preferred_versions: vec![rmcp::model::ProtocolVersion::V_2026_07_28],
            },
        )
        .await?;
    tracing::info!(
        service = service.name,
        phase = "in_process.client.ready",
        "in-process client transport ready"
    );
    let peer = client_service.peer().clone();
    tracing::info!(
        service = service.name,
        phase = "in_process.list_tools.start",
        process_code_mode_enabled = crate::config::process_code_mode_enabled(),
        "requesting in-process tool list"
    );
    // The in-process peer is labby's own trusted server — no adversarial
    // `nextCursor` is possible, so the unbounded helper is safe here.
    #[allow(clippy::disallowed_methods)]
    let tools = peer.list_all_tools().await?;
    tracing::info!(
        service = service.name,
        phase = "in_process.list_tools.finish",
        tool_count = tools.len(),
        process_code_mode_enabled = crate::config::process_code_mode_enabled(),
        "in-process tool list received"
    );

    Ok(InProcessRegistration {
        connection: Some(UpstreamConnection::new(
            client_service,
            Some(server_task),
            peer,
            UpstreamRuntimeMetadata::default(),
        )),
        tools,
        entry_name,
        upstream_name,
    })
}

#[cfg(all(test, feature = "skills"))]
mod skill_generation_tests {
    use crate::skills::facade::SkillRegistryContext;
    use crate::skills::registry::{FirstPartyGenerationManager, GenerationLimits};

    #[tokio::test]
    async fn code_mode_in_process_boundary_keeps_a_captured_generation_during_refresh() {
        let temp = tempfile::tempdir().unwrap();
        let dir = temp.path().join("code-mode-race");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("SKILL.md"),
            "---\nname: code-mode-race\ndescription: old\n---\nold\n",
        )
        .unwrap();
        let manager =
            FirstPartyGenerationManager::new(temp.path().into(), GenerationLimits::default());
        let old = SkillRegistryContext::from_generation(manager.generation());
        std::fs::write(
            dir.join("SKILL.md"),
            "---\nname: code-mode-race\ndescription: new\n---\nnew\n",
        )
        .unwrap();
        manager.refresh(None).unwrap();
        let uri = "skill://labby/code-mode-race/SKILL.md";
        let old_value = crate::mcp::skills::dispatch_at_in_process_boundary(
            &old,
            "skills.read",
            serde_json::json!({"uri": uri}),
        )
        .await
        .unwrap();
        let current = SkillRegistryContext::from_generation(manager.generation());
        let new_value = crate::mcp::skills::dispatch_at_in_process_boundary(
            &current,
            "skills.read",
            serde_json::json!({"uri": uri}),
        )
        .await
        .unwrap();
        assert!(old_value["text"].as_str().unwrap().contains("old"));
        assert!(new_value["text"].as_str().unwrap().contains("new"));
        assert_ne!(old_value["digest"], new_value["digest"]);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::Value;
    use std::future::Future;
    use std::pin::Pin;

    fn noop_dispatch(
        _action: String,
        _params: Value,
    ) -> Pin<Box<dyn Future<Output = Result<Value, crate::dispatch::error::ToolError>> + Send>>
    {
        Box::pin(async { Ok(serde_json::json!({})) })
    }

    const TEST_ACTIONS: &[labby_primitives::action::ActionSpec] =
        &[labby_primitives::action::ActionSpec {
            name: "demo.list",
            description: "List demo entries",
            params: &[],
            returns: "DemoList",
            destructive: false,
            requires_admin: false,
        }];

    #[tokio::test]
    async fn in_process_peer_is_not_an_access_policy_decision_point() {
        let service = RegisteredService {
            name: "gateway-alpha",
            description: "Gateway alpha",
            category: "network",
            kind: crate::registry::RegisteredServiceKind::BuiltInUpstreamApi,
            status: "available",
            actions: TEST_ACTIONS,
            dispatch: noop_dispatch,
        };

        let server = build_peer_server(&service);

        assert_eq!(
            server.access_runtime.status().await,
            crate::access::AccessRuntimeStatus::Blocked(
                crate::access::AccessBlockedReason::Unavailable
            )
        );
        assert_eq!(
            server.access_runtime.store().await.unwrap_err(),
            crate::access::AccessRuntimeError::Blocked(
                crate::access::AccessBlockedReason::Unavailable
            )
        );
    }

    /// FU-1 (issue #210, lab-48z4k): the mini in-process server must list its
    /// service in Raw mode even when the PROCESS code-mode flag is enabled —
    /// which is exactly the state of a serve process whose Code Mode catalog
    /// these peers exist to populate. Before the fix the peer derived
    /// `InProcessPeer` visibility from the global flag, suppressed its own
    /// service, and registered zero tools.
    ///
    /// Process-global Code Mode state is serialized by the test guard so this
    /// remains hermetic under both nextest and plain parallel `cargo test`.
    #[tokio::test]
    async fn in_process_peer_lists_its_service_under_process_code_mode() {
        let _guard = crate::config::process_code_mode_test_guard();
        crate::config::set_process_code_mode_enabled_for_test(true);

        let service = RegisteredService {
            name: "gateway-alpha",
            description: "Gateway alpha",
            category: "network",
            kind: crate::registry::RegisteredServiceKind::BuiltInUpstreamApi,
            status: "available",
            actions: TEST_ACTIONS,
            dispatch: noop_dispatch,
        };

        let registration = connect_in_process_service_peer(service)
            .await
            .expect("in-process registration");

        assert_eq!(
            registration.tools.len(),
            1,
            "the peer must expose exactly its own service tool"
        );
        let tool = &registration.tools[0];
        assert_eq!(tool.name.as_ref(), "gateway-alpha");
        let schema = tool
            .output_schema
            .as_ref()
            .expect("builtin peer tool carries the envelope outputSchema");
        assert_eq!(
            schema["properties"]["ok"]["const"],
            serde_json::json!(true),
            "schema and capability must arrive together (FU-1)"
        );
    }
}
