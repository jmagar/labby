//! Dispatch for `agent.*` actions within the marketplace service.
//!
//! Routes ACP agent discovery and install/uninstall operations to the
//! `lab-apis::acp_registry` SDK and the local provider config at
//! `~/.lab/acp-providers.json`.
//!
//! Distribution support:
//! - `npx` and `uvx`: write a provider config entry locally; remote via fleet WS.
//! - `binary`: download archive (HTTPS only, SSRF-guarded), compute SHA-256,
//!   extract with system tar/unzip, install to `~/.lab/bin/<agent_id>/`.
//!
//! Remote install via fleet WS supports `npx` only — the device-side `DistType`
//! has no `Uvx` or `Binary` variant.

use std::collections::HashMap;
use std::fs::File;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};
use std::time::Instant;

use serde_json::Value;

use crate::acp::providers::{AcpProviderEntry, read_providers, remove_provider, upsert_provider};
use crate::dispatch::error::ToolError;
use crate::dispatch::helpers::{action_schema, help_payload, require_str, to_json};
use crate::dispatch::marketplace::acp_catalog::ACP_ACTIONS;
use crate::dispatch::marketplace::acp_client;

#[cfg(feature = "acp_registry")]
use crate::dispatch::node::send::send_rpc_to_node;

#[cfg(feature = "acp_registry")]
use lab_apis::acp_registry::client::AcpRegistryClient;
#[cfg(feature = "acp_registry")]
use lab_apis::acp_registry::types::{Agent, BinaryAsset};

#[cfg(feature = "acp_registry")]
static BINARY_INSTALL_LOCKS: OnceLock<
    std::sync::Mutex<HashMap<String, Arc<tokio::sync::Mutex<()>>>>,
> = OnceLock::new();

/// Dispatch an `agent.*` action using a freshly constructed client.
pub async fn dispatch_acp(action: &str, params: Value) -> Result<Value, ToolError> {
    // help/schema are universal and feature-independent.
    match action {
        "help" => return Ok(help_payload("marketplace", ACP_ACTIONS)),
        "schema" => {
            let action_name = require_str(&params, "action")?;
            return action_schema(ACP_ACTIONS, action_name);
        }
        _ => {}
    }

    #[cfg(feature = "acp_registry")]
    {
        let client = acp_client::require_acp_client()?;
        dispatch_acp_with_client(&client, action, params).await
    }
    #[cfg(not(feature = "acp_registry"))]
    {
        let _ = (action, params);
        Err(acp_client::require_acp_client().unwrap_err())
    }
}

/// Dispatch an `agent.*` action with a pre-built client (used by API handlers).
#[cfg(feature = "acp_registry")]
pub async fn dispatch_acp_with_client(
    client: &AcpRegistryClient,
    action: &str,
    params: Value,
) -> Result<Value, ToolError> {
    let started = Instant::now();
    let result = dispatch_acp_with_client_inner(client, action, params).await;
    log_acp_dispatch_outcome(action, started, &result);
    result
}

#[cfg(feature = "acp_registry")]
async fn dispatch_acp_with_client_inner(
    client: &AcpRegistryClient,
    action: &str,
    params: Value,
) -> Result<Value, ToolError> {
    match action {
        "agent.list" => {
            let agents = client.list_agents().await?;
            to_json(enrich_agents_with_install_state(agents)?)
        }
        "agent.get" => {
            let id = require_str(&params, "id")?.to_string();
            let agent = client.get_agent(&id).await?.ok_or_else(|| ToolError::Sdk {
                sdk_kind: "not_found".to_string(),
                message: format!("agent `{id}` not found in registry"),
            })?;
            to_json(enrich_agents_with_install_state(vec![agent])?.remove(0))
        }
        "agent.install" => dispatch_install(client, &params).await,
        "agent.uninstall" => {
            let id = require_str(&params, "id")?.to_string();
            dispatch_uninstall(&id)
        }
        unknown => Err(ToolError::UnknownAction {
            message: format!("unknown action `marketplace.{unknown}`"),
            valid: ACP_ACTIONS.iter().map(|a| a.name.to_string()).collect(),
            hint: None,
        }),
    }
}

#[cfg(feature = "acp_registry")]
fn log_acp_dispatch_outcome(action: &str, started: Instant, result: &Result<Value, ToolError>) {
    let elapsed_ms = started.elapsed().as_millis();
    match result {
        Ok(_) => tracing::info!(
            surface = "mcp",
            service = "marketplace",
            action,
            event = "acp.dispatch.finished",
            elapsed_ms,
            "marketplace ACP dispatch finished"
        ),
        Err(error) if error.is_internal() => tracing::error!(
            surface = "mcp",
            service = "marketplace",
            action,
            event = "acp.dispatch.failed",
            elapsed_ms,
            kind = error.kind(),
            "marketplace ACP dispatch failed"
        ),
        Err(error) => tracing::warn!(
            surface = "mcp",
            service = "marketplace",
            action,
            event = "acp.dispatch.failed",
            elapsed_ms,
            kind = error.kind(),
            "marketplace ACP dispatch failed"
        ),
    }
}

#[cfg(feature = "acp_registry")]
fn enrich_agents_with_install_state(mut agents: Vec<Agent>) -> Result<Vec<Agent>, ToolError> {
    let providers = read_providers()?;
    let codex_ready = crate::acp::runtime::codex_provider_health().available;
    for agent in &mut agents {
        let installed = providers.iter().find(|provider| provider.id == agent.id);
        if let Some(provider) = installed {
            agent
                .extra
                .insert("installed".to_string(), Value::Bool(true));
            agent.extra.insert(
                "installedAt".to_string(),
                Value::String(provider.installed_at.clone()),
            );
            agent.extra.insert(
                "command".to_string(),
                Value::String(provider.command.clone()),
            );
        } else if agent.id == "codex-acp" && codex_ready {
            agent
                .extra
                .insert("installed".to_string(), Value::Bool(true));
            agent.extra.insert("builtin".to_string(), Value::Bool(true));
            agent.extra.insert(
                "command".to_string(),
                Value::String("npx @zed-industries/codex-acp".to_string()),
            );
        } else {
            agent
                .extra
                .insert("installed".to_string(), Value::Bool(false));
        }
    }
    Ok(agents)
}

