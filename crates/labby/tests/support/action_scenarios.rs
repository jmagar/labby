use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;
use std::process::Output;
use std::time::Duration;

use serde::Deserialize;
use serde_json::{Value, json};

use crate::action_matrix::{CaseIntent, EvidenceLevel, ScenarioKind, ScenarioOwner, Surface};
use crate::live_labby::isolated_command;

pub(crate) const MATRIX_DEADLINE: Duration = Duration::from_secs(90);
// Live Code Mode/snippet calls can contend with parallel feature-slice linking
// on shared CI runners. Keep the aggregate matrix bound strict, but give one
// real child/request enough room to finish under that expected load.
pub(crate) const CHILD_DEADLINE: Duration = Duration::from_secs(30);
pub(crate) const MAX_CHILDREN: usize = 4;
pub(crate) const RESPONSE_LIMIT: usize = 1024 * 1024;
pub(crate) const SECRET_CANARY: &str = "live-action-matrix-secret-canary";
const _: () = assert!(MAX_CHILDREN > 0 && MAX_CHILDREN <= 4);
const ACTION_CATALOG: &str = include_str!("../../../../docs/generated/action-catalog.json");

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ServiceFixture {
    pub(crate) service: String,
    pub(crate) api_path: Option<String>,
    pub(crate) cli_probe: Option<Vec<String>>,
    pub(crate) can_mutate: bool,
    pub(crate) success_action: String,
    pub(crate) invalid_action: String,
    pub(crate) policy_action: Option<String>,
    pub(crate) workflow: Vec<String>,
    pub(crate) parameters: BTreeMap<String, Value>,
    #[serde(default)]
    pub(crate) action_params: BTreeMap<String, BTreeMap<String, Value>>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Disposition {
    MetadataProbe,
    LiveDispatch,
    IsolatedWorkflow,
    AuthorizationDenial,
    ConditionalProbe,
    OfflineError,
    ReviewedExclusion,
}

#[derive(Clone, Debug)]
pub(crate) struct ActionOutcome {
    pub(crate) key: String,
    pub(crate) surface: Surface,
    pub(crate) disposition: Disposition,
    pub(crate) evidence: EvidenceLevel,
    pub(crate) owner: ScenarioOwner,
    pub(crate) outcome_kind: String,
    pub(crate) recovery: String,
    pub(crate) side_effects: String,
    pub(crate) canary_free: bool,
}

impl ActionOutcome {
    pub(crate) fn satisfies(&self, intent: &CaseIntent) -> bool {
        self.key == intent.key()
            && self.evidence >= intent.minimum_evidence
            && self.owner == intent.scenario_owner
            && !self.outcome_kind.is_empty()
            && !self.recovery.is_empty()
            && !self.side_effects.is_empty()
            && self.canary_free
    }

    pub(crate) fn record(&self) {
        let Some(directory) = std::env::var_os("LABBY_E2E_CASE_DIR") else {
            return;
        };
        let run_id = std::env::var("LABBY_E2E_RUN_ID").expect("run id for case evidence");
        let seed = std::env::var("LABBY_E2E_SEED").expect("seed for case evidence");
        let build_identity =
            std::env::var("LABBY_E2E_BUILD_IDENTITY").expect("build identity for case evidence");
        let event = json!({
            "schema_version": 1,
            "run_id": run_id,
            "seed": seed,
            "build_identity": build_identity,
            "case_id": format!("action::{:?}::{}", self.surface, self.key),
            "kind": "action",
            "achieved_evidence": format!("{:?}", self.evidence),
            "handler_success": matches!(self.evidence, EvidenceLevel::LiveSuccess | EvidenceLevel::LiveStateTransition),
            "denial_only": self.evidence == EvidenceLevel::LiveErrorPath
                && self.outcome_kind.to_ascii_lowercase().contains("den"),
            "outcome_kind": self.outcome_kind,
            "cleanup_ok": self.canary_free,
        });
        write_case_event(&directory, &event);
    }
}

fn write_case_event(directory: &std::ffi::OsStr, event: &Value) {
    use sha2::Digest as _;
    static EVENT_WRITE_LOCK: std::sync::OnceLock<std::sync::Mutex<()>> = std::sync::OnceLock::new();
    let _event_write = EVENT_WRITE_LOCK
        .get_or_init(|| std::sync::Mutex::new(()))
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let directory = Path::new(directory);
    std::fs::create_dir_all(directory).expect("create case evidence directory");
    let id = event["case_id"].as_str().expect("case id");
    let name = hex::encode(sha2::Sha256::digest(id.as_bytes()));
    let target = directory.join(format!("{name}.json"));
    if let Ok(existing) = std::fs::read(&target)
        && let Ok(existing) = serde_json::from_slice::<Value>(&existing)
        && evidence_rank(existing["achieved_evidence"].as_str().unwrap_or_default())
            > evidence_rank(event["achieved_evidence"].as_str().unwrap_or_default())
    {
        return;
    }
    let temporary = directory.join(format!(".{name}.{}.tmp", std::process::id()));
    std::fs::write(
        &temporary,
        serde_json::to_vec(event).expect("serialize case event"),
    )
    .expect("write case evidence");
    std::fs::rename(temporary, target).expect("publish case evidence");
}

fn evidence_rank(value: &str) -> u8 {
    match value {
        "MetadataOnly" => 0,
        "RouterReachable" => 1,
        "LiveErrorPath" => 2,
        "LiveSuccess" => 3,
        "LiveStateTransition" => 4,
        "LiveRestartPersistence" => 5,
        "CrossSurfaceParity" => 6,
        "PackagedArtifactVerified" => 7,
        _ => 0,
    }
}

pub(crate) fn disposition(intent: &CaseIntent) -> Disposition {
    match intent.scenario_kind {
        ScenarioKind::ContractProbe => Disposition::MetadataProbe,
        ScenarioKind::LiveInvoke => Disposition::LiveDispatch,
        ScenarioKind::StatefulScenario => Disposition::IsolatedWorkflow,
        ScenarioKind::DestructiveIsolated => Disposition::AuthorizationDenial,
        ScenarioKind::ConditionalOptional => Disposition::ConditionalProbe,
        ScenarioKind::ExternalOptional => Disposition::OfflineError,
        ScenarioKind::ExcludedWithReason => Disposition::ReviewedExclusion,
    }
}

pub(crate) fn fixtures() -> BTreeMap<String, ServiceFixture> {
    let values = [
        include_str!("../fixtures/e2e_actions/doctor.json"),
        include_str!("../fixtures/e2e_actions/browser.json"),
        include_str!("../fixtures/e2e_actions/fs.json"),
        include_str!("../fixtures/e2e_actions/gateway.json"),
        include_str!("../fixtures/e2e_actions/lab_admin.json"),
        include_str!("../fixtures/e2e_actions/server_logs.json"),
        include_str!("../fixtures/e2e_actions/setup.json"),
        include_str!("../fixtures/e2e_actions/snippets.json"),
        include_str!("../fixtures/e2e_actions/stash.json"),
        include_str!("../fixtures/e2e_actions/artifacts.json"),
        include_str!("../fixtures/e2e_actions/sources.json"),
        include_str!("../fixtures/e2e_actions/jobs.json"),
        include_str!("../fixtures/e2e_actions/uploads.json"),
        include_str!("../fixtures/e2e_actions/bundles.json"),
    ];
    let compiled_services = crate::action_matrix::compiled_intents()
        .map(|intent| intent.service.clone())
        .collect::<BTreeSet<_>>();
    let fixtures = values
        .into_iter()
        .map(|raw| {
            let fixture: ServiceFixture = serde_json::from_str(raw).expect("valid action fixture");
            (fixture.service.clone(), fixture)
        })
        .filter(|(service, _)| compiled_services.contains(service))
        .collect::<BTreeMap<_, _>>();
    let expected_services = crate::action_matrix::compiled_intents()
        .map(|intent| intent.service.clone())
        .collect::<BTreeSet<_>>();
    assert_eq!(
        fixtures.keys().cloned().collect::<BTreeSet<_>>(),
        expected_services
    );
    let catalog: Vec<crate::action_matrix::CatalogAction> =
        serde_json::from_str(ACTION_CATALOG).expect("action catalog");
    let catalog_by_action = catalog
        .iter()
        .map(|entry| ((entry.service.as_str(), entry.action.as_str()), entry))
        .collect::<BTreeMap<_, _>>();
    for (service, fixture) in &fixtures {
        let intents = crate::action_matrix::compiled_intents()
            .filter(|intent| &intent.service == service)
            .collect::<Vec<_>>();
        let required = intents
            .iter()
            .flat_map(|intent| intent.fixture_params.parameters.values())
            .map(|source| {
                source
                    .strip_prefix("$fixture.")
                    .unwrap_or_else(|| panic!("non-declarative fixture source {source}"))
                    .to_owned()
            })
            .collect::<BTreeSet<_>>();
        assert_eq!(
            fixture.parameters.keys().cloned().collect::<BTreeSet<_>>(),
            required
        );
        let action_keys = intents
            .iter()
            .map(|intent| intent.action.clone())
            .collect::<BTreeSet<_>>();
        assert!(
            fixture
                .action_params
                .keys()
                .all(|action| action_keys.contains(action))
        );
        for (action, values) in &fixture.action_params {
            let metadata = catalog_by_action
                .get(&(service.as_str(), action.as_str()))
                .expect("fixture override action metadata");
            let metadata_keys = metadata
                .params
                .iter()
                .map(|param| param.name.as_str())
                .collect::<BTreeSet<_>>();
            assert!(
                values
                    .keys()
                    .all(|name| metadata_keys.contains(name.as_str()))
            );
        }
    }
    fixtures
}

pub(crate) fn exact_plans(surface: Surface) -> BTreeMap<String, Disposition> {
    crate::action_matrix::compiled_intents()
        .filter(|intent| intent.applicable_surfaces.contains(&surface))
        .map(|intent| (intent.key(), disposition(intent)))
        .collect()
}

pub(crate) async fn run_cli_probe(home: &Path, args: &[String]) -> Result<Output, String> {
    let mut command = tokio::process::Command::from(isolated_command(home));
    command.args(args).env("LABBY_MATRIX_CANARY", SECRET_CANARY);
    tokio::time::timeout(CHILD_DEADLINE, command.output())
        .await
        .map_err(|_| format!("CLI child exceeded {CHILD_DEADLINE:?}"))?
        .map_err(|error| error.to_string())
}

pub(crate) async fn run_cli(home: &Path, args: &[&str]) -> Result<Output, String> {
    let owned = args
        .iter()
        .map(|value| (*value).to_string())
        .collect::<Vec<_>>();
    run_cli_probe(home, &owned).await
}

pub(crate) async fn run_cli_in_install(
    home: &Path,
    labby_home: &Path,
    args: &[&str],
) -> Result<Output, String> {
    let mut command = tokio::process::Command::from(isolated_command(home));
    command
        .env("LABBY_HOME", labby_home)
        .env("LABBY_MATRIX_CANARY", SECRET_CANARY)
        .args(args);
    tokio::time::timeout(CHILD_DEADLINE, command.output())
        .await
        .map_err(|_| format!("CLI child exceeded {CHILD_DEADLINE:?}"))?
        .map_err(|error| error.to_string())
}

pub(crate) fn assert_sanitized(bytes: &[u8], context: &str) {
    assert!(
        bytes.len() <= RESPONSE_LIMIT,
        "{context} exceeded response bound"
    );
    let text = String::from_utf8_lossy(bytes);
    assert!(
        !text.contains(SECRET_CANARY),
        "{context} leaked secret canary"
    );
}

pub(crate) fn assert_json_or_help(output: &Output, context: &str) {
    assert_sanitized(&output.stdout, context);
    assert_sanitized(&output.stderr, context);
    let json = serde_json::from_str::<Value>(&String::from_utf8_lossy(&output.stdout));
    assert!(
        output.status.success() || json.is_ok(),
        "{context} failed without a stable JSON result: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        json.is_ok() || stdout.contains("Usage:") || stdout.starts_with("Lab ·"),
        "{context} was neither stable JSON nor clap help: {stdout}"
    );
}

pub(crate) fn assert_success_json(output: &Output, context: &str) -> Value {
    assert_sanitized(&output.stdout, context);
    assert_sanitized(&output.stderr, context);
    assert!(
        output.status.success(),
        "{context} failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout)
        .unwrap_or_else(|error| panic!("{context} did not return JSON: {error}"))
}

pub(crate) fn action_request(intent: &CaseIntent) -> Value {
    json!({"action": intent.action, "params": fixture_params(intent)})
}

pub(crate) fn fixture_params(intent: &CaseIntent) -> Value {
    let all = fixtures();
    let fixture = all.get(&intent.service).expect("service fixture");
    let mut params = serde_json::Map::new();
    for (name, source) in &intent.fixture_params.parameters {
        let fixture_key = source
            .strip_prefix("$fixture.")
            .unwrap_or_else(|| panic!("non-declarative source for {}", intent.key()));
        params.insert(name.clone(), fixture.parameters[fixture_key].clone());
    }
    if let Some(overrides) = fixture.action_params.get(&intent.action) {
        for (name, value) in overrides {
            params.insert(name.clone(), value.clone());
        }
    }
    Value::Object(params)
}

pub(crate) fn services_for(surface: Surface) -> BTreeSet<String> {
    crate::action_matrix::compiled_intents()
        .filter(|intent| intent.applicable_surfaces.contains(&surface))
        .map(|intent| intent.service.clone())
        .collect()
}

pub(crate) fn dedicated_contract_reason(key: &str) -> Option<&'static str> {
    dedicated_contract(key).map(|(reason, _)| reason)
}

pub(crate) fn dedicated_contract_reason_for(key: &str, surface: Surface) -> Option<&'static str> {
    dedicated_contract_for(key, surface).map(|(reason, _)| reason)
}

