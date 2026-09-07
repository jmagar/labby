use std::sync::Arc;

use axum::http::{Request, header};
use labby_auth::auth_context::AuthContext;
use labby_auth::{Authenticator, VerifiedIdentity};
use labby_primitives::product_credential::{BoundAccessGrant, ProductCredentialGrant};

use super::{skill_library_callback_boundary, skill_library_callback_correlation};
use crate::dispatch::error::ToolError;

fn identity(subject: &str, authenticator: Authenticator) -> VerifiedIdentity {
    VerifiedIdentity::external(authenticator, "https://accounts.google.com", subject)
        .expect("fixture identity")
}

fn parts(
    verified: Option<VerifiedIdentity>,
    via_session: bool,
    scopes: &[&str],
    headers: &[(&str, &str)],
) -> axum::http::request::Parts {
    let mut request = Request::builder().uri("https://lab.example/mcp");
    for (name, value) in headers {
        request = request.header(*name, *value);
    }
    let (mut parts, ()) = request.body(()).expect("request").into_parts();
    parts.extensions.insert(AuthContext {
        sub: "raw-sub-must-not-authorize".to_owned(),
        actor_key: Some(Arc::from("safe-actor")),
        scopes: scopes.iter().map(|scope| (*scope).to_owned()).collect(),
        issuer: "raw-issuer-must-not-authorize".to_owned(),
        via_session,
        csrf_token: Some("server-csrf".to_owned()),
        email: Some("private@example.test".to_owned()),
    });
    if let Some(verified) = verified {
        parts.extensions.insert(verified);
    }
    parts
}

fn assert_forbidden(error: ToolError) {
    assert!(matches!(error, ToolError::Forbidden { .. }));
}

fn product_grants() -> (VerifiedIdentity, ProductCredentialGrant, BoundAccessGrant) {
    let resource = "https://team.example.com/mcp".to_owned();
    let source = ProductCredentialGrant {
        issuer: "local".into(),
        subject: "operator-1".into(),
        credential_id: "credential-1".into(),
        credential_generation: 1,
        scopes: vec!["lab:read".into(), "lab:admin".into()],
        resource: resource.clone(),
        audience: resource.clone(),
        expires_at: 4_000_000_000,
    };
    let bound = BoundAccessGrant {
        installation_id: "installation-1".into(),
        issuer: source.issuer.clone(),
        subject: source.subject.clone(),
        principal_id: "principal-1".into(),
        organization_id: "organization-1".into(),
        project_id: "project-1".into(),
        loadout_id: "team-skills".into(),
        loadout_generation: 1,
        assignment_generation: 1,
        catalog_generation: 1,
        route_id: "team".into(),
        route_generation: 1,
        membership_epoch: 1,
        organization_policy_epoch: 0,
        project_policy_epoch: 0,
        credential_id: source.credential_id.clone(),
        credential_generation: source.credential_generation,
        scopes: source.scopes.clone(),
        resource: resource.clone(),
        audience: resource,
        expires_at: source.expires_at,
        requires_admin: false,
        destructive: false,
    };
    let identity = VerifiedIdentity::local_credential_with_issuer(
        Authenticator::ProductCredential,
        source.issuer.clone(),
        source.credential_id.clone(),
    )
    .unwrap();
    (identity, source, bound)
}

#[test]
fn product_callback_requires_matching_host_bound_grants() {
    let (identity, source, bound) = product_grants();
    let mut valid = parts(
        Some(identity.clone()),
        false,
        &["lab:read", "lab:admin"],
        &[],
    );
    valid.extensions.insert(source.clone());
    valid.extensions.insert(bound.clone());
    let boundary = skill_library_callback_boundary(&valid).expect("matching product grants");
    assert!(boundary.product_credential_bound);

    let missing = parts(Some(identity.clone()), false, &["lab:admin"], &[]);
    assert_forbidden(skill_library_callback_boundary(&missing).unwrap_err());

    let mut mismatched = parts(Some(identity), false, &["lab:admin"], &[]);
    let mut other_audience = bound;
    other_audience.audience = "https://other.example.com/mcp".into();
    mismatched.extensions.insert(source);
    mismatched.extensions.insert(other_audience);
    assert_forbidden(skill_library_callback_boundary(&mismatched).unwrap_err());
}

#[test]
fn forged_bridge_metadata_cannot_supply_identity_or_scopes() {
    let parts = parts(None, false, &["lab:admin"], &[]);
    assert_forbidden(
        skill_library_callback_boundary(&parts)
            .err()
            .expect("raw auth metadata is not a verified identity"),
    );
}