// ---------------------------------------------------------------------------
// agent.install
// ---------------------------------------------------------------------------

#[cfg(feature = "acp_registry")]
async fn dispatch_install(client: &AcpRegistryClient, params: &Value) -> Result<Value, ToolError> {
    let id = require_str(params, "id")?.to_string();
    validate_agent_id_for_path(&id)?;

    let node_ids: Vec<String> = match params.get("node_ids") {
        Some(Value::Array(arr)) => arr
            .iter()
            .filter_map(Value::as_str)
            .map(str::to_string)
            .collect(),
        Some(_) => {
            return Err(ToolError::InvalidParam {
                message: "`node_ids` must be an array of strings".to_string(),
                param: "node_ids".to_string(),
            });
        }
        None => {
            return Err(ToolError::MissingParam {
                message: "missing required parameter `node_ids`".to_string(),
                param: "node_ids".to_string(),
            });
        }
    };

    if node_ids.is_empty() {
        return Err(ToolError::InvalidParam {
            message: "`node_ids` must not be empty".to_string(),
            param: "node_ids".to_string(),
        });
    }

    let platform_override = params
        .get("platform")
        .and_then(Value::as_str)
        .map(str::to_string);

    let agent = client.get_agent(&id).await?.ok_or_else(|| ToolError::Sdk {
        sdk_kind: "not_found".to_string(),
        message: format!("agent `{id}` not found in registry"),
    })?;

    let mut results = Vec::with_capacity(node_ids.len());
    for node_id in &node_ids {
        let outcome = if is_local_node(node_id) {
            install_local(&agent, &id, platform_override.as_deref()).await
        } else {
            install_remote(node_id, &agent, &id).await
        };
        match outcome {
            Ok(value) => results.push(serde_json::json!({
                "node_id": node_id,
                "ok": true,
                "result": value,
            })),
            Err(e) => results.push(serde_json::json!({
                "node_id": node_id,
                "ok": false,
                "error": serde_json::to_value(&e).unwrap_or(Value::Null),
            })),
        }
    }

    Ok(serde_json::json!({
        "agent_id": id,
        "results": results,
    }))
}

fn is_local_node(node_id: &str) -> bool {
    if node_id.eq_ignore_ascii_case("local") {
        return true;
    }
    if let Ok(host) = std::env::var("HOSTNAME")
        && !host.is_empty()
        && node_id.eq_ignore_ascii_case(&host)
    {
        return true;
    }
    false
}

#[cfg(feature = "acp_registry")]
async fn install_local(
    agent: &Agent,
    agent_id: &str,
    platform_override: Option<&str>,
) -> Result<Value, ToolError> {
    // Prefer binary if the platform is covered, then npx, then uvx.
    if let Some(map) = &agent.distribution.binary {
        let platform = platform_override
            .map(str::to_string)
            .unwrap_or_else(detect_platform);
        let asset = map.get(&platform).ok_or_else(|| ToolError::Sdk {
            sdk_kind: "not_found".to_string(),
            message: format!(
                "agent `{}` has no binary asset for platform `{}` (available: {})",
                agent.id,
                platform,
                map.keys().cloned().collect::<Vec<_>>().join(", ")
            ),
        })?;
        let (cmd_path, digest) = install_binary(agent_id, asset).await?;
        let entry = AcpProviderEntry {
            id: agent_id.to_string(),
            name: agent.name.clone(),
            version: agent.version.clone(),
            distribution: "binary".to_string(),
            command: cmd_path.to_string_lossy().into_owned(),
            args: asset.args.clone(),
            cwd: None,
            env: std::collections::BTreeMap::new(),
            installed_at: jiff::Timestamp::now().to_string(),
            sha256: Some(digest),
        };
        upsert_provider(&entry)?;
        return serde_json::to_value(&entry)
            .map_err(|e| ToolError::internal_message(format!("serialize provider: {e}")));
    }

    // Build structured (command, args) per distribution kind. Quoted args
    // and paths-with-spaces survive the JSON round-trip because each entry
    // in `args` is one literal argv element — no whitespace-joining.
    let (distribution_kind, command, args, sha256) = if let Some(asset) = &agent.distribution.npx {
        let pkg = match &asset.version {
            Some(v) => format!("{}@{}", asset.package, v),
            None => asset.package.clone(),
        };
        let mut args = vec!["-y".to_string(), pkg];
        args.extend(asset.args.iter().cloned());
        ("npx", "npx".to_string(), args, None)
    } else if let Some(asset) = &agent.distribution.uvx {
        let pkg = match &asset.version {
            Some(v) => format!("{}=={}", asset.package, v),
            None => asset.package.clone(),
        };
        let mut args = vec![pkg];
        args.extend(asset.args.iter().cloned());
        ("uvx", "uvx".to_string(), args, None)
    } else {
        return Err(ToolError::Sdk {
            sdk_kind: "not_supported".to_string(),
            message: format!(
                "agent `{agent_id}` has no supported local distribution method \
                 (binary/npx/uvx)"
            ),
        });
    };

    let entry = AcpProviderEntry {
        id: agent_id.to_string(),
        name: agent.name.clone(),
        version: agent.version.clone(),
        distribution: distribution_kind.to_string(),
        command,
        args,
        cwd: None,
        env: std::collections::BTreeMap::new(),
        installed_at: jiff::Timestamp::now().to_string(),
        sha256,
    };

    upsert_provider(&entry)?;
    serde_json::to_value(&entry)
        .map_err(|e| ToolError::internal_message(format!("serialize provider: {e}")))
}

fn detect_platform() -> String {
    // Mirror the registry's `<os>-<arch>` triple convention.
    let os = match std::env::consts::OS {
        "macos" => "darwin",
        other => other,
    };
    let arch = match std::env::consts::ARCH {
        "x86_64" => "x86_64",
        "aarch64" => "aarch64",
        other => other,
    };
    format!("{os}-{arch}")
}

// ---------------------------------------------------------------------------
// Remote fleet RPC install
// ---------------------------------------------------------------------------

