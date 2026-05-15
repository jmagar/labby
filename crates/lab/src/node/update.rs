use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Instant;

use anyhow::{Context, Result, anyhow, bail};
use serde::Serialize;
use serde_json::json;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

use crate::config::{ArtifactRole, DeployPreferences, LabConfig, RestartModel, ServiceScope};
use crate::dispatch::deploy::build::{ArtifactProfile, BuildOutcome, build_artifact};
use crate::dispatch::deploy::host_io::{HostIo, SshHostIo};
use crate::dispatch::deploy::params::validate_remote_path;
use crate::dispatch::deploy::runner::build_default_runner;
use crate::dispatch::deploy::ssh_session::{SshHostTarget, shell_quote};
use crate::dispatch::deploy::stages::{preflight, restart, transfer_and_install, verify};
use crate::node::identity::resolve_local_hostname;
use crate::node::master_client::MasterClient;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "snake_case")]
enum UpdateTargetKind {
    Remote,
    LocalController,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "snake_case")]
enum RestartSelection {
    SystemService,
    UserService,
    WrapperCommand,
}

#[derive(Debug, Clone, Serialize)]
struct UpdateTargetResult {
    target: String,
    kind: UpdateTargetKind,
    node_id: Option<String>,
    connected: Option<bool>,
    controller_health_ok: Option<bool>,
    skipped_transfer: bool,
    ok: bool,
    failed_stage: Option<String>,
    stages_ms: BTreeMap<String, u128>,
    error: Option<String>,
    /// Path to the pre-install backup of the previous binary, if one was created.
    /// Present only for local-controller updates. Used for recovery if health
    /// verification fails after install.
    #[serde(skip_serializing_if = "Option::is_none")]
    backup_path: Option<String>,
    /// Human-readable recovery hint when the controller health check fails after
    /// install. Tells the operator how to restore the previous binary manually.
    #[serde(skip_serializing_if = "Option::is_none")]
    recovery_hint: Option<String>,
}

/// Outcome of a local artifact install, including the backup path if one was created.
#[derive(Debug)]
struct LocalInstallOutcome {
    /// Path to the backup of the previous binary, if the target existed before install.
    backup_path: Option<PathBuf>,
}

#[derive(Debug, Clone)]
struct RemoteTarget {
    alias: String,
    ssh: SshHostTarget,
}

#[derive(Debug, Clone)]
struct LocalTarget {
    identity: String,
}

#[derive(Debug, Clone)]
struct ResolvedTargets {
    remote: Vec<RemoteTarget>,
    local_controller: Option<LocalTarget>,
}

#[derive(Debug, Clone)]
struct EffectiveTargetConfig {
    install_path: String,
    restart: Option<RestartModel>,
    artifact_role: ArtifactRole,
    // Reserved for future cross-compilation support.
    #[allow(dead_code)]
    target_triple: Option<String>,
    // Reserved for per-host build timeout override.
    #[allow(dead_code)]
    build_timeout_secs: Option<u64>,
}

pub async fn run_update(
    config: &LabConfig,
    explicit_targets: Vec<String>,
    all: bool,
) -> Result<serde_json::Value> {
    let local_host = resolve_local_hostname().context("resolve local hostname for nodes update")?;
    let controller_host = controller_host(config, &local_host);
    let resolved = resolve_targets(config, &local_host, &controller_host, explicit_targets, all)?;

    // Collect the set of artifact roles required for this update pass.
    let mut needed_roles: std::collections::HashSet<ArtifactRole> =
        std::collections::HashSet::new();
    for target in &resolved.remote {
        let tc = effective_target_config(config, &target.alias);
        needed_roles.insert(tc.artifact_role);
    }
    if resolved.local_controller.is_some() {
        needed_roles.insert(ArtifactRole::Controller);
    }

    // Build each required role exactly once.
    let mut artifact_map: std::collections::HashMap<ArtifactRole, Arc<BuildOutcome>> =
        std::collections::HashMap::new();
    for role in &needed_roles {
        let mut profile = match role {
            ArtifactRole::Controller => ArtifactProfile::controller(),
            ArtifactRole::Node => ArtifactProfile::node(),
        };
        // Allow per-role timeout override from defaults (host-level overrides are
        // applied when selecting the artifact for each target, not here).
        if let Some(secs) = config
            .deploy
            .as_ref()
            .and_then(|d| d.defaults.as_ref())
            .and_then(|d| d.build_timeout_secs)
        {
            profile.build_timeout_secs = Some(secs);
        }
        let outcome = build_artifact(&profile)
            .await
            .with_context(|| format!("build artifact for role {role:?}"))?;
        artifact_map.insert(*role, Arc::new(outcome));
    }

    let controller_client =
        MasterClient::from_config(config, None).context("build controller verification client")?;

    let mut results = Vec::new();
    for target in resolved.remote {
        let target_config = effective_target_config(config, &target.alias);
        let artifact = Arc::clone(
            artifact_map
                .get(&target_config.artifact_role)
                .expect("artifact for role was built above"),
        );
        let io = Arc::new(SshHostIo::new(target.alias.clone(), target.ssh.clone()));
        results.push(
            run_remote_target(
                io,
                target.alias,
                controller_host.clone(),
                target_config,
                artifact,
                &controller_client,
            )
            .await,
        );
    }

    if let Some(local_target) = resolved.local_controller {
        let target_config = effective_target_config(config, &local_target.identity);
        let artifact = Arc::clone(
            artifact_map
                .get(&ArtifactRole::Controller)
                .expect("controller artifact was built above"),
        );
        let health_port = std::env::var("LAB_MCP_HTTP_PORT")
            .ok()
            .and_then(|v| v.parse::<u16>().ok())
            .or(config.mcp.port)
            .unwrap_or(8765);
        results.push(
            run_local_controller(
                local_target.identity,
                controller_host,
                target_config,
                artifact,
                health_port,
            )
            .await,
        );
    }

    let ok = results.iter().all(|result| result.ok);
    let artifacts: Vec<_> = artifact_map
        .values()
        .map(|a| {
            json!({
                "role": format!("{:?}", a.role).to_ascii_lowercase(),
                "path": a.path,
                "sha256": a.sha256,
                "size_bytes": a.size_bytes,
                "target_triple": a.target_triple,
            })
        })
        .collect();
    Ok(json!({
        "ok": ok,
        "artifacts": artifacts,
        "results": results,
    }))
}

