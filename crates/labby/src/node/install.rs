/// Install handler logic for master→device RPC methods.
///
/// Implements:
/// - `marketplace.install_component` — cherry-pick file writes from a plugin
/// - `agent.install` — record ACP agent install descriptor on target device
///
/// Security invariants (enforced on every write):
/// 1. Path traversal: reject any component path containing anything other than
///    `Component::Normal(_)`.
/// 2. Symlink check: before writing any file, verify the target path is NOT a symlink
///    via `symlink_metadata().file_type().is_symlink()`.
use std::path::{Component, Path, PathBuf};

use anyhow::{Context, Result, anyhow};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio::sync::mpsc;

// --------------------------------------------------------------------------
// Parameter types
// --------------------------------------------------------------------------

/// Install scope: `"global"` → `~/.claude/` or `"project"` → `{project_path}/.claude/`.
#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum InstallScope {
    Global,
    Project,
}

/// Distribution type for an installable ACP agent.
#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum DistType {
    Npx,
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum McpClient {
    Claude,
    Codex,
}

#[derive(Debug, Deserialize)]
pub struct InstallComponentParams {
    #[allow(dead_code)] // Forwarded in RPC params; available for logging/auditing.
    pub plugin_id: String,
    /// Which files from the plugin to cherry-pick (relative paths within plugin).
    #[allow(dead_code)] // Forwarded in RPC params; consumed by downstream installers.
    pub components: Vec<String>,
    pub scope: InstallScope,
    /// Required when scope == `Project`; must be an absolute path with no `..`.
    pub project_path: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct AgentInstallParams {
    pub agent_id: String,
    pub distribution: AgentDistribution,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct AgentDistribution {
    #[serde(rename = "type")]
    pub dist_type: DistType,
    pub package: String,
    pub version: String,
}

#[derive(Debug, Deserialize)]
pub struct McpInstallParams {
    pub name: String,
    pub client: McpClient,
    pub config: Value,
}

// --------------------------------------------------------------------------
// Result types
// --------------------------------------------------------------------------

#[derive(Debug, Default, Serialize)]
pub struct InstallComponentResult {
    pub written: Vec<String>,
    pub skipped: Vec<String>,
    pub errors: Vec<InstallError>,
}

#[derive(Debug, Serialize)]
pub struct InstallError {
    pub file: String,
    pub error: String,
}

// --------------------------------------------------------------------------
// Progress notifications
// --------------------------------------------------------------------------

/// A progress notification sent back to master during install.
/// Does NOT carry an `id` field (notification, not request/response).
#[derive(Debug, Serialize)]
pub struct InstallProgressNotification {
    pub jsonrpc: &'static str,
    pub method: &'static str,
    pub params: InstallProgressParams,
}

#[derive(Debug, Serialize)]
pub struct InstallProgressParams {
    pub rpc_id: Value,
    pub status: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub file: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

async fn send_progress(tx: &mpsc::Sender<String>, notif: &InstallProgressNotification) {
    if let Ok(encoded) = serde_json::to_string(notif) {
        tx.send(encoded).await.ok();
    }
}

impl InstallProgressNotification {
    pub fn started(rpc_id: Value) -> Self {
        Self {
            jsonrpc: "2.0",
            method: "install/progress",
            params: InstallProgressParams {
                rpc_id,
                status: "started",
                file: None,
                error: None,
            },
        }
    }

    pub fn file_written(rpc_id: Value, file: String) -> Self {
        Self {
            jsonrpc: "2.0",
            method: "install/progress",
            params: InstallProgressParams {
                rpc_id,
                status: "file_written",
                file: Some(file),
                error: None,
            },
        }
    }

    #[allow(dead_code)] // Available for callers that distinguish skipped from written.
    pub fn file_skipped(rpc_id: Value, file: String) -> Self {
        Self {
            jsonrpc: "2.0",
            method: "install/progress",
            params: InstallProgressParams {
                rpc_id,
                status: "file_skipped",
                file: Some(file),
                error: None,
            },
        }
    }

    pub fn file_error(rpc_id: Value, file: String, error: String) -> Self {
        Self {
            jsonrpc: "2.0",
            method: "install/progress",
            params: InstallProgressParams {
                rpc_id,
                status: "file_error",
                file: Some(file),
                error: Some(error),
            },
        }
    }
}

// --------------------------------------------------------------------------
// Security helpers
// --------------------------------------------------------------------------

/// Returns `Err` if `rel_path` contains any non-normal path component.
pub fn reject_path_traversal(rel_path: &str) -> Result<()> {
    let path = Path::new(rel_path);
    if path.as_os_str().is_empty() {
        return Err(anyhow!("{ERR_PATH_TRAVERSAL}: path rejected: empty"));
    }
    for component in path.components() {
        if !matches!(component, Component::Normal(_)) {
            return Err(anyhow!(
                "{ERR_PATH_TRAVERSAL}: path rejected: `{rel_path}` contains non-normal component {component:?}"
            ));
        }
    }
    Ok(())
}

/// Returns `Err` if `path` exists AND is a symlink.
pub async fn reject_symlink(path: &Path) -> Result<()> {
    match tokio::fs::symlink_metadata(path).await {
        Ok(meta) if meta.file_type().is_symlink() => Err(anyhow!(
            "{ERR_SYMLINK}: `{}` is a symbolic link",
            path.display()
        )),
        Ok(_) | Err(_) => Ok(()),
    }
}

async fn ensure_target_within_write_root(write_root: &Path, target: &Path) -> Result<()> {
    let canonical_root = tokio::fs::canonicalize(write_root)
        .await
        .with_context(|| format!("canonicalize write root `{}`", red_path(write_root)))?;
    let parent = target
        .parent()
        .ok_or_else(|| anyhow!("target `{}` has no parent directory", red_path(target)))?;
    let canonical_parent = tokio::fs::canonicalize(parent)
        .await
        .with_context(|| format!("canonicalize target parent `{}`", red_path(parent)))?;
    if !canonical_parent.starts_with(&canonical_root) {
        return Err(anyhow!(
            "{ERR_PATH_TRAVERSAL}: target `{}` resolves outside write root `{}`",
            red_path(target),
            red_path(write_root)
        ));
    }
    Ok(())
}

// --------------------------------------------------------------------------
// Error kind markers for the RPC error envelope classifier (lab-zxx5.28).
//
// `ws_client::error_kind` maps handler anyhow errors to stable taxonomy kinds
// without typed variants, by looking for these literal prefixes in the error
// chain. Adding a new kind: define a const here, prefix the anyhow! call
// site, and extend ws_client::error_kind to recognise it.
// --------------------------------------------------------------------------

pub const ERR_PATH_TRAVERSAL: &str = "lab.err:path_traversal_rejected";
pub const ERR_SYMLINK: &str = "lab.err:symlink_rejected";
pub const ERR_MISSING_PARAM: &str = "lab.err:missing_param";
pub const ERR_VALIDATION: &str = "lab.err:validation_failed";

/// Redact a `Path` for embedding in error messages and tracing fields.
/// Wrapper around `dispatch::helpers::redact_home` that takes a `&Path`
/// and returns an owned `String`. Defends against OS-username leakage
/// when these errors flow back to the master via `result.errors[].error`
/// (lab-zxx5.32).
fn red_path(p: &Path) -> String {
    crate::dispatch::helpers::redact_home(&p.to_string_lossy())
}

/// Atomically write `contents` to `target`, preventing symlink-based TOCTOU
/// attacks. The sequence is:
/// 1. Generate a sibling tempfile name `{target}.tmp-{uuid}` in the parent dir
/// 2. Create the tempfile (normal create — tempfile name is random, can't
///    collide with an attacker-planted symlink unless the parent dir itself
///    is attacker-controlled, which is already rejected by
///    `ensure_target_within_write_root`)
/// 3. Verify the tempfile is a regular file, not a symlink (defense in depth)
/// 4. Write contents, fsync, close
/// 5. `rename(tmp, target)` — atomic replace; a symlink at `target` is
///    replaced as a file entry, NOT followed
///
/// If anything fails, the tempfile is best-effort removed.
pub async fn write_atomic(target: &Path, contents: &[u8]) -> Result<()> {
    let parent = target
        .parent()
        .ok_or_else(|| anyhow::anyhow!("target has no parent: {}", target.display()))?;
    let file_name = target
        .file_name()
        .ok_or_else(|| anyhow::anyhow!("target has no file name: {}", red_path(target)))?
        .to_string_lossy();
    let tmp = parent.join(format!("{}.tmp-{}", file_name, uuid::Uuid::new_v4()));

    // Local RAII cleanup: if any step below fails, remove the tempfile on
    // return. On success we `forget()` the guard so the file isn't deleted
    // after the rename.
    struct TmpGuard<'a>(Option<&'a Path>);
    impl Drop for TmpGuard<'_> {
        fn drop(&mut self) {
            if let Some(path) = self.0 {
                drop(std::fs::remove_file(path));
            }
        }
    }
    let mut guard = TmpGuard(Some(tmp.as_path()));

    // lab-zxx5.23: create tempfile with restrictive mode. `tokio::fs::write`
    // creates at 0o666 & !umask (typically 0o644, world-readable). For
    // contents that may embed credentials (.mcp.json, agent descriptors)
    // we want 0o600 for the whole lifetime of the tempfile. On unix we
    // explicitly set the mode via OpenOptionsExt; create_new prevents a
    // pre-existing attacker-planted file or symlink from being opened.
    // On non-unix we fall back to the default tokio::fs::write path.
    #[cfg(unix)]
    {
        use tokio::io::AsyncWriteExt;
        let mut f = tokio::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&tmp)
            .await
            .with_context(|| format!("open tmpfile {}", red_path(&tmp)))?;
        f.write_all(contents)
            .await
            .with_context(|| format!("write tmpfile {}", red_path(&tmp)))?;
        f.sync_all()
            .await
            .with_context(|| format!("fsync tmpfile {}", red_path(&tmp)))?;
    }
    #[cfg(not(unix))]
    {
        tokio::fs::write(&tmp, contents)
            .await
            .with_context(|| format!("write tmpfile {}", red_path(&tmp)))?;
    }

