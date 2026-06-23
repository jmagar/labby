//! Core Code Mode value types: tool ids, tool descriptors, execution responses,
//! callers, surfaces, and the tool scope.
//!
//! Vocabulary is host-source-neutral. A tool is an opaque `id` of the form
//! `<namespace>::<tool>`; the kernel never learns what backs the namespace.

use std::collections::{BTreeSet, VecDeque};

use serde::ser::SerializeStruct;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::error::ToolError;
use crate::snippet::store::{SnippetInfo, SnippetInputSpec, SnippetInputType};

use super::artifacts::CodeModeArtifactReceipt;
use super::util::{invalid_code_mode_id, lab_action_unknown_tool};

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct CodeModeToolId {
    pub(crate) raw: String,
    pub(crate) reference: CodeModeToolRef,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum CodeModeToolRef {
    Tool { namespace: String, tool: String },
}

impl CodeModeToolId {
    /// Parse a raw `<namespace>::<tool>` string into a `CodeModeToolId`.
    ///
    /// This is an inherent shim over the `FromStr` impl so call sites that
    /// already use `.parse(…)` or `CodeModeToolId::parse(…)` continue to
    /// compile without churn.
    pub(crate) fn parse(raw: &str) -> Result<Self, ToolError> {
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

        // Shared `<namespace>::<tool>` splitter — also used by `ToolExecuteSelector`.
        if let Some((namespace, tool)) = split_namespaced_id(raw) {
            return Ok(Self {
                raw: raw.to_string(),
                reference: CodeModeToolRef::Tool {
                    namespace: namespace.to_string(),
                    tool: tool.to_string(),
                },
            });
        }

        Err(invalid_code_mode_id(
            "Code Mode ids must use <namespace>::<tool>",
        ))
    }
}

/// Split a `<namespace>::<tool>` string into its two trimmed parts.
///
/// Returns `None` when the string has a wrong number of `::` separators or
/// when either part is empty after trimming. Used by both `CodeModeToolId` and
/// `ToolExecuteSelector` to avoid duplicating the splitting logic.
pub fn split_namespaced_id(raw: &str) -> Option<(&str, &str)> {
    let mut parts = raw.split("::");
    let namespace = parts.next()?.trim();
    let tool = parts.next()?.trim();
    // Ensure there is no third segment (e.g. `a::b::c` is invalid).
    if parts.next().is_some() {
        return None;
    }
    if namespace.is_empty() || tool.is_empty() {
        return None;
    }
    Some((namespace, tool))
}

#[must_use]
pub fn namespaced_tool_id(namespace: &str, tool: &str) -> String {
    format!("{namespace}::{tool}")
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ToolDescriptor {
    pub kind: CodeModeCatalogKind,
    pub id: String,
    pub name: String,
    pub namespace: String,
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

impl ToolDescriptor {
    /// Build a tool descriptor for a host-provided tool (`<namespace>::<tool>`).
    ///
    /// The host passes already-sanitized JSON Schemas; this constructor only
    /// generates the TypeScript signature / `.d.ts` for the in-sandbox catalog.
    #[must_use]
    pub fn tool(
        namespace: &str,
        tool: &str,
        description: &str,
        schema: Option<Value>,
        output_schema: Option<Value>,
    ) -> Self {
        let types = super::ts_signatures::generate_tool_types(
            namespace,
            tool,
            description,
            schema.as_ref(),
            output_schema.as_ref(),
        );
        Self {
            kind: CodeModeCatalogKind::Tool,
            id: namespaced_tool_id(namespace, tool),
            name: tool.to_string(),
            namespace: namespace.to_string(),
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
            namespace: "snippet".to_string(),
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
    pub(crate) namespace: String,
    pub(crate) name: String,
    pub(crate) helper: String,
    pub(crate) description: String,
    pub(crate) signature: String,
    pub(crate) tags: Vec<String>,
    pub(crate) inputs: Vec<CodeModeSnippetInputEntry>,
}

impl CodeModeDiscoveryEntry {
    #[must_use]
    pub(crate) fn from_catalog(entry: &ToolDescriptor) -> Self {
        let (path, helper) = match entry.kind {
            CodeModeCatalogKind::Tool => {
                let namespace = super::preamble::namespace_segment(&entry.namespace);
                let name = super::preamble::tool_name_to_snake(&entry.name);
                (
                    format!("{namespace}.{name}"),
                    format!("codemode.{namespace}.{name}"),
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
            namespace: entry.namespace.clone(),
            name: entry.name.clone(),
            helper,
            description: entry.description.clone(),
            signature: entry.signature.clone(),
            tags: entry.tags.clone(),
            inputs: entry.inputs.clone(),
        }
    }
}

/// A captured MCP Apps (mcp-ui) widget link.
///
/// Recorded by the host at the tool-call boundary when a tool result carries
/// `_meta.ui.resourceUri`, before the result envelope is discarded. `ui_meta`
/// holds the `_meta.ui` object verbatim (including `resourceUri`) so the final
/// `execute` response can mirror the widget identically.
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
        let (namespace, tool) = split_code_mode_call_id(&self.id);
        let mut state = serializer.serialize_struct("CodeModeExecutedCall", 7)?;
        state.serialize_field("id", &self.id)?;
        state.serialize_field("namespace", namespace)?;
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
pub(crate) fn split_code_mode_call_id(id: &str) -> (&str, &str) {
    id.split_once("::")
        .map_or(("", id), |(namespace, tool)| (namespace, tool))
}

#[derive(Debug, Clone)]
pub struct CodeModeExecutionError {
    error: ToolError,
    calls: Vec<CodeModeExecutedCall>,
}

impl CodeModeExecutionError {
    #[must_use]
    pub fn with_trace(error: ToolError, calls: Vec<CodeModeExecutedCall>) -> Self {
        Self { error, calls }
    }

    #[must_use]
    pub fn kind(&self) -> &str {
        self.error.kind()
    }

    #[must_use]
    pub fn calls(&self) -> &[CodeModeExecutedCall] {
        &self.calls
    }

    #[must_use]
    pub fn into_tool_error(self) -> ToolError {
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
                message: "Code Mode promotion source is ephemeral and may have expired, been evicted, lived in another host process, or disappeared after restart".to_string(),
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

    let Some(source_namespaces) = capability_fingerprint_namespaces(source) else {
        return false;
    };
    let Some(lookup_namespaces) = capability_fingerprint_namespaces(lookup) else {
        return false;
    };

    match (source_namespaces, lookup_namespaces) {
        (_, None) => true,
        (None, Some(_)) => false,
        (Some(source), Some(lookup)) => source.is_subset(&lookup),
    }
}

fn capability_fingerprint_namespaces(fingerprint: &str) -> Option<Option<BTreeSet<String>>> {
    if let Ok(value) = serde_json::from_str::<Value>(fingerprint) {
        let namespaces = value.get("namespaces")?;
        if namespaces.is_null() {
            return Some(None);
        }
        let set = namespaces
            .as_array()?
            .iter()
            .map(Value::as_str)
            .collect::<Option<Vec<_>>>()?
            .into_iter()
            .map(ToOwned::to_owned)
            .collect::<BTreeSet<_>>();
        return Some(Some(set));
    }

    let namespaces = fingerprint
        .split(';')
        .find_map(|part| part.strip_prefix("namespaces="))?;
    if namespaces == "*" {
        return Some(None);
    }
    let set = namespaces
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
        /// JWT `sub` claim for the caller, when available. The host decides how
        /// to map this onto its own credential/identity model when resolving
        /// and calling tools; the kernel itself never interprets it.
        sub: Option<String>,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CodeModeSurface {
    Mcp,
    Cli,
}

impl CodeModeSurface {
    /// Stable lowercase surface tag (`"mcp"` / `"cli"`) used by hosts when
    /// building their own runtime-owner / logging context.
    #[must_use]
    pub fn tag(self) -> &'static str {
        match self {
            CodeModeSurface::Mcp => "mcp",
            CodeModeSurface::Cli => "cli",
        }
    }
}

/// Whether a destructive tool call is permitted for this caller.
/// Code Mode execution is already scope-gated; do not add a second host-side
/// confirmation gate based on tool catalog metadata. Hosts call this when
/// applying destructive-tool policy.
#[must_use]
pub fn destructive_permitted(surface: CodeModeSurface, caller: &CodeModeCaller) -> bool {
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

    /// Whether this caller carries the `lab:admin` scope (trusted-local always
    /// counts as admin). Hosts use this when mapping the caller onto their own
    /// credential model.
    #[must_use]
    pub fn is_admin(&self) -> bool {
        match self {
            Self::TrustedLocal => true,
            Self::Scoped { scopes, .. } => scopes.iter().any(|scope| scope == "lab:admin"),
        }
    }

    /// The caller's `sub` identity, when available. `None` for trusted-local.
    #[must_use]
    pub fn subject(&self) -> Option<&str> {
        match self {
            Self::TrustedLocal => None,
            Self::Scoped { sub, .. } => sub.as_deref(),
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ToolScope {
    namespaces: Option<BTreeSet<String>>,
    tools: BTreeSet<String>,
}

impl ToolScope {
    #[must_use]
    pub fn new(namespaces: Vec<String>, tools: Vec<String>) -> Self {
        Self::new_inner(None, namespaces, tools)
    }

    #[must_use]
    pub fn scoped_namespaces(namespaces: Vec<String>, tools: Vec<String>) -> Self {
        Self::new_inner(Some(BTreeSet::new()), namespaces, tools)
    }

    fn new_inner(
        scoped_default: Option<BTreeSet<String>>,
        namespaces: Vec<String>,
        tools: Vec<String>,
    ) -> Self {
        fn clean_set(values: Vec<String>) -> BTreeSet<String> {
            values
                .into_iter()
                .map(|value| value.trim().to_string())
                .filter(|value| !value.is_empty())
                .collect()
        }
        let namespaces = clean_set(namespaces);
        Self {
            namespaces: if namespaces.is_empty() {
                scoped_default
            } else {
                Some(namespaces)
            },
            tools: clean_set(tools),
        }
    }

    #[must_use]
    pub fn allows(&self, namespace: &str, tool: &str) -> bool {
        (self
            .namespaces
            .as_ref()
            .is_none_or(|namespaces| namespaces.contains(namespace)))
            && (self.tools.is_empty()
                || self.tools.contains(tool)
                || self.tools.contains(&namespaced_tool_id(namespace, tool)))
    }

    #[must_use]
    pub fn is_scoped(&self) -> bool {
        self.namespaces.is_some()
    }

    #[must_use]
    pub fn allowed_namespaces(&self) -> Option<&BTreeSet<String>> {
        self.namespaces.as_ref()
    }

    #[must_use]
    pub fn fingerprint(&self) -> String {
        serde_json::json!({
            "namespaces": self.namespaces.as_ref().map(|set| set.iter().cloned().collect::<Vec<_>>()),
            "tools": self.tools.iter().cloned().collect::<Vec<_>>(),
        })
        .to_string()
    }
}
