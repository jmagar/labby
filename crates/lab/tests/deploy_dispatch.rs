#![allow(
    clippy::await_holding_lock,
    clippy::bool_assert_comparison,
    clippy::err_expect,
    clippy::field_reassign_with_default,
    clippy::float_cmp,
    clippy::len_zero,
    clippy::manual_string_new,
    clippy::needless_raw_string_hashes,
    clippy::panic,
    clippy::single_char_pattern,
    clippy::unnested_or_patterns
)]
#![cfg(feature = "deploy")]

use labby::dispatch::deploy;
use serde_json::json;

#[test]
fn catalog_lists_required_actions() {
    let names: Vec<&str> = deploy::ACTIONS.iter().map(|a| a.name).collect();
    for required in ["help", "schema", "config.list", "plan", "run", "rollback"] {
        assert!(names.contains(&required), "missing action: {required}");
    }
}

#[test]
fn run_and_rollback_are_destructive_and_others_are_not() {
    for action in deploy::ACTIONS {
        let expect_destructive = matches!(action.name, "run" | "rollback");
        assert_eq!(
            action.destructive, expect_destructive,
            "{} destructive flag wrong",
            action.name
        );
    }
}

#[tokio::test]
async fn unknown_action_returns_stable_kind() {
    let err = deploy::dispatch("not.a.real.action", json!({}))
        .await
        .unwrap_err();
    assert_eq!(err.kind(), "unknown_action");
}

#[tokio::test]
async fn help_lists_run_and_rollback() {
    let v = deploy::dispatch("help", json!({})).await.unwrap();
    assert!(v.is_object());
    let actions = v["actions"].as_array().expect("actions array");
    let names: Vec<&str> = actions
        .iter()
        .map(|a| a["name"].as_str().unwrap())
        .collect();
    assert!(names.contains(&"run"));
    assert!(names.contains(&"rollback"));
}

#[tokio::test]
async fn run_returns_internal_error_without_runner() {
    // dispatch() (no-runner entry point) immediately surfaces internal_error for
    // any action that requires the runner, without running auth or param validation.
    let err = deploy::dispatch("run", json!({ "confirm": true }))
        .await
        .unwrap_err();
    assert_eq!(err.kind(), "internal_error");
}
