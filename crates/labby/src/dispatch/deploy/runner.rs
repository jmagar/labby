//! Deploy orchestrator.
//!
//! V1 scope: trait + in-process default runner that drives the
//! build → preflight → transfer → install → restart → verify pipeline over
//! SSH using `tokio::process::Command` (wrapped in `SshSession`). Tests in
//! this crate substitute a recording `HostIo` mock for fast orchestration
//! coverage without a live SSH server.
//!
//! # `host='?'` sentinel convention
//!
//! Stage functions (`preflight`, `transfer_and_install`, `restart`, `verify`)
//! do not know which host alias the caller used — they operate on a generic
//! `HostIo`. When these functions construct a [`labby_apis::deploy::DeployError`]
//! variant that carries a `host` field they fill it with the string `"?"` as a
//! sentinel meaning "not yet resolved". The sentinel is then replaced by the
//! real host alias at the call site via [`host_err`], which receives the
//! resolved `host: &str` and constructs the final [`DeployHostResult`].
//!
//! Callers that forget to route errors through `host_err` will emit `host='?'`
//! in production logs. A [`debug_assert!`] inside `host_err` catches this
//! class of bug during tests (see [`host_err`] docs).
//!
//! # Shell-exception audit
//!
//! Only two code paths construct a `sh -c` command:
//!
//! * `preflight`'s canary-write probe — the canary path is derived from
//!   `remote_path`, which is allowlist-validated in `params::validate_remote_path`.
//! * `SshSession::upload_stream` — remote redirect needs a shell wrapper.
//!
//! Every other `io.run(&[...])` call uses per-token argv and interpolates no
//! untrusted strings.

use std::future::Future;
use std::path::Path;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Instant;

use super::host_io::SshHostIo;
// Re-export HostIo so existing callers (including tests/deploy_runner.rs)
// can still import it from this module path.
pub use super::host_io::HostIo;
use super::ssh_session::SshHostTarget;
use super::stages::{preflight, restart, transfer_and_install, verify};
use labby_apis::deploy::{
    DeployError, DeployHostResult, DeployPlan, DeployRequest, DeployRunSummary, DeployStage,
};
use serde_json::{Value, json};

use crate::config::ServiceScope;
use crate::dispatch::error::ToolError;

use super::build::{self, BuildOutcome};
use super::lock::HostLockRegistry;
use super::params;
use super::ssh_session::shell_quote;

/// Default per-host lock-acquire timeout for deploy and rollback operations.
///
/// 300 s (5 min) is intentionally generous: a large binary transfer over a
/// slow link can take 2–3 minutes. Set `DEPLOY_LOCK_TIMEOUT_SECS` or wire
/// `DeployDefaults::lock_timeout_secs` (future bead) to override at runtime.
const DEFAULT_LOCK_TIMEOUT_SECS: u64 = 300;

// Kept for external test implementations; not yet used in production surface code.
#[allow(dead_code, async_fn_in_trait)]
pub trait DeployRunner: Send + Sync {
    async fn plan(&self, req: DeployRequest) -> Result<DeployPlan, ToolError>;
    async fn run(&self, req: DeployRequest) -> Result<DeployRunSummary, ToolError>;
    async fn rollback(&self, req: DeployRequest) -> Result<DeployRunSummary, ToolError>;
    async fn config_list(&self) -> Result<Value, ToolError>;
}

// ── DefaultRunner ──────────────────────────────────────────────────────────

/// Default in-process runner wired into `SshSession` + the on-disk inventory.
pub struct DefaultRunner {
    pub config: crate::config::DeployPreferences,
    pub ssh_inventory: Arc<Vec<SshHostTarget>>,
    pub locks: Arc<HostLockRegistry>,
}

impl DefaultRunner {
    /// Construct a new default runner.
    #[must_use]
    pub fn new(
        config: crate::config::DeployPreferences,
        ssh_inventory: Arc<Vec<SshHostTarget>>,
        locks: Arc<HostLockRegistry>,
    ) -> Self {
        Self {
            config,
            ssh_inventory,
            locks,
        }
    }

    pub fn resolve_target(&self, alias: &str) -> Option<&SshHostTarget> {
        self.ssh_inventory.iter().find(|h| h.alias == alias)
    }

    fn effective_max_parallel(&self) -> Option<u32> {
        self.config.defaults.as_ref().and_then(|d| d.max_parallel)
    }

    fn effective_remote_path(&self, host: &str) -> String {
        self.config
            .hosts
            .get(host)
            .and_then(|o| o.remote_path.clone())
            .or_else(|| {
                self.config
                    .defaults
                    .as_ref()
                    .and_then(|d| d.remote_path.clone())
            })
            .unwrap_or_else(|| "/usr/local/bin/labby".to_string())
    }

    fn effective_unit(&self, host: &str) -> Option<String> {
        // An explicit empty string in host config means "no service for this host"
        // and short-circuits the fallback to defaults.
        if let Some(host_cfg) = self.config.hosts.get(host) {
            match host_cfg.service.as_deref() {
                Some("") => return None,
                Some(s) => return Some(s.to_string()),
                None => {}
            }
        }
        self.config
            .defaults
            .as_ref()
            .and_then(|d| d.service.clone())
            .filter(|s| !s.is_empty())
    }

    fn effective_scope(&self, host: &str) -> Option<ServiceScope> {
        self.config
            .hosts
            .get(host)
            .and_then(|o| o.service_scope)
            .or_else(|| self.config.defaults.as_ref().and_then(|d| d.service_scope))
    }

    fn canary_set(&self) -> std::collections::BTreeSet<String> {
        self.config
            .defaults
            .as_ref()
            .map(|d| d.canary_hosts.iter().cloned().collect())
            .unwrap_or_default()
    }

    fn partition_canary(&self, targets: &[String]) -> (Vec<String>, Vec<String>) {
        let set = self.canary_set();
        let mut canary = Vec::new();
        let mut rest = Vec::new();
        for t in targets {
            if set.contains(t) {
                canary.push(t.clone());
            } else {
                rest.push(t.clone());
            }
        }
        (canary, rest)
    }
}