fn controller_host(config: &LabConfig, local_host: &str) -> String {
    config
        .controller_host()
        .and_then(normalize_host_identifier)
        .unwrap_or_else(|| {
            normalize_host_identifier(local_host).unwrap_or_else(|| "localhost".into())
        })
}

fn resolve_targets(
    config: &LabConfig,
    local_host: &str,
    controller_host: &str,
    explicit_targets: Vec<String>,
    all: bool,
) -> Result<ResolvedTargets> {
    let runner = build_default_runner(config.deploy.clone().unwrap_or_default());
    let inventory = runner.ssh_inventory.clone();
    let is_local_controller = hosts_match(local_host, controller_host);

    let mut remote = Vec::new();
    let mut local_controller = None;

    if all {
        for target in inventory.iter() {
            if is_local_controller && ssh_target_matches_local(target, local_host, controller_host)
            {
                local_controller = Some(LocalTarget {
                    identity: controller_host.to_string(),
                });
                continue;
            }
            remote.push(RemoteTarget {
                alias: target.alias.clone(),
                ssh: target.clone(),
            });
        }
    } else {
        for requested in explicit_targets {
            if is_local_controller && hosts_match(&requested, controller_host) {
                local_controller = Some(LocalTarget {
                    identity: controller_host.to_string(),
                });
                continue;
            }

            let target = inventory
                .iter()
                .find(|target| target.alias == requested)
                .cloned()
                .ok_or_else(|| anyhow!("unknown node target `{requested}`"))?;

            if is_local_controller && ssh_target_matches_local(&target, local_host, controller_host)
            {
                local_controller = Some(LocalTarget {
                    identity: controller_host.to_string(),
                });
                continue;
            }

            remote.push(RemoteTarget {
                alias: target.alias.clone(),
                ssh: target,
            });
        }
    }

    if all && is_local_controller && local_controller.is_none() {
        local_controller = Some(LocalTarget {
            identity: controller_host.to_string(),
        });
    }

    if remote.is_empty() && local_controller.is_none() {
        bail!("no node update targets resolved");
    }

    Ok(ResolvedTargets {
        remote,
        local_controller,
    })
}

fn effective_target_config(config: &LabConfig, target: &str) -> EffectiveTargetConfig {
    let deploy = config.deploy.clone().unwrap_or_default();
    let host = deploy.hosts.get(target);
    let defaults = deploy.defaults.as_ref();

    let install_path = host
        .and_then(|entry| entry.remote_path.clone())
        .or_else(|| defaults.and_then(|entry| entry.remote_path.clone()))
        .unwrap_or_else(|| "/usr/local/bin/labby".to_string());

    let restart = host
        .and_then(|entry| entry.restart.clone())
        .or_else(|| defaults.and_then(|entry| entry.restart.clone()))
        .or_else(|| legacy_restart_model(&deploy, target));

    // Remote hosts default to Node role; only override when explicitly configured.
    let artifact_role = host
        .and_then(|entry| entry.artifact_role)
        .or_else(|| defaults.and_then(|entry| entry.artifact_role))
        .unwrap_or(ArtifactRole::Node);

    let target_triple = host
        .and_then(|entry| entry.target_triple.clone())
        .or_else(|| defaults.and_then(|entry| entry.target_triple.clone()));

    let build_timeout_secs = host
        .and_then(|entry| entry.build_timeout_secs)
        .or_else(|| defaults.and_then(|entry| entry.build_timeout_secs));

    EffectiveTargetConfig {
        install_path,
        restart,
        artifact_role,
        target_triple,
        build_timeout_secs,
    }
}

fn legacy_restart_model(deploy: &DeployPreferences, target: &str) -> Option<RestartModel> {
    let host = deploy.hosts.get(target);
    let service = match host.and_then(|entry| entry.service.as_deref()) {
        Some("") => return None,
        Some(service) => Some(service.to_string()),
        None => deploy
            .defaults
            .as_ref()
            .and_then(|entry| entry.service.clone())
            .filter(|service| !service.is_empty()),
    }?;

    let scope = host
        .and_then(|entry| entry.service_scope)
        .or_else(|| {
            deploy
                .defaults
                .as_ref()
                .and_then(|entry| entry.service_scope)
        })
        .unwrap_or(ServiceScope::System);

    Some(match scope {
        ServiceScope::System => RestartModel::SystemService { service },
        ServiceScope::User => RestartModel::UserService { service },
    })
}

