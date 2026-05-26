//! Deploy service — push the local `lab` release binary to SSH targets
//! with end-to-end integrity verification.
//!
//! This module is type-only: `DeployRequest`, `DeployPlan`, `DeployStage`,
//! `DeployHostResult`, `DeployRunSummary`, and `DeployError`. All
//! orchestration (build, transfer, install, restart, verify) lives in the
//! `lab` binary's shared dispatch layer at `crates/lab/src/dispatch/deploy/`.
//!
//! `deploy` is a synthetic service: it has no upstream API. It reuses the
//! `Bootstrap` category.

pub mod error;
pub mod types;

pub use error::DeployError;
pub use types::{
    DeployArtifactSummary, DeployHostResult, DeployPlan, DeployPlanHost, DeployRequest,
    DeployRunSummary, DeployStage, HostStatus, HostStatusEvent,
};

use crate::core::plugin::{Category, PluginMeta};

/// Compile-time metadata for the deploy module.
pub const META: PluginMeta = PluginMeta {
    name: "deploy",
    display_name: "Deploy",
    description: "Build-and-push the lab release binary to SSH targets with integrity verification",
    category: Category::Bootstrap,
    docs_url: "",
    required_env: &[],
    optional_env: &[],
    default_port: None,
    supports_multi_instance: false,
};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn meta_is_named_deploy() {
        assert_eq!(META.name, "deploy");
    }

    #[test]
    fn deploy_request_defaults_are_safe() {
        let r = DeployRequest::default();
        assert!(r.targets.is_empty());
        assert_eq!(r.max_parallel, None);
        assert!(!r.fail_fast);
        assert!(!r.confirm);
    }
}
