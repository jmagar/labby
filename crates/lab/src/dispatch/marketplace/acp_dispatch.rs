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
use std::path::PathBuf;
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
use lab_apis::acp_registry::installer::{AcpInstaller, InstallSpec};
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

/// Orchestrate a binary agent install to `~/.lab/bin/<agent_id>/`.
///
/// All download/verify/extract/install primitives (and the security guards
/// kept next to them — SSRF pinning, size cap, mandatory SHA-256, zip-slip
/// rejection, setuid strip) live in `lab_apis::acp_registry::installer`. This
/// function is a thin orchestrator: it resolves the integrity digest and
/// install directory, serializes concurrent installs of the same agent, builds
/// the [`InstallSpec`], delegates to the installer, and maps the error.
///
/// Returns `(installed_path, sha256_hex)`.
#[cfg(feature = "acp_registry")]
async fn install_binary(
    agent_id: &str,
    asset: &BinaryAsset,
) -> Result<(PathBuf, String), ToolError> {
    let expected_sha256 = expected_archive_sha256(asset)?;
    // Fail fast on a bad URL before taking the per-agent install lock.
    AcpInstaller::validate_archive_url(&asset.archive)?;

    let install_lock = binary_install_lock(agent_id)?;
    let _install_guard = install_lock.lock().await;

    let install_dir = agent_bin_dir(agent_id)?;

    let spec = InstallSpec {
        archive_url: asset.archive.clone(),
        expected_sha256,
        cmd: asset.cmd.clone(),
        install_dir,
    };

    let outcome = AcpInstaller::new().install(&spec).await?;
    Ok((outcome.installed_path, outcome.sha256))
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

/// Resolve the mandatory archive integrity digest, failing closed with
/// `integrity_missing` if the registry provides neither `sha256` nor `digest`.
/// Normalization (64-hex / `sha256:<hex>` validation) is delegated to the
/// installer so dispatch and the primitive share one parser.
#[cfg(feature = "acp_registry")]
fn expected_archive_sha256(asset: &BinaryAsset) -> Result<String, ToolError> {
    if let Some(value) = asset.sha256.as_deref() {
        return Ok(AcpInstaller::normalize_sha256(value, "sha256")?);
    }
    if let Some(value) = asset.digest.as_deref() {
        return Ok(AcpInstaller::normalize_sha256(value, "digest")?);
    }
    Err(ToolError::Sdk {
        sdk_kind: "integrity_missing".to_string(),
        message: format!(
            "binary archive `{}` has no SHA-256 integrity metadata; refusing install",
            asset.archive
        ),
    })
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

    // NB: The deep download/verify/extract/install guard tests (SSRF address
    // filtering, mandatory SHA-256, size cap, zip-slip/symlink rejection,
    // setuid strip, atomic install, peer re-validation) moved with the
    // primitive into `lab_apis::acp_registry::installer` and
    // `lab_apis::acp_registry::ssrf`. They are exercised there. The tests
    // below cover the orchestrator's remaining responsibilities.

    // The integrity-missing fail-closed gate stays in dispatch because it
    // decides between `integrity_missing` and delegating to the installer's
    // normalizer.
    #[cfg(feature = "acp_registry")]
    #[test]
    fn archive_url_validation_delegates_to_installer() {
        // The orchestrator must front-validate the URL via the shared installer
        // guard so a bad URL fails before the install lock is taken.
        use lab_apis::acp_registry::AcpInstaller;

        let err =
            AcpInstaller::validate_archive_url("https://192.168.1.20/agent.tar.gz").unwrap_err();
        assert_eq!(err.kind(), "ssrf_blocked");

        let err =
            AcpInstaller::validate_archive_url("http://example.com/agent.tar.gz").unwrap_err();
        assert_eq!(err.kind(), "invalid_param");
    }

    #[test]
    fn install_binary_orchestrates_through_installer() {
        // The download/extract/install primitives now live in lab-apis; the
        // orchestrator must delegate to the installer rather than re-implement
        // them.
        let source = include_str!("acp_dispatch.rs");
        assert!(
            source.contains("AcpInstaller::new().install(&spec)"),
            "install_binary must delegate to the lab-apis installer"
        );
        // Guard against the moved primitives drifting back in. The needles are
        // assembled at runtime so the literal `fn <name>(` strings never appear
        // verbatim in this file (which would make the scan match its own body).
        for name in [
            "download_archive",
            "extract_archive",
            "validate_no_escape",
            "install_executable_atomically",
            "check_archive_ip_not_private",
        ] {
            let needle = format!("fn {name}(");
            assert!(
                !source.contains(&needle),
                "primitive `{needle}` must move to lab-apis, not stay in dispatch"
            );
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
}
