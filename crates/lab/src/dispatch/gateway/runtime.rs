#[cfg(target_os = "linux")]
use std::collections::BTreeMap;
use std::collections::BTreeSet;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

#[cfg(unix)]
use nix::errno::Errno;
use tokio::sync::RwLock;

use crate::config::{LabConfig, UpstreamConfig};
use crate::dispatch::error::ToolError;
use crate::dispatch::gateway::manager::GatewayManager;
use crate::dispatch::gateway::projection::{redacted_gateway_target, upstream_summary};
use crate::dispatch::upstream::pool::UpstreamPool;
use crate::dispatch::upstream::types::UpstreamRuntimeOwner;
#[cfg(all(unix, target_os = "linux"))]
use crate::process::unix::terminate_process_group_sigkill;
#[cfg(unix)]
use crate::process::unix::{pid_is_alive, terminate_sigkill};
#[cfg(target_os = "linux")]
use crate::process::unix::{process_group_id, process_has_ancestor, read_cmdline};

#[derive(Clone, Default)]
pub struct GatewayRuntimeHandle {
    pool: Arc<RwLock<Option<Arc<UpstreamPool>>>>,
}

impl GatewayRuntimeHandle {
    pub async fn current_pool(&self) -> Option<Arc<UpstreamPool>> {
        self.pool.read().await.clone()
    }

