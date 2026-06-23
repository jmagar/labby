//! Action catalog for the workspace filesystem browser (`fs`) service.
//!
//! Single source of truth — the MCP adapter, HTTP adapter, and `lab.help`
//! catalog each read this const. The MCP adapter does NOT expose
//! `fs.preview` as a tool: see `mcp/services/fs.rs` for the filtered list
//! and the security rationale (an LLM agent with prompt-injection access
//! plus a body-controlled preview call can exfiltrate any readable file
//! in one round-trip).

use labby_apis::core::action::{ActionSpec, ParamSpec};

/// Full action catalog for the `fs` service. Includes both `fs.list`
/// (MCP + HTTP) and `fs.preview` (HTTP-only). Built-in actions `help` and
/// `schema` are handled inline in `dispatch.rs` and are not listed here.
pub const ACTIONS: &[ActionSpec] = &[
    ActionSpec {
        name: "fs.list",
        description: "List immediate entries of a directory inside the configured workspace root",
        destructive: false,
        requires_admin: false,
        params: &[ParamSpec {
            name: "path",
            ty: "string",
            required: false,
            description: "Workspace-relative path to list; empty or omitted means the workspace root",
        }],
        returns: "{entries: [{name, path, kind, size, modified, accessible}], truncated: bool}",
    },
    ActionSpec {
        name: "fs.preview",
        description: "Stream a capped byte window from a workspace file (HTTP-only, admin-session gated)",
        destructive: false,
        requires_admin: false,
        params: &[
            ParamSpec {
                name: "path",
                ty: "string",
                required: true,
                description: "Workspace-relative path of the file to preview",
            },
            ParamSpec {
                name: "max_bytes",
                ty: "integer",
                required: false,
                description: "Upper bound on bytes returned; server cap of 2 MiB always wins",
            },
        ],
        returns: "binary (streamed); mime from safe-MIME whitelist or application/octet-stream",
    },
];