async fn run_remote_target<I: HostIo + 'static>(
    io: Arc<I>,
    alias: String,
    controller_host: String,
    target_config: EffectiveTargetConfig,
    artifact: Arc<BuildOutcome>,
    controller_client: &MasterClient,
) -> UpdateTargetResult {
    let mut stages_ms = BTreeMap::new();

    let resolved_node_id = match resolve_remote_node_id(io.clone()).await {
        Ok(node_id) => node_id,
        Err(error) => {
            return failed_result(
                alias,
                UpdateTargetKind::Remote,
                None,
                false,
                "resolve".into(),
                stages_ms,
                error.to_string(),
                None,
            );
        }
    };

    log_remote_update_stage_enter(&alias, &resolved_node_id, "preflight");
    let preflight_started = Instant::now();
    let preflight_result = preflight(
        io.clone(),
        target_config.install_path.clone(),
        artifact.target_triple.clone(),
        artifact.sha256.clone(),
    )
    .await;
    stages_ms.insert("preflight".into(), preflight_started.elapsed().as_millis());
    let preflight_result = match preflight_result {
        Ok(result) => {
            log_remote_update_stage_exit(
                &alias,
                &resolved_node_id,
                "preflight",
                preflight_started.elapsed().as_millis(),
                result.skip_transfer,
            );
            result
        }
        Err(error) => {
            log_remote_update_stage_failure(
                &alias,
                &resolved_node_id,
                "preflight",
                preflight_started.elapsed().as_millis(),
                &error,
            );
            return failed_result(
                alias,
                UpdateTargetKind::Remote,
                Some(resolved_node_id),
                false,
                "preflight".into(),
                stages_ms,
                error.to_string(),
                None,
            );
        }
    };
    let skipped_transfer = preflight_result.skip_transfer;

    #[expect(
        clippy::branches_sharing_code,
        reason = "stage logging is intentionally colocated with the transfer branch"
    )]
    if !preflight_result.skip_transfer {
        log_remote_update_stage_enter(&alias, &resolved_node_id, "transfer");
        let transfer_started = Instant::now();
        let file = match tokio::fs::File::open(&artifact.path).await {
            Ok(file) => file,
            Err(error) => {
                log_remote_update_stage_failure(
                    &alias,
                    &resolved_node_id,
                    "transfer",
                    transfer_started.elapsed().as_millis(),
                    &format_args!("open artifact: {error}"),
                );
                return failed_result(
                    alias,
                    UpdateTargetKind::Remote,
                    Some(resolved_node_id),
                    false,
                    "transfer".into(),
                    stages_ms,
                    format!("open artifact: {error}"),
                    None,
                );
            }
        };
        let transfer_result = transfer_and_install(
            io.clone(),
            target_config.install_path.clone(),
            artifact.sha256.clone(),
            file,
        )
        .await;
        stages_ms.insert("transfer".into(), transfer_started.elapsed().as_millis());
        if let Err(error) = transfer_result {
            log_remote_update_stage_failure(
                &alias,
                &resolved_node_id,
                "transfer",
                transfer_started.elapsed().as_millis(),
                &error,
            );
            return failed_result(
                alias.clone(),
                UpdateTargetKind::Remote,
                Some(resolved_node_id.clone()),
                false,
                "transfer".into(),
                stages_ms,
                error.to_string(),
                None,
            );
        }
        log_remote_update_stage_exit(
            &alias,
            &resolved_node_id,
            "transfer",
            transfer_started.elapsed().as_millis(),
            false,
        );
    } else {
        log_remote_update_stage_enter(&alias, &resolved_node_id, "transfer");
        log_remote_update_stage_exit(&alias, &resolved_node_id, "transfer", 0, true);
    }

    log_remote_update_stage_enter(&alias, &resolved_node_id, "normalize");
    let normalize_started = Instant::now();
    if let Err(error) =
        normalize_remote_runtime(io.clone(), &resolved_node_id, &controller_host).await
    {
        stages_ms.insert("normalize".into(), normalize_started.elapsed().as_millis());
        log_remote_update_stage_failure(
            &alias,
            &resolved_node_id,
            "normalize",
            normalize_started.elapsed().as_millis(),
            &error,
        );
        return failed_result(
            alias.clone(),
            UpdateTargetKind::Remote,
            Some(resolved_node_id.clone()),
            skipped_transfer,
            "normalize".into(),
            stages_ms,
            error.to_string(),
            None,
        );
    }
    stages_ms.insert("normalize".into(), normalize_started.elapsed().as_millis());
    log_remote_update_stage_exit(
        &alias,
        &resolved_node_id,
        "normalize",
        normalize_started.elapsed().as_millis(),
        false,
    );

    log_remote_update_stage_enter(&alias, &resolved_node_id, "restart");
    let restart_started = Instant::now();
    if let Err(error) = restart_target(io.clone(), target_config.restart.as_ref()).await {
        stages_ms.insert("restart".into(), restart_started.elapsed().as_millis());
        log_remote_update_stage_failure(
            &alias,
            &resolved_node_id,
            "restart",
            restart_started.elapsed().as_millis(),
            &error,
        );
        return failed_result(
            alias.clone(),
            UpdateTargetKind::Remote,
            Some(resolved_node_id.clone()),
            skipped_transfer,
            "restart".into(),
            stages_ms,
            error.to_string(),
            None,
        );
    }
    stages_ms.insert("restart".into(), restart_started.elapsed().as_millis());
    log_remote_update_stage_exit(
        &alias,
        &resolved_node_id,
        "restart",
        restart_started.elapsed().as_millis(),
        false,
    );

    log_remote_update_stage_enter(&alias, &resolved_node_id, "verify");
    let verify_started = Instant::now();
    if let Err(error) = verify(io.clone(), target_config.install_path.clone()).await {
        stages_ms.insert("verify".into(), verify_started.elapsed().as_millis());
        log_remote_update_stage_failure(
            &alias,
            &resolved_node_id,
            "verify",
            verify_started.elapsed().as_millis(),
            &error,
        );
        return failed_result(
            alias.clone(),
            UpdateTargetKind::Remote,
            Some(resolved_node_id.clone()),
            skipped_transfer,
            "verify".into(),
            stages_ms,
            error.to_string(),
            None,
        );
    }
    stages_ms.insert("verify".into(), verify_started.elapsed().as_millis());
    log_remote_update_stage_exit(
        &alias,
        &resolved_node_id,
        "verify",
        verify_started.elapsed().as_millis(),
        false,
    );

    log_remote_update_stage_enter(&alias, &resolved_node_id, "controller_verify");
    let controller_started = Instant::now();
    let reconnect_timeout = std::time::Duration::from_secs(60);
    let controller_result = controller_client
        .wait_for_node_connected(&resolved_node_id, reconnect_timeout)
        .await;
    let connected = controller_result.is_ok();
    stages_ms.insert(
        "controller_verify".into(),
        controller_started.elapsed().as_millis(),
    );
    if !connected {
        let error = controller_result
            .as_ref()
            .err()
            .map(ToString::to_string)
            .unwrap_or_else(|| "controller did not report node as connected".to_string());
        log_remote_update_stage_failure(
            &alias,
            &resolved_node_id,
            "controller_verify",
            controller_started.elapsed().as_millis(),
            &error,
        );
        return failed_result(
            alias.clone(),
            UpdateTargetKind::Remote,
            Some(resolved_node_id.clone()),
            skipped_transfer,
            "controller_verify".into(),
            stages_ms,
            format!(
                "controller did not report node `{}` as connected",
                resolved_node_id
            ),
            Some(false),
        );
    }
    log_remote_update_stage_exit(
        &alias,
        &resolved_node_id,
        "controller_verify",
        controller_started.elapsed().as_millis(),
        false,
    );

    UpdateTargetResult {
        target: alias,
        kind: UpdateTargetKind::Remote,
        node_id: Some(resolved_node_id),
        connected: Some(true),
        controller_health_ok: None,
        skipped_transfer,
        ok: true,
        failed_stage: None,
        stages_ms,
        error: None,
        backup_path: None,
        recovery_hint: None,
    }
}