// Thin trait delegation — keeps async fn in trait semantics without
// the HRTB Send limitation that blocks Box::pin in the MCP registry.
// Actual implementations live as inherent pub async fn methods below.
impl DeployRunner for DefaultRunner {
    async fn plan(&self, req: DeployRequest) -> Result<DeployPlan, ToolError> {
        self.plan_impl(req).await
    }

    async fn run(&self, req: DeployRequest) -> Result<DeployRunSummary, ToolError> {
        self.run_impl(req).await
    }

    async fn rollback(&self, req: DeployRequest) -> Result<DeployRunSummary, ToolError> {
        self.rollback_impl(req).await
    }

    async fn config_list(&self) -> Result<Value, ToolError> {
        self.config_list_impl()
    }
}

// Inherent implementations — called directly by dispatch_with_runner so the
// future type is concrete (not an RPITIT from a trait), making Send provable.
//
// IMPORTANT: all `&self` accesses must occur BEFORE any `.await` point.
// Borrowing `self` across an await creates lifetime-parameterised captures
// that fail the higher-ranked Send check required by `Box::pin` in the MCP
// registry (Rust issue #100013). Extract all needed values synchronously,
// then hand off only owned / Arc values to the async work.
impl DefaultRunner {
    pub fn plan_impl(
        &self,
        req: DeployRequest,
    ) -> Pin<Box<dyn Future<Output = Result<DeployPlan, ToolError>> + Send + 'static>> {
        // --- sync: all self access before creating the future ---
        for alias in &req.targets {
            if self.resolve_target(alias).is_none() {
                let err: Result<DeployPlan, ToolError> = Err(DeployError::ValidationFailed {
                    field: "targets".into(),
                    reason: format!("unknown SSH alias: {alias}"),
                }
                .into());
                return Box::pin(async move { err });
            }
        }
        let canary_set = self.canary_set();
        let canary_hosts: Vec<String> = canary_set.iter().cloned().collect();
        let max_parallel = req
            .max_parallel
            .or_else(|| self.effective_max_parallel())
            .unwrap_or(1)
            .max(1);

        let host_details: Vec<labby_apis::deploy::DeployPlanHost> = req
            .targets
            .iter()
            .filter_map(|alias| {
                let target = self.resolve_target(alias)?;
                Some(labby_apis::deploy::DeployPlanHost {
                    alias: alias.clone(),
                    hostname: target.hostname.clone(),
                    ssh_user: target.user.clone(),
                    port: target.port,
                    remote_path: self.effective_remote_path(alias),
                    service: self.effective_unit(alias),
                    service_scope: self
                        .effective_scope(alias)
                        .map(|s| format!("{s:?}").to_lowercase()),
                    canary: canary_set.contains(alias),
                })
            })
            .collect();

        // Collect per-role artifact paths for the plan summary.
        let needed_roles: std::collections::HashSet<crate::config::ArtifactRole> = req
            .targets
            .iter()
            .map(|alias| self.resolve_artifact_role(alias))
            .collect();
        let role_artifact_paths: Vec<(crate::config::ArtifactRole, std::path::PathBuf)> =
            needed_roles
                .into_iter()
                .map(|role| {
                    let profile = match role {
                        crate::config::ArtifactRole::Controller => {
                            build::ArtifactProfile::controller()
                        }
                        crate::config::ArtifactRole::Node => build::ArtifactProfile::node(),
                    };
                    let path = build::expected_artifact_path_for_profile(&profile);
                    (role, path)
                })
                .collect();

