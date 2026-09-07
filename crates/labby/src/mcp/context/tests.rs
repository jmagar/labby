//! Tests for request-context auth/subject + scope/admin gate helpers.
//! Distributed from `server.rs` (bead `lab-kvji.24.1.6`).

use super::{
    AbsentAuth, actor_key_from_extensions, builtin_action_requires_admin,
    code_mode_read_scope_allowed, forwardable_client_capabilities, resolve_caller_authorization,
    subject_from_extensions, tool_execute_builtin_action_allowed, tool_execute_scope_allowed,
};
#[cfg(feature = "gateway")]
use super::{
    oauth_upstream_subject_for_request, team_credential_binding_matches,
    upstream_uses_capability_relay,
};
use crate::dispatch::error::ToolError;
use crate::registry::RegisteredService;
use labby_runtime::caller_auth::PropagatedCallerAuth;
use serde_json::Value;
use std::future::Future;
use std::pin::Pin;

#[cfg(feature = "gateway")]
#[test]
fn team_credential_checkout_rejects_stale_and_revoked_bindings() {
    use labby_runtime::gateway_authority::{TeamCredentialBinding, TeamCredentialStatus};

    let mut binding = TeamCredentialBinding {
        binding_id: "binding-v2".into(),
        team_id: "alpha".into(),
        upstream_name: "shared".into(),
        custodian_principal_id: "owner".into(),
        generation: 2,
        rotated_at_millis: 2,
        status: TeamCredentialStatus::Active,
    };
    assert!(team_credential_binding_matches(
        Some(&binding),
        "binding-v2",
        2
    ));
    assert!(!team_credential_binding_matches(
        Some(&binding),
        "binding-v1",
        1
    ));
    binding.status = TeamCredentialStatus::Revoked;
    assert!(!team_credential_binding_matches(
        Some(&binding),
        "binding-v2",
        2
    ));
    assert!(!team_credential_binding_matches(None, "binding-v2", 2));
}

#[test]
fn caller_authorization_read_gate_handles_every_transport_shape() {
    let reader = make_auth(&["lab:read"]);
    let execute = make_auth(&["lab"]);
    let unrelated = make_auth(&["profile"]);

    assert!(resolve_caller_authorization(Some(&reader), AbsentAuth::Untrusted, None,).can_read());
    assert!(resolve_caller_authorization(Some(&execute), AbsentAuth::Untrusted, None,).can_read());
    assert!(
        !resolve_caller_authorization(Some(&unrelated), AbsentAuth::Untrusted, None,).can_read()
    );
    assert!(resolve_caller_authorization(None, AbsentAuth::TrustedLocal, None).can_read());
    assert!(
        resolve_caller_authorization(
            None,
            AbsentAuth::Untrusted,
            Some(PropagatedCallerAuth::scoped(
                vec!["lab:read".to_string()],
                Some("alice".to_string()),
            )),
        )
        .can_read()
    );
    assert!(!resolve_caller_authorization(None, AbsentAuth::Untrusted, None).can_read());
}

#[test]
fn forwardable_capabilities_are_derived_from_current_request_metadata() {
    let capabilities = rmcp::model::ClientCapabilities::builder()
        .enable_elicitation()
        .build();
    let meta = rmcp::model::RequestMetaObject::with_client_context(
        rmcp::model::ProtocolVersion::V_2026_07_28,
        rmcp::model::Implementation::new("test-client", "1.0.0"),
        capabilities.clone(),
    );

    assert_eq!(
        forwardable_client_capabilities(Some(&meta)),
        Some(capabilities)
    );
    assert_eq!(
        forwardable_client_capabilities(None),
        Some(rmcp::model::ClientCapabilities::default())
    );

    let empty = rmcp::model::RequestMetaObject::with_client_context(
        rmcp::model::ProtocolVersion::V_2026_07_28,
        rmcp::model::Implementation::new("test-client", "1.0.0"),
        rmcp::model::ClientCapabilities::default(),
    );
    assert_eq!(
        forwardable_client_capabilities(Some(&empty)),
        Some(rmcp::model::ClientCapabilities::default())
    );
}