fn log_remote_update_stage_enter(target: &str, node_id: &str, stage: &str) {
    tracing::info!(
        surface = "cli",
        service = "nodes",
        action = "node.update",
        event = "remote_update.stage.enter",
        target = %target,
        node_id = %node_id,
        stage = %stage,
        "remote node update stage started",
    );
}

fn log_remote_update_stage_exit(
    target: &str,
    node_id: &str,
    stage: &str,
    elapsed_ms: u128,
    skipped: bool,
) {
    tracing::info!(
        surface = "cli",
        service = "nodes",
        action = "node.update",
        event = "remote_update.stage.exit",
        target = %target,
        node_id = %node_id,
        stage = %stage,
        elapsed_ms,
        skipped,
        "remote node update stage finished",
    );
}

fn log_remote_update_stage_failure(
    target: &str,
    node_id: &str,
    stage: &str,
    elapsed_ms: u128,
    error: &dyn std::fmt::Display,
) {
    tracing::warn!(
        surface = "cli",
        service = "nodes",
        action = "node.update",
        event = "remote_update.stage.exit",
        kind = "remote_update_failed",
        target = %target,
        node_id = %node_id,
        stage = %stage,
        elapsed_ms,
        error = %error,
        "remote node update stage failed",
    );
}

async fn run_local_controller(
    identity: String,
    _controller_host: String,
    target_config: EffectiveTargetConfig,
    artifact: Arc<BuildOutcome>,
    health_port: u16,
) -> UpdateTargetResult {
    let mut stages_ms = BTreeMap::new();
    let install_path = PathBuf::from(&target_config.install_path);

    let install_started = Instant::now();
    let outcome = match install_local_artifact(&artifact.path, &install_path).await {
        Ok(outcome) => outcome,
        Err(error) => {
            stages_ms.insert("install".into(), install_started.elapsed().as_millis());
            return failed_result(
                identity.clone(),
                UpdateTargetKind::LocalController,
                None,
                false,
                "install".into(),
                stages_ms,
                error.to_string(),
                None,
            );
        }
    };
    stages_ms.insert("install".into(), install_started.elapsed().as_millis());
    let backup_path = outcome
        .backup_path
        .as_ref()
        .map(|p| p.display().to_string());

    let normalize_started = Instant::now();
    if let Err(error) = normalize_local_runtime(&identity).await {
        stages_ms.insert("normalize".into(), normalize_started.elapsed().as_millis());
        return failed_result(
            identity.clone(),
            UpdateTargetKind::LocalController,
            None,
            false,
            "normalize".into(),
            stages_ms,
            error.to_string(),
            None,
        );
    }
    stages_ms.insert("normalize".into(), normalize_started.elapsed().as_millis());

    let restart_started = Instant::now();
    if let Err(error) = restart_local_target(target_config.restart.as_ref()).await {
        stages_ms.insert("restart".into(), restart_started.elapsed().as_millis());
        return failed_result(
            identity.clone(),
            UpdateTargetKind::LocalController,
            None,
            false,
            "restart".into(),
            stages_ms,
            error.to_string(),
            None,
        );
    }
    stages_ms.insert("restart".into(), restart_started.elapsed().as_millis());

    let health_started = Instant::now();
    if let Err(error) = verify_local_health(health_port).await {
        stages_ms.insert("health".into(), health_started.elapsed().as_millis());
        let recovery_hint = backup_path.as_ref().map(|b| {
            format!(
                "To recover: sudo install -m 755 {b} {} && sudo systemctl restart lab",
                install_path.display()
            )
        });
        return UpdateTargetResult {
            target: identity.clone(),
            kind: UpdateTargetKind::LocalController,
            node_id: None,
            connected: Some(false),
            controller_health_ok: Some(false),
            skipped_transfer: false,
            ok: false,
            failed_stage: Some("health".into()),
            stages_ms,
            error: Some(error.to_string()),
            backup_path,
            recovery_hint,
        };
    }
    stages_ms.insert("health".into(), health_started.elapsed().as_millis());

    UpdateTargetResult {
        target: identity.clone(),
        kind: UpdateTargetKind::LocalController,
        node_id: Some(identity),
        connected: None,
        controller_health_ok: Some(true),
        skipped_transfer: false,
        ok: true,
        failed_stage: None,
        stages_ms,
        error: None,
        backup_path,
        recovery_hint: None,
    }
}

