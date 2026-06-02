//! Core Code Mode value types: tool ids, catalog entries, execution responses,
//! callers, surfaces, and the capability filter.

use std::collections::BTreeSet;

use serde::Serialize;
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

        if let Some(rest) = raw.strip_prefix("upstream::") {
            let (upstream, tool) = rest.split_once("::").ok_or_else(|| {
                invalid_code_mode_id("upstream Code Mode ids must use upstream::<upstream>::<tool>")
            })?;
            if upstream.trim().is_empty() || tool.trim().is_empty() {
                return Err(invalid_code_mode_id(
                    "upstream Code Mode ids must include upstream and tool",
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
            "Code Mode ids must start with upstream::",
        ))
    }
}

#[must_use]
pub fn upstream_tool_id(upstream: &str, tool: &str) -> String {
    format!("upstream::{upstream}::{tool}")
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
    /// returns undefined, null, or throws (the throw case surfaces via ToolError).
    pub result: Option<Value>,
    pub calls: Vec<CodeModeExecutedCall>,
    /// Captured console.log/warn/error lines from the sandbox runner.
    /// Populated by the Boa CapturingLogger (non-WASM) or stderr (Javy/WASM).
    pub logs: Vec<String>,
}

/// Lightweight metadata for one host-brokered tool call. Cloudflare parity:
/// the per-call result payload is NOT carried here — only the model needs the
/// final `result`. Recording full per-call results bloated context and risked
/// leaking secrets through the truncation preview.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct CodeModeExecutedCall {
    pub id: String,
    pub ok: bool,
    pub elapsed_ms: u128,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error_kind: Option<String>,
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
    Mcp { allow_destructive_actions: bool },
    Cli,
}

impl CodeModeSurface {
    /// Whether destructive upstream tools are permitted on this surface.
    ///
    /// CLI is operator-driven and always permits destructive actions.
    /// MCP gates on the `allow_destructive_actions` field set at session time.
    #[must_use]
    pub fn allow_destructive_actions(self) -> bool {
        match self {
            Self::Mcp {
                allow_destructive_actions,
            } => allow_destructive_actions,
            Self::Cli => true,
        }
    }
}

/// Whether a destructive upstream tool call is explicitly permitted for this
/// `surface`.
///
/// Execute-capable scopes (`lab` / `lab:admin`) authorize running Code Mode, but
/// they do not confirm destructive upstream effects. MCP callers must pass
/// `confirm:true`; CLI is operator-driven and always permits destructive tools.
#[must_use]
pub(in crate::dispatch::gateway::code_mode) fn destructive_permitted(
    surface: CodeModeSurface,
    caller: &CodeModeCaller,
) -> bool {
    let _ = caller;
    surface.allow_destructive_actions()
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
            CodeModeSurface::Mcp { .. } => "mcp",
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
