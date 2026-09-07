#![allow(clippy::panic, dead_code)]

#[path = "support/action_matrix.rs"]
mod action_matrix;
#[path = "support/action_scenarios.rs"]
mod action_scenarios;
#[path = "support/cli_gateway_e2e.rs"]
mod cli_gateway_e2e;
#[path = "support/cli_misc_e2e.rs"]
mod cli_misc_e2e;
#[path = "support/evidence.rs"]
mod evidence;
#[path = "support/live_labby.rs"]
mod live_labby;

use action_matrix::{EvidenceLevel, Surface};
use action_scenarios::{ActionOutcome, MATRIX_DEADLINE, MAX_CHILDREN};

const WORKFLOW_DEADLINE: std::time::Duration = std::time::Duration::from_mins(3);

#[tokio::test]
async fn gateway_cli_actions_run_owned_end_to_end_workflows() {
    tokio::time::timeout(WORKFLOW_DEADLINE, cli_gateway_e2e::run())
        .await
        .expect("gateway CLI workflows exceeded absolute deadline");
}

#[tokio::test]
async fn miscellaneous_cli_actions_run_owned_end_to_end_workflows() {
    tokio::time::timeout(WORKFLOW_DEADLINE, cli_misc_e2e::run())
        .await
        .expect("miscellaneous CLI workflows exceeded absolute deadline");
}

#[derive(Clone, Copy, Eq, Ord, PartialEq, PartialOrd)]
struct CliActionCase {
    key: &'static str,
    argv: &'static [&'static str],
}

#[tokio::test]
async fn every_cli_action_evidence_comes_from_its_compiled_binding() {
    tokio::time::timeout(MATRIX_DEADLINE, async {
        let authoritative = action_matrix::compiled_intents()
            .filter(|intent| intent.applicable_surfaces.contains(&Surface::Cli))
            .map(action_matrix::CaseIntent::key)
            .collect::<std::collections::BTreeSet<_>>();
        let cases = cli_action_cases()
            .into_iter()
            .filter(|case| authoritative.contains(case.key))
            .collect::<std::collections::BTreeSet<_>>();
        assert_eq!(cases.len(), authoritative.len());
        assert_eq!(
            cases
                .iter()
                .map(|case| case.key.to_owned())
                .collect::<std::collections::BTreeSet<_>>(),
            authoritative
        );

        let root = tempfile::tempdir().expect("isolated CLI action roots");
        let mut tasks = tokio::task::JoinSet::new();
        for (index, case) in cases.into_iter().enumerate() {
            let home = root.path().join(format!("case-{index}"));
            std::fs::create_dir_all(home.join("tmp")).unwrap();
            tasks.spawn(async move {
                let output = run_cli_case(&home, case)
                    .await
                    .map_err(|error| format!("{}: {error}", case.key))?;
                Ok::<_, String>((case, output))
            });
            if tasks.len() == MAX_CHILDREN {
                record_asserted_cli_result(tasks.join_next().await.unwrap().unwrap().unwrap());
            }
        }
        while let Some(result) = tasks.join_next().await {
            record_asserted_cli_result(result.unwrap().unwrap());
        }
    })
    .await
    .expect("CLI action binding deadline");
}

async fn run_cli_case(
    home: &std::path::Path,
    case: CliActionCase,
) -> Result<std::process::Output, String> {
    if !case.key.starts_with("doctor:") {
        return action_scenarios::run_cli(home, case.argv).await;
    }
    let mut command = tokio::process::Command::from(live_labby::isolated_command(home));
    command
        .args(case.argv)
        .env("LABBY_MATRIX_CANARY", action_scenarios::SECRET_CANARY);
    tokio::time::timeout(std::time::Duration::from_secs(30), command.output())
        .await
        .map_err(|_| "CLI child exceeded 30s".to_owned())?
        .map_err(|error| error.to_string())
}