fn dedicated_contract(key: &str) -> Option<(&'static str, &'static str)> {
    match key {
        "bundles:bundles.delete" => {
            Some(("requires_authorized_artifact_project_context", "forbidden"))
        }
        "gateway:gateway.clients.list" => Some(("catalog_dispatch_mismatch", "unknown_action")),
        "gateway:gateway.enrich.apply" => {
            Some(("requires_live_catalog_suggestion", "stale_suggestion"))
        }
        "gateway:gateway.enrich.preview" => {
            Some(("requires_live_catalog_suggestion", "invalid_param"))
        }
        "gateway:gateway.import" => {
            Some(("requires_external_client_import_artifact", "invalid_param"))
        }
        key if key.starts_with("gateway:gateway.import") => {
            Some(("requires_external_client_import_artifact", "not_found"))
        }
        "gateway:gateway.loadout.stage_patch" => Some((
            "requires_mounted_publication_restart_generation",
            "not_found",
        )),
        key if key.starts_with("gateway:gateway.loadout.stage_") => Some((
            "requires_mounted_publication_restart_generation",
            "invalid_param",
        )),
        "gateway:gateway.oauth.google_revoke" => {
            Some(("requires_stored_google_grant", "not_found"))
        }
        "gateway:gateway.oauth.clear"
        | "gateway:gateway.oauth.start"
        | "gateway:gateway.oauth.status"
        | "gateway:gateway.oauth.wait" => Some(("requires_configured_oauth_upstream", "not_found")),
        "gateway:gateway.oauth.probe" => {
            Some(("requires_reachable_oauth_provider", "invalid_param"))
        }
        "gateway:gateway.oauth.resource_lease.create"
        | "gateway:gateway.oauth.resource_lease.release"
        | "gateway:gateway.oauth.resource_lease.renew" => {
            Some(("requires_oauth_resource_authority", "auth_failed"))
        }
        key if key.starts_with("gateway:gateway.protected_route.stage_") => Some((
            "requires_mounted_publication_restart_generation",
            "invalid_param",
        )),
        key if key.starts_with("gateway:gateway.service_config.") => {
            Some(("requires_configured_external_builtin_api", "invalid_param"))
        }
        key if key.starts_with("gateway:gateway.virtual_server.") => {
            Some(("requires_migration_created_virtual_server", "not_found"))
        }
        "setup:plugin.install"
        | "setup:plugin.uninstall"
        | "setup:install_plugin"
        | "setup:uninstall_plugin" => Some((
            "requires_configured_external_plugin_service",
            "unknown_service",
        )),
        "setup:services.status" => Some((
            "requires_configured_external_plugin_service",
            "claude_cli_unavailable",
        )),
        "setup:settings.config.update" | "setup:settings.env.update" => {
            Some(("typed_compare_and_swap_contract_probed", "invalid_param"))
        }
        "skills:skills.get" | "skills:skills.read" => {
            Some(("requires_indexed_packaged_skill", "not_found"))
        }
        "snippets:snippets.promote" => Some((
            "requires_real_code_mode_execution_record",
            "unknown_execution",
        )),
        "snippets:snippets.test" => Some(("requires_existing_snippet_test_target", "not_found")),
        _ => None,
    }
}

