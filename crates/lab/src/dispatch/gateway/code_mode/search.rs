//! Live in-sandbox discovery catalog construction.

use std::path::{Path, PathBuf};

use crate::dispatch::error::ToolError;
use crate::dispatch::gateway::manager::GatewayManager;
use crate::dispatch::helpers::lab_home;
use crate::dispatch::snippets::store::{builtin_snippet_dir, list_snippets};
use crate::dispatch::upstream::types::{UpstreamRuntimeOwner, UpstreamTool};

use super::CodeModeBroker;
use super::types::{CodeModeCatalogEntry, sanitize_code_mode_schema};
use super::util::serialized_catalog_size;

impl CodeModeBroker<'_> {
    /// Build the execute-path proxy catalog without re-probing healthy
    /// upstreams.
    ///
    /// Unlike `code_mode_catalog_allowed` (which forces a full growth-detecting
    /// reprobe via `allow_cold_connect = true`), this
    /// connects only upstreams that are not already healthy, then renders the
    /// catalog through the same `code_mode_catalog_from_tools` render cache.
    /// Tool calls resolve live, so a slightly-stale proxy is harmless on the
    /// execute hot path (lab-5h5xl).
    pub(in crate::dispatch::gateway::code_mode) async fn ensure_ready_code_mode_catalog_for_proxy(
        &self,
        manager: &GatewayManager,
        owner: &UpstreamRuntimeOwner,
        oauth_subject: Option<&str>,
        allowed_upstreams: Option<&std::collections::BTreeSet<String>>,
        include_snippets: bool,
    ) -> Result<(Vec<CodeModeCatalogEntry>, String, usize), ToolError> {
        let raw_tools = manager
            .code_mode_catalog_tools_ensure_ready(Some(owner), oauth_subject, allowed_upstreams)
            .await?;
        self.code_mode_catalog_from_tools(manager, raw_tools, include_snippets)
            .await
    }

    pub(in crate::dispatch::gateway::code_mode) async fn cached_code_mode_catalog_for_proxy(
        &self,
        manager: &GatewayManager,
        owner: &UpstreamRuntimeOwner,
        oauth_subject: Option<&str>,
        include_snippets: bool,
    ) -> Result<(Vec<CodeModeCatalogEntry>, String, usize), ToolError> {
        let raw_tools = manager
            .code_mode_catalog_tools_cached(Some(owner), oauth_subject)
            .await?;
        self.code_mode_catalog_from_tools(manager, raw_tools, include_snippets)
            .await
    }

    /// Build the discovery catalog through the growth-detecting reprobe path
    /// (`code_mode_catalog_tools_allowed`).
    ///
    /// The production execute path renders its proxy catalog via the cheaper
    /// `_for_proxy` variants above, so this full-reprobe builder is exercised
    /// only by the catalog-expansion tests today — hence the `not(test)`
    /// allow-dead.
    #[cfg_attr(not(test), allow(dead_code))]
    pub(in crate::dispatch::gateway::code_mode) async fn code_mode_catalog_allowed(
        &self,
        manager: &GatewayManager,
        allow_cold_connect: bool,
        owner: &UpstreamRuntimeOwner,
        oauth_subject: Option<&str>,
        allowed_upstreams: Option<&std::collections::BTreeSet<String>>,
        include_snippets: bool,
    ) -> Result<(Vec<CodeModeCatalogEntry>, String, usize), ToolError> {
        let raw_tools = manager
            .code_mode_catalog_tools_allowed(
                allow_cold_connect,
                Some(owner),
                oauth_subject,
                allowed_upstreams,
            )
            .await?;
        self.code_mode_catalog_from_tools(manager, raw_tools, include_snippets)
            .await
    }

    async fn code_mode_catalog_from_tools(
        &self,
        manager: &GatewayManager,
        raw_tools: Vec<UpstreamTool>,
        include_snippets: bool,
    ) -> Result<(Vec<CodeModeCatalogEntry>, String, usize), ToolError> {
        // --- P-H3: catalog render cache ---
        // Compute a cheap fingerprint from the sorted healthy tool ids. This
        // detects upstream additions/removals/renames without needing a pool
        // generation counter. The sort makes the fingerprint order-independent
        // (the pool may return tools in any order).
        let snippet_fingerprint = if include_snippets {
            snippet_directory_fingerprint("admin")
                .await?
                .unwrap_or_else(|| "snippets:absent".to_string())
        } else {
            "snippets:hidden".to_string()
        };

        let fingerprint = {
            let mut ids: Vec<String> = raw_tools
                .iter()
                .map(|t| format!("{}::{}", t.upstream_name, t.tool.name))
                .collect();
            ids.sort_unstable();
            format!("tools:\n{}\n{snippet_fingerprint}", ids.join("\n"))
        };

        // Check the manager-level render cache before re-building entries
        // (which includes `generate_tool_types` per tool — non-trivial work).
        if let Some((entries, catalog_json, serialized_size)) =
            manager.cached_catalog_render(&fingerprint).await
        {
            tracing::debug!(
                surface = "dispatch",
                service = super::SERVICE,
                action = "catalog.build",
                entry_count = entries.len(),
                "Code Mode discovery catalog served from render cache"
            );
            return Ok((entries, catalog_json, serialized_size));
        }

        // Cache miss — build entries (includes `generate_tool_types` per entry).
        let mut entries = raw_tools
            .into_iter()
            .map(|tool| {
                let upstream = tool.upstream_name.to_string();
                let name = tool.tool.name.to_string();
                let description = tool
                    .tool
                    .description
                    .as_ref()
                    .map(|description| description.to_string())
                    .unwrap_or_default();
                CodeModeCatalogEntry::upstream_tool(
                    &upstream,
                    &name,
                    &crate::dispatch::gateway::projection::sanitize_tool_text(&description, 2048),
                    sanitize_code_mode_schema(tool.input_schema),
                    sanitize_code_mode_schema(tool.output_schema),
                )
            })
            .collect::<Vec<_>>();

        if include_snippets {
            let snippets = snippet_metadata_for_catalog(manager, &snippet_fingerprint).await?;
            entries.extend(snippets.iter().map(CodeModeCatalogEntry::snippet));
        }

        entries.sort_by(|a, b| {
            a.kind.cmp(&b.kind).then_with(|| {
                a.upstream
                    .cmp(&b.upstream)
                    .then_with(|| a.name.cmp(&b.name))
            })
        });

        // The catalog is injected as `const tools` into the javy runner and
        // never enters the model context, so it is served complete and uncapped.
        let serialized_size = serialized_catalog_size(&entries)?;

        // Serialize once and store so subsequent calls within the same pool
        // state reuse this string directly.
        let catalog_json = serde_json::to_string(&entries).map_err(|err| ToolError::Sdk {
            sdk_kind: "internal_error".to_string(),
            message: format!("failed to serialize Code Mode discovery catalog: {err}"),
        })?;

        // Store the render for the next discovery lookup.
        manager
            .store_catalog_render_cache(super::CatalogRenderCache {
                fingerprint,
                entries: entries.clone(),
                catalog_json: catalog_json.clone(),
                serialized_size,
            })
            .await;

        Ok((entries, catalog_json, serialized_size))
    }
}

