#![cfg(feature = "gateway")]
#![allow(clippy::panic, dead_code)]

#[path = "support/action_matrix.rs"]
mod action_matrix;
#[path = "support/action_scenarios.rs"]
mod action_scenarios;
#[path = "support/evidence.rs"]
mod evidence;
#[path = "support/live_identity.rs"]
mod live_identity;
#[path = "support/live_labby.rs"]
mod live_labby;
#[path = "support/mcp_action_runner.rs"]
mod mcp_action_runner;

mod support {
    pub(crate) use crate::live_labby::{
        CleanupResult, LiveLabbyBuilder, LiveLabbyGuard, isolated_command,
    };
}

use std::collections::{BTreeMap, BTreeSet};

use action_matrix::{EvidenceLevel, ScenarioKind, Surface, compiled_intents, intents};
use mcp_action_runner::BuiltinMcpRunner;

const ACTION_CATALOG: &str = include_str!("../../../docs/generated/action-catalog.json");

fn mcp_intents() -> Vec<&'static action_matrix::CaseIntent> {
    compiled_intents()
        .filter(|intent| intent.applicable_surfaces.contains(&Surface::Mcp))
        .collect()
}

fn expected_service_tools() -> BTreeSet<String> {
    mcp_intents()
        .into_iter()
        .filter(|intent| cfg!(feature = "fs") || intent.service != "fs")
        .map(|intent| intent.service.clone())
        .collect()
}

fn result_text(result: &rmcp::model::CallToolResult) -> String {
    result
        .content
        .iter()
        .filter_map(|content| content.as_text().map(|text| text.text.as_str()))
        .collect::<Vec<_>>()
        .join("\n")
}

fn result_error_kind(text: &str) -> Option<String> {
    fn find_kind(value: &serde_json::Value) -> Option<&str> {
        value
            .get("kind")
            .and_then(serde_json::Value::as_str)
            .or_else(|| {
                value
                    .as_object()
                    .and_then(|object| object.values().find_map(find_kind))
            })
    }

    serde_json::from_str(text)
        .ok()
        .as_ref()
        .and_then(find_kind)
        .map(str::to_owned)
}

