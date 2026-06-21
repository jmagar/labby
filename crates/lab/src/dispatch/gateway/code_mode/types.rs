//! Core Code Mode value types: tool ids, catalog entries, execution responses,
//! callers, surfaces, and the capability filter.

use std::collections::{BTreeSet, VecDeque};

use serde::ser::SerializeStruct;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::dispatch::error::ToolError;
use crate::dispatch::gateway::SHARED_GATEWAY_OAUTH_SUBJECT;
use crate::dispatch::snippets::store::{SnippetInfo, SnippetInputSpec, SnippetInputType};
use crate::dispatch::upstream::types::UpstreamRuntimeOwner;

use super::artifacts::CodeModeArtifactReceipt;
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
    /// Parse a raw `<upstream>::<tool>` string into a `CodeModeToolId`.
    ///
    /// This is an inherent shim over the `FromStr` impl so call sites that
    /// already use `.parse(…)` or `CodeModeToolId::parse(…)` continue to
    /// compile without churn.
    pub fn parse(raw: &str) -> Result<Self, ToolError> {
        raw.parse()
    }
}

impl std::str::FromStr for CodeModeToolId {
    type Err = ToolError;

    fn from_str(raw: &str) -> Result<Self, Self::Err> {
        let raw = raw.trim();
        if raw.is_empty() {
            return Err(invalid_code_mode_id("Code Mode tool id must not be empty"));
        }

        if raw.starts_with("lab::") {
            return Err(lab_action_unknown_tool());
        }

        // Shared `<upstream>::<tool>` splitter — also used by `ToolExecuteSelector`.
        if let Some((upstream, tool)) = split_upstream_tool(raw) {
            return Ok(Self {
                raw: raw.to_string(),
                reference: CodeModeToolRef::UpstreamTool {
                    upstream: upstream.to_string(),
                    tool: tool.to_string(),
                },
            });
        }

        Err(invalid_code_mode_id(
            "Code Mode ids must use <upstream>::<tool>",
        ))
    }
}

/// Split a `<upstream>::<tool>` string into its two trimmed parts.
///
/// Returns `None` when the string has a wrong number of `::` separators or
/// when either part is empty after trimming. Used by both `CodeModeToolId` and
/// `ToolExecuteSelector` to avoid duplicating the splitting logic.
pub(crate) fn split_upstream_tool(raw: &str) -> Option<(&str, &str)> {
    let mut parts = raw.split("::");
    let upstream = parts.next()?.trim();
    let tool = parts.next()?.trim();
    // Ensure there is no third segment (e.g. `a::b::c` is invalid).
    if parts.next().is_some() {
        return None;
    }
    if upstream.is_empty() || tool.is_empty() {
        return None;
    }
    Some((upstream, tool))
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
    pub kind: CodeModeCatalogKind,
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
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tags: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub inputs: Vec<CodeModeSnippetInputEntry>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, PartialOrd, Ord)]
#[serde(rename_all = "snake_case")]
pub enum CodeModeCatalogKind {
    Tool,
    Snippet,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CodeModeSnippetInputEntry {
    pub name: String,
    #[serde(flatten)]
    pub spec: SnippetInputSpec,
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
        let types = super::ts_signatures::generate_tool_types(
            upstream,
            tool,
            description,
            schema.as_ref(),
            output_schema.as_ref(),
        );
        Self {
            kind: CodeModeCatalogKind::Tool,
            id: upstream_tool_id(upstream, tool),
            name: tool.to_string(),
            upstream: upstream.to_string(),
            description: description.to_string(),
            schema,
            output_schema,
            signature: types.signature,
            dts: types.dts,
            tags: Vec::new(),
            inputs: Vec::new(),
        }
    }