async fn snippet_metadata_for_catalog(
    manager: &GatewayManager,
    fingerprint: &str,
) -> Result<Vec<crate::dispatch::snippets::store::SnippetInfo>, ToolError> {
    if let Some(snippets) = manager.cached_snippet_metadata(fingerprint).await {
        return Ok(snippets);
    }

    let lab_home = lab_home();
    let builtin_dir = builtin_snippet_dir();
    let snippets = tokio::task::spawn_blocking(move || list_snippets(&lab_home, &builtin_dir))
        .await
        .map_err(|err| {
            ToolError::internal_message(format!("snippet metadata task failed: {err}"))
        })??;

    manager
        .store_snippet_metadata_cache(super::SnippetMetadataCache {
            fingerprint: fingerprint.to_string(),
            entries: snippets.clone(),
        })
        .await;
    Ok(snippets)
}

async fn snippet_directory_fingerprint(policy: &str) -> Result<Option<String>, ToolError> {
    let lab_home = lab_home();
    let user_dir = crate::dispatch::snippets::store::user_snippet_dir(&lab_home);
    let builtin_dir = builtin_snippet_dir();
    let policy = policy.to_string();
    tokio::task::spawn_blocking(move || {
        let mut parts = vec![format!("snippet_policy:{policy}")];
        let mut saw_dir = false;
        for dir in [user_dir, builtin_dir] {
            match directory_fingerprint_part(&dir)? {
                Some(part) => {
                    saw_dir = true;
                    parts.push(part);
                }
                None => parts.push(format!("{}:absent", dir.display())),
            }
        }
        Ok::<_, ToolError>(saw_dir.then(|| parts.join("\n")))
    })
    .await
    .map_err(|err| ToolError::internal_message(format!("snippet fingerprint task failed: {err}")))?
}

fn directory_fingerprint_part(dir: &Path) -> Result<Option<String>, ToolError> {
    let metadata = match std::fs::metadata(dir) {
        Ok(metadata) => metadata,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(err) => {
            return Err(ToolError::internal_message(format!(
                "read snippets directory `{}` metadata failed: {err}",
                dir.display()
            )));
        }
    };
    if !metadata.is_dir() {
        return Ok(None);
    }
    let modified = metadata
        .modified()
        .ok()
        .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|duration| duration.as_nanos())
        .unwrap_or(0);
    let entries = directory_entries_fingerprint(dir)?;
    Ok(Some(format!(
        "{}:{}:{}:{}",
        normalize_path(dir),
        modified,
        metadata.len(),
        entries.join("|")
    )))
}

fn directory_entries_fingerprint(dir: &Path) -> Result<Vec<String>, ToolError> {
    let entries = std::fs::read_dir(dir).map_err(|err| {
        ToolError::internal_message(format!(
            "read snippets directory `{}` failed: {err}",
            dir.display()
        ))
    })?;
    let mut parts = Vec::new();
    for entry in entries {
        let entry = entry.map_err(|err| {
            ToolError::internal_message(format!(
                "read snippets directory `{}` entry failed: {err}",
                dir.display()
            ))
        })?;
        let path = entry.path();
        let metadata = entry.metadata().map_err(|err| {
            ToolError::internal_message(format!(
                "read snippet entry `{}` metadata failed: {err}",
                path.display()
            ))
        })?;
        let modified = metadata
            .modified()
            .ok()
            .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|duration| duration.as_nanos())
            .unwrap_or(0);
        parts.push(format!(
            "{}:{}:{}:{}",
            entry.file_name().to_string_lossy(),
            metadata.is_file(),
            metadata.len(),
            modified
        ));
    }
    parts.sort_unstable();
    Ok(parts)
}

fn normalize_path(path: &Path) -> String {
    path.canonicalize()
        .unwrap_or_else(|_| PathBuf::from(path))
        .display()
        .to_string()
}