async fn assert_mcp_transition_readback(
    runner: &BuiltinMcpRunner,
    intent: &action_matrix::CaseIntent,
    mutation_text: &str,
) -> bool {
    let key = intent.key();
    let key_for_read = key.as_str();
    let read = |service: &'static str, action: &'static str, params: serde_json::Value| async move {
        let result = runner
            .call(
                service,
                action,
                params.as_object().cloned().expect("readback params object"),
            )
            .await
            .unwrap_or_else(|error| panic!("{key_for_read} readback wire failure: {error}"));
        (result.is_error != Some(true), result_text(&result))
    };

    match key.as_str() {
        "gateway:gateway.add" | "gateway:gateway.update" => {
            let (ok, text) = read(
                "gateway",
                "gateway.get",
                serde_json::json!({"name":"matrix-owned"}),
            )
            .await;
            assert!(
                ok && text.contains("matrix-owned"),
                "{key} readback: {text}"
            );
            if key.ends_with("update") {
                assert!(
                    text.contains("127.0.0.1:10"),
                    "{key} patch was not observable: {text}"
                );
            }
            true
        }
        "gateway:gateway.code_mode.set" => {
            let (ok, text) = read("gateway", "gateway.code_mode.get", serde_json::json!({})).await;
            assert!(ok && !text.trim().is_empty(), "{key} readback: {text}");
            assert_eq!(
                result_error_kind(mutation_text),
                None,
                "{key} returned an error-shaped success"
            );
            true
        }
        "gateway:gateway.loadout.add"
        | "gateway:gateway.loadout.patch"
        | "gateway:gateway.loadout.update" => {
            let (ok, text) = read(
                "gateway",
                "gateway.loadout.get",
                serde_json::json!({"name":"matrix-owned"}),
            )
            .await;
            assert!(
                ok && text.contains("matrix-owned"),
                "{key} readback: {text}"
            );
            true
        }
        "gateway:gateway.loadout.remove" => {
            let (ok, text) = read(
                "gateway",
                "gateway.loadout.get",
                serde_json::json!({"name":"matrix-owned"}),
            )
            .await;
            assert!(
                !ok && text.contains("not_found"),
                "{key} absence proof: {text}"
            );
            true
        }
        "gateway:gateway.remove" => {
            let (ok, text) = read(
                "gateway",
                "gateway.get",
                serde_json::json!({"name":"matrix-remove-owned"}),
            )
            .await;
            assert!(
                !ok && text.contains("not_found"),
                "{key} absence proof: {text}"
            );
            true
        }
        "gateway:gateway.mcp.disable"
        | "gateway:gateway.mcp.enable"
        | "gateway:gateway.mcp.restart" => {
            let (ok, text) = read(
                "gateway",
                "gateway.get",
                serde_json::json!({"name":"matrix-owned"}),
            )
            .await;
            assert!(
                ok && text.contains("matrix-owned"),
                "{key} readback: {text}"
            );
            true
        }
        "gateway:gateway.protected_route.add" | "gateway:gateway.protected_route.update" => {
            let (ok, text) = read(
                "gateway",
                "gateway.protected_route.get",
                serde_json::json!({"name":"matrix-owned"}),
            )
            .await;
            assert!(
                ok && text.contains("matrix-owned"),
                "{key} readback: {text}"
            );
            true
        }
        "gateway:gateway.protected_route.remove" => {
            let (ok, text) = read(
                "gateway",
                "gateway.protected_route.get",
                serde_json::json!({"name":"matrix-owned"}),
            )
            .await;
            assert!(
                !ok && text.contains("not_found"),
                "{key} absence proof: {text}"
            );
            true
        }
        "gateway:gateway.reload" => {
            assert!(
                mutation_text.contains("completed") && mutation_text.contains("true"),
                "{key} did not prove reload completion: {mutation_text}"
            );
            true
        }
        "snippets:snippets.create" => {
            let (ok, text) = read(
                "snippets",
                "snippets.get",
                serde_json::json!({"name":"matrix-owned"}),
            )
            .await;
            assert!(
                ok && text.contains("matrix-owned"),
                "{key} readback: {text}"
            );
            true
        }
        "snippets:snippets.remove" => {
            let (ok, text) = read(
                "snippets",
                "snippets.get",
                serde_json::json!({"name":"matrix-owned"}),
            )
            .await;
            assert!(
                !ok && text.contains("not_found"),
                "{key} absence proof: {text}"
            );
            true
        }
        "setup:draft.set" => {
            let (ok, text) = read("setup", "draft.get", serde_json::json!({})).await;
            assert!(ok && text.contains("LABBY_LOG"), "{key} readback: {text}");
            true
        }
        "setup:draft.discard" => {
            let (ok, text) = read("setup", "draft.get", serde_json::json!({})).await;
            assert!(
                ok && !text.contains("LABBY_LOG"),
                "{key} absence proof: {text}"
            );
            true
        }
        "setup:draft.commit" | "setup:finalize" | "setup:repair" | "setup:settings.update" => {
            let (ok, text) = read("setup", "state", serde_json::json!({})).await;
            assert!(ok && !text.trim().is_empty(), "{key} readback: {text}");
            true
        }
        "setup:plugin_hook" | "setup:plugin_sync" => {
            let (ok, text) = read("setup", "plugin_export", serde_json::json!({})).await;
            assert!(ok && !text.trim().is_empty(), "{key} readback: {text}");
            true
        }
        _ => false,
    }
}

