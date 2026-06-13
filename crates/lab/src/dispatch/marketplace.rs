//! Shared dispatch layer for the unified marketplace service.
//!
//! Covers three item types:
//! - Plugins (Claude Code marketplaces, cherry-pick from installed plugins)
//! - MCP Servers (from MCP Registry CDN — absorbed from dispatch/mcpregistry)
//! - ACP Agents (from cdn.agentclientprotocol.com registry JSON)

mod acp_catalog;
mod acp_client;
mod acp_dispatch;
mod backend;
mod backends;
mod catalog;
mod client;
mod diff;
mod dispatch;
mod fork;
mod mcp_catalog;
pub(crate) mod mcp_client;
mod mcp_dispatch;
mod mcp_params;
mod package;
mod params;
mod patch;
mod runtime;
pub(crate) mod service;
pub(crate) mod stash_meta;
pub mod store;
pub mod sync;
mod update;

pub use catalog::actions;
pub use client::NodeRpcPort;
pub use dispatch::{dispatch, dispatch_with_port};
pub use mcp_params::resolve_search_for_rest;
pub const LAB_REGISTRY_META_NAMESPACE: &str = "tv.tootie.lab/registry";

#[cfg(test)]
mod tests {
    use serde_json::json;

    const ARTIFACT_ACTIONS: &[(&str, bool, &str)] = &[
        ("artifact.fork", false, "ForkResult"),
        ("artifact.list", false, "ForkedPluginStatus[]"),
        ("artifact.unfork", true, "UnforkResult"),
        ("artifact.reset", true, "ResetResult"),
        ("artifact.diff", false, "ArtifactDiffResult"),
        ("artifact.patch", false, "PatchResult"),
        ("artifact.update.check", false, "UpdateCheckResult[]"),
        ("artifact.update.preview", false, "UpdatePreviewResult"),
        ("artifact.update.apply", true, "ApplyResult"),
        ("artifact.merge.suggest", false, "MergeSuggestResult"),
        ("artifact.config.set", false, "ConfigSetResult"),
    ];

    #[test]
    fn catalog_includes_all_artifact_action_specs() {
        let actions = super::actions();
        for (name, destructive, returns) in ARTIFACT_ACTIONS {
            let spec = actions
                .iter()
                .find(|spec| spec.name == *name)
                .unwrap_or_else(|| panic!("missing action spec for {name}"));
            assert_eq!(spec.destructive, *destructive, "{name} destructive flag");
            assert_eq!(spec.returns, *returns, "{name} returns");
        }
    }

    #[tokio::test]
    async fn help_lists_artifact_actions() {
        let help = super::dispatch("help", json!({})).await.unwrap();
        let rendered = serde_json::to_string(&help).unwrap();
        for (name, _, _) in ARTIFACT_ACTIONS {
            assert!(rendered.contains(name), "help missing {name}");
        }
    }

    #[tokio::test]
    async fn dispatch_with_client_artifact_fork_roundtrip() {
        let err = super::dispatch(
            "artifact.fork",
            json!({"plugin_id": "demo@local", "artifacts": ["agents/demo.md"]}),
        )
        .await
        .unwrap_err();
        assert_eq!(err.kind(), "not_implemented");
    }

    #[tokio::test]
    async fn dispatch_returns_unknown_action_for_invalid_artifact_action() {
        let err = super::dispatch("artifact.bogus", json!({}))
            .await
            .unwrap_err();
        assert_eq!(err.kind(), "unknown_action");
    }

    #[test]
    fn parse_fork_params_validates_artifact_paths() {
        let params = super::params::parse_fork_params(
            &json!({"plugin_id": "demo@local", "artifacts": ["../secret"]}),
        );
        assert_eq!(params.unwrap_err().kind(), "invalid_param");
    }

    #[test]
    fn parse_update_apply_params_validates_strategy_values() {
        let err = super::params::parse_update_apply_params(
            &json!({"plugin_id": "demo@local", "strategy": "overwrite_everything"}),
        )
        .unwrap_err();
        assert_eq!(err.kind(), "invalid_param");

        for strategy in ["keep_mine", "take_upstream", "always_ask", "ai_suggest"] {
            let parsed = super::params::parse_update_apply_params(
                &json!({"plugin_id": "demo@local", "strategy": strategy}),
            )
            .unwrap();
            assert!(parsed.strategy.is_some());
        }
    }
}