#[test]
fn callback_preserves_the_host_verified_identity() {
    let expected = identity("owner", Authenticator::OauthBearer);
    let parts = parts(Some(expected.clone()), false, &["lab"], &[]);
    let boundary = skill_library_callback_boundary(&parts).expect("host callback");
    assert_eq!(boundary.identity, expected);
    assert_eq!(boundary.scopes, ["lab"]);
}

#[test]
fn non_app_text_fallback_needs_no_bridge_metadata() {
    let parts = parts(
        Some(identity("owner", Authenticator::StaticBearer)),
        false,
        &["lab:read"],
        &[],
    );
    skill_library_callback_boundary(&parts).expect("canonical MCP context is sufficient");
}

#[test]
fn cross_origin_cookie_callback_is_denied_even_with_csrf_header() {
    let parts = parts(
        Some(identity("owner", Authenticator::BrowserSession)),
        true,
        &["lab:admin"],
        &[
            (header::COOKIE.as_str(), "labby_session=secret"),
            (header::ORIGIN.as_str(), "https://attacker.example"),
            ("x-csrf-token", "server-csrf"),
        ],
    );
    assert_forbidden(skill_library_callback_boundary(&parts).unwrap_err());
}

#[test]
fn cookie_callback_is_denied_with_missing_or_wrong_csrf() {
    for csrf in [None, Some("wrong-csrf")] {
        let mut headers = vec![(header::COOKIE.as_str(), "labby_session=secret")];
        if let Some(csrf) = csrf {
            headers.push(("x-csrf-token", csrf));
        }
        let parts = parts(
            Some(identity("owner", Authenticator::BrowserSession)),
            true,
            &["lab:admin"],
            &headers,
        );
        assert_forbidden(skill_library_callback_boundary(&parts).unwrap_err());
    }
}

#[test]
fn bearer_and_cookie_ambiguity_is_denied() {
    let parts = parts(
        Some(identity("owner", Authenticator::OauthBearer)),
        false,
        &["lab:admin"],
        &[
            (header::AUTHORIZATION.as_str(), "Bearer redacted"),
            (header::COOKIE.as_str(), "labby_session=redacted"),
        ],
    );
    assert_forbidden(skill_library_callback_boundary(&parts).unwrap_err());
}

#[test]
fn callback_error_does_not_reflect_cookie_or_identity_metadata() {
    let secret = "top-secret-cookie-value";
    let parts = parts(
        None,
        false,
        &["lab:admin"],
        &[(header::COOKIE.as_str(), secret)],
    );
    let error = skill_library_callback_boundary(&parts).unwrap_err();
    let rendered = format!("{error:?}");
    assert!(!rendered.contains(secret));
    assert!(!rendered.contains("private@example.test"));
    assert!(!rendered.contains("raw-sub-must-not-authorize"));
}

#[test]
fn unsafe_correlation_is_rejected_without_reflection() {
    let secret = "secret\nforged-log-field";
    let error = skill_library_callback_correlation(Some(secret)).unwrap_err();
    let rendered = format!("{error:?}");
    assert!(matches!(error, ToolError::InvalidParam { .. }));
    assert!(!rendered.contains(secret));
    assert!(!rendered.contains("forged-log-field"));
}

#[test]
fn stale_app_content_version_cannot_enter_list_params() {
    let stale = serde_json::json!({"content_version": "stale"});
    assert!(
        serde_json::from_value::<crate::dispatch::skill_library::params::PageParams>(stale)
            .is_err()
    );
}

#[test]
fn callback_action_catalog_has_no_app_only_or_stale_aliases() {
    let actions = crate::dispatch::skill_library::catalog::ACTIONS
        .iter()
        .map(|spec| spec.name)
        .collect::<Vec<_>>();
    assert_eq!(actions.len(), 19);
    assert!(actions.contains(&"artifacts.list"));
    assert!(!actions.contains(&"open"));
    assert!(!actions.contains(&"artifacts.open"));
}

#[test]
fn protected_route_service_mismatch_denies_the_artifacts_tool() {
    let denied = crate::mcp::route_scope::McpRouteScope::protected_subset(
        "restricted",
        std::iter::empty::<&str>(),
        ["gateway"],
        false,
    );
    let allowed = crate::mcp::route_scope::McpRouteScope::protected_subset(
        "artifacts",
        std::iter::empty::<&str>(),
        ["artifacts"],
        false,
    );
    assert!(!denied.allows_service("artifacts"));
    assert!(allowed.allows_service("artifacts"));
}

