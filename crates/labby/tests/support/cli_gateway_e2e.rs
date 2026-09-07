use std::path::Path;
use std::process::Output;

use serde_json::Value;

use crate::action_matrix::{EvidenceLevel, Surface};
use crate::action_scenarios::{self, ActionOutcome};

const UPSTREAM: &str = "matrix-owned-gateway";
const LOADOUT: &str = "matrix-owned-loadout";
const ROUTE: &str = "matrix-owned-route";

pub(crate) async fn run() {
    let guard = crate::live_labby::LiveLabbyBuilder::new()
        .env("LABBY_E2E_BOOTSTRAP_STATIC_OWNER", "1")
        .env("LABBY_E2E_TEAM_ID", "bootstrap-initial-team")
        .start()
        .await
        .expect("authoritative gateway CLI daemon");
    let owned = tempfile::tempdir().expect("owned gateway CLI home");
    std::fs::create_dir_all(owned.path().join("tmp")).unwrap();
    let upstream_script = owned.path().join("upstream.py");
    std::fs::write(&upstream_script, "raise SystemExit\n").expect("owned upstream fixture");
    let upstream_script = upstream_script.to_str().expect("UTF-8 fixture path");

    let add = success(
        &guard,
        owned.path(),
        &[
            "gateway",
            "add",
            "--name",
            UPSTREAM,
            "--command",
            "python3",
            "--arg",
            upstream_script,
            "--json",
        ],
    )
    .await;
    assert_eq!(add["config"]["name"], UPSTREAM);
    let get = success(
        &guard,
        owned.path(),
        &["gateway", "get", UPSTREAM, "--json"],
    )
    .await;
    assert_eq!(get["config"]["name"], UPSTREAM);
    record(
        "gateway:gateway.add",
        EvidenceLevel::LiveStateTransition,
        "owned_upstream_added",
    );
    record(
        "gateway:gateway.get",
        EvidenceLevel::LiveSuccess,
        "owned_upstream_read_back",
    );

    let discovered = success(
        &guard,
        owned.path(),
        &["gateway", "discover", "--include-existing", "--json"],
    )
    .await;
    assert!(discovered.is_array());
    record(
        "gateway:gateway.discover",
        EvidenceLevel::LiveSuccess,
        "owned_discovery_completed",
    );

    let preview = success(
        &guard,
        owned.path(),
        &[
            "gateway",
            "enrich",
            "--upstream",
            UPSTREAM,
            "--yes",
            "--json",
        ],
    )
    .await;
    assert_eq!(preview["provider"], "deterministic");
    assert_eq!(preview["proposals"][0]["upstream"], UPSTREAM);
    record(
        "gateway:gateway.enrich.preview",
        EvidenceLevel::LiveSuccess,
        "owned_preview_observed",
    );

    let metadata_hash = preview["proposals"][0]["metadata_hash"]
        .as_str()
        .expect("preview metadata hash");
    let apply = asserted(
        &guard,
        owned.path(),
        &[
            "gateway",
            "enrich",
            "apply",
            "--upstream",
            UPSTREAM,
            "--hint",
            "owned hint",
            "--metadata-hash",
            metadata_hash,
            "--yes",
            "--json",
        ],
    )
    .await;
    if apply.output.status.success() {
        assert_eq!(apply.body["upstream"], UPSTREAM);
        record(
            "gateway:gateway.enrich.apply",
            EvidenceLevel::LiveStateTransition,
            "owned_hint_applied",
        );
    } else {
        record_exact_error("gateway:gateway.enrich.apply", &apply.body);
    }

    let updated = success(
        &guard,
        owned.path(),
        &[
            "gateway",
            "update",
            UPSTREAM,
            "--proxy-skills",
            "true",
            "--json",
        ],
    )
    .await;
    assert_eq!(updated["config"]["proxy_skills"], true);
    let updated_get = success(
        &guard,
        owned.path(),
        &["gateway", "get", UPSTREAM, "--json"],
    )
    .await;
    assert_eq!(updated_get["config"]["proxy_skills"], true);
    record(
        "gateway:gateway.update",
        EvidenceLevel::LiveStateTransition,
        "owned_upstream_patch_observed",
    );

    let skills = asserted(
        &guard,
        owned.path(),
        &[
            "gateway",
            "skills",
            "list",
            "--upstream",
            UPSTREAM,
            "--json",
        ],
    )
    .await;
    if cfg!(feature = "skills") {
        assert!(
            skills.output.status.success(),
            "gateway skills list failed: {}",
            skills.body
        );
        assert_eq!(skills.body[0]["upstream"], UPSTREAM);
        record(
            "gateway:gateway.skills.list",
            EvidenceLevel::LiveSuccess,
            "owned_skills_status_observed",
        );
    } else {
        record_exact_error("gateway:gateway.skills.list", &skills.body);
    }

    let tested = success(
        &guard,
        owned.path(),
        &["gateway", "test", "--name", UPSTREAM, "--json"],
    )
    .await;
    assert_eq!(tested["name"], UPSTREAM);
    assert!(tested.get("connected").is_some());
    record(
        "gateway:gateway.test",
        EvidenceLevel::LiveSuccess,
        "owned_probe_completed",
    );

    let cleanup = success(
        &guard,
        owned.path(),
        &["gateway", "mcp", "cleanup", UPSTREAM, "--dry-run", "--json"],
    )
    .await;
    assert_eq!(cleanup["upstream"], UPSTREAM);
    assert_eq!(cleanup["dry_run"], true);
    record(
        "gateway:gateway.mcp.cleanup",
        EvidenceLevel::LiveSuccess,
        "owned_cleanup_plan_observed",
    );

    let disabled = success(
        &guard,
        owned.path(),
        &["gateway", "mcp", "disable", UPSTREAM, "--json"],
    )
    .await;
    assert_eq!(disabled["gateway"]["config"]["enabled"], false);
    assert_eq!(
        success(
            &guard,
            owned.path(),
            &["gateway", "get", UPSTREAM, "--json"]
        )
        .await["config"]["enabled"],
        false
    );
    record(
        "gateway:gateway.mcp.disable",
        EvidenceLevel::LiveStateTransition,
        "owned_upstream_disabled",
    );

    let enabled = success(
        &guard,
        owned.path(),
        &["gateway", "mcp", "enable", UPSTREAM, "--json"],
    )
    .await;
    assert_eq!(enabled["config"]["enabled"], true);
    assert_eq!(
        success(
            &guard,
            owned.path(),
            &["gateway", "get", UPSTREAM, "--json"]
        )
        .await["config"]["enabled"],
        true
    );
    record(
        "gateway:gateway.mcp.enable",
        EvidenceLevel::LiveStateTransition,
        "owned_upstream_enabled",
    );

    let restarted = success(
        &guard,
        owned.path(),
        &["gateway", "mcp", "restart", UPSTREAM, "--json"],
    )
    .await;
    assert_eq!(restarted["gateway"]["config"]["name"], UPSTREAM);
    assert!(restarted.get("cleanup").is_some());
    record(
        "gateway:gateway.mcp.restart",
        EvidenceLevel::LiveStateTransition,
        "owned_runtime_reconciled",
    );

    let loadout_add = success(
        &guard,
        owned.path(),
        &[
            "gateway",
            "loadout",
            "add",
            LOADOUT,
            "--upstream",
            UPSTREAM,
            "--json",
        ],
    )
    .await;
    assert_eq!(loadout_add["name"], LOADOUT);
    let loadout_get = success(
        &guard,
        owned.path(),
        &["gateway", "loadout", "get", LOADOUT, "--json"],
    )
    .await;
    assert_eq!(loadout_get["upstreams"][0], UPSTREAM);
    record(
        "gateway:gateway.loadout.add",
        EvidenceLevel::LiveStateTransition,
        "owned_loadout_added",
    );
    record(
        "gateway:gateway.loadout.get",
        EvidenceLevel::LiveSuccess,
        "owned_loadout_read_back",
    );

    let loadout_patch = success(
        &guard,
        owned.path(),
        &[
            "gateway",
            "loadout",
            "update",
            LOADOUT,
            "--description",
            "changed",
            "--json",
        ],
    )
    .await;
    assert_eq!(loadout_patch["description"], "changed");
    assert_eq!(
        success(
            &guard,
            owned.path(),
            &["gateway", "loadout", "get", LOADOUT, "--json"]
        )
        .await["description"],
        "changed"
    );
    record(
        "gateway:gateway.loadout.patch",
        EvidenceLevel::LiveStateTransition,
        "owned_loadout_patch_observed",
    );

    let loadout_remove = success(
        &guard,
        owned.path(),
        &["gateway", "loadout", "remove", LOADOUT, "--json"],
    )
    .await;
    assert_eq!(loadout_remove["name"], LOADOUT);
    assert_error_kind(
        &asserted(
            &guard,
            owned.path(),
            &["gateway", "loadout", "get", LOADOUT, "--json"],
        )
        .await,
        "not_found",
    );
    record(
        "gateway:gateway.loadout.remove",
        EvidenceLevel::LiveStateTransition,
        "owned_loadout_removed",
    );

    let route_add = success(
        &guard,
        owned.path(),
        &route_args("add", "owned.test", "/mcp"),
    )
    .await;
    assert_eq!(route_add["name"], ROUTE);
    let route_get = success(
        &guard,
        owned.path(),
        &["gateway", "protected-route", "get", ROUTE, "--json"],
    )
    .await;
    assert_eq!(route_get["public_host"], "owned.test");
    record(
        "gateway:gateway.protected_route.add",
        EvidenceLevel::LiveStateTransition,
        "owned_route_added",
    );
    record(
        "gateway:gateway.protected_route.get",
        EvidenceLevel::LiveSuccess,
        "owned_route_read_back",
    );

    let route_update = success(
        &guard,
        owned.path(),
        &route_args("update", "updated.test", "/updated"),
    )
    .await;
    assert_eq!(route_update["public_host"], "updated.test");
    assert_eq!(
        success(
            &guard,
            owned.path(),
            &["gateway", "protected-route", "get", ROUTE, "--json"]
        )
        .await["public_path"],
        "/updated"
    );
    record(
        "gateway:gateway.protected_route.update",
        EvidenceLevel::LiveStateTransition,
        "owned_route_patch_observed",
    );

    let route_remove = success(
        &guard,
        owned.path(),
        &["gateway", "protected-route", "remove", ROUTE, "--json"],
    )
    .await;
    assert_eq!(route_remove["name"], ROUTE);
    assert_error_kind(
        &asserted(
            &guard,
            owned.path(),
            &["gateway", "protected-route", "get", ROUTE, "--json"],
        )
        .await,
        "not_found",
    );
    record(
        "gateway:gateway.protected_route.remove",
        EvidenceLevel::LiveStateTransition,
        "owned_route_removed",
    );

    let removed = success(
        &guard,
        owned.path(),
        &["gateway", "remove", UPSTREAM, "--json"],
    )
    .await;
    assert_eq!(removed["config"]["name"], UPSTREAM);
    assert_error_kind(
        &asserted(
            &guard,
            owned.path(),
            &["gateway", "get", UPSTREAM, "--json"],
        )
        .await,
        "not_found",
    );
    record(
        "gateway:gateway.remove",
        EvidenceLevel::LiveStateTransition,
        "owned_upstream_removed",
    );
    let cleanup = guard.finish().await;
    assert!(
        cleanup.is_clean(),
        "gateway daemon cleanup: {:?}",
        cleanup.failures
    );
}

