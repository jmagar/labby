//! Top-level action router for the `stash` dispatch service.
//!
//! `dispatch()` handles the two built-in meta-actions (`help`, `schema`) and
//! delegates all service-specific actions to `dispatch_with_store()`.
//!
//! # Observability
//!
//! `dispatch()` emits one structured event per call at the `INFO` level on
//! success, `WARN` for caller errors, and `ERROR` for internal failures.
//! Fields follow the standard dispatch schema from `docs/dev/OBSERVABILITY.md`:
//! `surface`, `service`, `action`, `elapsed_ms`, and `kind` (errors only).
//!
//! Note: `surface` is hardcoded to `"mcp"` here — the shared dispatch layer
//! does not yet thread surface context through from the calling surface.

use serde_json::Value;

use crate::dispatch::error::ToolError;
use crate::dispatch::helpers::{action_schema, help_payload, require_str};

use super::catalog::ACTIONS;
use super::client::require_stash_root;
use super::params::{
    parse_adopt_params, parse_create_params, parse_deploy_params, parse_export_params,
    parse_get_params, parse_import_params, parse_link_params, parse_provider_sync_params,
    parse_revisions_params, parse_save_params, parse_target_add_params, parse_target_remove_params,
    parse_workspace_params,
};
use super::service;
use super::store::StashStore;

// ── Blocking helper ───────────────────────────────────────────────────────────

/// Run a synchronous closure in a `spawn_blocking` thread pool task.
///
/// All stash service functions are synchronous (they call `std::fs::*` and
/// `fd_lock`) and must run in a dedicated blocking thread, not on a Tokio
/// worker. This helper is the single wrapper used by all sync dispatch arms.
async fn run_blocking<F, T>(f: F) -> Result<T, ToolError>
where
    F: FnOnce() -> Result<T, ToolError> + Send + 'static,
    T: Send + 'static,
{
    tokio::task::spawn_blocking(f)
        .await
        .map_err(|e| ToolError::Sdk {
            sdk_kind: "internal_error".into(),
            message: format!("spawn_blocking panicked: {e}"),
        })?
}

/// Top-level MCP/CLI entry point.
///
/// Handles `help` and `schema` directly, then constructs the store and
/// delegates to [`dispatch_with_store`].
///
/// Emits structured dispatch events — see module docs for field details.
pub async fn dispatch(action: &str, params: Value) -> Result<Value, ToolError> {
    dispatch_for_surface("mcp", action, params).await
}

/// Surface-aware entry point for adapters that need accurate dispatch logs.
pub async fn dispatch_for_surface(
    surface: &'static str,
    action: &str,
    params: Value,
) -> Result<Value, ToolError> {
    let start = std::time::Instant::now();
    let result = dispatch_inner(action, params).await;
    let elapsed_ms = start.elapsed().as_millis();

    match &result {
        Ok(_) => tracing::info!(
            surface,
            service = "stash",
            action,
            elapsed_ms,
            "dispatch ok"
        ),
        Err(err) => {
            if err.is_internal() {
                tracing::error!(
                    surface,
                    service = "stash",
                    action,
                    elapsed_ms,
                    kind = err.kind(),
                    "dispatch error"
                );
            } else {
                tracing::warn!(
                    surface,
                    service = "stash",
                    action,
                    elapsed_ms,
                    kind = err.kind(),
                    "dispatch error"
                );
            }
        }
    }
    result
}

async fn dispatch_inner(action: &str, params: Value) -> Result<Value, ToolError> {
    match action {
        "help" => Ok(help_payload("stash", ACTIONS)),
        "schema" => {
            let a = require_str(&params, "action")?;
            action_schema(ACTIONS, a)
        }
        other => {
            if !ACTIONS.iter().any(|a| a.name == other) {
                return Err(ToolError::UnknownAction {
                    message: format!("unknown action `{other}` for service `stash`"),
                    valid: ACTIONS.iter().map(|a| a.name.to_string()).collect(),
                    hint: None,
                });
            }
            // Store construction + ensure_dirs are synchronous fs ops — run in
            // blocking thread pool so we never block a Tokio worker (lab-p760).
            let root = require_stash_root()?;
            let store = run_blocking(move || {
                let store = StashStore::new(root.clone());
                store.ensure_dirs().map_err(|e| ToolError::Sdk {
                    sdk_kind: "internal_error".into(),
                    message: format!("stash store init: {e}"),
                })?;
                Ok(store)
            })
            .await?;
            dispatch_with_store(&store, other, params).await
        }
    }
}