/// Send an `agent.install` JSON-RPC 2.0 message to a connected remote device.
///
/// Only `npx` distribution is supported because the device-side `DistType` only
/// has an `Npx` variant. `uvx` and `binary` return a structured error.
#[cfg(feature = "acp_registry")]
async fn install_remote(node_id: &str, agent: &Agent, agent_id: &str) -> Result<Value, ToolError> {
    // Remote install only supports npx; uvx and binary are device-side gaps.
    let (package, version) = if let Some(asset) = &agent.distribution.npx {
        (asset.package.clone(), asset.version.clone())
    } else if agent.distribution.uvx.is_some() {
        return Err(ToolError::Sdk {
            sdk_kind: "not_implemented".to_string(),
            message: format!(
                "remote install of `{agent_id}` is not supported for uvx distribution \
                 (node runtime only handles npx)"
            ),
        });
    } else if agent.distribution.binary.is_some() {
        return Err(ToolError::Sdk {
            sdk_kind: "not_implemented".to_string(),
            message: format!(
                "remote install of `{agent_id}` is not supported for binary distribution \
                 (node runtime only handles npx)"
            ),
        });
    } else {
        return Err(ToolError::Sdk {
            sdk_kind: "not_supported".to_string(),
            message: format!(
                "agent `{agent_id}` has no supported remote distribution method (npx required)"
            ),
        });
    };

    // lab-zxx5.21: route through send_rpc_to_node so a UUIDv4 rpc_id is
    // generated and response/progress correlation uses the same machinery
    // as cherry-pick. Previously used send_text_to_node with hard-coded
    // id=0, which collided under concurrency and blocked SSE progress.
    let params = serde_json::json!({
        "agent_id": agent_id,
        "distribution": {
            "type": "npx",
            "package": package,
            "version": version,
        }
    });

    let result = send_rpc_to_node(node_id, "agent.install", params).await?;
    // lab-zxx5.29: validate the node's response shape BEFORE reporting
    // success to the caller. A well-behaved node returns
    // `InstallComponentResult { written: [...], skipped: [...], errors: [...] }`.
    // A malformed node that replies with {} or Null would previously propagate
    // as `{"result": null}` — silent success. Reject with decode_error so SSE
    // clients and CLI callers see a real failure.
    validate_install_result(&result).map_err(|msg| ToolError::Sdk {
        sdk_kind: "decode_error".to_string(),
        message: format!("node `{node_id}` returned malformed agent.install result: {msg}"),
    })?;

    Ok(serde_json::json!({
        "node_id": node_id,
        "agent_id": agent_id,
        "result": result,
    }))
}