        // --- async: only owned values, no &self ---
        Box::pin(async move {
            let artifact = build::expected_artifact_path("labby");
            let artifact_sha256 = if matches!(artifact.try_exists(), Ok(true)) {
                let p = artifact.clone();
                tokio::task::spawn_blocking(move || build::sha256_file_blocking(&p))
                    .await
                    .map_err(|e| ToolError::internal_message(format!("sha256 join: {e}")))?
                    .ok()
            } else {
                None
            };
            let artifacts = role_artifact_paths
                .into_iter()
                .map(|(role, path)| labby_apis::deploy::DeployArtifactSummary {
                    role: format!("{role:?}").to_ascii_lowercase(),
                    path: path.to_string_lossy().into_owned(),
                    sha256: None,
                })
                .collect();
            Ok(DeployPlan {
                artifact_path: artifact.to_string_lossy().into_owned(),
                artifact_sha256,
                artifacts,
                host_details,
                max_parallel,
                canary_hosts,
            })
        })
    }

    pub fn run_impl(
        &self,
        req: DeployRequest,
    ) -> Pin<Box<dyn Future<Output = Result<DeployRunSummary, ToolError>> + Send + 'static>> {
        use tracing::Instrument;

        // --- sync: all self access before creating the future ---
        for alias in &req.targets {
            if self.resolve_target(alias).is_none() {
                let err: Result<DeployRunSummary, ToolError> = Err(DeployError::ValidationFailed {
                    field: "targets".into(),
                    reason: format!("unknown SSH alias: {alias}"),
                }
                .into());
                return Box::pin(async move { err });
            }
        }
        let max_parallel = req
            .max_parallel
            .or_else(|| self.effective_max_parallel())
            .unwrap_or(1)
            .max(1) as usize;
        let (canary, rest) = self.partition_canary(&req.targets);
        let canary_jobs = self.build_jobs(&canary);
        let rest_jobs = self.build_jobs(&rest);
        // Collect the set of artifact roles needed across all jobs (sync, before async block).
        let needed_roles: std::collections::HashSet<crate::config::ArtifactRole> = canary_jobs
            .iter()
            .chain(rest_jobs.iter())
            .map(|j| j.artifact_role)
            .collect();
        // Extract build timeout from config (sync, before async block).
        let build_timeout_secs = self
            .config
            .defaults
            .as_ref()
            .and_then(|d| d.build_timeout_secs);
        let locks = self.locks.clone();
        let run_id = uuid::Uuid::new_v4().to_string();
        let span = tracing::info_span!(
            "deploy.run",
            run_id = %run_id,
            service = "deploy",
            surface = "dispatch",
        );

        // --- async: only owned / Arc values, no &self ---
        Box::pin(
            async move {
                let runner_started = Instant::now();
                tracing::info!(
                    surface = "dispatch",
                    service = "deploy",
                    action = "runner.start",
                    operation = "run",
                    target_count = canary_jobs.len() + rest_jobs.len(),
                    max_parallel,
                    canary_count = canary_jobs.len(),
                    "deploy runner startup",
                );

                // Build each required role exactly once.
                let mut artifact_map: std::collections::HashMap<
                    crate::config::ArtifactRole,
                    Arc<BuildOutcome>,
                > = std::collections::HashMap::new();
                for role in &needed_roles {
                    let mut profile = match role {
                        crate::config::ArtifactRole::Controller => {
                            build::ArtifactProfile::controller()
                        }
                        crate::config::ArtifactRole::Node => build::ArtifactProfile::node(),
                    };
                    if let Some(secs) = build_timeout_secs {
                        profile.build_timeout_secs = Some(secs);
                    }
                    let outcome = match build::build_artifact(&profile).await {
                        Ok(o) => o,
                        Err(err) => {
                            tracing::warn!(
                                surface = "dispatch",
                                service = "deploy",
                                action = "runner.shutdown",
                                operation = "run",
                                elapsed_ms = runner_started.elapsed().as_millis(),
                                kind = err.kind(),
                                ok = false,
                                "deploy runner shutdown",
                            );
                            return Err(err.into());
                        }
                    };
                    // build_artifact() already emits a conforming build.finish event;
                    // no duplicate logging needed here.
                    artifact_map.insert(*role, Arc::new(outcome));
                }

                // Pick a representative sha256 for the summary (controller preferred,
                // then node, then empty — matches the single-binary common case).
                let summary_sha256 = artifact_map
                    .get(&crate::config::ArtifactRole::Controller)
                    .or_else(|| artifact_map.get(&crate::config::ArtifactRole::Node))
                    .map(|o| o.sha256.clone())
                    .unwrap_or_default();

                let mut all_results: Vec<DeployHostResult> = Vec::new();

                if !canary_jobs.is_empty() {
                    let canary_results = DefaultRunner::run_jobs(
                        canary_jobs,
                        artifact_map.clone(),
                        1,
                        req.fail_fast,
                        run_id.clone(),
                        locks.clone(),
                    )
                    .await;
                    let any_failed = canary_results.iter().any(|r| !r.succeeded);
                    all_results.extend(canary_results);
                    if req.fail_fast && any_failed {
                        for host in &rest {
                            all_results.push(aborted_result(host));
                        }
                        let summary = summarize(run_id, summary_sha256, all_results);
                        tracing::info!(
                            surface = "dispatch",
                            service = "deploy",
                            action = "runner.shutdown",
                            operation = "run",
                            elapsed_ms = runner_started.elapsed().as_millis(),
                            succeeded = summary.succeeded,
                            failed = summary.failed,
                            ok = summary.ok,
                            "deploy runner shutdown",
                        );
                        return Ok(summary);
                    }
                }

                if !rest_jobs.is_empty() {
                    let rest_results = DefaultRunner::run_jobs(
                        rest_jobs,
                        artifact_map,
                        max_parallel,
                        req.fail_fast,
                        run_id.clone(),
                        locks,
                    )
                    .await;
                    all_results.extend(rest_results);
                }

                let summary = summarize(run_id, summary_sha256, all_results);
                tracing::info!(
                    surface = "dispatch",
                    service = "deploy",
                    action = "runner.shutdown",
                    operation = "run",
                    elapsed_ms = runner_started.elapsed().as_millis(),
                    succeeded = summary.succeeded,
                    failed = summary.failed,
                    ok = summary.ok,
                    "deploy runner shutdown",
                );
                Ok(summary)
            }
            .instrument(span),
        )
    }

    pub fn rollback_impl(
        &self,
        req: DeployRequest,
    ) -> Pin<Box<dyn Future<Output = Result<DeployRunSummary, ToolError>> + Send + 'static>> {
        use tracing::Instrument;

        // --- sync: collect all per-host data from self before creating the future ---
        struct HostRollback {
            host: String,
            target: Option<SshHostTarget>,
            remote_path: String,
            unit: Option<String>,
            scope: Option<ServiceScope>,
        }
        let host_data: Vec<HostRollback> = req
            .targets
            .iter()
            .map(|host| HostRollback {
                target: self.resolve_target(host).cloned(),
                remote_path: self.effective_remote_path(host),
                unit: self.effective_unit(host),
                scope: self.effective_scope(host),
                host: host.clone(),
            })
            .collect();
        let locks = self.locks.clone();
        let max_parallel = self.effective_max_parallel().unwrap_or(1).max(1) as usize;
        let run_id = uuid::Uuid::new_v4().to_string();
        let span = tracing::info_span!(
            "deploy.rollback",
            run_id = %run_id,
            service = "deploy",
            surface = "dispatch",
        );

        // --- async: only owned / Arc values, no &self ---
        Box::pin(
            async move {
                use futures::stream::{self, StreamExt};

                let runner_started = Instant::now();
                tracing::info!(
                    surface = "dispatch",
                    service = "deploy",
                    action = "runner.start",
                    operation = "rollback",
                    target_count = host_data.len(),
                    max_parallel,
                    "deploy runner startup",
                );

                let results = stream::iter(host_data)
                    .map(|data| {
                        let locks = locks.clone();
                        async move {
                            let Some(target) = data.target else {
                                return DeployHostResult {
                                    host: data.host,
                                    reached_stage: DeployStage::Resolve,
                                    succeeded: false,
                                    skipped_transfer: false,
                                    transferred_bytes: None,
                                    error_kind: Some("validation_failed".into()),
                                    stage_timings_ms: Default::default(),
                                };
                            };
                            let lock_timeout =
                                std::time::Duration::from_secs(DEFAULT_LOCK_TIMEOUT_SECS);
                            let _guard = match locks.acquire(&data.host, lock_timeout).await {
                                Ok(g) => g,
                                Err(e) => {
                                    return DeployHostResult {
                                        host: data.host,
                                        reached_stage: DeployStage::Resolve,
                                        succeeded: false,
                                        skipped_transfer: false,
                                        transferred_bytes: None,
                                        error_kind: Some(e.kind().to_string()),
                                        stage_timings_ms: Default::default(),
                                    };
                                }
                            };
                            let io = Arc::new(SshHostIo::new(data.host.clone(), target));
                            rollback_one_host(
                                io,
                                data.host,
                                data.remote_path,
                                data.unit,
                                data.scope,
                            )
                            .await
                        }
                    })
                    .buffer_unordered(max_parallel)
                    .collect::<Vec<_>>()
                    .await;

                let summary = summarize(run_id, String::new(), results);
                tracing::info!(
                    surface = "dispatch",
                    service = "deploy",
                    action = "runner.shutdown",
                    operation = "rollback",
                    elapsed_ms = runner_started.elapsed().as_millis(),
                    succeeded = summary.succeeded,
                    failed = summary.failed,
                    ok = summary.ok,
                    "deploy runner shutdown",
                );
                Ok(summary)
            }
            .instrument(span),
        )
    }

    pub fn config_list_impl(&self) -> Result<Value, ToolError> {
        let hosts: Vec<&str> = self
            .ssh_inventory
            .iter()
            .map(|h| h.alias.as_str())
            .collect();
        let overrides: Vec<&String> = self.config.hosts.keys().collect();
        Ok(json!({
            "defaults": self.config.defaults,
            "hosts": hosts,
            "overrides": overrides,
        }))
    }
}

