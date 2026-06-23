//! Action router for the `setup` Bootstrap orchestrator.
//!
//! Dispatch event field policy: the actions in [`REDACTED_LOG_ACTIONS`]
//! log without their `params` field — drafts may carry secret values
//! en route to disk and must never be visible in logs. Dispatch sees
//! the trimmed action name (e.g. `"draft.set"`, not `"setup.draft.set"`),
//! so the allowlist matches on the trimmed form.
//!
//! `setup.draft.commit` invokes `doctor::dispatch("audit.full", _)`
//! synchronously (buffered) — the orchestrator must make an atomic
//! gate decision over the full audit, so streaming is reserved for a
//! future `setup.audit.preview` action that wraps `stream_audit_full`.

use labby_apis::core::PluginMeta;
use labby_apis::core::action::ActionSpec;
use labby_apis::setup::{CommitOutcome, DraftEntry, SetupClient};
use serde_json::{Value, json};

use std::time::Duration;

use crate::config::env_merge::{self, EnvEntry, MergeRequest, snapshot_mtime};
use crate::config::{config_toml_path, patch_built_in_upstream_apis_enabled};
#[cfg(feature = "gateway")]
use crate::dispatch::gateway::current_gateway_manager;

/// Maximum elapsed time for the inline doctor.audit.full call inside
/// setup.draft.commit. A misconfigured probe (network hang, dead host)
/// will return audit_timeout instead of stalling the wizard forever.
const AUDIT_TIMEOUT: Duration = Duration::from_secs(30);

/// Actions whose `params` field is dropped from dispatch event logs to
/// prevent secret-bearing draft values from leaking into log sinks.
/// Keep this in sync with the catalog — every action that accepts a
/// `value` parameter or commits the draft must be listed here.
const REDACTED_LOG_ACTIONS: &[&str] = &[
    "draft.set",
    "draft.commit",
    "finalize",
    "settings.update",
    "settings.env.update",
    "settings.config.update",
];
use crate::dispatch::error::ToolError;
use crate::dispatch::helpers::{action_schema, help_payload, to_json};
use crate::registry::{
    RegisteredService, RegisteredServiceKind, bootstrap_operator_services,
    built_in_upstream_api_services, service_meta,
};

use super::catalog::ACTIONS;
use super::claude_plugins;
use super::client::{cached_env_var_index, cached_registry, draft_path, env_path};
use super::draft;
use super::params::{parse_bool, parse_entries, parse_force, parse_service, parse_services_filter};
use super::secret_mask;
use super::state;

/// Top-level action dispatch.
pub async fn dispatch(action: &str, params: Value) -> Result<Value, ToolError> {
    let start = std::time::Instant::now();
    let result = dispatch_inner(action, &params).await;
    let elapsed_ms = start.elapsed().as_millis();
    let log_params = !REDACTED_LOG_ACTIONS.contains(&action);
    log_outcome(action, log_params, &params, elapsed_ms, &result);
    result
}

async fn dispatch_inner(action: &str, params: &Value) -> Result<Value, ToolError> {
    match action {
        "help" => Ok(help_payload("setup", ACTIONS)),
        "schema" => {
            let a = crate::dispatch::helpers::require_str(params, "action")?;
            action_schema(ACTIONS, a)
        }
        "state" => state_action(),
        "bootstrap" => super::bootstrap_action(),
        "schema.get" => schema_get_action(params),
        "draft.get" => draft_get_action(),
        "draft.set" => draft_set_action(params).await,
        "draft.discard" => draft_discard_action(),
        "draft.commit" => draft_commit_action(params).await,
        "settings.schema" => to_json(super::settings::schema_response()),
        "settings.state" => settings_state_action(params),
        "settings.update" => settings_update_action(params),
        "settings.env.update" => settings_env_update_action(params).await,
        "settings.config.update" => settings_config_update_action(params),
        "settings.advanced_state" => settings_advanced_state_action(params),
        "settings.env_schema" => settings_env_schema_action(),
        "plugin_hook" => plugin_hook_action(params).await,
        "plugin_sync" => plugin_sync_action(),
        "plugin_export" => plugin_export_action(),
        "plugin_connectivity" => plugin_connectivity_action(params).await,
        "check" => setup_check_action(),
        "repair" => setup_repair_action(),
        // Plugin-lifecycle actions. The dotted `<resource>.<verb>` forms are
        // the canonical names; the snake_case arms beside them are deprecated
        // aliases retained for backward compatibility. Every name routed here
        // is listed in `super::catalog::PLUGIN_LIFECYCLE_ACTIONS`, which the
        // HTTP loopback gate (`crate::api::services::setup`) reads directly —
        // so a name routable here but absent from that const would be a
        // loopback-restriction bypass. The `plugin_lifecycle_actions_*` tests
        // enforce that every const name both routes here and has a catalog
        // entry; keep all of them in sync when adding a lifecycle action.
        "plugins.installed" | "installed_plugins" => installed_plugins_action(params).await,
        "services.status" | "services_status" => services_status_action().await,
        "plugin.install" | "install_plugin" => install_plugin_action(params).await,
        "plugin.uninstall" | "uninstall_plugin" => uninstall_plugin_action(params).await,
        "finalize" => draft_commit_action(params).await,
        unknown => Err(ToolError::UnknownAction {
            message: format!("unknown action `{unknown}` for service `setup`"),
            valid: ACTIONS.iter().map(|s| s.name.to_string()).collect(),
            hint: None,
        }),
    }
}