/// Validate that a node RPC result conforms to the `InstallComponentResult`
/// shape produced by `handle_install_component` / `handle_agent_install`:
/// `{ written: Vec<String>, skipped: Vec<String>, errors: Vec<_> }`.
///
/// Returns a short explanation on mismatch. Used by `install_remote` and
/// `plugin_cherry_pick` to guard against silent-success-on-garbage.
fn validate_install_result(result: &Value) -> Result<(), &'static str> {
    let obj = result.as_object().ok_or("result is not an object")?;
    for field in &["written", "skipped", "errors"] {
        match obj.get(*field) {
            Some(Value::Array(_)) => {}
            Some(_) => return Err("result has a required field of the wrong type"),
            None => return Err("result is missing a required field (written/skipped/errors)"),
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Binary distribution local install
// ---------------------------------------------------------------------------

/// Download, extract, and install a binary agent to `~/.lab/bin/<agent_id>/`.
///
/// Returns `(installed_path, sha256_hex)`.
#[cfg(feature = "acp_registry")]
async fn install_binary(
    agent_id: &str,
    asset: &BinaryAsset,
) -> Result<(PathBuf, String), ToolError> {
    let expected_sha256 = expected_archive_sha256(asset)?;
    validate_archive_url(&asset.archive)?;

    let install_lock = binary_install_lock(agent_id)?;
    let _install_guard = install_lock.lock().await;

    let install_dir = agent_bin_dir(agent_id)?;
    std::fs::create_dir_all(&install_dir).map_err(|e| {
        ToolError::internal_message(format!("create {}: {e}", install_dir.display()))
    })?;

    // Download to a temp file next to the install dir so rename is atomic.
    let tmp_archive = tempfile::NamedTempFile::new_in(&install_dir)
        .map_err(|e| ToolError::internal_message(format!("temp archive: {e}")))?;

    let sha256 = download_archive(&asset.archive, tmp_archive.path()).await?;
    verify_archive_sha256(&expected_sha256, &sha256)?;

    // Extract to a temp dir in the same parent so we can do an atomic move.
    let tmp_extract = tempfile::TempDir::new_in(&install_dir)
        .map_err(|e| ToolError::internal_message(format!("temp extract dir: {e}")))?;

    extract_archive(tmp_archive.path(), tmp_extract.path(), &asset.archive)?;

    // Locate the binary: `cmd` is like `"./my-agent"` or `"my-agent"`.
    let binary_name = Path::new(asset.cmd.trim_start_matches("./"))
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or(asset.cmd.trim_start_matches("./"));

    let src =
        find_binary_in_dir(tmp_extract.path(), binary_name).ok_or_else(|| ToolError::Sdk {
            sdk_kind: "not_found".to_string(),
            message: format!(
                "binary `{binary_name}` not found in extracted archive for agent `{agent_id}`"
            ),
        })?;

    let dest = install_dir.join(binary_name);

    install_executable_atomically(&src, &dest)?;

    Ok((dest, sha256))
}

#[cfg(feature = "acp_registry")]
fn binary_install_lock(agent_id: &str) -> Result<Arc<tokio::sync::Mutex<()>>, ToolError> {
    let locks = BINARY_INSTALL_LOCKS.get_or_init(|| std::sync::Mutex::new(HashMap::new()));
    let mut locks = locks
        .lock()
        .map_err(|_| ToolError::internal_message("binary install lock poisoned"))?;
    Ok(locks
        .entry(agent_id.to_string())
        .or_insert_with(|| Arc::new(tokio::sync::Mutex::new(())))
        .clone())
}

#[cfg(feature = "acp_registry")]
fn expected_archive_sha256(asset: &BinaryAsset) -> Result<String, ToolError> {
    if let Some(value) = asset.sha256.as_deref() {
        return normalize_sha256(value, "sha256");
    }
    if let Some(value) = asset.digest.as_deref() {
        return normalize_sha256(value, "digest");
    }
    Err(ToolError::Sdk {
        sdk_kind: "integrity_missing".to_string(),
        message: format!(
            "binary archive `{}` has no SHA-256 integrity metadata; refusing install",
            asset.archive
        ),
    })
}

#[cfg(feature = "acp_registry")]
fn normalize_sha256(value: &str, field: &str) -> Result<String, ToolError> {
    let trimmed = value.trim();
    let hex = trimmed.strip_prefix("sha256:").unwrap_or(trimmed);
    if hex.len() == 64 && hex.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Ok(hex.to_ascii_lowercase());
    }
    Err(ToolError::Sdk {
        sdk_kind: "invalid_param".to_string(),
        message: format!(
            "binary archive `{field}` must be a SHA-256 digest as 64 hex chars or sha256:<hex>"
        ),
    })
}

#[cfg(feature = "acp_registry")]
fn verify_archive_sha256(expected: &str, actual: &str) -> Result<(), ToolError> {
    if expected.eq_ignore_ascii_case(actual) {
        return Ok(());
    }
    Err(ToolError::Sdk {
        sdk_kind: "integrity_mismatch".to_string(),
        message: format!("binary archive SHA-256 mismatch: expected {expected}, got {actual}"),
    })
}

// Note: `NamedTempFile::persist(dest)` uses `rename(2)` which is atomic on
// Linux.  Windows `MoveFileEx` without `MOVEFILE_REPLACE_EXISTING` fails when
// the destination exists, but Windows is not a supported platform for this
// binary, so no cross-platform fallback is implemented.
#[cfg(feature = "acp_registry")]
fn install_executable_atomically(src: &Path, dest: &Path) -> Result<(), ToolError> {
    let parent = dest
        .parent()
        .ok_or_else(|| ToolError::internal_message("install destination has no parent"))?;

    if let Ok(meta) = std::fs::symlink_metadata(dest)
        && meta.file_type().is_symlink()
    {
        return Err(ToolError::Sdk {
            sdk_kind: "invalid_param".to_string(),
            message: format!(
                "refusing to overwrite symlink at {} (must be a regular file)",
                dest.display()
            ),
        });
    }

    let mut input = File::open(src)
        .map_err(|e| ToolError::internal_message(format!("open {}: {e}", src.display())))?;
    let mut temp = tempfile::NamedTempFile::new_in(parent)
        .map_err(|e| ToolError::internal_message(format!("temp executable: {e}")))?;
    {
        let output = temp.as_file_mut();
        std::io::copy(&mut input, output).map_err(|e| {
            ToolError::internal_message(format!(
                "copy {} to temp executable in {}: {e}",
                src.display(),
                parent.display()
            ))
        })?;
        output.flush().map_err(|e| {
            ToolError::internal_message(format!("flush temp executable {}: {e}", dest.display()))
        })?;
    }

    #[cfg(unix)]
    {
        // Set exact 0o755 (rwxr-xr-x). Do not OR with source permissions:
        // source archives may carry setuid/setgid bits.
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(temp.path(), std::fs::Permissions::from_mode(0o755)).map_err(
            |e| {
                ToolError::internal_message(format!(
                    "chmod 0o755 temp executable for {}: {e}",
                    dest.display()
                ))
            },
        )?;
    }

    temp.as_file().sync_all().map_err(|e| {
        ToolError::internal_message(format!("fsync temp executable {}: {e}", dest.display()))
    })?;
    temp.persist(dest).map_err(|e| {
        ToolError::internal_message(format!("atomic rename {}: {e}", dest.display()))
    })?;
    fsync_parent_dir(parent);
    Ok(())
}

#[cfg(feature = "acp_registry")]
fn fsync_parent_dir(parent: &Path) {
    #[cfg(unix)]
    if let Ok(dir) = File::open(parent) {
        drop(dir.sync_all());
    }
    #[cfg(not(unix))]
    let _ = parent;
}

/// Resolve `~/.lab/bin/<agent_id>/`.
fn agent_bin_dir(agent_id: &str) -> Result<PathBuf, ToolError> {
    validate_agent_id_for_path(agent_id)?;
    let env_path = crate::config::dotenv_path().ok_or_else(|| ToolError::Sdk {
        sdk_kind: "internal_error".to_string(),
        message: "cannot determine ~/.lab path".to_string(),
    })?;
    let lab_dir = env_path
        .parent()
        .ok_or_else(|| ToolError::internal_message("dotenv path has no parent"))?;
    Ok(lab_dir.join("bin").join(agent_id))
}

fn validate_agent_id_for_path(agent_id: &str) -> Result<(), ToolError> {
    let valid = !agent_id.is_empty()
        && agent_id.len() <= 128
        && agent_id != "."
        && agent_id != ".."
        && agent_id
            .as_bytes()
            .first()
            .is_some_and(u8::is_ascii_alphanumeric)
        && agent_id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'));

    if valid {
        return Ok(());
    }

    Err(ToolError::InvalidParam {
        message:
            "agent_id must be 1-128 ASCII chars: letters, digits, dot, underscore, or dash; it must start with a letter or digit"
                .to_string(),
        param: "id".to_string(),
    })
}

/// Validate an archive URL: require HTTPS, reject loopback/private hosts.
///
/// lab-zxx5.27 hardening:
/// - reject `0.0.0.0` / `::` via `is_unspecified` (routes to listening
///   loopback sockets on Linux)
/// - reject IPv4-mapped IPv6 loopback (`::ffff:127.0.0.1`) via explicit
///   unwrap of the mapped V4 form (Rust stable IPv6Addr::is_loopback only
///   covers `::1`)
/// - reject common homelab private TLDs in addition to `.local`
///
/// lab-qq8y.7 hardening: archive downloads resolve the host, reject blocked
/// addresses, and pin those validated addresses into reqwest via
/// `resolve_to_addrs` so reqwest does not perform a second DNS lookup during
/// connect.
fn validate_archive_url(url: &str) -> Result<(), ToolError> {
    validated_archive_url(url).map(|_| ())
}

fn validated_archive_url(url: &str) -> Result<url::Url, ToolError> {
    const PRIVATE_SUFFIXES: &[&str] =
        &[".local", ".internal", ".lan", ".intranet", ".corp", ".home"];

    let parsed = url::Url::parse(url).map_err(|e| ToolError::Sdk {
        sdk_kind: "invalid_param".to_string(),
        message: format!("invalid archive URL `{url}`: {e}"),
    })?;
    if parsed.scheme() != "https" {
        return Err(ToolError::Sdk {
            sdk_kind: "invalid_param".to_string(),
            message: format!("archive URL must use https, got `{}`", parsed.scheme()),
        });
    }
    let host = parsed.host_str().ok_or_else(|| ToolError::Sdk {
        sdk_kind: "invalid_param".to_string(),
        message: format!("archive URL has no host: {url}"),
    })?;

    let host_lower = host.to_ascii_lowercase();
    if host_lower == "localhost"
        || host_lower.starts_with("127.")
        || host_lower == "::1"
        || host_lower.contains("::ffff:")
        || host_lower == "0.0.0.0"
        || PRIVATE_SUFFIXES.iter().any(|s| host_lower.ends_with(s))
    {
        return Err(ToolError::Sdk {
            sdk_kind: "invalid_param".to_string(),
            message: format!("archive URL host `{host}` is a local/loopback address"),
        });
    }
    if let Ok(addr) = host.parse::<std::net::IpAddr>() {
        check_archive_ip_not_private(addr, host)?;
    }

    Ok(parsed)
}

async fn archive_download_client(parsed: &url::Url) -> Result<reqwest::Client, ToolError> {
    let host = parsed.host_str().ok_or_else(|| ToolError::Sdk {
        sdk_kind: "invalid_param".to_string(),
        message: format!("archive URL has no host: {parsed}"),
    })?;
    let port = parsed.port_or_known_default().unwrap_or(443);
    let addrs = resolve_archive_host(host, port).await?;

    reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .no_proxy()
        .resolve_to_addrs(host, &addrs)
        .timeout(std::time::Duration::from_secs(300))
        .build()
        .map_err(|e| ToolError::internal_message(format!("build http client: {e}")))
}

async fn resolve_archive_host(
    host: &str,
    port: u16,
) -> Result<Vec<std::net::SocketAddr>, ToolError> {
    let addrs: Vec<std::net::SocketAddr> = tokio::net::lookup_host((host, port))
        .await
        .map_err(|e| ToolError::Sdk {
            sdk_kind: "network_error".to_string(),
            message: format!("resolve archive URL host `{host}`: {e}"),
        })?
        .collect();

    if addrs.is_empty() {
        return Err(ToolError::Sdk {
            sdk_kind: "network_error".to_string(),
            message: format!("resolve archive URL host `{host}` returned no addresses"),
        });
    }

    for addr in &addrs {
        check_archive_ip_not_private(addr.ip(), host)?;
    }

    Ok(addrs)
}

fn check_archive_ip_not_private(ip: std::net::IpAddr, host: &str) -> Result<(), ToolError> {
    let normalized = match ip {
        std::net::IpAddr::V6(v6) => match v6.to_ipv4_mapped() {
            Some(v4) => std::net::IpAddr::V4(v4),
            None => std::net::IpAddr::V6(v6),
        },
        other => other,
    };

    let blocked = match normalized {
        std::net::IpAddr::V4(v4) => {
            let octets = v4.octets();
            v4.is_private()
                || v4.is_link_local()
                || v4.is_loopback()
                || v4.is_unspecified()
                || (octets[0] == 100 && (64..=127).contains(&octets[1]))
        }
        std::net::IpAddr::V6(v6) => {
            v6.is_unique_local()
                || v6.is_unicast_link_local()
                || v6.is_loopback()
                || v6.is_unspecified()
        }
    };

    if blocked {
        return Err(ToolError::Sdk {
            sdk_kind: "ssrf_blocked".to_string(),
            message: format!(
                "archive URL host `{host}` resolves to private/loopback/link-local address {ip}; blocked to prevent SSRF"
            ),
        });
    }

    Ok(())
}

async fn cleanup_partial_archive(dest: &Path, action: &'static str) {
    if let Err(e) = tokio::fs::remove_file(dest).await {
        tracing::warn!(
            surface = "dispatch",
            service = "marketplace",
            action,
            path = %dest.display(),
            error = %e,
            "download-cleanup remove_file failed; partial archive retained"
        );
    }
}

fn archive_size_error(url: &str, size: u64) -> ToolError {
    ToolError::Sdk {
        sdk_kind: "content_too_large".to_string(),
        message: format!("download of {url} exceeded maximum ACP archive size of {size} bytes"),
    }
}

fn enforce_archive_size_limit(
    downloaded: &mut u64,
    chunk_len: usize,
    url: &str,
) -> Result<(), ToolError> {
    let chunk_len =
        u64::try_from(chunk_len).map_err(|_| archive_size_error(url, MAX_ACP_ARCHIVE_BYTES))?;
    let next = downloaded
        .checked_add(chunk_len)
        .ok_or_else(|| archive_size_error(url, MAX_ACP_ARCHIVE_BYTES))?;
    if next > MAX_ACP_ARCHIVE_BYTES {
        return Err(archive_size_error(url, MAX_ACP_ARCHIVE_BYTES));
    }
    *downloaded = next;
    Ok(())
}

/// Maximum bytes accepted for one ACP binary archive download.
///
/// The registry distributes small agent adapters; 256 MiB leaves enough room
/// for bundled runtimes while preventing unbounded disk growth from hostile or
/// misconfigured archive URLs. Oversized streams are aborted and partial files
/// are removed before surfacing the error.
const MAX_ACP_ARCHIVE_BYTES: u64 = 256 * 1024 * 1024;

/// Download `url` to `dest`, return the hex SHA-256 of the downloaded bytes.
///
/// Streaming implementation: chunks are fed to both the SHA-256 hasher and the
/// file writer concurrently so no full-archive buffer is needed in RAM.
///
/// lab-zxx5.18: download progress watchdog — if no bytes arrive for
/// `DOWNLOAD_STALL_TIMEOUT` (30s), the in-flight download is aborted,
/// the partial file is cleaned up, and an `install_timeout` error is
/// returned. This is separate from the overall reqwest timeout, which
/// caps the total duration; the watchdog catches a stalled connection
/// that's neither fast-failing nor completing.
async fn download_archive(url: &str, dest: &Path) -> Result<String, ToolError> {
    use futures::StreamExt;
    use sha2::{Digest, Sha256};
    use tokio::io::AsyncWriteExt;

    /// Abort the download if no bytes arrive within this window. Distinct
    /// from the overall `.timeout()` — catches stalled connections.
    const DOWNLOAD_STALL_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30);

    let parsed = validated_archive_url(url)?;
    let client = archive_download_client(&parsed).await?;

    let resp = client.get(url).send().await.map_err(|e| ToolError::Sdk {
        sdk_kind: "network_error".to_string(),
        message: format!("GET {url}: {e}"),
    })?;

    if !resp.status().is_success() {
        return Err(ToolError::Sdk {
            sdk_kind: "network_error".to_string(),
            message: format!("GET {url}: HTTP {}", resp.status()),
        });
    }
    if let Some(content_length) = resp.content_length()
        && content_length > MAX_ACP_ARCHIVE_BYTES
    {
        return Err(archive_size_error(url, MAX_ACP_ARCHIVE_BYTES));
    }

    let mut file = tokio::fs::File::create(dest)
        .await
        .map_err(|e| ToolError::internal_message(format!("create {}: {e}", dest.display())))?;

    let mut hasher = Sha256::new();
    let mut stream = resp.bytes_stream();
    let mut downloaded = 0_u64;

    loop {
        // Watchdog: each chunk fetch is wrapped in a stall timeout. A download
        // that stops producing bytes within the window is treated as a fatal
        // install_timeout rather than waiting out the full request timeout.
        match tokio::time::timeout(DOWNLOAD_STALL_TIMEOUT, stream.next()).await {
            Ok(Some(chunk_result)) => {
                let chunk = chunk_result.map_err(|e| ToolError::Sdk {
                    sdk_kind: "network_error".to_string(),
                    message: format!("read body chunk from {url}: {e}"),
                })?;
                if let Err(e) = enforce_archive_size_limit(&mut downloaded, chunk.len(), url) {
                    drop(file);
                    cleanup_partial_archive(dest, "download_archive.size.cleanup").await;
                    return Err(e);
                }
                hasher.update(&chunk);
                file.write_all(&chunk).await.map_err(|e| {
                    ToolError::internal_message(format!("write chunk to {}: {e}", dest.display()))
                })?;
            }
            Ok(None) => break,
            Err(_) => {
                // Stall — clean up the partial file and surface install_timeout.
                //
                // Safe to `remove_file(dest)` unconditionally: `dest` is the
                // path of a `NamedTempFile` created per call in `install_binary`.
                // A concurrent `install_binary` for the same agent_id allocates
                // its own tempfile with a distinct random suffix, so there is
                // no cross-call delete race here.
                //
                // lab-zxx5.32: log on cleanup failure so operators have
                // visibility into orphan tempfiles. Don't promote to fatal —
                // the install_timeout is the primary signal.
                drop(file);
                cleanup_partial_archive(dest, "download_archive.stall.cleanup").await;
                return Err(ToolError::Sdk {
                    sdk_kind: "install_timeout".to_string(),
                    message: format!(
                        "download of {url} stalled for more than {:?}; aborted",
                        DOWNLOAD_STALL_TIMEOUT
                    ),
                });
            }
        }
    }

    file.flush()
        .await
        .map_err(|e| ToolError::internal_message(format!("flush {}: {e}", dest.display())))?;
    // lab-zxx5.32: durably commit before returning the SHA. Without this,
    // the returned hash matches bytes that may not be on disk if the system
    // crashes between flush-to-userspace-buffer and disk-commit. Matches
    // the hardening in node/install.rs::write_atomic.
    file.sync_all()
        .await
        .map_err(|e| ToolError::internal_message(format!("fsync {}: {e}", dest.display())))?;

    Ok(hex::encode(hasher.finalize()))
}

