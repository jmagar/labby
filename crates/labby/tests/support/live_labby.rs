use std::collections::{BTreeMap, BTreeSet};
use std::ffi::OsString;
use std::future::Future;
use std::io::{Read as _, Seek as _, SeekFrom};
use std::net::{SocketAddr, TcpListener};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};
use tokio::process::{Child, Command as TokioCommand};

use super::evidence::{EvidenceKind, RunEvidence, sanitize};

#[cfg(not(windows))]
const DEFAULT_DEADLINE: Duration = Duration::from_secs(20);
#[cfg(windows)]
const DEFAULT_DEADLINE: Duration = Duration::from_secs(45);
const LOG_TAIL_BYTES: usize = 32 * 1024;
const DROP_DEADLINE: Duration = Duration::from_secs(3);
const CLEANUP_MAX_FILES: usize = 4_096;
const CLEANUP_MAX_BYTES: u64 = 64 * 1024 * 1024;
const CLEANUP_MAX_DEPTH: usize = 32;

#[cfg(unix)]
#[path = "live_labby/guardian.rs"]
mod guardian;

#[cfg(unix)]
#[path = "live_labby/process_inventory.rs"]
mod process_inventory;

#[cfg(unix)]
#[path = "live_labby/process_identity.rs"]
mod process_identity;

/// OS runtime inputs only; never inherit credentials, proxies, or user configuration.
fn isolated_runtime_env() -> Vec<(&'static str, OsString)> {
    let mut values = vec![("PATH", std::env::var_os("PATH").unwrap_or_default())];
    if cfg!(windows) {
        // Winsock provider initialization needs the Windows installation directory.
        for key in ["SystemRoot", "WINDIR"] {
            if let Some(value) = std::env::var_os(key) {
                values.push((key, value));
            }
        }
    }
    values
}

fn labby_binary() -> PathBuf {
    std::env::var_os("LABBY_E2E_BINARY")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(env!("CARGO_BIN_EXE_labby")))
}

struct RevocationGuard {
    session: String,
    command: Command,
    absent_paths: NonEmptyAbsencePaths,
}

#[derive(Debug)]
struct NonEmptyAbsencePaths {
    first: PathBuf,
    rest: Vec<PathBuf>,
}

impl NonEmptyAbsencePaths {
    fn try_from_paths(mut paths: Vec<PathBuf>) -> Result<Self, String> {
        if paths.is_empty() {
            return Err("credential/session revocation requires absence evidence".into());
        }
        let first = paths.remove(0);
        Ok(Self { first, rest: paths })
    }