async fn plugin_hook_action(params: &Value) -> Result<Value, ToolError> {
    let repair = params
        .get("repair")
        .map(|value| parse_required_bool(value, "repair"))
        .transpose()?
        .unwrap_or(true);
    let mode = if repair {
        super::plugin_hook::Mode::Repair
    } else {
        super::plugin_hook::Mode::Check
    };
    let setup = super::plugin_hook::run(mode)?;
    // sync_plugin_env mutates ~/.lab/.env — only run in Repair mode so
    // check-mode invocations are guaranteed non-mutating.
    let sync = if repair {
        Some(super::plugin_hook::sync_plugin_env()?)
    } else {
        None
    };
    let server_url = std::env::var("CLAUDE_PLUGIN_OPTION_SERVER_URL").ok();
    let connectivity = super::plugin_hook::validate_connectivity(server_url.as_deref()).await;
    to_json(super::plugin_hook::PluginHookReport {
        setup,
        sync,
        connectivity,
    })
}

fn plugin_sync_action() -> Result<Value, ToolError> {
    to_json(super::plugin_hook::sync_plugin_env()?)
}

fn plugin_export_action() -> Result<Value, ToolError> {
    to_json(super::plugin_hook::export_plugin_env()?)
}

async fn plugin_connectivity_action(params: &Value) -> Result<Value, ToolError> {
    let server_url = params
        .get("server_url")
        .and_then(Value::as_str)
        .map(str::to_owned);
    let outcome = super::plugin_hook::validate_connectivity(server_url.as_deref()).await;
    to_json(outcome)
}

fn setup_check_action() -> Result<Value, ToolError> {
    to_json(super::plugin_hook::run(super::plugin_hook::Mode::Check)?)
}

fn setup_repair_action() -> Result<Value, ToolError> {
    to_json(super::plugin_hook::run(super::plugin_hook::Mode::Repair)?)
}

async fn installed_plugins_action(params: &Value) -> Result<Value, ToolError> {
    let force = parse_bool(params, "force");
    let plugins = claude_plugins::installed_plugins(force).await?;
    Ok(claude_plugins::installed_plugins_json(plugins))
}

async fn services_status_action() -> Result<Value, ToolError> {
    let statuses = claude_plugins::services_status().await?;
    Ok(claude_plugins::services_status_json(statuses))
}

async fn install_plugin_action(params: &Value) -> Result<Value, ToolError> {
    let service = parse_service(params)?;
    to_json(claude_plugins::install_plugin(&service).await?)
}

async fn uninstall_plugin_action(params: &Value) -> Result<Value, ToolError> {
    let service = parse_service(params)?;
    to_json(claude_plugins::uninstall_plugin(&service).await?)
}

fn state_action() -> Result<Value, ToolError> {
    to_json(state::snapshot(cached_registry()))
}

fn schema_get_action(params: &Value) -> Result<Value, ToolError> {
    let registry = cached_registry();
    let filter = parse_services_filter(params);
    let mut services_out = serde_json::Map::new();
    for entry in registry.services() {
        if let Some(ref allowed) = filter
            && !allowed.iter().any(|s| s == entry.name)
        {
            continue;
        }
        let Some(meta) = service_meta(entry.name) else {
            continue;
        };
        services_out.insert(entry.name.to_string(), service_schema(entry, meta));
    }
    Ok(json!({ "services": Value::Object(services_out) }))
}

fn service_schema(entry: &RegisteredService, meta: &PluginMeta) -> Value {
    let env_var_to_schema = |is_required: bool, var: &labby_apis::core::EnvVar| -> Value {
        let mut entry = serde_json::Map::new();
        entry.insert("name".into(), json!(var.name));
        entry.insert("description".into(), json!(var.description));
        entry.insert("example".into(), json!(var.example));
        entry.insert("secret".into(), json!(var.secret));
        entry.insert("required".into(), json!(is_required));
        if let Some(ui) = var.ui {
            entry.insert("ui".into(), ui_schema_to_json(ui));
        }
        Value::Object(entry)
    };
    let mut env_array: Vec<Value> = meta
        .required_env
        .iter()
        .map(|v| env_var_to_schema(true, v))
        .collect();
    env_array.extend(
        meta.optional_env
            .iter()
            .map(|v| env_var_to_schema(false, v)),
    );
    json!({
        "name": meta.name,
        "display_name": meta.display_name,
        "description": meta.description,
        "category": format!("{:?}", meta.category).to_lowercase(),
        "supports_multi_instance": meta.supports_multi_instance,
        "default_port": meta.default_port,
        "built_in_upstream_api": entry.kind == RegisteredServiceKind::BuiltInUpstreamApi,
        "env": env_array,
    })
}

fn settings_state_action(params: &Value) -> Result<Value, ToolError> {
    let section = requested_section(params)?;
    let path = config_toml_path().ok_or_else(|| ToolError::Sdk {
        sdk_kind: "internal_error".into(),
        message: "HOME env var not set; cannot resolve config.toml path".into(),
    })?;
    let cfg = load_settings_config(&path)?;
    to_json(super::settings::state_response(
        &cfg,
        path.display().to_string(),
        env_path().display().to_string(),
        &section,
    ))
}

fn settings_advanced_state_action(params: &Value) -> Result<Value, ToolError> {
    let mut params = params.clone();
    if let Some(map) = params.as_object_mut() {
        map.insert("section".into(), Value::String("advanced".into()));
    } else {
        params = json!({ "section": "advanced" });
    }
    settings_state_action(&params)
}

fn requested_section(params: &Value) -> Result<String, ToolError> {
    Ok(params
        .get("section")
        .and_then(Value::as_str)
        .unwrap_or("core")
        .to_string())
}