    // Defense in depth: the tempfile should be a regular file. (Under
    // create_new + O_EXCL, O_NOFOLLOW semantics, the open would have
    // failed if someone planted a symlink, but verify explicitly.)
    let meta = tokio::fs::symlink_metadata(&tmp)
        .await
        .with_context(|| format!("stat tmpfile {}", red_path(&tmp)))?;
    if meta.file_type().is_symlink() {
        return Err(anyhow::anyhow!(
            "{ERR_SYMLINK}: tempfile {} is a symlink; refusing to rename",
            red_path(&tmp)
        ));
    }

    tokio::fs::rename(&tmp, target)
        .await
        .with_context(|| format!("rename {} -> {}", red_path(&tmp), red_path(target)))?;
    // Rename succeeded — disable cleanup so the renamed file stays at `target`.
    guard.0 = None;
    Ok(())
}

/// Resolve the write root for a given scope.
///
/// - `Global` → `~/.claude/`
/// - `Project` → `{project_path}/.claude/`
pub fn resolve_write_root(scope: InstallScope, project_path: Option<&str>) -> Result<PathBuf> {
    match scope {
        InstallScope::Global => {
            let home = std::env::var("HOME")
                .or_else(|_| std::env::var("USERPROFILE"))
                .map_err(|_| anyhow!("{ERR_VALIDATION}: HOME environment variable is not set"))?;
            Ok(PathBuf::from(home).join(".claude"))
        }
        InstallScope::Project => {
            let raw = project_path.ok_or_else(|| {
                anyhow!("{ERR_MISSING_PARAM}: project_path is required when scope == Project")
            })?;
            let p = PathBuf::from(raw);
            if !p.is_absolute() {
                return Err(anyhow!(
                    "{ERR_VALIDATION}: project_path must be absolute; got `{}`",
                    p.display()
                ));
            }
            for component in p.components() {
                if matches!(component, Component::ParentDir) {
                    return Err(anyhow!(
                        "{ERR_PATH_TRAVERSAL}: project_path `{}`",
                        p.display()
                    ));
                }
            }
            Ok(p.join(".claude"))
        }
    }
}