    fn any_exists(&self) -> Result<bool, String> {
        for path in std::iter::once(&self.first).chain(&self.rest) {
            match std::fs::symlink_metadata(path) {
                Ok(_) => return Ok(true),
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => {
                    return Err(format!(
                        "credential/session absence could not be verified: {error}"
                    ));
                }
            }
        }
        Ok(false)
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub(crate) struct RunIdentity {
    pub(crate) run_id: String,
    pub(crate) seed: u64,
    #[serde(default, skip_serializing)]
    pub(crate) nonce: String,
    pub(crate) git_sha: String,
    pub(crate) git_dirty: bool,
    pub(crate) binary_sha256: String,
    pub(crate) binary_version: String,
    pub(crate) platform: String,
    pub(crate) features: Vec<String>,
    pub(crate) ui_asset_sha256: String,
    pub(crate) fixture_versions: Vec<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub(crate) struct SanitizedConnectionDescriptor {
    pub(crate) run_id: String,
    pub(crate) base_url: String,
    pub(crate) health_url: String,
    pub(crate) ready_url: String,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub(crate) struct OwnershipLedger {
    pub(crate) generation: u64,
    pub(crate) created_at_ms: u128,
    pub(crate) nonce: String,
    pub(crate) root: PathBuf,
    /// Owned Rust Child / process-group leader; a guardian when supervised.
    pub(crate) pid: Option<u32>,
    pub(crate) process_start_identity: Option<String>,
    /// Distinct roles: never present a guardian PID as the actual Labby daemon.
    pub(crate) guardian_pid: Option<u32>,
    pub(crate) daemon_pid: Option<u32>,
    pub(crate) daemon_process_start_identity: Option<String>,
    pub(crate) process_group: Option<i32>,
    pub(crate) listener: Option<SocketAddr>,
    pub(crate) listener_identity: Option<String>,
    pub(crate) locks: Vec<PathBuf>,
    pub(crate) credential_sessions: Vec<String>,
    pub(crate) owned_roots: Vec<PathBuf>,
}

#[derive(Clone, Debug, Default)]
pub(crate) struct CleanupResult {
    pub(crate) primary_failure: Option<String>,
    pub(crate) graceful: bool,
    pub(crate) forced: bool,
    pub(crate) failures: Vec<String>,
    pub(crate) retention_failure: Option<String>,
}

impl CleanupResult {
    pub(crate) fn is_clean(&self) -> bool {
        self.failures.is_empty()
    }
}

#[derive(Clone)]
pub(crate) struct LiveLabbyBuilder {
    readiness_deadline: Duration,
    extra_env: BTreeMap<OsString, OsString>,
    args: Vec<OsString>,
    port: Option<u16>,
    bind_ip: std::net::IpAddr,
    ready_path: String,
    config: Option<String>,
    fail_evidence_writes: bool,
    existing_root: Option<PathBuf>,
    identity_probe: Option<IdentityProbe>,
}

type IdentityProbe = std::sync::Arc<
    dyn Fn(u32, SocketAddr, Option<&Path>, Instant) -> Result<String, String> + Send + Sync,
>;

impl Default for LiveLabbyBuilder {
    fn default() -> Self {
        Self {
            readiness_deadline: DEFAULT_DEADLINE,
            extra_env: BTreeMap::new(),
            args: Vec::new(),
            port: None,
            bind_ip: std::net::Ipv4Addr::LOCALHOST.into(),
            ready_path: "/ready".to_string(),
            config: None,
            fail_evidence_writes: false,
            existing_root: None,
            identity_probe: None,
        }
    }
}

impl LiveLabbyBuilder {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    pub(crate) fn readiness_deadline(mut self, deadline: Duration) -> Self {
        self.readiness_deadline = deadline;
        self
    }

    pub(crate) fn arg(mut self, arg: impl Into<OsString>) -> Self {
        self.args.push(arg.into());
        self
    }

    pub(crate) fn env(mut self, key: impl Into<OsString>, value: impl Into<OsString>) -> Self {
        self.extra_env.insert(key.into(), value.into());
        self
    }

    pub(crate) fn port(mut self, port: u16) -> Self {
        self.port = Some(port);
        self
    }

    pub(crate) fn bind_ip(mut self, bind_ip: std::net::IpAddr) -> Self {
        self.bind_ip = bind_ip;
        self
    }

    pub(crate) fn ready_path(mut self, path: impl Into<String>) -> Self {
        self.ready_path = path.into();
        self
    }

    pub(crate) fn config(mut self, config: impl Into<String>) -> Self {
        self.config = Some(config.into());
        self
    }

    pub(crate) fn fail_evidence_writes(mut self) -> Self {
        self.fail_evidence_writes = true;
        self
    }

    /// Start in a caller-owned canonical test root. This supports workflows
    /// whose offline setup phase must precede daemon startup in the same
    /// installation. The caller retains ownership and cleanup responsibility.
    pub(crate) fn existing_root(mut self, root: impl Into<PathBuf>) -> Self {
        self.existing_root = Some(root.into());
        self
    }

    pub(crate) async fn start(self) -> Result<LiveLabbyGuard, String> {
        self.start_with_retries(4).await
    }

    fn start_with_retries(
        self,
        attempts: u8,
    ) -> std::pin::Pin<Box<dyn Future<Output = Result<LiveLabbyGuard, String>>>> {
        Box::pin(async move {
            let retry = self.clone();
            let owned_parent = std::env::temp_dir().join("labby-live-e2e");
            std::fs::create_dir_all(&owned_parent).map_err(|error| error.to_string())?;
            let (root_guard, root) = if let Some(root) = &self.existing_root {
                (None, canonical_owned_root(root, &owned_parent)?)
            } else {
                let guard = tempfile::Builder::new()
                    .prefix("run-")
                    .tempdir_in(&owned_parent)
                    .map_err(|error| error.to_string())?;
                let root = canonical_owned_root(guard.path(), &owned_parent)?;
                (Some(guard), root)
            };
            let identity = build_identity()?;
            let credential_canary = random_secret_canary()?;
            let nonce_path = root.join("ownership.nonce");
            write_nonce(&nonce_path, &identity.nonce)?;
            let manifest_path = root.join("ownership.json");
            let stdout_path = root.join("stdout.log");
            let stderr_path = root.join("stderr.log");
            let home = root.join("home");
            let labby_home = root.join("labby-home");
            let xdg_config = root.join("xdg/config");
            let xdg_cache = root.join("xdg/cache");
            let xdg_runtime = root.join("xdg/runtime");
            let temp = root.join("tmp");
            for path in [
                &home,
                &labby_home,
                &xdg_config,
                &xdg_cache,
                &xdg_runtime,
                &temp,
            ] {
                std::fs::create_dir_all(path).map_err(|error| error.to_string())?;
            }
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt as _;
                std::fs::set_permissions(&labby_home, std::fs::Permissions::from_mode(0o700))
                    .map_err(|error| error.to_string())?;
            }
            if let Some(config) = &self.config {
                std::fs::write(labby_home.join("config.toml"), config)
                    .map_err(|error| error.to_string())?;
            }

            let address = if let Some(port) = self.port {
                SocketAddr::new(self.bind_ip, port)
            } else {
                let listener = TcpListener::bind(SocketAddr::new(self.bind_ip, 0))
                    .map_err(|error| error.to_string())?;
                let address = listener.local_addr().map_err(|error| error.to_string())?;
                drop(listener);
                address
            };

            let mut evidence = RunEvidence::new(identity.clone());
            evidence.push(EvidenceKind::Setup, format!("allocated {}", root.display()));
            let mut ledger = OwnershipLedger {
                generation: 1,
                created_at_ms: unix_timestamp_ms(),
                nonce: identity.nonce.clone(),
                root: root.clone(),
                listener: Some(address),
                listener_identity: Some(format!("tcp:{address}")),
                owned_roots: vec![root.clone()],
                ..OwnershipLedger::default()
            };
            write_ledger(&manifest_path, &ledger)?;

            let stdout = std::fs::File::create(&stdout_path).map_err(|error| error.to_string())?;
            let stderr = std::fs::File::create(&stderr_path).map_err(|error| error.to_string())?;
            let restart = RestartRecipe {
                address,
                home: home.clone(),
                labby_home: labby_home.clone(),
                xdg_config: xdg_config.clone(),
                xdg_cache: xdg_cache.clone(),
                xdg_runtime: xdg_runtime.clone(),
                temp: temp.clone(),
                args: self.args.clone(),
                extra_env: self.extra_env.clone(),
                identity_probe: self.identity_probe.clone(),
            };
            // The ownership nonce is persisted by design, so it must never double as
            // a credential canary. The credential is independent and never serialized.
            let mut secret_canaries = vec![credential_canary.clone()];
            secret_canaries.extend(self.extra_env.iter().filter_map(|(key, value)| {
                let key = key.to_string_lossy().to_ascii_uppercase();
                (key.contains("CANARY") || key.contains("SECRET") || key.contains("TOKEN"))
                    .then(|| value.to_string_lossy().into_owned())
            }));
            let mut command = TokioCommand::new(labby_binary());
            command
                .env_clear()
                .args([
                    "serve",
                    "--host",
                    &address.ip().to_string(),
                    "--port",
                    &address.port().to_string(),
                ])
                .args(self.args)
                .env("HOME", &home)
                .env("LABBY_HOME", &labby_home)
                .env("LABBY_LOG_DIR", root.join("logs"))
                .env("XDG_CONFIG_HOME", &xdg_config)
                .env("XDG_CACHE_HOME", &xdg_cache)
                .env("XDG_RUNTIME_DIR", &xdg_runtime)
                .env("TMPDIR", &temp)
                .env("LABBY_AUTH_MODE", "bearer")
                .env("LABBY_MCP_HTTP_TOKEN", &credential_canary)
                .envs(isolated_runtime_env())
                .envs(self.extra_env);
            #[cfg(unix)]
            let (mut command, guardian_admission) = guardian::supervise(command)?;
            command
                .stdin(Stdio::null())
                .stdout(stdout)
                .stderr(stderr)
                .kill_on_drop(true);
            configure_process_group(&mut command);
            let readiness_expires = Instant::now() + self.readiness_deadline;
            let mut child = command.spawn().map_err(|error| error.to_string())?;
            let (start_identity, owned_job) = capture_spawned_child_identity(
                &mut child,
                address,
                self.identity_probe.as_ref(),
                #[cfg(unix)]
                guardian_admission.as_deref(),
                readiness_expires,
            )?;
            #[cfg(windows)]
            let windows_job = owned_job;
            #[cfg(not(windows))]
            let _ = owned_job;
            ledger.pid = child.id();
            ledger.process_start_identity = Some(start_identity);
            ledger.daemon_pid = ledger.pid;
            ledger.daemon_process_start_identity = ledger.process_start_identity.clone();
            #[cfg(unix)]
            if guardian_admission.is_some() {
                ledger.guardian_pid = ledger.pid;
                ledger.daemon_pid = None;
                ledger.daemon_process_start_identity = None;
            }
            ledger.process_group = ledger.pid.and_then(|pid| i32::try_from(pid).ok());
            if let Err(error) = write_ledger(&manifest_path, &ledger) {
                #[cfg(windows)]
                let cleanup_job = windows_job;
                #[cfg(not(windows))]
                let cleanup_job = unassigned_cleanup_job();
                let mut failures = vec![format!("daemon ownership publication failed: {error}")];
                terminate_and_reap_owned_child(&mut child, cleanup_job, &mut failures);
                return Err(failures.join("; "));
            }
            evidence.push(
                EvidenceKind::Process,
                format!("spawned pid {:?}", child.id()),
            );

            let descriptor = SanitizedConnectionDescriptor {
                run_id: identity.run_id.clone(),
                base_url: format!("http://{address}"),
                health_url: format!("http://{address}/health"),
                ready_url: format!("http://{address}{}", self.ready_path),
            };
            let mut guard = LiveLabbyGuard {
                root_guard,
                root,
                manifest_path,
                nonce_path,
                stdout_path,
                stderr_path,
                child: Some(child),
                ledger,
                identity,
                descriptor,
                evidence,
                restart,
                secret_canaries,
                credential_canary,
                revocations: Vec::new(),
                primary_failure: None,
                fail_evidence_writes: self.fail_evidence_writes,
                #[cfg(windows)]
                windows_job,
                #[cfg(unix)]
                guardian_admission,
                finalized: false,
            };
            if let Err(error) = guard.wait_ready(readiness_expires).await {
                guard.primary_failure = Some(error.clone());
                let diagnostics = guard.diagnostics(Some(&error));
                drop(guard.finish_with_deadline(Duration::from_secs(5)).await);
                if attempts > 1
                    && retry.port.is_none()
                    && (diagnostics.contains("Address already in use")
                        || diagnostics.contains("address already in use")
                        || diagnostics.contains("os error 48"))
                {
                    return retry.start_with_retries(attempts - 1).await;
                }
                return Err(diagnostics);
            }
            Ok(guard)
        })
    }
}

pub(crate) struct LiveLabbyGuard {
    root_guard: Option<tempfile::TempDir>,
    root: PathBuf,
    manifest_path: PathBuf,
    nonce_path: PathBuf,
    stdout_path: PathBuf,
    stderr_path: PathBuf,
    child: Option<Child>,
    ledger: OwnershipLedger,
    identity: RunIdentity,
    descriptor: SanitizedConnectionDescriptor,
    evidence: RunEvidence,
    restart: RestartRecipe,
    secret_canaries: Vec<String>,
    credential_canary: String,
    revocations: Vec<RevocationGuard>,
    primary_failure: Option<String>,
    fail_evidence_writes: bool,
    #[cfg(windows)]
    windows_job: Option<labby_winjob::JobObject>,
    #[cfg(unix)]
    guardian_admission: Option<PathBuf>,
    finalized: bool,
}

#[derive(Clone)]
struct RestartRecipe {
    identity_probe: Option<IdentityProbe>,
    address: SocketAddr,
    home: PathBuf,
    labby_home: PathBuf,
    xdg_config: PathBuf,
    xdg_cache: PathBuf,
    xdg_runtime: PathBuf,
    temp: PathBuf,
    args: Vec<OsString>,
    extra_env: BTreeMap<OsString, OsString>,
}

impl LiveLabbyGuard {
    pub(crate) fn identity(&self) -> &RunIdentity {
        &self.identity
    }
    pub(crate) fn connection(&self) -> &SanitizedConnectionDescriptor {
        &self.descriptor
    }
    pub(crate) fn root(&self) -> &Path {
        &self.root
    }

    pub(crate) fn authorize_cli(&self, command: &mut TokioCommand) {
        command
            .env("LABBY_SERVER_URL", &self.descriptor.base_url)
            .env("LABBY_MCP_HTTP_TOKEN", &self.credential_canary)
            .env("LABBY_E2E_TEAM_ID", "bootstrap-initial-team");
    }

    pub(crate) async fn restart(&mut self) -> Result<(), String> {
        self.stop_process(Instant::now() + Duration::from_secs(5))
            .await?;
        let stdout = std::fs::OpenOptions::new()
            .append(true)
            .open(&self.stdout_path)
            .map_err(|error| error.to_string())?;
        let stderr = std::fs::OpenOptions::new()
            .append(true)
            .open(&self.stderr_path)
            .map_err(|error| error.to_string())?;
        let recipe = &self.restart;
        let mut command = TokioCommand::new(labby_binary());
        command
            .env_clear()
            .args([
                "serve",
                "--host",
                &recipe.address.ip().to_string(),
                "--port",
                &recipe.address.port().to_string(),
            ])
            .args(&recipe.args)
            .env("HOME", &recipe.home)
            .env("LABBY_HOME", &recipe.labby_home)
            .env("LABBY_LOG_DIR", self.root.join("logs"))
            .env("XDG_CONFIG_HOME", &recipe.xdg_config)
            .env("XDG_CACHE_HOME", &recipe.xdg_cache)
            .env("XDG_RUNTIME_DIR", &recipe.xdg_runtime)
            .env("TMPDIR", &recipe.temp)
            .env("LABBY_AUTH_MODE", "bearer")
            .env("LABBY_MCP_HTTP_TOKEN", &self.credential_canary)
            .envs(isolated_runtime_env())
            .envs(recipe.extra_env.clone());
        #[cfg(unix)]
        let (mut command, guardian_admission) = guardian::supervise(command)?;
        command
            .stdin(Stdio::null())
            .stdout(stdout)
            .stderr(stderr)
            .kill_on_drop(true);
        configure_process_group(&mut command);
        let readiness_expires = Instant::now() + DEFAULT_DEADLINE;
        let mut child = command.spawn().map_err(|error| error.to_string())?;
        let (start_identity, owned_job) = capture_spawned_child_identity(
            &mut child,
            recipe.address,
            recipe.identity_probe.as_ref(),
            #[cfg(unix)]
            guardian_admission.as_deref(),
            readiness_expires,
        )?;
        #[cfg(windows)]
        {
            self.windows_job = owned_job;
        }
        #[cfg(not(windows))]
        let _ = owned_job;
        self.ledger.generation += 1;
        self.ledger.pid = child.id();
        self.ledger.process_start_identity = Some(start_identity);
        self.ledger.guardian_pid = None;
        self.ledger.daemon_pid = self.ledger.pid;
        self.ledger.daemon_process_start_identity = self.ledger.process_start_identity.clone();
        #[cfg(unix)]
        {
            self.guardian_admission = guardian_admission;
            if self.guardian_admission.is_some() {
                self.ledger.guardian_pid = self.ledger.pid;
                self.ledger.daemon_pid = None;
                self.ledger.daemon_process_start_identity = None;
            }
        }
        self.ledger.process_group = self.ledger.pid.and_then(|pid| i32::try_from(pid).ok());
        if let Err(error) = write_ledger(&self.manifest_path, &self.ledger) {
            #[cfg(windows)]
            let cleanup_job = self.windows_job.take();
            #[cfg(not(windows))]
            let cleanup_job = unassigned_cleanup_job();
            let mut failures = vec![format!("daemon ownership publication failed: {error}")];
            terminate_and_reap_owned_child(&mut child, cleanup_job, &mut failures);
            return Err(failures.join("; "));
        }
        self.child = Some(child);
        self.evidence.push(EvidenceKind::Process, "restarted labby");
        self.wait_ready(readiness_expires).await
    }

    pub(crate) async fn finish(mut self) -> CleanupResult {
        self.finish_inner(Duration::from_secs(10)).await
    }

    pub(crate) async fn finish_with_deadline(&mut self, deadline: Duration) -> CleanupResult {
        self.finish_inner(deadline).await
    }

    pub(crate) async fn run_with_timeout<F, T>(
        &mut self,
        timeout: Duration,
        future: F,
    ) -> Result<T, String>
    where
        F: Future<Output = T>,
    {
        match tokio::time::timeout(timeout, future).await {
            Ok(value) => Ok(value),
            Err(_) => {
                let cleanup = self.finish_inner(Duration::from_secs(5)).await;
                Err(format!(
                    "supervised case timed out; cleanup={:?}",
                    cleanup.failures
                ))
            }
        }
    }

    pub(crate) async fn finish_on_supported_signal(mut self) -> CleanupResult {
        #[cfg(unix)]
        {
            let mut terminate =
                tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
                    .expect("install SIGTERM handler");
            tokio::select! {
                result = tokio::signal::ctrl_c() => drop(result),
                _ = terminate.recv() => {}
            }
        }
        #[cfg(not(unix))]
        drop(tokio::signal::ctrl_c().await);
        self.finish_inner(Duration::from_secs(10)).await
    }

    pub(crate) fn register_credential_session(
        &mut self,
        session: impl Into<String>,
        command: Command,
        absent_paths: Vec<PathBuf>,
    ) -> Result<(), String> {
        let absent_paths = NonEmptyAbsencePaths::try_from_paths(absent_paths)?;
        let session = session.into();
        self.ledger.credential_sessions.push(session.clone());
        self.revocations.push(RevocationGuard {
            session,
            command,
            absent_paths,
        });
        drop(write_ledger(&self.manifest_path, &self.ledger));
        Ok(())
    }

    /// Settle one guard only after the caller has verified authoritative
    /// credential/session denial. File absence alone is not revocation proof.
    pub(crate) fn confirm_credential_session_revoked(
        &mut self,
        session: &str,
    ) -> Result<(), String> {
        let index = self
            .revocations
            .iter()
            .position(|guard| guard.session == session)
            .ok_or_else(|| "confirmed revocation has no matching guard".to_string())?;
        if self.revocations[index].absent_paths.any_exists()? {
            return Err("confirmed revocation still has secret outputs".into());
        }
        self.revocations.remove(index);
        self.ledger.credential_sessions.retain(|id| id != session);
        Ok(())
    }

    async fn finish_inner(&mut self, timeout: Duration) -> CleanupResult {
        if self.finalized {
            return CleanupResult::default();
        }
        let deadline_exhausted = timeout.is_zero();
        let absolute = Instant::now() + timeout.max(Duration::from_secs(2));
        let mut result = CleanupResult {
            primary_failure: self.primary_failure.clone(),
            ..CleanupResult::default()
        };
        if deadline_exhausted {
            result.failures.push("cleanup deadline exhausted".into());
        }
        match self.stop_process(absolute).await {
            Ok(forced) => {
                result.forced = forced;
                result.graceful = !forced;
            }
            Err(error) => result.failures.push(error),
        }
        for lock in &self.ledger.locks {
            if lock.starts_with(&self.root) {
                drop(std::fs::remove_file(lock));
            } else {
                result
                    .failures
                    .push(format!("unsafe owned lock path: {}", lock.display()));
            }
        }
        let revocation_count = self.revocations.len();
        let mut revocations = std::mem::take(&mut self.revocations);
        match run_cleanup_blocking(absolute, "credential/session cleanup", move |deadline| {
            let mut failures = Vec::new();
            for revoke in &mut revocations {
                if Instant::now() >= absolute {
                    failures.push("credential/session cleanup deadline exhausted".into());
                    break;
                }
                if let Err(error) = run_owned_command(&mut revoke.command, deadline) {
                    failures.push(format!("credential/session revocation failed: {error}"));
                } else {
                    match revoke.absent_paths.any_exists() {
                        Ok(false) => {}
                        Ok(true) => {
                            failures.push("credential/session remained after revocation".into())
                        }
                        Err(error) => failures.push(error),
                    }
                }
            }
            failures
        })
        .await
        {
            Ok(failures) => result.failures.extend(failures),
            Err(error) => result.failures.push(error),
        }
        if revocation_count != self.ledger.credential_sessions.len() {
            result
                .failures
                .push("credential/session ledger has no matching revocation guard".into());
        }
        self.ledger.credential_sessions.clear();
        let root = self.root.clone();
        let stdout_path = self.stdout_path.clone();
        let stderr_path = self.stderr_path.clone();
        let canaries = self.secret_canaries.clone();
        let artifact_cleanup = run_cleanup_blocking(
            absolute,
            "artifact retention and secret scan",
            move |deadline| {
                run_artifact_cleanup_helper(&root, &stdout_path, &stderr_path, &canaries, deadline)
            },
        );
        match artifact_cleanup.await {
            Ok(Ok(failures)) => result.failures.extend(failures),
            Ok(Err(error)) => result.failures.push(error),
            Err(error) => result.failures.push(error),
        }
        let listener = self.ledger.listener.expect("listener recorded");
        loop {
            match TcpListener::bind(listener) {
                Ok(probe) => {
                    drop(probe);
                    break;
                }
                Err(_) if Instant::now() < absolute => {
                    tokio::time::sleep(Duration::from_millis(10)).await;
                }
                Err(_) => {
                    result
                        .failures
                        .push("owned listener remains bound".to_string());
                    break;
                }
            }
        }
        self.evidence.push(
            EvidenceKind::Cleanup,
            format!("cleanup failures={}", result.failures.len()),
        );
        let retained = std::env::temp_dir()
            .join("labby-live-e2e-evidence")
            .join(format!("{}.json", self.identity.run_id));
        let evidence_result = if self.fail_evidence_writes {
            Err(std::io::Error::other("injected evidence disk failure"))
        } else {
            self.evidence.write_atomic(&retained)
        };
        if let Err(error) = evidence_result {
            eprintln!(
                "labby-e2e evidence fallback run={} error={error}",
                self.identity.run_id
            );
            result
                .failures
                .push(format!("evidence write failed: {error}"));
            result.retention_failure = Some(error.to_string());
        } else {
            scan_file_for_canaries(&retained, &self.secret_canaries, &mut result.failures);
        }
        self.finalized = true;
        let owns_root = self.root_guard.is_some();
        if let Some(root_guard) = self.root_guard.take() {
            if let Err(error) = root_guard.close() {
                result
                    .failures
                    .push(format!("owned root deletion failed: {error}"));
            }
        }
        if owns_root && self.root.exists() {
            result.failures.push(format!(
                "owned root retained after cleanup: {}",
                self.root.display()
            ));
        }
        result
    }

    async fn wait_ready(&mut self, expires: Instant) -> Result<(), String> {
        // The workspace deliberately builds reqwest with rustls-no-provider so
        // each executable/test binary chooses its provider explicitly.
        drop(rustls::crypto::ring::default_provider().install_default());
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(1))
            .build()
            .map_err(|e| e.to_string())?;
        loop {
            if Instant::now() >= expires {
                return Err("readiness deadline exceeded".into());
            }
            if let Some(child) = self.child.as_mut() {
                let pid = child.id().ok_or("daemon was reaped before readiness")?;
                if let Some(status) = observe_owned_child(child, pid).map_err(|error| {
                    format!("daemon readiness process observation failed: {error}")
                })? {
                    return Err(format!("labby exited before readiness: {status}"));
                }
            }
            let request_deadline = tokio::time::Instant::from_std(expires);
            let health = tokio::time::timeout_at(
                request_deadline,
                client.get(&self.descriptor.health_url).send(),
            )
            .await
            .map_err(|_| "readiness deadline exceeded".to_string())?;
            let ready = tokio::time::timeout_at(
                request_deadline,
                client.get(&self.descriptor.ready_url).send(),
            )
            .await
            .map_err(|_| "readiness deadline exceeded".to_string())?;
            self.evidence.push(
                EvidenceKind::Readiness,
                format!(
                    "health={} ready={}",
                    health.as_ref().map(|r| r.status().as_u16()).unwrap_or(0),
                    ready.as_ref().map(|r| r.status().as_u16()).unwrap_or(0)
                ),
            );
            if health
                .as_ref()
                .is_ok_and(|response| response.status().is_success())
                && ready
                    .as_ref()
                    .is_ok_and(|response| response.status().is_success())
            {
                if Instant::now() >= expires {
                    return Err("readiness identity deadline exceeded".into());
                }
                #[cfg(unix)]
                guardian::record_daemon_identity(self, expires)?;
                if Instant::now() >= expires {
                    return Err("readiness identity deadline exceeded".into());
                }
                self.evidence
                    .push(EvidenceKind::Readiness, "health and ready succeeded");
                return Ok(());
            }
            if Instant::now() >= expires {
                return Err("readiness deadline exceeded".into());
            }
            tokio::time::sleep_until(tokio::time::Instant::from_std(
                expires.min(Instant::now() + Duration::from_millis(50)),
            ))
            .await;
        }
    }

    async fn stop_process(&mut self, deadline: Instant) -> Result<bool, String> {
        if let Err(error) = self.validate_ownership_before(deadline) {
            let mut failures = vec![error];
            if let Some(child) = self.child.as_mut() {
                #[cfg(windows)]
                let job = self.windows_job.take();
                #[cfg(not(windows))]
                let job = unassigned_cleanup_job();
                let kill_deadline = deadline.min(Instant::now() + Duration::from_secs(1));
                terminate_and_reap_owned_child_with_signal(
                    child,
                    job,
                    &mut failures,
                    kill_deadline,
                    |pid| signal_cleanup_group_with_deadline(pid, kill_deadline),
                );
            }
            return Err(failures.join("; "));
        }
        let Some(mut child) = self.child.take() else {
            return Ok(false);
        };
        let pid = child
            .id()
            .ok_or("owned daemon has no PID before settlement")?;
        let mut failures = Vec::new();
        #[cfg(unix)]
        {
            require_waitable_cleanup_owner(&child, pid, &mut failures);
            let _ = nix::sys::signal::killpg(
                nix::unistd::Pid::from_raw(pid as i32),
                nix::sys::signal::Signal::SIGTERM,
            );
        }
        #[cfg(not(unix))]
        {
            #[cfg(windows)]
            if let Some(job) = self.windows_job.take() {
                job.close().map_err(|error| error.to_string())?;
            }
            #[cfg(not(windows))]
            drop(child.start_kill());
        }
        let remaining = deadline.saturating_duration_since(Instant::now());
        let graceful_window = (remaining / 2).min(Duration::from_secs(2));
        let graceful_deadline = Instant::now() + graceful_window;
        let status = loop {
            match observe_owned_child(&mut child, pid) {
                Ok(Some(status)) => break Some(status),
                Ok(None) if Instant::now() < graceful_deadline => {
                    tokio::time::sleep(Duration::from_millis(10)).await
                }
                Ok(None) => break None,
                Err(error) => {
                    failures.push(format!("owned daemon exit observation failed: {error}"));
                    break None;
                }
            }
        };
        let forced = status.is_none();
        if let Some(status) = status {
            self.evidence
                .push(EvidenceKind::Process, format!("exit status={status}"));
        }
        #[cfg(unix)]
        let forced = if !forced {
            match process_group_members_typed(pid as i32, deadline) {
                Ok(members) => !members.is_empty(),
                Err(failure) => {
                    if failure.unsettled.is_some() {
                        terminate_and_reap_owned_child(
                            &mut child,
                            unassigned_cleanup_job(),
                            &mut failures,
                        );
                    }
                    failures.push(settle_inventory_failure(failure));
                    true
                }
            }
        } else {
            forced
        };
        let kill_deadline = deadline.min(Instant::now() + Duration::from_secs(1));
        terminate_and_reap_owned_child_with_signal(
            &mut child,
            unassigned_cleanup_job(),
            &mut failures,
            kill_deadline,
            |pid| signal_cleanup_group_with_deadline(pid, kill_deadline),
        );
        if failures.is_empty() {
            Ok(forced)
        } else {
            Err(failures.join("; "))
        }
    }

    fn validate_ownership(&mut self) -> Result<(), String> {
        self.validate_ownership_before(Instant::now() + Duration::from_secs(1))
    }

    fn validate_ownership_before(&mut self, deadline: Instant) -> Result<(), String> {
        let root_metadata =
            std::fs::symlink_metadata(&self.root).map_err(|error| error.to_string())?;
        if root_metadata.file_type().is_symlink() {
            return Err("owned root was replaced by a symlink".into());
        }
        let nonce_metadata =
            std::fs::symlink_metadata(&self.nonce_path).map_err(|error| error.to_string())?;
        if nonce_metadata.file_type().is_symlink() {
            return Err("ownership nonce was replaced by a symlink".into());
        }
        let nonce = std::fs::read_to_string(&self.nonce_path).map_err(|error| error.to_string())?;
        if nonce != self.identity.nonce {
            return Err("ownership nonce mismatch".into());
        }
        let bytes = std::fs::read(&self.manifest_path).map_err(|error| error.to_string())?;
        let persisted: OwnershipLedger = serde_json::from_slice(&bytes)
            .map_err(|error| format!("invalid ownership manifest: {error}"))?;
        if persisted.nonce != self.ledger.nonce
            || persisted.generation != self.ledger.generation
            || persisted.created_at_ms != self.ledger.created_at_ms
            || persisted.root != self.ledger.root
            || persisted.pid != self.ledger.pid
            || persisted.process_start_identity != self.ledger.process_start_identity
            || persisted.process_group != self.ledger.process_group
            || persisted.guardian_pid != self.ledger.guardian_pid
            || persisted.daemon_pid != self.ledger.daemon_pid
            || persisted.daemon_process_start_identity != self.ledger.daemon_process_start_identity
            || persisted.listener_identity != self.ledger.listener_identity
            || persisted.owned_roots != self.ledger.owned_roots
        {
            return Err("foreign or stale ownership manifest".into());
        }
        if persisted
            .nonce
            .chars()
            .any(|character| character.is_control())
            || persisted.owned_roots.iter().any(|path| {
                path.as_os_str()
                    .to_string_lossy()
                    .chars()
                    .any(|c| c.is_control())
            })
        {
            return Err("ownership manifest contains control characters".into());
        }
        if persisted.owned_roots != [self.root.clone()]
            || persisted.listener_identity.as_deref()
                != persisted
                    .listener
                    .map(|address| format!("tcp:{address}"))
                    .as_deref()
        {
            return Err("unsafe ownership manifest identity".into());
        }
        #[cfg(unix)]
        if self.child.is_some() && !self.observed_identity_matches_before(deadline) {
            return Err("owned PID start identity changed or could not be verified".into());
        }
        #[cfg(not(unix))]
        let _ = deadline;
        Ok(())
    }

    #[cfg(unix)]
    fn settle_observation_failure(&mut self, mut failure: process_inventory::Failure) -> String {
        if failure.unsettled.is_some() {
            let mut errors = vec![failure.message.clone()];
            if let Some(child) = self.child.as_mut() {
                terminate_and_reap_owned_child(child, unassigned_cleanup_job(), &mut errors);
            }
            failure.message = errors.join("; ");
        }
        settle_inventory_failure(failure)
    }

    #[cfg(unix)]
    fn observed_identity_matches_before(&mut self, deadline: Instant) -> bool {
        let (Some(pid), Some(expected)) =
            (self.ledger.pid, self.ledger.process_start_identity.clone())
        else {
            return false;
        };
        if process_identity::validate(pid, &expected).is_err() {
            return false;
        }
        let observed = process_identity::capture_typed(pid, deadline)
            .map_err(|failure| self.settle_observation_failure(failure));
        process_identity::matches(Some(pid), Some(&expected), observed)
    }

    pub(crate) fn diagnostics(&mut self, primary: Option<&str>) -> String {
        #[cfg(unix)]
        let process_inventory = match self.ledger.process_group.map(process_group_inventory) {
            Some(Ok(inventory)) => inventory,
            Some(Err(failure)) => vec![self.settle_observation_failure(failure)],
            None => Vec::new(),
        };
        #[cfg(not(unix))]
        let process_inventory: Vec<String> = Vec::new();
        let readiness_history = self
            .evidence
            .events
            .iter()
            .filter(|event| event.kind == EvidenceKind::Readiness)
            .map(|event| event.message.as_str())
            .collect::<Vec<_>>();
        format!(
            "run={} command={} version={} binary_sha256={} address={} primary={} stdout_tail={} stderr_tail={} health_ready_history={:?} process_inventory={:?} process_pid={:?} process_group={:?} generation={}",
            self.identity.run_id,
            labby_binary().display(),
            self.identity.binary_version,
            self.identity.binary_sha256,
            self.descriptor.base_url,
            primary.unwrap_or("none"),
            tail(&self.stdout_path),
            tail(&self.stderr_path),
            readiness_history,
            process_inventory,
            self.ledger.pid,
            self.ledger.process_group,
            self.ledger.generation,
        )
    }
}

impl Drop for LiveLabbyGuard {
    fn drop(&mut self) {
        if self.finalized {
            return;
        }
        self.evidence
            .push(EvidenceKind::Failure, "guard dropped without finish");
        let deadline = Instant::now() + DROP_DEADLINE;
        #[cfg(unix)]
        let safe_to_signal = self.observed_identity_matches_before(
            deadline.min(Instant::now() + Duration::from_secs(1)),
        );
        #[cfg(not(unix))]
        let safe_to_signal = self.child.as_ref().and_then(Child::id) == self.ledger.pid;
        if !safe_to_signal && self.ledger.pid.is_some() {
            self.evidence.push(
                EvidenceKind::Failure,
                "drop identity verification failed; only retained child/job authority may settle",
            );
        }
        if let Some(child) = self.child.as_mut() {
            #[cfg(windows)]
            let job = self.windows_job.take();
            #[cfg(not(windows))]
            let job = unassigned_cleanup_job();
            let mut failures = Vec::new();
            let kill_deadline = deadline.min(Instant::now() + Duration::from_secs(1));
            terminate_and_reap_owned_child_with_signal(
                child,
                job,
                &mut failures,
                kill_deadline,
                |pid| signal_cleanup_group_with_deadline(pid, kill_deadline),
            );
            for failure in failures {
                self.evidence.push(EvidenceKind::Failure, failure);
            }
        }
        for revoke in &mut self.revocations {
            if Instant::now() >= deadline {
                self.evidence
                    .push(EvidenceKind::Failure, "drop revocation deadline exhausted");
                break;
            }
            if let Err(error) = run_owned_command(&mut revoke.command, deadline) {
                self.evidence.push(
                    EvidenceKind::Failure,
                    format!("drop revocation failed: {error}"),
                );
            } else {
                match revoke.absent_paths.any_exists() {
                    Ok(false) => {}
                    Ok(true) => self.evidence.push(
                        EvidenceKind::Failure,
                        "drop revocation absence verification failed",
                    ),
                    Err(error) => self.evidence.push(EvidenceKind::Failure, error),
                }
            }
        }
        let mut artifact_scan_failures = Vec::new();
        let mut scan_budget = ScanBudget {
            deadline,
            bytes_remaining: CLEANUP_MAX_BYTES,
        };
        scan_artifact_tree_with_budget(
            &self.root,
            &self.secret_canaries,
            &mut artifact_scan_failures,
            &mut scan_budget,
        );
        for failure in artifact_scan_failures {
            self.evidence.push(EvidenceKind::Failure, failure);
        }
        let retained = std::env::temp_dir()
            .join("labby-live-e2e-evidence")
            .join(format!("{}.json", self.identity.run_id));
        if let Err(error) = self.evidence.write_atomic(&retained) {
            eprintln!(
                "labby-e2e drop evidence fallback run={} error={error}",
                self.identity.run_id
            );
        } else {
            let mut scan_failures = Vec::new();
            scan_file_for_canaries_bounded(
                &retained,
                &self.secret_canaries,
                &mut scan_failures,
                &mut scan_budget,
            );
            for failure in scan_failures {
                eprintln!(
                    "labby-e2e drop evidence secret scan run={} failure={failure}",
                    self.identity.run_id
                );
            }
        }
        if let Some(root_guard) = self.root_guard.take() {
            drop(root_guard.close());
        }
    }
}

pub(crate) fn isolated_command(home: &Path) -> Command {
    let mut command = Command::new(labby_binary());
    command
        .env_clear()
        .env("HOME", home)
        .env("LABBY_HOME", home.join(".labby"))
        .env("LABBY_LOG_DIR", home.join("logs"))
        .env("TMPDIR", home.join("tmp"))
        // Keep disposable CLI probes from attaching to an operator's daemon
        // on the default port. Tests that intentionally exercise remote
        // discovery override this value after constructing the command.
        .env("LABBY_MCP_HTTP_PORT", "0")
        .envs(isolated_runtime_env());
    command
}

pub(crate) fn sweep_stale_runs() -> Vec<String> {
    let parent = std::env::temp_dir().join("labby-live-e2e");
    let Ok(parent) = parent.canonicalize() else {
        return Vec::new();
    };
    let mut failures = Vec::new();
    let Ok(entries) = std::fs::read_dir(&parent) else {
        return failures;
    };
    for entry in entries.flatten() {
        let root = entry.path();
        let result = (|| -> Result<(), String> {
            if std::fs::symlink_metadata(&root)
                .map_err(|e| e.to_string())?
                .file_type()
                .is_symlink()
            {
                return Err("stale candidate root is a symlink".into());
            }
            let root = root.canonicalize().map_err(|e| e.to_string())?;
            if !root.starts_with(&parent) {
                return Err("stale candidate escaped parent".into());
            }
            let ledger: OwnershipLedger = serde_json::from_slice(
                &std::fs::read(root.join("ownership.json")).map_err(|e| e.to_string())?,
            )
            .map_err(|e| e.to_string())?;
            let nonce =
                std::fs::read_to_string(root.join("ownership.nonce")).map_err(|e| e.to_string())?;
            if ledger.root != root || ledger.nonce != nonce || nonce.chars().any(|c| c.is_control())
            {
                return Err("stale candidate ownership mismatch".into());
            }
            if unix_timestamp_ms().saturating_sub(ledger.created_at_ms) < 300_000 {
                return Ok(());
            }
            let Some(pid) = ledger.pid else {
                // A manifest without a spawned PID may be another test currently
                // between allocation and spawn, so it is never sweepable.
                return Ok(());
            };
            if pid_is_alive(pid) {
                return Ok(());
            }
            std::fs::remove_dir_all(&root).map_err(|e| e.to_string())
        })();
        if let Err(error) = result {
            failures.push(format!("{}: {error}", root.display()));
        }
    }
    failures
}

#[cfg(unix)]
fn pid_is_alive(pid: u32) -> bool {
    i32::try_from(pid)
        .ok()
        .is_some_and(|pid| nix::sys::signal::kill(nix::unistd::Pid::from_raw(pid), None).is_ok())
}

#[cfg(windows)]
fn pid_is_alive(pid: u32) -> bool {
    labby_winjob::pid_is_alive(pid)
}

#[cfg(not(any(unix, windows)))]
fn pid_is_alive(_pid: u32) -> bool {
    false
}

fn build_identity() -> Result<RunIdentity, String> {
    let mut nonce = [0_u8; 24];
    getrandom::fill(&mut nonce).map_err(|error| error.to_string())?;
    let nonce = hex::encode(nonce);
    let run_id = ulid::Ulid::new().to_string();
    let seed = std::env::var("LABBY_E2E_SEED")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or_else(|| u64::from_le_bytes(run_id.as_bytes()[..8].try_into().unwrap()));
    let binary_path = labby_binary();
    if !binary_path.is_absolute() {
        return Err("LABBY_E2E_BINARY must be absolute".into());
    }
    let binary = std::fs::read(&binary_path).map_err(|error| error.to_string())?;
    let binary_sha256 = hex::encode(Sha256::digest(binary));
    let binary_version = Command::new(&binary_path)
        .arg("--version")
        .output()
        .ok()
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .unwrap_or_else(|| "unknown".into());
    let git_sha = Command::new("git")
        .args(["rev-parse", "HEAD"])
        .output()
        .ok()
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .unwrap_or_else(|| "unknown".into());
    let git_dirty = Command::new("git")
        .args(["status", "--porcelain"])
        .output()
        .is_ok_and(|o| !o.stdout.is_empty());
    Ok(RunIdentity {
        run_id,
        seed,
        nonce,
        git_sha,
        git_dirty,
        binary_sha256,
        binary_version,
        platform: format!("{}-{}", std::env::consts::OS, std::env::consts::ARCH),
        features: enabled_features(),
        ui_asset_sha256: "not-built".to_string(),
        fixture_versions: vec!["live-harness-fixture:v1".to_string()],
    })
}

fn unix_timestamp_ms() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
}

fn random_secret_canary() -> Result<String, String> {
    let mut bytes = [0_u8; 24];
    getrandom::fill(&mut bytes).map_err(|error| error.to_string())?;
    Ok(format!("labby-e2e-secret-{}", hex::encode(bytes)))
}

fn enabled_features() -> Vec<String> {
    [
        ("gateway", cfg!(feature = "gateway")),
        ("fs", cfg!(feature = "fs")),
        ("skills", cfg!(feature = "skills")),
        ("lab-admin", cfg!(feature = "lab-admin")),
        ("api-docs", cfg!(feature = "api-docs")),
        ("systemd", cfg!(feature = "systemd")),
    ]
    .into_iter()
    .filter(|(_, enabled)| *enabled)
    .map(|(name, _)| name.to_string())
    .collect()
}

fn canonical_owned_root(root: &Path, parent: &Path) -> Result<PathBuf, String> {
    let metadata = std::fs::symlink_metadata(root).map_err(|error| error.to_string())?;
    if metadata.file_type().is_symlink() {
        return Err("owned root must not be a symlink".into());
    }
    let root = root.canonicalize().map_err(|error| error.to_string())?;
    let parent = parent.canonicalize().map_err(|error| error.to_string())?;
    if !root.starts_with(&parent) {
        return Err("owned root escaped allocated parent".into());
    }
    Ok(root)
}

fn write_nonce(path: &Path, nonce: &str) -> Result<(), String> {
    std::fs::write(path, nonce).map_err(|error| error.to_string())?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))
            .map_err(|e| e.to_string())?;
    }
    Ok(())
}

