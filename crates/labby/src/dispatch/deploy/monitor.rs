//! Host-reachability monitor for deployed SSH targets.
//!
//! Probes each target by attempting a TCP connection to its SSH port with a
//! short timeout. Emits one `HostStatusEvent` JSON line to stdout whenever a
//! host transitions between `online` and `offline`, plus an initial snapshot
//! line for every host on startup.
//!
//! Single-instance: refuses to start if another `labby deploy monitor` is already
//! running, using a pidfile at `~/.labby/run/deploy-monitor.lock`. Stale pidfiles
//! (process no longer alive) are silently overwritten.
//!
//! Suitable as input for Claude Code's Monitor tool (reads stdout line by line).

use std::collections::HashMap;
use std::io::Write as _;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result, bail};
#[cfg(unix)]
use nix::sys::signal::kill;
#[cfg(unix)]
use nix::unistd::Pid;
use tokio::net::TcpStream;
use tokio::signal;

use labby_apis::deploy::{HostStatus, HostStatusEvent};

use super::runner::DefaultRunner;
use super::ssh_session::SshHostTarget;

/// Probe a single host by attempting a TCP connect to its SSH port.
async fn probe_host(target: &SshHostTarget, timeout: Duration) -> (HostStatus, String) {
    let host = target.hostname.as_deref().unwrap_or(target.alias.as_str());
    let port = target.port.unwrap_or(22);
    let addr = format!("{host}:{port}");

    let status = match tokio::time::timeout(timeout, TcpStream::connect(&addr)).await {
        Ok(Ok(_)) => HostStatus::Online,
        _ => HostStatus::Offline,
    };
    (status, addr)
}

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

#[allow(clippy::print_stdout)] // CLI watch streams NDJSON events to stdout by design.
fn emit(event: &HostStatusEvent) {
    let line = serde_json::to_string(event).unwrap_or_default();
    println!("{line}");
    drop(std::io::stdout().flush());
}

/// Pidfile path used to enforce single-instance.
fn lock_path() -> PathBuf {
    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."));
    home.join(".labby/run/deploy-monitor.lock")
}

/// Returns true if a process with the given PID is alive.
///
/// Uses `kill(pid, 0)` semantics on Unix: signal 0 doesn't actually send a
/// signal, it just probes whether the process exists and we have permission
/// to signal it. `ESRCH` means the PID is gone (stale pidfile).
///
/// On non-Unix platforms there is no equivalent stdlib check without pulling
/// in a Windows-specific dependency. Conservatively assume the process is
/// alive — the lock-acquisition error message already tells the user how to
/// recover from a stale pidfile (delete the file manually).
#[cfg(unix)]
fn pid_alive(pid: i32) -> bool {
    matches!(kill(Pid::from_raw(pid), None), Ok(()))
}

#[cfg(not(unix))]
fn pid_alive(_pid: i32) -> bool {
    true
}

/// RAII guard for the single-instance pidfile. Removes the file on drop.
struct LockGuard {
    path: PathBuf,
}

impl LockGuard {
    fn acquire(path: PathBuf) -> Result<Self> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("create lock directory {}", parent.display()))?;
        }

        if let Ok(existing) = std::fs::read_to_string(&path)
            && let Ok(pid) = existing.trim().parse::<i32>()
            && pid_alive(pid)
        {
            bail!(
                "another `labby deploy monitor` is already running (pid {pid}); \
                 stop it or delete {} if you're sure it's stale",
                path.display()
            );
        }

        std::fs::write(&path, std::process::id().to_string())
            .with_context(|| format!("write lock file {}", path.display()))?;
        Ok(Self { path })
    }
}

impl Drop for LockGuard {
    fn drop(&mut self) {
        drop(std::fs::remove_file(&self.path));
    }
}

/// Watch the given hosts, emitting JSON state-change events to stdout.
///
/// Holds a single-instance pidfile lock; returns an error if another
/// `labby deploy monitor` is already running. Runs until Ctrl-C is received.
pub async fn watch_hosts(
    runner: &DefaultRunner,
    targets: Vec<String>,
    interval: Duration,
    probe_timeout: Duration,
) -> Result<()> {
    let _lock = LockGuard::acquire(lock_path())?;
    watch_hosts_inner(runner, targets, interval, probe_timeout).await;
    Ok(())
}

async fn watch_hosts_inner(
    runner: &DefaultRunner,
    targets: Vec<String>,
    interval: Duration,
    probe_timeout: Duration,
) {
    let targets: Arc<Vec<SshHostTarget>> = Arc::new(
        targets
            .iter()
            .filter_map(|alias| runner.resolve_target(alias).cloned())
            .collect(),
    );

    // Initial state: assume all hosts offline until first probe.
    let mut states: HashMap<String, HostStatus> = targets
        .iter()
        .map(|t| (t.alias.clone(), HostStatus::Offline))
        .collect();

    // Initial snapshot — probe all hosts once before entering the loop.
    for target in targets.iter() {
        let (status, addr) = probe_host(target, probe_timeout).await;
        states.insert(target.alias.clone(), status);
        emit(&HostStatusEvent {
            ts: now_secs(),
            host: target.alias.clone(),
            status,
            addr,
        });
    }

    let mut ticker = tokio::time::interval(interval);
    ticker.tick().await; // consume the immediate first tick

    loop {
        tokio::select! {
            _ = ticker.tick() => {
                for target in targets.iter() {
                    let (new_status, addr) = probe_host(target, probe_timeout).await;
                    let prev = states.get(&target.alias).copied().unwrap_or(HostStatus::Offline);
                    if new_status != prev {
                        states.insert(target.alias.clone(), new_status);
                        emit(&HostStatusEvent {
                            ts: now_secs(),
                            host: target.alias.clone(),
                            status: new_status,
                            addr,
                        });
                    }
                }
            }
            _ = signal::ctrl_c() => {
                break;
            }
        }
    }
}
