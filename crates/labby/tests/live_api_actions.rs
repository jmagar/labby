#![allow(clippy::panic, dead_code)]

#[path = "support/action_matrix.rs"]
mod action_matrix;
#[path = "support/action_scenarios.rs"]
mod action_scenarios;
#[path = "support/evidence.rs"]
mod evidence;
#[path = "support/live_labby.rs"]
mod live_labby;

use action_matrix::{EvidenceLevel, ScenarioKind, Surface};
use action_scenarios::{ActionOutcome, MATRIX_DEADLINE, RESPONSE_LIMIT, SECRET_CANARY};
use std::collections::{BTreeMap, BTreeSet};

async fn post_action(
    client: &reqwest::Client,
    base: &str,
    path: &str,
    action: &str,
    params: serde_json::Value,
    authorized: bool,
) -> (reqwest::StatusCode, bytes::Bytes) {
    let mut request = client
        .post(format!("{base}{path}"))
        .header("content-type", "application/json")
        .json(&serde_json::json!({"action": action, "params": params}));
    if action.starts_with("gateway.loadout.") || action.starts_with("gateway.protected_route.") {
        request = request.header("x-labby-team-id", "bootstrap-initial-team");
    }
    if authorized {
        request = request.bearer_auth(SECRET_CANARY);
    }
    let (status, bytes) = tokio::time::timeout(action_scenarios::CHILD_DEADLINE, async {
        let response = request.send().await?;
        let status = response.status();
        let bytes = response.bytes().await?;
        Ok::<_, reqwest::Error>((status, bytes))
    })
    .await
    .unwrap_or_else(|_| panic!("{action} exceeded request+body deadline"))
    .unwrap();
    action_scenarios::assert_sanitized(&bytes, action);
    (status, bytes)
}

async fn ensure_action_fixture(
    client: &reqwest::Client,
    base: &str,
    intent: &action_matrix::CaseIntent,
) {
    let prerequisite = if matches!(
        intent.action.as_str(),
        "draft.get" | "draft.commit" | "finalize"
    ) {
        Some((
            "/v1/setup",
            "draft.set",
            serde_json::json!({"entries":[{"key":"LABBY_LOG","value":"labby=debug"}]}),
        ))
    } else if matches!(intent.action.as_str(), "gateway.update" | "gateway.remove") {
        let name = if intent.action == "gateway.remove" {
            "matrix-remove-owned"
        } else {
            "matrix-owned"
        };
        Some((
            "/v1/gateway",
            "gateway.add",
            serde_json::json!({"spec":{"name":name,"url":"http://127.0.0.1:9/mcp"}}),
        ))
    } else if intent.action.starts_with("gateway.loadout.")
        && !matches!(
            intent.action.as_str(),
            "gateway.loadout.add" | "gateway.loadout.list" | "gateway.loadout.list_state"
        )
    {
        Some((
            "/v1/gateway",
            "gateway.loadout.add",
            serde_json::json!({"loadout":{"name":"matrix-owned","upstreams":["matrix-owned"],"services":[]}}),
        ))
    } else if intent.action.starts_with("gateway.protected_route.")
        && !matches!(
            intent.action.as_str(),
            "gateway.protected_route.add"
                | "gateway.protected_route.list"
                | "gateway.protected_route.list_state"
        )
    {
        Some((
            "/v1/gateway",
            "gateway.protected_route.add",
            serde_json::json!({"route":{
                "name":"matrix-owned","enabled":true,
                "public_host":"matrix.invalid","public_path":"/matrix-owned",
                "upstream":"matrix-owned","backend_url":"","scopes":[]
            }}),
        ))
    } else if matches!(
        intent.action.as_str(),
        "snippets.test" | "snippets.validate"
    ) {
        Some((
            "/v1/snippets",
            "snippets.create",
            serde_json::json!({
                "name":"matrix-owned","body":"async () => ({ ok: true })","force":true
            }),
        ))
    } else {
        None
    };
    if let Some((path, action, params)) = prerequisite {
        drop(post_action(client, base, path, action, params, true).await);
    }
}