    #[must_use]
    pub fn snippet(info: &SnippetInfo) -> Self {
        let description = info
            .description
            .clone()
            .unwrap_or_else(|| format!("Code Mode snippet `{}`", info.name));
        let inputs = info
            .inputs
            .iter()
            .map(|(name, spec)| CodeModeSnippetInputEntry {
                name: name.clone(),
                spec: spec.clone(),
            })
            .collect::<Vec<_>>();
        Self {
            kind: CodeModeCatalogKind::Snippet,
            id: format!("snippet::{}", info.name),
            name: info.name.clone(),
            upstream: "snippet".to_string(),
            description,
            schema: Some(snippet_inputs_schema(&info.inputs)),
            output_schema: None,
            signature: format!("codemode.run({:?}, input?)", info.name),
            dts: String::new(),
            tags: info.tags.clone(),
            inputs,
        }
    }
}

fn snippet_inputs_schema(inputs: &std::collections::BTreeMap<String, SnippetInputSpec>) -> Value {
    let mut properties = serde_json::Map::new();
    let mut required = Vec::new();
    for (name, spec) in inputs {
        if spec.required {
            required.push(Value::String(name.clone()));
        }
        let mut field = serde_json::Map::new();
        if let Some(json_type) = snippet_input_json_type(spec.ty) {
            field.insert("type".to_string(), Value::String(json_type.to_string()));
        }
        if let Some(description) = &spec.description {
            field.insert(
                "description".to_string(),
                Value::String(description.clone()),
            );
        }
        if let Some(default) = &spec.default {
            field.insert("default".to_string(), default.clone());
        }
        properties.insert(name.clone(), Value::Object(field));
    }
    json!({
        "type": "object",
        "properties": properties,
        "required": required,
        "additionalProperties": false,
    })
}