fn write_ledger(path: &Path, ledger: &OwnershipLedger) -> Result<(), String> {
    let temporary = path.with_extension("json.tmp");
    std::fs::write(
        &temporary,
        serde_json::to_vec_pretty(ledger).map_err(|e| e.to_string())?,
    )
    .map_err(|e| e.to_string())?;
    std::fs::rename(temporary, path).map_err(|e| e.to_string())
}

fn tail(path: &Path) -> String {
    let Ok(mut file) = std::fs::File::open(path) else {
        return String::new();
    };
    let length = file.metadata().map(|metadata| metadata.len()).unwrap_or(0);
    let start = length.saturating_sub(LOG_TAIL_BYTES as u64);
    if file.seek(SeekFrom::Start(start)).is_err() {
        return String::new();
    }
    let mut bytes = Vec::with_capacity(LOG_TAIL_BYTES.min((length - start) as usize));
    if file
        .take(LOG_TAIL_BYTES as u64)
        .read_to_end(&mut bytes)
        .is_err()
    {
        return String::new();
    }
    sanitize(&String::from_utf8_lossy(&bytes))
}

fn cap_log_file(path: &Path) -> std::io::Result<()> {
    let mut file = std::fs::File::open(path)?;
    let length = file.metadata()?.len();
    if length > LOG_TAIL_BYTES as u64 {
        file.seek(SeekFrom::End(-(LOG_TAIL_BYTES as i64)))?;
        let mut bytes = Vec::with_capacity(LOG_TAIL_BYTES);
        file.take(LOG_TAIL_BYTES as u64).read_to_end(&mut bytes)?;
        let temporary = path.with_extension("rotating.tmp");
        std::fs::write(&temporary, &bytes)?;
        std::fs::rename(temporary, path)?;
    }
    Ok(())
}

fn scan_file_for_canaries(path: &Path, canaries: &[String], failures: &mut Vec<String>) {
    let mut budget = ScanBudget {
        deadline: Instant::now() + DEFAULT_DEADLINE,
        bytes_remaining: u64::MAX,
    };
    scan_file_for_canaries_bounded(path, canaries, failures, &mut budget);
}

struct ScanBudget {
    deadline: Instant,
    bytes_remaining: u64,
}