async fn prepare_mcp_transition(runner: &BuiltinMcpRunner, intent: &action_matrix::CaseIntent) {
    let prerequisite = match intent.key().as_str() {
        "setup:draft.commit" => Some((
            "draft.set",
            serde_json::json!({"entries": [{
                "key": "LABBY_LOG",
                "value": "labby=debug"
            }]}),
        )),
        "gateway:gateway.remove" => Some((
            "gateway.add",
            serde_json::json!({"spec": {
                "name": "matrix-remove-owned",
                "url": "http://127.0.0.1:9/mcp"
            }}),
        )),
        "gateway:gateway.loadout.update" => Some((
            "gateway.loadout.add",
            serde_json::json!({"loadout": {
                "name": "matrix-owned",
                "upstreams": ["matrix-owned"],
                "services": []
            }}),
        )),
        "gateway:gateway.protected_route.update" => Some((
            "gateway.protected_route.add",
            serde_json::json!({"route": {
                "name": "matrix-owned",
                "enabled": true,
                "public_host": "matrix.invalid",
                "public_path": "/matrix-owned",
                "upstream": "matrix-owned",
                "backend_url": "",
                "scopes": []
            }}),
        )),
        _ => None,
    };
    let Some((action, params)) = prerequisite else {
        return;
    };
    let service = if intent.service == "setup" {
        "setup"
    } else {
        "gateway"
    };
    let result = runner
        .call(
            service,
            action,
            params.as_object().cloned().expect("prerequisite params"),
        )
        .await
        .unwrap_or_else(|error| panic!("{} prerequisite wire failure: {error}", intent.key()));
    assert_ne!(
        result.is_error,
        Some(true),
        "{} prerequisite failed: {}",
        intent.key(),
        result_text(&result)
    );
}

#[test]
fn every_mcp_visible_classification_has_one_bounded_execution_plan() {
    let cases = mcp_intents();
    let case_count = cases.len();
    let mut plans = BTreeMap::new();
    for intent in cases {
        let disposition = match intent.scenario_kind {
            ScenarioKind::ContractProbe => "metadata_probe",
            ScenarioKind::LiveInvoke => "live_success_or_stable_error",
            ScenarioKind::StatefulScenario => "isolated_workflow",
            ScenarioKind::DestructiveIsolated => "confirmation_bound_workflow",
            ScenarioKind::ConditionalOptional => "conditional_http_subject",
            ScenarioKind::ExternalOptional => "offline_error_path",
            ScenarioKind::ExcludedWithReason => "reviewed_exclusion",
        };
        assert!(plans.insert(intent.key(), disposition).is_none());
        assert!(!intent.scenario_id.is_empty());
        assert!(!intent.fixture_params.fixture.is_empty());
    }
    assert_eq!(plans.len(), case_count);
}

#[test]
fn mcp_projection_and_security_axes_are_derived_from_canonical_metadata() {
    let catalog: Vec<action_matrix::CatalogAction> = serde_json::from_str(ACTION_CATALOG).unwrap();
    let catalog = action_matrix::catalog_map(&catalog).unwrap();
    for intent in mcp_intents() {
        let action = catalog[&intent.key()];
        assert!(action.surface_availability.mcp);
        if action.requires_admin {
            assert_eq!(action.required_scopes, ["lab:admin"]);
        }
        if action.destructive {
            let canonical = intent.canonical_action.as_ref().map_or(intent, |key| {
                intents().iter().find(|case| case.key() == *key).unwrap()
            });
            assert_eq!(canonical.scenario_kind, ScenarioKind::DestructiveIsolated);
        }
    }
}

#[tokio::test]
async fn raw_mode_catalog_is_exact_and_builtin_help_executes_live() {
    let runner = BuiltinMcpRunner::start().await.expect("live MCP runner");
    let fingerprint = runner.identity_fingerprint();
    assert_eq!(fingerprint.len(), 64);
    let advertised = runner.list_tool_names().await.expect("bounded tools/list");
    let all_services = expected_service_tools();
    let mut expected = all_services
        .iter()
        .filter(|service| {
            !matches!(
                service.as_str(),
                "lab_admin" | "bundles" | "jobs" | "sources" | "uploads"
            )
        })
        .cloned()
        .collect::<BTreeSet<_>>();
    if !cfg!(target_os = "linux") {
        expected.remove("stash");
    }
    let advertised_services = advertised
        .intersection(&all_services)
        .cloned()
        .collect::<BTreeSet<_>>();
    assert_eq!(advertised_services, expected);
    assert!(
        !advertised.contains("lab_admin"),
        "local-only tool leaked over HTTP MCP"
    );
    for unexpected in ["acp", "deploy", "fleet", "marketplace", "registry"] {
        assert!(!advertised.contains(unexpected));
    }
    for service in expected {
        let result = runner
            .call(&service, "help", serde_json::Map::new())
            .await
            .unwrap_or_else(|error| panic!("{service}.help: {error}"));
        assert_ne!(
            result.is_error,
            Some(true),
            "{service}.help failed: {}",
            result_text(&result)
        );
    }
    let cleanup = runner.finish().await;
    assert!(cleanup.is_clean(), "cleanup: {:?}", cleanup.failures);
}

