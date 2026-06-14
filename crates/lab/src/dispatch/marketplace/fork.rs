//! Marketplace artifact fork lifecycle stubs.
//!
//! Full fork lifecycle behavior belongs to `lab-iut1.3`. This module wires the
//! action surface with stable signatures and structured placeholder errors.

use serde_json::Value;

use crate::dispatch::error::ToolError;
use crate::dispatch::marketplace::params::{
    ArtifactListParams, ArtifactResetParams, ForkParams, UnforkParams,
};

pub(super) async fn artifact_fork(params: ForkParams) -> Result<Value, ToolError> {
    crate::dispatch::marketplace::stash_bridge::fork_artifacts(&params.plugin_id, params.artifacts)
        .await
}

pub(super) async fn artifact_list(params: ArtifactListParams) -> Result<Value, ToolError> {
    crate::dispatch::marketplace::stash_bridge::list_forks(params.plugin_id).await
}

pub(super) async fn artifact_unfork(params: UnforkParams) -> Result<Value, ToolError> {
    tracing::info!(
        surface = "dispatch",
        service = "marketplace",
        action = "artifact.unfork",
        plugin_id = %params.plugin_id,
        "destructive action intent: removing marketplace fork from stash"
    );
    crate::dispatch::marketplace::stash_bridge::unfork(&params.plugin_id, params.artifacts).await
}

pub(super) async fn artifact_reset(params: ArtifactResetParams) -> Result<Value, ToolError> {
    tracing::info!(
        surface = "dispatch",
        service = "marketplace",
        action = "artifact.reset",
        plugin_id = %params.plugin_id,
        "destructive action intent: resetting forked artifact from base snapshot"
    );
    crate::dispatch::marketplace::stash_bridge::reset(&params.plugin_id, params.artifacts).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dispatch::marketplace::params::{
        parse_artifact_reset_params, parse_unfork_params, parse_update_apply_params,
    };
    use serde_json::json;

    #[tokio::test]
    async fn artifact_list_empty_when_no_forks_exist() {
        let result = artifact_list(ArtifactListParams {
            plugin_id: None,
            instance: None,
        })
        .await
        .unwrap();
        let rows = result.as_array().unwrap();
        assert!(rows.is_empty());
    }

    #[tokio::test]
    async fn destructive_marketplace_actions_are_confirmed_before_dispatch() {
        let actions = crate::dispatch::marketplace::actions();
        for name in ["artifact.unfork", "artifact.reset", "artifact.update.apply"] {
            let spec = actions
                .iter()
                .find(|action| action.name == name)
                .expect(name);
            assert!(spec.destructive, "{name} must remain cataloged destructive");
        }
    }

    #[test]
    fn destructive_artifact_parsers_do_not_require_confirm_after_surface_gate() {
        assert!(parse_unfork_params(&json!({"plugin_id": "demo@labby"})).is_ok());
        assert!(parse_artifact_reset_params(&json!({"plugin_id": "demo@labby"})).is_ok());
        assert!(parse_update_apply_params(&json!({"plugin_id": "demo@labby"})).is_ok());
    }
}
