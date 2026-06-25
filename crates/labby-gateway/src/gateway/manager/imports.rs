//! Discovery import orchestration: auto-import, the pending-import queue, and
//! import tombstone management.

use crate::gateway::config::insert_upstream;
use crate::gateway::types::{
    GatewayView, ImportErrorView, ImportResultView, ImportSkipReason, ImportSkipView,
    ImportTombstoneView, PendingDiscoveryOutcome, PendingImportApprovalView, PendingImportView,
};
use crate::upstream::types::UpstreamRuntimeOwner;
use labby_runtime::error::ToolError;
use labby_runtime::gateway_config::{GatewayConfig, UpstreamConfig, UpstreamImportTombstone};

use super::GatewayManager;
use super::import_matchers::{
    ImportTombstoneSelector, discovered_is_tombstoned, partition_discovered_for_import,
};

/// Maximum number of import tombstones retained in config.
///
/// When this limit is exceeded the oldest entries (by `removed_at` timestamp,
/// or insertion order when timestamps are equal) are pruned at write time so
/// the config file does not grow without bound.
const MAX_IMPORT_TOMBSTONES: usize = 200;

/// Maximum number of pending (unreviewed) upstream imports retained in config.
///
/// When this limit is exceeded the oldest entries (by insertion order) are
/// dropped at write time.  Operators can always re-run discovery to surface
/// them again.
const MAX_PENDING_IMPORTS: usize = 200;

/// Trim `cfg.upstream_import_tombstones` to at most [`MAX_IMPORT_TOMBSTONES`]
/// entries, retaining the most recently tombstoned ones.
///
/// Tombstones are sorted by `removed_at` in ascending order (oldest first) so
/// `drain(..excess)` removes the oldest entries.  Entries without a
/// `removed_at` value sort before those that have one, ensuring they are
/// pruned first when the list overflows.
pub(super) fn cap_import_tombstones(cfg: &mut GatewayConfig) {
    let len = cfg.upstream_import_tombstones.len();
    if len <= MAX_IMPORT_TOMBSTONES {
        return;
    }
    // Sort ascending by removed_at (None < Some) so oldest are at the front.
    cfg.upstream_import_tombstones
        .sort_by(|a, b| a.removed_at.cmp(&b.removed_at));
    let excess = len - MAX_IMPORT_TOMBSTONES;
    cfg.upstream_import_tombstones.drain(..excess);
}

/// Trim `cfg.upstream_pending` to at most [`MAX_PENDING_IMPORTS`] entries,
/// keeping the most recently queued ones (tail of the vec, since discovery
/// appends to the end).
pub(super) fn cap_pending_imports(cfg: &mut GatewayConfig) {
    let len = cfg.upstream_pending.len();
    if len <= MAX_PENDING_IMPORTS {
        return;
    }
    let excess = len - MAX_PENDING_IMPORTS;
    cfg.upstream_pending.drain(..excess);
}

fn pending_import_view(upstream: &UpstreamConfig) -> PendingImportView {
    PendingImportView {
        name: upstream.name.clone(),
        url: upstream.url.clone(),
        command: upstream.command.clone(),
        source_client: upstream
            .imported_from
            .as_ref()
            .map(|source| source.client.clone())
            .unwrap_or_default(),
        source_path: upstream
            .imported_from
            .as_ref()
            .map(|source| source.path.clone())
            .unwrap_or_default(),
    }
}

fn import_tombstone_view(tombstone: &UpstreamImportTombstone) -> ImportTombstoneView {
    ImportTombstoneView {
        name: tombstone.name.clone(),
        source_client: tombstone.imported_from.client.clone(),
        source_path: tombstone.imported_from.path.clone(),
        server_name: tombstone.imported_from.server_name.clone(),
        transport_fingerprint: tombstone.imported_from.transport_fingerprint.clone(),
        removed_at: tombstone.removed_at.clone(),
    }
}