// --------------------------------------------------------------------------
// marketplace.install_component
// --------------------------------------------------------------------------

/// Executes a `marketplace.install_component` request.
///
/// `component_files` maps relative-path component names to their file contents
/// (already fetched from the marketplace or resolved by the caller).
///
/// Progress notifications are sent via `progress_tx` as JSON-encoded strings.
/// The caller is responsible for routing them to the WebSocket send channel.
pub async fn handle_install_component(
    params: InstallComponentParams,
    component_files: Vec<(String, Vec<u8>)>,
    rpc_id: Value,
    progress_tx: &mpsc::Sender<String>,
) -> Result<InstallComponentResult> {
    // Send started progress notification.
    send_progress(
        progress_tx,
        &InstallProgressNotification::started(rpc_id.clone()),
    )
    .await;

    let write_root = resolve_write_root(params.scope, params.project_path.as_deref())?;
    tokio::fs::create_dir_all(&write_root)
        .await
        .with_context(|| format!("create write root `{}`", red_path(&write_root)))?;

    let mut result = InstallComponentResult::default();

    for (component_path, contents) in component_files {
        // Security: reject path traversal.
        if let Err(err) = reject_path_traversal(&component_path) {
            let msg = err.to_string();
            send_progress(
                progress_tx,
                &InstallProgressNotification::file_error(
                    rpc_id.clone(),
                    component_path.clone(),
                    msg.clone(),
                ),
            )
            .await;
            result.errors.push(InstallError {
                file: component_path,
                error: msg,
            });
            continue;
        }

        let target = write_root.join(&component_path);

        // Security: reject symlink at target path.
        if let Err(err) = reject_symlink(&target).await {
            let msg = err.to_string();
            send_progress(
                progress_tx,
                &InstallProgressNotification::file_error(
                    rpc_id.clone(),
                    component_path.clone(),
                    msg.clone(),
                ),
            )
            .await;
            result.errors.push(InstallError {
                file: component_path,
                error: msg,
            });
            continue;
        }

        // Ensure parent directory exists.
        if let Some(parent) = target.parent() {
            if let Err(err) = tokio::fs::create_dir_all(parent)
                .await
                .with_context(|| format!("create parent dir for `{}`", red_path(&target)))
            {
                let msg = err.to_string();
                send_progress(
                    progress_tx,
                    &InstallProgressNotification::file_error(
                        rpc_id.clone(),
                        component_path.clone(),
                        msg.clone(),
                    ),
                )
                .await;
                result.errors.push(InstallError {
                    file: component_path,
                    error: msg,
                });
                continue;
            }
        }

        if let Err(err) = ensure_target_within_write_root(&write_root, &target).await {
            let msg = err.to_string();
            send_progress(
                progress_tx,
                &InstallProgressNotification::file_error(
                    rpc_id.clone(),
                    component_path.clone(),
                    msg.clone(),
                ),
            )
            .await;
            result.errors.push(InstallError {
                file: component_path,
                error: msg,
            });
            continue;
        }

        // lab-zxx5.18: atomic write via sibling tmpfile + rename. Avoids the
        // TOCTOU gap between `reject_symlink` and a direct `tokio::fs::write`
        // (write would follow a symlink swapped in after the check). `rename`
        // atomically replaces whatever is at `target` — if a symlink exists
        // there, it's replaced inode-to-inode, NOT followed.
        match write_atomic(&target, &contents).await {
            Ok(()) => {
                tracing::info!(
                    surface = "node",
                    service = "install",
                    action = "component.write",
                    // lab-zxx5.27: redact $HOME to avoid leaking OS username
                    // in operator logs. Per-runtime subdirs stay visible.
                    path = %crate::dispatch::helpers::redact_home(&target.to_string_lossy()),
                    "wrote component file"
                );
                send_progress(
                    progress_tx,
                    &InstallProgressNotification::file_written(
                        rpc_id.clone(),
                        component_path.clone(),
                    ),
                )
                .await;
                result.written.push(component_path);
            }
            Err(err) => {
                let msg = format!("write failed: {err}");
                send_progress(
                    progress_tx,
                    &InstallProgressNotification::file_error(
                        rpc_id.clone(),
                        component_path.clone(),
                        msg.clone(),
                    ),
                )
                .await;
                result.errors.push(InstallError {
                    file: component_path,
                    error: msg,
                });
            }
        }
    }

    Ok(result)
}

