//! `CodeModeBroker::search` and live read-only catalog construction.

use serde_json::Value;

use crate::dispatch::error::ToolError;
use crate::dispatch::gateway::manager::GatewayManager;
use crate::dispatch::upstream::types::UpstreamRuntimeOwner;

use super::CodeModeBroker;
use super::protocol::CODE_MODE_SEARCH_TIMEOUT;
use super::types::{
    CodeModeCaller, CodeModeCapabilityFilter, CodeModeCatalogEntry, CodeModeSurface,
    sanitize_code_mode_schema,
};
use super::util::serialized_catalog_size;

impl CodeModeBroker<'_> {
    pub async fn search(
        &self,
        code: &str,
        caller: CodeModeCaller,
        surface: CodeModeSurface,
    ) -> Result<Value, ToolError> {
        if !caller.can_read() {
            return Err(ToolError::Sdk {
                sdk_kind: "forbidden".to_string(),
                message: "code_search requires one of scopes: lab:read, lab, lab:admin".to_string(),
            });
        }

        let Some(manager) = self.gateway_manager else {
            return Ok(Value::Array(Vec::new()));
        };

        let require_fresh_catalog = true;
        let owner = caller.runtime_owner(surface);
        let oauth_subject = caller.oauth_subject();
        let (catalog, serialized_size) = self
            .code_search_catalog(manager, require_fresh_catalog, &owner, oauth_subject)
            .await?;
        tracing::info!(
            surface = "dispatch",
            service = "code_search",
            action = "catalog.build",
            catalog_size_bytes = serialized_size,
            entry_count = catalog.len(),
            "Code Mode search catalog ready"
        );

        // Run the caller's JS filter over the catalog inside the Javy runner. The
        // catalog is injected as a global `const tools = [...]`, mirroring the
        // typed proxy `execute` injects. `max_tool_calls = 0` means any stray
        // `callTool` in the search filter errors out — search must not call tools.
        let catalog_json = serde_json::to_string(&catalog).map_err(|err| ToolError::Sdk {
            sdk_kind: "internal_error".to_string(),
            message: format!("failed to serialize Code Mode search catalog: {err}"),
        })?;
        let proxy = format!("const tools = {catalog_json};\n");
        // Search passes the caller's code to the runner *raw* (no
        // `normalize_user_code`). The runner's invoker requires the code to
        // evaluate to a function and throws otherwise, so a non-function search
        // input still surfaces as `server_error` — preserving the contract the
        // old in-process `evaluate_code_search` enforced. Normalizing here would
        // wrap a bare expression like `42` into `async () => 42`, silently
        // turning a contract violation into a successful run.
        let response = self
            .run_in_runner(
                code.to_string(),
                proxy,
                0,
                CODE_MODE_SEARCH_TIMEOUT,
                caller,
                surface,
                0,
                0,
                CodeModeCapabilityFilter::default(),
            )
            .await?;
        // search must return an array/Value; undefined/None → [].
        Ok(response.result.unwrap_or_else(|| Value::Array(Vec::new())))
    }

    pub(in crate::dispatch::gateway::code_mode) async fn code_search_catalog(
        &self,
        manager: &GatewayManager,
        allow_cold_connect: bool,
        owner: &UpstreamRuntimeOwner,
        oauth_subject: Option<&str>,
    ) -> Result<(Vec<CodeModeCatalogEntry>, usize), ToolError> {
        let mut entries = manager
            .code_mode_catalog_tools(allow_cold_connect, Some(owner), oauth_subject)
            .await?
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

        // The catalog is injected as `const tools` into the Boa sandbox and never
        // enters the model context (only the caller's filtered result does), so it
        // is served complete and uncapped — matching Cloudflare's Code Mode design.
        let serialized_size = serialized_catalog_size(&entries)?;

        Ok((entries, serialized_size))
    }
}