fn record_asserted_cli_result((case, output): (CliActionCase, std::process::Output)) {
    action_scenarios::assert_sanitized(&output.stdout, case.key);
    action_scenarios::assert_sanitized(&output.stderr, case.key);
    let intent = action_matrix::intents()
        .iter()
        .find(|intent| intent.key() == case.key)
        .expect("authoritative CLI intent");
    let body = machine_json(&output);
    let (evidence, outcome_kind) = if output.status.success() {
        if !output.stdout.is_empty() || !output.stderr.is_empty() {
            let rendered = String::from_utf8_lossy(&output.stdout);
            assert!(
                body.is_some()
                    || (rendered.starts_with("[dry-run]")
                        && rendered.contains(case.key.split_once(':').unwrap().1)),
                "{} successful result was neither JSON nor its asserted dry-run contract",
                case.key
            );
        }
        if intent.minimum_evidence > EvidenceLevel::LiveSuccess {
            eprintln!(
                "CLI_UNCOVERED {} requires {:?}",
                case.key, intent.minimum_evidence
            );
            return;
        }
        (
            EvidenceLevel::LiveSuccess,
            "compiled_cli_success".to_owned(),
        )
    } else if body
        .as_ref()
        .is_some_and(|body| validated_nonzero_domain_result(case.key, body))
    {
        (
            EvidenceLevel::LiveSuccess,
            "validated_cli_domain_result".to_owned(),
        )
    } else {
        let body = body.unwrap_or_else(|| {
            panic!(
                "{} CLI error was not stable JSON: {}",
                case.key,
                String::from_utf8_lossy(&output.stderr)
            )
        });
        assert_eq!(body["ok"], false, "{} error envelope", case.key);
        let error_kind = body["error"]["kind"]
            .as_str()
            .unwrap_or_else(|| panic!("{} error omitted error.kind", case.key));
        let outcome_kind = action_scenarios::dedicated_contract_reason_for(case.key, Surface::Cli)
            .filter(|_| {
                action_scenarios::dedicated_contract_accepts_for(case.key, Surface::Cli, error_kind)
            })
            .map_or_else(
                || format!("compiled_cli_error:{error_kind}"),
                |reason| format!("dedicated_contract:{reason}:{error_kind}"),
            );
        (EvidenceLevel::LiveErrorPath, outcome_kind)
    };
    ActionOutcome {
        key: case.key.to_owned(),
        surface: Surface::Cli,
        disposition: action_scenarios::disposition(intent),
        evidence,
        owner: intent.scenario_owner,
        outcome_kind,
        recovery: "asserted_machine_readable_cli_result".into(),
        side_effects: "isolated_disposable_home".into(),
        canary_free: true,
    }
    .record();
}

fn validated_nonzero_domain_result(key: &str, body: &serde_json::Value) -> bool {
    match key {
        "doctor:audit.full" => validated_findings_for_services(
            body,
            &["access", "auth", "gateway", "lab", "oauth_relay", "system"],
        ),
        "doctor:system.checks" => validated_findings_for_services(body, &["lab", "system"]),
        "doctor:auth.check" => validated_doctor_findings(body, "auth", "auth:"),
        "doctor:oauth.relay.check" => validated_doctor_findings(body, "oauth_relay", "registry:"),
        "doctor:proxy.preflight" => validated_doctor_findings(body, "proxy", "proxy:"),
        _ => false,
    }
}

fn validated_doctor_findings(body: &serde_json::Value, service: &str, check_prefix: &str) -> bool {
    validated_findings(body)
        && body["findings"].as_array().is_some_and(|findings| {
            findings.iter().all(|finding| {
                finding["service"] == service
                    && finding["check"]
                        .as_str()
                        .is_some_and(|check| check.starts_with(check_prefix))
            })
        })
}

fn validated_findings_for_services(body: &serde_json::Value, services: &[&str]) -> bool {
    validated_findings(body)
        && body["findings"].as_array().is_some_and(|findings| {
            findings.iter().all(|finding| {
                finding["service"]
                    .as_str()
                    .is_some_and(|service| services.contains(&service))
            })
        })
}

fn validated_findings(body: &serde_json::Value) -> bool {
    body.get("findings")
        .and_then(serde_json::Value::as_array)
        .is_some_and(|findings| {
            !findings.is_empty()
                && findings.iter().all(|finding| {
                    finding
                        .get("service")
                        .and_then(serde_json::Value::as_str)
                        .is_some_and(|service| !service.is_empty())
                        && finding
                            .get("check")
                            .and_then(serde_json::Value::as_str)
                            .is_some_and(|check| !check.is_empty())
                        && finding
                            .get("severity")
                            .and_then(serde_json::Value::as_str)
                            .is_some_and(|severity| matches!(severity, "ok" | "warn" | "fail"))
                        && finding
                            .get("message")
                            .and_then(serde_json::Value::as_str)
                            .is_some()
                })
                && findings
                    .iter()
                    .any(|finding| matches!(finding["severity"].as_str(), Some("warn" | "fail")))
        })
}

