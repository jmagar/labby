#![allow(clippy::panic)]

#[path = "support/lib.rs"]
mod support;

use std::collections::{BTreeMap, BTreeSet};

use serde_json::Value;
use support::action_matrix::{
    CatalogAction, EXPECTED_ACTIONS, EXPECTED_API_ACTIONS, EXPECTED_CLI_ACTIONS,
    EXPECTED_MCP_ACTIONS, EXPECTED_SHARED_CLI_MCP_API_ACTIONS, EXPECTED_WEB_ACTIONS, EvidenceLevel,
    PersistenceClass, ScenarioKind, ScenarioOwner, Surface, catalog_map, intent_map,
    intent_map_from, intents, validate_intent_shape,
};

const ACTION_CATALOG: &str = include_str!("../../../docs/generated/action-catalog.json");

fn catalog() -> Vec<CatalogAction> {
    serde_json::from_str(ACTION_CATALOG).expect("generated action catalog must parse")
}

#[test]
fn every_catalog_action_has_one_well_formed_intent() {
    let catalog_values = catalog();
    let catalog =
        catalog_map(&catalog_values).unwrap_or_else(|errors| panic!("{}", errors.join("\n")));
    let intents = intent_map().unwrap_or_else(|errors| panic!("{}", errors.join("\n")));
    let catalog_keys = catalog.keys().cloned().collect::<BTreeSet<_>>();
    let intent_keys = intents.keys().cloned().collect::<BTreeSet<_>>();
    let missing = catalog_keys.difference(&intent_keys).collect::<Vec<_>>();
    let stale = intent_keys.difference(&catalog_keys).collect::<Vec<_>>();
    assert!(
        missing.is_empty() && stale.is_empty(),
        "action intent exact-set mismatch\nmissing: {missing:#?}\nstale: {stale:#?}"
    );
    let shape_errors = intents
        .values()
        .flat_map(|intent| validate_intent_shape(intent))
        .collect::<Vec<_>>();
    assert!(shape_errors.is_empty(), "{}", shape_errors.join("\n"));
    for (key, action) in &catalog {
        let intent = intents[key];
        assert_eq!(
            intent.applicable_surfaces,
            action.surfaces(),
            "{key}: committed surfaces drifted from canonical metadata"
        );
        let required = action
            .params
            .iter()
            .filter(|param| param.required)
            .map(|param| param.name.as_str())
            .collect::<BTreeSet<_>>();
        let recipe = intent
            .fixture_params
            .parameters
            .keys()
            .map(String::as_str)
            .collect::<BTreeSet<_>>();
        assert_eq!(
            recipe, required,
            "{key}: fixture recipe must bind every required parameter exactly"
        );
        if action.builtin {
            assert_eq!(
                intents[key].scenario_kind,
                ScenarioKind::ContractProbe,
                "{key}: synthesized help/schema actions are metadata contracts"
            );
        }
    }
}

#[test]
fn invalid_overlay_changes_produce_actionable_failures() {
    let mut duplicate = intents().to_vec();
    duplicate.push(duplicate[0].clone());
    let duplicate_errors = intent_map_from(&duplicate).expect_err("duplicate must fail");
    assert!(duplicate_errors[0].contains("duplicate action intent:"));

    let mut unsafe_intent = intents()[0].clone();
    unsafe_intent.fixture_params.fixture = "developer_home".to_string();
    assert!(
        validate_intent_shape(&unsafe_intent)
            .iter()
            .any(|error| error.contains("named hermetic recipe"))
    );

    let mut impossible = intents()[0].clone();
    impossible.required = false;
    assert!(
        validate_intent_shape(&impossible)
            .iter()
            .any(|error| error.contains("non-optional case must be required"))
    );

    let catalog_keys = catalog()
        .into_iter()
        .map(|action| action.key())
        .collect::<BTreeSet<_>>();
    let first_key = intents()[0].key();
    let mut missing = catalog_keys.clone();
    missing.remove(&first_key);
    let missing_diff = catalog_keys
        .difference(&missing)
        .map(String::as_str)
        .collect::<Vec<_>>();
    assert_eq!(missing_diff, [first_key.as_str()]);

    let mut stale = catalog_keys.clone();
    stale.insert("retired:stale".to_string());
    let stale_diff = stale
        .difference(&catalog_keys)
        .map(String::as_str)
        .collect::<Vec<_>>();
    assert_eq!(stale_diff, ["retired:stale"]);
}

