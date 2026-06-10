//! MCP adapter for the `fs` workspace filesystem browser service.
//!
//! The catalog and dispatch logic live in `crates/lab/src/dispatch/fs/`.
//!
//! # Why this file filters `fs.preview` out
//!
//! `crate::dispatch::fs::catalog::ACTIONS` contains both `fs.list` and `fs.preview`
//! — the latter is intentionally **not** exposed over MCP. An LLM agent
//! constructs the request body, so no param-based confirmation gate is
//! safe against prompt injection: an attacker who gets a prompt-injection
//! payload into the session can ask the agent to `fs.preview(".env")` and
//! exfiltrate the file in one round-trip. The deny-list is defense-in-depth
//! but not a sound boundary; the only safe policy is "HTTP-only".
//!
//! This mirrors the `api/CLAUDE.md` decision to remove the `X-Lab-Confirm`
//! header confirmation path.
//!
//! # Two-layer filter
//!
//! 1. **Discovery filter** — `help` and `schema` are intercepted here and
//!    served from `MCP_ACTIONS` (no `fs.preview`). This ensures a
//!    prompt-injected agent enumerating tools via `help` never sees the
//!    preview action at all.
//! 2. **Execution filter** — `fs.preview` requests are rejected here with
//!    a stable `http_only` error kind before reaching the shared dispatch
//!    layer. Defense-in-depth: if a caller somehow guesses the action
//!    name, the dispatcher refuses.

use lab_apis::core::action::ActionSpec;
use serde_json::Value;

use crate::dispatch::error::ToolError;
use crate::dispatch::helpers::{action_schema, help_payload, require_str};

/// MCP-exposed slice of the fs action catalog. Filters out `fs.preview`.
pub static ACTIONS: &[ActionSpec] = MCP_ACTIONS;

/// Canonical actions that are intentionally filtered out of the MCP surface
/// (HTTP-only) along with the HTTP route the rejection envelope should point
/// callers at. Shared between the dispatch rejection arm and the coverage
/// test so adding a new HTTP-only action touches exactly one place. Storing
/// the route explicitly avoids a fragile fs.* → /v1/fs/* string derivation
/// when a future HTTP-only action does not follow that mapping.
const HTTP_ONLY_ACTIONS: &[(&str, &str)] = &[("fs.preview", "/v1/fs/preview")];

/// Compile-time filtered view of [`crate::dispatch::fs::catalog::ACTIONS`].
///
/// `fs.preview` must not be discoverable on MCP (see module-level doc on the
/// prompt-injection exfil risk). Since `&'static [ActionSpec]` cannot be
/// safely runtime-sliced into another `&'static` slice without leaking, we
/// redeclare the MCP-visible subset here. The deep-equality test below locks
/// the redeclaration to the canonical catalog so descriptions/params/returns
/// cannot drift unnoticed.
static MCP_ACTIONS: &[ActionSpec] = &[
    // Mirror of `dispatch::fs::catalog::ACTIONS[0]`. Filtered: fs.preview
    // omitted — see module-level doc for rationale.
    ActionSpec {
        name: "fs.list",
        description: "List immediate entries of a directory inside the configured workspace root",
        destructive: false,
        requires_admin: false,
        params: &[lab_apis::core::action::ParamSpec {
            name: "path",
            ty: "string",
            required: false,
            description: "Workspace-relative path to list; empty or omitted means the workspace root",
        }],
        returns: "{entries: [{name, path, kind, size, modified, accessible}], truncated: bool}",
    },
];

/// Build the `http_only` rejection envelope returned when an MCP caller
/// invokes an action that is only available over the HTTP surface.
fn http_only_error(action: &str, http_path: &str) -> ToolError {
    ToolError::Sdk {
        sdk_kind: "http_only".to_string(),
        message: format!("{action} is not available on the MCP surface; use GET {http_path}"),
    }
}