fn seed_authority_fixtures(root: &std::path::Path) {
    use sha2::Digest as _;

    let connection = rusqlite::Connection::open(root.join("labby-home/access.db")).unwrap();
    connection
        .execute(
            "INSERT OR IGNORE INTO principals(principal_id,organization_id,kind,status,display_name,created_at,updated_at) VALUES(?1,'bootstrap-local','user','active',NULL,1,1)",
            ["matrix-principal"],
        )
        .unwrap();
    connection
        .execute(
            "INSERT OR IGNORE INTO dev_container_templates(template_id,image_digest,max_active_instances,cpu_millis,memory_bytes,disk_bytes,max_lifetime_seconds,host_capabilities_json,status,policy_epoch,created_at,updated_at) VALUES('matrix-template','sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa',32,1000,1073741824,1073741824,3600,'[]','approved',1,1,1)",
            [],
        )
        .unwrap();
    connection
        .execute(
            "INSERT OR IGNORE INTO dev_container_owner_quotas(owner_kind,owner_id,max_active_instances,policy_epoch,updated_at) VALUES('personal','bootstrap-owner',32,1,1)",
            [],
        )
        .unwrap();
    connection
        .execute(
            "INSERT OR IGNORE INTO groups(group_id,organization_id,kind,name,status,policy_epoch,membership_epoch,created_by,created_at,updated_at,deleted_at) VALUES('matrix-invite-team','bootstrap-local','team','Matrix Invite Team','active',1,1,'matrix-principal',1,1,NULL)",
            [],
        )
        .unwrap();
    let invitation_digest = sha2::Sha256::digest([0xaa; 32]);
    connection
        .execute(
            "INSERT OR IGNORE INTO team_invitations(invitation_digest,organization_id,team_id,role,invited_principal_id,inviter_principal_id,team_membership_epoch,status,accepted_principal_id,created_at,expires_at,accepted_at,revoked_at,updated_at) VALUES(?1,'bootstrap-local','matrix-invite-team','member','bootstrap-owner','matrix-principal',1,'pending',NULL,1,4102444800,NULL,NULL,1)",
            [invitation_digest.as_slice()],
        )
        .unwrap();
}