/// Dispatch an action against a pre-constructed store.
///
/// Called by `dispatch()` after store setup, and may be called directly by
/// API handlers that hold the store in `AppState`.
///
/// # Blocking safety (lab-p760)
///
/// All synchronous service functions (`components_list`, `component_get`, etc.)
/// perform `std::fs::*` I/O and/or acquire `fd_lock` advisory locks. They must
/// run in `spawn_blocking` thread pool tasks, not on Tokio worker threads.
///
/// Arms that call `.await` on already-async service functions (import, save,
/// export, deploy) are exempt — those functions manage their own spawn_blocking
/// internally.
pub async fn dispatch_with_store(
    store: &StashStore,
    action: &str,
    params: Value,
) -> Result<Value, ToolError> {
    match action {
        // ── Sync arms: all wrapped in run_blocking ────────────────────────────
        "components.list" => {
            let s = store.clone();
            run_blocking(move || service::components_list(&s)).await
        }
        "component.get" => {
            let p = parse_get_params(&params)?;
            let s = store.clone();
            run_blocking(move || service::component_get(&s, p)).await
        }
        "component.create" => {
            let p = parse_create_params(&params)?;
            let s = store.clone();
            run_blocking(move || service::component_create(&s, p)).await
        }
        "component.workspace" => {
            let p = parse_workspace_params(&params)?;
            let s = store.clone();
            run_blocking(move || service::component_workspace(&s, p)).await
        }
        "component.revisions" => {
            let p = parse_revisions_params(&params)?;
            let s = store.clone();
            run_blocking(move || service::component_revisions(&s, p)).await
        }
        "providers.list" => {
            let s = store.clone();
            run_blocking(move || service::providers_list(&s, &params)).await
        }
        "provider.link" => {
            let p = parse_link_params(&params)?;
            let s = store.clone();
            run_blocking(move || service::provider_link(&s, p)).await
        }
        "provider.push" => {
            let p = parse_provider_sync_params(&params)?;
            let s = store.clone();
            run_blocking(move || service::provider_push(&s, p)).await
        }
        "provider.pull" => {
            let p = parse_provider_sync_params(&params)?;
            let s = store.clone();
            run_blocking(move || service::provider_pull(&s, p)).await
        }
        "targets.list" => {
            let s = store.clone();
            run_blocking(move || service::targets_list(&s)).await
        }
        "target.add" => {
            let p = parse_target_add_params(&params)?;
            let s = store.clone();
            run_blocking(move || service::target_add(&s, p)).await
        }
        "target.remove" => {
            let p = parse_target_remove_params(&params)?;
            let s = store.clone();
            run_blocking(move || service::target_remove(&s, p)).await
        }
        // ── Async arms: already manage their own spawn_blocking internally ────
        "component.import" => {
            let p = parse_import_params(&params)?;
            service::component_import(store, p).await
        }
        "component.adopt" => {
            let p = parse_adopt_params(&params)?;
            service::component_adopt(store, p).await
        }
        "component.save" => {
            let p = parse_save_params(&params)?;
            service::component_save(store, p).await
        }
        "component.export" => {
            let p = parse_export_params(&params)?;
            service::component_export(store, p).await
        }
        "component.deploy" => {
            let p = parse_deploy_params(&params)?;
            service::component_deploy(store, p).await
        }
        unknown => Err(ToolError::UnknownAction {
            message: format!("unknown action `{unknown}` for service `stash`"),
            valid: ACTIONS.iter().map(|a| a.name.to_string()).collect(),
            hint: None,
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use tempfile::tempdir;

    fn make_store() -> (StashStore, tempfile::TempDir) {
        let dir = tempdir().expect("tempdir");
        let store = StashStore::new(dir.path().to_path_buf());
        store.ensure_dirs().expect("ensure_dirs");
        (store, dir)
    }

    #[tokio::test]
    async fn dispatch_import_accepts_operator_workspace_path() {
        let (store, _stash_dir) = make_store();
        let created = dispatch_with_store(
            &store,
            "component.create",
            json!({"kind": "settings", "name": "operator-settings"}),
        )
        .await
        .expect("create component");
        let id = created
            .get("id")
            .and_then(Value::as_str)
            .expect("component id");

        let source_dir = tempdir().expect("source tempdir");
        let source = source_dir.path().join("settings.json");
        std::fs::write(&source, b"{}").expect("source file");

        let imported = dispatch_with_store(
            &store,
            "component.import",
            json!({"id": id, "source_path": source.display().to_string()}),
        )
        .await
        .expect("import from operator path");

        assert_eq!(imported.get("id").and_then(Value::as_str), Some(id));
        assert!(store.workspace_dir(id).join("settings.json").is_file());
    }

    #[tokio::test]
    async fn dispatch_adopt_imports_and_saves_marketplace_component() {
        let (store, _stash_dir) = make_store();
        let source_dir = tempdir().expect("source tempdir");
        std::fs::write(source_dir.path().join("SKILL.md"), "# Demo skill\n").unwrap();

        let value = dispatch_with_store(
            &store,
            "component.adopt",
            json!({
                "kind": "skill",
                "name": "demo-skill",
                "label": "Demo Skill",
                "source_path": source_dir.path().display().to_string(),
                "origin": {
                    "kind": "marketplace",
                    "plugin_id": "demo@labby",
                    "artifact_path": "skills/demo",
                    "source_version": "1.0.0",
                    "source_fingerprint": "abc123"
                },
                "save_label": "Fork from demo@labby"
            }),
        )
        .await
        .unwrap();

        let component_id = value
            .get("component")
            .unwrap()
            .get("id")
            .unwrap()
            .as_str()
            .unwrap();
        let component = store.read_component(component_id).unwrap().unwrap();
        assert_eq!(component.name, "demo-skill");
        assert_eq!(
            component.head_revision_id,
            value
                .get("revision")
                .unwrap()
                .get("id")
                .and_then(|v| v.as_str())
                .map(str::to_string)
        );
        assert!(store.workspace_dir(component_id).join("SKILL.md").is_file());
        assert!(component.origin_meta.is_some());
    }
}