#[tokio::test]
async fn actual_http_adapter_rejects_hostile_callback_transports_with_safe_correlation() {
    use rmcp::model::{CallToolRequestParams, NumberOrString};
    use rmcp::service::{RequestContext, serve_directly};

    use crate::mcp::logging::{LoggingLevel, logging_level_rank};
    use crate::mcp::server::LabMcpServer;

    let server = LabMcpServer {
        registry: Arc::new(crate::registry::build_default_registry()),
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
        transport_label: "http",
        logging_level: Arc::new(std::sync::atomic::AtomicU8::new(logging_level_rank(
            LoggingLevel::Emergency,
        ))),
        route_scope: crate::mcp::route_scope::McpRouteScope::Root,
        relay_session_id: 0,
        code_mode_widget_callbacks_enabled_for_test: false,
    };
    let (transport, _client_transport) = tokio::io::duplex(256 * 1024);
    let running =
        serve_directly::<rmcp::RoleServer, _, _, std::io::Error, _>(server, transport, None);
    let call = || {
        CallToolRequestParams::new("artifacts").with_arguments(serde_json::Map::from_iter([
            (
                "action".to_owned(),
                serde_json::Value::String("artifacts.list".to_owned()),
            ),
            ("params".to_owned(), serde_json::json!({})),
        ]))
    };
    let cases = [
        (
            "cross-origin-cookie",
            true,
            Authenticator::BrowserSession,
            vec![
                (header::COOKIE.as_str(), "labby_session=cross-origin-secret"),
                (header::ORIGIN.as_str(), "https://attacker.example"),
                ("x-csrf-token", "server-csrf"),
                ("x-request-id", "safe-cross-origin"),
            ],
            Some("safe-cross-origin"),
        ),
        (
            "missing-csrf",
            true,
            Authenticator::BrowserSession,
            vec![
                (header::COOKIE.as_str(), "labby_session=missing-csrf-secret"),
                ("x-request-id", "safe-missing-csrf"),
            ],
            Some("safe-missing-csrf"),
        ),
        (
            "wrong-csrf",
            true,
            Authenticator::BrowserSession,
            vec![
                (header::COOKIE.as_str(), "labby_session=wrong-csrf-secret"),
                ("x-csrf-token", "wrong-csrf-secret"),
                ("x-request-id", "safe-wrong-csrf"),
            ],
            Some("safe-wrong-csrf"),
        ),
        (
            "bearer-cookie-ambiguity",
            false,
            Authenticator::OauthBearer,
            vec![
                (header::AUTHORIZATION.as_str(), "Bearer bearer-secret"),
                (header::COOKIE.as_str(), "labby_session=ambiguous-secret"),
                ("x-request-id", "../../unsafe-correlation-secret"),
            ],
            None,
        ),
    ];

    for (label, via_session, authenticator, headers, expected_correlation) in cases {
        let mut context = RequestContext::new(NumberOrString::Number(1), running.peer().clone());
        let mut request = Request::builder().uri("https://lab.example/mcp");
        for (name, value) in &headers {
            request = request.header(*name, *value);
        }
        let (mut parts, ()) = request.body(()).expect("hostile request").into_parts();
        parts.headers.insert(
            "x-labby-project-id",
            "bootstrap-default".parse().expect("project header"),
        );
        parts
            .extensions
            .insert(identity("hostile-owner", authenticator));
        parts.extensions.insert(AuthContext {
            sub: "untrusted-raw-sub".to_owned(),
            actor_key: None,
            scopes: vec!["lab:admin".to_owned()],
            issuer: "untrusted-raw-issuer".to_owned(),
            via_session,
            csrf_token: Some("server-csrf".to_owned()),
            email: Some("private@example.test".to_owned()),
        });
        context.extensions.insert(parts);

        let denied = Box::pin(running.service().call_tool_impl(call(), context))
            .await
            .expect("adapter returns a structured denial");
        assert!(denied.is_error.unwrap_or(false), "{label}: {denied:?}");
        let text = denied.content[0]
            .as_text()
            .expect("text error envelope")
            .text
            .as_str();
        let envelope: serde_json::Value =
            serde_json::from_str(text).expect("structured error envelope");
        assert_eq!(envelope["error"]["kind"], "unknown_action", "{label}");
        let correlation = envelope["error"]["correlation_id"]
            .as_str()
            .expect("client-visible correlation");
        if let Some(expected) = expected_correlation {
            assert_eq!(correlation, expected, "{label}");
        } else {
            assert!(correlation.starts_with("mcp-skill-library-rejection-"));
        }
        for secret in [
            "cross-origin-secret",
            "missing-csrf-secret",
            "wrong-csrf-secret",
            "bearer-secret",
            "ambiguous-secret",
            "unsafe-correlation-secret",
            "private@example.test",
            "untrusted-raw-sub",
        ] {
            assert!(!text.contains(secret), "{label} reflected `{secret}`");
        }
    }
}