#[tokio::test]
async fn code_mode_hides_raw_service_tools_without_testing_code_mode_primitives() {
    let runner = BuiltinMcpRunner::start_code_mode()
        .await
        .expect("Code Mode MCP runner");
    let advertised = runner.list_tool_names().await.expect("bounded tools/list");
    let services = expected_service_tools();
    let visible_services = advertised
        .intersection(&services)
        .cloned()
        .collect::<BTreeSet<_>>();
    assert_eq!(
        visible_services,
        BTreeSet::from(["server_logs".to_string()])
    );
    assert!(advertised.contains("codemode"));
    let hidden = runner
        .call("doctor", "help", serde_json::Map::new())
        .await
        .expect("hidden execution returns a protocol result");
    assert_eq!(hidden.is_error, Some(true));
    assert!(result_text(&hidden).contains("hidden"));
    let cleanup = runner.finish().await;
    assert!(cleanup.is_clean(), "cleanup: {:?}", cleanup.failures);
}

#[tokio::test]
async fn live_errors_are_structured_redacted_and_terminal() {
    let runner = BuiltinMcpRunner::start().await.expect("live MCP runner");
    let unknown = runner
        .call("doctor", "definitely.unknown", serde_json::Map::new())
        .await
        .unwrap();
    assert_eq!(unknown.is_error, Some(true));
    let unknown_text = result_text(&unknown);
    assert!(unknown_text.contains("unknown_action"));
    assert!(unknown_text.contains("valid"));

    let mut invalid = serde_json::Map::new();
    invalid.insert("action".into(), serde_json::Value::Bool(true));
    let invalid = runner.call("doctor", "schema", invalid).await.unwrap();
    assert_eq!(invalid.is_error, Some(true));
    let text = result_text(&invalid).to_ascii_lowercase();
    assert!(text.contains("invalid_param") || text.contains("validation"));
    assert!(!text.contains("live-mcp-action-matrix-token"));

    let cleanup = runner.finish().await;
    assert!(cleanup.is_clean(), "cleanup: {:?}", cleanup.failures);
}

#[tokio::test]
async fn every_http_feasible_surface_action_reaches_live_dispatch() {
    let runner = BuiltinMcpRunner::start().await.expect("live MCP runner");
    let expected = mcp_intents()
        .into_iter()
        // lab_admin is intentionally local-only and therefore cannot be
        // exercised through the HTTP MCP route owned by this runner.
        .filter(|intent| intent.service != "lab_admin")
        .collect::<Vec<_>>();
    let expected_count = expected.len();

    let mut consumed = BTreeSet::new();
    for intent in expected {
        prepare_mcp_transition(&runner, intent).await;
        let params = action_scenarios::fixture_params(intent)
            .as_object()
            .cloned()
            .expect("fixture params are an object");
        let result = runner
            .call(&intent.service, &intent.action, params)
            .await
            .unwrap_or_else(|error| panic!("{} wire failure: {error}", intent.key()));
        let text = result_text(&result);
        assert!(
            result.is_error != Some(true) || !text.trim().is_empty(),
            "{} returned an empty error envelope",
            intent.key()
        );
        assert!(
            !text.contains("live-mcp-action-matrix-token"),
            "{} reflected the bearer secret",
            intent.key()
        );
        let succeeded = result.is_error != Some(true);
        let transition_observed = succeeded
            && matches!(
                intent.scenario_kind,
                ScenarioKind::StatefulScenario | ScenarioKind::DestructiveIsolated
            )
            && assert_mcp_transition_readback(&runner, intent, &text).await;
        let evidence = if succeeded {
            match intent.scenario_kind {
                ScenarioKind::ContractProbe => EvidenceLevel::MetadataOnly,
                ScenarioKind::LiveInvoke => EvidenceLevel::LiveSuccess,
                ScenarioKind::StatefulScenario | ScenarioKind::DestructiveIsolated => {
                    if transition_observed {
                        EvidenceLevel::LiveStateTransition
                    } else {
                        EvidenceLevel::LiveSuccess
                    }
                }
                ScenarioKind::ConditionalOptional => EvidenceLevel::RouterReachable,
                ScenarioKind::ExternalOptional | ScenarioKind::ExcludedWithReason => {
                    EvidenceLevel::LiveSuccess
                }
            }
        } else {
            EvidenceLevel::LiveErrorPath
        };
        let error_kind = (!succeeded)
            .then(|| result_error_kind(&text))
            .flatten()
            .unwrap_or_else(|| "mcp_error".to_owned());
        let dedicated = (!succeeded)
            .then(|| action_scenarios::dedicated_contract_reason_for(&intent.key(), Surface::Mcp))
            .flatten()
            .filter(|_| {
                action_scenarios::dedicated_contract_accepts_for(
                    &intent.key(),
                    Surface::Mcp,
                    &error_kind,
                )
            });
        action_scenarios::ActionOutcome {
            key: intent.key(),
            surface: Surface::Mcp,
            disposition: action_scenarios::disposition(intent),
            evidence,
            owner: intent.scenario_owner,
            outcome_kind: dedicated.map_or_else(
                || {
                    if succeeded {
                        "live_result"
                    } else {
                        &error_kind
                    }
                    .to_owned()
                },
                |reason| format!("dedicated_contract:{reason}:{error_kind}"),
            ),
            recovery: "isolated_runner".into(),
            side_effects: if succeeded {
                "owned_state"
            } else {
                "none_observed"
            }
            .into(),
            canary_free: !text.contains(action_scenarios::SECRET_CANARY),
        }
        .record();
        assert!(consumed.insert(intent.key()), "duplicate action execution");
    }
    assert_eq!(consumed.len(), expected_count);

    let cleanup = runner.finish().await;
    assert!(cleanup.is_clean(), "cleanup: {:?}", cleanup.failures);
}