fn parse_update_entries(
    params: &Value,
) -> Result<Vec<super::settings::SettingsUpdateEntry>, ToolError> {
    serde_json::from_value(params.get("entries").cloned().unwrap_or(Value::Null)).map_err(|_| {
        ToolError::InvalidParam {
            message: "entries must be an array of settings updates".into(),
            param: "entries".into(),
        }
    })
}

async fn settings_env_update_action(params: &Value) -> Result<Value, ToolError> {
    let entries = parse_update_entries(params)?;
    let env_entries = super::settings::env_entries_from_updates(&entries)?;
    let env = env_path();
    let expected_mtime = snapshot_mtime(&env);
    super::settings::validate_env_previous(&entries, &env)?;
    let outcome = env_merge::merge(
        &env,
        MergeRequest {
            entries: env_entries
                .into_iter()
                .map(|entry| EnvEntry::new(entry.key, entry.value))
                .collect(),
            force: true,
            expected_mtime,
        },
    )
    .map_err(map_merge_err)?;
    tracing::info!(
        surface = "dispatch",
        service = "setup",
        action = "settings.env.update.success",
        written = outcome.written,
        "settings env update success"
    );
    settings_state_action(params)
}

fn settings_config_update_action(params: &Value) -> Result<Value, ToolError> {
    let entries = parse_update_entries(params)?;
    let patches = super::settings::config_patches_from_entries(&entries)?;
    let path = config_toml_path().ok_or_else(|| ToolError::Sdk {
        sdk_kind: "internal_error".into(),
        message: "HOME env var not set; cannot resolve config.toml path".into(),
    })?;
    let expected = super::settings::expected_config_scalars(&entries)?;
    let outcome = crate::config::patch_config_scalars_checked(&path, &patches, &expected)
        .map_err(config_io_error)?;
    if patches
        .iter()
        .any(|patch| patch.path == "services.built_in_upstream_apis_enabled")
    {
        refresh_built_in_upstream_registry(outcome.config.services.built_in_upstream_apis_enabled);
    }
    to_json(super::settings::SettingsMutationOutcome {
        state: super::settings::state_response(
            &outcome.config,
            path.display().to_string(),
            env_path().display().to_string(),
            requested_section(params)?.as_str(),
        ),
        backup_path: outcome.backup_path.map(|path| path.display().to_string()),
    })
}

fn settings_env_schema_action() -> Result<Value, ToolError> {
    to_json(super::settings::env_schema())
}

fn settings_update_action(params: &Value) -> Result<Value, ToolError> {
    let path = config_toml_path().ok_or_else(|| ToolError::Sdk {
        sdk_kind: "internal_error".into(),
        message: "HOME env var not set; cannot resolve config.toml path".into(),
    })?;
    let enabled = parse_built_in_upstream_apis_enabled(params)?;
    let current = load_settings_config(&path)?;
    let previous_enabled = current.services.built_in_upstream_apis_enabled;
    let changed = previous_enabled != enabled;
    let cfg = if changed {
        patch_built_in_upstream_apis_enabled(&path, enabled).map_err(config_io_error)?
    } else {
        current
    };
    if changed {
        refresh_built_in_upstream_registry(enabled);
    }
    let restart_required = false;
    Ok(settings_state_json(
        &cfg,
        path.display().to_string(),
        restart_required,
        changed,
        Some(previous_enabled),
    ))
}

fn refresh_built_in_upstream_registry(enabled: bool) {
    crate::registry::set_runtime_built_in_upstream_apis_enabled(enabled);
    // Notify the live gateway manager so upstream discovery reflects the change.
    // The gateway manager only exists with the gateway feature; without it there
    // is nothing to refresh.
    #[cfg(feature = "gateway")]
    if let Some(manager) = current_gateway_manager() {
        let registry: std::sync::Arc<
            dyn labby_gateway::gateway::service_registry::GatewayServiceRegistry,
        > = std::sync::Arc::new(crate::registry::filter_built_in_upstream_apis(
            crate::registry::build_default_registry(),
            enabled,
        ));
        manager.set_builtin_service_registry(registry);
    }
}

fn load_settings_config(path: &std::path::Path) -> Result<crate::config::LabConfig, ToolError> {
    crate::config::load_toml(&[path.to_path_buf()]).map_err(config_io_error)
}

fn config_io_error(error: anyhow::Error) -> ToolError {
    let detail = format!("{error:#}");
    let param = stale_setting_param(&detail)
        .unwrap_or("config.toml")
        .to_string();
    ToolError::InvalidParam {
        message: format!("invalid settings config: {detail}"),
        param,
    }
}

fn stale_setting_param(message: &str) -> Option<&str> {
    const PREFIX: &str = "setting `";
    const SUFFIX: &str = "` changed since it was loaded";
    let start = message.find(PREFIX)? + PREFIX.len();
    let rest = &message[start..];
    let end = rest.find(SUFFIX)?;
    Some(&rest[..end])
}

fn parse_built_in_upstream_apis_enabled(params: &Value) -> Result<bool, ToolError> {
    let flat_key = "services.built_in_upstream_apis_enabled";
    let object = params.as_object().ok_or_else(|| ToolError::InvalidParam {
        message: "settings.update requires an object patch".into(),
        param: "params".into(),
    })?;

    for key in object.keys() {
        if key != flat_key && key != "services" && key != "confirm" {
            return Err(ToolError::InvalidParam {
                message: format!("unknown settings.update parameter `{key}`"),
                param: key.clone(),
            });
        }
    }

    let mut parsed = parse_optional_bool(params.get(flat_key), flat_key)?;
    if let Some(services) = params.get("services") {
        let services = services
            .as_object()
            .ok_or_else(|| ToolError::InvalidParam {
                message: "services must be an object".into(),
                param: "services".into(),
            })?;
        for key in services.keys() {
            if key != "built_in_upstream_apis_enabled" {
                return Err(ToolError::InvalidParam {
                    message: format!("unknown settings.update services parameter `{key}`"),
                    param: format!("services.{key}"),
                });
            }
        }
        if let Some(value) = services.get("built_in_upstream_apis_enabled") {
            let nested = parse_required_bool(value, "services.built_in_upstream_apis_enabled")?;
            if let Some(flat) = parsed
                && flat != nested
            {
                return Err(ToolError::InvalidParam {
                    message: "conflicting values for services.built_in_upstream_apis_enabled"
                        .into(),
                    param: flat_key.into(),
                });
            }
            parsed = Some(nested);
        }
    }

    parsed.ok_or_else(|| ToolError::InvalidParam {
        message: "settings.update requires services.built_in_upstream_apis_enabled".into(),
        param: flat_key.into(),
    })
}