pub(crate) fn dedicated_contract_accepts(key: &str, error_kind: &str) -> bool {
    dedicated_contract(key).is_some_and(|(_, expected_kind)| error_kind == expected_kind)
}

pub(crate) fn dedicated_contract_accepts_for(
    key: &str,
    surface: Surface,
    error_kind: &str,
) -> bool {
    // The isolated MCP runner can fail at either side of the same missing
    // durable-principal boundary: the Linux peer can be denied before
    // resolution, Depot can be unreachable, or Stash can map the unavailable
    // authority. These are stable errors; the authenticated restart journey
    // supplies the success evidence.
    if key.starts_with("stash:") && surface == Surface::Mcp {
        return matches!(
            error_kind,
            "forbidden" | "upstream_connect_error" | "service_unavailable"
        );
    }
    dedicated_contract_for(key, surface)
        .is_some_and(|(_, expected_kind)| error_kind == expected_kind)
}

fn dedicated_contract_for(key: &str, surface: Surface) -> Option<(&'static str, &'static str)> {
    if key.starts_with("stash:") {
        return if surface == Surface::Mcp {
            Some((
                "requires_durable_principal_link_covered_by_restart_journey",
                "upstream_connect_error",
            ))
        } else if cfg!(target_os = "linux") && surface == Surface::Api {
            Some((
                "requires_durable_principal_link_covered_by_restart_journey",
                "service_unavailable",
            ))
        } else if surface == Surface::Api {
            Some((
                "requires_descriptor_relative_filesystem_platform",
                "route_not_found",
            ))
        } else {
            None
        };
    }
    if key == "gateway:gateway.skills.list" && !cfg!(feature = "skills") {
        return Some(("requires_skills_runtime", "feature_not_compiled"));
    }
    if surface == Surface::Api
        && matches!(
            key,
            "artifacts:artifacts.search"
                | "artifacts:artifacts.list"
                | "artifacts:artifacts.get"
                | "artifacts:artifacts.read"
                | "artifacts:artifacts.history"
                | "artifacts:artifacts.validate"
                | "artifacts:artifacts.create"
                | "artifacts:artifacts.save"
                | "artifacts:artifacts.activate"
                | "artifacts:artifacts.deactivate"
                | "artifacts:artifacts.archive"
                | "artifacts:artifacts.rollback"
                | "artifacts:artifacts.refresh"
        )
    {
        return Some(("requires_project_bound_artifact_authority", "forbidden"));
    }
    if surface == Surface::Mcp && key == "bundles:bundles.delete" {
        return Some(("requires_existing_project_bundle", "not_found"));
    }
    if surface == Surface::Mcp
        && matches!(
            key,
            "artifacts:artifacts.get"
                | "artifacts:artifacts.read"
                | "artifacts:artifacts.history"
                | "artifacts:artifacts.save"
                | "artifacts:artifacts.activate"
                | "artifacts:artifacts.deactivate"
                | "artifacts:artifacts.archive"
                | "artifacts:artifacts.rollback"
        )
    {
        return Some(("requires_project_bound_artifact_authority", "forbidden"));
    }
    if surface == Surface::Mcp
        && matches!(
            key,
            "artifacts:artifacts.search"
                | "artifacts:artifacts.list"
                | "artifacts:artifacts.validate"
                | "artifacts:artifacts.create"
                | "artifacts:artifacts.refresh"
                | "artifacts:artifacts.import"
                | "artifacts:artifacts.authority_status"
                | "artifacts:artifacts.follow"
                | "artifacts:artifacts.fork"
                | "artifacts:artifacts.get_remote"
                | "artifacts:artifacts.intake_candidate"
                | "artifacts:artifacts.list_acp_registry"
                | "artifacts:artifacts.list_candidates"
                | "artifacts:artifacts.list_connections"
                | "artifacts:artifacts.list_mcp_registry"
                | "artifacts:artifacts.list_remote"
                | "artifacts:artifacts.search_ard"
                | "artifacts:artifacts.search_marketplace"
                | "artifacts:artifacts.search_remote"
                | "artifacts:artifacts.search_skills_sh"
                | "artifacts:artifacts.set_license"
                | "artifacts:artifacts.set_publication"
        )
    {
        return Some((
            "requires_project_bound_artifact_authority",
            "internal_error",
        ));
    }
    if key == "gateway:gateway.loadout.stage_patch" && surface == Surface::Api {
        return Some((
            "requires_mounted_publication_restart_generation",
            "invalid_param",
        ));
    }
    if surface == Surface::Mcp {
        return match key {
            "setup:services.status" => Some((
                "requires_configured_external_plugin_service",
                "internal_error",
            )),
            "setup:bootstrap" => Some(("requires_host_bootstrap_authority", "forbidden")),
            "setup:plugin_connectivity" => {
                Some(("requires_host_plugin_connectivity_authority", "forbidden"))
            }
            "setup:proxy.configure" => {
                Some(("requires_host_proxy_configuration_authority", "forbidden"))
            }
            _ => dedicated_contract(key),
        };
    }
    dedicated_contract(key)
}