#[tokio::test]
async fn authenticated_http_call_tool_reaches_process_library_for_read_and_mutation() {
    use std::time::Duration;

    use labby_runtime::artifacts::ArtifactStore;
    use rmcp::model::{CallToolRequestParams, NumberOrString};
    use rmcp::service::{RequestContext, serve_directly};

    use crate::access::{AccessRuntime, AccessStore, BootstrapOwnerInput};
    use crate::dispatch::skill_library::blocking::BoundedBlockingExecutor;
    use crate::dispatch::skill_library::dispatch::{
        ActivationCoordinator, ArtifactFirstPartyProjection, GenerationProjection,
        SkillLibraryService,
    };
    use crate::mcp::logging::{LoggingLevel, logging_level_rank};
    use crate::mcp::server::LabMcpServer;

    let root = tempfile::tempdir().expect("temporary Skill Library root");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        std::fs::set_permissions(root.path(), std::fs::Permissions::from_mode(0o700))
            .expect("private root");
    }
    let eli = identity("eli", Authenticator::OauthBearer);
    let pujit = identity("pujit", Authenticator::OauthBearer);
    let jake = identity("jake", Authenticator::OauthBearer);
    let access_path = root.path().join("access.db");
    let access_store = AccessStore::open(access_path.clone())
        .await
        .expect("access store");
    access_store
        .bootstrap_owner(
            BootstrapOwnerInput::new(eli.clone(), "Local", "Default").expect("owner input"),
        )
        .await
        .expect("bootstrap owner");
    access_store
        .execute_test_statement(
            "INSERT INTO principals VALUES
               ('pujit-principal','bootstrap-local','user','active','Pujit',2,2),
               ('jake-principal','bootstrap-local','user','active','Jake',2,2);
             INSERT INTO principal_links VALUES
               ('pujit-link','pujit-principal','external','https://accounts.google.com','pujit',NULL,'active',1,1,2,2),
               ('jake-link','jake-principal','external','https://accounts.google.com','jake',NULL,'active',1,1,2,2);
             INSERT INTO project_memberships VALUES
               ('pujit-membership','bootstrap-local','bootstrap-default','pujit-principal','member','active','bootstrap-owner',2,2),
               ('jake-membership','bootstrap-local','bootstrap-default','jake-principal','admin','active','bootstrap-owner',2,2);",
        )
        .await
        .expect("three canonical principals");
    drop(access_store);
    let access_runtime = Arc::new(AccessRuntime::initialize(access_path).await);

    let store =
        Arc::new(ArtifactStore::new(root.path().join("artifacts")).expect("artifact store"));
    let projection: Arc<dyn GenerationProjection<crate::skills::registry::FirstPartyGeneration>> =
        Arc::new(ArtifactFirstPartyProjection);
    let snapshot = store.library_snapshot().expect("initial library snapshot");
    let initial = projection
        .prepare(&store, &snapshot, None)
        .expect("initial generation");
    let publication = Arc::new(ActivationCoordinator::new(initial, snapshot.version));
    let service = Arc::new(SkillLibraryService::new(
        store,
        BoundedBlockingExecutor::new(2, Duration::from_secs(1), Duration::from_secs(10))
            .expect("blocking executor"),
        Arc::clone(&publication),
        projection,
    ));
    let imports = Arc::new(
        crate::dispatch::skill_library::import::ImportCoordinator::from_config(
            &crate::config::ArtifactPreferences::default(),
            &root.path().join("acquisition"),
        )
        .expect("import coordinator"),
    );
    let controls = Arc::new(
        crate::dispatch::artifact_control::ArtifactControlPlane::from_config(
            &crate::config::ArtifactPreferences::default(),
        )
        .expect("control plane"),
    );
    let runtime = Arc::new(crate::dispatch::skill_library::ProcessSkillLibraryRuntime {
        service,
        imports,
        controls,
    });
    assert!(
        crate::dispatch::skill_library::install_process_runtime(runtime).is_ok(),
        "the production process Skill Library installs once in this regression"
    );

    let server = LabMcpServer {
        registry: Arc::new(crate::registry::build_default_registry()),
        access_runtime: Arc::clone(&access_runtime),
        file_stash_runtime: Arc::new(crate::file_stash::FileStashRuntime::blocked()),
        #[cfg(feature = "gateway")]
        gateway_manager: None,
        peers: Default::default(),
        code_mode_app_state: Default::default(),
        last_listed_tool_contract: Default::default(),
        route_runtime: Default::default(),
        #[cfg(feature = "gateway")]
        client_registry: Default::default(),
        transport_label: "http",
        logging_level: Arc::new(std::sync::atomic::AtomicU8::new(logging_level_rank(
            LoggingLevel::Emergency,
        ))),
        route_scope: crate::mcp::route_scope::McpRouteScope::Root,
        relay_session_id: 0,
        code_mode_widget_callbacks_enabled_for_test: false,
    };
    let (transport, _client_transport) = tokio::io::duplex(256 * 1024);
    let running =
        serve_directly::<rmcp::RoleServer, _, _, std::io::Error, _>(server, transport, None);
    let context = |verified: &VerifiedIdentity| {
        let mut context = RequestContext::new(NumberOrString::Number(1), running.peer().clone());
        let mut request = Request::builder()
            .uri("https://lab.example/mcp")
            .header("x-labby-project-id", "bootstrap-default")
            .header(header::AUTHORIZATION, "Bearer redacted")
            .body(())
            .expect("request")
            .into_parts()
            .0;
        request.extensions.insert(verified.clone());
        request.extensions.insert(AuthContext {
            sub: "untrusted-raw-sub".to_owned(),
            actor_key: None,
            scopes: vec!["lab:admin".to_owned()],
            issuer: "untrusted-raw-issuer".to_owned(),
            via_session: false,
            csrf_token: None,
            email: None,
        });
        context.extensions.insert(request);
        context
    };
    let call = |action: &str, params: serde_json::Value| {
        CallToolRequestParams::new("artifacts").with_arguments(serde_json::Map::from_iter([
            (
                "action".to_owned(),
                serde_json::Value::String(action.to_owned()),
            ),
            ("params".to_owned(), params),
        ]))
    };
    let value = |response: &rmcp::model::CallToolResult| {
        let envelope = response.structured_content.clone().unwrap_or_else(|| {
            let text = response.content[0]
                .as_text()
                .expect("text Skill Library response")
                .text
                .as_str();
            serde_json::from_str::<serde_json::Value>(text).expect("JSON Skill Library response")
        });
        envelope.get("data").cloned().unwrap_or(envelope)
    };

    let listed = Box::pin(
        running
            .service()
            .call_tool_impl(call("artifacts.list", serde_json::json!({})), context(&eli)),
    )
    .await
    .expect("management list response");
    assert!(!listed.is_error.unwrap_or(false), "{listed:?}");

    let skill_text =
        "---\nname: mcp-production-wire\ndescription: prove MCP production wiring\n---\nbody\n";
    let support_text = "exact support bytes\n";
    let validated = Box::pin(running.service().call_tool_impl(
        call(
            "artifacts.validate",
            serde_json::json!({
                "name": "mcp-production-wire",
                "files": [
                    {"path": "SKILL.md", "content": skill_text},
                    {"path": "references/exact.md", "content": support_text}
                ]
            }),
        ),
        context(&eli),
    ))
    .await
    .expect("management validation response");
    assert!(!validated.is_error.unwrap_or(false), "{validated:?}");
    let validated = value(&validated);
    assert_eq!(validated["valid"], true);
    let expected_revision = validated["revision_id"]
        .as_str()
        .expect("validated revision")
        .to_owned();

    let created = Box::pin(running.service().call_tool_impl(
        call(
            "artifacts.create",
            serde_json::json!({
                "name": "mcp-production-wire",
                "visibility": "shared",
                "files": [{
                    "path": "SKILL.md",
                    "content": skill_text
                }, {
                    "path": "references/exact.md",
                    "content": support_text
                }],
                "expected_library_version": 0,
                "idempotency_key": "mcp-production-wire-create"
            }),
        ),
        context(&eli),
    ))
    .await
    .expect("management mutation response");
    assert!(!created.is_error.unwrap_or(false), "{created:?}");
    let created = value(&created);
    let artifact_id = created["artifact_id"]
        .as_str()
        .expect("created artifact")
        .to_owned();
    assert_eq!(created["active_revision_id"], serde_json::Value::Null);
    assert_eq!(created["canonical_uri"], serde_json::Value::Null);

    let activated = Box::pin(running.service().call_tool_impl(
        call(
            "artifacts.activate",
            serde_json::json!({
                "artifact_id": artifact_id,
                "expected_revision_id": expected_revision,
                "expected_library_version": 1,
                "idempotency_key": "mcp-production-wire-activate"
            }),
        ),
        context(&eli),
    ))
    .await
    .expect("owner activation response");
    assert!(!activated.is_error.unwrap_or(false), "{activated:?}");
    let activated = value(&activated);
    assert_eq!(activated["active_revision_id"], expected_revision);
    assert_eq!(
        activated["canonical_uri"],
        "skill://labby/mcp-production-wire/SKILL.md"
    );
    assert_eq!(activated["new_generation"], 2);

    let mut observed = Vec::new();
    for (label, principal) in [("eli", &eli), ("pujit", &pujit), ("jake", &jake)] {
        let listed = Box::pin(running.service().call_tool_impl(
            call("artifacts.list", serde_json::json!({})),
            context(principal),
        ))
        .await
        .unwrap_or_else(|error| panic!("{label} list transport: {error}"));
        assert!(!listed.is_error.unwrap_or(false), "{label}: {listed:?}");
        let listed = value(&listed);
        let item = listed["items"]
            .as_array()
            .expect("list items")
            .iter()
            .find(|item| item["artifact_id"] == artifact_id)
            .unwrap_or_else(|| panic!("{label} shared item"));

        let got = Box::pin(running.service().call_tool_impl(
            call(
                "artifacts.get",
                serde_json::json!({"artifact_id": artifact_id}),
            ),
            context(principal),
        ))
        .await
        .unwrap_or_else(|error| panic!("{label} get transport: {error}"));
        assert!(!got.is_error.unwrap_or(false), "{label}: {got:?}");
        let got = value(&got);

        let read = Box::pin(running.service().call_tool_impl(
            call(
                "artifacts.read",
                serde_json::json!({
                    "artifact_id": artifact_id,
                    "revision_id": expected_revision,
                    "path": "SKILL.md"
                }),
            ),
            context(principal),
        ))
        .await
        .unwrap_or_else(|error| panic!("{label} read transport: {error}"));
        assert!(!read.is_error.unwrap_or(false), "{label}: {read:?}");
        let read = value(&read);

        assert_eq!(item["latest_revision_id"], expected_revision, "{label}");
        assert_eq!(got["latest_revision_id"], expected_revision, "{label}");
        assert_eq!(item["active_revision_id"], expected_revision, "{label}");
        assert_eq!(got["active_revision_id"], expected_revision, "{label}");
        assert_eq!(item["published_library_version"], 2, "{label}");
        assert_eq!(got["published_library_version"], 2, "{label}");
        assert_eq!(got["current_generation"], 2, "{label}");
        assert_eq!(
            item["canonical_uri"], "skill://labby/mcp-production-wire/SKILL.md",
            "{label}"
        );
        assert_eq!(got["canonical_uri"], item["canonical_uri"], "{label}");
        assert_eq!(read["text"], skill_text, "{label}");
        assert_eq!(read["revision_id"], expected_revision, "{label}");
        observed.push((
            item["canonical_uri"].clone(),
            item["latest_revision_id"].clone(),
            item["published_library_version"].clone(),
            read["text"].clone(),
        ));
    }
    assert!(observed.windows(2).all(|pair| pair[0] == pair[1]));

    let support_uri = "skill://labby/mcp-production-wire/references/exact.md";
    let generation = publication.generation();
    let mut resource_facts = Vec::new();
    for (label, principal) in [("eli", &eli), ("pujit", &pujit), ("jake", &jake)] {
        let caller = crate::dispatch::skill_library::auth::SkillLibraryCaller::new(
            principal.clone(),
            ["lab:read".to_owned()],
            crate::dispatch::skill_library::auth::SkillLibraryTransport::bearer(
                crate::dispatch::skill_library::auth::SkillLibrarySurface::Mcp,
                true,
            ),
        );
        let decision = crate::dispatch::skill_library::auth::authorize_at_boundary(
            &access_runtime,
            caller,
            "bootstrap-default",
            crate::dispatch::skill_library::auth::SkillLibraryAction::Read,
            &crate::dispatch::skill_library::audit::CanonicalArtifactId::parse(&artifact_id)
                .expect("canonical artifact id"),
            crate::dispatch::skill_library::auth::SkillLibraryTarget::SharedActive,
            &crate::dispatch::skill_library::audit::SkillLibraryCorrelationId::parse(format!(
                "{label}-resource-read"
            ))
            .expect("resource correlation"),
        )
        .await
        .unwrap_or_else(|error| panic!("{label} resource authorization: {error}"));
        let registry =
            crate::skills::facade::SkillRegistryContext::from_generation(Arc::clone(&generation))
                .with_artifact_access(decision.artifact_access_snapshot());
        let resource = crate::mcp::handlers_resources::read_skill_resource_with_registry(
            &registry,
            support_uri,
        )
        .await
        .unwrap_or_else(|error| panic!("{label} resource adapter: {error}"));
        assert_eq!(resource.uri, support_uri, "{label}");
        assert_eq!(resource.text(), Some(support_text), "{label}");
        assert!(resource.digest.starts_with("sha256:"), "{label}");

        let compatibility_list = crate::dispatch::skills::dispatch_with_context(
            &registry,
            "skills.list",
            serde_json::json!({"limit": 100}),
        )
        .await
        .unwrap_or_else(|error| panic!("{label} compatibility list: {error}"));
        let compatibility_skill = compatibility_list["skills"]
            .as_array()
            .expect("compatibility skills")
            .iter()
            .find(|skill| skill["uri"] == "skill://labby/mcp-production-wire/SKILL.md")
            .unwrap_or_else(|| panic!("{label} compatibility shared skill"));
        let support_digest = compatibility_skill["resources"]
            .as_array()
            .expect("compatibility resources")
            .iter()
            .find(|entry| entry["uri"] == support_uri)
            .unwrap_or_else(|| panic!("{label} compatibility support resource"))["digest"]
            .clone();
        assert_eq!(support_digest, resource.digest, "{label}");

        let compatibility_get = crate::dispatch::skills::dispatch_with_context(
            &registry,
            "skills.get",
            serde_json::json!({"uri": support_uri}),
        )
        .await
        .unwrap_or_else(|error| panic!("{label} compatibility get: {error}"));
        assert_eq!(
            compatibility_get["skill"]["uri"], "skill://labby/mcp-production-wire/SKILL.md",
            "{label}"
        );
        assert_eq!(
            compatibility_get["skill"]["resources"]
                .as_array()
                .expect("get resources")
                .iter()
                .find(|entry| entry["uri"] == support_uri)
                .expect("get support resource")["digest"],
            resource.digest,
            "{label}"
        );

        for (uri, expected_text) in [
            ("skill://labby/mcp-production-wire/SKILL.md", skill_text),
            (support_uri, support_text),
        ] {
            let compatibility_read = crate::dispatch::skills::dispatch_with_context(
                &registry,
                "skills.read",
                serde_json::json!({"uri": uri}),
            )
            .await
            .unwrap_or_else(|error| panic!("{label} compatibility read: {error}"));
            assert_eq!(compatibility_read["uri"], uri, "{label}");
            assert_eq!(compatibility_read["text"], expected_text, "{label}");
            assert!(
                compatibility_read["digest"]
                    .as_str()
                    .is_some_and(|digest| digest.starts_with("sha256:")),
                "{label}"
            );
        }
        let resource_text = resource.text().unwrap().to_owned();
        resource_facts.push((resource.uri, resource.digest, resource_text));
    }
    assert!(resource_facts.windows(2).all(|pair| pair[0] == pair[1]));

    let member_denied = Box::pin(running.service().call_tool_impl(
        call(
            "artifacts.deactivate",
            serde_json::json!({
                "artifact_id": artifact_id,
                "expected_library_version": 2,
                "idempotency_key": "pujit-must-not-mutate"
            }),
        ),
        context(&pujit),
    ))
    .await
    .expect("member denial response");
    assert!(member_denied.is_error.unwrap_or(false));
    let member_denied_envelope = member_denied.structured_content.clone().unwrap_or_else(|| {
        serde_json::from_str(
            member_denied.content[0]
                .as_text()
                .expect("member denial text")
                .text
                .as_str(),
        )
        .expect("member denial envelope")
    });
    assert_eq!(member_denied_envelope["error"]["kind"], "forbidden");

    let unchanged = Box::pin(running.service().call_tool_impl(
        call(
            "artifacts.get",
            serde_json::json!({"artifact_id": artifact_id}),
        ),
        context(&eli),
    ))
    .await
    .expect("owner observes state after member denial");
    assert!(!unchanged.is_error.unwrap_or(false), "{unchanged:?}");
    let unchanged = value(&unchanged);
    assert_eq!(unchanged["active_revision_id"], expected_revision);
    assert_eq!(unchanged["latest_revision_id"], expected_revision);
    assert_eq!(unchanged["published_library_version"], 2);
    assert_eq!(unchanged["current_generation"], 2);

    let admin_deactivated = Box::pin(running.service().call_tool_impl(
        call(
            "artifacts.deactivate",
            serde_json::json!({
                "artifact_id": artifact_id,
                "expected_library_version": 2,
                "idempotency_key": "jake-admin-deactivate"
            }),
        ),
        context(&jake),
    ))
    .await
    .expect("admin mutation response");
    assert!(
        !admin_deactivated.is_error.unwrap_or(false),
        "{admin_deactivated:?}"
    );
}