fn scan_file_for_canaries_bounded(
    path: &Path,
    canaries: &[String],
    failures: &mut Vec<String>,
    budget: &mut ScanBudget,
) {
    let Ok(mut file) = std::fs::File::open(path) else {
        return;
    };
    let secrets = canaries
        .iter()
        .filter(|canary| !canary.is_empty())
        .map(String::as_bytes)
        .collect::<Vec<_>>();
    let overlap = secrets
        .iter()
        .map(|secret| secret.len())
        .max()
        .unwrap_or(1)
        .saturating_sub(1);
    let mut retained = Vec::new();
    let mut chunk = [0_u8; 64 * 1024];
    loop {
        if Instant::now() >= budget.deadline {
            failures.push(format!(
                "secret scan deadline exhausted at {}",
                path.display()
            ));
            return;
        }
        // Read at most one byte beyond the remaining allowance. That detects an
        // over-budget file from bytes actually returned by the filesystem without
        // trusting sparse-file metadata or buffering the file as a whole.
        let read_limit = budget
            .bytes_remaining
            .saturating_add(1)
            .min(chunk.len() as u64) as usize;
        let Ok(read) = file.read(&mut chunk[..read_limit]) else {
            failures.push(format!("secret scan failed for {}", path.display()));
            return;
        };
        if read == 0 {
            return;
        }
        if read as u64 > budget.bytes_remaining {
            budget.bytes_remaining = 0;
            failures.push(format!(
                "artifact scan byte cap exceeded while reading {}",
                path.display()
            ));
            return;
        }
        budget.bytes_remaining -= read as u64;
        retained.extend_from_slice(&chunk[..read]);
        if secrets.iter().any(|secret| {
            retained
                .windows(secret.len())
                .any(|window| window == *secret)
        }) {
            failures.push(format!("secret canary appeared in {}", path.display()));
            return;
        }
        let keep = overlap.min(retained.len());
        retained.drain(..retained.len() - keep);
    }
}

fn scan_artifact_tree(root: &Path, canaries: &[String], failures: &mut Vec<String>) {
    scan_artifact_tree_bounded(root, canaries, failures, Instant::now() + DEFAULT_DEADLINE);
}

fn scan_artifact_tree_bounded(
    root: &Path,
    canaries: &[String],
    failures: &mut Vec<String>,
    deadline: Instant,
) {
    let mut budget = ScanBudget {
        deadline,
        bytes_remaining: CLEANUP_MAX_BYTES,
    };
    scan_artifact_tree_with_budget(root, canaries, failures, &mut budget);
}

fn scan_artifact_tree_with_budget(
    root: &Path,
    canaries: &[String],
    failures: &mut Vec<String>,
    budget: &mut ScanBudget,
) {
    let mut pending = vec![(root.to_path_buf(), 0_usize)];
    let mut files = 0_usize;
    let mut discovered = 0_usize;
    while let Some((path, depth)) = pending.pop() {
        if Instant::now() >= budget.deadline {
            failures.push("artifact scan deadline exhausted".into());
            return;
        }
        if depth > CLEANUP_MAX_DEPTH {
            failures.push(format!(
                "artifact scan depth cap exceeded at {}",
                path.display()
            ));
            continue;
        }
        let Ok(metadata) = std::fs::symlink_metadata(&path) else {
            continue;
        };
        if metadata.file_type().is_symlink() {
            continue;
        }
        if metadata.is_dir() {
            if let Err(error) = enqueue_cleanup_directory(
                &path,
                depth,
                &mut pending,
                &mut discovered,
                budget.deadline,
            ) {
                failures.push(error);
                return;
            }
        } else if metadata.is_file() {
            files += 1;
            if files > CLEANUP_MAX_FILES {
                failures.push(format!(
                    "artifact scan resource cap exceeded (files={files})"
                ));
                return;
            }
            scan_file_for_canaries_bounded(&path, canaries, failures, budget);
            if failures.last().is_some_and(|failure| {
                failure.contains("byte cap exceeded") || failure.contains("scan deadline exhausted")
            }) {
                return;
            }
        }
    }
}

fn enqueue_cleanup_directory(
    path: &Path,
    depth: usize,
    pending: &mut Vec<(PathBuf, usize)>,
    discovered: &mut usize,
    deadline: Instant,
) -> Result<(), String> {
    let entries = std::fs::read_dir(path)
        .map_err(|error| format!("cleanup directory read failed: {error}"))?;
    for entry in entries {
        if Instant::now() >= deadline {
            return Err("cleanup directory scan deadline exhausted".into());
        }
        if *discovered >= CLEANUP_MAX_FILES {
            return Err("cleanup directory entry cap exceeded".into());
        }
        let entry = entry.map_err(|error| format!("cleanup directory entry failed: {error}"))?;
        *discovered += 1;
        pending.push((entry.path(), depth + 1));
    }
    Ok(())
}

fn cap_log_tree(root: &Path, failures: &mut Vec<String>) {
    cap_log_tree_bounded(root, failures, Instant::now() + DEFAULT_DEADLINE);
}

fn cap_log_tree_bounded(root: &Path, failures: &mut Vec<String>, deadline: Instant) {
    let mut pending = vec![(root.to_path_buf(), 0_usize)];
    let mut files = 0_usize;
    let mut discovered = 0_usize;
    let mut bytes = 0_u64;
    while let Some((path, depth)) = pending.pop() {
        if Instant::now() >= deadline {
            failures.push("log retention deadline exhausted".into());
            return;
        }
        if depth > CLEANUP_MAX_DEPTH {
            failures.push(format!(
                "log retention depth cap exceeded at {}",
                path.display()
            ));
            continue;
        }
        let Ok(metadata) = std::fs::symlink_metadata(&path) else {
            continue;
        };
        if metadata.file_type().is_symlink() {
            continue;
        }
        if metadata.is_dir() {
            if let Err(error) =
                enqueue_cleanup_directory(&path, depth, &mut pending, &mut discovered, deadline)
            {
                failures.push(error);
                return;
            }
        } else if metadata.is_file() {
            files += 1;
            bytes = bytes.saturating_add(metadata.len());
            if files > CLEANUP_MAX_FILES || bytes > CLEANUP_MAX_BYTES {
                failures.push(format!(
                    "log retention resource cap exceeded (files={files}, bytes={bytes})"
                ));
                return;
            }
            if let Err(error) = cap_log_file(&path) {
                failures.push(format!(
                    "log retention failed for {}: {error}",
                    path.display()
                ));
            }
        }
    }
}

async fn run_cleanup_blocking<T>(
    deadline: Instant,
    label: &'static str,
    operation: impl FnOnce(Instant) -> T + Send + 'static,
) -> Result<T, String>
where
    T: Send + 'static,
{
    if deadline <= Instant::now() {
        return Err(format!("{label} deadline exhausted"));
    }
    // Inner cleanup is cooperative and joined: it never detaches mutation work.
    // The documented `labby-live-e2e.sh` process-group supervisor is the hard
    // wall-clock boundary because an in-process future cannot interrupt a
    // blocked filesystem syscall. The real-shard watchdog regression proves a
    // stuck test process is killed before it can mutate after supervision ends.
    let received = tokio::task::spawn_blocking(move || operation(deadline))
        .await
        .map_err(|error| format!("{label} worker failed: {error}"))?;
    Ok(received)
}

fn run_owned_command(command: &mut Command, deadline: Instant) -> Result<(), String> {
    #[cfg(unix)]
    let helper_registry = std::env::var_os("LABBY_E2E_HELPER_REGISTRY").map(PathBuf::from);
    #[cfg(unix)]
    if let Some(registry) = &helper_registry {
        let token = std::env::var("LABBY_E2E_GROUP_TOKEN")
            .map_err(|_| "supervised cleanup helper has no owned shard token".to_string())?;
        *command = supervised_cleanup_command(command, registry, &token)?;
    }
    #[cfg(unix)]
    let admission = helper_registry
        .as_ref()
        .map(|registry| supervised_admission_path(command, registry))
        .transpose()?;
    command
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt as _;
        command.process_group(0).stderr(Stdio::piped());
    }
    let child = command.spawn().map_err(|error| error.to_string())?;
    #[cfg(unix)]
    let mut child = child;
    #[cfg(unix)]
    let mut diagnostics = CleanupStderr::new(child.stderr.take().expect("requested helper stderr"));
    #[cfg(unix)]
    let setup = diagnostics.configure();
    #[cfg(unix)]
    let configured = setup.is_ok();
    let result = run_spawned_owned_child(
        child,
        deadline,
        |pid| {
            #[cfg(unix)]
            setup?;
            assign_cleanup_job(pid)
        },
        |child| {
            #[cfg(unix)]
            diagnostics.drain()?;
            #[cfg(unix)]
            if let Some(admission) = &admission
                && let Some(status) = supervised_helper_status(admission, child.id())?
            {
                return Ok(Some(status));
            }
            child.try_wait()
        },
    );
    #[cfg(unix)]
    if let Err(error) = result {
        // Nonblocking and bounded after settlement as well as during polling.
        let drain_error = if configured {
            diagnostics.drain().err()
        } else {
            None
        }
        .map(|_| " stderr_read_failed")
        .unwrap_or("");
        return Err(format!("{error}; {}{drain_error}", diagnostics.summary()));
    }
    result
}

#[cfg(unix)]
struct CleanupStderr {
    pipe: std::process::ChildStderr,
    bytes: Vec<u8>,
    truncated: bool,
}

#[cfg(unix)]
impl CleanupStderr {
    fn new(pipe: std::process::ChildStderr) -> Self {
        Self {
            pipe,
            bytes: Vec::new(),
            truncated: false,
        }
    }

    fn configure(&self) -> Result<(), String> {
        use rustix::fs::{OFlags, fcntl_getfl, fcntl_setfl};
        let flags = fcntl_getfl(&self.pipe).map_err(|error| error.to_string())?;
        fcntl_setfl(&self.pipe, flags | OFlags::NONBLOCK).map_err(|error| error.to_string())
    }

    fn drain(&mut self) -> std::io::Result<()> {
        let mut buffer = [0_u8; 1024];
        // A chatty helper cannot monopolize the deadline/status polling loop.
        for _ in 0..8 {
            match self.pipe.read(&mut buffer) {
                Ok(0) => break,
                Ok(count) => {
                    let retained = count.min(2048_usize.saturating_sub(self.bytes.len()));
                    self.bytes.extend_from_slice(&buffer[..retained]);
                    self.truncated |= retained < count;
                }
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => break,
                Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
                Err(error) => return Err(error),
            }
        }
        Ok(())
    }

    fn summary(&self) -> String {
        cleanup_stderr_summary(&self.bytes, self.truncated)
    }
}

#[cfg(unix)]
fn cleanup_stderr_summary(bytes: &[u8], truncated: bool) -> String {
    // Raw subprocess stderr may contain credentials or private paths. Emit only
    // fixed diagnostic categories and a fingerprint of the bounded capture.
    let text = String::from_utf8_lossy(bytes).to_ascii_lowercase();
    let categories = [
        ("file exists", "already_exists"),
        ("permission denied", "permission_denied"),
        ("no such file", "missing_file"),
        ("not found", "not_found"),
        ("panicked", "test_panic"),
        ("invalid", "invalid_input"),
    ]
    .into_iter()
    .filter_map(|(needle, category)| text.contains(needle).then_some(category))
    .collect::<Vec<_>>();
    format!(
        "stderr categories={categories:?} bytes={} truncated={truncated} fingerprint={}",
        bytes.len(),
        hex::encode(Sha256::digest(bytes))
    )
}

#[cfg(unix)]
const CLEANUP_HELPER_ADMISSION_GATE: &str =
    include_str!("../../../../scripts/ci/labby-owned-process-gate.sh");

#[cfg(unix)]
fn supervised_helper_status(
    admission: &Path,
    pid: u32,
) -> std::io::Result<Option<std::process::ExitStatus>> {
    use std::io::Read as _;
    use std::os::unix::process::ExitStatusExt as _;
    let file = match std::fs::File::open(admission.join("status")) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error),
    };
    let mut status = String::new();
    file.take(257).read_to_string(&mut status)?;
    let mut fields = status.lines();
    let invalid = || {
        std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "invalid cleanup helper status",
        )
    };
    if status.len() > 256
        || fields.next().and_then(|value| value.parse::<u32>().ok()) != Some(pid)
        || fields.next() != admission.file_name().and_then(|name| name.to_str())
    {
        return Err(invalid());
    }
    let code = fields
        .next()
        .ok_or_else(invalid)?
        .parse::<u8>()
        .map_err(|_| invalid())?;
    if fields.next().is_some() {
        return Err(invalid());
    }
    Ok(Some(std::process::ExitStatus::from_raw(
        i32::from(code) << 8,
    )))
}

#[cfg(unix)]
fn supervised_admission_path(command: &Command, registry: &Path) -> Result<PathBuf, String> {
    let id = command
        .get_envs()
        .find_map(|(key, value)| (key == "LABBY_E2E_ADMISSION_ID").then_some(value).flatten())
        .and_then(|value| value.to_str())
        .ok_or("missing cleanup helper admission identity")?;
    if !id.starts_with("admission-")
        || id.len() != 58
        || !id[10..].bytes().all(|byte| byte.is_ascii_hexdigit())
    {
        return Err("invalid cleanup helper admission identity".into());
    }
    Ok(registry.join(id))
}

#[cfg(unix)]
fn supervised_cleanup_command(
    command: &Command,
    registry: &Path,
    token: &str,
) -> Result<Command, String> {
    use std::os::unix::fs::{MetadataExt as _, PermissionsExt as _};
    let metadata = std::fs::symlink_metadata(registry).map_err(|error| error.to_string())?;
    if !registry.is_absolute()
        || !metadata.is_dir()
        || metadata.file_type().is_symlink()
        || metadata.permissions().mode() & 0o077 != 0
        || metadata.uid() != nix::unistd::geteuid().as_raw()
        || token.is_empty()
    {
        return Err("cleanup helper supervisor registry is not a private owned directory".into());
    }
    let mut wrapper = Command::new("/bin/sh");
    let mut admission_nonce = [0_u8; 24];
    getrandom::fill(&mut admission_nonce).map_err(|error| error.to_string())?;
    // Both supported helper kinds use explicit configuration. Do not restore
    // ambient credentials that the offline-recovery command deliberately drops.
    wrapper.env_clear().env("PATH", "/usr/bin:/bin");
    for (key, value) in command.get_envs() {
        if let Some(value) = value {
            wrapper.env(key, value);
        } else {
            wrapper.env_remove(key);
        }
    }
    wrapper
        .env(
            "LABBY_E2E_ADMISSION_ID",
            format!("admission-{}", hex::encode(admission_nonce)),
        )
        .env("LABBY_E2E_GROUP_TOKEN", token)
        .args(["-c", CLEANUP_HELPER_ADMISSION_GATE, "labby-cleanup-helper"])
        .arg(registry)
        .arg(command.get_program())
        .args(command.get_args());
    if let Some(directory) = command.get_current_dir() {
        wrapper.current_dir(directory);
    }
    Ok(wrapper)
}

// Poll callbacks cannot reap a Unix leader: its waitable identity reserves the
// process-group number until every descendant has settled.
struct CleanupChildObserver<'a>(&'a mut std::process::Child);

impl CleanupChildObserver<'_> {
    #[cfg(unix)]
    fn id(&self) -> u32 {
        self.0.id()
    }

    fn try_wait(&mut self) -> std::io::Result<Option<std::process::ExitStatus>> {
        #[cfg(unix)]
        {
            observe_cleanup_child(self.id())
        }
        #[cfg(not(unix))]
        self.0.try_wait()
    }
}

#[cfg(unix)]
fn observe_cleanup_child(pid: u32) -> std::io::Result<Option<std::process::ExitStatus>> {
    use rustix::process::{Pid, WaitId, WaitIdOptions, waitid};
    use std::os::unix::process::ExitStatusExt as _;
    let pid = Pid::from_raw(pid as i32).ok_or_else(|| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "invalid cleanup child PID",
        )
    })?;
    let status = waitid(
        WaitId::Pid(pid),
        WaitIdOptions::EXITED | WaitIdOptions::NOHANG | WaitIdOptions::NOWAIT,
    )?;
    status
        .map(|status| {
            let raw = if let Some(code) = status.exit_status() {
                code << 8
            } else if let Some(signal) = status.terminating_signal() {
                signal | if status.dumped() { 0x80 } else { 0 }
            } else {
                return Err(std::io::Error::other(
                    "unexpected cleanup child wait status",
                ));
            };
            Ok(std::process::ExitStatus::from_raw(raw))
        })
        .transpose()
}