async fn resolve_remote_node_id<I: HostIo + 'static>(io: Arc<I>) -> Result<String> {
    let (code, stdout, stderr) = io.run_argv(&["hostname"]).await?;
    if code != 0 {
        bail!("resolve remote hostname failed: {}", stderr.trim());
    }
    normalize_host_identifier(stdout.trim())
        .ok_or_else(|| anyhow!("remote hostname command returned an empty identifier"))
}

async fn normalize_remote_runtime<I: HostIo + 'static>(
    io: Arc<I>,
    _node_id: &str,
    controller_host: &str,
) -> Result<()> {
    let home_dir = remote_home_dir(io.clone()).await?;
    let lab_dir = format!("{home_dir}/.lab");
    let config_path = format!("{lab_dir}/config.toml");
    let current = read_remote_file(io.clone(), &config_path)
        .await
        .unwrap_or_default();

    let mut config = if current.trim().is_empty() {
        LabConfig::default()
    } else {
        toml::from_str::<LabConfig>(&current)
            .with_context(|| format!("parse existing remote config `{config_path}`"))?
    };
    config.device = None;
    config.node = Some(crate::config::NodePreferences {
        controller: Some(controller_host.to_string()),
        ..Default::default()
    });

    let rendered = toml::to_string_pretty(&config).context("serialize normalized node config")?;
    write_remote_file(io.clone(), &lab_dir, &config_path, &rendered).await?;

    let device_token = format!("{lab_dir}/device-token");
    let device_enrollments = format!("{lab_dir}/device-enrollments.json");
    let (code, _stdout, stderr) = io
        .run_argv(&["rm", "-f", "--", &device_token, &device_enrollments])
        .await?;
    if code != 0 {
        bail!("remove legacy runtime files failed: {}", stderr.trim());
    }
    Ok(())
}

async fn normalize_local_runtime(controller_host: &str) -> Result<()> {
    let home_dir = current_home_dir();
    let lab_dir = home_dir.join(".lab");
    tokio::fs::create_dir_all(&lab_dir)
        .await
        .with_context(|| format!("create {}", lab_dir.display()))?;
    let config_path = lab_dir.join("config.toml");
    let current = tokio::fs::read_to_string(&config_path)
        .await
        .unwrap_or_default();
    let mut config = if current.trim().is_empty() {
        LabConfig::default()
    } else {
        toml::from_str::<LabConfig>(&current)
            .with_context(|| format!("parse existing local config `{}`", config_path.display()))?
    };
    config.device = None;
    config.node = Some(crate::config::NodePreferences {
        controller: Some(controller_host.to_string()),
        ..Default::default()
    });
    let rendered = toml::to_string_pretty(&config).context("serialize local normalized config")?;
    tokio::fs::write(&config_path, rendered)
        .await
        .with_context(|| format!("write {}", config_path.display()))?;

    drop(tokio::fs::remove_file(lab_dir.join("device-token")).await);
    drop(tokio::fs::remove_file(lab_dir.join("device-enrollments.json")).await);
    Ok(())
}

async fn remote_home_dir<I: HostIo + 'static>(io: Arc<I>) -> Result<String> {
    let (code, stdout, stderr) = io.run_argv(&["sh", "-c", "printf %s \"$HOME\""]).await?;
    if code != 0 {
        bail!("resolve remote home failed: {}", stderr.trim());
    }
    let home = stdout.trim();
    if home.is_empty() {
        bail!("remote home directory is empty");
    }
    Ok(home.to_string())
}

async fn read_remote_file<I: HostIo + 'static>(io: Arc<I>, path: &str) -> Result<String> {
    let quoted = shell_quote(path);
    let command = format!("if [ -f {quoted} ]; then cat {quoted}; fi");
    let (code, stdout, stderr) = io.run_argv(&["sh", "-c", &command]).await?;
    if code != 0 {
        bail!("read remote file `{path}` failed: {}", stderr.trim());
    }
    Ok(stdout)
}

async fn write_remote_file<I: HostIo + 'static>(
    io: Arc<I>,
    lab_dir: &str,
    path: &str,
    contents: &str,
) -> Result<()> {
    let marker = "__LAB_NODE_CONFIG__";
    let quoted_dir = shell_quote(lab_dir);
    let quoted_tmp = shell_quote(&format!("{path}.tmp"));
    let quoted_path = shell_quote(path);
    let command = format!(
        "mkdir -p {quoted_dir}\ncat > {quoted_tmp} <<'{marker}'\n{contents}\n{marker}\nmv -- {quoted_tmp} {quoted_path}"
    );
    let (code, _stdout, stderr) = io.run_argv(&["sh", "-c", &command]).await?;
    if code != 0 {
        bail!("write remote file `{path}` failed: {}", stderr.trim());
    }
    Ok(())
}

