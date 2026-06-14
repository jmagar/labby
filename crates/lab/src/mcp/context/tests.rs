//! Tests for request-context auth/subject + scope/admin gate helpers.
//! Distributed from `server.rs` (bead `lab-kvji.24.1.6`).

#[cfg(feature = "gateway")]
use super::oauth_upstream_subject_for_request;
use super::{
    actor_key_from_extensions, code_mode_search_scope_allowed, subject_from_extensions,
    tool_execute_builtin_action_allowed, tool_execute_scope_allowed,
};
use crate::dispatch::error::ToolError;
use crate::registry::RegisteredService;
use serde_json::Value;
use std::future::Future;
use std::pin::Pin;

fn noop_dispatch(
    _action: String,
    _params: Value,
) -> Pin<Box<dyn Future<Output = Result<Value, ToolError>> + Send>> {
    Box::pin(async { Ok(Value::Null) })
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
            super::builtin_action_requires_admin(&entry, spec.name),
            "MCP admin gate must follow snippets catalog for `{}`",
            spec.name
        );
        if spec.requires_admin {
            assert!(
                !tool_execute_builtin_action_allowed(&entry, spec.name, Some(&read_only)),
                "`{}` should reject non-admin MCP callers",
                spec.name
            );
            assert!(
                tool_execute_builtin_action_allowed(&entry, spec.name, Some(&admin)),
                "`{}` should allow admin MCP callers",
                spec.name
            );
        }
    }
}

#[test]
fn marketplace_and_stash_builtin_actions_follow_catalog_admin_scope() {
    let registry = crate::registry::build_default_registry();
    let read_only = make_auth(&["lab:read"]);
    let admin = make_auth(&["lab:admin"]);

    for service_name in ["marketplace", "stash"] {
        let entry = registry
            .services()
            .iter()
            .find(|service| service.name == service_name)
            .unwrap_or_else(|| panic!("{service_name} service"));
        for spec in entry.actions {
            assert_eq!(
                spec.requires_admin,
                super::builtin_action_requires_admin(entry, spec.name),
                "MCP admin gate must follow {service_name} catalog for `{}`",
                spec.name
            );
            assert_eq!(
                spec.requires_admin,
                super::builtin_action_requires_admin(
                    entry,
                    &format!("{service_name}.{}", spec.name)
                ),
                "MCP admin gate must strip {service_name} prefix for `{}`",
                spec.name
            );
            if spec.requires_admin {
                assert!(
                    !tool_execute_builtin_action_allowed(entry, spec.name, Some(&read_only)),
                    "`{}` should reject non-admin MCP callers",
                    spec.name
                );
                assert!(
                    tool_execute_builtin_action_allowed(entry, spec.name, Some(&admin)),
                    "`{}` should allow admin MCP callers",
                    spec.name
                );
            }
        }
    }
}

#[test]
fn code_mode_scope_allows_read_but_tool_execute_does_not() {
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

    assert!(code_mode_search_scope_allowed(None));
    assert!(code_mode_search_scope_allowed(Some(&base)));
    assert!(code_mode_search_scope_allowed(Some(&lab)));
    assert!(code_mode_search_scope_allowed(Some(&admin)));
    assert!(!code_mode_search_scope_allowed(Some(&empty)));
    assert!(!code_mode_search_scope_allowed(Some(&unrelated)));

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
fn code_mode_search_scope_allowed_permits_all_expected_scopes() {
    // None = stdio transport → trusted (always permitted)
    assert!(code_mode_search_scope_allowed(None));

    // lab:read is the minimum acceptable scope for code_mode
    let read_only = make_auth(&["lab:read"]);
    assert!(code_mode_search_scope_allowed(Some(&read_only)));

    // bare lab must also pass code_mode
    let lab = make_auth(&["lab"]);
    assert!(code_mode_search_scope_allowed(Some(&lab)));

    // lab:admin must pass code_mode (identified as a gap in the original review)
    let admin = make_auth(&["lab:admin"]);
    assert!(code_mode_search_scope_allowed(Some(&admin)));

    // empty scopes → denied
    let no_scopes = make_auth(&[]);
    assert!(!code_mode_search_scope_allowed(Some(&no_scopes)));

    // unrelated scope → denied
    let unrelated = make_auth(&["mcp:read"]);
    assert!(!code_mode_search_scope_allowed(Some(&unrelated)));
}

#[test]
fn code_mode_allows_lab_read_but_execute_requires_lab() {
    // Intentional asymmetry: code_mode is a read-only discovery operation and therefore
    // accepts lab:read in addition to the stronger lab / lab:admin.
    // tool_execute must NOT accept lab:read — it executes upstream tools
    // which may have side effects.
    let read_only = make_auth(&["lab:read"]);

    // code_mode: lab:read is permitted
    assert!(
        code_mode_search_scope_allowed(Some(&read_only)),
        "code_mode should accept lab:read"
    );

    // tool_execute: lab:read must NOT be sufficient
    assert!(
        !tool_execute_scope_allowed(Some(&read_only)),
        "tool_execute must reject lab:read — requires lab or lab:admin"
    );
}