/// MCP dispatch entry point.
///
/// `help` and `schema` are intercepted here against `MCP_ACTIONS` so the
/// filtered catalog is the only thing MCP clients can discover. Every
/// other action except the explicitly rejected `fs.preview` falls through
/// to the shared dispatch layer.
pub async fn dispatch(action: &str, params: Value) -> Result<Value, ToolError> {
    match action {
        "help" => Ok(help_payload("fs", MCP_ACTIONS)),
        "schema" => {
            let a = require_str(&params, "action")?;
            action_schema(MCP_ACTIONS, a)
        }
        other => match HTTP_ONLY_ACTIONS.iter().find(|(name, _)| *name == other) {
            Some((_, http_path)) => Err(http_only_error(other, http_path)),
            None => crate::dispatch::fs::dispatch(other, params).await,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mcp_actions_exclude_fs_preview() {
        let names: Vec<&str> = MCP_ACTIONS.iter().map(|a| a.name).collect();
        assert!(!names.contains(&"fs.preview"), "{names:?}");
        assert!(names.contains(&"fs.list"));
    }

    #[test]
    fn mcp_actions_cover_canonical_except_http_only() {
        let mcp_names: Vec<&str> = MCP_ACTIONS.iter().map(|a| a.name).collect();
        for canonical in crate::dispatch::fs::catalog::ACTIONS {
            if HTTP_ONLY_ACTIONS
                .iter()
                .any(|(name, _)| *name == canonical.name)
            {
                assert!(
                    !mcp_names.contains(&canonical.name),
                    "`{}` is in HTTP_ONLY_ACTIONS but still present in MCP_ACTIONS",
                    canonical.name
                );
                continue;
            }
            assert!(
                mcp_names.contains(&canonical.name),
                "canonical action `{}` missing from MCP_ACTIONS — add it to MCP_ACTIONS or HTTP_ONLY_ACTIONS",
                canonical.name
            );
        }
    }

    /// Deep field-by-field equality between every MCP_ACTIONS entry and its
    /// canonical counterpart in `dispatch::fs::ACTIONS`. Locks the invariant
    /// that the redeclaration here is a pure subset — descriptions, params,
    /// returns, and destructive metadata must not drift. If this test fails
    /// after a catalog edit, update MCP_ACTIONS to mirror the canonical entry
    /// — do NOT weaken this assertion.
    #[test]
    fn mcp_actions_deep_match_canonical() {
        for mcp in MCP_ACTIONS {
            let canonical = crate::dispatch::fs::catalog::ACTIONS
                .iter()
                .find(|c| c.name == mcp.name)
                .unwrap_or_else(|| {
                    panic!("MCP action `{}` missing from canonical catalog", mcp.name)
                });
            assert_eq!(
                mcp.description, canonical.description,
                "description drift on `{}`",
                mcp.name
            );
            assert_eq!(
                mcp.destructive, canonical.destructive,
                "destructive drift on `{}`",
                mcp.name
            );
            assert_eq!(
                mcp.returns, canonical.returns,
                "returns drift on `{}`",
                mcp.name
            );
            assert_eq!(
                mcp.params.len(),
                canonical.params.len(),
                "params length drift on `{}`",
                mcp.name
            );
            for (m, c) in mcp.params.iter().zip(canonical.params.iter()) {
                assert_eq!(m.name, c.name, "param name drift on `{}`", mcp.name);
                assert_eq!(m.ty, c.ty, "param ty drift on `{}::{}`", mcp.name, m.name);
                assert_eq!(
                    m.required, c.required,
                    "param required drift on `{}::{}`",
                    mcp.name, m.name
                );
                assert_eq!(
                    m.description, c.description,
                    "param description drift on `{}::{}`",
                    mcp.name, m.name
                );
            }
        }
    }

    #[tokio::test]
    async fn dispatch_rejects_fs_preview_with_http_only_kind() {
        let err = dispatch("fs.preview", serde_json::json!({"path": "foo"}))
            .await
            .expect_err("err");
        match err {
            ToolError::Sdk { sdk_kind, .. } => assert_eq!(sdk_kind, "http_only"),
            other => panic!("expected Sdk http_only; got {other:?}"),
        }
    }

    #[tokio::test]
    async fn help_does_not_list_fs_preview() {
        let value = dispatch("help", Value::Null).await.expect("ok");
        let names: Vec<String> = value["actions"]
            .as_array()
            .expect("actions array")
            .iter()
            .map(|a| a["name"].as_str().unwrap().to_string())
            .collect();
        assert!(!names.contains(&"fs.preview".to_string()), "{names:?}");
        assert!(names.contains(&"fs.list".to_string()));
    }

    #[tokio::test]
    async fn schema_refuses_fs_preview() {
        let err = dispatch("schema", serde_json::json!({"action": "fs.preview"}))
            .await
            .expect_err("err");
        match err {
            ToolError::UnknownAction { .. } => {}
            other => panic!("expected UnknownAction; got {other:?}"),
        }
    }

    #[tokio::test]
    async fn schema_returns_fs_list_schema() {
        let value = dispatch("schema", serde_json::json!({"action": "fs.list"}))
            .await
            .expect("ok");
        assert_eq!(value["action"].as_str(), Some("fs.list"));
    }
}