/// Read `~/.ssh/config` and return the parsed fleet inventory.
///
/// File and env I/O lives here in the `lab` crate, not in `lab-apis`.
/// `labby_apis::core::ssh::parse_ssh_config` is the pure parser; this
/// function supplies the contents.
fn load_deploy_inventory() -> Vec<SshHostTarget> {
    let home = match std::env::var("HOME") {
        Ok(h) => h,
        Err(_) => {
            tracing::warn!("deploy: $HOME not set; SSH inventory will be empty");
            return Vec::new();
        }
    };
    let path = std::path::PathBuf::from(home).join(".ssh").join("config");
    let contents = match std::fs::read_to_string(&path) {
        Ok(c) => c,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(e) => {
            tracing::warn!(error = %e, path = %path.display(), "deploy: could not read ~/.ssh/config");
            return Vec::new();
        }
    };

    // Read denylist from env (comma-separated), defaulting to github.com.
    const DEFAULT_DENYLIST: &[&str] = &["github.com"];
    let denylist_raw = std::env::var("LAB_DEPLOY_SSH_DENYLIST").ok();
    let denylist: std::collections::BTreeSet<String> = match denylist_raw.as_deref() {
        Some(raw) if !raw.is_empty() => raw
            .split(',')
            .map(|s| s.trim().to_ascii_lowercase())
            .filter(|s| !s.is_empty())
            .collect(),
        _ => DEFAULT_DENYLIST
            .iter()
            .map(|s| s.to_ascii_lowercase())
            .collect(),
    };

    labby_apis::core::ssh::parse_ssh_config(&contents)
        .into_iter()
        .filter(|t| !denylist.contains(&t.alias.to_ascii_lowercase()))
        .collect()
}

/// Build a `DefaultRunner` from the loaded config and `~/.ssh/config`
/// inventory.
///
/// Failures loading the SSH inventory are treated as non-fatal — they
/// produce an empty inventory (useful so `config.list` still works) and
/// log a warning. Both CLI and MCP surfaces call this at dispatch time
/// rather than at startup, keeping the construction surface-neutral.
pub fn build_default_runner(config: crate::config::DeployPreferences) -> DefaultRunner {
    let inventory = load_deploy_inventory();
    DefaultRunner::new(
        config,
        Arc::new(inventory),
        Arc::new(HostLockRegistry::default()),
    )
}

/// Process-global `DefaultRunner` initialised once at first MCP dispatch.
///
/// The MCP registry dispatch closure must return a `'static`-compatible
/// future, so the runner must live in a static rather than a local.
/// CLI dispatch owns its runner directly (config is threaded in), so
/// only the MCP path uses this slot.
static MCP_RUNNER: std::sync::OnceLock<DefaultRunner> = std::sync::OnceLock::new();

/// Return a reference to the process-global `DefaultRunner`, initialising
/// it from on-disk config and `~/.ssh/config` on first call.
///
/// Config load failures are non-fatal: the runner is built with default
/// preferences so that `help` / `schema` / `config.list` still work.
pub fn mcp_runner() -> &'static DefaultRunner {
    MCP_RUNNER.get_or_init(|| {
        let prefs = crate::config::load_toml(&crate::config::toml_candidates())
            .ok()
            .and_then(|cfg| cfg.deploy)
            .unwrap_or_default();
        build_default_runner(prefs)
    })
}

/// Parameters for a single-host run, pre-resolved so the orchestrator can
/// fan out without capturing `&self` across an `await` boundary.
#[derive(Clone)]
struct HostJob {
    host: String,
    target: SshHostTarget,
    remote_path: String,
    unit: Option<String>,
    scope: Option<ServiceScope>,
    master_url: Option<String>,
    /// Artifact role for this host, resolved from per-host override or defaults.
    artifact_role: crate::config::ArtifactRole,
}

impl DefaultRunner {
    fn master_url(&self) -> Option<String> {
        self.config
            .defaults
            .as_ref()
            .and_then(|d| d.master_url.clone())
    }

    /// Resolve the artifact role for a host: per-host override → defaults → Node.
    fn resolve_artifact_role(&self, host: &str) -> crate::config::ArtifactRole {
        self.config
            .hosts
            .get(host)
            .and_then(|h| h.artifact_role)
            .or_else(|| self.config.defaults.as_ref().and_then(|d| d.artifact_role))
            .unwrap_or(crate::config::ArtifactRole::Node)
    }