/// Extract `archive` into `dest_dir` using system `tar` or `unzip`.
///
/// lab-zxx5.24: zip-slip defense via post-extract canonical-containment walk.
///
/// lab-zxx5.30: partial-extraction defense. `exit.success() == true` is
/// not sufficient — BSD tar on macOS and older unzip exit 0 in several
/// partial scenarios (control-char entries, truncated mid-stream on some
/// libarchive builds). We now:
/// 1. List the archive first (`tar -tzf` / `unzip -l`) to get the expected
///    entry-count,
/// 2. Capture stderr during extraction (via `Command::output()`) and treat
///    any non-empty stderr as a hard failure — these tools are silent on
///    success,
/// 3. Post-extract, walk `dest_dir` and count files. If the count falls
///    below the expected minimum, fail closed.
fn extract_archive(archive: &Path, dest_dir: &Path, url: &str) -> Result<(), ToolError> {
    let archive_s = archive.to_string_lossy();
    let dest_s = dest_dir.to_string_lossy();

    // Step 1: list the archive to learn what should be extracted.
    let expected_file_count = list_archive_file_count(archive, url)?;

    // Step 2: extract, capturing stderr for warnings-as-errors detection.
    let output = if url.ends_with(".zip") {
        std::process::Command::new("unzip")
            .args(["-q", &archive_s, "-d", &dest_s])
            .output()
    } else {
        let flag = if url.ends_with(".tar.xz") || url.ends_with(".txz") {
            "-xJf"
        } else {
            "-xzf"
        };
        std::process::Command::new("tar")
            .args([flag, &archive_s, "-C", &dest_s, "--no-same-owner"])
            .output()
    };

    let output = output.map_err(|e| ToolError::Sdk {
        sdk_kind: "internal_error".to_string(),
        message: format!("run extraction tool: {e}"),
    })?;

    if !output.status.success() {
        return Err(ToolError::Sdk {
            sdk_kind: "internal_error".to_string(),
            message: format!(
                "extraction failed (exit {}): {}",
                output.status,
                String::from_utf8_lossy(&output.stderr)
            ),
        });
    }

    // Step 3: any stderr content on a "successful" run is a warning that,
    // for security-sensitive extractions, we treat as failure. tar/unzip
    // with `-q` are silent on clean success.
    if !output.stderr.is_empty() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        // Some tar implementations emit benign "Ignoring unknown extended
        // header keyword" lines on archives generated by newer tar. Don't
        // fail on those specifically — but fail on everything else.
        let non_benign: Vec<&str> = stderr
            .lines()
            .filter(|line| !line.is_empty())
            .filter(|line| !line.contains("Ignoring unknown extended header"))
            .filter(|line| !line.contains("Removing leading"))
            .collect();
        if !non_benign.is_empty() {
            return Err(ToolError::Sdk {
                sdk_kind: "internal_error".to_string(),
                message: format!(
                    "extraction tool emitted warnings; treating as failure: {}",
                    non_benign.join(" | ")
                ),
            });
        }
    }

    // Step 4: containment walk + count verification.
    let canonical_root = std::fs::canonicalize(dest_dir).map_err(|e| ToolError::Sdk {
        sdk_kind: "internal_error".to_string(),
        message: format!("canonicalize extract root {}: {e}", dest_dir.display()),
    })?;
    let actual_file_count = validate_no_escape(&canonical_root, dest_dir)?;

    if actual_file_count < expected_file_count {
        return Err(ToolError::Sdk {
            sdk_kind: "internal_error".to_string(),
            message: format!(
                "partial extraction detected: expected at least {expected_file_count} files, found {actual_file_count}"
            ),
        });
    }

    Ok(())
}