impl GatewayManager {
    /// Import all externally configured MCP servers that are not already in the gateway config.
    ///
    /// Discovery-produced specs are intentionally persisted with `enabled = false`.
    /// This makes newly found servers visible and editable in the gateway without
    /// exposing them to downstream MCP clients until an operator enables them.
    pub async fn auto_import_discovered_configs(&self) -> Result<ImportResultView, ToolError> {
        let Some(home) = crate::gateway::discovery::home_dir() else {
            tracing::warn!(
                surface = "dispatch",
                service = "gateway",
                action = "gateway.import",
                event = "auto_import.skipped",
                reason = "home_dir_unavailable",
                "automatic MCP config import skipped"
            );
            return Ok(ImportResultView::default());
        };

        let discovered =
            tokio::task::spawn_blocking(move || crate::gateway::discovery::discover_all(&home))
                .await
                .map_err(|e| {
                    ToolError::internal_message(format!("discovery task panicked: {e}"))
                })?;

        let cfg = self.current_config().await;
        let (mut result, specs_to_add) = partition_discovered_for_import(&cfg, discovered);

        if !specs_to_add.is_empty() {
            let outcome = self
                .batch_add(specs_to_add, Some("gateway.auto_import"), None)
                .await?;
            result.imported.extend(outcome.views);
            for (name, err) in outcome.errors {
                if matches!(err, ToolError::Conflict { .. }) {
                    result.skipped.push(ImportSkipView {
                        name,
                        reason: ImportSkipReason::Conflict,
                    });
                } else {
                    result.errors.push(ImportErrorView {
                        name,
                        message: err.to_string(),
                    });
                }
            }
        }

        tracing::info!(
            surface = "dispatch",
            service = "gateway",
            action = "gateway.import",
            event = "auto_import.finish",
            imported = result.imported.len(),
            skipped = result.skipped.len(),
            errors = result.errors.len(),
            "automatic MCP config import finished"
        );

        Ok(result)
    }

    /// Scan external MCP configs and add newly discovered servers to the
    /// `upstream_pending` queue without applying them.
    pub async fn discover_into_pending(&self) -> Result<PendingDiscoveryOutcome, ToolError> {
        let Some(home) = crate::gateway::discovery::home_dir() else {
            return Ok(PendingDiscoveryOutcome::default());
        };

        let discovered =
            tokio::task::spawn_blocking(move || crate::gateway::discovery::discover_all(&home))
                .await
                .map_err(|e| {
                    ToolError::internal_message(format!("discovery task panicked: {e}"))
                })?;

        let _mutation_guard = self.config_mutation.lock().await;
        let mut cfg = self.config.read().await.clone();

        let mut queued = 0usize;
        let mut skipped = 0usize;

        for server in discovered {
            // Skip tombstoned.
            if discovered_is_tombstoned(&cfg, &server) {
                skipped += 1;
                continue;
            }
            // Skip already in upstreams.
            if cfg.upstream.iter().any(|u| u.name == server.name) {
                skipped += 1;
                continue;
            }
            // Skip already pending.
            if cfg.upstream_pending.iter().any(|u| u.name == server.name) {
                skipped += 1;
                continue;
            }
            cfg.upstream_pending.push(server.spec);
            queued += 1;
        }

        if queued > 0 {
            cap_pending_imports(&mut cfg);
            self.persist_config(cfg).await?;
        }

        Ok(PendingDiscoveryOutcome { queued, skipped })
    }

    /// List the `upstream_pending` queue.
    pub async fn list_pending_imports(&self) -> Vec<PendingImportView> {
        let cfg = self.config.read().await;
        cfg.upstream_pending
            .iter()
            .map(pending_import_view)
            .collect()
    }

    /// Approve a pending import by name: move from `upstream_pending` into `upstream`
    /// with `enabled = false` (same as auto-import — operator must explicitly enable).
    pub async fn approve_pending_import(
        &self,
        name: &str,
    ) -> Result<PendingImportApprovalView, ToolError> {
        let import = {
            let _mutation_guard = self.config_mutation.lock().await;
            let mut cfg = self.config.read().await.clone();

            let idx = cfg
                .upstream_pending
                .iter()
                .position(|u| u.name == name)
                .ok_or_else(|| ToolError::Sdk {
                    sdk_kind: "not_found".into(),
                    message: format!("pending import `{name}` not found"),
                })?;

            let mut spec = cfg.upstream_pending.remove(idx);
            let view = pending_import_view(&spec);

            spec.enabled = false;
            cfg.upstream.push(spec);
            self.persist_config(cfg).await?;

            view
        };
        let enrichment_suggestion = self.preview_enrichment_for_new_upstream(name).await;
        Ok(PendingImportApprovalView {
            import,
            enrichment_suggestion,
        })
    }

