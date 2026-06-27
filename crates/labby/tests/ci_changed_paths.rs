use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{SystemTime, UNIX_EPOCH};

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("crate lives under crates/labby")
        .to_path_buf()
}

fn classify(event: &str, files: &[&str]) -> HashMap<String, String> {
    let temp_dir = std::env::temp_dir().join(format!(
        "lab-ci-paths-{}-{}-{}",
        std::process::id(),
        files.len(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system time after unix epoch")
            .as_nanos()
    ));
    drop(fs::remove_dir_all(&temp_dir));
    fs::create_dir_all(&temp_dir).expect("create temp dir");
    let changed = temp_dir.join("changed.txt");
    let output = temp_dir.join("github_output.txt");
    fs::write(&changed, files.join("\n")).expect("write changed file list");

    let status = Command::new("python3")
        .arg(repo_root().join("scripts/ci/changed_paths.py"))
        .arg("--event")
        .arg(event)
        .arg("--changed-files")
        .arg(&changed)
        .arg("--output")
        .arg(&output)
        .stdout(Stdio::null())
        .status()
        .expect("run changed_paths.py");
    assert!(status.success(), "changed_paths.py exited with {status}");

    let raw = fs::read_to_string(&output).expect("read github output");
    raw.lines()
        .map(|line| {
            let (key, value) = line.split_once('=').expect("key=value output");
            (key.to_string(), value.to_string())
        })
        .collect()
}

#[test]
fn docs_only_changes_skip_expensive_runtime_categories() {
    let out = classify(
        "pull_request",
        &[
            "docs/runtime/CICD.md",
            "docs/sessions/2026-06-27-example.md",
        ],
    );
    assert_eq!(out["docs"], "true");
    assert_eq!(out["rust_compile"], "false");
    assert_eq!(out["rust_test"], "false");
    assert_eq!(out["web"], "false");
    assert_eq!(out["docker"], "false");
    assert_eq!(out["security"], "false");
    assert_eq!(out["release"], "false");
    assert_eq!(out["docs_check"], "true");
}

#[test]
fn rust_changes_enable_compile_test_security_release_and_container_smoke() {
    let out = classify("pull_request", &["crates/labby/src/dispatch/gateway.rs"]);
    assert_eq!(out["rust_compile"], "true");
    assert_eq!(out["rust_test"], "true");
    assert_eq!(out["security"], "true");
    assert_eq!(out["release"], "true");
    assert_eq!(out["docker"], "true");
    assert_eq!(out["web"], "false");
}

#[test]
fn rust_manifest_changes_compile_without_full_tests() {
    let out = classify("pull_request", &["Cargo.toml"]);
    assert_eq!(out["rust_compile"], "true");
    assert_eq!(out["rust_test"], "false");
    assert_eq!(out["security"], "false");
    assert_eq!(out["release"], "true");
}

#[test]
fn frontend_changes_enable_web_release_and_container_without_rust_tests() {
    let out = classify("pull_request", &["apps/gateway-admin/app/page.tsx"]);
    assert_eq!(out["web"], "true");
    assert_eq!(out["release"], "true");
    assert_eq!(out["docker"], "true");
    assert_eq!(out["rust_compile"], "false");
    assert_eq!(out["rust_test"], "false");
    assert_eq!(out["security"], "false");
}

#[test]
fn explicit_policy_files_route_to_the_right_checks() {
    let actionlint = classify("pull_request", &[".github/actionlint.yaml"]);
    assert_eq!(actionlint["workflow"], "true");

    let gitleaks = classify("pull_request", &[".gitleaksignore"]);
    assert_eq!(gitleaks["security"], "true");
    assert_eq!(gitleaks["rust_compile"], "false");
    assert_eq!(gitleaks["rust_test"], "false");

    let deny = classify("pull_request", &["deny.toml"]);
    assert_eq!(deny["security"], "true");
    assert_eq!(deny["rust_compile"], "true");
    assert_eq!(deny["rust_test"], "false");

    let generated_doc = classify("pull_request", &["docs/generated/cli-help.md"]);
    assert_eq!(generated_doc["docs_check"], "true");
    assert_eq!(generated_doc["rust_compile"], "false");
    assert_eq!(generated_doc["rust_test"], "false");
}

#[test]
fn workflow_changes_enable_everything() {
    let out = classify("pull_request", &[".github/workflows/ci.yml"]);
    for (key, value) in out {
        assert_eq!(value, "true", "{key} should be true for workflow changes");
    }
}

#[test]
fn scheduled_and_manual_runs_enable_everything() {
    for event in ["schedule", "workflow_dispatch"] {
        let out = classify(event, &["docs/runtime/CICD.md"]);
        for (key, value) in out {
            assert_eq!(value, "true", "{key} should be true for {event}");
        }
    }
}

#[test]
fn ci_workflow_uses_changed_path_classifier_and_stable_gate() {
    let workflow =
        fs::read_to_string(repo_root().join(".github/workflows/ci.yml")).expect("read ci.yml");

    assert!(
        workflow.contains("  changes:"),
        "CI must define a changes job"
    );
    assert!(
        workflow.contains("scripts/ci/changed_paths.py"),
        "CI must run the changed-path classifier"
    );
    assert!(
        workflow.contains("needs.changes.outputs.rust_compile"),
        "CI jobs must use changed-path outputs"
    );
    assert!(
        workflow.contains("needs.changes.outputs.rust_test"),
        "full test jobs must be separately gated from compile jobs"
    );
    assert!(
        workflow.contains("needs.changes.outputs.docs_check"),
        "generated docs freshness must have an explicit routing category"
    );
    assert!(
        workflow.contains("  ci-gate:"),
        "CI must expose a stable aggregate ci-gate job"
    );
    assert!(
        workflow.contains("success|skipped"),
        "ci-gate must accept intentionally skipped jobs"
    );
}