/// Ask `tar` or `unzip` how many file entries the archive contains.
/// Used by `extract_archive` as a pre-flight expectation. We count only
/// regular-file-looking entries, not directories, to stay consistent with
/// the post-extract walk in `validate_no_escape`.
fn list_archive_file_count(archive: &Path, url: &str) -> Result<usize, ToolError> {
    let archive_s = archive.to_string_lossy();
    let output = if url.ends_with(".zip") {
        // `unzip -Z -1` prints one entry per line; trailing `/` indicates a dir.
        std::process::Command::new("unzip")
            .args(["-Z", "-1", &archive_s])
            .output()
    } else {
        // `tar -tzf` / `tar -tJf` prints one entry per line; dirs end with `/`.
        let flag = if url.ends_with(".tar.xz") || url.ends_with(".txz") {
            "-tJf"
        } else {
            "-tzf"
        };
        std::process::Command::new("tar")
            .args([flag, &archive_s])
            .output()
    };
    let output = output.map_err(|e| ToolError::Sdk {
        sdk_kind: "internal_error".to_string(),
        message: format!("list archive: {e}"),
    })?;
    if !output.status.success() {
        return Err(ToolError::Sdk {
            sdk_kind: "internal_error".to_string(),
            message: format!("archive listing failed (exit {})", output.status),
        });
    }
    let listing = String::from_utf8_lossy(&output.stdout);
    let count = listing
        .lines()
        .filter(|line| !line.is_empty())
        .filter(|line| !line.ends_with('/'))
        .count();
    Ok(count)
}

