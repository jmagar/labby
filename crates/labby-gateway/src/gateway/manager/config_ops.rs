//! Config reads/writes for upstream entries: `add`, `batch_add`, `update`,
//! `remove`, service env config, and the code-mode config mutation.

use std::collections::BTreeMap;

use tokio::time::Instant;

use crate::gateway::config::{
    default_gateway_bearer_env_name, insert_upstream, remove_upstream, tombstone_removed_import,
    update_upstream, validate_bearer_token_env_name, validate_code_mode,
};
use crate::gateway::config_mutation::read_env_values;
use crate::gateway::params::GatewayUpdatePatch;
use crate::gateway::projection::*;
use crate::gateway::types::{GatewayRuntimeView, GatewayView, ServiceConfigView};
use crate::upstream::types::UpstreamRuntimeOwner;
use labby_runtime::error::ToolError;
use labby_runtime::gateway_config::{CodeModeConfig, GatewayConfig, UpstreamConfig};

use super::GatewayManager;

/// Outcome of a `batch_add` call.
///
/// `views` contains one [`GatewayView`] for each spec that was successfully
/// inserted. `errors` contains `(name, error)` pairs for every spec that
/// failed validation or insertion.
#[derive(Debug, Default)]
pub struct BatchAddOutcome {
    pub views: Vec<GatewayView>,
    pub errors: Vec<(String, ToolError)>,
}

impl GatewayManager {
    /// Return the resolved canonical public URL pair for the app and MCP gateway.
    ///
    /// Merges env vars over config file over legacy `[auth].public_url` field.
    pub fn public_urls(&self) -> labby_runtime::gateway_config::ResolvedPublicUrls {
        self.store.public_urls()
    }

    pub async fn get_service_config(&self, service: &str) -> Result<ServiceConfigView, ToolError> {
        let meta =
            self.registered_service_meta(service)
                .ok_or_else(|| ToolError::InvalidParam {
                    message: format!("unknown service `{service}`"),
                    param: "service".to_string(),
                })?;
        let values = read_env_values(&self.env_path())?;
        Ok(service_config_view(meta, &values))
    }

    pub async fn set_service_config(
        &self,
        service: &str,
        values: &BTreeMap<String, String>,
    ) -> Result<ServiceConfigView, ToolError> {
        let meta =
            self.registered_service_meta(service)
                .ok_or_else(|| ToolError::InvalidParam {
                    message: format!("unknown service `{service}`"),
                    param: "service".to_string(),
                })?;

        for field in values.keys() {
            let valid = meta
                .required_env
                .iter()
                .chain(meta.optional_env.iter())
                .any(|env| env.name == field);
            if !valid {
                return Err(ToolError::InvalidParam {
                    message: format!("field `{field}` is not valid for service `{service}`"),
                    param: "values".to_string(),
                });
            }
        }

        let _mutation_guard = self.config_mutation.lock().await;
        // The host store owns env-file backup/atomic-write semantics and any
        // cached service-client refresh; the manager only validates + delegates.
        if !values.is_empty() {
            self.store.persist_service_env(service, values).await?;
        }

        let values = read_env_values(&self.env_path())?;
        Ok(service_config_view(meta, &values))
    }

    /// Return a snapshot of the current gateway config (read-only).
    pub async fn current_config(&self) -> GatewayConfig {
        self.config.read().await.clone()
    }

    pub async fn upstream_config(&self, name: &str) -> Option<UpstreamConfig> {
        self.config
            .read()
            .await
            .upstream
            .iter()
            .find(|upstream| upstream.name == name)
            .cloned()
    }