#[test]
fn authoritative_inventory_totals_are_locked() {
    let catalog = catalog();
    assert_eq!(catalog.len(), EXPECTED_ACTIONS);
    assert_eq!(intents().len(), EXPECTED_ACTIONS);
    assert_eq!(
        catalog
            .iter()
            .filter(|a| a.surface_availability.cli)
            .count(),
        EXPECTED_CLI_ACTIONS
    );
    assert_eq!(
        catalog
            .iter()
            .filter(|a| a.surface_availability.mcp)
            .count(),
        EXPECTED_MCP_ACTIONS
    );
    assert_eq!(
        catalog
            .iter()
            .filter(|a| a.surface_availability.api)
            .count(),
        EXPECTED_API_ACTIONS
    );
    assert_eq!(
        catalog
            .iter()
            .filter(|a| a.surface_availability.web_ui)
            .count(),
        EXPECTED_WEB_ACTIONS
    );
    assert_eq!(
        catalog
            .iter()
            .filter(|a| a.surface_availability.cli
                && a.surface_availability.mcp
                && a.surface_availability.api)
            .count(),
        EXPECTED_SHARED_CLI_MCP_API_ACTIONS
    );
}

#[test]
fn aliases_inherit_the_canonical_scenario_and_policy() {
    let catalog_values = catalog();
    let catalog = catalog_map(&catalog_values).unwrap();
    let intents = intent_map().unwrap();
    let aliases = intents
        .values()
        .filter(|intent| intent.canonical_action.is_some())
        .collect::<Vec<_>>();
    assert_eq!(
        aliases.len(),
        4,
        "all compatibility aliases must be explicit"
    );
    for alias in aliases {
        let canonical_key = alias.canonical_action.as_ref().unwrap();
        let canonical_intent = intents
            .get(canonical_key)
            .unwrap_or_else(|| panic!("{}: missing canonical intent {canonical_key}", alias.key()));
        let alias_action = catalog[&alias.key()];
        let canonical_action = catalog[canonical_key];
        assert_eq!(alias.scenario_id, canonical_intent.scenario_id);
        assert_eq!(alias_action.requires_admin, canonical_action.requires_admin);
        assert_eq!(alias_action.destructive, canonical_action.destructive);
        assert_eq!(
            alias_action.required_scopes,
            canonical_action.required_scopes
        );
        // Compatibility aliases are transport spellings, not independent CLI or
        // Web UI bindings. Those adapter projections are intentionally allowed
        // to differ from the canonical action.
        let without_adapter_aliases = |mut surfaces: BTreeSet<Surface>| {
            surfaces.remove(&Surface::Cli);
            surfaces.remove(&Surface::WebUi);
            surfaces
        };
        assert_eq!(
            without_adapter_aliases(alias_action.surfaces()),
            without_adapter_aliases(canonical_action.surfaces())
        );
    }
}

#[test]
fn feature_shape_intent_is_explicit_without_the_live_harness() {
    let allowed = BTreeSet::from([
        "all",
        "api-docs",
        "base",
        "default",
        "fs",
        "gateway-host",
        "lab-admin",
        "no-default",
        "skills",
    ])
    .into_iter()
    .map(String::from)
    .collect();
    for intent in intents() {
        let unknown = intent
            .applicable_features
            .difference(&allowed)
            .collect::<Vec<_>>();
        assert!(
            unknown.is_empty(),
            "{}: unknown feature shapes {unknown:?}",
            intent.key()
        );
        match intent.service.as_str() {
            "fs" => assert!(intent.applicable_features.contains("fs")),
            "lab_admin" => assert!(intent.applicable_features.contains("lab-admin")),
            "artifacts" | "bundles" | "jobs" | "skills" | "sources" | "uploads" => {
                assert!(intent.applicable_features.contains("skills"));
            }
            "browser" | "gateway" | "snippets" => {
                assert!(intent.applicable_features.contains("gateway-host"));
            }
            _ => assert!(intent.applicable_features.contains("base")),
        }
    }
}