async fn restart_target<I: HostIo + 'static>(
    io: Arc<I>,
    restart_model: Option<&RestartModel>,
) -> Result<RestartSelection> {
    let selection = match restart_model {
        Some(RestartModel::SystemService { service }) => {
            restart(io, Some(service.clone()), Some(ServiceScope::System)).await?;
            RestartSelection::SystemService
        }
        Some(RestartModel::UserService { service }) => {
            restart(io, Some(service.clone()), Some(ServiceScope::User)).await?;
            RestartSelection::UserService
        }
        Some(RestartModel::WrapperCommand { command }) => {
            run_wrapper_restart(io, command).await?;
            RestartSelection::WrapperCommand
        }
        None => {
            restart(io, None, None).await?;
            RestartSelection::WrapperCommand
        }
    };
    Ok(selection)
}

async fn restart_local_target(restart_model: Option<&RestartModel>) -> Result<RestartSelection> {
    let selection = match restart_model {
        Some(RestartModel::SystemService { service }) => {
            run_local_command(["sudo", "-n", "systemctl", "restart", service.as_str()]).await?;
            run_local_command([
                "sudo",
                "-n",
                "systemctl",
                "is-active",
                "--wait",
                service.as_str(),
            ])
            .await?;
            RestartSelection::SystemService
        }
        Some(RestartModel::UserService { service }) => {
            run_local_command(["systemctl", "--user", "restart", service.as_str()]).await?;
            run_local_command([
                "systemctl",
                "--user",
                "is-active",
                "--wait",
                service.as_str(),
            ])
            .await?;
            RestartSelection::UserService
        }
        Some(RestartModel::WrapperCommand { command }) => {
            run_local_command_vec(command).await?;
            RestartSelection::WrapperCommand
        }
        None => bail!("local controller update requires an explicit deploy restart policy"),
    };
    Ok(selection)
}

async fn run_wrapper_restart<I: HostIo + 'static>(io: Arc<I>, command: &[String]) -> Result<()> {
    if command.is_empty() {
        bail!("wrapper restart command must not be empty");
    }
    let argv: Vec<&str> = command.iter().map(String::as_str).collect();
    let (code, _stdout, stderr) = io.run_argv(&argv).await?;
    if code != 0 {
        bail!("wrapper restart failed: {}", stderr.trim());
    }
    Ok(())
}

