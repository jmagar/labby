//! Core Code Mode value types: tool ids, catalog entries, execution responses,
//! callers, surfaces, and the capability filter.

use std::collections::{BTreeSet, VecDeque};

use serde::Serialize;
use serde::ser::SerializeStruct;
use serde_json::Value;

use crate::dispatch::error::ToolError;
use crate::dispatch::gateway::SHARED_GATEWAY_OAUTH_SUBJECT;
use crate::dispatch::upstream::types::UpstreamRuntimeOwner;

use super::util::{invalid_code_mode_id, lab_action_unknown_tool};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CodeModeToolId {
    pub(crate) raw: String,
    pub(crate) reference: CodeModeToolRef,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CodeModeToolRef {
    UpstreamTool { upstream: String, tool: String },
}

impl CodeModeToolId {
    pub fn parse(raw: &str) -> Result<Self, ToolError> {
        let raw = raw.trim();
        if raw.is_empty() {
            return Err(invalid_code_mode_id("Code Mode tool id must not be empty"));
        }

        if raw.starts_with("lab::") {
            return Err(lab_action_unknown_tool());
        }

        let mut parts = raw.split("::");
        if let (Some(upstream), Some(tool), None) = (parts.next(), parts.next(), parts.next()) {
            if upstream.trim().is_empty() || tool.trim().is_empty() {
                return Err(invalid_code_mode_id(
                    "Code Mode ids must include upstream and tool",
                ));
            }
            return Ok(Self {
                raw: raw.to_string(),
                reference: CodeModeToolRef::UpstreamTool {
                    upstream: upstream.trim().to_string(),
                    tool: tool.trim().to_string(),
                },
            });
        }

        Err(invalid_code_mode_id(
            "Code Mode ids must use <upstream>::<tool>",
        ))
    }
}

#[must_use]
pub fn upstream_tool_id(upstream: &str, tool: &str) -> String {
    format!("{upstream}::{tool}")
}

#[must_use]
pub fn sanitize_code_mode_schema(schema: Option<Value>) -> Option<Value> {
    crate::dispatch::gateway::projection::sanitize_schema(schema)
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct CodeModeCatalogEntry {
    pub id: String,
    pub name: String,
    pub upstream: String,
    pub description: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub schema: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub output_schema: Option<Value>,
    pub signature: String,
    pub dts: String,
}

impl CodeModeCatalogEntry {
    #[must_use]
    pub fn upstream_tool(
        upstream: &str,
        tool: &str,
        description: &str,
        schema: Option<Value>,
        output_schema: Option<Value>,
    ) -> Self {
        let types = super::types_legacy::generate_tool_types(
            upstream,
            tool,
            description,
            schema.as_ref(),
            output_schema.as_ref(),
        );
        Self {
            id: upstream_tool_id(upstream, tool),
            name: tool.to_string(),
            upstream: upstream.to_string(),
            description: description.to_string(),
            schema,
            output_schema,
            signature: types.signature,
            dts: types.dts,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct CodeModeExecutionResponse {
    /// The final return value of the async function. None when the function
    /// returns undefined or throws (the throw case surfaces via ToolError).
    /// Explicit JavaScript `null` is represented as `Some(Value::Null)` and
    /// serializes as `"result": null`; undefined omits the field.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<Value>,
    pub calls: Vec<CodeModeExecutedCall>,
    /// Captured console.log/warn/error lines from the runner. Sourced from the
    /// javy runner subprocess (drained from its stderr); the current javy path
    /// returns no protocol-carried logs, so this is empty until console capture
    /// is wired through.
    pub logs: Vec<String>,
}

/// Lightweight metadata for one host-brokered tool call. Cloudflare parity:
/// the per-call result payload is NOT carried here — only the model needs the
/// final `result`. Recording full per-call results bloated context and risked
/// leaking secrets through the truncation preview.
#[derive(Debug, Clone, PartialEq)]
pub struct CodeModeExecutedCall {
    pub id: String,
    pub ok: bool,
    pub elapsed_ms: u128,
    /// Redacted/capped params captured at the broker boundary. Raw params must
    /// never be stored in this public trace type.
    pub params: Option<Value>,
    pub error_kind: Option<String>,
}

impl Serialize for CodeModeExecutedCall {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let (upstream, tool) = split_code_mode_call_id(&self.id);
        let mut state = serializer.serialize_struct("CodeModeExecutedCall", 7)?;
        state.serialize_field("id", &self.id)?;
        state.serialize_field("upstream", upstream)?;
        state.serialize_field("tool", tool)?;
        state.serialize_field("ok", &self.ok)?;
        state.serialize_field("elapsed_ms", &self.elapsed_ms)?;
        if let Some(params) = &self.params {
            state.serialize_field("params", params)?;
        }
        if let Some(error_kind) = &self.error_kind {
            state.serialize_field("error_kind", error_kind)?;
        }
        state.end()
    }
}

#[must_use]
pub(in crate::dispatch::gateway::code_mode) fn split_code_mode_call_id(id: &str) -> (&str, &str) {
    id.split_once("::")
        .map_or(("", id), |(upstream, tool)| (upstream, tool))
}

#[derive(Debug, Clone)]
pub(crate) struct CodeModeExecutionError {
    error: ToolError,
    calls: Vec<CodeModeExecutedCall>,
}

impl CodeModeExecutionError {
    #[must_use]
    pub(crate) fn with_trace(error: ToolError, calls: Vec<CodeModeExecutedCall>) -> Self {
        Self { error, calls }
    }

    #[must_use]
    pub(crate) fn kind(&self) -> &str {
        self.error.kind()
    }

    #[must_use]
    pub(crate) fn calls(&self) -> &[CodeModeExecutedCall] {
        &self.calls
    }

    #[must_use]
    pub(crate) fn into_tool_error(self) -> ToolError {
        self.error
    }
}

impl std::fmt::Display for CodeModeExecutionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.error.fmt(f)
    }
}

impl std::error::Error for CodeModeExecutionError {}

impl From<ToolError> for CodeModeExecutionError {
    fn from(error: ToolError) -> Self {
        Self::with_trace(error, Vec::new())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CodeModeHistoryKind {
    Search,
    Execute,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct CodeModeHistoryEntry {
    pub seq: u64,
    pub kind: CodeModeHistoryKind,
    pub ok: bool,
    pub elapsed_ms: u128,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error_kind: Option<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub calls: Vec<CodeModeExecutedCall>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub match_count: Option<usize>,
}

#[derive(Debug, Clone)]
pub struct CodeModeHistory {
    entries: VecDeque<CodeModeHistoryEntry>,
    max_entries: usize,
    max_bytes: usize,
    next_seq: u64,
}

impl Default for CodeModeHistory {
    fn default() -> Self {
        Self::new(50, 256 * 1024)
    }
}

impl CodeModeHistory {
    #[must_use]
    pub fn new(max_entries: usize, max_bytes: usize) -> Self {
        Self {
            entries: VecDeque::new(),
            max_entries: max_entries.max(1),
            max_bytes: max_bytes.max(1024),
            next_seq: 1,
        }
    }

    pub fn push(&mut self, mut entry: CodeModeHistoryEntry) {
        entry.seq = self.next_seq;
        self.next_seq = self.next_seq.saturating_add(1);
        self.entries.push_back(entry);
        self.trim();
    }

    #[must_use]
    pub fn snapshot(&self) -> Vec<CodeModeHistoryEntry> {
        self.entries.iter().cloned().collect()
    }

    fn trim(&mut self) {
        while self.entries.len() > self.max_entries {
            self.entries.pop_front();
        }
        while self.serialized_size() > self.max_bytes && self.entries.len() > 1 {
            self.entries.pop_front();
        }
        if self.serialized_size() > self.max_bytes
            && let Some(entry) = self.entries.pop_back()
        {
            self.entries
                .push_back(Self::oversized_entry_sentinel(entry.seq, entry.kind));
        }
    }

    fn serialized_size(&self) -> usize {
        serde_json::to_vec(&self.entries)
            .map(|bytes| bytes.len())
            .unwrap_or(usize::MAX)
    }

    fn oversized_entry_sentinel(seq: u64, kind: CodeModeHistoryKind) -> CodeModeHistoryEntry {
        CodeModeHistoryEntry {
            seq,
            kind,
            ok: false,
            elapsed_ms: 0,
            error_kind: Some("history_entry_too_large".to_string()),
            calls: Vec::new(),
            match_count: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CodeModeCaller {
    TrustedLocal,
    Scoped {
        scopes: Vec<String>,
        /// JWT `sub` claim for the caller, when available. Used as the upstream
        /// OAuth subject only for *non-admin* callers, so a user with their own
        /// upstream grant authenticates as themselves. `lab:admin` callers (and
        /// callers with no `sub`) collapse to `SHARED_GATEWAY_OAUTH_SUBJECT` —
        /// see [`CodeModeCaller::oauth_subject`] for the rationale.
        sub: Option<String>,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CodeModeSurface {
    Mcp,
    Cli,
}

/// Whether a destructive upstream tool call is permitted for this caller.
/// Code Mode execution is already scope-gated; do not add a second host-side
/// confirmation gate based on upstream catalog metadata.
#[must_use]
pub(in crate::dispatch::gateway::code_mode) fn destructive_permitted(
    surface: CodeModeSurface,
    caller: &CodeModeCaller,
) -> bool {
    match surface {
        CodeModeSurface::Cli => true,
        CodeModeSurface::Mcp => caller.can_execute(),
    }
}

impl CodeModeCaller {
    #[must_use]
    pub fn can_read(&self) -> bool {
        match self {
            Self::TrustedLocal => true,
            Self::Scoped { scopes, .. } => scopes
                .iter()
                .any(|scope| matches!(scope.as_str(), "lab:read" | "lab" | "lab:admin")),
        }
    }

    #[must_use]
    pub fn can_execute(&self) -> bool {
        match self {
            Self::TrustedLocal => true,
            Self::Scoped { scopes, .. } => scopes
                .iter()
                .any(|scope| matches!(scope.as_str(), "lab" | "lab:admin")),
        }
    }

    #[must_use]
    pub fn runtime_owner(&self, surface: CodeModeSurface) -> UpstreamRuntimeOwner {
        let surface = match surface {
            CodeModeSurface::Mcp => "mcp",
            CodeModeSurface::Cli => "cli",
        };
        let subject = match self {
            Self::TrustedLocal => None,
            Self::Scoped { sub, .. } => sub.clone(),
        };
        let raw = subject
            .as_ref()
            .map(|subject| format!("{surface}:{subject}"))
            .unwrap_or_else(|| format!("{surface}:trusted-local"));
        UpstreamRuntimeOwner {
            surface: surface.to_string(),
            subject,
            request_id: None,
            session_id: None,
            client_name: None,
            raw: Some(raw),
        }
    }

    #[must_use]
    pub fn oauth_subject(&self) -> Option<&str> {
        match self {
            Self::TrustedLocal => Some(SHARED_GATEWAY_OAUTH_SUBJECT),
            // Parity with `oauth_upstream_subject_for_request` (the direct
            // upstream-tool-call path): admin/operator callers share the single
            // gateway-owned upstream credential rather than a per-user grant that
            // was never provisioned. Without this collapse, an admin caller's raw
            // `sub` misses the credential store (`initialize_from_store` → false),
            // so the proactive token refresh in `build_auth_client` is never
            // reached and OAuth upstreams (e.g. axon) get stranded with an expired
            // token. Non-admin callers keep their own `sub` so a personal upstream
            // grant is used; a `sub`-less caller falls back to the shared subject.
            Self::Scoped { scopes, .. } if scopes.iter().any(|scope| scope == "lab:admin") => {
                Some(SHARED_GATEWAY_OAUTH_SUBJECT)
            }
            Self::Scoped { sub: Some(s), .. } => Some(s.as_str()),
            Self::Scoped { sub: None, .. } => Some(SHARED_GATEWAY_OAUTH_SUBJECT),
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CodeModeCapabilityFilter {
    upstreams: BTreeSet<String>,
    tools: BTreeSet<String>,
}

impl CodeModeCapabilityFilter {
    #[must_use]
    pub fn new(upstreams: Vec<String>, tools: Vec<String>) -> Self {
        fn clean_set(values: Vec<String>) -> BTreeSet<String> {
            values
                .into_iter()
                .map(|value| value.trim().to_string())
                .filter(|value| !value.is_empty())
                .collect()
        }
        Self {
            upstreams: clean_set(upstreams),
            tools: clean_set(tools),
        }
    }

    #[must_use]
    pub fn allows(&self, upstream: &str, tool: &str) -> bool {
        (self.upstreams.is_empty() || self.upstreams.contains(upstream))
            && (self.tools.is_empty()
                || self.tools.contains(tool)
                || self.tools.contains(&upstream_tool_id(upstream, tool)))
    }
}