#[test]
fn independently_defined_feature_shapes_match_intent_projections() {
    let base = BTreeSet::from(["doctor", "server_logs", "setup", "stash"]);
    let gateway = BTreeSet::from([
        "artifacts",
        "browser",
        "bundles",
        "doctor",
        "gateway",
        "jobs",
        "server_logs",
        "setup",
        "snippets",
        "sources",
        "stash",
        "uploads",
    ]);
    let shapes = BTreeMap::from([
        ("base", base.clone()),
        ("no-default", base.clone()),
        ("api-docs", base.clone()),
        ("default", gateway.clone()),
        ("gateway-host", gateway),
        (
            "fs",
            BTreeSet::from(["doctor", "fs", "server_logs", "setup", "stash"]),
        ),
        (
            "skills",
            BTreeSet::from([
                "artifacts",
                "bundles",
                "doctor",
                "jobs",
                "server_logs",
                "setup",
                "sources",
                "stash",
                "uploads",
            ]),
        ),
        (
            "lab-admin",
            BTreeSet::from(["doctor", "lab_admin", "server_logs", "setup", "stash"]),
        ),
        (
            "all",
            BTreeSet::from([
                "browser",
                "doctor",
                "fs",
                "gateway",
                "lab_admin",
                "server_logs",
                "setup",
                "artifacts",
                "bundles",
                "jobs",
                "snippets",
                "sources",
                "stash",
                "uploads",
            ]),
        ),
    ]);
    let catalog = catalog();
    for (shape, services) in shapes {
        let independently_expected = catalog
            .iter()
            .filter(|action| services.contains(action.service.as_str()))
            .map(CatalogAction::key)
            .collect::<BTreeSet<_>>();
        let declared = intents()
            .iter()
            .filter(|intent| intent.applicable_features.contains(shape))
            .map(|intent| intent.key())
            .collect::<BTreeSet<_>>();
        assert_eq!(
            declared, independently_expected,
            "{shape}: action projection drift"
        );
    }

    let compiled_services = labby::registry::build_docs_registry()
        .services()
        .iter()
        .map(|service| service.name)
        .collect::<BTreeSet<_>>();
    let compiled_shape = if cfg!(feature = "all") {
        "all"
    } else if cfg!(feature = "gateway") {
        "gateway-host"
    } else if cfg!(feature = "fs") {
        "fs"
    } else if cfg!(feature = "skills") {
        "skills"
    } else if cfg!(feature = "lab-admin") {
        "lab-admin"
    } else if cfg!(feature = "api-docs") {
        "api-docs"
    } else {
        "no-default"
    };
    let declared_services = intents()
        .iter()
        .filter(|intent| intent.applicable_features.contains(compiled_shape))
        .map(|intent| intent.service.as_str())
        .collect::<BTreeSet<_>>();
    assert_eq!(
        declared_services, compiled_services,
        "compiled registry does not match {compiled_shape} intent"
    );
}

