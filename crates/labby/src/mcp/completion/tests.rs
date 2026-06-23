//! Tests for completion helpers. Distributed from `server.rs`
//! (bead `lab-kvji.24.1.6`). Hosts the shared `completion_test_registry`
//! fixture (also duplicated in `handlers_tools/tests.rs` to keep each
//! `tests.rs` self-contained per the test-distribution plan).

use super::complete_prompt_arg;
use crate::dispatch::error::ToolError;
use crate::registry::{RegisteredService, ToolRegistry};
use labby_apis::core::action::ActionSpec;
use serde_json::Value;
use std::future::Future;
use std::pin::Pin;

const TEST_ACTIONS_ONE: &[ActionSpec] = &[
    ActionSpec {
        name: "queue.list",
        description: "List queue",
        destructive: false,
        requires_admin: false,
        params: &[],
        returns: "object",
    },
    ActionSpec {
        name: "movie.search",
        description: "Search movies",
        destructive: false,
        requires_admin: false,
        params: &[],
        returns: "object",
    },
];

const TEST_ACTIONS_TWO: &[ActionSpec] = &[
    ActionSpec {
        name: "calendar.list",
        description: "List calendar",
        destructive: false,
        requires_admin: false,
        params: &[],
        returns: "object",
    },
    ActionSpec {
        name: "movie.lookup",
        description: "Look up movie",
        destructive: false,
        requires_admin: false,
        params: &[],
        returns: "object",
    },
];

fn noop_dispatch(
    _action: String,
    _params: Value,
) -> Pin<Box<dyn Future<Output = Result<Value, ToolError>> + Send>> {
    Box::pin(async { Ok(Value::Null) })
}

fn completion_test_registry() -> ToolRegistry {
    let mut registry = ToolRegistry::new();
    registry.register(RegisteredService {
        name: "radarr",
        description: "Movies",
        category: "media",
        kind: crate::registry::RegisteredServiceKind::BuiltInUpstreamApi,
        status: "available",
        actions: TEST_ACTIONS_ONE,
        dispatch: noop_dispatch,
    });
    registry.register(RegisteredService {
        name: "sonarr",
        description: "Shows",
        category: "media",
        kind: crate::registry::RegisteredServiceKind::BuiltInUpstreamApi,
        status: "available",
        actions: TEST_ACTIONS_TWO,
        dispatch: noop_dispatch,
    });
    registry
}

#[test]
fn completion_run_action_empty_action_prefix_uses_cached_action_names() {
    let registry = completion_test_registry();

    let completion = complete_prompt_arg(&registry, "run-action", "action", "");

    assert_eq!(completion.values, registry.action_name_completions(""));
    assert_eq!(completion.total, Some(registry.action_names().len() as u32));
    assert_eq!(completion.has_more, Some(false));
}

#[test]
fn completion_run_action_action_prefix_filters_cached_action_names() {
    let registry = completion_test_registry();

    let completion = complete_prompt_arg(&registry, "run-action", "action", "movie.");

    assert_eq!(
        completion.values,
        vec!["movie.lookup".to_string(), "movie.search".to_string()]
    );
}

#[test]
fn completion_prompt_service_arguments_filter_service_names() {
    let registry = completion_test_registry();

    let run_action = complete_prompt_arg(&registry, "run-action", "service", "ra");
    let discover = complete_prompt_arg(&registry, "service-discover", "service", "so");

    assert_eq!(run_action.values, vec!["radarr".to_string()]);
    assert_eq!(discover.values, vec!["sonarr".to_string()]);
}

#[test]
fn completion_unknown_prompt_argument_returns_empty_result() {
    let registry = completion_test_registry();

    let completion = complete_prompt_arg(&registry, "run-action", "params", "{");

    assert!(completion.values.is_empty());
    assert_eq!(completion.total, Some(0));
    assert_eq!(completion.has_more, Some(false));
}