    fn build_jobs(&self, hosts: &[String]) -> Vec<HostJob> {
        let master_url = self.master_url();
        hosts
            .iter()
            .filter_map(|h| {
                let target = self.resolve_target(h).cloned()?;
                Some(HostJob {
                    host: h.clone(),
                    target,
                    remote_path: self.effective_remote_path(h),
                    unit: self.effective_unit(h),
                    scope: self.effective_scope(h),
                    master_url: master_url.clone(),
                    artifact_role: self.resolve_artifact_role(h),
                })
            })
            .collect()
    }

    /// Fan out `jobs` at `max_parallel` concurrency, honoring fail-fast.
    ///
    /// Each job carries its own `artifact_role`; the caller supplies an
    /// `artifacts` map built once before this call so each role is compiled
    /// at most once even when multiple hosts share a role.
    ///
    /// `locks` is passed by `Arc` so the stream closures do not borrow
    /// `&self` — a Rust 2024 RPIT-lifetime-capture limitation (#100013)
    /// rejects `&self`-capturing futures inside `buffer_unordered` when the
    /// surrounding function returns `impl Future`.
    async fn run_jobs(
        jobs: Vec<HostJob>,
        artifacts: std::collections::HashMap<crate::config::ArtifactRole, Arc<BuildOutcome>>,
        max_parallel: usize,
        fail_fast: bool,
        run_id: String,
        locks: Arc<HostLockRegistry>,
    ) -> Vec<DeployHostResult> {
        use futures::stream::{self, StreamExt};
        use std::sync::atomic::{AtomicBool, Ordering};
        use tracing::Instrument;

        let artifacts = Arc::new(artifacts);
        let stop = Arc::new(AtomicBool::new(false));

        stream::iter(jobs)
            .map(move |job| {
                let stop = stop.clone();
                let artifacts = artifacts.clone();
                let run_id = run_id.clone();
                let locks = locks.clone();
                async move {
                    if stop.load(Ordering::SeqCst) {
                        return aborted_result(&job.host);
                    }
                    let build = Arc::clone(
                        artifacts
                            .get(&job.artifact_role)
                            .expect("artifact for role was built before run_jobs"),
                    );
                    let span = tracing::info_span!(
                        "deploy.host",
                        host = %job.host,
                        run_id = %run_id,
                    );
                    let res = run_single_job(job, build, locks).instrument(span).await;
                    if fail_fast && !res.succeeded {
                        stop.store(true, Ordering::SeqCst);
                    }
                    res
                }
            })
            .buffer_unordered(max_parallel.max(1))
            .collect()
            .await
    }
}

/// Generic orchestrator used by both `DefaultRunner` and orchestration tests.
///
/// Takes an `io_factory` that produces a `HostIo` for each host, plus the
/// stage knobs from the job. Tests use this to inject `RecordingIo`; the
/// production path uses `SshHostIo` via `run_single_job`.
///
/// Gated to test and `test-utils` builds only — not part of the public
/// production API.
#[cfg(any(test, feature = "test-utils", feature = "deploy"))]
#[allow(dead_code)]
pub async fn orchestrate_with_io<I, F>(
    hosts: Vec<(String, Option<String>, Option<ServiceScope>, String)>,
    build: Arc<BuildOutcome>,
    max_parallel: usize,
    fail_fast: bool,
    run_id: String,
    io_factory: F,
) -> Vec<DeployHostResult>
where
    I: HostIo + Send + Sync + 'static,
    F: Fn(&str) -> I + Send + Sync + Clone + 'static,
{
    use futures::stream::{self, StreamExt};
    use std::sync::atomic::{AtomicBool, Ordering};
    use tracing::Instrument;

    let stop = Arc::new(AtomicBool::new(false));

    stream::iter(hosts)
        .map(move |(host, unit, scope, remote_path)| {
            let stop = stop.clone();
            let build = build.clone();
            let run_id = run_id.clone();
            let io_factory = io_factory.clone();
            async move {
                if stop.load(Ordering::SeqCst) {
                    return aborted_result(&host);
                }
                let span = tracing::info_span!(
                    "deploy.host",
                    host = %host,
                    run_id = %run_id,
                );
                let io = Arc::new(io_factory(&host));
                let res =
                    run_host_pipeline(io, host.clone(), remote_path, unit, scope, build, None)
                        .instrument(span)
                        .await;
                if fail_fast && !res.succeeded {
                    stop.store(true, Ordering::SeqCst);
                }
                res
            }
        })
        .buffer_unordered(max_parallel.max(1))
        .collect()
        .await
}

/// Drive a fully-resolved single-host job: acquire lock, walk stages.
///
/// Takes all arguments by value so the resulting future does not hold any
/// borrowed references across await points, which would trigger HRTB errors
/// in `Box::pin(…+Send+'static)` contexts (Rust issue #100013).
async fn run_single_job(
    job: HostJob,
    build: Arc<BuildOutcome>,
    locks: Arc<HostLockRegistry>,
) -> DeployHostResult {
    let io = Arc::new(SshHostIo::new(job.host.clone(), job.target.clone()));
    let lock_timeout = std::time::Duration::from_secs(DEFAULT_LOCK_TIMEOUT_SECS);
    let _guard = match locks.acquire(&job.host, lock_timeout).await {
        Ok(g) => g,
        Err(e) => {
            return DeployHostResult {
                host: job.host.clone(),
                reached_stage: DeployStage::Resolve,
                succeeded: false,
                skipped_transfer: false,
                transferred_bytes: None,
                error_kind: Some(e.kind().to_string()),
                stage_timings_ms: Default::default(),
            };
        }
    };
    run_host_pipeline(
        io,
        job.host,
        job.remote_path,
        job.unit,
        job.scope,
        build,
        job.master_url,
    )
    .await
}