fn parse_optional_bool(
    value: Option<&Value>,
    param: &'static str,
) -> Result<Option<bool>, ToolError> {
    value
        .map(|value| parse_required_bool(value, param))
        .transpose()
}

fn parse_required_bool(value: &Value, param: &'static str) -> Result<bool, ToolError> {
    value.as_bool().ok_or_else(|| ToolError::InvalidParam {
        message: format!("{param} must be a boolean"),
        param: param.into(),
    })
}

fn settings_state_json(
    cfg: &crate::config::LabConfig,
    config_path: String,
    restart_required: bool,
    changed: bool,
    previous_built_in_upstream_apis_enabled: Option<bool>,
) -> Value {
    let registry = cached_registry();
    json!({
        "config_path": config_path,
        "changed": changed,
        "previous": {
            "services": {
                "built_in_upstream_apis_enabled": previous_built_in_upstream_apis_enabled,
            },
        },
        "restart_required": restart_required,
        "restart_note": "Changes to built-in upstream API services apply to gateway discovery immediately. Restart labby serve only if you need HTTP route mounting to match the new policy.",
        "services": {
            "built_in_upstream_apis_enabled": cfg.services.built_in_upstream_apis_enabled,
            "built_in_upstream_api_services": built_in_upstream_api_services(registry),
            "bootstrap_services": bootstrap_operator_services(registry),
        },
        "surfaces": settings_surfaces_json(cfg),
    })
}

fn settings_surfaces_json(cfg: &crate::config::LabConfig) -> Value {
    let mcp_transport = std::env::var("LAB_MCP_TRANSPORT")
        .ok()
        .or_else(|| cfg.mcp.transport.clone())
        .unwrap_or_else(|| "http".into());
    let mcp_host = std::env::var("LAB_MCP_HTTP_HOST")
        .ok()
        .or_else(|| cfg.mcp.host.clone())
        .unwrap_or_else(|| "127.0.0.1".into());
    let mcp_port = std::env::var("LAB_MCP_HTTP_PORT")
        .ok()
        .and_then(|value| value.parse::<u16>().ok())
        .or(cfg.mcp.port)
        .unwrap_or(8765);
    let web_auth_disabled = std::env::var(crate::config::WEB_UI_AUTH_DISABLED_ENV)
        .ok()
        .or_else(|| std::env::var(crate::config::WEB_UI_AUTH_DISABLED_LEGACY_ENV).ok())
        .and_then(|value| match value.as_str() {
            "1" | "true" | "TRUE" | "yes" | "YES" => Some(true),
            "0" | "false" | "FALSE" | "no" | "NO" => Some(false),
            _ => None,
        })
        .or(cfg.web.disable_auth)
        .unwrap_or(false);
    let auth_mode = std::env::var("LAB_AUTH_MODE")
        .ok()
        .or_else(|| cfg.auth.as_ref().and_then(|auth| auth.mode.clone()))
        .unwrap_or_else(|| "bearer".into());
    let public_url = std::env::var("LAB_PUBLIC_URL")
        .ok()
        .or_else(|| cfg.auth.as_ref().and_then(|auth| auth.public_url.clone()));

    json!({
        "mcp": {
            "transport": mcp_transport,
            "host": mcp_host,
            "port": mcp_port,
            "stateful": cfg.mcp.stateful,
        },
        "web": {
            "auth_disabled": web_auth_disabled,
            "assets_dir": cfg.web.assets_dir.as_ref().map(|path| path.display().to_string()),
        },
        "auth": {
            "mode": auth_mode,
            "public_url": public_url,
        },
    })
}

fn ui_schema_to_json(ui: &labby_apis::core::plugin_ui::UiSchema) -> Value {
    use labby_apis::core::plugin_ui::FieldKind;
    let kind_str = match ui.kind {
        FieldKind::Text => "text",
        FieldKind::Secret => "secret",
        FieldKind::Url => "url",
        FieldKind::Bool => "bool",
        FieldKind::Number => "number",
        FieldKind::FilePath => "file_path",
        FieldKind::Enum { .. } => "enum",
    };
    let enum_values: Option<Vec<&str>> = match ui.kind {
        FieldKind::Enum { values } => Some(values.to_vec()),
        _ => None,
    };
    json!({
        "kind": kind_str,
        "enum_values": enum_values,
        "advanced": ui.advanced,
        "help_url": ui.help_url,
        "depends_on": ui.depends_on,
        "validation": {
            "required": ui.validation.required,
            "min_length": ui.validation.min_length,
            "max_length": ui.validation.max_length,
            "pattern": ui.validation.pattern,
        },
    })
}

fn draft_get_action() -> Result<Value, ToolError> {
    let path = draft_path();
    let entries = draft::read_entries(&path);
    let masked: Vec<Value> = entries
        .into_iter()
        .map(|e| {
            let value = secret_mask::mask_value(&e.key, &e.value);
            json!({ "key": e.key, "value": value })
        })
        .collect();
    Ok(json!({ "entries": masked }))
}