    pub async fn add(
        &self,
        mut spec: UpstreamConfig,
        bearer_token_value: Option<String>,
        origin: Option<&str>,
        owner: Option<UpstreamRuntimeOwner>,
    ) -> Result<GatewayView, ToolError> {
        let started = Instant::now();
        let _mutation_guard = self.config_mutation.lock().await;
        let mut cfg = self.config.read().await.clone();

        // Trim and validate bearer_token_env unconditionally so whitespace typos
        // are caught before they silently fail env-var lookup later.
        if let Some(ref env_name) = spec.bearer_token_env {
            let trimmed = env_name.trim().to_string();
            validate_bearer_token_env_name(&trimmed)?;
            spec.bearer_token_env = Some(trimmed);
        }

        if let Some(token_value) = bearer_token_value.as_deref().map(str::trim)
            && !token_value.is_empty()
        {
            let env_name =
                resolve_gateway_bearer_env_name(&spec.name, spec.bearer_token_env.as_deref())?;
            spec.bearer_token_env = Some(env_name.clone());
            insert_upstream(&mut cfg, spec.clone())?;
            self.persist_gateway_bearer_token(&env_name, token_value)
                .await?;
        } else {
            insert_upstream(&mut cfg, spec.clone())?;
        }

        // Log only after validation (inside insert_upstream) has passed so
        // spec.name is confirmed well-formed before it enters any log sink.
        tracing::info!(
            surface = "dispatch",
            service = "gateway",
            action = "gateway.add",
            event = "install.start",
            phase = "start",
            gateway = %spec.name,
            target = ?redacted_gateway_target(&spec),
            "gateway reconcile"
        );
        self.persist_config(cfg).await?;
        let diff = self.reload_with_origin_unlocked(origin, owner).await?;
        tracing::info!(
            surface = "dispatch",
            service = "gateway",
            action = "gateway.add",
            event = "install.finish",
            phase = "finish",
            gateway = %spec.name,
            target = ?redacted_gateway_target(&spec),
            tools_changed = diff.tools_changed,
            resources_changed = diff.resources_changed,
            prompts_changed = diff.prompts_changed,
            elapsed_ms = started.elapsed().as_millis(),
            "gateway reconcile"
        );
        self.get(&spec.name).await
    }

    /// Add multiple upstream servers in a single config-persist + reload cycle.
    ///
    /// Each spec is validated and inserted individually. Specs that fail validation
    /// are collected into `errors`; specs that succeed populate `views`. If every
    /// spec fails the first error is returned as `Err`. Otherwise, a single
    /// `persist_config` + `reload_with_origin_unlocked` is issued for all successes.
    pub async fn batch_add(
        &self,
        specs: Vec<UpstreamConfig>,
        origin: Option<&str>,
        owner: Option<UpstreamRuntimeOwner>,
    ) -> Result<BatchAddOutcome, ToolError> {
        if specs.is_empty() {
            return Ok(BatchAddOutcome::default());
        }
        let started = std::time::Instant::now();
        let _mutation_guard = self.config_mutation.lock().await;
        let mut cfg = self.config.read().await.clone();

        let mut added_names = Vec::new();
        let mut errors: Vec<(String, ToolError)> = Vec::new();
        for mut spec in specs {
            if let Some(ref env_name) = spec.bearer_token_env {
                let trimmed = env_name.trim().to_string();
                if let Err(e) = validate_bearer_token_env_name(&trimmed) {
                    errors.push((spec.name, e));
                    continue;
                }
                spec.bearer_token_env = Some(trimmed);
            }
            match insert_upstream(&mut cfg, spec.clone()) {
                Ok(()) => added_names.push(spec.name),
                Err(e) => errors.push((spec.name, e)),
            }
        }

        if added_names.is_empty() && !errors.is_empty() {
            // Every spec failed — return the first error to the caller.
            return Err(errors.remove(0).1);
        }

        self.persist_config(cfg).await?;
        let diff = self.reload_with_origin_unlocked(origin, owner).await?;

        tracing::info!(
            surface = "dispatch",
            service = "gateway",
            action = "gateway.import",
            event = "batch_install.finish",
            added = added_names.len(),
            skipped = errors.len(),
            tools_changed = diff.tools_changed,
            elapsed_ms = started.elapsed().as_millis(),
            "gateway batch reconcile"
        );

        let mut views = Vec::new();
        for name in &added_names {
            if let Ok(view) = self.get(name).await {
                views.push(view);
            }
        }
        Ok(BatchAddOutcome { views, errors })
    }