    pub async fn swap(&self, pool: Option<Arc<UpstreamPool>>) {
        *self.pool.write().await = pool;
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub(super) struct PersistedGatewayRuntimeState {
    #[serde(default)]
    reconciled_at_epoch_secs: Option<u64>,
    #[serde(default)]
    entries: Vec<PersistedGatewayRuntimeEntry>,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub(super) struct PersistedGatewayRuntimeEntry {
    upstream: String,
    pid: u32,
    #[serde(default)]
    pgid: Option<u32>,
    #[serde(default)]
    started_at_epoch_secs: Option<u64>,
    #[serde(default)]
    observed_at_epoch_secs: u64,
    #[serde(default)]
    origin: Option<String>,
    #[serde(default)]
    owner: Option<crate::dispatch::gateway::types::GatewayRuntimeOwnerView>,
    #[serde(default)]
    transport: Option<String>,
    #[serde(default)]
    target: Option<String>,
}

impl GatewayManager {
    fn runtime_state_path(&self) -> PathBuf {
        let parent = self
            .path
            .parent()
            .map(ToOwned::to_owned)
            .unwrap_or_else(|| PathBuf::from("."));
        let stem = self
            .path
            .file_stem()
            .and_then(|value| value.to_str())
            .unwrap_or("gateway");
        parent.join(format!("{stem}.runtime.json"))
    }

    async fn load_runtime_state(&self) -> PersistedGatewayRuntimeState {
        let path = self.runtime_state_path();
        let Ok(raw) = tokio::fs::read_to_string(path).await else {
            return PersistedGatewayRuntimeState::default();
        };
        serde_json::from_str(&raw).unwrap_or_default()
    }

    async fn persist_runtime_state(
        &self,
        state: &PersistedGatewayRuntimeState,
    ) -> Result<(), ToolError> {
        let path = self.runtime_state_path();
        if let Some(parent) = path.parent() {
            tokio::fs::create_dir_all(parent).await.map_err(|error| {
                ToolError::internal_message(format!(
                    "failed to create runtime state directory {}: {error}",
                    parent.display()
                ))
            })?;
        }
        let body = serde_json::to_vec_pretty(state).map_err(|error| {
            ToolError::internal_message(format!("failed to serialize runtime state: {error}"))
        })?;
        tokio::fs::write(&path, body).await.map_err(|error| {
            ToolError::internal_message(format!(
                "failed to write runtime state {}: {error}",
                path.display()
            ))
        })
    }

    pub(super) async fn reconcile_runtime_state(
        &self,
        cfg: &LabConfig,
        pool: Option<&UpstreamPool>,
    ) -> Result<PersistedGatewayRuntimeState, ToolError> {
        let mut state = self.load_runtime_state().await;
        state
            .entries
            .retain(persisted_runtime_process_still_matches);

        if let Some(pool) = pool {
            for upstream in &cfg.upstream {
                if let Some(runtime) = pool.upstream_runtime_metadata(&upstream.name).await
                    && let Some(pid) = runtime.pid
                {
                    state
                        .entries
                        .retain(|entry| !(entry.upstream == upstream.name && entry.pid == pid));
                    state.entries.push(PersistedGatewayRuntimeEntry {
                        upstream: upstream.name.clone(),
                        pid,
                        pgid: runtime.pgid,
                        started_at_epoch_secs: runtime
                            .started_at
                            .and_then(system_time_to_epoch_secs),
                        observed_at_epoch_secs: epoch_now_secs(),
                        origin: runtime.origin.clone(),
                        owner: runtime.owner.as_ref().map(runtime_owner_view),
                        transport: Some(if upstream.command.is_some() {
                            "stdio".to_string()
                        } else {
                            "http".to_string()
                        }),
                        target: redacted_gateway_target(upstream),
                    });
                }
            }
        }

        state.reconciled_at_epoch_secs = Some(epoch_now_secs());
        state.entries.sort_by(|left, right| {
            left.upstream
                .cmp(&right.upstream)
                .then(left.pid.cmp(&right.pid))
        });
        self.persist_runtime_state(&state).await?;
        Ok(state)
    }

    pub async fn mcp_runtime_list(
        &self,
    ) -> Result<Vec<super::types::GatewayMcpRuntimeView>, ToolError> {
        let cfg = self.config.read().await.clone();
        let pool = self.runtime.current_pool().await;
        let persisted = self.reconcile_runtime_state(&cfg, pool.as_deref()).await?;
        let mut rows = Vec::with_capacity(cfg.upstream.len());
        for upstream in &cfg.upstream {
            let summary = upstream_summary(pool.as_deref(), &upstream.name).await;
            let runtime = match pool.as_deref() {
                Some(pool) => pool.upstream_runtime_metadata(&upstream.name).await,
                None => None,
            };
            let live_pid = runtime.as_ref().and_then(|meta| meta.pid);
            let persisted_rows: Vec<&PersistedGatewayRuntimeEntry> = persisted
                .entries
                .iter()
                .filter(|entry| entry.upstream == upstream.name)
                .collect();
            let persisted_stale_count = persisted_rows
                .iter()
                .filter(|entry| Some(entry.pid) != live_pid)
                .count();
            let live_stale_count = if upstream.command.is_some() {
                likely_stale_process_group_count(
                    upstream,
                    live_pid.zip(runtime.as_ref().and_then(|meta| meta.pgid)),
                )
            } else {
                0
            };
            let stale_count = persisted_stale_count.max(live_stale_count);
            let fallback = if let Some(pid) = live_pid {
                persisted_rows.into_iter().find(|entry| entry.pid == pid)
            } else {
                persisted_rows.into_iter().max_by_key(|entry| {
                    entry
                        .started_at_epoch_secs
                        .unwrap_or(entry.observed_at_epoch_secs)
                })
            };
            let connected = upstream.enabled
                && (summary.exposed_tool_count > 0
                    || summary.exposed_resource_count > 0
                    || summary.exposed_prompt_count > 0);
            rows.push(super::types::GatewayMcpRuntimeView {
                name: upstream.name.clone(),
                enabled: upstream.enabled,
                connected,
                discovered_tool_count: summary.discovered_tool_count,
                exposed_tool_count: summary.exposed_tool_count,
                discovered_resource_count: summary.discovered_resource_count,
                exposed_resource_count: summary.exposed_resource_count,
                discovered_prompt_count: summary.discovered_prompt_count,
                exposed_prompt_count: summary.exposed_prompt_count,
                likely_stale_count: stale_count,
                pid: live_pid.or_else(|| fallback.map(|entry| entry.pid)),
                pgid: runtime
                    .as_ref()
                    .and_then(|meta| meta.pgid)
                    .or_else(|| fallback.and_then(|entry| entry.pgid)),
                age_seconds: runtime
                    .as_ref()
                    .and_then(|meta| meta.started_at)
                    .and_then(|started_at| {
                        std::time::SystemTime::now().duration_since(started_at).ok()
                    })
                    .map(|elapsed: Duration| elapsed.as_secs())
                    .or_else(|| {
                        fallback
                            .and_then(|entry| entry.started_at_epoch_secs)
                            .and_then(age_from_epoch_secs)
                    }),
                origin: runtime
                    .as_ref()
                    .and_then(|meta| meta.origin.clone())
                    .or_else(|| fallback.and_then(|entry| entry.origin.clone())),
                owner: runtime
                    .as_ref()
                    .and_then(|meta| meta.owner.as_ref().map(runtime_owner_view))
                    .or_else(|| fallback.and_then(|entry| entry.owner.clone())),
                transport: Some(if upstream.command.is_some() {
                    "stdio".to_string()
                } else {
                    "http".to_string()
                }),
                target: fallback
                    .and_then(|entry| entry.target.clone())
                    .or_else(|| redacted_gateway_target(upstream)),
                runtime_state_path: Some(self.runtime_state_path().display().to_string()),
                reconciled_at: persisted
                    .reconciled_at_epoch_secs
                    .and_then(epoch_secs_to_rfc3339),
            });
        }
        Ok(rows)
    }

    pub async fn cleanup_upstream_processes(
        &self,
        name: &str,
        aggressive: bool,
        dry_run: bool,
    ) -> Result<super::types::GatewayCleanupView, ToolError> {
        let upstream = self
            .config
            .read()
            .await
            .upstream
            .iter()
            .find(|existing| existing.name == name)
            .cloned()
            .ok_or_else(|| ToolError::Sdk {
                sdk_kind: "not_found".to_string(),
                message: format!("gateway `{name}` not found"),
            })?;

        let gateway_patterns = upstream_cleanup_patterns(&upstream, false);
        let local_patterns = local_cleanup_patterns();
        let aggressive_patterns = if aggressive {
            upstream_cleanup_patterns(&upstream, true)
        } else {
            Vec::new()
        };

        let gateway_matches = matching_processes(&gateway_patterns);
        let local_matches = matching_processes(&local_patterns);
        let aggressive_matches = if aggressive {
            matching_processes(&aggressive_patterns)
        } else {
            Vec::new()
        };

        let view = super::types::GatewayCleanupView {
            upstream: upstream.name,
            aggressive,
            dry_run,
            gateway_matched: count_matched_processes(&gateway_matches),
            local_matched: count_matched_processes(&local_matches),
            aggressive_matched: if aggressive {
                count_matched_processes(&aggressive_matches)
            } else {
                0
            },
            gateway_killed: if dry_run {
                count_matched_processes(&gateway_matches)
            } else {
                kill_matched_processes(&gateway_matches)
            },
            local_killed: if dry_run {
                count_matched_processes(&local_matches)
            } else {
                kill_matched_processes(&local_matches)
            },
            aggressive_killed: if aggressive {
                if dry_run {
                    count_matched_processes(&aggressive_matches)
                } else {
                    kill_matched_processes(&aggressive_matches)
                }
            } else {
                0
            },
            gateway_matches: gateway_matches.iter().map(cleanup_match_view).collect(),
            local_matches: local_matches.iter().map(cleanup_match_view).collect(),
            aggressive_matches: aggressive_matches.iter().map(cleanup_match_view).collect(),
        };

        let cfg = self.config.read().await.clone();
        let current_pool = self.runtime.current_pool().await;
        self.reconcile_runtime_state(&cfg, current_pool.as_deref())
            .await?;

        Ok(view)
    }
}

fn local_cleanup_patterns() -> Vec<String> {
    vec![
        "labby mcp".to_string(),
        "target/debug/labby mcp".to_string(),
    ]
}

#[cfg(target_os = "linux")]
fn likely_stale_process_group_count(
    upstream: &UpstreamConfig,
    live_runtime: Option<(u32, u32)>,
) -> usize {
    let patterns = upstream_cleanup_patterns(upstream, false);
    let mut groups = BTreeSet::new();
    for matched in matching_processes(&patterns) {
        for pid in matched.pids {
            let group = process_group_id(pid).unwrap_or(pid);
            if live_runtime.is_some_and(|(live_pid, live_pgid)| {
                group == live_pgid || process_has_ancestor(pid, live_pid)
            }) {
                continue;
            }
            groups.insert(group);
        }
    }
    groups.len()
}

#[cfg(not(target_os = "linux"))]
fn likely_stale_process_group_count(
    _upstream: &UpstreamConfig,
    _live_runtime: Option<(u32, u32)>,
) -> usize {
    0
}

pub(super) fn upstream_cleanup_patterns(
    upstream: &UpstreamConfig,
    aggressive: bool,
) -> Vec<String> {
    let mut patterns = Vec::new();
    let command = upstream.command.as_deref().unwrap_or("");
    let joined_args = upstream.args.join(" ");
    let joined = if command.is_empty() {
        joined_args.clone()
    } else if joined_args.is_empty() {
        command.to_string()
    } else {
        format!("{command} {joined_args}")
    };
    if let Some(command) = upstream.command.as_deref() {
        let mut joined = command.to_string();
        for arg in &upstream.args {
            joined.push(' ');
            joined.push_str(arg);
        }
        patterns.push(joined);
        for arg in &upstream.args {
            if arg.contains(&upstream.name) {
                patterns.push(arg.clone());
            }
        }
    }
    if joined.contains("chrome-devtools-mcp") || upstream.name.contains("chrome-devtools") {
        patterns.push("chrome-devtools-mcp".to_string());
        patterns.push("chrome-devtools".to_string());
        patterns.push("chrome-devtools-mcp/build/src/telemetry/watchdog/main.js".to_string());
        patterns.push("npm exec chrome-devtools-mcp@latest".to_string());
    }
    if joined.contains("github-chat-mcp") || upstream.name.contains("github-chat") {
        patterns.push("github-chat-mcp".to_string());
        patterns.push("uvx github-chat-mcp".to_string());
        patterns.push("uv tool uvx github-chat-mcp".to_string());
        patterns.push("uv run github-chat-mcp".to_string());
        patterns.push("github-chat".to_string());
        patterns.push("/github-chat-mcp".to_string());
    }
    if aggressive {
        patterns.push(upstream.name.clone());
    }
    patterns.sort();
    patterns.dedup();
    patterns
}

fn epoch_now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_secs())
        .unwrap_or(0)
}

fn system_time_to_epoch_secs(time: std::time::SystemTime) -> Option<u64> {
    time.duration_since(std::time::UNIX_EPOCH)
        .ok()
        .map(|elapsed| elapsed.as_secs())
}

fn age_from_epoch_secs(epoch_secs: u64) -> Option<u64> {
    let started_at = std::time::UNIX_EPOCH.checked_add(Duration::from_secs(epoch_secs))?;
    std::time::SystemTime::now()
        .duration_since(started_at)
        .ok()
        .map(|elapsed| elapsed.as_secs())
}

fn epoch_secs_to_rfc3339(epoch_secs: u64) -> Option<String> {
    let seconds = i64::try_from(epoch_secs).ok()?;
    let timestamp = jiff::Timestamp::from_second(seconds).ok()?;
    Some(timestamp.to_string())
}

pub(super) fn runtime_origin_tag(origin: Option<&str>) -> Option<String> {
    origin
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
}

fn runtime_owner_view(
    owner: &UpstreamRuntimeOwner,
) -> crate::dispatch::gateway::types::GatewayRuntimeOwnerView {
    crate::dispatch::gateway::types::GatewayRuntimeOwnerView {
        surface: owner.surface.clone(),
        subject: owner.subject.clone(),
        request_id: owner.request_id.clone(),
        session_id: owner.session_id.clone(),
        client_name: owner.client_name.clone(),
        raw: owner.raw.clone(),
    }
}

#[cfg(unix)]
fn process_is_alive(pid: u32) -> bool {
    pid_is_alive(pid)
}

#[cfg(not(unix))]
fn process_is_alive(_pid: u32) -> bool {
    false
}

#[cfg(target_os = "linux")]
fn persisted_runtime_process_still_matches(entry: &PersistedGatewayRuntimeEntry) -> bool {
    if !process_is_alive(entry.pid) {
        return false;
    }
    entry
        .pgid
        .is_none_or(|stored_pgid| process_group_id(entry.pid) == Some(stored_pgid))
}

#[cfg(not(target_os = "linux"))]
fn persisted_runtime_process_still_matches(entry: &PersistedGatewayRuntimeEntry) -> bool {
    process_is_alive(entry.pid)
}

#[derive(Debug, Clone, Default)]
struct GatewayCleanupMatch {
    pattern: String,
    pids: Vec<u32>,
}

fn cleanup_match_view(matched: &GatewayCleanupMatch) -> super::types::GatewayCleanupMatchView {
    super::types::GatewayCleanupMatchView {
        pattern: matched.pattern.clone(),
        pids: matched.pids.clone(),
    }
}

#[cfg(target_os = "linux")]
fn current_and_parent_pids() -> std::collections::HashSet<u32> {
    let mut pids = std::collections::HashSet::from([std::process::id()]);
    let parent = nix::unistd::getppid();
    if parent.as_raw() > 0 {
        pids.insert(parent.as_raw() as u32);
    }
    pids
}

#[cfg(target_os = "linux")]
fn matching_processes(patterns: &[String]) -> Vec<GatewayCleanupMatch> {
    let excluded_pids = current_and_parent_pids();
    let mut matched: BTreeMap<String, BTreeSet<u32>> = BTreeMap::new();
    for entry in std::fs::read_dir("/proc")
        .ok()
        .into_iter()
        .flatten()
        .flatten()
    {
        let pid_str = entry.file_name();
        let Ok(pid) = pid_str.to_string_lossy().parse::<u32>() else {
            continue;
        };
        if excluded_pids.contains(&pid) {
            continue;
        }
        let Some(cmdline) = read_cmdline(pid) else {
            continue;
        };
        for pattern in patterns.iter().filter(|pattern| !pattern.trim().is_empty()) {
            if cmdline.contains(pattern) {
                matched.entry(pattern.clone()).or_default().insert(pid);
            }
        }
    }
    matched
        .into_iter()
        .map(|(pattern, pids)| GatewayCleanupMatch {
            pattern,
            pids: pids.into_iter().collect(),
        })
        .collect()
}

#[cfg(not(target_os = "linux"))]
fn matching_processes(_patterns: &[String]) -> Vec<GatewayCleanupMatch> {
    Vec::new()
}

#[cfg(all(test, target_os = "linux"))]
pub(super) fn process_matches_patterns(cmdline: &str, patterns: &[String]) -> bool {
    patterns
        .iter()
        .filter(|pattern| !pattern.trim().is_empty())
        .any(|pattern| cmdline.contains(pattern))
}

fn count_matched_processes(matches: &[GatewayCleanupMatch]) -> usize {
    let mut unique = BTreeSet::new();
    for matched in matches {
        unique.extend(matched.pids.iter().copied());
    }
    unique.len()
}

fn kill_matched_processes(matches: &[GatewayCleanupMatch]) -> usize {
    let mut unique_pids = BTreeSet::new();
    let mut unique_groups = BTreeSet::new();
    for matched in matches {
        for pid in matched.pids.iter().copied() {
            let target = process_kill_target(pid);
            match target {
                ProcessKillTarget::Pid(pid) => {
                    unique_pids.insert(pid);
                }
                ProcessKillTarget::Group(pgid) => {
                    unique_groups.insert(pgid);
                }
            }
        }
    }
    for pgid in &unique_groups {
        let _ = terminate_process_group(*pgid);
    }
    for pid in &unique_pids {
        let _ = terminate_process(*pid);
    }
    unique_groups.len() + unique_pids.len()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum ProcessKillTarget {
    Pid(u32),
    // constructed only on Linux; match arm retained for exhaustiveness on all platforms
    #[allow(dead_code)]
    Group(u32),
}

#[cfg(target_os = "linux")]
fn process_kill_target(pid: u32) -> ProcessKillTarget {
    let pgid = process_group_id(pid).unwrap_or(pid);
    let current_pgid = process_group_id(std::process::id());
    if Some(pgid) == current_pgid {
        ProcessKillTarget::Pid(pid)
    } else {
        ProcessKillTarget::Group(pgid)
    }
}

#[cfg(not(target_os = "linux"))]
fn process_kill_target(pid: u32) -> ProcessKillTarget {
    ProcessKillTarget::Pid(pid)
}

#[cfg(all(unix, target_os = "linux"))]
fn terminate_process_group(pgid: u32) -> Result<(), Errno> {
    terminate_process_group_sigkill(pgid)
}

#[cfg(all(unix, not(target_os = "linux")))]
fn terminate_process_group(pgid: u32) -> Result<(), Errno> {
    terminate_process(pgid)
}

#[cfg(unix)]
fn terminate_process(pid: u32) -> Result<(), Errno> {
    terminate_sigkill(pid)
}

#[cfg(not(unix))]
fn terminate_process(_pid: u32) -> Result<(), ()> {
    Ok(())
}

#[cfg(not(unix))]
fn terminate_process_group(_pid: u32) -> Result<(), ()> {
    Ok(())
}