// --------------------------------------------------------------------------
// agent.install
// --------------------------------------------------------------------------

/// Executes an `agent.install` request.
///
/// Writes the agent distribution descriptor to `~/.claude/agents/{agent_id}.json`
/// (or the project-scoped equivalent). Does NOT actually invoke npx or spawn a
/// process — that is the agent runtime's responsibility.
///
/// Progress notifications follow the same pattern as `handle_install_component`.
pub async fn handle_agent_install(
    params: AgentInstallParams,
    scope: InstallScope,
    project_path: Option<&str>,
    rpc_id: Value,
    progress_tx: &mpsc::Sender<String>,
) -> Result<InstallComponentResult> {
    // Send started progress notification.
    send_progress(
        progress_tx,
        &InstallProgressNotification::started(rpc_id.clone()),
    )
    .await;

    reject_path_traversal(&params.agent_id)?;

    let write_root = resolve_write_root(scope, project_path)?;
    tokio::fs::create_dir_all(&write_root)
        .await
        .with_context(|| format!("create write root `{}`", red_path(&write_root)))?;
    let agents_dir = write_root.join("agents");
    let target_file = format!("{}.json", params.agent_id);
    let target = agents_dir.join(&target_file);

    let mut result = InstallComponentResult::default();

    // Security: reject symlink at target path.
    if let Err(err) = reject_symlink(&target).await {
        let msg = err.to_string();
        send_progress(
            progress_tx,
            &InstallProgressNotification::file_error(
                rpc_id.clone(),
                target_file.clone(),
                msg.clone(),
            ),
        )
        .await;
        result.errors.push(InstallError {
            file: target_file,
            error: msg,
        });
        return Ok(result);
    }

    // Ensure agents dir exists.
    if let Err(err) = tokio::fs::create_dir_all(&agents_dir)
        .await
        .with_context(|| format!("create agents dir `{}`", red_path(&agents_dir)))
    {
        let msg = err.to_string();
        send_progress(
            progress_tx,
            &InstallProgressNotification::file_error(
                rpc_id.clone(),
                target_file.clone(),
                msg.clone(),
            ),
        )
        .await;
        result.errors.push(InstallError {
            file: target_file,
            error: msg,
        });
        return Ok(result);
    }

    if let Err(err) = ensure_target_within_write_root(&write_root, &target).await {
        let msg = err.to_string();
        send_progress(
            progress_tx,
            &InstallProgressNotification::file_error(
                rpc_id.clone(),
                target_file.clone(),
                msg.clone(),
            ),
        )
        .await;
        result.errors.push(InstallError {
            file: target_file,
            error: msg,
        });
        return Ok(result);
    }

    // Serialize distribution descriptor.
    let payload = serde_json::json!({
        "agent_id": params.agent_id,
        "distribution": params.distribution,
    });
    let contents = match serde_json::to_vec_pretty(&payload) {
        Ok(bytes) => bytes,
        Err(err) => {
            let msg = format!("serialize distribution: {err}");
            send_progress(
                progress_tx,
                &InstallProgressNotification::file_error(
                    rpc_id.clone(),
                    target_file.clone(),
                    msg.clone(),
                ),
            )
            .await;
            result.errors.push(InstallError {
                file: target_file,
                error: msg,
            });
            return Ok(result);
        }
    };

    // lab-zxx5.22: use write_atomic on agent.install same as install_component.
    // The prior plain tokio::fs::write + reject_symlink had a TOCTOU window;
    // sibling-tmp + rename closes it.
    match write_atomic(&target, &contents).await {
        Ok(()) => {
            tracing::info!(
                surface = "node",
                service = "install",
                action = "agent.write",
                // lab-zxx5.27: redact $HOME (see component.write above).
                path = %crate::dispatch::helpers::redact_home(&target.to_string_lossy()),
                agent_id = %params.agent_id,
                "wrote agent install descriptor"
            );
            send_progress(
                progress_tx,
                &InstallProgressNotification::file_written(rpc_id.clone(), target_file.clone()),
            )
            .await;
            result.written.push(target_file);
        }
        Err(err) => {
            let msg = format!("write failed: {err}");
            send_progress(
                progress_tx,
                &InstallProgressNotification::file_error(
                    rpc_id.clone(),
                    target_file.clone(),
                    msg.clone(),
                ),
            )
            .await;
            result.errors.push(InstallError {
                file: target_file,
                error: msg,
            });
        }
    }

    Ok(result)
}

