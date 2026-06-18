//! `CodeModeBroker::search` and live in-sandbox discovery catalog construction.

use serde_json::Value;

use crate::dispatch::error::ToolError;
use crate::dispatch::gateway::manager::GatewayManager;
use crate::dispatch::upstream::types::UpstreamRuntimeOwner;

use super::CodeModeBroker;
use super::protocol::CODE_MODE_DISCOVERY_TIMEOUT;
use super::types::{
    CodeModeCaller, CodeModeCapabilityFilter, CodeModeCatalogEntry, CodeModeSurface,
    sanitize_code_mode_schema,
};
use super::util::serialized_catalog_size;

impl CodeModeBroker<'_> {
    #[allow(dead_code)]
    pub async fn search(
        &self,
        code: &str,
        caller: CodeModeCaller,
        surface: CodeModeSurface,
    ) -> Result<Value, ToolError> {
        self.search_allowed(code, caller, surface, None).await
    }

    pub async fn search_allowed(
        &self,
        code: &str,
        caller: CodeModeCaller,
        surface: CodeModeSurface,
        allowed_upstreams: Option<&std::collections::BTreeSet<String>>,
    ) -> Result<Value, ToolError> {
        if !caller.can_read() {
            return Err(ToolError::Sdk {
                sdk_kind: "forbidden".to_string(),
                message: "codemode.search requires one of scopes: lab:read, lab, lab:admin"
                    .to_string(),
            });
        }

        let Some(manager) = self.gateway_manager else {
            return Ok(Value::Array(Vec::new()));
        };

        // `require_fresh_catalog = true` triggers `refresh_code_mode_catalog`,
        // which is now bounded by a 30 s wall-clock TTL and a single-flight
        // guard — back-to-back searches do not re-probe upstreams within the
        // freshness window. See `manager/code_mode_runtime.rs`.
        let require_fresh_catalog = true;
        let owner = caller.runtime_owner(surface);
        let oauth_subject = caller.oauth_subject();
        // Returns (entries, catalog_json, serialized_size) — all from the
        // render cache when the healthy tool set has not changed.
        let (catalog, catalog_json, serialized_size) = self
            .code_mode_catalog_allowed(
                manager,
                require_fresh_catalog,
                &owner,
                oauth_subject,
                allowed_upstreams,
            )
            .await?;
        tracing::info!(
            surface = "dispatch",
            service = "codemode",
            action = "catalog.build",
            catalog_size_bytes = serialized_size,
            entry_count = catalog.len(),
            "Code Mode discovery catalog ready"
        );

        // Run the caller's JS filter over the catalog inside the Javy runner. The
        // catalog is injected as a global `const tools = [...]`. `max_tool_calls = 0`
        // means any stray `callTool` in the discovery filter errors out — discovery
        // must not call tools.
        //
        // Use the pre-serialized `catalog_json` from the render cache so we do
        // not pay `serde_json::to_string` again when the catalog is unchanged.
        let proxy = format!("const tools = {catalog_json};\n");
        // Discovery passes the caller's code to the runner *raw* (no
        // `normalize_user_code`). The runner's invoker requires the code to
        // evaluate to a function and throws otherwise, so a non-function search
        // input still surfaces as `server_error` — preserving the contract the
        // old in-process `evaluate_code_mode_catalog` enforced. Normalizing here would
        // wrap a bare expression like `42` into `async () => 42`, silently
        // turning a contract violation into a successful run.
        let response = self
            .run_in_runner(
                code.to_string(),
                proxy,
                0,
                CODE_MODE_DISCOVERY_TIMEOUT,
                caller,
                surface,
                0,
                0,
                false,
                CodeModeCapabilityFilter::default(),
            )
            .await
            .map_err(super::types::CodeModeExecutionError::into_tool_error)?;
        // Discovery must return an array/Value; undefined/None -> [].
        Ok(response.result.unwrap_or_else(|| Value::Array(Vec::new())))
    }

    /// Build or return the cached Code Mode discovery catalog.
    ///
    /// Returns `(entries, catalog_json, serialized_size)`. The `catalog_json`
    /// is the pre-serialized `serde_json::to_string(&entries)` string, ready
    /// to inject as `const tools = ...;` into the runner. Both are served from
    /// the manager-level render cache when the healthy tool fingerprint matches,
    /// avoiding repeated `generate_tool_types` calls and JSON serialization.
    #[allow(dead_code)]
    pub(in crate::dispatch::gateway::code_mode) async fn code_mode_catalog(
        &self,
        manager: &GatewayManager,
        allow_cold_connect: bool,
        owner: &UpstreamRuntimeOwner,
        oauth_subject: Option<&str>,
    ) -> Result<(Vec<CodeModeCatalogEntry>, String, usize), ToolError> {
        self.code_mode_catalog_allowed(manager, allow_cold_connect, owner, oauth_subject, None)
            .await
    }

    pub(in crate::dispatch::gateway::code_mode) async fn code_mode_catalog_allowed(
        &self,
        manager: &GatewayManager,
        allow_cold_connect: bool,
        owner: &UpstreamRuntimeOwner,
        oauth_subject: Option<&str>,
        allowed_upstreams: Option<&std::collections::BTreeSet<String>>,
    ) -> Result<(Vec<CodeModeCatalogEntry>, String, usize), ToolError> {
        let raw_tools = manager
            .code_mode_catalog_tools_allowed(
                allow_cold_connect,
                Some(owner),
                oauth_subject,
                allowed_upstreams,
            )
            .await?;

        // --- P-H3: catalog render cache ---
        // Compute a cheap fingerprint from the sorted healthy tool ids. This
        // detects upstream additions/removals/renames without needing a pool
        // generation counter. The sort makes the fingerprint order-independent
        // (the pool may return tools in any order).
        let fingerprint = {
            let mut ids: Vec<String> = raw_tools
                .iter()
                .map(|t| format!("{}::{}", t.upstream_name, t.tool.name))
                .collect();
            ids.sort_unstable();
            ids.join("\n")
        };

        // Check the manager-level render cache before re-building entries
        // (which includes `generate_tool_types` per tool — non-trivial work).
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

        entries.sort_by(|a, b| {
            a.upstream
                .cmp(&b.upstream)
                .then_with(|| a.name.cmp(&b.name))
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