fn snippet_input_json_type(ty: SnippetInputType) -> Option<&'static str> {
    match ty {
        SnippetInputType::String => Some("string"),
        SnippetInputType::Integer => Some("integer"),
        SnippetInputType::Number => Some("number"),
        SnippetInputType::Boolean => Some("boolean"),
        SnippetInputType::Object => Some("object"),
        SnippetInputType::Array => Some("array"),
        SnippetInputType::Json => None,
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct CodeModeDiscoveryEntry {
    pub(crate) kind: CodeModeCatalogKind,
    pub(crate) id: String,
    pub(crate) path: String,
    pub(crate) upstream: String,
    pub(crate) name: String,
    pub(crate) helper: String,
    pub(crate) description: String,
    pub(crate) signature: String,
    pub(crate) tags: Vec<String>,
    pub(crate) inputs: Vec<CodeModeSnippetInputEntry>,
    /// Input JSON Schema for the underlying tool. Carried from the catalog so
    /// `codemode.describe` can surface field names/types/required-ness, but
    /// deliberately `skip`-ped from the serialized `__codemodeDiscovery` search
    /// index: embedding full schemas there would balloon the per-execute
    /// preamble and slow startup. `generate_discovery_js` emits the type body
    /// (not this raw schema) only through the `describe` lookup map.
    #[serde(skip)]
    pub(crate) schema: Option<Value>,
    /// Generated `.d.ts` declaration block for the tool (empty for snippets).
    /// Like `schema`, this is `skip`-ped from the search index and surfaced
    /// only via `codemode.describe`.
    #[serde(skip)]
    pub(crate) dts: String,
}

impl CodeModeDiscoveryEntry {
    #[must_use]
    pub(crate) fn from_catalog(entry: &CodeModeCatalogEntry) -> Self {
        let (path, helper) = match entry.kind {
            CodeModeCatalogKind::Tool => {
                let upstream = super::preamble::upstream_name_to_namespace(&entry.upstream);
                let name = super::preamble::tool_name_to_snake(&entry.name);
                (
                    format!("{upstream}.{name}"),
                    format!("codemode.{upstream}.{name}"),
                )
            }
            CodeModeCatalogKind::Snippet => (
                format!("snippet.{}", entry.name),
                format!("codemode.run({:?}, input)", entry.name),
            ),
        };
        Self {
            kind: entry.kind,
            id: entry.id.clone(),
            path,
            upstream: entry.upstream.clone(),
            name: entry.name.clone(),
            helper,
            description: entry.description.clone(),
            signature: entry.signature.clone(),
            tags: entry.tags.clone(),
            inputs: entry.inputs.clone(),
            schema: entry.schema.clone(),
            dts: entry.dts.clone(),
        }
    }
}

/// A captured upstream MCP Apps (mcp-ui) widget link.
///
/// Recorded at the broker boundary when an upstream tool result carries
/// `_meta.ui.resourceUri`, before `unwrap_code_mode_upstream_result` discards
/// the envelope. `ui_meta` holds the upstream's `_meta.ui` object verbatim
/// (including `resourceUri`) so the final `execute` `CallToolResult` can mirror
/// the upstream identically.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct UiLink {
    pub ui_meta: Value,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct CodeModeExecutionResponse {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub execution_id: Option<String>,
    /// The final return value of the async function. None when the function
    /// returns undefined or throws (the throw case surfaces via ToolError).
    /// Explicit JavaScript `null` is represented as `Some(Value::Null)` and
    /// serializes as `"result": null`; undefined omits the field.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<Value>,
    /// Captured mcp-ui widget link (last-wins across the run). The MCP boundary
    /// attaches this as `_meta.ui` on the returned `CallToolResult` so the host
    /// renders the native widget. `None` when no widget-bearing call ran.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ui: Option<UiLink>,
    pub calls: Vec<CodeModeExecutedCall>,
    /// Captured console.log/warn/error lines from the runner. Sourced from the
    /// javy runner subprocess (drained from its stderr); the current javy path
    /// returns no protocol-carried logs, so this is empty until console capture
    /// is wired through.
    pub logs: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub artifacts: Vec<CodeModeArtifactReceipt>,
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
    Execute,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct CodeModeHistoryEntry {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub execution_id: Option<String>,
    pub seq: u64,
    pub route_scope: String,
    pub kind: CodeModeHistoryKind,
    pub ok: bool,
    pub elapsed_ms: u128,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub input_tokens: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub output_tokens: Option<usize>,
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
    /// Accumulated serialized byte estimate for all entries in `entries`.
    ///
    /// Maintained as a running total to avoid re-serializing the entire deque
    /// on every push or eviction. Updated when entries are added or removed.
    /// The estimate uses the serialized JSON size of each individual entry; the
    /// VecDeque framing bytes (brackets, commas) are a constant ~2 bytes and are
    /// ignored — acceptable given the ~1 KB min entry size and 256 KB default cap.
    running_bytes: usize,
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
            running_bytes: 0,
            max_entries: max_entries.max(1),
            max_bytes: max_bytes.max(1024),
            next_seq: 1,
        }
    }

    pub fn push(&mut self, mut entry: CodeModeHistoryEntry) {
        entry.seq = self.next_seq;
        self.next_seq = self.next_seq.saturating_add(1);
        let entry_bytes = entry_serialized_size(&entry);
        self.entries.push_back(entry);
        self.running_bytes = self.running_bytes.saturating_add(entry_bytes);
        self.trim();
    }

    #[must_use]
    pub fn snapshot(&self) -> Vec<CodeModeHistoryEntry> {
        self.entries.iter().cloned().collect()
    }

    #[must_use]
    pub fn snapshot_for_route_scope(&self, route_scope: Option<&str>) -> Vec<CodeModeHistoryEntry> {
        match route_scope {
            None => self.snapshot(),
            Some(route_scope) => self
                .entries
                .iter()
                .filter(|entry| entry.route_scope == route_scope)
                .cloned()
                .collect(),
        }
    }

    fn trim(&mut self) {
        while self.entries.len() > self.max_entries {
            if let Some(evicted) = self.entries.pop_front() {
                self.running_bytes = self
                    .running_bytes
                    .saturating_sub(entry_serialized_size(&evicted));
            }
        }
        while self.running_bytes > self.max_bytes && self.entries.len() > 1 {
            if let Some(evicted) = self.entries.pop_front() {
                self.running_bytes = self
                    .running_bytes
                    .saturating_sub(entry_serialized_size(&evicted));
            }
        }
        if self.running_bytes > self.max_bytes {
            if let Some(entry) = self.entries.pop_back() {
                let old_bytes = entry_serialized_size(&entry);
                let sentinel = Self::oversized_entry_sentinel(entry.seq, entry.kind);
                let sentinel_bytes = entry_serialized_size(&sentinel);
                self.running_bytes = self
                    .running_bytes
                    .saturating_sub(old_bytes)
                    .saturating_add(sentinel_bytes);
                self.entries.push_back(sentinel);
            }
        }
    }

    fn oversized_entry_sentinel(seq: u64, kind: CodeModeHistoryKind) -> CodeModeHistoryEntry {
        CodeModeHistoryEntry {
            execution_id: None,
            seq,
            route_scope: "root".to_string(),
            kind,
            ok: false,
            elapsed_ms: 0,
            input_tokens: None,
            output_tokens: None,
            error_kind: Some("history_entry_too_large".to_string()),
            calls: Vec::new(),
            match_count: None,
        }
    }
}