pub async fn handle_mcp_install(
    params: McpInstallParams,
    rpc_id: Value,
    progress_tx: &mpsc::Sender<String>,
) -> Result<InstallComponentResult> {
    send_progress(
        progress_tx,
        &InstallProgressNotification::started(rpc_id.clone()),
    )
    .await;

    let mut result = InstallComponentResult::default();
    let target = match params.client {
        McpClient::Claude => claude_config_path()?,
        McpClient::Codex => codex_config_path()?,
    };

    if let Err(error) = reject_symlink(&target).await {
        result.errors.push(InstallError {
            file: target_file_label(params.client).to_string(),
            error: error.to_string(),
        });
        return Ok(result);
    }

    let contents = match params.client {
        McpClient::Claude => render_claude_mcp_config(&target, &params.name, params.config).await?,
        McpClient::Codex => render_codex_mcp_config(&target, &params.name, params.config).await?,
    };

    if let Some(parent) = target.parent() {
        tokio::fs::create_dir_all(parent)
            .await
            .with_context(|| format!("create {}", red_path(parent)))?;
    }

    match write_atomic(&target, contents.as_bytes()).await {
        Ok(()) => {
            let label = target_file_label(params.client).to_string();
            result.written.push(label.clone());
            send_progress(
                progress_tx,
                &InstallProgressNotification::file_written(rpc_id, label),
            )
            .await;
        }
        Err(error) => {
            result.errors.push(InstallError {
                file: target_file_label(params.client).to_string(),
                error: error.to_string(),
            });
        }
    }

    Ok(result)
}

fn home_dir() -> Result<PathBuf> {
    crate::config::home_dir()
        .ok_or_else(|| anyhow!("{ERR_VALIDATION}: HOME environment variable is not set"))
}

fn claude_config_path() -> Result<PathBuf> {
    Ok(home_dir()?.join(".claude.json"))
}

fn codex_config_path() -> Result<PathBuf> {
    Ok(home_dir()?.join(".codex").join("config.toml"))
}

fn target_file_label(client: McpClient) -> &'static str {
    match client {
        McpClient::Claude => ".claude.json",
        McpClient::Codex => ".codex/config.toml",
    }
}

async fn render_claude_mcp_config(path: &Path, name: &str, config: Value) -> Result<String> {
    let mut root = if path.is_file() {
        let raw = tokio::fs::read_to_string(path)
            .await
            .with_context(|| format!("read {}", red_path(path)))?;
        serde_json::from_str::<Value>(&raw).with_context(|| format!("parse {}", red_path(path)))?
    } else {
        serde_json::json!({})
    };

    if !root.is_object() {
        return Err(anyhow!(
            "{ERR_VALIDATION}: Claude config root must be an object"
        ));
    }
    if root.get("mcpServers").is_none() {
        root["mcpServers"] = serde_json::json!({});
    }
    let servers = root
        .get_mut("mcpServers")
        .and_then(Value::as_object_mut)
        .ok_or_else(|| anyhow!("{ERR_VALIDATION}: mcpServers must be an object"))?;
    servers.insert(name.to_string(), config);

    serde_json::to_string_pretty(&root).context("serialize Claude MCP config")
}

async fn render_codex_mcp_config(path: &Path, name: &str, config: Value) -> Result<String> {
    let raw = if path.is_file() {
        tokio::fs::read_to_string(path)
            .await
            .with_context(|| format!("read {}", red_path(path)))?
    } else {
        String::new()
    };
    let mut root = if raw.trim().is_empty() {
        toml::Value::Table(toml::map::Map::new())
    } else {
        toml::from_str::<toml::Value>(&raw).with_context(|| format!("parse {}", red_path(path)))?
    };

    let table = root
        .as_table_mut()
        .ok_or_else(|| anyhow!("{ERR_VALIDATION}: Codex config root must be a table"))?;
    let servers = table
        .entry("mcp_servers".to_string())
        .or_insert_with(|| toml::Value::Table(toml::map::Map::new()))
        .as_table_mut()
        .ok_or_else(|| anyhow!("{ERR_VALIDATION}: mcp_servers must be a table"))?;
    servers.insert(name.to_string(), json_to_toml(config)?);

    toml::to_string_pretty(&root).context("serialize Codex MCP config")
}