#[test]
fn nonzero_cli_domain_results_require_an_exact_action_schema() {
    let valid = serde_json::json!({
        "findings": [{
            "service": "auth",
            "check": "auth:bearer-token",
            "severity": "fail",
            "message": "token is not configured"
        }]
    });
    assert!(validated_nonzero_domain_result("doctor:auth.check", &valid));
    let proxy = serde_json::json!({
        "findings": [{
            "service": "proxy",
            "check": "proxy:config",
            "severity": "fail",
            "message": "proxy configuration is unavailable"
        }]
    });
    assert!(validated_nonzero_domain_result(
        "doctor:proxy.preflight",
        &proxy
    ));
    assert!(!validated_nonzero_domain_result(
        "doctor:system.checks",
        &valid
    ));
    assert!(!validated_nonzero_domain_result(
        "gateway:gateway.get",
        &valid
    ));

    for malformed in [
        serde_json::json!({}),
        serde_json::json!({"findings": []}),
        serde_json::json!({"findings": [{"service": "auth", "check": "auth:x", "severity": "mystery", "message": "x"}]}),
        serde_json::json!({"findings": [{"service": "auth", "check": "wrong:x", "severity": "fail", "message": "x"}]}),
        serde_json::json!({"ok": true}),
    ] {
        assert!(!validated_nonzero_domain_result(
            "doctor:auth.check",
            &malformed
        ));
    }
}