async fn draft_set_action(params: &Value) -> Result<Value, ToolError> {
    let entries = parse_entries(params)?;
    let force = parse_force(params);

    // Server-side defense-in-depth validation against the UiSchema. The
    // frontend has already validated, but never trust it.
    validate_against_registry(&entries)?;

    let path = draft_path();
    let outcome = draft::merge_entries(&path, entries, force).map_err(map_merge_err)?;

    Ok(json!({
        "written": outcome.written,
        "skipped": outcome.skipped,
        "backup_path": outcome.backup_path,
    }))
}

fn draft_discard_action() -> Result<Value, ToolError> {
    let path = draft_path();
    let removed = draft::discard(&path).map_err(|e| ToolError::Sdk {
        sdk_kind: "internal_error".to_string(),
        message: format!("discard draft `{}` failed: {e}", path.display()),
    })?;
    Ok(json!({ "removed": removed }))
}

fn validate_against_registry(entries: &[DraftEntry]) -> Result<(), ToolError> {
    let index = cached_env_var_index();
    for entry in entries {
        if let Some(var) = index.get(entry.key.as_str())
            && let Some(ui) = var.ui
        {
            SetupClient::validate_against_ui_schema(&entry.key, &entry.value, ui).map_err(|e| {
                ToolError::InvalidParam {
                    message: format!("validation failed for {}: {e}", entry.key),
                    param: entry.key.clone(),
                }
            })?;
        }
    }
    Ok(())
}

async fn draft_commit_action(params: &Value) -> Result<Value, ToolError> {
    let force = parse_force(params);
    let env = env_path();
    let draft = draft_path();

    if !draft.exists() {
        return Err(ToolError::InvalidParam {
            message: "no draft to commit (.env.draft missing)".into(),
            param: "draft".into(),
        });
    }

    // Snapshot mtime before the audit so an interleaved writer is detected.
    let expected_mtime = snapshot_mtime(&env);

    // Run doctor.audit.full inline. The orchestrator-exception clause in
    // dispatch/CLAUDE.md permits Bootstrap services to invoke peer dispatch.
    // Bounded by AUDIT_TIMEOUT so a hung service probe cannot stall the
    // wizard indefinitely (doctor's Semaphore(5) bounds concurrency, not
    // total elapsed time).
    let audit_call = crate::dispatch::doctor::dispatch("audit.full", json!({}));
    let audit = match tokio::time::timeout(AUDIT_TIMEOUT, audit_call).await {
        Ok(result) => result?,
        Err(_) => {
            return Err(ToolError::Sdk {
                sdk_kind: "audit_timeout".into(),
                message: format!(
                    "doctor.audit.full did not return within {}s",
                    AUDIT_TIMEOUT.as_secs()
                ),
            });
        }
    };
    let (audit_pass_count, audit_total_count, all_pass) = audit_summary(&audit);
    if !all_pass {
        // Return the structured audit response inline (no preflight_failed wrap).
        return Ok(json!({
            "ok": false,
            "audit": audit,
            "audit_pass_count": audit_pass_count,
            "audit_total_count": audit_total_count,
        }));
    }

    let entries = draft::read_entries(&draft);
    let outcome = env_merge::merge(
        &env,
        MergeRequest {
            entries: entries
                .into_iter()
                .map(|e| EnvEntry::new(e.key, e.value))
                .collect(),
            force,
            expected_mtime,
        },
    )
    .map_err(map_merge_err)?;
    // env_merge::merge owns rollback semantics: on a post-backup failure
    // it surfaces commit_rollback_failed with the backup_path so the
    // operator can recover manually. Dispatch does not retry the merge.

    // Successful commit — clear the draft so the wizard does not re-replay.
    std::fs::remove_file(&draft).ok();

    let result = CommitOutcome {
        written: outcome.written,
        skipped: outcome.skipped,
        backup_path: outcome.backup_path,
        audit_pass_count,
        audit_total_count,
    };

    tracing::info!(
        surface = "dispatch",
        service = "setup",
        action = "draft.commit.success",
        audit_pass_count,
        audit_total_count,
        written = result.written,
        backup_path = result
            .backup_path
            .as_ref()
            .map(|p| p.to_string_lossy().into_owned()),
        "setup commit success"
    );

    to_json(result)
}

fn audit_summary(audit: &Value) -> (usize, usize, bool) {
    // Single-pass count without cloning the findings array.
    let (pass, total) = audit
        .get("findings")
        .and_then(Value::as_array)
        .map(|arr| {
            arr.iter().fold((0usize, 0usize), |(pass, total), f| {
                let is_err = f.get("severity").and_then(Value::as_str) == Some("error");
                (pass + usize::from(!is_err), total + 1)
            })
        })
        .unwrap_or((0, 0));
    (pass, total, pass == total)
}

pub(super) fn map_merge_err(err: env_merge::MergeError) -> ToolError {
    ToolError::Sdk {
        sdk_kind: err.kind().to_string(),
        message: err.to_string(),
    }
}

fn log_outcome(
    action: &str,
    log_params: bool,
    params: &Value,
    elapsed_ms: u128,
    result: &Result<Value, ToolError>,
) {
    if matches!(
        action,
        "plugin_hook"
            | "plugin_sync"
            | "plugin_export"
            | "plugin_connectivity"
            | "check"
            | "repair"
    ) {
        return;
    }

    let params_field = if log_params {
        params.clone()
    } else {
        // Drop body for setup.draft.* to avoid logging secrets.
        Value::String("<redacted>".into())
    };
    match result {
        Ok(_) => tracing::info!(
            surface = "dispatch",
            service = "setup",
            action,
            elapsed_ms,
            params = ?params_field,
            "dispatch ok"
        ),
        Err(e) => tracing::warn!(
            surface = "dispatch",
            service = "setup",
            action,
            elapsed_ms,
            kind = e.kind(),
            params = ?params_field,
            "dispatch warn"
        ),
    }
}