async fn prepare_authority_action(
    client: &reqwest::Client,
    base: &str,
    root: &std::path::Path,
    intent: &action_matrix::CaseIntent,
    mut params: serde_json::Value,
) -> serde_json::Value {
    let action_id = intent.action.replace('.', "-");
    if intent.service == "access" {
        if intent.action == "access.team_invitation.create" {
            params["token"] = serde_json::Value::String("b".repeat(64));
        }
        if let Some(team_id) = params.get_mut("team_id") {
            *team_id = serde_json::Value::String(format!("matrix-{action_id}"));
        }
        let team_id = params.get("team_id").and_then(serde_json::Value::as_str);
        if let Some(team_id) = team_id
            && intent.action != "access.team.create"
        {
            drop(
                post_action(
                    client,
                    base,
                    "/v1/access/admin",
                    "access.team.create",
                    serde_json::json!({"team_id":team_id,"name":format!("Matrix {action_id}")}),
                    true,
                )
                .await,
            );
        }
        if matches!(
            intent.action.as_str(),
            "access.team.member.role.set"
                | "access.team.member.suspend"
                | "access.team.member.remove"
        ) {
            drop(post_action(client, base, "/v1/access/admin", "access.team.member.add", serde_json::json!({"team_id":team_id.unwrap(),"principal_id":"matrix-principal","role":"member"}), true).await);
        }
        if intent.action == "access.team.activate" {
            drop(
                post_action(
                    client,
                    base,
                    "/v1/access/admin",
                    "access.team.suspend",
                    serde_json::json!({"team_id":team_id.unwrap()}),
                    true,
                )
                .await,
            );
        }
        if intent.action == "access.gateway_credential.revoke" {
            drop(post_action(client, base, "/v1/access/admin", "access.gateway_credential.bind", serde_json::json!({"team_id":team_id.unwrap(),"upstream_name":"matrix-upstream","binding_id":format!("binding-{action_id}")}), true).await);
        }
    } else if intent.service == "agents" {
        let agent_id = format!("matrix-{action_id}");
        params["agent_id"] = serde_json::Value::String(agent_id.clone());
        if intent.action != "agents.create" {
            let fixture = &action_scenarios::fixtures()["agents"];
            let mut create = serde_json::Map::new();
            for name in [
                "owner_kind",
                "owner_id",
                "content_digest",
                "repository_digest",
                "image_digest",
                "harness_digest",
                "loadout_digest",
                "catalog_generation",
            ] {
                create.insert(name.to_owned(), fixture.parameters[name].clone());
            }
            create.insert("agent_id".into(), agent_id.into());
            drop(
                post_action(
                    client,
                    base,
                    "/v1/agents",
                    "agents.create",
                    serde_json::Value::Object(create),
                    true,
                )
                .await,
            );
        }
        if intent.action == "agents.session.status" {
            let (_, body) = post_action(
                client,
                base,
                "/v1/agents",
                "agents.run",
                serde_json::json!({"agent_id":params["agent_id"]}),
                true,
            )
            .await;
            let response: serde_json::Value = serde_json::from_slice(&body).unwrap();
            params["session_id"] = response["session_id"].clone();
        }
    } else if intent.service == "dev_containers" {
        let instance_id = format!("matrix-{action_id}");
        params["instance_id"] = serde_json::Value::String(instance_id.clone());
        if intent.action != "dev_containers.create" {
            let (status, body) = post_action(client, base, "/v1/dev-containers", "dev_containers.create", serde_json::json!({"instance_id":instance_id,"template_id":"matrix-template","owner_kind":"personal","owner_id":"bootstrap-owner"}), true).await;
            assert!(
                status.is_success(),
                "{} create prerequisite failed: {}",
                intent.key(),
                String::from_utf8_lossy(&body)
            );
        }
        if matches!(
            intent.action.as_str(),
            "dev_containers.start" | "dev_containers.stop"
        ) {
            let state = if intent.action == "dev_containers.start" {
                "stopped"
            } else {
                "running"
            };
            let connection = rusqlite::Connection::open(root.join("labby-home/access.db")).unwrap();
            connection.execute("UPDATE dev_container_instances SET desired_state=?1,observed_state=?1 WHERE instance_id=?2", rusqlite::params![state, params["instance_id"].as_str().unwrap()]).unwrap();
        }
    } else if intent.service == "tasks" {
        let task_id = format!("matrix-{action_id}");
        let agent_id = format!("matrix-agent-{action_id}");
        params["task_id"] = serde_json::Value::String(task_id.clone());
        params["agent_id"] = serde_json::Value::String(agent_id.clone());
        let agents = &action_scenarios::fixtures()["agents"];
        let mut create_agent = serde_json::Map::new();
        for name in [
            "owner_kind",
            "owner_id",
            "content_digest",
            "repository_digest",
            "image_digest",
            "harness_digest",
            "loadout_digest",
            "catalog_generation",
        ] {
            create_agent.insert(name.to_owned(), agents.parameters[name].clone());
        }
        create_agent.insert("agent_id".into(), agent_id.into());
        drop(
            post_action(
                client,
                base,
                "/v1/agents",
                "agents.create",
                serde_json::Value::Object(create_agent),
                true,
            )
            .await,
        );
        if intent.action != "tasks.create" {
            drop(post_action(client, base, "/v1/tasks", "tasks.create", serde_json::json!({"task_id":task_id,"idempotency_key":format!("idem-{action_id}"),"owner_kind":"personal","owner_id":"bootstrap-owner","agent_id":params["agent_id"],"input_digest":"sha256:ffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff"}), true).await);
        }
        if intent.action == "tasks.result" {
            drop(
                post_action(
                    client,
                    base,
                    "/v1/tasks",
                    "tasks.queue",
                    serde_json::json!({"task_id":params["task_id"]}),
                    true,
                )
                .await,
            );
        }
    }
    params
}