#[test]
fn security_invariants_are_independent_of_execution_intent() {
    #[derive(Clone, Copy)]
    struct Invariant {
        allowed: bool,
        minimum: EvidenceLevel,
    }
    let invariants = BTreeMap::from([
        (
            (false, false),
            Invariant {
                allowed: true,
                minimum: EvidenceLevel::MetadataOnly,
            },
        ),
        (
            (false, true),
            Invariant {
                allowed: true,
                minimum: EvidenceLevel::LiveStateTransition,
            },
        ),
        (
            (true, false),
            Invariant {
                allowed: true,
                minimum: EvidenceLevel::MetadataOnly,
            },
        ),
        (
            (true, true),
            Invariant {
                allowed: true,
                minimum: EvidenceLevel::LiveStateTransition,
            },
        ),
    ]);
    let catalog = catalog();
    let catalog_by_key = catalog_map(&catalog).expect("catalog keys are unique");
    let intents = intent_map().unwrap();
    let mut observed_combinations = BTreeSet::new();
    for action in &catalog {
        let intent = intents[&action.key()];
        observed_combinations.insert((action.requires_admin, action.destructive));
        let invariant = invariants[&(action.requires_admin, action.destructive)];
        assert!(
            invariant.allowed,
            "{} has forbidden non-admin destructive policy",
            action.key()
        );
        if action.requires_admin {
            assert_eq!(action.required_scopes, ["lab:admin"]);
            assert!(action.auth_posture.contains("lab:admin"));
            let execution_intent = intent
                .canonical_action
                .as_ref()
                .map_or(intent, |canonical| intents[canonical]);
            assert!(
                execution_intent.minimum_evidence >= EvidenceLevel::LiveErrorPath,
                "{}: admin execution denial needs live error-path evidence",
                action.key()
            );
            assert_ne!(
                execution_intent.scenario_owner,
                ScenarioOwner::CatalogGovernance
            );
        } else {
            assert!(action.required_scopes.is_empty());
        }
        if action.destructive {
            let execution_intent = intent
                .canonical_action
                .as_ref()
                .map_or(intent, |canonical| intents[canonical]);
            assert_eq!(
                execution_intent.scenario_kind,
                ScenarioKind::DestructiveIsolated
            );
            assert!(
                execution_intent.required,
                "destructive denial/confirmation cannot be skipped"
            );
            assert!(execution_intent.minimum_evidence >= invariant.minimum);
            assert_eq!(execution_intent.setup_ref, "disposable-home");
            assert_eq!(
                execution_intent.scenario_owner,
                ScenarioOwner::StatefulWorkflowRunner
            );
            assert_eq!(
                execution_intent.persistence_class,
                PersistenceClass::Durable
            );
        } else {
            assert_ne!(
                intent.scenario_kind,
                ScenarioKind::DestructiveIsolated,
                "{}: non-destructive action cannot request confirmation",
                action.key()
            );
        }
        if matches!(intent.scenario_kind, ScenarioKind::LiveInvoke) {
            assert_eq!(intent.setup_ref, "disposable-home");
            assert!(intent.required);
        }
    }
    assert_eq!(
        observed_combinations,
        BTreeSet::from([(false, false), (false, true), (true, false), (true, true)]),
        "all allowed admin x destructive combinations must remain covered"
    );
    let preview = catalog
        .iter()
        .find(|a| a.key() == "fs:fs.preview")
        .expect("fs.preview action");
    assert!(preview.requires_http_subject);
    assert!(!preview.surface_availability.cli);
    assert!(
        !preview.surface_availability.mcp,
        "HTTP-only action must be denied in MCP discovery"
    );
    assert!(preview.surface_availability.api && preview.surface_availability.web_ui);

    let local_only = catalog
        .iter()
        .filter(|action| action.service == "lab_admin")
        .collect::<Vec<_>>();
    assert!(
        !local_only.is_empty(),
        "the local-only lab_admin posture must remain represented"
    );
    for action in local_only {
        assert!(
            !action.surface_availability.cli && action.surface_availability.mcp,
            "{}: local-only operations remain available only to their registered local MCP adapter",
            action.key()
        );
        assert!(
            !action.surface_availability.api && !action.surface_availability.web_ui,
            "{}: local-only operations must be denied from remote discovery",
            action.key()
        );
    }

    for (key, intent) in &intents {
        if let Some(canonical) = &intent.canonical_action {
            assert_eq!(
                intent.scenario_kind,
                ScenarioKind::ContractProbe,
                "{key}: alias must remain a cheap probe"
            );
            assert_eq!(intent.scenario_owner, ScenarioOwner::CatalogGovernance);
            assert_eq!(intent.scenario_id, intents[canonical].scenario_id);
        }
        assert!(
            intent.fixture_params.fixture == "catalog_metadata"
                || intent.fixture_params.fixture == "fs_browser_subject"
                || intent.fixture_params.fixture == "synthetic_external"
                || intent.fixture_params.fixture.starts_with("isolated_"),
            "{key}: fixture may not use developer state or ambient credentials"
        );
        for surface in [Surface::Cli, Surface::Mcp, Surface::Api, Surface::WebUi] {
            let discovered = catalog_by_key[key].surfaces().contains(&surface);
            assert_eq!(
                intent.applicable_surfaces.contains(&surface),
                discovered,
                "{key}: discovery denial drift on {surface:?}"
            );
        }
    }
}