/// Recursive helper for `extract_archive`: walk `dir` and verify every
/// entry canonicalizes under `canonical_root`. Returns the count of
/// regular files encountered (used by `extract_archive` to detect partial
/// extraction).
///
/// Rejects any symlink anywhere in the tree as a defense-in-depth measure
/// (archives that add symlinks pointing outside are another escape vector
/// even when the symlink itself is inside the tree).
///
/// lab-zxx5.31: fails CLOSED on `symlink_metadata` errors. The previous
/// `Err(_) => continue` silently skipped entries we couldn't stat, meaning
/// an attacker-crafted permission-denied entry could evade rejection. A
/// stat failure in a security-critical walk is treated as a refusal to
/// validate the tree.
fn validate_no_escape(canonical_root: &Path, dir: &Path) -> Result<usize, ToolError> {
    let rd = std::fs::read_dir(dir).map_err(|e| ToolError::Sdk {
        sdk_kind: "internal_error".to_string(),
        message: format!("walk extract dir {}: {e}", dir.display()),
    })?;
    let mut file_count: usize = 0;
    for entry in rd.flatten() {
        let path = entry.path();
        let meta = std::fs::symlink_metadata(&path).map_err(|e| ToolError::Sdk {
            sdk_kind: "internal_error".to_string(),
            message: format!(
                "stat {} during extract walk (failing closed): {e}",
                path.display()
            ),
        })?;
        if meta.file_type().is_symlink() {
            return Err(ToolError::Sdk {
                sdk_kind: "path_traversal_rejected".to_string(),
                message: format!(
                    "archive contains symlink at `{}`; rejected (zip-slip defense)",
                    path.display()
                ),
            });
        }
        // canonicalize resolves any remaining non-symlink indirection and
        // lets us prefix-match against the root.
        let canon = std::fs::canonicalize(&path).map_err(|e| ToolError::Sdk {
            sdk_kind: "internal_error".to_string(),
            message: format!("canonicalize {}: {e}", path.display()),
        })?;
        if !canon.starts_with(canonical_root) {
            return Err(ToolError::Sdk {
                sdk_kind: "path_traversal_rejected".to_string(),
                message: format!(
                    "archive entry `{}` escapes extract root `{}`",
                    canon.display(),
                    canonical_root.display()
                ),
            });
        }
        if meta.file_type().is_dir() {
            file_count += validate_no_escape(canonical_root, &path)?;
        } else if meta.file_type().is_file() {
            file_count += 1;
        }
    }
    Ok(file_count)
}

/// Walk `dir` recursively to find the first file whose name matches `binary_name`.
fn find_binary_in_dir(dir: &Path, binary_name: &str) -> Option<PathBuf> {
    let entries = std::fs::read_dir(dir).ok()?;
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            if let Some(found) = find_binary_in_dir(&path, binary_name) {
                return Some(found);
            }
        } else if path.file_name().and_then(|n| n.to_str()) == Some(binary_name) {
            return Some(path);
        }
    }
    None
}

// ---------------------------------------------------------------------------
// agent.uninstall
// ---------------------------------------------------------------------------