#[allow(dead_code)]
fn assert_action_count_const() {
    // Compile-time sanity: ACTIONS must list every action this dispatch
    // handles, including help + schema.
    let _: &[ActionSpec] = ACTIONS;
}

#[cfg(test)]
mod tests {
    #![allow(clippy::await_holding_lock, clippy::panic)]

    use super::*;
    use crate::registry::build_default_registry;
    use std::sync::Mutex;

    static ENV_LOCK: Mutex<()> = Mutex::new(());

    #[tokio::test]
    async fn unknown_action_returns_unknown_action() {
        let err = dispatch("does.not.exist", Value::Null).await.unwrap_err();
        assert!(matches!(err, ToolError::UnknownAction { .. }));
    }

    #[tokio::test]
    async fn help_returns_catalog() {
        let v = dispatch("help", Value::Null).await.unwrap();
        assert!(v.get("actions").is_some());
    }

    #[test]
    fn setup_actions_are_unique() {
        let mut seen = std::collections::BTreeSet::new();
        for action in ACTIONS {
            assert!(
                seen.insert(action.name),
                "duplicate setup action {}",
                action.name
            );
        }
    }

    #[test]
    fn config_io_error_preserves_stale_setting_param() {
        let err = config_io_error(anyhow::anyhow!(
            "setting `mcp.port` changed since it was loaded"
        ));
        match err {
            ToolError::InvalidParam { param, .. } => assert_eq!(param, "mcp.port"),
            other => panic!("expected invalid_param, got {other:?}"),
        }
    }

    #[test]
    fn setup_catalog_covers_dispatch_actions() {
        let names: std::collections::BTreeSet<&str> =
            ACTIONS.iter().map(|action| action.name).collect();

        for required in [
            "schema.get",
            "state",
            "draft.set",
            "draft.discard",
            "draft.commit",
            "settings.schema",
            "settings.state",
            "settings.update",
            "settings.env.update",
            "settings.config.update",
            "settings.advanced_state",
            "settings.env_schema",
            "finalize",
            // Canonical dotted plugin-lifecycle names:
            "plugins.installed",
            "services.status",
            "plugin.install",
            "plugin.uninstall",
            // Deprecated snake_case aliases (still routed):
            "installed_plugins",
            "services_status",
            "install_plugin",
            "uninstall_plugin",
            "plugin_sync",
            "plugin_export",
            "plugin_connectivity",
        ] {
            assert!(names.contains(required), "missing setup action {required}");
        }
    }

    #[test]
    fn plugin_lifecycle_actions_are_cataloged() {
        // Every loopback-gated name must have a catalog ActionSpec — otherwise
        // the unknown-action gate in the API surface would reject it before
        // the loopback gate ever runs, and discovery would not list it.
        let names: std::collections::BTreeSet<&str> =
            ACTIONS.iter().map(|action| action.name).collect();
        for action in super::super::PLUGIN_LIFECYCLE_ACTIONS {
            assert!(
                names.contains(action),
                "plugin-lifecycle action `{action}` is in PLUGIN_LIFECYCLE_ACTIONS \
                 but has no catalog ActionSpec"
            );
        }
    }

    #[test]
    fn plugin_lifecycle_canonical_alias_pairs_share_metadata() {
        // PLUGIN_LIFECYCLE_ACTIONS is ordered (canonical, alias). Both members
        // of a pair route to the same handler, so they MUST carry identical
        // destructive / requires_admin / returns metadata — otherwise one form
        // could skip the destructive-confirmation or admin-scope gate that the
        // other enforces. Guards against hand-copy drift between the entries.
        let spec = |name: &str| {
            ACTIONS
                .iter()
                .find(|action| action.name == name)
                .unwrap_or_else(|| panic!("missing catalog entry for `{name}`"))
        };
        for pair in super::super::PLUGIN_LIFECYCLE_ACTIONS.chunks_exact(2) {
            let canonical = spec(pair[0]);
            let alias = spec(pair[1]);
            assert_eq!(
                canonical.destructive, alias.destructive,
                "`{}` and `{}` disagree on destructive",
                pair[0], pair[1]
            );
            assert_eq!(
                canonical.requires_admin, alias.requires_admin,
                "`{}` and `{}` disagree on requires_admin",
                pair[0], pair[1]
            );
            assert_eq!(
                canonical.returns, alias.returns,
                "`{}` and `{}` disagree on returns",
                pair[0], pair[1]
            );
        }
        // Spot-check the intended classification so a future edit that flips
        // both members of a pair together is still caught.
        assert!(spec("plugin.install").destructive);
        assert!(spec("plugin.uninstall").destructive);
        assert!(!spec("plugins.installed").destructive);
        assert!(!spec("services.status").destructive);
    }