    pub(crate) async fn update(
        &self,
        name: &str,
        patch: GatewayUpdatePatch,
        bearer_token_value: Option<String>,
        origin: Option<&str>,
        owner: Option<UpstreamRuntimeOwner>,
    ) -> Result<GatewayView, ToolError> {
        let started = Instant::now();
        let mut patch = patch;
        let updated_name = patch.name.clone().unwrap_or_else(|| name.to_string());
        tracing::info!(
            surface = "dispatch",
            service = "gateway",
            action = "gateway.update",
            event = "install.update.start",
            phase = "start",
            gateway = %name,
            new_gateway = %updated_name,
            "gateway reconcile"
        );
        let _mutation_guard = self.config_mutation.lock().await;
        let mut cfg = self.config.read().await.clone();

        // Trim and validate bearer_token_env unconditionally so whitespace typos
        // are caught before they silently fail env-var lookup later.
        if let Some(Some(ref env_name)) = patch.bearer_token_env {
            let trimmed = env_name.trim().to_string();
            validate_bearer_token_env_name(&trimmed)?;
            patch.bearer_token_env = Some(Some(trimmed));
        }

        if let Some(token_value) = bearer_token_value.as_deref().map(str::trim)
            && !token_value.is_empty()
        {
            // Resolve env var name: prefer patch > existing config > error.
            // Auto-generation is intentionally not used here — callers must be
            // explicit so the stored env name is predictable and auditable.
            let env_name = if let Some(env) = patch
                .bearer_token_env
                .as_ref()
                .and_then(|value| value.as_deref())
            {
                env.to_string()
            } else if let Some(existing_env) = cfg
                .upstream
                .iter()
                .find(|u| u.name == name)
                .and_then(|u| u.bearer_token_env.as_deref())
            {
                existing_env.to_string()
            } else {
                return Err(ToolError::InvalidParam {
                    message: "bearer_token_env is required when providing bearer_token_value: \
                              set bearer_token_env in the patch or ensure the existing gateway \
                              already has one configured"
                        .to_string(),
                    param: "bearer_token_env".to_string(),
                });
            };
            patch.bearer_token_env = Some(Some(env_name.clone()));
            update_upstream(&mut cfg, name, patch)?;
            self.persist_gateway_bearer_token(&env_name, token_value)
                .await?;
        } else {
            update_upstream(&mut cfg, name, patch)?;
        }
        self.persist_config(cfg).await?;
        let diff = self.reload_with_origin_unlocked(origin, owner).await?;
        tracing::info!(
            surface = "dispatch",
            service = "gateway",
            action = "gateway.update",
            event = "install.update.finish",
            phase = "finish",
            gateway = %name,
            new_gateway = %updated_name,
            tools_changed = diff.tools_changed,
            resources_changed = diff.resources_changed,
            prompts_changed = diff.prompts_changed,
            elapsed_ms = started.elapsed().as_millis(),
            "gateway reconcile"
        );
        self.get(&updated_name).await
    }

    pub async fn remove(
        &self,
        name: &str,
        origin: Option<&str>,
        owner: Option<UpstreamRuntimeOwner>,
    ) -> Result<GatewayView, ToolError> {
        let started = Instant::now();
        tracing::info!(
            surface = "dispatch",
            service = "gateway",
            action = "gateway.remove",
            event = "remove.start",
            phase = "start",
            gateway = %name,
            "gateway reconcile"
        );
        let _mutation_guard = self.config_mutation.lock().await;
        let mut cfg = self.config.read().await.clone();
        let code_mode = cfg.code_mode.clone();
        let removed = remove_upstream(&mut cfg, name)?;
        tombstone_removed_import(&mut cfg, &removed);
        self.persist_config(cfg).await?;
        let diff = self.reload_with_origin_unlocked(origin, owner).await?;
        tracing::info!(
            surface = "dispatch",
            service = "gateway",
            action = "gateway.remove",
            event = "remove.finish",
            phase = "finish",
            gateway = %name,
            target = ?redacted_gateway_target(&removed),
            tools_changed = diff.tools_changed,
            resources_changed = diff.resources_changed,
            prompts_changed = diff.prompts_changed,
            elapsed_ms = started.elapsed().as_millis(),
            "gateway reconcile"
        );
        Ok(GatewayView {
            config: config_view(&removed, &code_mode),
            runtime: GatewayRuntimeView {
                name: removed.name,
                ..GatewayRuntimeView::default()
            },
        })
    }

    pub async fn set_code_mode_config(
        &self,
        next: CodeModeConfig,
        origin: Option<&str>,
        owner: Option<UpstreamRuntimeOwner>,
    ) -> Result<CodeModeConfig, ToolError> {
        // Field-level validation (ranges, etc.) runs before acquiring the lock —
        // it is idempotent and does not read shared state.
        validate_code_mode(&next)?;
        let _mutation_guard = self.config_mutation.lock().await;
        let mut cfg = self.config.read().await.clone();
        let old_enabled = cfg.code_mode.enabled;
        cfg.code_mode = next.clone();
        self.persist_config(cfg).await?;
        self.reload_with_origin_unlocked(origin, owner).await?;
        tracing::info!(
            surface = "dispatch",
            service = "gateway",
            action = "gateway.mode_change",
            mode = "code_mode",
            enabled = next.enabled,
            previous = old_enabled,
            timeout_ms = next.timeout_ms,
            max_response_bytes = next.max_response_bytes,
            max_response_tokens = next.max_response_tokens,
            "gateway mode changed"
        );
        Ok(self.code_mode_config().await)
    }
}

fn resolve_gateway_bearer_env_name(
    gateway_name: &str,
    explicit_env_name: Option<&str>,
) -> Result<String, ToolError> {
    match explicit_env_name.map(str::trim) {
        Some(name) if !name.is_empty() => {
            validate_bearer_token_env_name(name)?;
            Ok(name.to_string())
        }
        _ => Ok(default_gateway_bearer_env_name(gateway_name)),
    }
}