fn machine_json(output: &std::process::Output) -> Option<serde_json::Value> {
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

fn record_observed_transition(key: &str) {
    record_workflow_outcome(
        key,
        EvidenceLevel::LiveStateTransition,
        "owned_cli_state_transition_observed",
    );
}

fn record_observed_success(key: &str) {
    record_workflow_outcome(
        key,
        EvidenceLevel::LiveSuccess,
        "owned_cli_success_observed",
    );
}

fn record_workflow_outcome(key: &str, evidence: EvidenceLevel, outcome_kind: &str) {
    let intent = action_matrix::intents()
        .iter()
        .find(|intent| intent.key() == key)
        .expect("workflow action intent");
    ActionOutcome {
        key: key.to_owned(),
        surface: Surface::Cli,
        disposition: action_scenarios::disposition(intent),
        evidence,
        owner: intent.scenario_owner,
        outcome_kind: outcome_kind.into(),
        recovery: "workflow_rollback_observed".into(),
        side_effects: "isolated_disposable_home".into(),
        canary_free: true,
    }
    .record();
}

fn cli_action_cases() -> std::collections::BTreeSet<CliActionCase> {
    const MISSING: &str = "matrix-missing";
    let cases: &[(&str, &[&str])] = &[
        ("doctor:audit.full", &["doctor", "--json"]),
        ("doctor:auth.check", &["doctor", "auth", "--json"]),
        (
            "doctor:oauth.relay.check",
            &["doctor", "oauth-relay", "--probe-targets", "--json"],
        ),
        (
            "doctor:proxy.check",
            &["doctor", "proxy", "--route", "/missing", "--json"],
        ),
        ("doctor:proxy.preflight", &["doctor", "proxy", "--json"]),
        ("doctor:system.checks", &["doctor", "system", "--json"]),
        (
            "gateway:gateway.add",
            &["gateway", "add", "--name", MISSING, "--json"],
        ),
        (
            "gateway:gateway.clients.list",
            &["gateway", "clients", "list", "--json"],
        ),
        (
            "gateway:gateway.code_mode.get",
            &["gateway", "code", "status", "--json"],
        ),
        (
            "gateway:gateway.code_mode.set",
            &["gateway", "code", "enable", "--json"],
        ),
        (
            "gateway:gateway.discover",
            &["gateway", "discover", "--clients", "not-a-client", "--json"],
        ),
        (
            "gateway:gateway.enrich.apply",
            &[
                "gateway",
                "enrich",
                "apply",
                "--upstream",
                MISSING,
                "--hint",
                "x",
                "--metadata-hash",
                "x",
                "--yes",
                "--json",
            ],
        ),
        (
            "gateway:gateway.enrich.preview",
            &[
                "gateway",
                "enrich",
                "--upstream",
                MISSING,
                "--yes",
                "--json",
            ],
        ),
        (
            "gateway:gateway.get",
            &["gateway", "get", MISSING, "--json"],
        ),
        (
            "gateway:gateway.import",
            &["gateway", "import", "--name", MISSING, "--yes", "--json"],
        ),
        (
            "gateway:gateway.import_pending.approve",
            &["gateway", "pending", "approve", MISSING, "--yes", "--json"],
        ),
        (
            "gateway:gateway.import_pending.list",
            &["gateway", "pending", "list", "--json"],
        ),
        (
            "gateway:gateway.import_pending.reject",
            &["gateway", "pending", "reject", MISSING, "--yes", "--json"],
        ),
        ("gateway:gateway.list", &["gateway", "list", "--json"]),
        (
            "gateway:gateway.loadout.add",
            &[
                "gateway",
                "loadout",
                "add",
                MISSING,
                "--service",
                "not-a-service",
                "--json",
            ],
        ),
        (
            "gateway:gateway.loadout.get",
            &["gateway", "loadout", "get", MISSING, "--json"],
        ),
        (
            "gateway:gateway.loadout.list_state",
            &["gateway", "loadout", "list", "--json"],
        ),
        (
            "gateway:gateway.loadout.patch",
            &[
                "gateway",
                "loadout",
                "update",
                MISSING,
                "--description",
                "x",
                "--json",
            ],
        ),
        (
            "gateway:gateway.loadout.remove",
            &["gateway", "loadout", "remove", MISSING, "--json"],
        ),
        (
            "gateway:gateway.loadout.stage_patch",
            &[
                "gateway",
                "loadout",
                "update",
                MISSING,
                "--description",
                "x",
                "--stage-for-restart",
                "--json",
            ],
        ),
        (
            "gateway:gateway.loadout.stage_remove",
            &[
                "gateway",
                "loadout",
                "remove",
                MISSING,
                "--stage-for-restart",
                "--json",
            ],
        ),
        (
            "gateway:gateway.mcp.cleanup",
            &["gateway", "mcp", "cleanup", MISSING, "--dry-run", "--json"],
        ),
        (
            "gateway:gateway.mcp.disable",
            &["gateway", "mcp", "disable", MISSING, "--json"],
        ),
        (
            "gateway:gateway.mcp.enable",
            &["gateway", "mcp", "enable", MISSING, "--json"],
        ),
        (
            "gateway:gateway.mcp.list",
            &["gateway", "mcp", "list", "--json"],
        ),
        (
            "gateway:gateway.mcp.restart",
            &["gateway", "mcp", "restart", MISSING, "--json"],
        ),
        (
            "gateway:gateway.oauth.clear",
            &["gateway", "mcp", "auth", "clear", MISSING, "--json"],
        ),
        (
            "gateway:gateway.oauth.google_revoke",
            &[
                "gateway",
                "mcp",
                "auth",
                "revoke-google",
                MISSING,
                "--confirm",
                "--json",
            ],
        ),
        (
            "gateway:gateway.oauth.start",
            &["gateway", "mcp", "auth", "start", MISSING, "--json"],
        ),
        (
            "gateway:gateway.oauth.status",
            &["gateway", "mcp", "auth", "status", MISSING, "--json"],
        ),
        (
            "gateway:gateway.oauth.wait",
            &[
                "gateway",
                "mcp",
                "auth",
                "start",
                MISSING,
                "--wait",
                "--wait-timeout-secs",
                "1",
                "--json",
            ],
        ),
        ("gateway:gateway.protected_route.add", &PROTECTED_ADD),
        (
            "gateway:gateway.protected_route.get",
            &["gateway", "protected-route", "get", MISSING, "--json"],
        ),
        (
            "gateway:gateway.protected_route.list_state",
            &["gateway", "protected-route", "list", "--json"],
        ),
        (
            "gateway:gateway.protected_route.remove",
            &["gateway", "protected-route", "remove", MISSING, "--json"],
        ),
        (
            "gateway:gateway.protected_route.stage_add",
            &PROTECTED_STAGE_ADD,
        ),
        (
            "gateway:gateway.protected_route.stage_remove",
            &[
                "gateway",
                "protected-route",
                "remove",
                MISSING,
                "--stage-for-restart",
                "--json",
            ],
        ),
        (
            "gateway:gateway.protected_route.stage_update",
            &PROTECTED_STAGE_UPDATE,
        ),
        ("gateway:gateway.protected_route.test", &PROTECTED_TEST),
        ("gateway:gateway.protected_route.update", &PROTECTED_UPDATE),
        (
            "gateway:gateway.public_urls.get",
            &["gateway", "public-urls", "--json"],
        ),
        ("gateway:gateway.reload", &["gateway", "reload", "--json"]),
        (
            "gateway:gateway.remove",
            &["gateway", "remove", MISSING, "--json"],
        ),
        (
            "gateway:gateway.skills.list",
            &["gateway", "skills", "list", "--upstream", MISSING, "--json"],
        ),
        (
            "gateway:gateway.test",
            &["gateway", "test", "--name", MISSING, "--json"],
        ),
        (
            "gateway:gateway.update",
            &[
                "gateway",
                "update",
                MISSING,
                "--url",
                "http://127.0.0.1:9/mcp",
                "--json",
            ],
        ),
        (
            "gateway:gateway.usage.calls",
            &["gateway", "usage", "calls", "--limit", "1", "--json"],
        ),
        (
            "gateway:gateway.usage.metrics",
            &[
                "gateway",
                "usage",
                "metrics",
                "--bucket-count",
                "1",
                "--json",
            ],
        ),
        (
            "gateway:gateway.virtual_server.quarantine.list",
            &["gateway", "quarantine", "list", "--json"],
        ),
        (
            "gateway:gateway.virtual_server.quarantine.restore",
            &["gateway", "quarantine", "restore", MISSING, "--json"],
        ),
        (
            "server_logs:server_logs.query",
            &["logs", "--no-follow", "--container", MISSING, "--json"],
        ),
        ("setup:check", &["setup", "check", "--json"]),
        (
            "setup:draft.discard",
            &["setup", "draft", "discard", "--dry-run", "--json"],
        ),
        (
            "setup:plugin.install",
            &["setup", "install-plugin", MISSING, "--dry-run", "--json"],
        ),
        (
            "setup:plugin.uninstall",
            &["setup", "uninstall-plugin", MISSING, "--dry-run", "--json"],
        ),
        (
            "setup:plugin_connectivity",
            &[
                "setup",
                "plugin-connectivity",
                "--server-url",
                "http://127.0.0.1:9",
                "--json",
            ],
        ),
        ("setup:plugin_export", &["setup", "plugin-export", "--json"]),
        (
            "setup:plugin_hook",
            &["setup", "plugin-hook", "--no-repair", "--json"],
        ),
        ("setup:plugin_sync", &["setup", "plugin-sync", "--json"]),
        (
            "setup:plugins.installed",
            &["setup", "installed-plugins", "--json"],
        ),
        (
            "setup:proxy.configure",
            &[
                "setup",
                "proxy",
                "--server-url",
                "http://127.0.0.1:9",
                "--json",
            ],
        ),
        ("setup:repair", &["setup", "repair", "--json"]),
        (
            "setup:services.status",
            &["setup", "services-status", "--json"],
        ),
        ("setup:state", &["setup", "--json"]),
        (
            "snippets:snippets.create",
            &["snippets", "create", MISSING, "--json"],
        ),
        (
            "snippets:snippets.exec",
            &["snippets", "exec", MISSING, "--json"],
        ),
        (
            "snippets:snippets.get",
            &["snippets", "get", MISSING, "--json"],
        ),
        ("snippets:snippets.list", &["snippets", "list", "--json"]),
        (
            "snippets:snippets.remove",
            &["snippets", "remove", MISSING, "--yes", "--json"],
        ),
        (
            "snippets:snippets.test",
            &["snippets", "test", MISSING, "--json"],
        ),
        (
            "snippets:snippets.validate",
            &["snippets", "validate", MISSING, "--json"],
        ),
    ];
    cases
        .iter()
        .map(|(key, argv)| CliActionCase { key, argv })
        .collect()
}

const PROTECTED_ADD: [&str; 14] = [
    "gateway",
    "protected-route",
    "add",
    "--name",
    "matrix-missing",
    "--public-host",
    "invalid.test",
    "--public-path",
    "/mcp",
    "--upstream",
    "matrix-missing",
    "--enabled",
    "true",
    "--json",
];
const PROTECTED_STAGE_ADD: [&str; 13] = [
    "gateway",
    "protected-route",
    "add",
    "--name",
    "matrix-missing",
    "--public-host",
    "invalid.test",
    "--public-path",
    "/mcp",
    "--upstream",
    "matrix-missing",
    "--stage-for-restart",
    "--json",
];
const PROTECTED_UPDATE: [&str; 13] = [
    "gateway",
    "protected-route",
    "update",
    "matrix-missing",
    "--public-host",
    "invalid.test",
    "--public-path",
    "/mcp",
    "--upstream",
    "matrix-missing",
    "--enabled",
    "true",
    "--json",
];
const PROTECTED_STAGE_UPDATE: [&str; 12] = [
    "gateway",
    "protected-route",
    "update",
    "matrix-missing",
    "--public-host",
    "invalid.test",
    "--public-path",
    "/mcp",
    "--upstream",
    "matrix-missing",
    "--stage-for-restart",
    "--json",
];
const PROTECTED_TEST: [&str; 12] = [
    "gateway",
    "protected-route",
    "test",
    "--name",
    "matrix-missing",
    "--public-host",
    "invalid.test",
    "--public-path",
    "/mcp",
    "--upstream",
    "matrix-missing",
    "--json",
];

#[tokio::test]
async fn compiled_cli_service_probes_use_stable_json_and_isolated_state() {
    tokio::time::timeout(MATRIX_DEADLINE, async {
        let root = tempfile::tempdir().expect("isolated CLI matrix root");
        let fixtures = action_scenarios::fixtures();
        let mut tasks = tokio::task::JoinSet::new();
        for fixture in fixtures
            .values()
            .filter(|fixture| fixture.cli_probe.is_some())
        {
            let home = root.path().join(&fixture.service);
            std::fs::create_dir_all(home.join("tmp")).unwrap();
            let args = fixture.cli_probe.clone().unwrap();
            let service = fixture.service.clone();
            tasks.spawn(async move {
                let output = action_scenarios::run_cli_probe(&home, &args).await?;
                Ok::<_, String>((service, output))
            });
            assert!(
                tasks.len() <= MAX_CHILDREN,
                "CLI child concurrency exceeded bound"
            );
            if tasks.len() == MAX_CHILDREN {
                let result = tasks.join_next().await.unwrap().unwrap().unwrap();
                action_scenarios::assert_json_or_help(&result.1, &result.0);
            }
        }
        while let Some(result) = tasks.join_next().await {
            let (service, output) = result.unwrap().unwrap();
            action_scenarios::assert_json_or_help(&output, &service);
        }
    })
    .await
    .expect("CLI matrix absolute deadline");
}

#[test]
fn mutation_capable_cli_services_are_explicit_and_disposable() {
    let fixtures = action_scenarios::fixtures();
    let mutable = fixtures
        .values()
        .filter(|fixture| {
            fixture.can_mutate
                && action_matrix::intents().iter().any(|intent| {
                    intent.service == fixture.service
                        && intent.applicable_surfaces.contains(&Surface::Cli)
                })
        })
        .map(|fixture| fixture.service.as_str())
        .collect::<std::collections::BTreeSet<_>>();
    assert_eq!(
        mutable,
        std::collections::BTreeSet::from(["gateway", "setup", "snippets"])
    );
    let known = action_matrix::intents()
        .iter()
        .map(action_matrix::CaseIntent::key)
        .collect::<std::collections::BTreeSet<_>>();
    for fixture in fixtures.values().filter(|fixture| {
        action_matrix::intents().iter().any(|intent| {
            intent.service == fixture.service && intent.applicable_surfaces.contains(&Surface::Cli)
        })
    }) {
        for action in [
            Some(&fixture.success_action),
            Some(&fixture.invalid_action),
            fixture.policy_action.as_ref(),
        ]
        .into_iter()
        .flatten()
        {
            assert!(
                known.contains(&format!("{}:{action}", fixture.service)),
                "{} fixture references unknown action {action}",
                fixture.service
            );
        }
        assert_eq!(
            fixture.can_mutate,
            !fixture.workflow.is_empty(),
            "{} workflow ownership drifted",
            fixture.service
        );
    }
}

#[tokio::test]
async fn stateful_cli_workflows_observe_mutations_and_always_roll_them_back() {
    tokio::time::timeout(MATRIX_DEADLINE, async {
        let root = tempfile::tempdir().expect("owned workflow root");
        let home = root.path();
        std::fs::create_dir_all(home.join("tmp")).unwrap();

        // Snippets: create -> read -> validate -> remove -> prove absence.
        let create = action_scenarios::run_cli(
            home,
            &[
                "snippets",
                "create",
                "matrix-owned",
                "--code",
                "async () => ({ ok: true })",
                "--json",
            ],
        )
        .await
        .unwrap();
        action_scenarios::assert_success_json(&create, "snippets.create");
        let get = action_scenarios::run_cli(home, &["snippets", "get", "matrix-owned", "--json"])
            .await
            .unwrap();
        action_scenarios::assert_success_json(&get, "snippets.get");
        assert!(String::from_utf8_lossy(&get.stdout).contains("matrix-owned"));
        record_observed_transition("snippets:snippets.create");
        record_observed_success("snippets:snippets.get");
        let validate =
            action_scenarios::run_cli(home, &["snippets", "validate", "matrix-owned", "--json"])
                .await
                .unwrap();
        action_scenarios::assert_success_json(&validate, "snippets.validate");
        record_observed_success("snippets:snippets.validate");
        let remove = action_scenarios::run_cli(
            home,
            &["snippets", "remove", "matrix-owned", "--yes", "--json"],
        )
        .await
        .unwrap();
        action_scenarios::assert_success_json(&remove, "snippets.remove");
        let absent =
            action_scenarios::run_cli(home, &["snippets", "get", "matrix-owned", "--json"])
                .await
                .unwrap();
        assert!(
            !absent.status.success(),
            "removed snippet remained observable"
        );
        action_scenarios::assert_sanitized(&absent.stdout, "snippets.absent");
        action_scenarios::assert_sanitized(&absent.stderr, "snippets.absent");
        record_observed_transition("snippets:snippets.remove");

        // Setup: discard is destructive, but only the harness-owned draft may
        // be touched. Both dry-run interruption and authorized cleanup are
        // observable at the filesystem boundary.
        let draft = home.join(".labby/.env.draft");
        std::fs::create_dir_all(draft.parent().unwrap()).unwrap();
        std::fs::write(&draft, "LABBY_MATRIX_VALUE=owned\n").unwrap();
        let dry_run =
            action_scenarios::run_cli(home, &["setup", "draft", "discard", "--dry-run", "--json"])
                .await
                .unwrap();
        assert!(dry_run.status.success(), "setup dry-run failed");
        action_scenarios::assert_sanitized(&dry_run.stdout, "setup.draft.discard.dry_run");
        action_scenarios::assert_sanitized(&dry_run.stderr, "setup.draft.discard.dry_run");
        assert!(
            String::from_utf8_lossy(&dry_run.stdout).contains("draft.discard"),
            "dry-run did not bind the canonical action"
        );
        assert!(draft.exists(), "interrupted discard changed state");
        let discard =
            action_scenarios::run_cli(home, &["setup", "draft", "discard", "--yes", "--json"])
                .await
                .unwrap();
        action_scenarios::assert_success_json(&discard, "setup.draft.discard");
        assert!(
            !draft.exists(),
            "authorized discard did not clean owned state"
        );
        record_observed_transition("setup:draft.discard");

        // Gateway: create -> read -> update -> delete uses only deliberately
        // unreachable loopback endpoints, so no external service or credential
        // can be contacted while durable configuration is exercised.
        let gateway_daemon = live_labby::LiveLabbyBuilder::new()
            .env("LABBY_E2E_BOOTSTRAP_STATIC_OWNER", "1")
            .env("LABBY_E2E_TEAM_ID", "bootstrap-initial-team")
            .start()
            .await
            .expect("stateful gateway daemon");
        let gateway_add = action_scenarios::run_cli_against(
            home,
            &[
                "gateway",
                "add",
                "--name",
                "matrix-owned",
                "--url",
                "http://127.0.0.1:9/mcp",
                "--json",
            ],
            &gateway_daemon,
        )
        .await
        .unwrap();
        action_scenarios::assert_success_json(&gateway_add, "gateway.add");
        let gateway_get = action_scenarios::run_cli_against(
            home,
            &["gateway", "get", "matrix-owned", "--json"],
            &gateway_daemon,
        )
        .await
        .unwrap();
        action_scenarios::assert_success_json(&gateway_get, "gateway.get");
        assert!(String::from_utf8_lossy(&gateway_get.stdout).contains("127.0.0.1:9"));
        record_observed_transition("gateway:gateway.add");
        record_observed_success("gateway:gateway.get");
        let reload = action_scenarios::run_cli_against(
            home,
            &["gateway", "reload", "--json"],
            &gateway_daemon,
        )
        .await
        .unwrap();
        let reload = action_scenarios::assert_success_json(&reload, "gateway.reload");
        assert_eq!(
            reload["completed"], true,
            "gateway reload did not complete its runtime reconciliation"
        );
        record_observed_transition("gateway:gateway.reload");
        let gateway_update = action_scenarios::run_cli_against(
            home,
            &[
                "gateway",
                "update",
                "matrix-owned",
                "--url",
                "http://127.0.0.1:10/mcp",
                "--json",
            ],
            &gateway_daemon,
        )
        .await
        .unwrap();
        action_scenarios::assert_success_json(&gateway_update, "gateway.update");
        let gateway_updated = action_scenarios::run_cli_against(
            home,
            &["gateway", "get", "matrix-owned", "--json"],
            &gateway_daemon,
        )
        .await
        .unwrap();
        action_scenarios::assert_success_json(&gateway_updated, "gateway.get.updated");
        assert!(
            String::from_utf8_lossy(&gateway_updated.stdout).contains("127.0.0.1:10"),
            "gateway update was not observable"
        );
        record_observed_transition("gateway:gateway.update");
        let gateway_remove = action_scenarios::run_cli_against(
            home,
            &["gateway", "remove", "matrix-owned", "--json"],
            &gateway_daemon,
        )
        .await
        .unwrap();
        action_scenarios::assert_success_json(&gateway_remove, "gateway.remove");
        let gateway_absent = action_scenarios::run_cli_against(
            home,
            &["gateway", "get", "matrix-owned", "--json"],
            &gateway_daemon,
        )
        .await
        .unwrap();
        assert!(
            !gateway_absent.status.success(),
            "removed gateway remained observable"
        );
        action_scenarios::assert_sanitized(&gateway_absent.stdout, "gateway.absent");
        action_scenarios::assert_sanitized(&gateway_absent.stderr, "gateway.absent");
        record_observed_transition("gateway:gateway.remove");

        // Code Mode: observe both sides of the setting transition and restore
        // the isolated home to its initial disabled posture.
        let enable = action_scenarios::run_cli_against(
            home,
            &["gateway", "code", "enable", "--json"],
            &gateway_daemon,
        )
        .await
        .unwrap();
        action_scenarios::assert_success_json(&enable, "gateway.code_mode.enable");
        let enabled = action_scenarios::run_cli_against(
            home,
            &["gateway", "code", "status", "--json"],
            &gateway_daemon,
        )
        .await
        .unwrap();
        let enabled = action_scenarios::assert_success_json(&enabled, "gateway.code_mode.enabled");
        assert_eq!(enabled["enabled"], true, "Code Mode did not enable");
        let disable = action_scenarios::run_cli_against(
            home,
            &["gateway", "code", "disable", "--json"],
            &gateway_daemon,
        )
        .await
        .unwrap();
        action_scenarios::assert_success_json(&disable, "gateway.code_mode.disable");
        let disabled = action_scenarios::run_cli_against(
            home,
            &["gateway", "code", "status", "--json"],
            &gateway_daemon,
        )
        .await
        .unwrap();
        let disabled =
            action_scenarios::assert_success_json(&disabled, "gateway.code_mode.disabled");
        assert_eq!(disabled["enabled"], false, "Code Mode cleanup failed");
        record_observed_transition("gateway:gateway.code_mode.set");

        // Missing transport parameters prove the stable invalid-input contract
        // after cleanup and cannot silently fall back to ambient configuration.
        let invalid = action_scenarios::run_cli_against(
            home,
            &["gateway", "test", "--json"],
            &gateway_daemon,
        )
        .await
        .unwrap();
        assert!(
            !invalid.status.success(),
            "invalid gateway proposal unexpectedly succeeded"
        );
        action_scenarios::assert_sanitized(&invalid.stdout, "gateway.invalid");
        action_scenarios::assert_sanitized(&invalid.stderr, "gateway.invalid");
        let cleanup = gateway_daemon.finish().await;
        assert!(
            cleanup.is_clean(),
            "stateful gateway cleanup: {:?}",
            cleanup.failures
        );
    })
    .await
    .expect("stateful workflows absolute deadline");
}

#[tokio::test]
async fn legacy_cli_aliases_reach_the_same_dispatch_contract() {
    let root = tempfile::tempdir().expect("alias root");
    std::fs::create_dir_all(root.path().join("tmp")).unwrap();
    for (command, canonical) in [
        ("install-plugin", "plugin.install"),
        ("uninstall-plugin", "plugin.uninstall"),
    ] {
        let output = action_scenarios::run_cli(
            root.path(),
            &["setup", command, "matrix-missing", "--dry-run", "--json"],
        )
        .await
        .unwrap();
        action_scenarios::assert_sanitized(&output.stdout, command);
        action_scenarios::assert_sanitized(&output.stderr, command);
        assert!(output.status.success(), "{command} alias dry-run failed");
        let rendered = String::from_utf8_lossy(&output.stdout);
        assert!(
            rendered.contains(canonical),
            "CLI alias did not resolve to {canonical}: {rendered}"
        );
    }
    for command in ["installed-plugins", "services-status"] {
        let output = action_scenarios::run_cli(root.path(), &["setup", command, "--help"])
            .await
            .unwrap();
        action_scenarios::assert_json_or_help(&output, command);
    }
}

#[tokio::test]
async fn explicit_remote_failure_never_falls_back_or_creates_local_state() {
    let root = tempfile::tempdir().expect("explicit remote root");
    std::fs::create_dir_all(root.path().join("tmp")).unwrap();
    let mut command = tokio::process::Command::from(live_labby::isolated_command(root.path()));
    command
        .env("LABBY_SERVER_URL", "http://127.0.0.1:9")
        .args(["gateway", "list", "--json"]);
    let output = tokio::time::timeout(action_scenarios::CHILD_DEADLINE, command.output())
        .await
        .expect("explicit remote failure deadline")
        .unwrap();
    assert!(
        !output.status.success(),
        "explicit remote silently fell back"
    );
    action_scenarios::assert_sanitized(&output.stdout, "explicit remote");
    action_scenarios::assert_sanitized(&output.stderr, "explicit remote");
    assert!(!root.path().join(".labby/config.toml").exists());
    assert!(!root.path().join(".labby/auth.db").exists());
}