#[test]
fn every_api_classification_has_exactly_one_execution_or_contract_plan() {
    let plans = action_scenarios::exact_plans(Surface::Api);
    let compiled_api_actions = action_matrix::compiled_intents()
        .filter(|intent| intent.applicable_surfaces.contains(&Surface::Api))
        .count();
    assert_eq!(plans.len(), compiled_api_actions);
}

#[tokio::test]
async fn every_api_action_reaches_live_http_or_proves_auth_denial() {
    tokio::time::timeout(MATRIX_DEADLINE, async {
        let owned_parent = std::env::temp_dir().join("labby-live-e2e");
        std::fs::create_dir_all(&owned_parent).unwrap();
        let owned_root = tempfile::Builder::new()
            .prefix("api-actions-")
            .tempdir_in(&owned_parent)
            .unwrap();
        let workspace = owned_root.path().join("workspace");
        std::fs::create_dir_all(&workspace).unwrap();
        std::fs::write(workspace.join("fixture.txt"), b"owned fixture\n").unwrap();
        let guard = live_labby::LiveLabbyBuilder::new()
            .env("LABBY_MCP_HTTP_TOKEN", SECRET_CANARY)
            .env("LABBY_E2E_BOOTSTRAP_STATIC_OWNER", "1")
            .env("LABBY_E2E_TEAM_ID", "bootstrap-initial-team")
            .env("LABBY_E2E_DETERMINISTIC_EXECUTORS", "1")
            .existing_root(owned_root.path())
            .config(format!("[workspace]\nroot = {:?}\n", workspace))
            .start()
            .await
            .expect("live API daemon");
        let client = reqwest::Client::builder().no_proxy().build().unwrap();
        seed_authority_fixtures(guard.root());
        let fixtures = action_scenarios::fixtures();
        let mut successes = BTreeSet::new();
        let mut structured_errors = BTreeSet::new();
        let mut destructive_denials = BTreeSet::new();
        let mut observed = BTreeMap::new();
        let mut outcomes = BTreeMap::new();
        for (service, path, action) in [
            ("access", "/v1/access/admin", "access.team.list"),
            ("agents", "/v1/agents", "agents.list"),
            ("tasks", "/v1/tasks", "tasks.list"),
        ] {
            let (status, body) = post_action(
                &client,
                &guard.connection().base_url,
                path,
                action,
                serde_json::json!({}),
                false,
            )
            .await;
            assert_eq!(status, reqwest::StatusCode::UNAUTHORIZED);
            let value: serde_json::Value = serde_json::from_slice(&body).unwrap();
            assert!(value.get("kind").is_some() || value["error"].get("kind").is_some());
            structured_errors.insert(service.to_owned());
        }
        let expected_api_actions = action_matrix::compiled_intents()
            .filter(|intent| intent.applicable_surfaces.contains(&Surface::Api))
            .count();
        for intent in action_matrix::compiled_intents()
            .filter(|intent| intent.applicable_surfaces.contains(&Surface::Api))
        {
            ensure_action_fixture(&client, &guard.connection().base_url, intent).await;
            let fixture = &fixtures[&intent.service];
            let Some(path) = &fixture.api_path else {
                panic!("{} missing API recipe", intent.key())
            };
            let destructive = intent.scenario_kind == ScenarioKind::DestructiveIsolated;
            let (status, bytes) = if intent.service == "fs" {
                let route = match intent.action.as_str() {
                    "fs.list" => "/v1/fs/list",
                    "schema" => "/v1/fs/list?path=../escape",
                    // help is a catalog contract and preview is owned by the
                    // browser-subject route runner; both prove the live
                    // published catalog without forging a browser subject.
                    "help" | "fs.preview" => "/v1/catalog",
                    other => panic!("unplanned fs API action {other}"),
                };
                let response = client
                    .get(format!("{}{route}", guard.connection().base_url))
                    .bearer_auth(SECRET_CANARY)
                    .send()
                    .await
                    .unwrap();
                let status = response.status();
                (status, response.bytes().await.unwrap())
            } else {
                let params = prepare_authority_action(
                    &client,
                    &guard.connection().base_url,
                    guard.root(),
                    intent,
                    action_scenarios::fixture_params(intent),
                )
                .await;
                if destructive {
                    let (denied_status, denied_body) = post_action(
                        &client,
                        &guard.connection().base_url,
                        path,
                        &intent.action,
                        params.clone(),
                        false,
                    )
                    .await;
                    if intent.service == "bundles"
                        || (intent.service == "stash" && !cfg!(target_os = "linux"))
                    {
                        assert!(
                            matches!(
                                denied_status,
                                reqwest::StatusCode::UNAUTHORIZED | reqwest::StatusCode::NOT_FOUND
                            ),
                            "{} destructive request did not fail closed before dispatch: {denied_status}",
                            intent.key()
                        );
                    } else {
                        assert_eq!(
                            denied_status,
                            reqwest::StatusCode::UNAUTHORIZED,
                            "{} destructive request was not denied before dispatch",
                            intent.key()
                        );
                    }
                    let denied: serde_json::Value = serde_json::from_slice(&denied_body).unwrap();
                    let denied_error = denied.get("error").unwrap_or(&denied);
                    assert!(denied_error.get("kind").is_some());
                    if denied_status == reqwest::StatusCode::UNAUTHORIZED {
                        destructive_denials.insert(intent.service.clone());
                    }
                }
                post_action(
                    &client,
                    &guard.connection().base_url,
                    path,
                    &intent.action,
                    params,
                    true,
                )
                .await
            };
            assert!(
                bytes.len() <= RESPONSE_LIMIT,
                "{} exceeded body cap",
                intent.key()
            );
            action_scenarios::assert_sanitized(&bytes, &intent.key());
            assert!(status.is_success() || status.is_client_error() || status.is_server_error());
            let value: serde_json::Value = serde_json::from_slice(&bytes)
                .unwrap_or_else(|error| panic!("{} non-JSON HTTP envelope: {error}", intent.key()));
            if status.is_success() {
                successes.insert(intent.service.clone());
            } else {
                let rendered = value.to_string();
                assert!(
                    rendered.contains("error")
                        || rendered.contains("kind")
                        || rendered.contains("message"),
                    "{} lost structured error metadata: {rendered}",
                    intent.key()
                );
                let error = value.get("error").unwrap_or(&value);
                assert!(
                    error.get("kind").is_some(),
                    "{} lost error kind",
                    intent.key()
                );
                assert!(
                    error.get("recovery").is_some(),
                    "{} lost recovery metadata",
                    intent.key()
                );
                assert!(
                    error.get("side_effects").is_some(),
                    "{} lost side-effect metadata",
                    intent.key()
                );
                structured_errors.insert(intent.service.clone());
            }
            assert!(observed.insert(intent.key(), status).is_none());
            let error = value.get("error").unwrap_or(&value);
            let evidence = if status.is_success() {
                match intent.scenario_kind {
                    ScenarioKind::ContractProbe => EvidenceLevel::MetadataOnly,
                    ScenarioKind::LiveInvoke => EvidenceLevel::LiveSuccess,
                    ScenarioKind::StatefulScenario | ScenarioKind::DestructiveIsolated => {
                        EvidenceLevel::LiveStateTransition
                    }
                    ScenarioKind::ConditionalOptional => EvidenceLevel::RouterReachable,
                    ScenarioKind::ExternalOptional | ScenarioKind::ExcludedWithReason => {
                        EvidenceLevel::LiveSuccess
                    }
                }
            } else {
                EvidenceLevel::LiveErrorPath
            };
            let error_kind = error
                .get("kind")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("ok");
            let dedicated =
                action_scenarios::dedicated_contract_reason_for(&intent.key(), Surface::Api)
                    .filter(|_| {
                        !status.is_success()
                            && action_scenarios::dedicated_contract_accepts_for(
                                &intent.key(),
                                Surface::Api,
                                error_kind,
                            )
                    });
            let outcome = ActionOutcome {
                key: intent.key(),
                surface: Surface::Api,
                disposition: action_scenarios::disposition(intent),
                evidence,
                owner: intent.scenario_owner,
                outcome_kind: dedicated.map_or_else(
                    || error_kind.to_string(),
                    |reason| format!("dedicated_contract:{reason}:{error_kind}"),
                ),
                recovery: error
                    .get("recovery")
                    .map(serde_json::Value::to_string)
                    .unwrap_or_else(|| "none_required".into()),
                side_effects: error
                    .get("side_effects")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or(
                        if matches!(
                            intent.scenario_kind,
                            ScenarioKind::StatefulScenario | ScenarioKind::DestructiveIsolated
                        ) {
                            "owned_state_observed_and_cleanup_registered"
                        } else {
                            "none_expected"
                        },
                    )
                    .to_string(),
                canary_free: !value.to_string().contains(SECRET_CANARY),
            };
            assert_eq!(outcome.surface, Surface::Api);
            assert_eq!(outcome.disposition, action_scenarios::disposition(intent));
            outcome.record();
            outcomes.insert(intent.key(), outcome);
        }

        assert_eq!(observed.len(), expected_api_actions);
        let insufficient = action_matrix::compiled_intents()
            .filter(|intent| intent.applicable_surfaces.contains(&Surface::Api))
            .filter(|intent| {
                let outcome = &outcomes[&intent.key()];
                !outcome.satisfies_surface(intent, Surface::Api)
                    && !(action_scenarios::dedicated_contract_reason_for(
                        &intent.key(),
                        Surface::Api,
                    )
                    .is_some()
                        && outcome.evidence == EvidenceLevel::LiveErrorPath
                        && outcome.outcome_kind.starts_with("dedicated_contract:"))
            })
            .map(|intent| {
                let outcome = &outcomes[&intent.key()];
                format!(
                    "{}={:?}:{}",
                    intent.key(),
                    outcome.evidence,
                    outcome.outcome_kind
                )
            })
            .collect::<Vec<_>>();
        assert!(
            insufficient.is_empty(),
            "API outcomes below declared minimum evidence: {insufficient:?}"
        );
        // The registered server-logs actions are all valid read-only calls, so
        // exercise its adapter's unknown-action mapping explicitly instead of
        // misclassifying a valid catalog action as the negative case.
        let (invalid_logs_status, invalid_logs_body) = post_action(
            &client,
            &guard.connection().base_url,
            "/v1/server_logs",
            "server_logs.__e2e_invalid__",
            serde_json::json!({}),
            true,
        )
        .await;
        assert!(invalid_logs_status.is_client_error());
        let invalid_logs: serde_json::Value =
            serde_json::from_slice(&invalid_logs_body).expect("structured server-logs error");
        let invalid_logs_error = invalid_logs.get("error").unwrap_or(&invalid_logs);
        assert!(invalid_logs_error.get("kind").is_some());
        assert!(invalid_logs_error.get("recovery").is_some());
        assert!(invalid_logs_error.get("side_effects").is_some());
        structured_errors.insert("server_logs".into());
        let api_services = action_scenarios::services_for(Surface::Api);
        let mut success_capable_services = api_services.clone();
        // Provider-backed control-plane services prove their fail-closed path
        // in this hermetic run; live success is covered by the synthetic
        // provider integration suite.
        for provider_backed in ["artifacts", "bundles", "jobs", "sources", "uploads"] {
            success_capable_services.remove(provider_backed);
        }
        // Stash requires a durable principal link, which this context-free
        // catalog sweep intentionally does not forge. Its Linux success path
        // is covered by the authenticated two-principal restart journey.
        success_capable_services.remove("stash");
        assert_eq!(
            successes, success_capable_services,
            "every locally self-contained API service needs a live success"
        );
        assert_eq!(
            structured_errors, api_services,
            "every API service needs an invalid/error path"
        );
        let required_destructive_denials = BTreeSet::from([
            "dev_containers".into(),
            "gateway".into(),
            "setup".into(),
            "snippets".into(),
        ]);
        assert!(
            required_destructive_denials.is_subset(&destructive_denials),
            "mounted destructive services must deny unauthenticated dispatch"
        );
        assert!(
            destructive_denials
                .difference(&required_destructive_denials)
                .all(|service| service == "bundles"),
            "only the optional provider-backed bundles service may add a denial"
        );

        // Valid reversible workflow: API create, CLI observation, API delete,
        // then CLI absence. Both surfaces share only the harness-owned install.
        let (create_status, _) = post_action(
            &client,
            &guard.connection().base_url,
            "/v1/snippets",
            "snippets.create",
            serde_json::json!({"name":"api-cli-owned","body":"async () => ({ ok: true })"}),
            true,
        )
        .await;
        assert!(create_status.is_success(), "valid snippets.create failed");
        let cli_home = guard.root().join("home");
        let labby_home = guard.root().join("labby-home");
        let cli_get = action_scenarios::run_cli_in_install(
            &cli_home,
            &labby_home,
            &["snippets", "get", "api-cli-owned", "--json"],
        )
        .await
        .unwrap();
        action_scenarios::assert_success_json(&cli_get, "cross-surface snippets.get");
        let (remove_status, _) = post_action(
            &client,
            &guard.connection().base_url,
            "/v1/snippets",
            "snippets.remove",
            serde_json::json!({"name":"api-cli-owned"}),
            true,
        )
        .await;
        assert!(remove_status.is_success(), "valid snippets.remove failed");
        let cli_absent = action_scenarios::run_cli_in_install(
            &cli_home,
            &labby_home,
            &["snippets", "get", "api-cli-owned", "--json"],
        )
        .await
        .unwrap();
        assert!(
            !cli_absent.status.success(),
            "API cleanup was not visible to CLI"
        );
        action_scenarios::assert_sanitized(&cli_absent.stderr, "cross-surface cleanup");

        let cli_create = action_scenarios::run_cli_in_install(
            &cli_home,
            &labby_home,
            &[
                "snippets",
                "create",
                "cli-api-owned",
                "--code",
                "async () => ({ ok: true })",
                "--json",
            ],
        )
        .await
        .unwrap();
        action_scenarios::assert_success_json(&cli_create, "reverse snippets.create");
        let (api_get_status, api_get_body) = post_action(
            &client,
            &guard.connection().base_url,
            "/v1/snippets",
            "snippets.get",
            serde_json::json!({"name":"cli-api-owned"}),
            true,
        )
        .await;
        assert!(api_get_status.is_success(), "CLI mutation missing from API");
        let cli_value: serde_json::Value = serde_json::from_slice(&cli_create.stdout).unwrap();
        let api_value: serde_json::Value = serde_json::from_slice(&api_get_body).unwrap();
        assert!(cli_value.to_string().contains("cli-api-owned"));
        assert!(api_value.to_string().contains("cli-api-owned"));
        let cli_remove = action_scenarios::run_cli_in_install(
            &cli_home,
            &labby_home,
            &["snippets", "remove", "cli-api-owned", "--yes", "--json"],
        )
        .await
        .unwrap();
        action_scenarios::assert_success_json(&cli_remove, "reverse snippets.remove");
        let (api_absent_status, api_absent_body) = post_action(
            &client,
            &guard.connection().base_url,
            "/v1/snippets",
            "snippets.get",
            serde_json::json!({"name":"cli-api-owned"}),
            true,
        )
        .await;
        assert!(!api_absent_status.is_success());
        let absent: serde_json::Value = serde_json::from_slice(&api_absent_body).unwrap();
        let absent_error = absent.get("error").unwrap_or(&absent);
        assert!(absent_error.get("kind").is_some());
        assert!(absent_error.get("side_effects").is_some());

        let cleanup = guard.finish().await;
        assert!(cleanup.is_clean(), "cleanup: {:?}", cleanup.failures);
    })
    .await
    .expect("API matrix absolute deadline");
}