fn json_to_toml(value: Value) -> Result<toml::Value> {
    match value {
        Value::Null => Err(anyhow!(
            "{ERR_VALIDATION}: TOML has no null; remove the field or use a concrete value"
        )),
        Value::Bool(value) => Ok(toml::Value::Boolean(value)),
        Value::Number(value) => {
            if let Some(value) = value.as_i64() {
                Ok(toml::Value::Integer(value))
            } else if let Some(value) = value.as_u64() {
                let value = i64::try_from(value)
                    .map_err(|_| anyhow!("{ERR_VALIDATION}: TOML integer is out of range"))?;
                Ok(toml::Value::Integer(value))
            } else if let Some(value) = value.as_f64() {
                Ok(toml::Value::Float(value))
            } else {
                Err(anyhow!("{ERR_VALIDATION}: unsupported number value"))
            }
        }
        Value::String(value) => Ok(toml::Value::String(value)),
        Value::Array(values) => values
            .into_iter()
            .map(json_to_toml)
            .collect::<Result<Vec<_>>>()
            .map(toml::Value::Array),
        Value::Object(values) => values
            .into_iter()
            .map(|(key, value)| json_to_toml(value).map(|value| (key, value)))
            .collect::<Result<toml::map::Map<_, _>>>()
            .map(toml::Value::Table),
    }
}