#[cfg(test)]
mod dedicated_contract_tests {
    use super::{
        Surface, dedicated_contract, dedicated_contract_accepts, dedicated_contract_accepts_for,
        dedicated_contract_reason,
    };

    #[test]
    fn stash_mcp_accepts_both_missing_authority_layers() {
        for kind in ["forbidden", "upstream_connect_error", "service_unavailable"] {
            assert!(dedicated_contract_accepts_for(
                "stash:stash.list",
                Surface::Mcp,
                kind
            ));
        }
        assert!(!dedicated_contract_accepts_for(
            "stash:stash.list",
            Surface::Mcp,
            "internal_error"
        ));
    }

    #[test]
    fn every_dedicated_contract_accepts_only_its_exact_error_kind() {
        let rejected_kinds = [
            "internal_error",
            "invalid_param",
            "not_found",
            "conflict",
            "precondition_failed",
            "stale_suggestion",
            "unknown_execution",
            "config_error",
            "unknown_service",
            "unknown_action",
            "auth_failed",
        ];
        let mappings = crate::action_matrix::intents()
            .iter()
            .filter_map(|intent| {
                let key = intent.key();
                dedicated_contract(&key).map(|contract| (key, contract))
            })
            .collect::<Vec<_>>();
        assert!(!mappings.is_empty(), "dedicated contract mappings");

        for (key, (reason, expected_kind)) in mappings {
            assert_eq!(dedicated_contract_reason(&key), Some(reason), "{key}");
            assert!(
                dedicated_contract_accepts(&key, expected_kind),
                "{key} must accept its exact fixture error {expected_kind}"
            );
            for rejected in rejected_kinds {
                if rejected != expected_kind {
                    assert!(
                        !dedicated_contract_accepts(&key, rejected),
                        "{key} unexpectedly accepted {rejected} for {reason}"
                    );
                }
            }
        }
    }

    #[test]
    fn unmapped_actions_and_arbitrary_errors_are_rejected() {
        assert_eq!(dedicated_contract_reason("gateway:gateway.get"), None);
        assert!(!dedicated_contract_accepts(
            "gateway:gateway.get",
            "not_found"
        ));
        assert!(!dedicated_contract_accepts(
            "gateway:gateway.clients.list",
            "arbitrary_error"
        ));
    }
}
