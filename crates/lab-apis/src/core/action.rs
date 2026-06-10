//! Action discovery metadata.
//!
//! Every service exposes a `&'static [ActionSpec]` describing its dotted
//! actions. The MCP `help` action, the `lab://<service>/actions` resource,
//! the top-level `lab.help` meta-tool, the `lab://catalog` resource, and
//! the CLI `lab help` command all read from this same slice. One source of
//! truth for discovery.

/// Compile-time metadata for one dotted action exposed by a service.
#[derive(Debug, Clone, Copy)]
pub struct ActionSpec {
    /// Dotted action name, e.g. `"movie.search"` or `"queue.list"`.
    pub name: &'static str,
    /// One-line human-readable description.
    pub description: &'static str,
    /// True if this action mutates external state. Drives both MCP elicitation
    /// and CLI confirmation prompts — a single source of truth.
    pub destructive: bool,
    /// True if this action requires the `lab:admin` scope on authenticated HTTP
    /// and MCP transports. Defaults to `false` for all services except gateway,
    /// which sets it to `true` on every action except `help` and `schema`.
    ///
    /// Mirrors `destructive` as the single source of truth for scope gating:
    /// - API surface: `api/services/gateway.rs` reads this field instead of a
    ///   bespoke match arm.
    /// - MCP surface: `mcp/context.rs` reads this field instead of a bespoke
    ///   match arm.
    /// - Stdio transport: `None` auth is always trusted (admin by design).
    pub requires_admin: bool,
    /// Declared parameter list.
    pub params: &'static [ParamSpec],
    /// Type-name hint for the return shape, e.g. `"Movie[]"`. Not a runtime
    /// contract — purely informational, echoed in `help` output.
    pub returns: &'static str,
}

/// One declared action parameter.
#[derive(Debug, Clone, Copy)]
pub struct ParamSpec {
    /// Parameter name.
    pub name: &'static str,
    /// Free-form type label: `"string"`, `"integer"`, `"number"`, `"boolean"`,
    /// `"object"`, `"array"`, `"string[]"`, `"integer[]"`, union literals like
    /// `"string|null"`, or enum literals like `"queued|running|done"`.
    /// Translated to JSON Schema by the MCP layer in one helper.
    pub ty: &'static str,
    /// True if this parameter must be present.
    pub required: bool,
    /// Description shown by `help`.
    pub description: &'static str,
}