#[derive(Debug, Clone)]
pub struct CodeModeExecutionSource {
    pub execution_id: String,
    pub created_at_ms: i64,
    pub actor_key: Option<String>,
    pub is_admin: bool,
    pub route_scope: String,
    pub surface: CodeModeSurface,
    pub capability_filter_fingerprint: String,
    pub code: String,
}

#[derive(Debug, Clone)]
pub struct CodeModeSourceLookup {
    pub actor_key: Option<String>,
    pub is_admin: bool,
    pub route_scope: String,
    pub capability_filter_fingerprint: String,
}

#[derive(Debug, Clone)]
pub struct CodeModeSourceStore {
    entries: VecDeque<CodeModeExecutionSource>,
    running_bytes: usize,
    max_entries: usize,
    max_bytes: usize,
}

impl Default for CodeModeSourceStore {
    fn default() -> Self {
        Self::new(50, 512 * 1024)
    }
}

impl CodeModeSourceStore {
    #[must_use]
    pub fn new(max_entries: usize, max_bytes: usize) -> Self {
        Self {
            entries: VecDeque::new(),
            running_bytes: 0,
            max_entries: max_entries.max(1),
            max_bytes: max_bytes.max(1024),
        }
    }

    pub fn push(&mut self, source: CodeModeExecutionSource) {
        let bytes = source.code.len();
        if bytes > self.max_bytes {
            return;
        }
        self.running_bytes = self.running_bytes.saturating_add(bytes);
        self.entries.push_back(source);
        while self.entries.len() > self.max_entries || self.running_bytes > self.max_bytes {
            if let Some(evicted) = self.entries.pop_front() {
                self.running_bytes = self.running_bytes.saturating_sub(evicted.code.len());
            } else {
                break;
            }
        }
    }

    #[must_use]
    pub fn resolve(
        &self,
        execution_id: &str,
        lookup: &CodeModeSourceLookup,
    ) -> Result<CodeModeExecutionSource, ToolError> {
        let Some(source) = self
            .entries
            .iter()
            .find(|entry| entry.execution_id == execution_id)
            .cloned()
        else {
            return Err(ToolError::Sdk {
                sdk_kind: "unknown_execution".to_string(),
                message: "Code Mode promotion source is ephemeral and may have expired, been evicted, lived in another gateway process, or disappeared after restart".to_string(),
            });
        };
        if !lookup.is_admin {
            return Err(ToolError::Forbidden {
                message: "promoting Code Mode executions requires lab:admin".to_string(),
                required_scopes: vec!["lab:admin".to_string()],
            });
        }
        if source.route_scope != lookup.route_scope
            || !source_capability_within_lookup(
                &source.capability_filter_fingerprint,
                &lookup.capability_filter_fingerprint,
            )
        {
            return Err(ToolError::Forbidden {
                message: "Code Mode promotion source is outside this route or capability scope"
                    .to_string(),
                required_scopes: vec!["lab:admin".to_string()],
            });
        }
        if source.actor_key != lookup.actor_key {
            return Err(ToolError::Forbidden {
                message: "Code Mode promotion source belongs to a different actor".to_string(),
                required_scopes: vec!["lab:admin".to_string()],
            });
        }
        Ok(source)
    }
}