fn run_spawned_owned_child<A, P>(
    mut child: std::process::Child,
    deadline: Instant,
    assign_job: A,
    mut poll: P,
) -> Result<(), String>
where
    A: FnOnce(u32) -> Result<OwnedCleanupJob, String>,
    P: FnMut(&mut CleanupChildObserver<'_>) -> std::io::Result<Option<std::process::ExitStatus>>,
{
    let mut post_spawn_errors = Vec::new();
    let owned_job = match assign_job(child.id()) {
        Ok(job) => job,
        Err(error) => {
            post_spawn_errors.push(format!("cleanup helper job assignment failed: {error}"));
            unassigned_cleanup_job()
        }
    };
    if !post_spawn_errors.is_empty() {
        terminate_and_reap_owned_child(&mut child, owned_job, &mut post_spawn_errors);
        return Err(format!(
            "cleanup helper post-spawn setup failed; helper killed and reaped: {}",
            post_spawn_errors.join("; ")
        ));
    }
    loop {
        match poll(&mut CleanupChildObserver(&mut child)) {
            Ok(Some(status)) => {
                let mut failures = Vec::new();
                if !status.success() {
                    failures.push(format!("cleanup helper exited with {status}"));
                }
                terminate_and_reap_owned_child(&mut child, owned_job, &mut failures);
                return if failures.is_empty() {
                    Ok(())
                } else {
                    Err(failures.join("; "))
                };
            }
            Ok(None) => {}
            Err(error) => {
                post_spawn_errors.push(format!("cleanup helper status poll failed: {error}"));
                terminate_and_reap_owned_child(&mut child, owned_job, &mut post_spawn_errors);
                return Err(format!(
                    "cleanup helper polling failed; helper killed and reaped: {}",
                    post_spawn_errors.join("; ")
                ));
            }
        }
        if Instant::now() >= deadline {
            let mut termination_errors = Vec::new();
            terminate_and_reap_owned_child(&mut child, owned_job, &mut termination_errors);
            let detail = if termination_errors.is_empty() {
                String::new()
            } else {
                format!("; termination errors: {}", termination_errors.join("; "))
            };
            return Err(format!(
                "cleanup helper deadline exhausted; helper killed and reaped{detail}"
            ));
        }
        std::thread::sleep(Duration::from_millis(5));
    }
}

#[cfg(windows)]
type OwnedCleanupJob = Option<labby_winjob::JobObject>;
#[cfg(not(windows))]
struct OwnedCleanupJob;

#[cfg(windows)]
fn assign_cleanup_job(pid: u32) -> Result<OwnedCleanupJob, String> {
    labby_winjob::JobObject::assign(pid)
        .map(Some)
        .map_err(|error| error.to_string())
}

#[cfg(not(windows))]
fn assign_cleanup_job(_pid: u32) -> Result<OwnedCleanupJob, String> {
    Ok(OwnedCleanupJob)
}

fn unassigned_cleanup_job() -> OwnedCleanupJob {
    #[cfg(windows)]
    {
        None
    }
    #[cfg(not(windows))]
    {
        OwnedCleanupJob
    }
}

trait ChildControl {
    fn owned_pid(&self) -> Option<u32>;
    fn start_owned_kill(&mut self) -> std::io::Result<()>;
    fn final_try_wait(&mut self) -> std::io::Result<Option<std::process::ExitStatus>>;
}

impl ChildControl for std::process::Child {
    fn owned_pid(&self) -> Option<u32> {
        Some(self.id())
    }
    fn start_owned_kill(&mut self) -> std::io::Result<()> {
        self.kill()
    }
    fn final_try_wait(&mut self) -> std::io::Result<Option<std::process::ExitStatus>> {
        self.try_wait()
    }
}

impl ChildControl for Child {
    fn owned_pid(&self) -> Option<u32> {
        self.id()
    }
    fn start_owned_kill(&mut self) -> std::io::Result<()> {
        self.start_kill()
    }
    fn final_try_wait(&mut self) -> std::io::Result<Option<std::process::ExitStatus>> {
        self.try_wait()
    }
}

fn observe_owned_child(
    child: &mut impl ChildControl,
    pid: u32,
) -> std::io::Result<Option<std::process::ExitStatus>> {
    #[cfg(unix)]
    {
        let _ = child;
        observe_cleanup_child(pid)
    }
    #[cfg(not(unix))]
    {
        let _ = pid;
        child.final_try_wait()
    }
}

fn terminate_and_reap_owned_child(
    child: &mut impl ChildControl,
    owned_job: OwnedCleanupJob,
    errors: &mut Vec<String>,
) {
    let deadline = Instant::now() + Duration::from_secs(1);
    terminate_and_reap_owned_child_with_signal(child, owned_job, errors, deadline, |pid| {
        signal_cleanup_group_with_deadline(pid, deadline)
    });
}

#[cfg(unix)]
fn signal_cleanup_group(pid: u32) -> Result<(), String> {
    signal_cleanup_group_with_deadline(pid, Instant::now() + Duration::from_secs(1)).map(|_| ())
}

// A positive empty observation is consumed immediately by the same retained
// child owner. It is never cached or used to authorize a later signal.
enum SignalDisposition {
    Sent,
    #[cfg(unix)]
    ExitedAndEmpty {
        pid: u32,
    },
}

fn signal_cleanup_group_with_deadline(
    pid: u32,
    deadline: Instant,
) -> Result<SignalDisposition, String> {
    #[cfg(unix)]
    {
        signal_cleanup_group_with_probe(
            pid,
            deadline,
            |pid| {
                nix::sys::signal::killpg(
                    nix::unistd::Pid::from_raw(pid as i32),
                    nix::sys::signal::Signal::SIGKILL,
                )
            },
            process_group_members_checked_before,
        )
    }
    #[cfg(not(unix))]
    {
        let _ = (pid, deadline);
        Ok(SignalDisposition::Sent)
    }
}

#[cfg(unix)]
fn signal_cleanup_group_with_probe(
    pid: u32,
    deadline: Instant,
    signal: impl FnOnce(u32) -> Result<(), nix::errno::Errno>,
    inventory: impl FnOnce(i32, Instant) -> Result<Vec<u32>, String>,
) -> Result<SignalDisposition, String> {
    match signal(pid) {
        Ok(()) | Err(nix::errno::Errno::ESRCH) => Ok(SignalDisposition::Sent),
        Err(nix::errno::Errno::EPERM)
            if observe_cleanup_child(pid)
                .map_err(|error| format!("cleanup helper exit verification failed: {error}"))?
                .is_some()
                && inventory(pid as i32, deadline)?.is_empty() =>
        {
            // macOS reports EPERM for a group containing only its retained
            // zombie leader. Prove that exact waitable child has exited and
            // no live member remains; never ignore a live-group denial.
            Ok(SignalDisposition::ExitedAndEmpty { pid })
        }
        Err(error) => Err(format!("cleanup helper process-group kill failed: {error}")),
    }
}

fn terminate_and_reap_owned_child_with_signal(
    child: &mut impl ChildControl,
    owned_job: OwnedCleanupJob,
    errors: &mut Vec<String>,
    reap_deadline: Instant,
    mut signal_group: impl FnMut(u32) -> Result<SignalDisposition, String>,
) {
    terminate_and_reap_owned_child_with_operations(
        child,
        owned_job,
        errors,
        reap_deadline,
        &mut signal_group,
        process_group_members_for_cleanup,
    );
}

fn process_group_members_for_cleanup(group: i32, deadline: Instant) -> Result<Vec<u32>, String> {
    #[cfg(unix)]
    {
        process_group_members_checked_before(group, deadline)
    }
    #[cfg(not(unix))]
    {
        let _ = (group, deadline);
        Ok(Vec::new())
    }
}

#[cfg(unix)]
fn require_waitable_cleanup_owner(child: &impl ChildControl, pid: u32, errors: &mut Vec<String>) {
    if child.owned_pid() != Some(pid) {
        errors.push("cleanup child handle does not match its ownership PID".into());
        abort_unsettled_cleanup_helper(pid, errors);
    }
    if let Err(error) = observe_cleanup_child(pid) {
        errors.push(format!(
            "cleanup helper ownership observation failed: {error}"
        ));
        // Returning an error would drop a Tokio kill_on_drop guard, which can
        // itself signal the rejected numeric PID. Never unwind lost authority.
        abort_unsettled_cleanup_helper(pid, errors);
    }
}

#[cfg(unix)]
fn reap_verified_empty_group(
    child: &mut impl ChildControl,
    owner_pid: u32,
    observed_pid: u32,
    deadline: Instant,
    errors: &mut Vec<String>,
) {
    require_waitable_cleanup_owner(child, owner_pid, errors);
    if owner_pid != observed_pid || Instant::now() >= deadline {
        errors.push("empty cleanup observation does not match its live ownership budget".into());
        abort_unsettled_cleanup_helper(owner_pid, errors);
    }
    if !matches!(observe_owned_child(child, owner_pid), Ok(Some(_))) {
        errors.push("empty cleanup observation has no exited retained owner".into());
        abort_unsettled_cleanup_helper(owner_pid, errors);
    }
    match child.final_try_wait() {
        Ok(Some(_)) => {}
        result => {
            errors.push(format!("cleanup helper final reap failed: {result:?}"));
            abort_unsettled_cleanup_helper(owner_pid, errors);
        }
    }
}

fn terminate_and_reap_owned_child_with_operations(
    child: &mut impl ChildControl,
    owned_job: OwnedCleanupJob,
    errors: &mut Vec<String>,
    reap_deadline: Instant,
    mut signal_group: impl FnMut(u32) -> Result<SignalDisposition, String>,
    mut inventory: impl FnMut(i32, Instant) -> Result<Vec<u32>, String>,
) {
    let Some(pid) = child.owned_pid() else {
        #[cfg(unix)]
        {
            errors.push("owned Unix child was reaped before group settlement".into());
            abort_unsettled_cleanup_helper(0, errors);
        }
        #[cfg(windows)]
        if let Some(job) = owned_job {
            if let Err(error) = job.close() {
                errors.push(format!("cleanup helper job termination failed: {error}"));
            }
        }
        #[cfg(not(unix))]
        return;
    };
    #[cfg(not(unix))]
    let _ = &mut inventory;
    #[cfg(unix)]
    require_waitable_cleanup_owner(child, pid, errors);
    // Setup can fail before a group exists; direct kill/reap is still required.
    match signal_group(pid) {
        #[cfg(unix)]
        Ok(SignalDisposition::ExitedAndEmpty { pid: observed_pid }) => {
            reap_verified_empty_group(child, pid, observed_pid, reap_deadline, errors);
            return;
        }
        Ok(SignalDisposition::Sent) => {}
        Err(error) => errors.push(error),
    }
    #[cfg(windows)]
    if let Some(job) = owned_job {
        if let Err(error) = job.close() {
            errors.push(format!("cleanup helper job termination failed: {error}"));
        }
    }
    #[cfg(not(windows))]
    let _ = owned_job;
    if let Err(error) = child.start_owned_kill() {
        match observe_owned_child(child, pid) {
            Ok(Some(_)) => {}
            Ok(None) | Err(_) => errors.push(format!("cleanup helper direct kill failed: {error}")),
        }
    }
    #[cfg(unix)]
    let mut last_members = Vec::new();
    loop {
        match observe_owned_child(child, pid) {
            Ok(Some(_)) => {
                #[cfg(unix)]
                match inventory(pid as i32, reap_deadline) {
                    Ok(members) if members.is_empty() => {
                        reap_verified_empty_group(child, pid, pid, reap_deadline, errors);
                        break;
                    }
                    Ok(members) => {
                        last_members = members;
                        // The first signal need not settle every descendant.
                        // NOWAIT retains the leader's PID, preventing numeric
                        // group reuse until the final drain and reap.
                        match signal_group(pid) {
                            Ok(SignalDisposition::ExitedAndEmpty { pid: observed_pid }) => {
                                reap_verified_empty_group(
                                    child,
                                    pid,
                                    observed_pid,
                                    reap_deadline,
                                    errors,
                                );
                                break;
                            }
                            Ok(SignalDisposition::Sent) => {}
                            Err(error) => {
                                errors.push(error);
                                abort_unsettled_cleanup_helper(pid, errors);
                            }
                        }
                    }
                    Err(error) => {
                        errors.push(error);
                        abort_unsettled_cleanup_helper(pid, errors);
                    }
                }
                #[cfg(not(unix))]
                break;
            }
            Ok(None) => {}
            Err(error) => {
                errors.push(format!("cleanup helper reap failed: {error}"));
                abort_unsettled_cleanup_helper(pid, errors);
            }
        }
        if Instant::now() >= reap_deadline {
            // Returning would permit a mutation-capable helper to outlive its
            // owner, so fail-stop this disposable integration-test process.
            errors.push("cleanup helper kill/reap verification deadline exhausted".into());
            #[cfg(unix)]
            errors.push(format!(
                "last observed owned group survivors (first 32): {:?}",
                &last_members[..last_members.len().min(32)]
            ));
            abort_unsettled_cleanup_helper(pid, errors);
        }
        std::thread::sleep(Duration::from_millis(5));
    }
}

fn abort_unsettled_cleanup_helper(pid: u32, errors: &[String]) -> ! {
    use std::io::Write as _;
    // Bypass libtest capture: abort does not flush captured test output. Only
    // bounded process-control diagnostics belong here, never command arguments.
    let mut stderr = std::io::stderr().lock();
    let thread = std::thread::current();
    write_cleanup_fail_stop_evidence(&mut stderr, pid, thread.name(), errors);
    drop(stderr.flush());
    std::process::abort();
}

fn write_cleanup_fail_stop_evidence(
    output: &mut impl std::io::Write,
    pid: u32,
    thread_name: Option<&str>,
    errors: &[String],
) {
    drop(writeln!(
        output,
        "cleanup helper fail-stop: owned pid/group {pid}"
    ));
    let test_name: String = thread_name.unwrap_or("unnamed").chars().take(160).collect();
    drop(writeln!(output, "cleanup owner thread: {test_name}"));
    // Keep the terminal reason even when earlier cleanup failures fill the cap.
    for error in &errors[errors.len().saturating_sub(8)..] {
        let detail: String = error.chars().take(512).collect();
        drop(writeln!(output, "{detail}"));
    }
}

fn run_artifact_cleanup_helper(
    root: &Path,
    stdout_path: &Path,
    stderr_path: &Path,
    canaries: &[String],
    deadline: Instant,
) -> Result<Vec<String>, String> {
    let control = tempfile::tempdir().map_err(|error| error.to_string())?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        std::fs::set_permissions(control.path(), std::fs::Permissions::from_mode(0o700))
            .map_err(|error| error.to_string())?;
    }
    let canaries_path = control.path().join("canaries.json");
    let response_path = control.path().join("response.json");
    let mut canary_file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&canaries_path)
        .map_err(|error| error.to_string())?;
    // Apply the platform's owner-only policy while the file is still empty.
    labby_auth::util::harden_secret_file(&canaries_path).map_err(|error| error.to_string())?;
    std::io::Write::write_all(
        &mut canary_file,
        &serde_json::to_vec(canaries).map_err(|error| error.to_string())?,
    )
    .map_err(|error| error.to_string())?;
    drop(canary_file);
    let mut command = Command::new(std::env::current_exe().map_err(|error| error.to_string())?);
    // libtest names omit the crate prefix, while this shared module is included
    // at different depths by different integration-test binaries.
    let module = module_path!()
        .split_once("::")
        .expect("nested test support")
        .1;
    let helper_name = format!("{module}::tests::artifact_cleanup_helper_entrypoint");
    command
        .args([&helper_name, "--exact", "--ignored", "--nocapture"])
        .env("LABBY_ARTIFACT_HELPER_ROOT", root)
        .env("LABBY_ARTIFACT_HELPER_STDOUT", stdout_path)
        .env("LABBY_ARTIFACT_HELPER_STDERR", stderr_path)
        .env("LABBY_ARTIFACT_HELPER_CANARIES", &canaries_path)
        .env("LABBY_ARTIFACT_HELPER_RESPONSE", &response_path);
    run_owned_command(&mut command, deadline)?;
    let response = std::fs::read(&response_path).map_err(|error| error.to_string())?;
    serde_json::from_slice(&response).map_err(|error| error.to_string())
}

// Diagnostic-only formatting. Ownership paths use the fallible native probe.
fn process_start_identity(pid: u32) -> String {
    process_start_identity_before(pid, Instant::now() + Duration::from_secs(1))
        .unwrap_or_else(|_| format!("pid:{pid}:unknown"))
}

fn capture_spawned_child_identity(
    child: &mut Child,
    address: SocketAddr,
    probe: Option<&IdentityProbe>,
    #[cfg(unix)] admission: Option<&Path>,
    deadline: Instant,
) -> Result<(String, OwnedCleanupJob), String> {
    let pid = child.id().ok_or("spawned child has no owned PID")?;
    let job = match assign_cleanup_job(pid) {
        Ok(job) => job,
        Err(error) => {
            let mut failures = vec![format!("daemon containment assignment failed: {error}")];
            terminate_and_reap_owned_child(child, unassigned_cleanup_job(), &mut failures);
            return Err(failures.join("; "));
        }
    };
    #[cfg(unix)]
    let mut failed_probe = None;
    let observation = match probe {
        Some(probe) => probe(
            pid,
            address,
            {
                #[cfg(unix)]
                {
                    admission
                }
                #[cfg(not(unix))]
                {
                    None
                }
            },
            deadline,
        ),
        None => {
            let deadline = deadline.min(Instant::now() + Duration::from_secs(1));
            #[cfg(unix)]
            {
                process_identity::capture_typed(pid, deadline).map_err(|failure| {
                    let message = failure.message.clone();
                    failed_probe = Some(failure);
                    message
                })
            }
            #[cfg(not(unix))]
            process_start_identity_before(pid, deadline)
        }
    }
    .and_then(|identity| {
        #[cfg(unix)]
        process_identity::validate(pid, &identity)?;
        Ok(identity)
    });
    match observation {
        Ok(identity) => Ok((identity, job)),
        Err(error) => {
            let mut failures = vec![format!("daemon process identity capture failed: {error}")];
            // The retained Child is independent cleanup authority. Never store
            // an unknown fallback and later compare two failed observations.
            terminate_and_reap_owned_child(child, job, &mut failures);
            #[cfg(unix)]
            if let Some(failure) = failed_probe {
                // Only after the retained daemon child has settled may a
                // failed native probe trigger its own fail-stop boundary.
                drop(settle_inventory_failure(failure));
            }
            Err(failures.join("; "))
        }
    }
}