    #[tokio::test]
    async fn dotted_plugin_mutation_actions_route_to_handlers() {
        // Prove the new dotted match arms actually route to the real handlers
        // rather than falling through to the `unknown` arm: a typo in a match
        // literal would surface here as `unknown_action` instead of the
        // handler's `missing_param`. The two mutation actions reach
        // `parse_service` (→ missing_param) before any subprocess, so this is
        // deterministic and side-effect-free. The read actions spawn the
        // `claude` CLI, so they are intentionally not dispatched in a unit test.
        for action in [
            "plugin.install",
            "plugin.uninstall",
            "install_plugin",
            "uninstall_plugin",
        ] {
            let err = dispatch(action, json!({})).await.unwrap_err();
            assert_eq!(
                err.kind(),
                "missing_param",
                "`{action}` should route to its handler and require `service`, got {err:?}"
            );
            match err {
                ToolError::InvalidParam { param, .. } | ToolError::MissingParam { param, .. } => {
                    assert_eq!(
                        param, "service",
                        "`{action}` should fault the `service` param"
                    );
                }
                other => panic!("`{action}` returned unexpected error: {other:?}"),
            }
        }
    }

    #[test]
    fn settings_update_accepts_flat_and_nested_toggle_param() {
        assert!(
            !parse_built_in_upstream_apis_enabled(
                &json!({"services.built_in_upstream_apis_enabled": false})
            )
            .unwrap()
        );
        assert!(
            parse_built_in_upstream_apis_enabled(
                &json!({"services": {"built_in_upstream_apis_enabled": true}})
            )
            .unwrap()
        );
    }

    #[test]
    fn settings_update_rejects_empty_and_unknown_toggle_patches() {
        for params in [
            json!({}),
            json!({"services": {}}),
            json!({"services": {"built_in_upstream_api_enabled": false}}),
            json!({"unexpected": false}),
        ] {
            let err = parse_built_in_upstream_apis_enabled(&params).unwrap_err();
            assert_eq!(err.kind(), "invalid_param", "{params}");
        }
    }

    #[test]
    fn settings_update_catalog_requires_toggle_param() {
        let action = ACTIONS
            .iter()
            .find(|action| action.name == "settings.update")
            .expect("settings.update action");
        assert!(action.destructive);
        assert!(action.requires_admin);
        let param = action
            .params
            .iter()
            .find(|param| param.name == "services.built_in_upstream_apis_enabled")
            .expect("toggle param");
        assert!(param.required);
    }

    #[test]
    fn setup_settings_mutations_require_admin_scope() {
        for action_name in [
            "settings.update",
            "settings.config.update",
            "settings.env.update",
        ] {
            let action = ACTIONS
                .iter()
                .find(|action| action.name == action_name)
                .expect(action_name);
            assert!(action.requires_admin, "{action_name} must require admin");
            assert!(action.destructive, "{action_name} must be destructive");
        }
    }

    #[tokio::test]
    #[allow(clippy::await_holding_lock)]
    async fn settings_update_dispatch_persists_and_preserves_config_toml() {
        let _guard = ENV_LOCK.lock().expect("env lock");
        let previous_runtime = crate::registry::runtime_built_in_upstream_apis_enabled();
        let temp = tempfile::tempdir().expect("tempdir");
        let config_dir = temp.path().join(".config/lab");
        std::fs::create_dir_all(&config_dir).expect("config dir");
        let config_path = config_dir.join("config.toml");
        std::fs::write(
            &config_path,
            "# keep me\n[services]\n# upstream policy\nbuilt_in_upstream_apis_enabled = true\n[plugin_owned]\nfuture = \"keep\"\n",
        )
        .expect("write config");
        crate::config::set_test_config_toml_path(Some(config_path.clone()));
        crate::registry::set_runtime_built_in_upstream_apis_enabled(true);

        let updated = dispatch(
            "settings.update",
            json!({"services": {"built_in_upstream_apis_enabled": false}, "confirm": true}),
        )
        .await
        .expect("settings update");
        assert_eq!(updated["services"]["built_in_upstream_apis_enabled"], false);
        assert_eq!(updated["changed"], true);
        assert_eq!(updated["restart_required"], false);
        assert_eq!(
            updated["previous"]["services"]["built_in_upstream_apis_enabled"],
            true
        );
        assert!(!crate::registry::runtime_built_in_upstream_apis_enabled());

        let persisted = std::fs::read_to_string(&config_path).expect("read config");
        assert!(persisted.contains("# keep me"));
        assert!(persisted.contains("[plugin_owned]"));
        assert!(persisted.contains("built_in_upstream_apis_enabled = false"));

        let state = dispatch("settings.state", json!({"section": "features"}))
            .await
            .expect("settings state");
        assert_eq!(
            state["values"]["services.built_in_upstream_apis_enabled"],
            false
        );

        crate::registry::set_runtime_built_in_upstream_apis_enabled(previous_runtime);
        crate::config::set_test_config_toml_path(None);
    }

    #[tokio::test]
    async fn settings_config_update_dispatch_persists_and_rejects_stale_previous() {
        let _guard = ENV_LOCK.lock().expect("env lock");
        let temp = tempfile::tempdir().expect("tempdir");
        let config_dir = temp.path().join(".config/lab");
        std::fs::create_dir_all(&config_dir).expect("config dir");
        let config_path = config_dir.join("config.toml");
        let original = "# keep me\n[mcp]\nport = 8765\n[plugin_owned]\nfuture = \"keep\"\n";
        std::fs::write(&config_path, original).expect("write config");
        crate::config::set_test_config_toml_path(Some(config_path.clone()));

        let updated = dispatch(
            "settings.config.update",
            json!({
                "section": "surfaces",
                "confirm": true,
                "entries": [{
                    "key": "mcp.port",
                    "value": 8766,
                    "previous": 8765
                }]
            }),
        )
        .await
        .expect("settings config update");

        assert_eq!(updated["state"]["values"]["mcp.port"], 8766);
        assert!(
            updated["backup_path"]
                .as_str()
                .is_some_and(|path| path.contains("config.toml.bak."))
        );
        let persisted = std::fs::read_to_string(&config_path).expect("read config");
        assert!(persisted.contains("# keep me"));
        assert!(persisted.contains("[plugin_owned]"));
        assert!(persisted.contains("port = 8766"));

        let err = dispatch(
            "settings.config.update",
            json!({
                "section": "surfaces",
                "confirm": true,
                "entries": [{
                    "key": "mcp.port",
                    "value": 8767,
                    "previous": 8765
                }]
            }),
        )
        .await
        .unwrap_err();
        assert_eq!(err.kind(), "invalid_param");
        match err {
            ToolError::InvalidParam { param, .. } => assert_eq!(param, "mcp.port"),
            other => panic!("expected invalid_param, got {other:?}"),
        }
        assert!(
            std::fs::read_to_string(&config_path)
                .expect("read config")
                .contains("port = 8766")
        );

        crate::config::set_test_config_toml_path(None);
    }