fn dispatch_uninstall(id: &str) -> Result<Value, ToolError> {
    let removed = remove_provider(id)?;
    Ok(serde_json::json!({
        "id": id,
        "removed": removed,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validate_agent_id_for_path_accepts_safe_ids() {
        for id in ["codex-acp", "agent_1", "zed.agent", "A1-b_2.c3"] {
            validate_agent_id_for_path(id).expect(id);
        }
    }

    #[test]
    fn validate_agent_id_for_path_rejects_path_and_control_ids() {
        let too_long = "a".repeat(129);
        let invalid_ids = [
            "",
            ".",
            "..",
            "../escape",
            "escape/child",
            "escape\\child",
            "/absolute",
            ".hidden",
            "-leading-dash",
            "_leading-underscore",
            "contains space",
            "contains\nnewline",
            "éclair",
            too_long.as_str(),
        ];
        for id in invalid_ids {
            let err = validate_agent_id_for_path(id).expect_err(id);
            assert_eq!(err.kind(), "invalid_param");
        }
    }

    #[test]
    fn acp_dispatch_observability_uses_stable_dispatch_fields() {
        let source = include_str!("acp_dispatch.rs");

        for required in [
            "event = \"acp.dispatch.finished\"",
            "event = \"acp.dispatch.failed\"",
            "surface = \"mcp\"",
            "service = \"marketplace\"",
            "elapsed_ms",
            "kind = error.kind()",
        ] {
            assert!(source.contains(required), "missing {required}");
        }
    }

    #[cfg(feature = "acp_registry")]
    #[test]
    fn binary_asset_without_integrity_metadata_fails_closed() {
        let asset = BinaryAsset {
            archive: "https://example.com/agent.tar.gz".to_string(),
            sha256: None,
            digest: None,
            cmd: "./agent".to_string(),
            args: Vec::new(),
        };

        let err = expected_archive_sha256(&asset).expect_err("missing integrity must fail");

        assert_eq!(err.kind(), "integrity_missing");
    }

    #[cfg(feature = "acp_registry")]
    #[test]
    fn binary_asset_digest_metadata_is_normalized() {
        let asset = BinaryAsset {
            archive: "https://example.com/agent.tar.gz".to_string(),
            sha256: None,
            digest: Some(format!("sha256:{}", "A".repeat(64))),
            cmd: "./agent".to_string(),
            args: Vec::new(),
        };

        let expected = expected_archive_sha256(&asset).expect("valid digest");

        assert_eq!(expected, "a".repeat(64));
    }

    #[cfg(feature = "acp_registry")]
    #[test]
    fn digest_mismatch_verification_helper_fails() {
        let expected = "a".repeat(64);
        let actual = "b".repeat(64);

        let err = verify_archive_sha256(&expected, &actual).expect_err("mismatch must fail");

        assert_eq!(err.kind(), "integrity_mismatch");
    }

    #[test]
    fn archive_url_rejects_local_and_private_hosts() {
        for url in [
            "http://example.com/agent.tar.gz",
            "https://agent.local/agent.tar.gz",
            "https://127.0.0.1/agent.tar.gz",
            "https://[::ffff:127.0.0.1]/agent.tar.gz",
        ] {
            let err = validate_archive_url(url).expect_err(url);
            assert_eq!(err.kind(), "invalid_param");
        }

        for url in ["https://192.168.1.20/agent.tar.gz"] {
            let err = validate_archive_url(url).expect_err(url);
            assert_eq!(err.kind(), "ssrf_blocked");
        }
    }

    #[test]
    fn archive_resolved_addresses_reject_private_and_rebound_ranges() {
        for ip in [
            "10.0.0.1",
            "169.254.169.254",
            "100.64.0.1",
            "::1",
            "fc00::1",
            "fe80::1",
            "::ffff:169.254.169.254",
        ] {
            let err = check_archive_ip_not_private(ip.parse().expect(ip), "registry.example.com")
                .expect_err(ip);

            assert_eq!(err.kind(), "ssrf_blocked");
        }
    }

    #[test]
    fn archive_size_limit_rejects_oversized_stream() {
        let mut downloaded = MAX_ACP_ARCHIVE_BYTES - 1;
        let err =
            enforce_archive_size_limit(&mut downloaded, 2, "https://example.com/agent.tar.gz")
                .expect_err("oversized archive must fail");

        assert_eq!(err.kind(), "content_too_large");
        assert_eq!(downloaded, MAX_ACP_ARCHIVE_BYTES - 1);
    }

    #[tokio::test]
    async fn oversized_archive_cleanup_removes_partial_file() {
        let dir = tempfile::tempdir().expect("tempdir");
        let partial = dir.path().join("agent.tar.gz.partial");
        tokio::fs::write(&partial, b"partial archive")
            .await
            .expect("write partial");

        cleanup_partial_archive(&partial, "test.cleanup").await;

        assert!(
            tokio::fs::metadata(&partial).await.is_err(),
            "partial archive should be removed"
        );
    }

    #[cfg(feature = "acp_registry")]
    #[test]
    fn install_executable_atomically_replaces_file_contents() {
        let dir = tempfile::tempdir().expect("tempdir");
        let src = dir.path().join("src-agent");
        let dest = dir.path().join("agent");
        std::fs::write(&src, b"new agent").expect("write src");
        std::fs::write(&dest, b"old agent").expect("write dest");

        install_executable_atomically(&src, &dest).expect("atomic install");

        assert_eq!(std::fs::read(&dest).expect("read dest"), b"new agent");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&dest)
                .expect("metadata")
                .permissions()
                .mode()
                & 0o777;
            assert_eq!(mode, 0o755);
        }
    }

    #[test]
    fn provider_config_is_written_after_binary_install_returns() {
        let source = include_str!("acp_dispatch.rs");
        let install_pos = source
            .find("let (cmd_path, digest) = install_binary(agent_id, asset).await?;")
            .expect("install_binary call");
        let upsert_pos = source
            .find("upsert_provider(&entry)?;")
            .expect("upsert_provider call");

        assert!(
            install_pos < upsert_pos,
            "provider config must be written only after install_binary succeeds"
        );
    }

    #[test]
    fn archive_download_client_disables_proxy_dns_bypass() {
        let source = include_str!("acp_dispatch.rs");
        let no_proxy_pos = source.find(".no_proxy()").expect("no_proxy hardening");
        let pin_pos = source
            .find(".resolve_to_addrs(host, &addrs)")
            .expect("resolved address pinning");

        assert!(
            no_proxy_pos < pin_pos,
            "archive downloads must disable proxies before pinning resolved addresses"
        );
    }
}
