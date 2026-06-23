//! Public types for the deploy service.

use serde::{Deserialize, Serialize};

/// Inputs for `deploy.plan` / `deploy.run` / `deploy.rollback`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeployRequest {
    /// Explicit list of SSH aliases to deploy to. If empty, the dispatch layer
    /// rejects the request; there is no implicit "all" in V1.
    #[serde(default)]
    pub targets: Vec<String>,
    /// Maximum number of hosts to work on in parallel. `None` falls back to
    /// the config default (safe default: 1).
    #[serde(default)]
    pub max_parallel: Option<u32>,
    /// Abort remaining hosts on the first failure.
    #[serde(default)]
    pub fail_fast: bool,
    /// Operator confirmation required by the destructive gate. The dispatch
    /// layer is responsible for rejecting `confirm: true` when the MCP
    /// caller did not complete live elicitation (headless-bypass rejection).
    #[serde(default)]
    pub confirm: bool,
}

/// Per-host resolved configuration shown by `deploy.plan`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeployPlanHost {
    /// SSH alias used to address this host.
    pub alias: String,
    /// Resolved `HostName` from `~/.ssh/config`, if present.
    pub hostname: Option<String>,
    /// SSH user from `~/.ssh/config`, if present.
    pub ssh_user: Option<String>,
    /// SSH port from `~/.ssh/config`, if present.
    pub port: Option<u16>,
    /// Remote filesystem path where the binary will be installed.
    pub remote_path: String,
    /// Systemd unit that will be restarted, if configured.
    pub service: Option<String>,
    /// Systemd scope (`system` or `user`), if configured.
    pub service_scope: Option<String>,
    /// Whether this host is in the canary group.
    pub canary: bool,
}

/// Per-role artifact information included in plan and run summary responses.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeployArtifactSummary {
    /// Artifact role: `"controller"` or `"node"`.
    pub role: String,
    /// Filesystem path of the artifact.
    pub path: String,
    /// SHA-256 hex digest of the artifact, if known.
    pub sha256: Option<String>,
}

/// Output of `deploy.plan` — what `run` would do if invoked now.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeployPlan {
    pub artifact_path: String,
    pub artifact_sha256: Option<String>,
    /// Per-role artifact information. Additive field — populated when
    /// multiple artifact roles are required (controller + node split).
    #[serde(default)]
    pub artifacts: Vec<DeployArtifactSummary>,
    /// Per-host resolved SSH target and install config.
    pub host_details: Vec<DeployPlanHost>,
    pub max_parallel: u32,
    pub canary_hosts: Vec<String>,
}

/// Stage of the deploy pipeline that was most recently reached for a host.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DeployStage {
    Resolve,
    Build,
    Preflight,
    Transfer,
    Install,
    Restart,
    Verify,
    PhoneHome,
}

/// Per-host result row in `DeployRunSummary`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeployHostResult {
    pub host: String,
    pub reached_stage: DeployStage,
    pub succeeded: bool,
    /// True when sha256 matched and transfer was skipped.
    pub skipped_transfer: bool,
    pub transferred_bytes: Option<u64>,
    /// Stable kind; full detail at local WARN only.
    pub error_kind: Option<String>,
    pub stage_timings_ms: std::collections::BTreeMap<String, u128>,
}

/// Reachability state of a monitored host.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HostStatus {
    Online,
    Offline,
}

/// A single state-change event emitted by `deploy monitor`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HostStatusEvent {
    /// Unix timestamp (seconds).
    pub ts: u64,
    pub host: String,
    pub status: HostStatus,
    /// Address and port that was probed.
    pub addr: String,
}

/// Result of `deploy.run` / `deploy.rollback`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeployRunSummary {
    pub run_id: String,
    pub artifact_sha256: String,
    /// Per-role artifact information. Additive field — populated when
    /// multiple artifact roles were built (controller + node split).
    #[serde(default)]
    pub artifacts: Vec<DeployArtifactSummary>,
    pub hosts: Vec<DeployHostResult>,
    pub succeeded: usize,
    pub failed: usize,
    /// `true` iff `failed == 0`.
    pub ok: bool,
}