#[cfg(feature = "gateway")]
#[tokio::test]
async fn explicit_mcp_action_allowlist_permits_list_and_denies_create() {
    use rmcp::model::{CallToolRequestParams, NumberOrString};
    use rmcp::service::{RequestContext, serve_directly};

    use crate::mcp::logging::{LoggingLevel, logging_level_rank};
    use crate::mcp::server::LabMcpServer;

    let manager = Arc::new(
        crate::dispatch::gateway::config_store::test_gateway_manager(
            std::path::PathBuf::from("config.toml"),
            crate::dispatch::gateway::manager::GatewayRuntimeHandle::default(),
        )
        .with_builtin_service_registry(Arc::new(crate::registry::build_default_registry())),
    );
    manager
        .seed_config_unchecked_for_tests(
            crate::config::LabConfig {
                virtual_servers: vec![crate::config::VirtualServerConfig {
                    id: "artifacts-policy".to_owned(),
                    service: "artifacts".to_owned(),
                    enabled: true,
                    surfaces: crate::config::VirtualServerSurfacesConfig {
                        mcp: true,
                        ..Default::default()
                    },
                    mcp_policy: Some(crate::config::VirtualServerMcpPolicyConfig {
                        allowed_actions: vec!["artifacts.list".to_owned()],
                    }),
                }],
                ..Default::default()
            }
            .to_gateway_config(),
        )
        .await;
    let server = LabMcpServer {
        registry: Arc::new(crate::registry::build_default_registry()),
        access_runtime: Arc::new(crate::access::AccessRuntime::blocked_unavailable()),
        file_stash_runtime: Arc::new(crate::file_stash::FileStashRuntime::blocked()),
        gateway_manager: Some(manager),
        peers: Default::default(),
        code_mode_app_state: Default::default(),
        last_listed_tool_contract: Default::default(),
        route_runtime: Default::default(),
        client_registry: Default::default(),
        transport_label: "http",
        logging_level: Arc::new(std::sync::atomic::AtomicU8::new(logging_level_rank(
            LoggingLevel::Emergency,
        ))),
        route_scope: crate::mcp::route_scope::McpRouteScope::Root,
        relay_session_id: 0,
        code_mode_widget_callbacks_enabled_for_test: false,
    };
    let (transport, _client_transport) = tokio::io::duplex(256 * 1024);
    let running =
        serve_directly::<rmcp::RoleServer, _, _, std::io::Error, _>(server, transport, None);
    let mut context = RequestContext::new(NumberOrString::Number(1), running.peer().clone());
    let mut request = Request::builder()
        .uri("https://lab.example/mcp")
        .header("x-labby-project-id", "bootstrap-default")
        .header(header::AUTHORIZATION, "Bearer redacted")
        .body(())
        .expect("request")
        .into_parts()
        .0;
    request
        .extensions
        .insert(identity("policy-owner", Authenticator::OauthBearer));
    request.extensions.insert(AuthContext {
        sub: "untrusted-sub".to_owned(),
        actor_key: None,
        scopes: vec!["lab:admin".to_owned()],
        issuer: "untrusted-issuer".to_owned(),
        via_session: false,
        csrf_token: None,
        email: None,
    });
    context.extensions.insert(request);

    assert!(
        running
            .service()
            .skill_library_http_action_allowed(&context, "artifacts.list")
            .await
    );
    assert!(
        !running
            .service()
            .skill_library_http_action_allowed(&context, "artifacts.create")
            .await
    );
    let denied = Box::pin(running.service().call_tool_impl(
        CallToolRequestParams::new("artifacts").with_arguments(serde_json::Map::from_iter([
            (
                "action".to_owned(),
                serde_json::Value::String("artifacts.create".to_owned()),
            ),
            ("params".to_owned(), serde_json::json!({})),
        ])),
        context,
    ))
    .await
    .expect("policy denial response");
    assert!(denied.is_error.unwrap_or(false));
    let text = denied.content[0]
        .as_text()
        .expect("text error")
        .text
        .as_str();
    assert!(text.contains("unknown_action"), "{text}");
}