pub async fn run_host_pipeline<I: HostIo + 'static>(
    io: Arc<I>,
    host: String,
    remote_path: String,
    unit: Option<String>,
    scope: Option<ServiceScope>,
    build: Arc<BuildOutcome>,
    _master_url: Option<String>,
) -> DeployHostResult {
    let mut timings: std::collections::BTreeMap<String, u128> = std::collections::BTreeMap::new();
    let mut transferred_bytes: Option<u64> = None;

    // Preflight
    let t = stage_enter(&host, "preflight");
    let pre = match preflight(
        io.clone(),
        remote_path.clone(),
        build.target_triple.clone(),
        build.sha256.clone(),
    )
    .await
    {
        Ok(p) => {
            let elapsed_ms = t.elapsed().as_millis();
            timings.insert("preflight".into(), elapsed_ms);
            stage_exit(&host, "preflight", elapsed_ms, true, None);
            p
        }
        Err(e) => {
            let elapsed_ms = t.elapsed().as_millis();
            timings.insert("preflight".into(), elapsed_ms);
            stage_exit(&host, "preflight", elapsed_ms, false, Some(e.kind()));
            return host_err(&host, DeployStage::Preflight, e, timings, false);
        }
    };
    let skipped_transfer = pre.skip_transfer;
    if skipped_transfer {
        tracing::info!(
            surface = "dispatch",
            service = "deploy",
            action = "stage.skip",
            host = %host,
            stage = "transfer",
            reason = "remote_sha256_match",
            "deploy stage skipped",
        );
    }

    // Transfer + install (conditional).
    if !pre.skip_transfer {
        let t = stage_enter(&host, "transfer");
        let reader = match tokio::fs::File::open(&build.path).await {
            Ok(f) => f,
            Err(e) => {
                let elapsed_ms = t.elapsed().as_millis();
                timings.insert("transfer".into(), elapsed_ms);
                stage_exit(&host, "transfer", elapsed_ms, false, Some("build_failed"));
                return host_err(
                    &host,
                    DeployStage::Transfer,
                    DeployError::BuildFailed {
                        reason: format!("open artifact: {e}"),
                    },
                    timings,
                    false,
                );
            }
        };
        let outcome = match transfer_and_install(
            io.clone(),
            remote_path.clone(),
            build.sha256.clone(),
            reader,
        )
        .await
        {
            Ok(o) => {
                let elapsed_ms = t.elapsed().as_millis();
                timings.insert("transfer".into(), elapsed_ms);
                stage_exit(&host, "transfer", elapsed_ms, true, None);
                o
            }
            Err(e) => {
                let elapsed_ms = t.elapsed().as_millis();
                timings.insert("transfer".into(), elapsed_ms);
                stage_exit(&host, "transfer", elapsed_ms, false, Some(e.kind()));
                return host_err(&host, DeployStage::Install, e, timings, false);
            }
        };
        transferred_bytes = Some(outcome.bytes);
    }

    // Restart
    let t = stage_enter(&host, "restart");
    if let Err(e) = restart(io.clone(), unit, scope).await {
        let elapsed_ms = t.elapsed().as_millis();
        timings.insert("restart".into(), elapsed_ms);
        stage_exit(&host, "restart", elapsed_ms, false, Some(e.kind()));
        // skipped_transfer carries the actual preflight outcome: if transfer
        // was already skipped before restart failed, report that faithfully.
        return host_err(&host, DeployStage::Restart, e, timings, skipped_transfer);
    }
    let elapsed_ms = t.elapsed().as_millis();
    timings.insert("restart".into(), elapsed_ms);
    stage_exit(&host, "restart", elapsed_ms, true, None);

    // Verify
    let t = stage_enter(&host, "verify");
    if let Err(e) = verify(io.clone(), remote_path.clone()).await {
        let elapsed_ms = t.elapsed().as_millis();
        timings.insert("verify".into(), elapsed_ms);
        stage_exit(&host, "verify", elapsed_ms, false, Some(e.kind()));
        return host_err(&host, DeployStage::Verify, e, timings, skipped_transfer);
    }
    let elapsed_ms = t.elapsed().as_millis();
    timings.insert("verify".into(), elapsed_ms);
    stage_exit(&host, "verify", elapsed_ms, true, None);

    DeployHostResult {
        host,
        reached_stage: DeployStage::PhoneHome,
        succeeded: true,
        skipped_transfer,
        transferred_bytes,
        error_kind: None,
        stage_timings_ms: timings,
    }
}