    /// Reject a pending import by name: tombstone it so it never re-appears.
    pub async fn reject_pending_import(&self, name: &str) -> Result<PendingImportView, ToolError> {
        let _mutation_guard = self.config_mutation.lock().await;
        let mut cfg = self.config.read().await.clone();

        let idx = cfg
            .upstream_pending
            .iter()
            .position(|u| u.name == name)
            .ok_or_else(|| ToolError::Sdk {
                sdk_kind: "not_found".into(),
                message: format!("pending import `{name}` not found"),
            })?;

        let spec = cfg.upstream_pending.remove(idx);
        let view = pending_import_view(&spec);

        // Create a tombstone so this server is never re-discovered.
        if let Some(source) = spec.imported_from {
            cfg.upstream_import_tombstones
                .push(UpstreamImportTombstone::now(spec.name, source));
            cap_import_tombstones(&mut cfg);
        }

        self.persist_config(cfg).await?;

        Ok(view)
    }

    pub async fn list_import_tombstones(&self) -> Vec<ImportTombstoneView> {
        let cfg = self.config.read().await;
        cfg.upstream_import_tombstones
            .iter()
            .map(import_tombstone_view)
            .collect()
    }

    pub async fn clear_import_tombstone(
        &self,
        selector: ImportTombstoneSelector,
    ) -> Result<Vec<ImportTombstoneView>, ToolError> {
        let _mutation_guard = self.config_mutation.lock().await;
        let mut cfg = self.config.read().await.clone();
        let before = cfg.upstream_import_tombstones.len();
        cfg.upstream_import_tombstones
            .retain(|tombstone| !selector.matches_tombstone(tombstone));
        if cfg.upstream_import_tombstones.len() == before {
            return Err(ToolError::Sdk {
                sdk_kind: "not_found".to_string(),
                message: format!("gateway import tombstone `{}` not found", selector.name),
            });
        }

        let views: Vec<_> = cfg
            .upstream_import_tombstones
            .iter()
            .map(import_tombstone_view)
            .collect();
        self.persist_config(cfg).await?;
        Ok(views)
    }

    pub async fn restore_import_tombstone(
        &self,
        selector: ImportTombstoneSelector,
        origin: Option<&str>,
        owner: Option<UpstreamRuntimeOwner>,
    ) -> Result<GatewayView, ToolError> {
        let Some(home) = crate::gateway::discovery::home_dir() else {
            return Err(ToolError::internal_message(
                "cannot determine home directory for MCP config restore",
            ));
        };
        let discovered =
            tokio::task::spawn_blocking(move || crate::gateway::discovery::discover_all(&home))
                .await
                .map_err(|e| {
                    ToolError::internal_message(format!("discovery task panicked: {e}"))
                })?;

        let _mutation_guard = self.config_mutation.lock().await;
        let mut cfg = self.config.read().await.clone();
        if !cfg
            .upstream_import_tombstones
            .iter()
            .any(|tombstone| selector.matches_tombstone(tombstone))
        {
            return Err(ToolError::Sdk {
                sdk_kind: "not_found".to_string(),
                message: format!("gateway import tombstone `{}` not found", selector.name),
            });
        }

        let Some(server) = discovered.into_iter().find(|server| {
            selector.matches_discovered(server) && discovered_is_tombstoned(&cfg, server)
        }) else {
            return Err(ToolError::Sdk {
                sdk_kind: "not_found".to_string(),
                message: format!(
                    "discovered MCP server `{}` no longer matches a tombstoned import",
                    selector.name
                ),
            });
        };

        if cfg
            .upstream
            .iter()
            .any(|upstream| upstream.name == server.spec.name)
        {
            cfg.upstream_import_tombstones
                .retain(|tombstone| !selector.matches_tombstone(tombstone));
            self.persist_config(cfg).await?;
            return self.get(&server.spec.name).await;
        }

        let restored_name = server.spec.name.clone();
        insert_upstream(&mut cfg, server.spec)?;
        self.persist_config(cfg).await?;
        let diff = self.reload_with_origin_unlocked(origin, owner).await?;
        tracing::info!(
            surface = "dispatch",
            service = "gateway",
            action = "gateway.import_tombstones.restore",
            gateway = %restored_name,
            tools_changed = diff.tools_changed,
            resources_changed = diff.resources_changed,
            prompts_changed = diff.prompts_changed,
            "gateway import tombstone restored"
        );
        self.get(&restored_name).await
    }
}
