//! Tests for request-context auth/subject + scope/admin gate helpers.
//! Distributed from `server.rs` (bead `lab-kvji.24.1.6`).

use super::{
    actor_key_from_extensions, oauth_upstream_subject_for_request, subject_from_extensions,
    tool_execute_builtin_action_allowed, tool_execute_scope_allowed, tool_search_scope_allowed,
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
fn tool_search_allows_lab_read_but_execute_requires_lab() {
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