#[cfg(feature = "gateway")]
#[test]
fn singleton_upstream_can_opt_out_of_dedicated_capability_connections() {
    let default: crate::config::UpstreamConfig = toml::from_str(
        r#"
name = "ordinary"
command = "ordinary-server"
"#,
    )
    .expect("ordinary upstream config parses");
    assert!(upstream_uses_capability_relay(&default));

    let singleton: crate::config::UpstreamConfig = toml::from_str(
        r#"
name = "singleton"
command = "singleton-server"

[env]
MCP_UPSTREAM_RELAY_MODE = "pooled"
"#,
    )
    .expect("singleton upstream config parses");
    assert!(!upstream_uses_capability_relay(&singleton));
}

fn noop_dispatch(
    _action: String,
    _params: Value,
) -> Pin<Box<dyn Future<Output = Result<Value, ToolError>> + Send>> {
    Box::pin(async { Ok(Value::Null) })
}

fn make_auth(scopes: &[&str]) -> labby_auth::auth_context::AuthContext {
    labby_auth::auth_context::AuthContext {
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
fn server_reads_subject_scoped_upstream_pool_from_request_extensions() {
    let mut parts = axum::http::Request::new(()).into_parts().0;
    parts
        .extensions
        .insert(labby_auth::auth_context::AuthContext {
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
#[cfg(feature = "gateway")]
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
    let read_only = labby_auth::auth_context::AuthContext {
        sub: "alice".to_string(),
        actor_key: None,
        scopes: vec!["lab".to_string()],
        issuer: "https://lab.example.com".to_string(),
        via_session: true,
        csrf_token: None,
        email: None,
    };
    let admin = labby_auth::auth_context::AuthContext {
        scopes: vec!["lab:admin".to_string()],
        ..read_only.clone()
    };

    assert!(tool_execute_builtin_action_allowed(
        &entry,
        "gateway.help",
        &resolve_caller_authorization(Some(&read_only), AbsentAuth::TrustedLocal, None)
    ));
    assert!(!tool_execute_builtin_action_allowed(
        &entry,
        "gateway.import",
        &resolve_caller_authorization(Some(&read_only), AbsentAuth::TrustedLocal, None)
    ));
    assert!(tool_execute_builtin_action_allowed(
        &entry,
        "gateway.import",
        &resolve_caller_authorization(Some(&admin), AbsentAuth::TrustedLocal, None)
    ));
    assert!(tool_execute_builtin_action_allowed(
        &entry,
        "gateway.import",
        &resolve_caller_authorization(None, AbsentAuth::TrustedLocal, None)
    ));
}

#[test]
#[cfg(feature = "gateway")]
fn snippets_builtin_actions_require_catalog_admin_scope() {
    let entry = RegisteredService {
        name: "snippets",
        description: "Snippets",
        category: "bootstrap",
        kind: crate::registry::RegisteredServiceKind::BootstrapOperator,
        status: "available",
        actions: crate::dispatch::snippets::ACTIONS,
        dispatch: noop_dispatch,
    };
    let read_only = make_auth(&["lab:read"]);
    let admin = make_auth(&["lab:admin"]);

    for spec in crate::dispatch::snippets::ACTIONS {
        assert_eq!(
            spec.requires_admin,
            builtin_action_requires_admin(&entry, spec.name),
            "MCP admin gate must follow snippets catalog for `{}`",
            spec.name
        );
        if spec.requires_admin {
            assert!(
                !tool_execute_builtin_action_allowed(
                    &entry,
                    spec.name,
                    &resolve_caller_authorization(Some(&read_only), AbsentAuth::TrustedLocal, None)
                ),
                "`{}` should reject non-admin MCP callers",
                spec.name
            );
            assert!(
                tool_execute_builtin_action_allowed(
                    &entry,
                    spec.name,
                    &resolve_caller_authorization(Some(&admin), AbsentAuth::TrustedLocal, None)
                ),
                "`{}` should allow admin MCP callers",
                spec.name
            );
        }
    }
}

#[test]
fn doctor_relay_check_uses_catalog_admin_gate() {
    let entry = RegisteredService {
        name: "doctor",
        description: "Doctor",
        category: "bootstrap",
        kind: crate::registry::RegisteredServiceKind::BootstrapOperator,
        status: "available",
        actions: crate::dispatch::doctor::ACTIONS,
        dispatch: noop_dispatch,
    };
    let read_only = make_auth(&["lab:read"]);
    let admin = make_auth(&["lab:admin"]);
    assert!(builtin_action_requires_admin(&entry, "oauth.relay.check"));
    assert!(!tool_execute_builtin_action_allowed(
        &entry,
        "oauth.relay.check",
        &resolve_caller_authorization(Some(&read_only), AbsentAuth::TrustedLocal, None)
    ));
    assert!(tool_execute_builtin_action_allowed(
        &entry,
        "doctor.oauth.relay.check",
        &resolve_caller_authorization(Some(&admin), AbsentAuth::TrustedLocal, None)
    ));
}

#[test]
fn every_registered_action_uses_its_catalog_admin_metadata() {
    for entry in crate::registry::build_default_registry().services() {
        for spec in entry.actions {
            assert_eq!(
                builtin_action_requires_admin(entry, spec.name),
                spec.requires_admin,
                "{}::{}",
                entry.name,
                spec.name
            );
        }
    }
}

#[test]
fn code_mode_scope_allows_read_but_tool_execute_does_not() {
    let base = labby_auth::auth_context::AuthContext {
        sub: "alice".to_string(),
        actor_key: None,
        scopes: vec!["lab:read".to_string()],
        issuer: "https://lab.example.com".to_string(),
        via_session: true,
        csrf_token: None,
        email: None,
    };
    let lab = labby_auth::auth_context::AuthContext {
        scopes: vec!["lab".to_string()],
        ..base.clone()
    };
    let admin = labby_auth::auth_context::AuthContext {
        scopes: vec!["lab:admin".to_string()],
        ..base.clone()
    };
    let empty = labby_auth::auth_context::AuthContext {
        scopes: Vec::new(),
        ..base.clone()
    };
    let unrelated = labby_auth::auth_context::AuthContext {
        scopes: vec!["profile".to_string()],
        ..base.clone()
    };

    assert!(code_mode_read_scope_allowed(None));
    assert!(code_mode_read_scope_allowed(Some(&base)));
    assert!(code_mode_read_scope_allowed(Some(&lab)));
    assert!(code_mode_read_scope_allowed(Some(&admin)));
    assert!(!code_mode_read_scope_allowed(Some(&empty)));
    assert!(!code_mode_read_scope_allowed(Some(&unrelated)));

    assert!(
        !tool_execute_scope_allowed(Some(&base)),
        "lab:read can read Code Mode resources but cannot execute"
    );
}

#[test]
fn setup_state_and_destructive_actions_require_admin_scope() {
    let registry = crate::registry::build_default_registry();
    let entry = registry
        .services()
        .iter()
        .find(|service| service.name == "setup")
        .expect("setup service");
    let read_only = labby_auth::auth_context::AuthContext {
        sub: "alice".to_string(),
        actor_key: None,
        scopes: vec!["lab".to_string()],
        issuer: "https://lab.example.com".to_string(),
        via_session: true,
        csrf_token: None,
        email: None,
    };
    let admin = labby_auth::auth_context::AuthContext {
        scopes: vec!["lab:admin".to_string()],
        ..read_only.clone()
    };

    assert!(!tool_execute_builtin_action_allowed(
        entry,
        "state",
        &resolve_caller_authorization(Some(&read_only), AbsentAuth::TrustedLocal, None)
    ));
    assert!(tool_execute_builtin_action_allowed(
        entry,
        "state",
        &resolve_caller_authorization(Some(&admin), AbsentAuth::TrustedLocal, None)
    ));
    assert!(!tool_execute_builtin_action_allowed(
        entry,
        "repair",
        &resolve_caller_authorization(Some(&read_only), AbsentAuth::TrustedLocal, None)
    ));
    assert!(tool_execute_builtin_action_allowed(
        entry,
        "repair",
        &resolve_caller_authorization(Some(&admin), AbsentAuth::TrustedLocal, None)
    ));
}

#[test]
#[cfg(feature = "gateway")]
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
#[cfg(feature = "gateway")]
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
fn code_mode_read_scope_allowed_permits_all_expected_scopes() {
    // None = stdio transport → trusted (always permitted)
    assert!(code_mode_read_scope_allowed(None));

    // lab:read is the minimum acceptable scope for Code Mode resources.
    let read_only = make_auth(&["lab:read"]);
    assert!(code_mode_read_scope_allowed(Some(&read_only)));

    // Bare lab must also pass Code Mode resource reads.
    let lab = make_auth(&["lab"]);
    assert!(code_mode_read_scope_allowed(Some(&lab)));

    // lab:admin must pass Code Mode resource reads.
    let admin = make_auth(&["lab:admin"]);
    assert!(code_mode_read_scope_allowed(Some(&admin)));

    // empty scopes → denied
    let no_scopes = make_auth(&[]);
    assert!(!code_mode_read_scope_allowed(Some(&no_scopes)));

    // unrelated scope → denied
    let unrelated = make_auth(&["mcp:read"]);
    assert!(!code_mode_read_scope_allowed(Some(&unrelated)));
}

#[test]
fn code_mode_resources_allow_lab_read_but_tool_calls_require_lab() {
    // Intentional asymmetry: Code Mode resources are read-only and therefore
    // accept lab:read in addition to the stronger lab / lab:admin.
    // Executable tool calls must NOT accept lab:read because they can broker
    // upstream side effects.
    let read_only = make_auth(&["lab:read"]);

    // Code Mode resource read: lab:read is permitted.
    assert!(
        code_mode_read_scope_allowed(Some(&read_only)),
        "Code Mode resource reads should accept lab:read"
    );

    // Executable tool calls: lab:read must NOT be sufficient.
    assert!(
        !tool_execute_scope_allowed(Some(&read_only)),
        "tool_execute must reject lab:read — requires lab or lab:admin"
    );
}

/// The FU-1 escalation: a transport that cannot carry auth must not inherit the
/// stdio trust model.
///
/// `auth_context_from_extensions` resolves auth by reading `http::request::Parts`
/// out of the rmcp extensions. The in-process peer is served over a duplex pipe
/// with no HTTP layer, so it yields `None` for *every* caller — including a
/// remote `lab`-scoped OAuth caller who arrived through Code Mode. Before this
/// gate took `AbsentAuth`, that `None` hit the stdio-trust branch and allowed
/// `requires_admin` builtins outright.
#[test]
#[cfg(feature = "gateway")]
fn absent_auth_on_the_in_process_transport_is_not_trusted() {
    let entry = RegisteredService {
        name: "gateway",
        description: "Gateway",
        category: "bootstrap",
        kind: crate::registry::RegisteredServiceKind::BootstrapOperator,
        status: "available",
        actions: crate::dispatch::gateway::ACTIONS,
        dispatch: noop_dispatch,
    };

    // Real stdio: absent auth still means a local operator, unchanged.
    assert!(tool_execute_builtin_action_allowed(
        &entry,
        "gateway.import",
        &resolve_caller_authorization(None, AbsentAuth::TrustedLocal, None)
    ));

    // In-process peer: the same `None` proves nothing and must be refused.
    assert!(
        !tool_execute_builtin_action_allowed(
            &entry,
            "gateway.import",
            &resolve_caller_authorization(None, AbsentAuth::Untrusted, None)
        ),
        "a requires_admin builtin must not be reachable through the in-process peer \
         on absent auth alone"
    );

    // Non-admin actions stay reachable — this closes an escalation, not the door.
    assert!(tool_execute_builtin_action_allowed(
        &entry,
        "gateway.help",
        &resolve_caller_authorization(None, AbsentAuth::Untrusted, None)
    ));

    // An explicit admin scope still works over the in-process peer, so the fix
    // does not depend on the transport when the caller actually proved scope.
    let admin = labby_auth::auth_context::AuthContext {
        sub: "admin".to_string(),
        actor_key: None,
        scopes: vec!["lab:admin".to_string()],
        issuer: "https://lab.example.com".to_string(),
        via_session: true,
        csrf_token: None,
        email: None,
    };
    assert!(tool_execute_builtin_action_allowed(
        &entry,
        "gateway.import",
        &resolve_caller_authorization(Some(&admin), AbsentAuth::Untrusted, None)
    ));
}

/// The second half of the same gap: `LOCAL_ONLY_ACTIONS` were guarded by
/// `auth.is_some()`, so the in-process peer's absent auth satisfied them too.
/// These mint credentials and probe caller-selected URLs.
#[test]
fn setup_local_only_actions_are_refused_on_the_in_process_transport() {
    let entry = RegisteredService {
        name: "setup",
        description: "Setup",
        category: "bootstrap",
        kind: crate::registry::RegisteredServiceKind::BootstrapOperator,
        status: "available",
        actions: crate::dispatch::setup::ACTIONS,
        dispatch: noop_dispatch,
    };
    let Some(local_only) = crate::dispatch::setup::LOCAL_ONLY_ACTIONS.first() else {
        return;
    };

    assert!(
        tool_execute_builtin_action_allowed(
            &entry,
            local_only,
            &resolve_caller_authorization(None, AbsentAuth::TrustedLocal, None)
        ),
        "trusted local stdio keeps its access"
    );
    assert!(
        !tool_execute_builtin_action_allowed(
            &entry,
            local_only,
            &resolve_caller_authorization(None, AbsentAuth::Untrusted, None)
        ),
        "the in-process peer must not reach a local-only setup action"
    );
}

/// The invariant: an authenticated admin is never denied a builtin, on any
/// transport, however they arrived.
///
/// Fixing the FU-1 escalation by failing closed on the in-process transport
/// also denied genuine admins reaching builtins through Code Mode — their scope
/// simply never crossed the hop. Propagating the caller's authorization in
/// request `_meta` restores them without reopening the hole.
#[test]
#[cfg(feature = "gateway")]
fn an_authenticated_admin_is_never_denied_a_builtin() {
    let entry = RegisteredService {
        name: "gateway",
        description: "Gateway",
        category: "bootstrap",
        kind: crate::registry::RegisteredServiceKind::BootstrapOperator,
        status: "available",
        actions: crate::dispatch::gateway::ACTIONS,
        dispatch: noop_dispatch,
    };
    let admin = labby_auth::auth_context::AuthContext {
        sub: "admin".to_string(),
        actor_key: None,
        scopes: vec!["lab:admin".to_string()],
        issuer: "https://lab.example.com".to_string(),
        via_session: true,
        csrf_token: None,
        email: None,
    };
    let admin_propagated =
        PropagatedCallerAuth::scoped(vec!["lab:admin".to_string()], Some("admin".to_string()));

    // Every way an admin can arrive, against an action that requires admin.
    let arrivals = [
        // Direct over MCP-over-HTTP.
        resolve_caller_authorization(Some(&admin), AbsentAuth::TrustedLocal, None),
        // Direct, on a transport that cannot vouch for absent auth — a real
        // context always wins regardless of transport.
        resolve_caller_authorization(Some(&admin), AbsentAuth::Untrusted, None),
        // Local stdio operator.
        resolve_caller_authorization(None, AbsentAuth::TrustedLocal, None),
        // Through Code Mode's in-process hop, scope propagated. This is the
        // case that regressed when the escalation was fixed.
        resolve_caller_authorization(None, AbsentAuth::Untrusted, Some(admin_propagated.clone())),
        // A local operator through the same hop.
        resolve_caller_authorization(
            None,
            AbsentAuth::Untrusted,
            Some(PropagatedCallerAuth::trusted_local()),
        ),
    ];
    for (index, caller) in arrivals.iter().enumerate() {
        assert!(caller.is_admin(), "arrival {index} should resolve as admin");
        assert!(
            tool_execute_builtin_action_allowed(&entry, "gateway.import", caller),
            "an authenticated admin was denied gateway.import via arrival {index}"
        );
    }
}

/// The other half: propagation restores admins without restoring the hole.
#[test]
#[cfg(feature = "gateway")]
fn propagation_does_not_reopen_the_escalation() {
    let entry = RegisteredService {
        name: "gateway",
        description: "Gateway",
        category: "bootstrap",
        kind: crate::registry::RegisteredServiceKind::BootstrapOperator,
        status: "available",
        actions: crate::dispatch::gateway::ACTIONS,
        dispatch: noop_dispatch,
    };

    // A lab-scoped, non-admin caller through the hop is still refused — this is
    // the FU-1 attack path.
    let non_admin = resolve_caller_authorization(
        None,
        AbsentAuth::Untrusted,
        Some(PropagatedCallerAuth::scoped(
            vec!["lab".to_string()],
            Some("mallory".to_string()),
        )),
    );
    assert!(!non_admin.is_admin());
    assert!(!tool_execute_builtin_action_allowed(
        &entry,
        "gateway.import",
        &non_admin
    ));
    // ...but keeps the access their scope does grant.
    assert!(tool_execute_builtin_action_allowed(
        &entry,
        "gateway.help",
        &non_admin
    ));

    // Missing propagation still fails closed rather than falling back to trust.
    let unknown = resolve_caller_authorization(None, AbsentAuth::Untrusted, None);
    assert!(!unknown.is_admin());
    assert!(!tool_execute_builtin_action_allowed(
        &entry,
        "gateway.import",
        &unknown
    ));
}

/// Propagated facts must never widen an authorization the caller presented.
#[test]
#[cfg(feature = "gateway")]
fn a_real_auth_context_is_never_widened_by_propagated_facts() {
    let read_only = labby_auth::auth_context::AuthContext {
        sub: "alice".to_string(),
        actor_key: None,
        scopes: vec!["lab".to_string()],
        issuer: "https://lab.example.com".to_string(),
        via_session: true,
        csrf_token: None,
        email: None,
    };
    // Even handed an admin-claiming `_meta`, a request that carried its own
    // non-admin context stays non-admin.
    let caller = resolve_caller_authorization(
        Some(&read_only),
        AbsentAuth::Untrusted,
        Some(PropagatedCallerAuth::trusted_local()),
    );
    assert!(!caller.is_admin());
    assert!(!caller.is_trusted_local());
}

/// Local-only setup actions follow the same rule: a local operator keeps them
/// through Code Mode, a remote caller never gains them.
#[test]
fn local_only_actions_track_the_propagated_locality() {
    let entry = RegisteredService {
        name: "setup",
        description: "Setup",
        category: "bootstrap",
        kind: crate::registry::RegisteredServiceKind::BootstrapOperator,
        status: "available",
        actions: crate::dispatch::setup::ACTIONS,
        dispatch: noop_dispatch,
    };
    let Some(local_only) = crate::dispatch::setup::LOCAL_ONLY_ACTIONS.first() else {
        return;
    };

    let local_via_hop = resolve_caller_authorization(
        None,
        AbsentAuth::Untrusted,
        Some(PropagatedCallerAuth::trusted_local()),
    );
    assert!(
        tool_execute_builtin_action_allowed(&entry, local_only, &local_via_hop),
        "a local operator keeps local-only actions through Code Mode"
    );

    // A remote admin does NOT get them: these are gated on locality, not scope.
    let remote_admin = resolve_caller_authorization(
        None,
        AbsentAuth::Untrusted,
        Some(PropagatedCallerAuth::scoped(
            vec!["lab:admin".to_string()],
            Some("admin".to_string()),
        )),
    );
    assert!(remote_admin.is_admin());
    assert!(
        !tool_execute_builtin_action_allowed(&entry, local_only, &remote_admin),
        "local-only actions are gated on locality, not on admin scope"
    );
}
