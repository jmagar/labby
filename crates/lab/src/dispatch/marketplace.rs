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
mod stash_bridge;
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
#[allow(clippy::panic)]
mod tests {
    use lab_apis::stash::StashOrigin;
    use serde_json::{Value, json};
    use tempfile::tempdir;
    use tokio::runtime::Builder;

    const ARTIFACT_ACTIONS: &[(&str, bool, &str)] = &[
        ("artifact.fork", true, "ForkResponse"),
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
    async fn dispatch_artifact_fork_returns_not_found_for_unknown_plugin_source() {
        let err = super::dispatch(
            "artifact.fork",
            json!({"plugin_id": "missing@local", "artifacts": ["agents/demo.md"]}),
        )
        .await
        .unwrap_err();
        assert_ne!(err.kind(), "not_implemented");
    }

    #[test]
    fn dispatch_artifact_fork_creates_stash_component_for_file_artifact() {
        let dir = tempdir().unwrap();
        let home = dir.path();
        let plugins = home.join(".claude").join("plugins");
        let plugin = plugins
            .join("marketplaces")
            .join("demo-market")
            .join("demo-plugin");
        std::fs::create_dir_all(plugin.join("agents")).unwrap();
        std::fs::write(plugin.join("plugin.json"), r#"{"name":"demo-plugin"}"#).unwrap();
        std::fs::write(plugin.join("agents/demo.md"), "# Agent\n").unwrap();
        std::fs::write(
            plugins.join("known_marketplaces.json"),
            json!({
                "demo-market": {
                    "installLocation": plugins.join("marketplaces").join("demo-market")
                }
            })
            .to_string(),
        )
        .unwrap();

        let stash_root = home.join(".lab").join("stash");
        let result = super::client::with_test_plugins_root(home, || {
            crate::dispatch::stash::client::with_test_stash_root(stash_root.clone(), || {
                Builder::new_current_thread()
                    .enable_all()
                    .build()
                    .unwrap()
                    .block_on(async {
                        super::dispatch(
                            "artifact.fork",
                            json!({
                                "plugin_id": "demo-plugin@demo-market",
                                "artifacts": ["agents/demo.md"]
                            }),
                        )
                        .await
                    })
            })
        })
        .unwrap();

        let fork = &result["forks"][0];
        let component_id = fork["component_id"].as_str().unwrap();
        assert!(!component_id.is_empty());
        assert!(!fork["revision_id"].as_str().unwrap().is_empty());
        assert_eq!(fork["forked_artifacts"], json!(["agents/demo.md"]));

        let store = crate::dispatch::stash::store::StashStore::new(stash_root.clone());
        let component = store.read_component(component_id).unwrap().unwrap();
        match component.origin_meta.unwrap() {
            StashOrigin::Marketplace(origin) => {
                assert_eq!(origin.plugin_id, "demo-plugin@demo-market");
                assert_eq!(origin.artifact_path.as_deref(), Some("agents/demo.md"));
            }
            other => panic!("unexpected origin: {other:?}"),
        }
        assert!(
            store
                .workspace_dir(component_id)
                .join("agents/demo.md")
                .exists()
        );
        assert!(
            stash_root
                .join("marketplace")
                .join(component_id)
                .join("base/agents/demo.md")
                .exists()
        );

        let preview = super::client::with_test_plugins_root(home, || {
            crate::dispatch::stash::client::with_test_stash_root(stash_root, || {
                Builder::new_current_thread()
                    .enable_all()
                    .build()
                    .unwrap()
                    .block_on(async {
                        super::dispatch(
                            "artifact.update.preview",
                            json!({
                                "plugin_id": "demo-plugin@demo-market",
                                "artifact_path": "agents/demo.md"
                            }),
                        )
                        .await
                    })
            })
        })
        .unwrap();
        assert_eq!(
            preview["plugin_id"],
            Value::String("demo-plugin@demo-market".into())
        );
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