fn process_start_identity_before(pid: u32, deadline: Instant) -> Result<String, String> {
    #[cfg(unix)]
    {
        process_identity::capture(pid, deadline)
    }
    #[cfg(not(unix))]
    {
        let _ = deadline;
        // Display only: Windows cleanup authority is the retained Child/Job
        // handle, never a repeated numeric PID observation.
        Ok(format!("pid:{pid}:owned-child-handle"))
    }
}

#[cfg(unix)]
fn owned_process_identity_matches(ledger: &OwnershipLedger) -> bool {
    owned_process_identity_matches_before(ledger, Instant::now() + Duration::from_secs(1))
}

#[cfg(unix)]
fn owned_process_identity_matches_before(ledger: &OwnershipLedger, deadline: Instant) -> bool {
    let (Some(pid), Some(expected)) = (ledger.pid, ledger.process_start_identity.as_deref()) else {
        return false;
    };
    if process_identity::validate(pid, expected).is_err() {
        return false;
    }
    process_identity::matches(
        Some(pid),
        Some(expected),
        process_start_identity_before(pid, deadline),
    )
}

#[cfg(unix)]
fn process_group_members_checked(group: i32) -> Result<Vec<u32>, String> {
    process_group_members_checked_before(group, Instant::now() + Duration::from_secs(1))
}

#[cfg(unix)]
fn process_group_members_checked_before(group: i32, deadline: Instant) -> Result<Vec<u32>, String> {
    process_group_members_typed(group, deadline).map_err(settle_inventory_failure)
}

#[cfg(unix)]
fn process_group_members_typed(
    group: i32,
    deadline: Instant,
) -> Result<Vec<u32>, process_inventory::Failure> {
    // Linux can expose a process that exits between `ps` collecting its PID
    // and PGID columns as a transient `-` PGID. Never authorize from that
    // partial snapshot, but allow a fresh, fully parseable snapshot to replace
    // it while the original cleanup deadline still applies.
    let mut last_parse_error = None;
    for _ in 0..3 {
        let text = process_inventory::read(deadline)?;
        match parse_process_group_inventory(group, &text) {
            Ok(members) => return Ok(members),
            Err(error) => last_parse_error = Some(error),
        }
    }
    Err(last_parse_error
        .expect("at least one inventory snapshot was parsed")
        .into())
}

#[cfg(unix)]
fn settle_inventory_failure(mut failure: process_inventory::Failure) -> String {
    if let Some(mut probe) = failure.unsettled.take() {
        // Observational probes grant no authority over the observed PID/group.
        // This handle owns only the native probe itself.
        drop(probe.kill());
        abort_unsettled_cleanup_helper(probe.id(), &[failure.message]);
    }
    failure.message
}

#[cfg(unix)]
fn parse_process_group_inventory(group: i32, inventory: &str) -> Result<Vec<u32>, String> {
    let mut members = Vec::new();
    for (row, line) in inventory
        .lines()
        .enumerate()
        .filter(|(_, line)| !line.trim().is_empty())
    {
        let mut fields = line.split_whitespace();
        let pid = fields.next().and_then(|value| value.parse::<u32>().ok());
        let pgid = fields.next().and_then(|value| value.parse::<i32>().ok());
        let state = fields.next();
        let (Some(pid), Some(pgid), Some(state)) = (pid, pgid, state) else {
            return Err(format!(
                "process inventory contained an invalid row: row={} fields={} pid_numeric={} pgid_numeric={} state_present={}",
                row + 1,
                line.split_whitespace().count(),
                pid.is_some(),
                pgid.is_some(),
                state.is_some(),
            ));
        };
        if pgid == group && !state.starts_with('Z') {
            members.push(pid);
        }
    }
    Ok(members)
}

#[cfg(unix)]
fn process_group_inventory(group: i32) -> Result<Vec<String>, process_inventory::Failure> {
    let deadline = Instant::now() + Duration::from_secs(1);
    let members = process_group_members_typed(group, deadline)?;
    let mut result = Vec::new();
    for pid in members.into_iter().take(32) {
        result.push(process_identity::capture_typed(pid, deadline)?);
    }
    Ok(result)
}

#[cfg(unix)]
fn configure_process_group(command: &mut TokioCommand) {
    command.process_group(0);
}