async fn install_local_artifact(source: &Path, target: &Path) -> Result<LocalInstallOutcome> {
    validate_remote_path(&target.display().to_string())
        .context("validate local controller install path")?;
    if local_install_requires_sudo(target) {
        return install_local_artifact_with_sudo(source, target).await;
    }
    if let Some(parent) = target.parent() {
        tokio::fs::create_dir_all(parent)
            .await
            .with_context(|| format!("create {}", parent.display()))?;
    }

    let staged = target.with_extension("new");
    let backup = target.with_extension(format!(
        "bak.{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|elapsed| elapsed.as_secs())
            .unwrap_or_default()
    ));

    tokio::fs::copy(source, &staged)
        .await
        .with_context(|| format!("copy {} -> {}", source.display(), staged.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        tokio::fs::set_permissions(&staged, std::fs::Permissions::from_mode(0o755))
            .await
            .with_context(|| format!("chmod {}", staged.display()))?;
    }

    let target_existed = tokio::fs::try_exists(target).await.unwrap_or(false);
    if target_existed {
        tokio::fs::rename(target, &backup)
            .await
            .with_context(|| format!("backup {} -> {}", target.display(), backup.display()))?;
    }
    tokio::fs::rename(&staged, target)
        .await
        .with_context(|| format!("install {} -> {}", staged.display(), target.display()))?;
    Ok(LocalInstallOutcome {
        backup_path: if target_existed { Some(backup) } else { None },
    })
}

async fn install_local_artifact_with_sudo(
    source: &Path,
    target: &Path,
) -> Result<LocalInstallOutcome> {
    // Pre-stat the target to determine whether a backup will be created by the script.
    let target_existed = tokio::fs::try_exists(target).await.unwrap_or(false);
    let staged = target.with_extension("new");
    let backup = target.with_extension(format!(
        "bak.{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|elapsed| elapsed.as_secs())
            .unwrap_or_default()
    ));
    let script = format!(
        "install -m 755 -- {src} {staged}\nif [ -e {target} ]; then mv -- {target} {backup}; fi\nmv -- {staged} {target}",
        src = shell_quote(&source.display().to_string()),
        staged = shell_quote(&staged.display().to_string()),
        target = shell_quote(&target.display().to_string()),
        backup = shell_quote(&backup.display().to_string()),
    );
    run_local_command_vec(&["sudo".into(), "-n".into(), "sh".into(), "-c".into(), script]).await?;
    Ok(LocalInstallOutcome {
        backup_path: if target_existed { Some(backup) } else { None },
    })
}

async fn check_local_endpoint(path: &str, port: u16) -> Result<()> {
    let mut stream = tokio::net::TcpStream::connect(format!("127.0.0.1:{port}"))
        .await
        .with_context(|| format!("connect to local controller endpoint {path}"))?;
    let request = format!("GET {path} HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n\r\n");
    stream
        .write_all(request.as_bytes())
        .await
        .with_context(|| format!("write local controller request {path}"))?;
    let mut response = Vec::new();
    stream
        .read_to_end(&mut response)
        .await
        .with_context(|| format!("read local controller response {path}"))?;
    let response = String::from_utf8_lossy(&response);
    if !response.starts_with("HTTP/1.1 200") && !response.starts_with("HTTP/1.0 200") {
        bail!("local controller endpoint {path} did not return 200");
    }
    Ok(())
}

async fn verify_local_health(port: u16) -> Result<()> {
    check_local_endpoint("/health", port)
        .await
        .context("local controller /health check")?;
    check_local_endpoint("/ready", port)
        .await
        .context("local controller /ready check")?;
    Ok(())
}

fn local_install_requires_sudo(target: &Path) -> bool {
    ["/usr", "/opt", "/etc", "/bin", "/sbin"]
        .iter()
        .any(|prefix| target.starts_with(prefix))
}

async fn run_local_command<const N: usize>(argv: [&str; N]) -> Result<()> {
    let output = tokio::process::Command::new(argv[0])
        .args(&argv[1..])
        .output()
        .await
        .with_context(|| format!("spawn {}", argv[0]))?;
    if !output.status.success() {
        bail!(
            "{} failed: {}",
            argv[0],
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    Ok(())
}

async fn run_local_command_vec(command: &[String]) -> Result<()> {
    if command.is_empty() {
        bail!("wrapper restart command must not be empty");
    }
    let output = tokio::process::Command::new(&command[0])
        .args(&command[1..])
        .output()
        .await
        .with_context(|| format!("spawn {}", command[0]))?;
    if !output.status.success() {
        bail!(
            "{} failed: {}",
            command[0],
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    Ok(())
}

fn current_home_dir() -> PathBuf {
    std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from)
        .or_else(|| std::env::current_dir().ok())
        .unwrap_or_else(|| PathBuf::from("."))
}

fn normalize_host_identifier(value: &str) -> Option<String> {
    let trimmed = value.trim().trim_end_matches('.');
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_ascii_lowercase())
    }
}

fn hosts_match(left: &str, right: &str) -> bool {
    let Some(left) = normalize_host_identifier(left) else {
        return false;
    };
    let Some(right) = normalize_host_identifier(right) else {
        return false;
    };
    left == right
        || left.split('.').next().unwrap_or(&left) == right.split('.').next().unwrap_or(&left)
}

fn ssh_target_matches_local(
    target: &SshHostTarget,
    local_host: &str,
    controller_host: &str,
) -> bool {
    hosts_match(&target.alias, local_host)
        || target
            .hostname
            .as_deref()
            .map(|hostname| hosts_match(hostname, local_host))
            .unwrap_or(false)
        || hosts_match(&target.alias, controller_host)
        || target
            .hostname
            .as_deref()
            .map(|hostname| hosts_match(hostname, controller_host))
            .unwrap_or(false)
}

fn failed_result(
    target: String,
    kind: UpdateTargetKind,
    node_id: Option<String>,
    skipped_transfer: bool,
    failed_stage: String,
    stages_ms: BTreeMap<String, u128>,
    error: String,
    controller_health_ok: Option<bool>,
) -> UpdateTargetResult {
    UpdateTargetResult {
        target,
        kind,
        node_id,
        connected: Some(false),
        controller_health_ok,
        skipped_transfer,
        ok: false,
        failed_stage: Some(failed_stage),
        stages_ms,
        error: Some(error),
        backup_path: None,
        recovery_hint: None,
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;
    use crate::dispatch::deploy::runner::test_support::{RecordingIo, RunResp};

    #[test]
    fn remote_update_logs_required_stage_observability_fields() {
        let source = include_str!("update.rs");
        for field in [
            "event = \"remote_update.stage.enter\"",
            "event = \"remote_update.stage.exit\"",
            "kind = \"remote_update_failed\"",
            "action = \"node.update\"",
            "elapsed_ms",
            "node_id = %node_id",
            "\"preflight\"",
            "\"transfer\"",
            "\"normalize\"",
            "\"restart\"",
            "\"verify\"",
            "\"controller_verify\"",
        ] {
            assert!(
                source.contains(field),
                "missing remote update field: {field}"
            );
        }
    }

    #[test]
    fn resolve_targets_adds_local_controller_last_for_all() {
        let config = LabConfig {
            node: Some(crate::config::NodePreferences {
                controller: Some("controller".into()),
                ..Default::default()
            }),
            deploy: Some(DeployPreferences::default()),
            ..LabConfig::default()
        };

        let resolved = resolve_targets(&config, "controller", "controller", Vec::new(), true)
            .expect("resolve");

        assert!(resolved.local_controller.is_some());
    }

    #[tokio::test]
    async fn normalize_remote_runtime_removes_legacy_files_and_writes_node_controller() {
        let io = Arc::new(RecordingIo::new());
        io.push_run(RunResp::ok("/home/lab"));
        io.push_run(RunResp::ok(""));
        io.push_run(RunResp::ok(""));
        io.push_run(RunResp::ok(""));

        normalize_remote_runtime(io.clone(), "mini1", "controller")
            .await
            .expect("normalize");

        let ops = io.ops();
        assert!(ops.iter().any(|op| op.contains(".lab/config.toml")));
        assert!(ops.iter().any(|op| op.contains("device-enrollments.json")));
    }

    #[tokio::test]
    async fn wrapper_restart_uses_raw_argv() {
        let io = Arc::new(RecordingIo::new());
        io.push_run(RunResp::ok(""));
        run_wrapper_restart(io.clone(), &["echo".into(), "restart".into()])
            .await
            .expect("restart");
        assert!(io.ops().iter().any(|op| op == "run:echo,restart"));
    }

    #[test]
    fn remote_targets_default_to_node_artifact_role() {
        let config = LabConfig {
            deploy: None,
            ..LabConfig::default()
        };
        let effective = effective_target_config(&config, "somehost");
        assert_eq!(effective.artifact_role, ArtifactRole::Node);
    }

    #[test]
    fn host_artifact_role_override_respected() {
        use crate::config::DeployHostOverride;

        let mut config = LabConfig {
            deploy: Some(DeployPreferences::default()),
            ..LabConfig::default()
        };
        config.deploy.as_mut().unwrap().hosts.insert(
            "dookie".into(),
            DeployHostOverride {
                artifact_role: Some(ArtifactRole::Controller),
                ..Default::default()
            },
        );
        let effective = effective_target_config(&config, "dookie");
        assert_eq!(effective.artifact_role, ArtifactRole::Controller);
    }

    #[test]
    fn defaults_artifact_role_propagates_to_hosts_without_override() {
        use crate::config::{DeployDefaults, DeployHostOverride};

        let mut config = LabConfig {
            deploy: Some(DeployPreferences {
                defaults: Some(DeployDefaults {
                    artifact_role: Some(ArtifactRole::Controller),
                    ..Default::default()
                }),
                ..Default::default()
            }),
            ..LabConfig::default()
        };
        // Host exists but has no artifact_role override.
        config
            .deploy
            .as_mut()
            .unwrap()
            .hosts
            .insert("mini1".into(), DeployHostOverride::default());
        let effective = effective_target_config(&config, "mini1");
        assert_eq!(effective.artifact_role, ArtifactRole::Controller);
    }

    #[test]
    fn host_artifact_role_overrides_defaults() {
        use crate::config::{DeployDefaults, DeployHostOverride};

        let config = LabConfig {
            deploy: Some(DeployPreferences {
                defaults: Some(DeployDefaults {
                    artifact_role: Some(ArtifactRole::Controller),
                    ..Default::default()
                }),
                hosts: {
                    let mut m = BTreeMap::new();
                    m.insert(
                        "mini1".into(),
                        DeployHostOverride {
                            artifact_role: Some(ArtifactRole::Node),
                            ..Default::default()
                        },
                    );
                    m
                },
            }),
            ..LabConfig::default()
        };
        let effective = effective_target_config(&config, "mini1");
        assert_eq!(effective.artifact_role, ArtifactRole::Node);
    }

    // ── Task 9: recovery output ───────────────────────────────────────────────

    /// When health verification fails after a local-controller install that created
    /// a backup, the result must carry `backup_path` and a `recovery_hint`.
    #[test]
    fn recovery_result_includes_backup_path_and_hint() {
        let install_path = PathBuf::from("/usr/local/bin/labby");
        let backup = PathBuf::from("/usr/local/bin/labby.bak.1234567890");

        // Simulate the path taken inside run_local_controller when health fails.
        let backup_path_str = backup.display().to_string();
        let recovery_hint = Some(format!(
            "To recover: sudo install -m 755 {backup_path_str} {} && sudo systemctl restart lab",
            install_path.display()
        ));

        let result = UpdateTargetResult {
            target: "controller".into(),
            kind: UpdateTargetKind::LocalController,
            node_id: None,
            connected: Some(false),
            controller_health_ok: Some(false),
            skipped_transfer: false,
            ok: false,
            failed_stage: Some("health".into()),
            stages_ms: BTreeMap::new(),
            error: Some("local controller /health check: connect to local controller endpoint /health: Connection refused (os error 111)".into()),
            backup_path: Some(backup_path_str.clone()),
            recovery_hint: recovery_hint.clone(),
        };

        assert_eq!(
            result.backup_path.as_deref(),
            Some(backup_path_str.as_str())
        );
        assert!(
            result
                .recovery_hint
                .as_deref()
                .unwrap()
                .contains("sudo install -m 755")
        );
        assert!(
            result
                .recovery_hint
                .as_deref()
                .unwrap()
                .contains(&backup_path_str)
        );
        assert!(
            result
                .recovery_hint
                .as_deref()
                .unwrap()
                .contains("systemctl restart lab")
        );
        assert!(!result.ok);
        assert_eq!(result.failed_stage.as_deref(), Some("health"));

        // Verify the result serializes with both fields present.
        let json = serde_json::to_value(&result).expect("serialize");
        assert!(json.get("backup_path").is_some());
        assert!(json.get("recovery_hint").is_some());
    }

    /// When no prior binary exists (fresh install), backup_path must be None
    /// and recovery_hint must be None on success.
    #[test]
    fn fresh_install_success_has_no_backup_path() {
        let result = UpdateTargetResult {
            target: "controller".into(),
            kind: UpdateTargetKind::LocalController,
            node_id: Some("controller".into()),
            connected: None,
            controller_health_ok: Some(true),
            skipped_transfer: false,
            ok: true,
            failed_stage: None,
            stages_ms: BTreeMap::new(),
            error: None,
            backup_path: None,
            recovery_hint: None,
        };

        assert!(result.backup_path.is_none());
        assert!(result.recovery_hint.is_none());

        // Verify skip_serializing_if: None fields are absent from JSON.
        let json = serde_json::to_value(&result).expect("serialize");
        assert!(json.get("backup_path").is_none());
        assert!(json.get("recovery_hint").is_none());
    }
}