fn route_args(command: &'static str, host: &'static str, path: &'static str) -> Vec<&'static str> {
    match command {
        "add" => vec![
            "gateway",
            "protected-route",
            "add",
            "--name",
            ROUTE,
            "--public-host",
            host,
            "--public-path",
            path,
            "--upstream",
            UPSTREAM,
            "--json",
        ],
        "update" => vec![
            "gateway",
            "protected-route",
            "update",
            ROUTE,
            "--public-host",
            host,
            "--public-path",
            path,
            "--upstream",
            UPSTREAM,
            "--enabled",
            "true",
            "--json",
        ],
        _ => panic!("unsupported protected-route command {command}"),
    }
}

struct AssertedOutput {
    output: Output,
    body: Value,
}

async fn asserted(
    guard: &crate::live_labby::LiveLabbyGuard,
    home: &Path,
    args: &[&str],
) -> AssertedOutput {
    let output = action_scenarios::run_cli_against(home, args, guard)
        .await
        .unwrap_or_else(|error| panic!("{}: {error}", args.join(" ")));
    action_scenarios::assert_sanitized(&output.stdout, &args.join(" "));
    action_scenarios::assert_sanitized(&output.stderr, &args.join(" "));
    let body = machine_json(&output).unwrap_or_else(|| {
        panic!(
            "{} did not return machine-readable JSON: {}",
            args.join(" "),
            String::from_utf8_lossy(&output.stderr)
        )
    });
    AssertedOutput { output, body }
}

async fn success(guard: &crate::live_labby::LiveLabbyGuard, home: &Path, args: &[&str]) -> Value {
    let result = asserted(guard, home, args).await;
    assert!(
        result.output.status.success(),
        "{} failed: {}",
        args.join(" "),
        result.body
    );
    result.body
}

fn assert_error_kind(result: &AssertedOutput, expected: &str) {
    assert!(
        !result.output.status.success(),
        "expected {expected}: {}",
        result.body
    );
    assert_eq!(result.body["ok"], false);
    assert_eq!(result.body["error"]["kind"], expected);
}

fn record_exact_error(key: &str, body: &Value) {
    assert_eq!(body["ok"], false, "{key}: {body}");
    let kind = body["error"]["kind"].as_str().expect("stable error kind");
    let reason = action_scenarios::dedicated_contract_reason_for(key, Surface::Cli)
        .filter(|_| action_scenarios::dedicated_contract_accepts_for(key, Surface::Cli, kind))
        .unwrap_or_else(|| panic!("{key}: unexpected dedicated error {kind}"));
    record(
        key,
        EvidenceLevel::LiveErrorPath,
        &format!("dedicated_contract:{reason}:{kind}"),
    );
}

fn record(key: &str, evidence: EvidenceLevel, outcome_kind: &str) {
    let intent = crate::action_matrix::intents()
        .iter()
        .find(|intent| intent.key() == key)
        .unwrap_or_else(|| panic!("missing authoritative intent {key}"));
    assert!(
        evidence >= intent.minimum_evidence || outcome_kind.starts_with("dedicated_contract:"),
        "{key}: {evidence:?} is below {:?}",
        intent.minimum_evidence
    );
    ActionOutcome {
        key: key.into(),
        surface: Surface::Cli,
        disposition: action_scenarios::disposition(intent),
        evidence,
        owner: intent.scenario_owner,
        outcome_kind: outcome_kind.into(),
        recovery: "owned_gateway_workflow_rollback".into(),
        side_effects: "isolated_disposable_home".into(),
        canary_free: true,
    }
    .record();
}

fn machine_json(output: &Output) -> Option<Value> {
    for bytes in [&output.stdout, &output.stderr] {
        if let Ok(value) = serde_json::from_slice(bytes) {
            return Some(value);
        }
    }
    [&output.stdout, &output.stderr]
        .into_iter()
        .flat_map(|bytes| {
            String::from_utf8_lossy(bytes)
                .lines()
                .map(str::to_owned)
                .collect::<Vec<_>>()
        })
        .rev()
        .find_map(|line| serde_json::from_str(&line).ok())
}
