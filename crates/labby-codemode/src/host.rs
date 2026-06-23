//! `CodeModeHost`: the tool-source-neutral seam between the Code Mode kernel and
//! whatever provides its tools (an MCP proxy pool, a REST client, an in-memory
//! stub — the kernel can't tell).
//!
//! The trait vocabulary is deliberately neutral. A tool is an opaque string
//! `id` (`<namespace>::<tool>`) plus JSON params; a tool descriptor is the
//! neutral [`ToolDescriptor`]; the visibility filter is the neutral
//! [`ToolScope`]. Each host converts its own tool representation into a
//! `ToolDescriptor` inside its `CodeModeHost` impl, so the kernel never learns
//! what backs the namespace.

use serde_json::Value;

use crate::error::ToolError;
use crate::pool::RunnerPool;
use crate::types::{CodeModeCaller, CodeModeSurface, ToolDescriptor, ToolScope, UiLink};
use labby_runtime::CodeModeConfig;

/// A rendered Code Mode discovery catalog: the descriptors the sandbox's
/// `search`/`describe`/proxy read, plus their pre-serialized JSON form.
///
/// Hosts may serve this from a render cache keyed on a cheap fingerprint of
/// their tool set; the kernel does not require caching and treats this purely
/// as a projection.
#[derive(Debug, Clone)]
pub struct ToolsRender {
    /// The descriptors (tools + snippets) visible to this execution.
    pub entries: Vec<ToolDescriptor>,
    /// `serde_json::to_string(&entries)` — the `const tools = ...` payload.
    pub catalog_json: String,
    /// Serialized catalog size in bytes (for tracing).
    pub serialized_size: usize,
}

/// A snippet resolved by the host: its canonical name plus the JS source and
/// the merged input the runner should execute it with.
#[derive(Debug, Clone)]
pub struct ResolvedSnippet {
    pub name: String,
    pub code: String,
    pub input: Value,
}

/// The result of one host-brokered tool call: the unwrapped JSON value plus an
/// optional captured MCP Apps (mcp-ui) widget link (last-wins across the run).
#[derive(Debug, Clone)]
pub struct ToolCallOutcome {
    pub value: Value,
    pub ui: Option<UiLink>,
}

/// Injects the tool source into the Code Mode kernel.
///
/// Implementations live entirely outside this crate. Methods take the neutral
/// [`ToolScope`] / [`CodeModeCaller`] / [`CodeModeSurface`]; how those map onto
/// a concrete credential or connection model is the host's business.
pub trait CodeModeHost: Send + Sync {
    /// Project the host's tool source into the in-sandbox discovery catalog the
    /// `tools` proxy + in-sandbox `search`/`describe` read. Pure projection; no
    /// transport implied.
    fn list_tools(
        &self,
        caller: &CodeModeCaller,
        surface: CodeModeSurface,
        scope: &ToolScope,
        include_snippets: bool,
        use_cache: bool,
    ) -> impl Future<Output = Result<ToolsRender, ToolError>> + Send;

    /// Route a `callTool(id, params)` to the host's tool source and return the
    /// unwrapped result (plus any captured widget link). The kernel has already
    /// checked the id against `scope`.
    fn call_tool(
        &self,
        id: &str,
        params: Value,
        caller: &CodeModeCaller,
        surface: CodeModeSurface,
        scope: &ToolScope,
    ) -> impl Future<Output = Result<ToolCallOutcome, ToolError>> + Send;

    /// Resolve a Code Mode snippet by name (engine lives in-crate; only the
    /// source lookup is host-provided so policy/visibility stays host-side).
    fn resolve_snippet(
        &self,
        name: &str,
        input: Value,
    ) -> impl Future<Output = Result<ResolvedSnippet, ToolError>> + Send;

    /// Code Mode configuration (timeouts, log/response caps).
    fn config(&self) -> impl Future<Output = CodeModeConfig> + Send;

    /// The host-owned warm runner pool the kernel checks runners out of.
    fn runner_pool(&self) -> &RunnerPool;
}

/// A no-op host used by tests that drive the runner kernel directly without a
/// real tool source: it exposes no tools, rejects all tool/snippet calls, and
/// owns its own warm pool. Never constructed in the production build.
#[cfg(test)]
pub(crate) struct NoopHost {
    pool: RunnerPool,
}

#[cfg(test)]
impl Default for NoopHost {
    fn default() -> Self {
        Self {
            pool: RunnerPool::from_env(),
        }
    }
}

#[cfg(test)]
impl CodeModeHost for NoopHost {
    async fn list_tools(
        &self,
        _caller: &CodeModeCaller,
        _surface: CodeModeSurface,
        _scope: &ToolScope,
        _include_snippets: bool,
        _use_cache: bool,
    ) -> Result<ToolsRender, ToolError> {
        Ok(ToolsRender {
            entries: Vec::new(),
            catalog_json: "[]".to_string(),
            serialized_size: 2,
        })
    }

    async fn call_tool(
        &self,
        _id: &str,
        _params: Value,
        _caller: &CodeModeCaller,
        _surface: CodeModeSurface,
        _scope: &ToolScope,
    ) -> Result<ToolCallOutcome, ToolError> {
        Err(ToolError::Sdk {
            sdk_kind: "unknown_tool".to_string(),
            message: "NoopHost exposes no tools".to_string(),
        })
    }

    async fn resolve_snippet(
        &self,
        _name: &str,
        _input: Value,
    ) -> Result<ResolvedSnippet, ToolError> {
        Err(ToolError::Sdk {
            sdk_kind: "not_found".to_string(),
            message: "NoopHost exposes no snippets".to_string(),
        })
    }

    async fn config(&self) -> CodeModeConfig {
        CodeModeConfig::default()
    }

    fn runner_pool(&self) -> &RunnerPool {
        &self.pool
    }
}