    #[tokio::test]
    async fn settings_config_update_rejects_env_file_shadowed_config_field() {
        let _guard = ENV_LOCK.lock().expect("env lock");
        let temp = tempfile::tempdir().expect("tempdir");
        let config_dir = temp.path().join(".config/lab");
        std::fs::create_dir_all(&config_dir).expect("config dir");
        let config_path = config_dir.join("config.toml");
        std::fs::write(&config_path, "[mcp]\nport = 8765\n").expect("write config");
        crate::config::set_test_config_toml_path(Some(config_path.clone()));

        let lab_dir = temp.path().join("lab-home");
        std::fs::create_dir_all(&lab_dir).expect("lab dir");
        std::fs::write(lab_dir.join(".env"), "LAB_MCP_HTTP_PORT=9999\n").expect("write env");
        crate::dispatch::helpers::set_test_lab_home(Some(lab_dir));

        let err = dispatch(
            "settings.config.update",
            json!({
                "section": "surfaces",
                "confirm": true,
                "entries": [{
                    "key": "mcp.port",
                    "value": 8766,
                    "previous": 8765
                }]
            }),
        )
        .await
        .unwrap_err();

        assert_eq!(err.kind(), "invalid_param");
        match err {
            ToolError::InvalidParam { param, .. } => assert_eq!(param, "mcp.port"),
            other => panic!("expected invalid_param, got {other:?}"),
        }
        assert!(
            std::fs::read_to_string(&config_path)
                .expect("read config")
                .contains("port = 8765")
        );

        crate::dispatch::helpers::set_test_lab_home(None);
        crate::config::set_test_config_toml_path(None);
    }

    #[tokio::test]
    async fn settings_env_update_overwrites_existing_key_when_previous_matches() {
        let _guard = ENV_LOCK.lock().expect("env lock");
        let temp = tempfile::tempdir().expect("tempdir");
        let lab_dir = temp.path().join("lab-home");
        std::fs::create_dir_all(&lab_dir).expect("lab dir");
        let env_file = lab_dir.join(".env");
        std::fs::write(&env_file, "LAB_LOG=labby=info\n").expect("write env");
        crate::dispatch::helpers::set_test_lab_home(Some(lab_dir.clone()));

        let updated = dispatch(
            "settings.env.update",
            json!({
                "section": "core",
                "confirm": true,
                "entries": [{
                    "key": "LAB_LOG",
                    "value": "labby=debug",
                    "previous": "labby=info"
                }]
            }),
        )
        .await
        .expect("settings env update");

        assert_eq!(updated["values"]["LAB_LOG"], "labby=debug");
        assert!(
            std::fs::read_to_string(&env_file)
                .unwrap()
                .contains("LAB_LOG=labby=debug")
        );

        crate::dispatch::helpers::set_test_lab_home(None);
    }

    #[tokio::test]
    async fn settings_env_update_rejects_stale_previous_value() {
        let _guard = ENV_LOCK.lock().expect("env lock");
        let temp = tempfile::tempdir().expect("tempdir");
        let lab_dir = temp.path().join("lab-home");
        std::fs::create_dir_all(&lab_dir).expect("lab dir");
        let env_file = lab_dir.join(".env");
        std::fs::write(&env_file, "LAB_LOG=labby=warn\n").expect("write env");
        crate::dispatch::helpers::set_test_lab_home(Some(lab_dir.clone()));

        let err = dispatch(
            "settings.env.update",
            json!({
                "section": "core",
                "confirm": true,
                "entries": [{
                    "key": "LAB_LOG",
                    "value": "labby=debug",
                    "previous": "labby=info"
                }]
            }),
        )
        .await
        .unwrap_err();

        assert_eq!(err.kind(), "invalid_param");
        assert!(
            std::fs::read_to_string(&env_file)
                .unwrap()
                .contains("LAB_LOG=labby=warn")
        );

        crate::dispatch::helpers::set_test_lab_home(None);
    }

    #[tokio::test]
    async fn schema_get_lists_services_with_meta() {
        let v = dispatch("schema.get", json!({})).await.unwrap();
        let services = v.get("services").and_then(Value::as_object).unwrap();
        // Every service that has a PluginMeta entry should appear; synthetic
        // services without meta (doctor/setup) are skipped — they
        // have no env config to render in the wizard.
        for entry in build_default_registry().services() {
            if service_meta(entry.name).is_some() {
                assert!(
                    services.contains_key(entry.name),
                    "missing service: {}",
                    entry.name
                );
            }
        }
        assert!(!services.is_empty());
    }

    #[tokio::test]
    async fn schema_get_filter_returns_subset() {
        let v = dispatch("schema.get", json!({"services": ["radarr"]}))
            .await
            .unwrap();
        let services = v.get("services").and_then(Value::as_object).unwrap();
        // With filter, only requested services that have meta should appear.
        for key in services.keys() {
            assert_eq!(key, "radarr");
        }
    }
}