#[test]
fn retired_products_are_absent_from_authoritative_projections() {
    let retired = ["acp", "deploy", "fleet", "marketplace", "nodes", "registry"];
    for action in catalog() {
        assert!(
            !retired.contains(&action.service.as_str()),
            "retired service returned: {}",
            action.service
        );
    }
    let services: Vec<Value> =
        serde_json::from_str(include_str!("../../../docs/generated/service-catalog.json")).unwrap();
    for service in services {
        let name = service["name"].as_str().unwrap_or_default();
        assert!(
            !retired.contains(&name),
            "retired service projection returned: {name}"
        );
    }
    let routes: Vec<Value> =
        serde_json::from_str(include_str!("../../../docs/generated/api-routes.json")).unwrap();
    for route in routes {
        let path = route["path"].as_str().unwrap_or_default();
        assert!(
            retired
                .iter()
                .all(|name| path != format!("/v1/{name}")
                    && !path.starts_with(&format!("/v1/{name}/"))),
            "retired route returned: {path}"
        );
    }
    let features = include_str!("../../../docs/generated/feature-matrix.json");
    for name in ["acp", "deploy", "fleet", "marketplace", "stash"] {
        assert!(
            !features.contains(&format!("\"feature\": \"{name}\"")),
            "retired feature returned: {name}"
        );
    }
    let feature_json: Value = serde_json::from_str(features).unwrap();
    let feature_names = feature_json["features"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|feature| feature["feature"].as_str())
        .collect::<BTreeSet<_>>();
    assert!(retired.iter().all(|name| !feature_names.contains(name)));
    let openapi: Value =
        serde_json::from_str(include_str!("../../../docs/generated/openapi.json")).unwrap();
    for path in openapi["paths"].as_object().unwrap().keys() {
        assert!(
            retired
                .iter()
                .all(|name| path != &format!("/v1/{name}")
                    && !path.starts_with(&format!("/v1/{name}/"))),
            "retired OpenAPI path returned: {path}"
        );
    }
    for intent in intents() {
        assert!(retired.iter().all(|name| intent.service != *name));
        if let Some(canonical) = &intent.canonical_action {
            assert!(
                retired
                    .iter()
                    .all(|name| !canonical.starts_with(&format!("{name}:"))),
                "retired alias target returned: {canonical}"
            );
        }
    }
    let cli_help = include_str!("../../../docs/generated/cli-help.md");
    let mcp_help: Value =
        serde_json::from_str(include_str!("../../../docs/generated/mcp-help.json"))
            .expect("generated MCP help must be valid JSON");
    let mcp_services = mcp_help["services"]
        .as_array()
        .expect("generated MCP help must contain services");
    let web_nav = include_str!("../../../apps/gateway-admin/components/console/nav-model.ts");
    for name in retired {
        assert!(
            !cli_help.contains(&format!("## `labby {name}")),
            "retired CLI command returned: {name}"
        );
        assert!(
            mcp_services
                .iter()
                .all(|service| service["name"].as_str() != Some(name)),
            "retired MCP service returned: {name}"
        );
        assert!(
            !web_nav.contains(&format!("href: '/{name}")),
            "retired web navigation returned: {name}"
        );
    }
}

#[test]
fn retired_services_are_rejected_as_configured_gateway_subset_targets() {
    for service in ["acp", "deploy", "fleet", "marketplace", "nodes", "registry"] {
        let source = format!(
            r#"
[[protected_mcp_routes]]
name = "retired"
enabled = true
public_host = "mcp.example.test"
public_path = "/retired"

[protected_mcp_routes.target]
kind = "gateway_subset"
services = ["{service}"]
"#
        );
        let config: labby::config::LabConfig = toml::from_str(&source).expect("config syntax");
        let error = config
            .validate()
            .expect_err("retired service target must be rejected");
        assert!(
            error.to_string().contains("unknown gateway_subset service"),
            "{service}: unexpected config rejection: {error}"
        );
    }
}

#[test]
fn caller_bound_stash_is_rejected_as_a_context_free_gateway_subset_target() {
    let source = r#"
[[protected_mcp_routes]]
name = "caller-bound"
enabled = true
public_host = "mcp.example.test"
public_path = "/caller-bound"

[protected_mcp_routes.target]
kind = "gateway_subset"
services = ["stash"]
"#;
    let config: labby::config::LabConfig = toml::from_str(source).expect("config syntax");
    let error = config
        .validate()
        .expect_err("caller-bound service must not be accepted by context-free gateway dispatch");
    assert!(error.to_string().contains("unknown gateway_subset service"));
}

#[test]
fn generated_outcomes_cannot_downgrade_required_intent() {
    use support::action_matrix::{CaseOutcome, OutcomeStatus, outcome_satisfies};
    let intent = intents().iter().find(|intent| intent.required).unwrap();
    let skipped = CaseOutcome {
        key: intent.key(),
        status: OutcomeStatus::Skipped,
        achieved_evidence: EvidenceLevel::PackagedArtifactVerified,
        timing_ms: 0,
        failure_class: None,
        cleanup_ok: true,
        artifacts: Vec::new(),
    };
    assert!(!outcome_satisfies(intent, &skipped));
}