#[cfg(not(unix))]
fn configure_process_group(_command: &mut TokioCommand) {}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    };

    #[test]
    fn artifact_cleanup_subprocess_executes_the_exact_nested_entrypoint() {
        let root = tempfile::tempdir().unwrap();
        let stdout = root.path().join("stdout.log");
        let stderr = root.path().join("stderr.log");
        std::fs::write(&stdout, b"clean output").unwrap();
        std::fs::write(&stderr, b"clean errors").unwrap();
        let failures = run_artifact_cleanup_helper(
            root.path(),
            &stdout,
            &stderr,
            &[],
            Instant::now() + Duration::from_secs(5),
        )
        .expect("helper must actually execute and publish a response");
        assert!(failures.is_empty(), "{failures:?}");
    }

    #[test]
    fn directory_enumeration_is_bounded_before_queueing_entries() {
        let root = tempfile::tempdir().unwrap();
        std::fs::create_dir(root.path().join("directory-also-counts")).unwrap();
        let mut discovered = CLEANUP_MAX_FILES;
        let mut pending = Vec::new();
        let error = enqueue_cleanup_directory(
            root.path(),
            0,
            &mut pending,
            &mut discovered,
            Instant::now() + Duration::from_secs(1),
        )
        .unwrap_err();
        assert!(error.contains("entry cap exceeded"));
        assert!(pending.is_empty());
        assert_eq!(discovered, CLEANUP_MAX_FILES);
    }

    #[test]
    fn cleanup_fail_stop_evidence_preserves_reason_with_bounded_output() {
        let mut output = Vec::new();
        let mut errors = vec!["oldest-error-must-be-omitted".to_string()];
        errors.extend((0..7).map(|_| "é".repeat(600)));
        errors.push("terminal process inventory failed".into());
        write_cleanup_fail_stop_evidence(&mut output, 123, Some(&"t".repeat(200)), &errors);
        let report = String::from_utf8(output).unwrap();
        assert!(report.contains("owned pid/group 123"));
        assert!(report.contains("terminal process inventory failed"));
        assert!(!report.contains("oldest-error-must-be-omitted"));
        assert_eq!(report.lines().count(), 10);
        assert_eq!(
            report.lines().nth(1).unwrap().len(),
            "cleanup owner thread: ".len() + 160
        );
        assert!(
            report
                .lines()
                .skip(2)
                .take(7)
                .all(|line| line.chars().count() == 512)
        );
    }

    #[cfg(unix)]
    #[test]
    fn process_inventory_rejects_invalid_rows_and_ignores_only_known_zombies() {
        assert_eq!(
            parse_process_group_inventory(12, "10 12 S\n11 12 Z+\n13 14 R\n").unwrap(),
            vec![10]
        );
        assert!(parse_process_group_inventory(12, "unavailable").is_err());
        assert!(parse_process_group_inventory(12, "10 12").is_err());
        let error =
            parse_process_group_inventory(12, "10 12 S\nprivate-sentinel 12\n").unwrap_err();
        assert_eq!(
            error,
            "process inventory contained an invalid row: row=2 fields=2 pid_numeric=false pgid_numeric=true state_present=false"
        );
        assert!(!error.contains("private-sentinel"));
        assert!(error.len() < 200);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn leader_exit_does_not_leave_term_resistant_descendants_alive() {
        let mut guard = LiveLabbyBuilder::new().start().await.unwrap();
        guard
            .stop_process(Instant::now() + Duration::from_secs(5))
            .await
            .unwrap();
        let marker = guard.root.join("descendant-ready");
        let mut command = TokioCommand::new("sh");
        command
            .args([
                "-c",
                "(trap '' TERM; echo ready >\"$1\"; while :; do sleep 1; done) & wait",
                "fixture",
            ])
            .arg(&marker)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .kill_on_drop(true);
        configure_process_group(&mut command);
        let child = command.spawn().unwrap();
        guard.ledger.pid = child.id();
        guard.ledger.guardian_pid = None;
        guard.ledger.daemon_pid = None;
        guard.ledger.daemon_process_start_identity = None;
        guard.guardian_admission = None;
        guard.ledger.process_start_identity = guard.ledger.pid.map(process_start_identity);
        guard.ledger.process_group = guard.ledger.pid.and_then(|pid| i32::try_from(pid).ok());
        write_ledger(&guard.manifest_path, &guard.ledger).unwrap();
        guard.child = Some(child);
        let deadline = Instant::now() + Duration::from_secs(5);
        while !marker.exists() {
            assert!(Instant::now() < deadline, "descendant did not become ready");
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        let group = guard.ledger.process_group.unwrap();
        let cleanup = guard.finish().await;
        assert!(cleanup.is_clean(), "{:?}", cleanup.failures);
        assert!(cleanup.forced, "surviving descendant requires escalation");
        assert!(process_group_members_checked(group).unwrap().is_empty());
    }

    #[test]
    #[ignore = "one-shot artifact cleanup subprocess entrypoint"]
    fn artifact_cleanup_helper_entrypoint() {
        let root = PathBuf::from(std::env::var_os("LABBY_ARTIFACT_HELPER_ROOT").unwrap());
        let stdout_path = PathBuf::from(std::env::var_os("LABBY_ARTIFACT_HELPER_STDOUT").unwrap());
        let stderr_path = PathBuf::from(std::env::var_os("LABBY_ARTIFACT_HELPER_STDERR").unwrap());
        let canaries_path =
            PathBuf::from(std::env::var_os("LABBY_ARTIFACT_HELPER_CANARIES").unwrap());
        let response_path =
            PathBuf::from(std::env::var_os("LABBY_ARTIFACT_HELPER_RESPONSE").unwrap());
        let canaries: Vec<String> =
            serde_json::from_slice(&std::fs::read(canaries_path).unwrap()).unwrap();
        let deadline = Instant::now() + DEFAULT_DEADLINE;
        let mut failures = Vec::new();
        let mut scan_budget = ScanBudget {
            deadline,
            bytes_remaining: CLEANUP_MAX_BYTES,
        };
        for path in [&stdout_path, &stderr_path] {
            if let Err(error) = cap_log_file(path) {
                failures.push(format!("log retention failed: {error}"));
            }
            scan_file_for_canaries_bounded(path, &canaries, &mut failures, &mut scan_budget);
        }
        cap_log_tree_bounded(&root.join("logs"), &mut failures, deadline);
        scan_artifact_tree_with_budget(&root, &canaries, &mut failures, &mut scan_budget);
        std::fs::write(response_path, serde_json::to_vec(&failures).unwrap()).unwrap();
    }

    #[cfg(unix)]
    #[test]
    #[ignore = "outer supervisor cancellation fixture"]
    fn supervised_cleanup_cancellation_fixture() {
        let marker = std::env::var_os("LABBY_E2E_WEDGED_MARKER").expect("owned marker");
        let mut command = Command::new("/bin/sh");
        command
            .args([
                "-c",
                "trap '' TERM; sleep 6; touch -- \"$1\"; while :; do sleep 1; done",
                "fixture",
            ])
            .arg(marker);
        run_owned_command(&mut command, Instant::now() + Duration::from_secs(30)).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn closed_supervisor_admission_cannot_execute_mutation() {
        let registry = tempfile::tempdir().unwrap();
        use std::os::unix::fs::PermissionsExt as _;
        std::fs::set_permissions(registry.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
        std::fs::create_dir(registry.path().join("closed")).unwrap();
        let marker = registry.path().join("forbidden-mutation");
        let mut command = Command::new("/usr/bin/touch");
        command.arg(&marker);
        let mut wrapper =
            supervised_cleanup_command(&command, registry.path(), "test-owned-token").unwrap();
        let error =
            run_owned_command(&mut wrapper, Instant::now() + Duration::from_secs(2)).unwrap_err();
        assert!(error.contains("70"), "{error}");
        assert!(!marker.exists());
    }

    #[cfg(unix)]
    #[test]
    fn stale_pid_and_cross_admission_status_cannot_complete_a_new_helper() {
        use std::os::unix::fs::PermissionsExt as _;
        let registry = tempfile::tempdir().unwrap();
        std::fs::set_permissions(registry.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
        let mut command = Command::new("/bin/sh");
        command.args(["-c", "exit 7"]);
        let previous =
            supervised_cleanup_command(&command, registry.path(), "owned-token").unwrap();
        let current = supervised_cleanup_command(&command, registry.path(), "owned-token").unwrap();
        let previous = supervised_admission_path(&previous, registry.path()).unwrap();
        let current = supervised_admission_path(&current, registry.path()).unwrap();
        assert_ne!(previous, current);
        let reused_pid = 12345;
        let legacy = registry.path().join(reused_pid.to_string());
        std::fs::create_dir(&legacy).unwrap();
        std::fs::write(legacy.join("status"), b"0\n").unwrap();
        std::fs::create_dir(&previous).unwrap();
        let old_status = format!(
            "{reused_pid}\n{}\n0\n",
            previous.file_name().unwrap().to_str().unwrap()
        );
        std::fs::write(previous.join("status"), &old_status).unwrap();
        assert!(
            supervised_helper_status(&current, reused_pid)
                .unwrap()
                .is_none()
        );
        std::fs::create_dir(&current).unwrap();
        std::fs::write(current.join("status"), old_status).unwrap();
        assert!(supervised_helper_status(&current, reused_pid).is_err());
        let id = current.file_name().unwrap().to_str().unwrap();
        std::fs::write(
            current.join("status"),
            format!("{}\n{id}\n0\n", reused_pid + 1),
        )
        .unwrap();
        assert!(supervised_helper_status(&current, reused_pid).is_err());
        std::fs::write(current.join("status"), format!("{reused_pid}\n{id}\n7\n")).unwrap();
        assert_eq!(
            supervised_helper_status(&current, reused_pid)
                .unwrap()
                .unwrap()
                .code(),
            Some(7)
        );
    }

    #[cfg(unix)]
    #[test]
    fn guardian_admission_does_not_collide_with_a_reused_numeric_pid_record() {
        use std::os::unix::{fs::PermissionsExt as _, process::CommandExt as _};
        let registry = tempfile::tempdir().unwrap();
        std::fs::set_permissions(registry.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
        let marker = registry.path().join("executed");
        let mut fixture = Command::new("/bin/sh");
        fixture
            .args(["-c", "touch -- \"$1\"; exit 7", "fixture"])
            .arg(&marker);
        let wrapper = supervised_cleanup_command(&fixture, registry.path(), "reuse-token").unwrap();
        let admission = supervised_admission_path(&wrapper, registry.path()).unwrap();
        // Deterministically recreate the retained record for this exact new
        // leader's PID before admission, without waiting for kernel PID wrap.
        let script = CLEANUP_HELPER_ADMISSION_GATE.replace(
            "admission_id=${admission##*/}",
            "mkdir \"$registry/$$\"\nprintf '0\\n' >\"$registry/$$/status\"\nadmission_id=${admission##*/}",
        );
        let mut command = Command::new("/bin/sh");
        command
            .env_clear()
            .env("PATH", "/usr/bin:/bin")
            .env("LABBY_E2E_GROUP_TOKEN", "reuse-token")
            .env("LABBY_E2E_ADMISSION_ID", admission.file_name().unwrap())
            .args(["-c", &script, "reused-pid-fixture"])
            .arg(registry.path())
            .arg(fixture.get_program())
            .args(fixture.get_args())
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .process_group(0);
        let child = command.spawn().unwrap();
        let pid = child.id();
        let result = run_spawned_owned_child(
            child,
            Instant::now() + Duration::from_secs(3),
            assign_cleanup_job,
            |child| match supervised_helper_status(&admission, child.id())? {
                Some(status) => Ok(Some(status)),
                None => child.try_wait(),
            },
        );
        assert!(result.unwrap_err().contains("exit status: 7"));
        assert!(marker.exists());
        assert_eq!(
            std::fs::read_to_string(registry.path().join(pid.to_string()).join("status")).unwrap(),
            "0\n"
        );
        assert!(
            process_group_members_checked(pid as i32)
                .unwrap()
                .is_empty()
        );
    }

    #[cfg(unix)]
    #[test]
    fn helper_stderr_is_bounded_and_only_reports_safe_categories() {
        let mut command = Command::new("/bin/sh");
        command.args([
            "-c",
            "printf 'mkdir: secret=private-sentinel: File exists\\n' >&2; exit 1",
        ]);
        let error =
            run_owned_command(&mut command, Instant::now() + Duration::from_secs(3)).unwrap_err();
        assert!(error.contains("already_exists"), "{error}");
        assert!(!error.contains("private-sentinel"), "{error}");
        assert!(error.len() < 512, "{error}");
        let summary = cleanup_stderr_summary(&vec![b'x'; 2048], true);
        assert!(summary.contains("bytes=2048 truncated=true"));
        assert!(summary.len() < 256);
    }

    #[cfg(unix)]
    #[test]
    fn supervised_guardian_reports_command_status_and_is_reaped() {
        use std::os::unix::{fs::PermissionsExt as _, process::CommandExt as _};
        for mode in ["remove-marker", "invalid-command-fixture"] {
            let registry = tempfile::tempdir().unwrap();
            std::fs::set_permissions(registry.path(), std::fs::Permissions::from_mode(0o700))
                .unwrap();
            let marker = registry.path().join("marker");
            std::fs::write(&marker, b"owned").unwrap();
            let mut command = Command::new(env!("CARGO_BIN_EXE_live-harness-fixture"));
            command.args([mode, "0"]).arg(&marker);
            let mut wrapper =
                supervised_cleanup_command(&command, registry.path(), "test-owned-token").unwrap();
            let admission = supervised_admission_path(&wrapper, registry.path()).unwrap();
            wrapper
                .process_group(0)
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null());
            let child = wrapper.spawn().unwrap();
            let group = child.id() as i32;
            let result = run_spawned_owned_child(
                child,
                Instant::now() + Duration::from_secs(3),
                assign_cleanup_job,
                |child| match supervised_helper_status(&admission, child.id())? {
                    Some(status) => Ok(Some(status)),
                    None => child.try_wait(),
                },
            );
            if mode == "remove-marker" {
                result.unwrap();
                assert!(!marker.exists());
            } else {
                assert!(result.unwrap_err().contains("cleanup helper exited"));
            }
            assert!(process_group_members_checked(group).unwrap().is_empty());
        }
    }

    #[test]
    fn artifact_scan_detects_a_canary_across_stream_chunks() {
        let temp = tempfile::tempdir().unwrap();
        let artifact = temp.path().join("large.log");
        let canary = "stream-boundary-canary";
        let mut bytes = vec![b'x'; 64 * 1024 - 7];
        bytes.extend_from_slice(canary.as_bytes());
        bytes.extend(std::iter::repeat_n(b'y', 64 * 1024));
        std::fs::write(&artifact, bytes).unwrap();
        let mut failures = Vec::new();
        scan_file_for_canaries(&artifact, &[canary.into()], &mut failures);
        assert_eq!(failures.len(), 1);
    }

    #[test]
    fn recursive_cleanup_enforces_depth_and_deadline_budgets() {
        let temp = tempfile::tempdir().unwrap();
        let mut nested = temp.path().to_path_buf();
        for index in 0..=CLEANUP_MAX_DEPTH {
            nested = nested.join(index.to_string());
            std::fs::create_dir(&nested).unwrap();
        }
        std::fs::write(nested.join("beyond-budget.log"), b"safe").unwrap();

        let mut failures = Vec::new();
        scan_artifact_tree_bounded(
            temp.path(),
            &[],
            &mut failures,
            Instant::now() + Duration::from_secs(1),
        );
        assert!(failures.iter().any(|failure| failure.contains("depth cap")));

        failures.clear();
        cap_log_tree_bounded(temp.path(), &mut failures, Instant::now());
        assert!(
            failures
                .iter()
                .any(|failure| failure.contains("deadline exhausted"))
        );
    }

    #[tokio::test(flavor = "current_thread")]
    async fn blocking_cleanup_work_does_not_stall_the_async_executor() {
        let cleanup = run_cleanup_blocking(
            Instant::now() + Duration::from_secs(1),
            "blocking proof",
            |_| {
                std::thread::sleep(Duration::from_millis(50));
                42
            },
        );
        let tick = tokio::time::sleep(Duration::from_millis(5));
        let (value, ()) = tokio::join!(cleanup, tick);
        assert_eq!(value.unwrap(), 42);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn deadline_aware_cleanup_settles_before_return() {
        let settled = Arc::new(AtomicBool::new(false));
        let worker_settled = Arc::clone(&settled);
        let mutations = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let worker_mutations = Arc::clone(&mutations);
        run_cleanup_blocking(
            Instant::now() + Duration::from_millis(5),
            "owned timeout proof",
            move |deadline| {
                while Instant::now() < deadline {
                    worker_mutations.fetch_add(1, Ordering::SeqCst);
                    std::hint::spin_loop();
                }
                worker_settled.store(true, Ordering::SeqCst);
            },
        )
        .await
        .unwrap();

        assert!(settled.load(Ordering::SeqCst));
        let after_return = mutations.load(Ordering::SeqCst);
        std::thread::sleep(Duration::from_millis(5));
        assert_eq!(mutations.load(Ordering::SeqCst), after_return);
    }

    #[test]
    fn non_cooperative_cleanup_helper_is_killed_reaped_and_cannot_mutate_later() {
        let temp = tempfile::tempdir().unwrap();
        let marker = temp.path().join("post-deadline-mutation");
        let mut command = delayed_mutation_fixture(&marker);

        let error = run_owned_command(&mut command, Instant::now() + Duration::from_millis(20))
            .unwrap_err();

        assert!(error.contains("killed and reaped"), "{error}");
        assert!(!marker.exists());
        std::thread::sleep(Duration::from_millis(300));
        assert!(
            !marker.exists(),
            "killed helper mutated after cleanup returned"
        );
    }

    #[cfg(unix)]
    #[test]
    fn successful_helper_leader_exit_still_reaps_background_mutations() {
        let temp = tempfile::tempdir().unwrap();
        let marker = temp.path().join("late-helper-mutation");
        let mut command = Command::new("sh");
        command
            .args(["-c", "(sleep 0.20; touch -- \"$1\") & exit 0", "fixture"])
            .arg(&marker);
        run_owned_command(&mut command, Instant::now() + Duration::from_secs(2)).unwrap();
        std::thread::sleep(Duration::from_millis(300));
        assert!(
            !marker.exists(),
            "helper descendant mutated after successful settlement"
        );
    }

    #[cfg(unix)]
    #[test]
    fn absence_evidence_rejects_dangling_links_and_unreadable_paths() {
        use std::os::unix::fs::symlink;
        let temp = tempfile::tempdir().unwrap();
        let dangling = temp.path().join("dangling");
        symlink(temp.path().join("missing"), &dangling).unwrap();
        assert!(
            NonEmptyAbsencePaths::try_from_paths(vec![dangling])
                .unwrap()
                .any_exists()
                .unwrap()
        );
        let loop_path = temp.path().join("loop");
        symlink(&loop_path, &loop_path).unwrap();
        let evidence =
            NonEmptyAbsencePaths::try_from_paths(vec![loop_path.join("credential")]).unwrap();
        assert!(
            evidence
                .any_exists()
                .unwrap_err()
                .contains("could not be verified")
        );
    }

    #[test]
    fn containment_failure_still_falls_back_to_direct_kill_and_bounded_reap() {
        let temp = tempfile::tempdir().unwrap();
        let marker = temp.path().join("post-settlement-mutation");
        let mut child = delayed_mutation_fixture(&marker).spawn().unwrap();
        let mut errors = vec!["injected process-group termination failure".to_owned()];

        terminate_and_reap_owned_child(&mut child, unassigned_cleanup_job(), &mut errors);

        assert_eq!(errors, ["injected process-group termination failure"]);
        std::thread::sleep(Duration::from_millis(300));
        assert!(
            !marker.exists(),
            "fallback-killed helper mutated after reap"
        );
    }

    enum InjectedPostSpawnFailure {
        Assignment,
        Poll,
    }

    fn delayed_mutation_fixture(marker: &Path) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_live-harness-fixture"));
        command.args(["delayed-mutation", "0"]).arg(marker);
        command
    }

    fn assert_injected_post_spawn_failure_settles(failure: InjectedPostSpawnFailure) {
        let temp = tempfile::tempdir().unwrap();
        let marker = temp.path().join("post-settlement-mutation");
        let mut command = delayed_mutation_fixture(&marker);
        #[cfg(unix)]
        {
            use std::os::unix::process::CommandExt as _;
            command.process_group(0);
        }
        let child = command.spawn().unwrap();
        let error = match failure {
            InjectedPostSpawnFailure::Assignment => run_spawned_owned_child(
                child,
                Instant::now() + Duration::from_secs(1),
                |_| Err("injected post-spawn setup failure".to_owned()),
                |child| child.try_wait(),
            ),
            InjectedPostSpawnFailure::Poll => run_spawned_owned_child(
                child,
                Instant::now() + Duration::from_secs(1),
                |_| Ok(unassigned_cleanup_job()),
                |_| Err(std::io::Error::other("injected child status poll failure")),
            ),
        }
        .unwrap_err();

        assert!(error.contains("helper killed and reaped"), "{error}");
        assert!(error.contains("injected"), "{error}");
        std::thread::sleep(Duration::from_millis(300));
        assert!(!marker.exists(), "spawned helper mutated after settlement");
    }

    #[test]
    fn post_spawn_setup_failure_is_killed_and_reaped_before_return() {
        assert_injected_post_spawn_failure_settles(InjectedPostSpawnFailure::Assignment);
    }

    #[test]
    fn child_status_poll_failure_is_killed_and_reaped_before_return() {
        assert_injected_post_spawn_failure_settles(InjectedPostSpawnFailure::Poll);
    }

    #[test]
    fn artifact_scan_enforces_actual_byte_budget_inside_a_file() {
        let temp = tempfile::tempdir().unwrap();
        let artifact = temp.path().join("over-budget.log");
        std::fs::write(&artifact, b"ninebytes").unwrap();
        let mut failures = Vec::new();
        let mut budget = ScanBudget {
            deadline: Instant::now() + Duration::from_secs(1),
            bytes_remaining: 8,
        };

        scan_file_for_canaries_bounded(&artifact, &[], &mut failures, &mut budget);

        assert_eq!(budget.bytes_remaining, 0);
        assert!(
            failures
                .iter()
                .any(|failure| failure.contains("byte cap exceeded")),
            "{failures:?}"
        );
    }

    #[test]
    fn artifact_scan_checks_deadline_during_file_reads() {
        let temp = tempfile::tempdir().unwrap();
        let artifact = temp.path().join("deadline.log");
        std::fs::write(&artifact, b"safe").unwrap();
        let mut failures = Vec::new();
        let mut budget = ScanBudget {
            deadline: Instant::now(),
            bytes_remaining: CLEANUP_MAX_BYTES,
        };

        scan_file_for_canaries_bounded(&artifact, &[], &mut failures, &mut budget);

        assert!(
            failures
                .iter()
                .any(|failure| failure.contains("deadline exhausted")),
            "{failures:?}"
        );
    }

    #[test]
    fn direct_and_recursive_artifact_scans_share_one_byte_budget() {
        let temp = tempfile::tempdir().unwrap();
        let direct = temp.path().join("stdout.log");
        let tree = temp.path().join("artifacts");
        std::fs::create_dir(&tree).unwrap();
        std::fs::write(&direct, b"123456").unwrap();
        std::fs::write(tree.join("later.log"), b"789").unwrap();
        let mut failures = Vec::new();
        let mut budget = ScanBudget {
            deadline: Instant::now() + Duration::from_secs(1),
            bytes_remaining: 8,
        };

        scan_file_for_canaries_bounded(&direct, &[], &mut failures, &mut budget);
        scan_artifact_tree_with_budget(&tree, &[], &mut failures, &mut budget);

        assert_eq!(budget.bytes_remaining, 0);
        assert!(
            failures
                .iter()
                .any(|failure| failure.contains("byte cap exceeded")),
            "{failures:?}"
        );
    }

    #[test]
    fn log_tail_and_retention_read_only_the_bounded_suffix() {
        let temp = tempfile::tempdir().unwrap();
        let artifact = temp.path().join("large.log");
        let mut file = std::fs::File::create(&artifact).unwrap();
        file.set_len((LOG_TAIL_BYTES * 64) as u64).unwrap();
        file.seek(SeekFrom::End(-(LOG_TAIL_BYTES as i64))).unwrap();
        std::io::Write::write_all(&mut file, &vec![b'z'; LOG_TAIL_BYTES]).unwrap();
        drop(file);
        assert_eq!(tail(&artifact).len(), LOG_TAIL_BYTES);
        cap_log_file(&artifact).unwrap();
        assert_eq!(
            std::fs::metadata(&artifact).unwrap().len(),
            LOG_TAIL_BYTES as u64
        );
    }

    #[cfg(unix)]
    #[test]
    fn process_signaling_requires_the_recorded_start_identity() {
        let pid = std::process::id();
        let mut ledger = OwnershipLedger {
            pid: Some(pid),
            process_start_identity: Some(process_start_identity(pid)),
            ..OwnershipLedger::default()
        };
        assert!(owned_process_identity_matches(&ledger));
        ledger.process_start_identity = Some(format!("pid:{pid}:reused"));
        assert!(!owned_process_identity_matches(&ledger));
    }

    #[test]
    fn isolated_children_do_not_inherit_cloud_git_ssh_proxy_or_provider_state() {
        let temp = tempfile::tempdir().unwrap();
        let command = isolated_command(temp.path());
        let explicit = command
            .get_envs()
            .filter_map(|(key, value)| value.map(|_| key.to_string_lossy().into_owned()))
            .collect::<BTreeSet<_>>();
        for forbidden in [
            "AWS_ACCESS_KEY_ID",
            "AZURE_CLIENT_SECRET",
            "GOOGLE_APPLICATION_CREDENTIALS",
            "GITHUB_TOKEN",
            "GH_TOKEN",
            "SSH_AUTH_SOCK",
            "HTTP_PROXY",
            "HTTPS_PROXY",
            "OPENAI_API_KEY",
            "ANTHROPIC_API_KEY",
        ] {
            assert!(
                !explicit.contains(forbidden),
                "ambient {forbidden} was inherited"
            );
        }
        let mut expected = BTreeSet::from([
            "HOME".into(),
            "LABBY_HOME".into(),
            "LABBY_LOG_DIR".into(),
            "LABBY_MCP_HTTP_PORT".into(),
            "PATH".into(),
            "TMPDIR".into(),
        ]);
        for key in ["SystemRoot", "WINDIR"] {
            if cfg!(windows) && std::env::var_os(key).is_some() {
                expected.insert(key.into());
            }
        }
        assert_eq!(explicit, expected);
    }

    #[tokio::test]
    async fn isolated_runtime_supports_tcp_before_and_after_restart() {
        #[cfg(windows)]
        assert!(
            isolated_runtime_env()
                .iter()
                .any(|(key, _)| *key == "SystemRoot")
        );
        let mut guard = LiveLabbyBuilder::new()
            .start()
            .await
            .expect("isolated HTTP daemon");
        for restarted in [false, true] {
            if restarted {
                guard.restart().await.expect("restart isolated HTTP daemon");
            }
            let address = guard
                .connection()
                .base_url
                .trim_start_matches("http://")
                .to_string();
            tokio::time::timeout(
                Duration::from_secs(5),
                tokio::net::TcpStream::connect(&address),
            )
            .await
            .expect("bounded TCP connect")
            .expect("TCP socket");
        }
        let cleanup = guard.finish().await;
        assert!(
            cleanup.is_clean(),
            "isolated runtime cleanup: {:?}",
            cleanup.failures
        );
    }

    #[tokio::test]
    async fn ownership_validation_rejects_nonce_partial_stale_and_pid_reuse_simulations() {
        let mut guard = LiveLabbyBuilder::new().start().await.expect("live labby");
        let original_nonce = std::fs::read(&guard.nonce_path).unwrap();
        let original_manifest = std::fs::read(&guard.manifest_path).unwrap();
        let original_ledger = guard.ledger.clone();

        std::fs::write(&guard.nonce_path, "foreign-nonce").unwrap();
        assert!(
            guard
                .validate_ownership()
                .unwrap_err()
                .contains("nonce mismatch")
        );
        std::fs::write(&guard.nonce_path, &original_nonce).unwrap();

        std::fs::write(&guard.manifest_path, b"{\"partial\":").unwrap();
        assert!(
            guard
                .validate_ownership()
                .unwrap_err()
                .contains("invalid ownership manifest")
        );
        std::fs::write(&guard.manifest_path, &original_manifest).unwrap();

        let mut stale = original_ledger.clone();
        stale.generation += 1;
        write_ledger(&guard.manifest_path, &stale).unwrap();
        assert!(
            guard
                .validate_ownership()
                .unwrap_err()
                .contains("stale ownership")
        );
        std::fs::write(&guard.manifest_path, &original_manifest).unwrap();

        let mut swapped_daemon = original_ledger.clone();
        swapped_daemon.daemon_pid = Some(u32::MAX);
        write_ledger(&guard.manifest_path, &swapped_daemon).unwrap();
        assert!(
            guard
                .validate_ownership()
                .unwrap_err()
                .contains("stale ownership")
        );
        std::fs::write(&guard.manifest_path, &original_manifest).unwrap();

        #[cfg(unix)]
        {
            guard.ledger.process_start_identity = Some("pid-reuse-simulation".into());
            write_ledger(&guard.manifest_path, &guard.ledger).unwrap();
            assert!(
                guard
                    .validate_ownership()
                    .unwrap_err()
                    .contains("start identity")
            );
            guard.ledger = original_ledger;
            std::fs::write(&guard.manifest_path, &original_manifest).unwrap();
        }

        let cleanup = guard.finish().await;
        assert!(cleanup.is_clean(), "{:?}", cleanup.failures);
    }

    #[tokio::test]
    async fn stale_owned_lock_is_removed_and_verified_during_cleanup() {
        let mut guard = LiveLabbyBuilder::new().start().await.expect("live labby");
        let lock = guard.root.join("stale-owned.lock");
        std::fs::write(&lock, "owned").unwrap();
        guard.ledger.locks.push(lock.clone());
        write_ledger(&guard.manifest_path, &guard.ledger).unwrap();
        let cleanup = guard.finish().await;
        assert!(cleanup.is_clean(), "{:?}", cleanup.failures);
        assert!(!lock.exists());
    }

    #[tokio::test]
    async fn credential_sessions_are_revoked_instead_of_only_forgotten() {
        let mut guard = LiveLabbyBuilder::new().start().await.expect("live labby");
        let marker = guard.root.join("synthetic-session");
        std::fs::write(&marker, b"present").unwrap();
        let mut command = Command::new(env!("CARGO_BIN_EXE_live-harness-fixture"));
        command.args(["remove-marker", "0"]).arg(&marker);
        guard
            .register_credential_session("synthetic-session", command, vec![marker.clone()])
            .unwrap();
        assert!(
            guard
                .confirm_credential_session_revoked("synthetic-session")
                .unwrap_err()
                .contains("still has secret outputs")
        );
        assert!(
            guard
                .confirm_credential_session_revoked("another-session")
                .is_err()
        );
        let cleanup = guard.finish().await;
        assert!(cleanup.is_clean(), "{:?}", cleanup.failures);
        assert!(!marker.exists(), "revocation helper was not invoked");
    }

    #[test]
    fn credential_session_requires_absence_evidence_before_registration() {
        let error = NonEmptyAbsencePaths::try_from_paths(vec![]).unwrap_err();
        assert!(error.contains("requires absence evidence"), "{error}");
    }

    #[tokio::test]
    async fn exact_artifact_and_retained_evidence_bytes_are_canary_scanned() {
        let artifact_guard = LiveLabbyBuilder::new().start().await.expect("live labby");
        let canary = artifact_guard.secret_canaries[0].clone();
        std::fs::create_dir_all(artifact_guard.root.join("logs")).unwrap();
        std::fs::write(artifact_guard.root.join("logs/leaked.log"), &canary).unwrap();
        let artifact_cleanup = artifact_guard.finish().await;
        assert!(
            artifact_cleanup
                .failures
                .iter()
                .any(|failure| failure.contains("secret canary appeared"))
        );

        let mut evidence_guard = LiveLabbyBuilder::new().start().await.expect("live labby");
        let canary = evidence_guard.secret_canaries[0].clone();
        evidence_guard.evidence.push(EvidenceKind::Failure, &canary);
        let evidence_cleanup = evidence_guard.finish().await;
        assert!(
            evidence_cleanup
                .failures
                .iter()
                .any(|failure| failure.contains("secret canary appeared"))
        );
    }

    #[tokio::test]
    async fn ownership_nonce_and_bearer_never_escape_operator_safe_outputs() {
        let mut guard = LiveLabbyBuilder::new().start().await.expect("live labby");
        let nonce = guard.identity.nonce.clone();
        let bearer = guard.credential_canary.clone();
        assert!(guard.validate_ownership().is_ok());
        let ledger_bytes = std::fs::read(&guard.manifest_path).unwrap();
        assert!(
            ledger_bytes
                .windows(nonce.len())
                .any(|bytes| bytes == nonce.as_bytes())
        );

        let diagnostics = guard.diagnostics(None);
        assert!(!diagnostics.contains(&nonce));
        assert!(!diagnostics.contains(&bearer));
        let mut artifact_scan = Vec::new();
        scan_artifact_tree(&guard.root, &guard.secret_canaries, &mut artifact_scan);
        let rendered_scan = artifact_scan.join("\n");
        assert!(!rendered_scan.contains(&nonce));
        assert!(!rendered_scan.contains(&bearer));

        let retained = std::env::temp_dir()
            .join("labby-live-e2e-evidence")
            .join(format!("{}.json", guard.identity.run_id));
        let cleanup = guard.finish().await;
        assert!(cleanup.is_clean(), "{:?}", cleanup.failures);
        let retained_bytes = std::fs::read(retained).unwrap();
        assert!(
            !retained_bytes
                .windows(nonce.len())
                .any(|bytes| bytes == nonce.as_bytes())
        );
        assert!(
            !retained_bytes
                .windows(bearer.len())
                .any(|bytes| bytes == bearer.as_bytes())
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn retained_owned_root_is_an_explicit_cleanup_failure() {
        use std::os::unix::fs::PermissionsExt as _;

        let guard = LiveLabbyBuilder::new().start().await.expect("live labby");
        let root = guard.root.clone();
        std::fs::set_permissions(&root, std::fs::Permissions::from_mode(0o500)).unwrap();
        let cleanup = guard.finish().await;
        assert!(!cleanup.is_clean());
        assert!(
            cleanup
                .failures
                .iter()
                .any(|failure| failure.contains("owned root deletion failed")
                    || failure.contains("owned root retained"))
        );
        std::fs::set_permissions(&root, std::fs::Permissions::from_mode(0o700)).unwrap();
        std::fs::remove_dir_all(root).unwrap();
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn ownership_validation_rejects_nonce_symlink_swap() {
        use std::os::unix::fs::symlink;

        let mut guard = LiveLabbyBuilder::new().start().await.expect("live labby");
        let backup = guard.root.join("nonce.backup");
        std::fs::rename(&guard.nonce_path, &backup).unwrap();
        symlink(&backup, &guard.nonce_path).unwrap();
        assert!(guard.validate_ownership().unwrap_err().contains("symlink"));
        std::fs::remove_file(&guard.nonce_path).unwrap();
        std::fs::rename(backup, &guard.nonce_path).unwrap();
        let cleanup = guard.finish().await;
        assert!(cleanup.is_clean(), "{:?}", cleanup.failures);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn ownership_validation_rejects_root_symlink_and_control_character_manifest() {
        use std::os::unix::fs::symlink;
        let mut guard = LiveLabbyBuilder::new().start().await.expect("live labby");
        let moved = guard.root.with_extension("moved");
        std::fs::rename(&guard.root, &moved).unwrap();
        symlink(&moved, &guard.root).unwrap();
        assert!(guard.validate_ownership().unwrap_err().contains("symlink"));
        std::fs::remove_file(&guard.root).unwrap();
        std::fs::rename(&moved, &guard.root).unwrap();

        let original = guard.ledger.clone();
        guard
            .ledger
            .owned_roots
            .push(guard.root.join("unsafe\npath"));
        write_ledger(&guard.manifest_path, &guard.ledger).unwrap();
        assert!(
            guard
                .validate_ownership()
                .unwrap_err()
                .contains("control characters")
        );
        guard.ledger = original;
        write_ledger(&guard.manifest_path, &guard.ledger).unwrap();
        assert!(guard.finish().await.is_clean());
    }

    #[test]
    fn stale_sweep_removes_only_nonce_matched_dead_runs() {
        let parent = std::env::temp_dir().join("labby-live-e2e");
        std::fs::create_dir_all(&parent).unwrap();
        let stale = tempfile::Builder::new()
            .prefix("run-")
            .tempdir_in(&parent)
            .unwrap();
        let root = stale.keep();
        let nonce = "stale-owned-nonce";
        std::fs::write(root.join("ownership.nonce"), nonce).unwrap();
        write_ledger(
            &root.join("ownership.json"),
            &OwnershipLedger {
                generation: 1,
                created_at_ms: 0,
                nonce: nonce.into(),
                root: root.canonicalize().unwrap(),
                pid: Some(u32::MAX),
                owned_roots: vec![root.canonicalize().unwrap()],
                ..OwnershipLedger::default()
            },
        )
        .unwrap();
        let failures = sweep_stale_runs();
        assert!(
            failures
                .iter()
                .all(|failure| !failure.starts_with(&root.display().to_string()))
        );
        assert!(!root.exists());
    }

    #[tokio::test]
    async fn exhausted_cleanup_deadline_is_a_case_failure_but_still_kills_the_child() {
        let mut guard = LiveLabbyBuilder::new().start().await.expect("live labby");
        let cleanup = guard.finish_with_deadline(Duration::ZERO).await;
        assert!(!cleanup.is_clean(), "zero deadline must not silently pass");
    }

    #[tokio::test]
    #[allow(clippy::panic)]
    async fn outer_wrapper_cleans_on_timeout_and_drop_covers_panic_unwind() {
        let mut guard = LiveLabbyBuilder::new().start().await.expect("live labby");
        let timeout = guard
            .run_with_timeout(Duration::from_millis(10), std::future::pending::<()>())
            .await
            .expect_err("pending case times out");
        assert!(timeout.contains("timed out"));

        let guard = LiveLabbyBuilder::new().start().await.expect("live labby");
        let root = guard.root.clone();
        let unwind = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _owned = guard;
            panic!("injected case panic");
        }));
        assert!(unwind.is_err());
        assert!(!root.exists(), "panic Drop leaked owned root");

        let signal_contract = LiveLabbyGuard::finish_on_supported_signal;
        std::hint::black_box(signal_contract);
    }

    #[cfg(unix)]
    #[tokio::test]
    #[allow(clippy::panic)]
    async fn panic_drop_reaps_forked_grandchild_and_held_listener() {
        let mut guard = LiveLabbyBuilder::new().start().await.expect("live labby");
        guard
            .stop_process(Instant::now() + Duration::from_secs(5))
            .await
            .unwrap();
        let marker = guard.root.join("panic-grandchild.marker");
        let mut command = TokioCommand::new(env!("CARGO_BIN_EXE_live-harness-fixture"));
        command
            .env_clear()
            .args(["grandchild-listener", "0"])
            .arg(&marker)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .kill_on_drop(true);
        configure_process_group(&mut command);
        let child = command.spawn().unwrap();
        guard.ledger.pid = child.id();
        guard.ledger.guardian_pid = None;
        guard.ledger.daemon_pid = None;
        guard.ledger.daemon_process_start_identity = None;
        guard.guardian_admission = None;
        guard.ledger.process_start_identity = guard.ledger.pid.map(process_start_identity);
        guard.ledger.process_group = guard.ledger.pid.and_then(|pid| i32::try_from(pid).ok());
        write_ledger(&guard.manifest_path, &guard.ledger).unwrap();
        guard.child = Some(child);
        let address = wait_for_fixture_listener(&marker, Duration::from_secs(10)).await;
        guard.ledger.listener = Some(address);
        guard.ledger.listener_identity = Some(format!("tcp:{address}"));
        write_ledger(&guard.manifest_path, &guard.ledger).unwrap();
        let root = guard.root.clone();
        let unwind = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _guard = guard;
            panic!("panic with grandchild");
        }));
        assert!(unwind.is_err());
        assert!(!root.exists());
        assert!(
            TcpListener::bind(address).is_ok(),
            "grandchild retained listener"
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn ignored_termination_forces_owned_child_shutdown() {
        let mut guard = LiveLabbyBuilder::new().start().await.expect("live labby");
        guard
            .stop_process(Instant::now() + Duration::from_secs(5))
            .await
            .unwrap();
        let ready = guard.root.join("ignore-term.ready");
        let mut command = TokioCommand::new("/bin/sh");
        command
            .env_clear()
            .args([
                "-c",
                "trap '' TERM; : > \"$1\"; while :; do sleep 1; done",
                "ignore-term-fixture",
            ])
            .arg(&ready)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .kill_on_drop(true);
        configure_process_group(&mut command);
        let child = command.spawn().unwrap();
        guard.ledger.pid = child.id();
        guard.ledger.guardian_pid = None;
        guard.ledger.daemon_pid = None;
        guard.ledger.daemon_process_start_identity = None;
        guard.guardian_admission = None;
        guard.ledger.process_start_identity = guard.ledger.pid.map(process_start_identity);
        guard.ledger.process_group = guard.ledger.pid.and_then(|pid| i32::try_from(pid).ok());
        write_ledger(&guard.manifest_path, &guard.ledger).unwrap();
        guard.child = Some(child);
        // The full nextest workspace runs this shared support proof from several
        // integration binaries concurrently. Reserve enough bounded time for
        // the shell to be scheduled and install its signal trap on a loaded CI
        // host before exercising forced termination.
        let readiness_deadline = Instant::now() + Duration::from_secs(10);
        while !ready.exists() && Instant::now() < readiness_deadline {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        assert!(ready.exists(), "ignore-TERM fixture did not become ready");
        let started = Instant::now();
        let cleanup = guard.finish_with_deadline(Duration::from_millis(250)).await;
        assert!(cleanup.is_clean(), "{:?}", cleanup.failures);
        assert!(cleanup.forced, "ignored SIGTERM must use forced shutdown");
        // The process stop itself remains constrained by the 250 ms deadline;
        // the outer measurement also includes retained-evidence scans and
        // filesystem cleanup, which can be delayed by parallel test binaries.
        assert!(started.elapsed() < Duration::from_secs(8));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn forked_grandchild_and_held_listener_are_reaped_with_the_owned_group() {
        let mut guard = LiveLabbyBuilder::new().start().await.expect("live labby");
        guard
            .stop_process(Instant::now() + Duration::from_secs(5))
            .await
            .unwrap();
        let marker = guard.root.join("grandchild.marker");
        let mut command = TokioCommand::new(env!("CARGO_BIN_EXE_live-harness-fixture"));
        command
            .env_clear()
            .args(["grandchild-listener", "0"])
            .arg(&marker)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .kill_on_drop(true);
        configure_process_group(&mut command);
        let child = command.spawn().unwrap();
        guard.ledger.pid = child.id();
        guard.ledger.guardian_pid = None;
        guard.ledger.daemon_pid = None;
        guard.ledger.daemon_process_start_identity = None;
        guard.guardian_admission = None;
        guard.ledger.process_start_identity = guard.ledger.pid.map(process_start_identity);
        guard.ledger.process_group = guard.ledger.pid.and_then(|pid| i32::try_from(pid).ok());
        write_ledger(&guard.manifest_path, &guard.ledger).unwrap();
        guard.child = Some(child);
        let address = wait_for_fixture_listener(&marker, Duration::from_secs(10)).await;
        guard.ledger.listener = Some(address);
        guard.ledger.listener_identity = Some(format!("tcp:{address}"));
        write_ledger(&guard.manifest_path, &guard.ledger).unwrap();
        assert!(
            TcpListener::bind(address).is_err(),
            "fixture must hold listener"
        );
        let cleanup = guard.finish_with_deadline(Duration::from_secs(3)).await;
        assert!(cleanup.is_clean(), "{:?}", cleanup.failures);
        assert!(
            TcpListener::bind(address).is_ok(),
            "grandchild listener leaked"
        );
    }

    #[cfg(unix)]
    async fn wait_for_fixture_listener(marker: &Path, timeout: Duration) -> SocketAddr {
        let deadline = Instant::now() + timeout;
        loop {
            if let Ok(value) = std::fs::read_to_string(marker) {
                let fields = value.split_whitespace().collect::<Vec<_>>();
                if fields.len() == 3
                    && fields[2] == "ready"
                    && let Ok(port) = fields[1].parse::<u16>()
                {
                    return SocketAddr::from(([127, 0, 0, 1], port));
                }
            }
            assert!(
                Instant::now() < deadline,
                "grandchild listener did not become ready"
            );
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    }
}
