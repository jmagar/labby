//! Host-side Code Mode discovery catalog construction.
//!
//! Projects the gateway's live `UpstreamTool` set (plus snippet metadata) into
//! the crate-neutral `ToolDescriptor` catalog and serves it through the
//! manager-level render cache. Called from `code_mode_host.rs`'s
//! `CodeModeHost::list_tools` impl.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use lab_codemode::snippet::store::{SnippetInfo, builtin_snippet_dir, list_snippets};
use lab_codemode::{ToolDescriptor, ToolsRender, serialized_catalog_size};

use lab_runtime::error::ToolError;
use crate::gateway::manager::GatewayManager;
use crate::gateway::projection::{sanitize_schema, sanitize_tool_text};
use lab_runtime::lab_home;
use crate::upstream::types::{UpstreamRuntimeOwner, UpstreamTool};

/// Build (or serve from cache) the Code Mode discovery catalog for the proxy.
///
/// `use_cache` selects the on-disk one-shot CLI catalog cache vs the live
/// cold-connect path; `allowed_upstreams` scopes the projected tool set.
pub(crate) async fn build_tools_render(
    manager: &GatewayManager,
    allow_cold_connect: bool,
    owner: &UpstreamRuntimeOwner,
    oauth_subject: Option<&str>,
    allowed_upstreams: Option<&BTreeSet<String>>,
    include_snippets: bool,
    use_cache: bool,
) -> Result<ToolsRender, ToolError> {
    let raw_tools = if use_cache {
        manager
            .code_mode_catalog_tools_cached(Some(owner), oauth_subject)
            .await?
    } else {
        manager
            .code_mode_catalog_tools_allowed(
                allow_cold_connect,
                Some(owner),
                oauth_subject,
                allowed_upstreams,
            )
            .await?
    };
    catalog_from_tools(manager, raw_tools, include_snippets).await
}

async fn catalog_from_tools(
    manager: &GatewayManager,
    raw_tools: Vec<UpstreamTool>,
    include_snippets: bool,
) -> Result<ToolsRender, ToolError> {
    // --- catalog render cache ---
    // Compute a cheap fingerprint from the sorted healthy tool ids. This detects
    // upstream additions/removals/renames without needing a pool generation
    // counter. The sort makes the fingerprint order-independent.
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

    if let Some((entries, catalog_json, serialized_size)) =
        manager.cached_catalog_render(&fingerprint).await
    {
        tracing::debug!(
            surface = "dispatch",
            service = "codemode",
            action = "catalog.build",
            entry_count = entries.len(),
            "Code Mode discovery catalog served from render cache"
        );
        return Ok(ToolsRender {
            entries,
            catalog_json,
            serialized_size,
        });
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
            ToolDescriptor::tool(
                &upstream,
                &name,
                &sanitize_tool_text(&description, 2048),
                sanitize_schema(tool.input_schema),
                sanitize_schema(tool.output_schema),
            )
        })
        .collect::<Vec<_>>();

    if include_snippets {
        let snippets = snippet_metadata_for_catalog(manager, &snippet_fingerprint).await?;
        entries.extend(snippets.iter().map(ToolDescriptor::snippet));
    }

    entries.sort_by(|a, b| {
        a.kind.cmp(&b.kind).then_with(|| {
            a.namespace
                .cmp(&b.namespace)
                .then_with(|| a.name.cmp(&b.name))
        })
    });

    // The catalog is injected as `const tools` into the javy runner and never
    // enters the model context, so it is served complete and uncapped.
    let serialized_size = serialized_catalog_size(&entries)?;
    let catalog_json = serde_json::to_string(&entries).map_err(|err| ToolError::Sdk {
        sdk_kind: "internal_error".to_string(),
        message: format!("failed to serialize Code Mode discovery catalog: {err}"),
    })?;

    manager
        .store_catalog_render_cache(super::CatalogRenderCache {
            fingerprint,
            entries: entries.clone(),
            catalog_json: catalog_json.clone(),
            serialized_size,
        })
        .await;

    Ok(ToolsRender {
        entries,
        catalog_json,
        serialized_size,
    })
}

async fn snippet_metadata_for_catalog(
    manager: &GatewayManager,
    fingerprint: &str,
) -> Result<Vec<SnippetInfo>, ToolError> {
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
    let user_dir = lab_codemode::snippet::store::user_snippet_dir(&lab_home);
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