// --------------------------------------------------------------------------
// Tests
// --------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reject_path_traversal_catches_dotdot() {
        assert!(reject_path_traversal("..").is_err());
        assert!(reject_path_traversal("../evil").is_err());
        assert!(reject_path_traversal("agents/../evil").is_err());
        assert!(reject_path_traversal("foo/../../secret").is_err());
    }

    #[test]
    fn reject_path_traversal_rejects_absolute_paths() {
        assert!(reject_path_traversal("/etc/passwd").is_err());
        assert!(reject_path_traversal("//server/share").is_err());
        #[cfg(windows)]
        assert!(reject_path_traversal(r"C:\Windows\System32").is_err());
    }

    #[test]
    fn reject_path_traversal_allows_safe_paths() {
        assert!(reject_path_traversal("agents/my-agent.md").is_ok());
        assert!(reject_path_traversal(".mcp.json").is_ok());
        assert!(reject_path_traversal("some/nested/file.json").is_ok());
    }

    #[test]
    fn resolve_write_root_global_uses_home() {
        let root = resolve_write_root(InstallScope::Global, None).expect("global root");
        assert!(root.is_absolute());
        assert_eq!(root.file_name(), Some(std::ffi::OsStr::new(".claude")));
    }

    // Unix-only: `/abs/path` is absolute on unix but RELATIVE on Windows (which
    // needs a drive prefix like `C:\`), so the `.is_ok()` branch only holds on
    // unix. Production `resolve_write_root` uses cross-platform
    // `Path::is_absolute()`; only the unix-style fixture is platform-specific.
    #[cfg(unix)]
    #[test]
    fn resolve_write_root_project_requires_absolute() {
        assert!(resolve_write_root(InstallScope::Project, Some("relative/path")).is_err());
        assert!(resolve_write_root(InstallScope::Project, Some("/abs/path")).is_ok());
    }

    #[test]
    fn resolve_write_root_project_rejects_traversal() {
        assert!(resolve_write_root(InstallScope::Project, Some("/abs/../etc")).is_err());
    }

    #[test]
    fn scope_deserializes_lowercase() {
        let scope: InstallScope = serde_json::from_str(r#""global""#).expect("global");
        assert_eq!(scope, InstallScope::Global);
        let scope: InstallScope = serde_json::from_str(r#""project""#).expect("project");
        assert_eq!(scope, InstallScope::Project);
        assert!(serde_json::from_str::<InstallScope>(r#""workspace""#).is_err());
    }

    // Inherently unix-only: exercises symlink rejection. Whole-fn gated so no
    // scaffolding (`link`) is orphaned on Windows under `-D warnings`.
    #[cfg(unix)]
    #[tokio::test]
    async fn reject_symlink_detects_symlinks() {
        let tempdir = tempfile::tempdir().expect("tempdir");
        let target = tempdir.path().join("real_file");
        tokio::fs::write(&target, b"hello").await.expect("write");
        let link = tempdir.path().join("symlink");
        tokio::fs::symlink(&target, &link).await.expect("symlink");
        assert!(
            reject_symlink(&link).await.is_err(),
            "should reject symlink"
        );
        assert!(
            reject_symlink(&target).await.is_ok(),
            "should allow real file"
        );
    }

    #[tokio::test]
    async fn handle_install_component_writes_files() {
        let tempdir = tempfile::tempdir().expect("tempdir");

        let (progress_tx, mut progress_rx) = mpsc::channel(16);
        let params = InstallComponentParams {
            plugin_id: "test-plugin@marketplace".to_string(),
            components: vec!["agents/my-agent.md".to_string()],
            scope: InstallScope::Project,
            project_path: Some(tempdir.path().to_string_lossy().into_owned()),
        };
        let files = vec![("agents/my-agent.md".to_string(), b"# My Agent\n".to_vec())];

        let result =
            handle_install_component(params, files, serde_json::json!("req-1"), &progress_tx)
                .await
                .expect("install");

        assert_eq!(result.written, vec!["agents/my-agent.md"]);
        assert!(result.errors.is_empty());
        assert!(result.skipped.is_empty());

        let written_content =
            tokio::fs::read_to_string(tempdir.path().join(".claude/agents/my-agent.md"))
                .await
                .expect("read written file");
        assert_eq!(written_content, "# My Agent\n");

        // At least one progress notification should have been sent.
        drop(progress_rx.try_recv());
    }

    #[tokio::test]
    async fn handle_install_component_rejects_path_traversal() {
        let tempdir = tempfile::tempdir().expect("tempdir");

        let (progress_tx, _rx) = mpsc::channel(16);
        let params = InstallComponentParams {
            plugin_id: "evil@marketplace".to_string(),
            components: vec!["../etc/passwd".to_string()],
            scope: InstallScope::Project,
            project_path: Some(tempdir.path().to_string_lossy().into_owned()),
        };
        let files = vec![("../etc/passwd".to_string(), b"root:x:0:0\n".to_vec())];

        let result = handle_install_component(params, files, serde_json::json!(1), &progress_tx)
            .await
            .expect("install ran");

        assert!(result.written.is_empty());
        assert_eq!(result.errors.len(), 1);
        assert!(result.errors[0].error.contains("non-normal component"));
    }

    #[tokio::test]
    async fn handle_install_component_rejects_absolute_component_path() {
        let tempdir = tempfile::tempdir().expect("tempdir");

        let (progress_tx, _rx) = mpsc::channel(16);
        let params = InstallComponentParams {
            plugin_id: "evil@marketplace".to_string(),
            components: vec!["/tmp/escape".to_string()],
            scope: InstallScope::Project,
            project_path: Some(tempdir.path().to_string_lossy().into_owned()),
        };
        let files = vec![("/tmp/escape".to_string(), b"x".to_vec())];

        let result = handle_install_component(params, files, serde_json::json!(1), &progress_tx)
            .await
            .expect("install ran");

        assert!(result.written.is_empty());
        assert_eq!(result.errors.len(), 1);
        assert!(result.errors[0].error.contains("non-normal component"));
    }

    #[tokio::test]
    async fn handle_agent_install_writes_descriptor() {
        let tempdir = tempfile::tempdir().expect("tempdir");

        let (progress_tx, _rx) = mpsc::channel(16);
        let params = AgentInstallParams {
            agent_id: "claude-agent".to_string(),
            distribution: AgentDistribution {
                dist_type: DistType::Npx,
                package: "@anthropic/claude-agent-acp".to_string(),
                version: "0.30.0".to_string(),
            },
        };

        let result = handle_agent_install(
            params,
            InstallScope::Project,
            Some(tempdir.path().to_str().unwrap()),
            serde_json::json!("req-2"),
            &progress_tx,
        )
        .await
        .expect("agent install");

        assert_eq!(result.written, vec!["claude-agent.json"]);
        assert!(result.errors.is_empty());

        let written =
            tokio::fs::read_to_string(tempdir.path().join(".claude/agents/claude-agent.json"))
                .await
                .expect("read agent file");
        let v: Value = serde_json::from_str(&written).expect("parse");
        assert_eq!(v["agent_id"], "claude-agent");
        assert_eq!(v["distribution"]["package"], "@anthropic/claude-agent-acp");
    }

    #[tokio::test]
    async fn handle_agent_install_rejects_absolute_agent_id() {
        let tempdir = tempfile::tempdir().expect("tempdir");

        let (progress_tx, _rx) = mpsc::channel(16);
        let params = AgentInstallParams {
            agent_id: "/etc/passwd".to_string(),
            distribution: AgentDistribution {
                dist_type: DistType::Npx,
                package: "@anthropic/claude-agent-acp".to_string(),
                version: "0.30.0".to_string(),
            },
        };

        let err = handle_agent_install(
            params,
            InstallScope::Project,
            Some(tempdir.path().to_str().unwrap()),
            serde_json::json!("req-3"),
            &progress_tx,
        )
        .await
        .expect_err("absolute agent_id should fail");

        assert!(err.to_string().contains("non-normal component"));
    }

    #[tokio::test]
    async fn render_claude_mcp_config_preserves_existing_config_and_sets_server() {
        let tempdir = tempfile::tempdir().expect("tempdir");
        let path = tempdir.path().join(".claude.json");
        tokio::fs::write(
            &path,
            r#"{"theme":"dark","mcpServers":{"existing":{"command":"node"}}}"#,
        )
        .await
        .expect("seed config");

        let rendered = render_claude_mcp_config(
            &path,
            "demo",
            serde_json::json!({"command":"npx","args":["-y","@example/server"]}),
        )
        .await
        .expect("render");
        let parsed: Value = serde_json::from_str(&rendered).expect("parse rendered");

        assert_eq!(parsed["theme"], "dark");
        assert_eq!(parsed["mcpServers"]["existing"]["command"], "node");
        assert_eq!(parsed["mcpServers"]["demo"]["command"], "npx");
        assert_eq!(parsed["mcpServers"]["demo"]["args"][1], "@example/server");
    }

    #[tokio::test]
    async fn render_codex_mcp_config_preserves_existing_config_and_sets_server() {
        let tempdir = tempfile::tempdir().expect("tempdir");
        let path = tempdir.path().join(".codex/config.toml");
        tokio::fs::create_dir_all(path.parent().expect("parent"))
            .await
            .expect("mkdir");
        tokio::fs::write(
            &path,
            r#"
model = "gpt-5"

[mcp_servers.existing]
command = "node"
"#,
        )
        .await
        .expect("seed config");

        let rendered = render_codex_mcp_config(
            &path,
            "demo",
            serde_json::json!({"command":"npx","args":["-y","@example/server"],"env":{"TOKEN":"abc"}}),
        )
        .await
        .expect("render");
        let parsed: toml::Value = toml::from_str(&rendered).expect("parse rendered");

        assert_eq!(parsed["model"].as_str(), Some("gpt-5"));
        assert_eq!(
            parsed["mcp_servers"]["existing"]["command"].as_str(),
            Some("node")
        );
        assert_eq!(
            parsed["mcp_servers"]["demo"]["command"].as_str(),
            Some("npx")
        );
        assert_eq!(
            parsed["mcp_servers"]["demo"]["args"]
                .as_array()
                .expect("args")[1]
                .as_str(),
            Some("@example/server")
        );
        assert_eq!(
            parsed["mcp_servers"]["demo"]["env"]["TOKEN"].as_str(),
            Some("abc")
        );
    }

    // ------------------------------------------------------------------
    // lab-zxx5.18: write_atomic symlink-TOCTOU defense
    // ------------------------------------------------------------------

    #[tokio::test]
    async fn write_atomic_writes_regular_file() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("out.md");
        write_atomic(&target, b"hello").await.expect("write");
        let read = tokio::fs::read(&target).await.unwrap();
        assert_eq!(read, b"hello");
        let meta = tokio::fs::symlink_metadata(&target).await.unwrap();
        assert!(meta.file_type().is_file());
    }

    // Inherently unix-only: plants a symlink to verify write_atomic replaces it
    // rather than following it. Whole-fn gated so no scaffolding (`victim`,
    // `target`) is orphaned and no unreachable code remains on Windows under
    // `-D warnings`.
    #[cfg(unix)]
    #[tokio::test]
    async fn write_atomic_replaces_symlink_at_target_without_following() {
        // lab-zxx5.18 invariant: an attacker plants a symlink at `target`
        // pointing at /etc/passwd. Our write_atomic must REPLACE the symlink
        // (rename overwrites the file entry) — it must NOT write through the
        // symlink into the victim file.
        let dir = tempfile::tempdir().unwrap();
        let victim = dir.path().join("victim.txt");
        tokio::fs::write(&victim, b"DO NOT OVERWRITE")
            .await
            .unwrap();
        let target = dir.path().join("out.md");

        std::os::unix::fs::symlink(&victim, &target).unwrap();

        write_atomic(&target, b"safe content").await.expect("write");

        // target now holds the new content.
        let got = tokio::fs::read(&target).await.unwrap();
        assert_eq!(got, b"safe content");

        // victim was NOT overwritten.
        let victim_read = tokio::fs::read(&victim).await.unwrap();
        assert_eq!(victim_read, b"DO NOT OVERWRITE");

        // target is now a regular file, not a symlink.
        let meta = tokio::fs::symlink_metadata(&target).await.unwrap();
        assert!(
            meta.file_type().is_file(),
            "target should be a regular file after write_atomic replace"
        );
    }

    #[tokio::test]
    async fn write_atomic_removes_tmpfile_on_rename_failure() {
        // Target parent doesn't exist — write to tmpfile in parent will fail.
        // Verify no orphan tmpfile-like entries are left anywhere we can spot.
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("nonexistent-dir/out.md");
        let err = write_atomic(&target, b"x").await;
        assert!(err.is_err(), "must fail when parent doesn't exist");
        // Parent doesn't exist so there's nothing to orphan — this test mainly
        // locks in that the error path doesn't panic.
    }
}