async fn rollback_one_host<I: HostIo + 'static>(
    io: Arc<I>,
    host: String,
    remote_path: String,
    unit: Option<String>,
    scope: Option<ServiceScope>,
) -> DeployHostResult {
    let mut timings: std::collections::BTreeMap<String, u128> = std::collections::BTreeMap::new();

    // Validate remote_path against the allowlist before any shell use.
    if let Err(e) = params::validate_remote_path(&remote_path) {
        return host_err(&host, DeployStage::Resolve, e, timings, false);
    }

    // Find the most recent .bak.<ts> file in the parent directory.
    let parent = match Path::new(&remote_path).parent() {
        Some(p) => p.to_string_lossy().into_owned(),
        None => {
            return host_err(
                &host,
                DeployStage::Resolve,
                DeployError::ValidationFailed {
                    field: "remote_path".into(),
                    reason: "no parent".into(),
                },
                timings,
                false,
            );
        }
    };
    let binary = Path::new(&remote_path)
        .file_name()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| "labby".to_string());

    let t = stage_enter(&host, "rollback.find");
    // shell_quote parent and binary so any (theoretically allowlisted)
    // special characters in the path do not break the shell command.
    let sq_parent = shell_quote(&parent);
    let sq_binary = shell_quote(&binary);
    let pattern = format!("{sq_parent}/{sq_binary}.bak.*");
    // ls -1 <pattern> | sort | tail -n1
    let cmd = format!("ls -1 {pattern} 2>/dev/null | sort | tail -n1");
    let (code, stdout, stderr) = match io.run_argv(&["sh", "-c", &cmd]).await {
        Ok(v) => v,
        Err(e) => {
            let elapsed_ms = t.elapsed().as_millis();
            timings.insert("rollback.find".into(), elapsed_ms);
            stage_exit(&host, "rollback.find", elapsed_ms, false, Some(e.kind()));
            return host_err(&host, DeployStage::Resolve, e, timings, false);
        }
    };
    if code != 0 || stdout.trim().is_empty() {
        let elapsed_ms = t.elapsed().as_millis();
        timings.insert("rollback.find".into(), elapsed_ms);
        stage_exit(
            &host,
            "rollback.find",
            elapsed_ms,
            false,
            Some("validation_failed"),
        );
        return host_err(
            &host,
            DeployStage::Resolve,
            DeployError::ValidationFailed {
                field: "backup".into(),
                reason: format!("no backup found under {parent}: {}", stderr.trim()),
            },
            timings,
            false,
        );
    }
    let backup = stdout.trim().to_string();
    let elapsed_ms = t.elapsed().as_millis();
    timings.insert("rollback.find".into(), elapsed_ms);
    stage_exit(&host, "rollback.find", elapsed_ms, true, None);

    let t = stage_enter(&host, "rollback.restore");
    let (code, _stdout, stderr) = match io.run_argv(&["mv", "--", &backup, &remote_path]).await {
        Ok(v) => v,
        Err(e) => {
            let elapsed_ms = t.elapsed().as_millis();
            timings.insert("rollback.restore".into(), elapsed_ms);
            stage_exit(&host, "rollback.restore", elapsed_ms, false, Some(e.kind()));
            return host_err(&host, DeployStage::Install, e, timings, false);
        }
    };
    let elapsed_ms = t.elapsed().as_millis();
    timings.insert("rollback.restore".into(), elapsed_ms);
    if code != 0 {
        stage_exit(
            &host,
            "rollback.restore",
            elapsed_ms,
            false,
            Some("install_failed"),
        );
        return host_err(
            &host,
            DeployStage::Install,
            DeployError::InstallFailed {
                host: host.clone(),
                reason: format!("rollback rename: {}", stderr.trim()),
            },
            timings,
            false,
        );
    }
    stage_exit(&host, "rollback.restore", elapsed_ms, true, None);

    let t = stage_enter(&host, "rollback.restart");
    if let Err(e) = restart(io.clone(), unit, scope).await {
        let elapsed_ms = t.elapsed().as_millis();
        timings.insert("rollback.restart".into(), elapsed_ms);
        stage_exit(&host, "rollback.restart", elapsed_ms, false, Some(e.kind()));
        return host_err(&host, DeployStage::Restart, e, timings, false);
    }
    let elapsed_ms = t.elapsed().as_millis();
    timings.insert("rollback.restart".into(), elapsed_ms);
    stage_exit(&host, "rollback.restart", elapsed_ms, true, None);

    let t = stage_enter(&host, "rollback.verify");
    if let Err(e) = verify(io, remote_path).await {
        let elapsed_ms = t.elapsed().as_millis();
        timings.insert("rollback.verify".into(), elapsed_ms);
        stage_exit(&host, "rollback.verify", elapsed_ms, false, Some(e.kind()));
        return host_err(&host, DeployStage::Verify, e, timings, false);
    }
    let elapsed_ms = t.elapsed().as_millis();
    timings.insert("rollback.verify".into(), elapsed_ms);
    stage_exit(&host, "rollback.verify", elapsed_ms, true, None);

    DeployHostResult {
        host,
        reached_stage: DeployStage::Verify,
        succeeded: true,
        skipped_transfer: false,
        transferred_bytes: None,
        error_kind: None,
        stage_timings_ms: timings,
    }
}

fn stage_enter(host: &str, stage: &'static str) -> Instant {
    tracing::info!(
        surface = "dispatch",
        service = "deploy",
        action = "stage.enter",
        host = %host,
        stage,
        "deploy stage enter",
    );
    Instant::now()
}

fn stage_exit(
    host: &str,
    stage: &'static str,
    elapsed_ms: u128,
    succeeded: bool,
    kind: Option<&str>,
) {
    match kind {
        Some(kind) => tracing::warn!(
            surface = "dispatch",
            service = "deploy",
            action = "stage.exit",
            host = %host,
            stage,
            elapsed_ms,
            succeeded,
            kind,
            "deploy stage exit",
        ),
        None => tracing::info!(
            surface = "dispatch",
            service = "deploy",
            action = "stage.exit",
            host = %host,
            stage,
            elapsed_ms,
            succeeded,
            "deploy stage exit",
        ),
    }
}

/// Returns `true` when `err` still carries the unresolved `'?'` sentinel host
/// value that stage functions emit. Any variant without a `host` field is also
/// considered "unresolved" for assertion purposes.
///
/// Used exclusively by the `debug_assert!` in [`host_err`].
fn err_has_sentinel_host(err: &DeployError) -> bool {
    match err {
        DeployError::SshUnreachable { host }
        | DeployError::PreflightFailed { host, .. }
        | DeployError::TransferFailed { host, .. }
        | DeployError::InstallFailed { host, .. }
        | DeployError::RestartFailed { host, .. }
        | DeployError::VerifyFailed { host, .. }
        | DeployError::Conflict { host }
        | DeployError::ArchMismatch { host, .. }
        | DeployError::IntegrityMismatch { host } => host == "?",
        // Variants without a `host` field are not subject to the sentinel rule.
        DeployError::ValidationFailed { .. }
        | DeployError::BuildFailed { .. }
        | DeployError::PartialFailure { .. }
        | DeployError::AuthFailed { .. }
        | DeployError::Api(_) => true,
    }
}

/// Emit a WARN log for a per-host stage failure and build the corresponding
/// [`DeployHostResult`].
///
/// ## Sentinel contract
///
/// Stage functions fill `host` fields on [`DeployError`] with the literal
/// string `"?"` as a placeholder (see module-level docs). This function
/// receives the resolved `host` alias and uses it for the result and log.
///
/// A [`debug_assert!`] fires in test/debug builds if the error already has a
/// *non-`?`* host, which would indicate a double-wrap or a direct construction
/// that bypassed the sentinel convention.
fn host_err(
    host: &str,
    reached: DeployStage,
    err: DeployError,
    timings: std::collections::BTreeMap<String, u128>,
    skipped_transfer: bool,
) -> DeployHostResult {
    debug_assert!(
        err_has_sentinel_host(&err),
        "host_err called with already-resolved host in error '{}' — double-wrap?",
        err,
    );
    tracing::warn!(
        host = %host,
        reached_stage = ?reached,
        error = %err,
        error_kind = err.kind(),
        "deploy.host.error"
    );
    DeployHostResult {
        host: host.to_string(),
        reached_stage: reached,
        succeeded: false,
        skipped_transfer,
        transferred_bytes: None,
        error_kind: Some(err.kind().to_string()),
        stage_timings_ms: timings,
    }
}