fn source_capability_within_lookup(source: &str, lookup: &str) -> bool {
    if source == lookup {
        return true;
    }

    let Some(source_upstreams) = capability_fingerprint_upstreams(source) else {
        return false;
    };
    let Some(lookup_upstreams) = capability_fingerprint_upstreams(lookup) else {
        return false;
    };

    match (source_upstreams, lookup_upstreams) {
        (_, None) => true,
        (None, Some(_)) => false,
        (Some(source), Some(lookup)) => source.is_subset(&lookup),
    }
}

fn capability_fingerprint_upstreams(fingerprint: &str) -> Option<Option<BTreeSet<String>>> {
    if let Ok(value) = serde_json::from_str::<Value>(fingerprint) {
        let upstreams = value.get("upstreams")?;
        if upstreams.is_null() {
            return Some(None);
        }
        let set = upstreams
            .as_array()?
            .iter()
            .map(Value::as_str)
            .collect::<Option<Vec<_>>>()?
            .into_iter()
            .map(ToOwned::to_owned)
            .collect::<BTreeSet<_>>();
        return Some(Some(set));
    }

    let upstreams = fingerprint
        .split(';')
        .find_map(|part| part.strip_prefix("upstreams="))?;
    if upstreams == "*" {
        return Some(None);
    }
    let set = upstreams
        .split(',')
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
        .collect::<BTreeSet<_>>();
    Some(Some(set))
}

/// Serialized byte size of a single history entry.
///
/// Used to maintain the `running_bytes` counter without re-serializing the
/// entire deque on every mutation. Falls back to `usize::MAX` on a (very
/// unlikely) serialization error so the history is conservatively treated as
/// over-budget rather than silently growing without bound.
fn entry_serialized_size(entry: &CodeModeHistoryEntry) -> usize {
    serde_json::to_vec(entry)
        .map(|bytes| bytes.len())
        .unwrap_or(usize::MAX / 2)
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
    pub fn can_use_snippets(&self) -> bool {
        match self {
            Self::TrustedLocal => true,
            Self::Scoped { scopes, .. } => scopes.iter().any(|scope| scope == "lab:admin"),
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
    upstreams: Option<BTreeSet<String>>,
    tools: BTreeSet<String>,
}

impl CodeModeCapabilityFilter {
    #[must_use]
    pub fn new(upstreams: Vec<String>, tools: Vec<String>) -> Self {
        Self::new_inner(None, upstreams, tools)
    }

    #[must_use]
    pub fn scoped_upstreams(upstreams: Vec<String>, tools: Vec<String>) -> Self {
        Self::new_inner(Some(BTreeSet::new()), upstreams, tools)
    }

    fn new_inner(
        scoped_default: Option<BTreeSet<String>>,
        upstreams: Vec<String>,
        tools: Vec<String>,
    ) -> Self {
        fn clean_set(values: Vec<String>) -> BTreeSet<String> {
            values
                .into_iter()
                .map(|value| value.trim().to_string())
                .filter(|value| !value.is_empty())
                .collect()
        }
        let upstreams = clean_set(upstreams);
        Self {
            upstreams: if upstreams.is_empty() {
                scoped_default
            } else {
                Some(upstreams)
            },
            tools: clean_set(tools),
        }
    }

    #[must_use]
    pub fn allows(&self, upstream: &str, tool: &str) -> bool {
        (self
            .upstreams
            .as_ref()
            .is_none_or(|upstreams| upstreams.contains(upstream)))
            && (self.tools.is_empty()
                || self.tools.contains(tool)
                || self.tools.contains(&upstream_tool_id(upstream, tool)))
    }

    #[must_use]
    pub fn is_scoped_to_upstreams(&self) -> bool {
        self.upstreams.is_some()
    }

    #[must_use]
    pub fn allowed_upstreams(&self) -> Option<&BTreeSet<String>> {
        self.upstreams.as_ref()
    }

    #[must_use]
    pub fn fingerprint(&self) -> String {
        serde_json::json!({
            "upstreams": self.upstreams.as_ref().map(|set| set.iter().cloned().collect::<Vec<_>>()),
            "tools": self.tools.iter().cloned().collect::<Vec<_>>(),
        })
        .to_string()
    }
}