#[cfg(feature = "lab-admin")]
#[tokio::test]
#[cfg(feature = "lab-admin")]
async fn local_stdio_executes_all_lab_admin_intents_before_recording_evidence() {
    let root = tempfile::tempdir().expect("local stdio MCP root");
    std::fs::create_dir_all(root.path().join("tmp")).unwrap();
    let mut command = support::isolated_command(root.path());
    command.env("LABBY_ADMIN_ENABLED", "1").arg("mcp");
    let runner = BuiltinMcpRunner::start_stdio(command)
        .await
        .expect("local stdio MCP runner");
    let tools = runner.list_tool_names().await.expect("stdio tools/list");
    assert!(
        tools.contains("lab_admin"),
        "local-only tool missing: {tools:?}"
    );

    let intents = mcp_intents()
        .into_iter()
        .filter(|intent| intent.service == "lab_admin")
        .collect::<Vec<_>>();
    let intent_count = intents.len();
    assert_eq!(intents.len(), 3);
    let mut consumed = BTreeSet::new();
    for intent in intents {
        let params = action_scenarios::fixture_params(intent)
            .as_object()
            .cloned()
            .expect("fixture params are an object");
        let result = runner
            .call("lab_admin", &intent.action, params)
            .await
            .unwrap_or_else(|error| panic!("{} stdio wire failure: {error}", intent.key()));
        let text = result_text(&result);
        assert_ne!(
            result.is_error,
            Some(true),
            "{} stdio execution failed: {text}",
            intent.key()
        );
        assert!(
            !text.trim().is_empty(),
            "{} returned no evidence",
            intent.key()
        );
        assert!(!text.contains(action_scenarios::SECRET_CANARY));

        action_scenarios::ActionOutcome {
            key: intent.key(),
            surface: Surface::Mcp,
            disposition: action_scenarios::disposition(intent),
            evidence: match intent.scenario_kind {
                ScenarioKind::ContractProbe => EvidenceLevel::MetadataOnly,
                ScenarioKind::StatefulScenario | ScenarioKind::DestructiveIsolated => {
                    EvidenceLevel::LiveSuccess
                }
                _ => EvidenceLevel::LiveSuccess,
            },
            owner: intent.scenario_owner,
            outcome_kind: "live_stdio_result".into(),
            recovery: "isolated_stdio_runner".into(),
            side_effects: "owned_state".into(),
            canary_free: true,
        }
        .record();
        assert!(consumed.insert(intent.key()), "duplicate stdio execution");
    }
    assert_eq!(consumed.len(), intent_count);
    let cleanup = runner.finish().await;
    assert!(cleanup.is_clean(), "cleanup: {:?}", cleanup.failures);
}