fn aborted_result(host: &str) -> DeployHostResult {
    DeployHostResult {
        host: host.to_string(),
        reached_stage: DeployStage::Resolve,
        succeeded: false,
        skipped_transfer: false,
        transferred_bytes: None,
        error_kind: Some("aborted".into()),
        stage_timings_ms: Default::default(),
    }
}

fn summarize(
    run_id: String,
    artifact_sha256: String,
    hosts: Vec<DeployHostResult>,
) -> DeployRunSummary {
    let succeeded = hosts.iter().filter(|r| r.succeeded).count();
    let failed = hosts.len() - succeeded;
    DeployRunSummary {
        run_id,
        artifact_sha256,
        artifacts: vec![],
        succeeded,
        failed,
        ok: failed == 0,
        hosts,
    }
}

// ── NoopRunner (kept for surface bring-up / fallback) ──────────────────────

/// Placeholder runner. Kept so callers without a wired `DefaultRunner` can
/// still register dispatch without a panic.
#[allow(dead_code)]
#[derive(Default)]
pub struct NoopRunner;

impl DeployRunner for NoopRunner {
    async fn plan(&self, _req: DeployRequest) -> Result<DeployPlan, ToolError> {
        Err(ToolError::internal_message(
            "deploy runner is not wired on this build",
        ))
    }

    async fn run(&self, _req: DeployRequest) -> Result<DeployRunSummary, ToolError> {
        Err(ToolError::internal_message(
            "deploy runner is not wired on this build",
        ))
    }

    async fn rollback(&self, _req: DeployRequest) -> Result<DeployRunSummary, ToolError> {
        Err(ToolError::internal_message(
            "deploy runner is not wired on this build",
        ))
    }

    async fn config_list(&self) -> Result<Value, ToolError> {
        Ok(json!({ "defaults": null, "hosts": [], "overrides": [] }))
    }
}

// ── test_support ─────────────────────────────────────────────────────────────

#[cfg(any(test, feature = "test-utils", feature = "deploy"))]
#[doc(hidden)]
#[allow(dead_code)]
pub mod test_support {
    //! Recording `HostIo` used by both inline stage tests and the
    //! `tests/deploy_runner.rs` orchestrator tests.

    use super::*;
    use std::sync::Mutex;
    use tokio::io::{AsyncRead, AsyncReadExt};

    /// Pre-programmed response for a single `run_argv` call, matched in order.
    #[derive(Debug, Clone)]
    pub struct RunResp {
        pub code: i32,
        pub stdout: String,
        pub stderr: String,
    }

    impl RunResp {
        pub fn ok(stdout: impl Into<String>) -> Self {
            Self {
                code: 0,
                stdout: stdout.into(),
                stderr: String::new(),
            }
        }
        pub fn fail(code: i32, stderr: impl Into<String>) -> Self {
            Self {
                code,
                stdout: String::new(),
                stderr: stderr.into(),
            }
        }
    }

    /// Recording `HostIo` that appends every op to `log` and returns
    /// scripted responses from `run_queue` / `sha_queue`.
    #[derive(Default)]
    pub struct RecordingIo {
        pub log: Arc<Mutex<Vec<String>>>,
        pub run_queue: Arc<Mutex<Vec<RunResp>>>,
        pub sha_queue: Arc<Mutex<Vec<Option<String>>>>,
        pub upload_bytes: Arc<Mutex<Vec<u64>>>,
    }

    impl RecordingIo {
        pub fn new() -> Self {
            Self::default()
        }

        pub fn push_run(&self, resp: RunResp) {
            self.run_queue.lock().unwrap().push(resp);
        }

        pub fn push_sha(&self, value: Option<String>) {
            self.sha_queue.lock().unwrap().push(value);
        }

        pub fn ops(&self) -> Vec<String> {
            self.log.lock().unwrap().clone()
        }
    }

    impl HostIo for RecordingIo {
        fn run_argv(
            &self,
            argv: &[&str],
        ) -> Pin<
            Box<dyn Future<Output = Result<(i32, String, String), DeployError>> + Send + 'static>,
        > {
            let joined = argv.join(",");
            let log = self.log.clone();
            let run_queue = self.run_queue.clone();
            Box::pin(async move {
                log.lock().unwrap().push(format!("run:{joined}"));
                let resp = run_queue
                    .lock()
                    .unwrap()
                    .drain(..1)
                    .next()
                    .unwrap_or_else(|| RunResp::ok(""));
                Ok((resp.code, resp.stdout, resp.stderr))
            })
        }

        fn upload_stream<R>(
            &self,
            remote_path: &str,
            mut reader: R,
        ) -> Pin<Box<dyn Future<Output = Result<u64, DeployError>> + Send + 'static>>
        where
            R: AsyncRead + Unpin + Send + 'static,
        {
            let remote_path = remote_path.to_string();
            let log = self.log.clone();
            let upload_bytes = self.upload_bytes.clone();
            Box::pin(async move {
                log.lock().unwrap().push(format!("upload:{remote_path}"));
                let mut buf = Vec::new();
                let bytes =
                    reader
                        .read_to_end(&mut buf)
                        .await
                        .map_err(|e| DeployError::TransferFailed {
                            host: "?".into(),
                            reason: e.to_string(),
                        })? as u64;
                upload_bytes.lock().unwrap().push(bytes);
                Ok(bytes)
            })
        }

        fn sha256_remote(
            &self,
            remote_path: &str,
        ) -> Pin<Box<dyn Future<Output = Result<Option<String>, DeployError>> + Send + 'static>>
        {
            let remote_path = remote_path.to_string();
            let log = self.log.clone();
            let sha_queue = self.sha_queue.clone();
            Box::pin(async move {
                log.lock().unwrap().push(format!("sha256:{remote_path}"));
                let val = sha_queue.lock().unwrap().drain(..1).next().flatten();
                Ok(val)
            })
        }
    }
}