#[tokio::test]
async fn project_bound_non_admin_identity_narrows_discovery_and_denies_execution() {
    let identity = live_identity::LiveIdentity::bootstrap("mcp-matrix-non-admin")
        .await
        .expect("public identity bootstrap");
    let tuple = mcp_action_runner::IdentityTuple::from_public(&identity.identity);
    let fingerprint = tuple.fingerprint();
    let missing = BuiltinMcpRunner::connect_project(identity.base(), "", tuple.clone()).await;
    assert!(
        missing.is_err(),
        "missing credential must fail initialization"
    );
    let runner = BuiltinMcpRunner::connect_project(
        identity.base(),
        identity.credential_for_request(),
        tuple,
    )
    .await
    .expect("project-bound MCP client");
    assert_eq!(runner.identity_fingerprint(), fingerprint);

    let tools = runner.list_tool_names().await.expect("scoped tools/list");
    // This Loadout has no upstreams. The protected gateway-subset route must
    // therefore reveal no raw operator service tools at all.
    assert_eq!(tools, BTreeSet::from(["gateway".to_string()]));
    assert!(!tools.contains("setup"));
    assert!(!tools.contains("lab_admin"));

    let denied = runner
        .call("setup", "state", serde_json::Map::new())
        .await
        .expect("hidden execution returns an MCP result");
    assert_eq!(denied.is_error, Some(true));
    let denial = result_text(&denied);
    let denial_kind = denial.to_ascii_lowercase();
    assert!(
        denial_kind.contains("hidden")
            || denial_kind.contains("scope")
            || denial_kind.contains("unknown")
            || denial_kind.contains("not_found"),
        "unexpected non-enumerating denial: {denial}"
    );
    assert!(!denial.contains(identity.credential_for_request()));

    runner.disconnect().await;
    let cleanup = identity.cleanup().await.expect("identity cleanup");
    assert!(cleanup.is_clean(), "cleanup: {:?}", cleanup.failures);
}

#[tokio::test]
async fn read_only_non_admin_discovers_mixed_service_but_cannot_execute_admin_action() {
    let setup_policy = live_identity::policy(&["lab:read"])
        .replace("services = [\"gateway\"]", "services = [\"setup\"]");
    let identity = live_identity::LiveIdentity::bootstrap_with_policy(
        "mcp-matrix-read-only",
        300,
        &setup_policy,
    )
    .await
    .expect("public read-only identity bootstrap");
    let tuple = mcp_action_runner::IdentityTuple::from_public(&identity.identity);
    assert!(!tuple.scopes.iter().any(|scope| scope == "lab:admin"));
    let runner = BuiltinMcpRunner::connect_project(
        identity.base(),
        identity.credential_for_request(),
        tuple,
    )
    .await
    .expect("read-only protected MCP client");

    let tools = runner.list_tool_names().await.expect("scoped tools/list");
    assert!(
        tools.contains("setup"),
        "mixed-scope setup tool must be visible"
    );
    let setup_contract = runner
        .tool_contract("setup")
        .await
        .expect("setup descriptor")
        .expect("visible setup descriptor");
    assert!(
        !setup_contract.contains(identity.credential_for_request()),
        "discovery must not reflect the credential"
    );

    let denied = runner
        .call("setup", "services.status", serde_json::Map::new())
        .await
        .expect("scope denial is an MCP result");
    assert_eq!(denied.is_error, Some(true));
    let denial = result_text(&denied).to_ascii_lowercase();
    assert!(
        denial.contains("forbidden")
            || denial.contains("scope")
            || denial.contains("admin")
            || denial.contains("not_found"),
        "unexpected scope denial: {denial}"
    );
    assert!(!denial.contains(identity.credential_for_request()));

    runner.disconnect().await;
    let cleanup = identity.cleanup().await.expect("identity cleanup");
    assert!(cleanup.is_clean(), "cleanup: {:?}", cleanup.failures);
}
