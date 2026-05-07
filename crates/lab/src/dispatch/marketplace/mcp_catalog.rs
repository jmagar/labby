//! ActionSpec catalog for `mcp.*` actions in the marketplace dispatch.
//!
//! These actions were absorbed from `dispatch/mcpregistry/catalog.rs`
//! as part of lab-zxx5.2, renamed with the `mcp.` prefix.

use lab_apis::core::action::{ActionSpec, ParamSpec};

pub const MCP_ACTIONS: &[ActionSpec] = &[
    ActionSpec {
        name: "mcp.config",
        description: "Return the resolved MCP registry base URL",
        destructive: false,
        returns: "RegistryConfig",
        params: &[],
    },
    ActionSpec {
        name: "mcp.list",
        description: "List MCP servers from the local registry mirror with optional search, owner filter, and bounded pagination.",
        destructive: false,
        returns: "ServerListResponse",
        params: &[
            ParamSpec {
                name: "search",
                ty: "string",
                required: false,
                description: "Search query to filter servers by name or description",
            },
            ParamSpec {
                name: "owner",
                ty: "string",
                required: false,
                description: "GitHub username or org. Client-side convenience that maps to `search=io.github.{owner}/` (lowercased, trimmed). Ignored if `search` is also set. Does not match non-GitHub publishers. Rejected with `invalid_param` if empty or containing `/` or whitespace.",
            },
            ParamSpec {
                name: "limit",
                ty: "integer",
                required: false,
                description: "Maximum number of results to return (default: 10, max: 100)",
            },
            ParamSpec {
                name: "cursor",
                ty: "string",
                required: false,
                description: "Pagination cursor from a previous response metadata.nextCursor field",
            },
            ParamSpec {
                name: "version",
                ty: "string",
                required: false,
                description: "Filter by package version string",
            },
            ParamSpec {
                name: "updated_since",
                ty: "string",
                required: false,
                description: "ISO 8601 datetime; return only servers updated after this time",
            },
            ParamSpec {
                name: "featured",
                ty: "boolean",
                required: false,
                description: "Filter by Lab metadata curation.featured",
            },
            ParamSpec {
                name: "reviewed",
                ty: "boolean",
                required: false,
                description: "Filter by Lab metadata trust.reviewed",
            },
            ParamSpec {
                name: "recommended",
                ty: "boolean",
                required: false,
                description: "Filter by Lab metadata ux.recommended_for_homelab",
            },
            ParamSpec {
                name: "hidden",
                ty: "boolean",
                required: false,
                description: "Filter by Lab metadata curation.hidden",
            },
            ParamSpec {
                name: "tag",
                ty: "string",
                required: false,
                description: "Filter by a Lab metadata curation tag",
            },
        ],
    },
    ActionSpec {
        name: "mcp.get",
        description: "Get details for a single MCP server by its registry name from the registry client/store surface.",
        destructive: false,
        returns: "ServerResponse",
        params: &[ParamSpec {
            name: "name",
            ty: "string",
            required: true,
            description: "Server name as listed in the registry (e.g. `@modelcontextprotocol/server-github`)",
        }],
    },
    ActionSpec {
        name: "mcp.versions",
        description: "List available versions for a named MCP server from the registry client/store surface.",
        destructive: false,
        returns: "ServerListResponse",
        params: &[ParamSpec {
            name: "name",
            ty: "string",
            required: true,
            description: "Server name to list versions for",
        }],
    },
    ActionSpec {
        name: "mcp.validate",
        description: "Validate a ServerJSON document against the registry schema. Returns a ValidationResult with a boolean valid field and an errors array. Call before mcp.install to surface schema problems without creating a gateway.",
        destructive: false,
        returns: "ValidationResult",
        params: &[ParamSpec {
            name: "server_json",
            ty: "object",
            required: true,
            description: "ServerJSON document to validate (must include name, description, version)",
        }],
    },
    ActionSpec {
        name: "mcp.install",
        description: "Install an MCP server from the registry to Lab gateway upstreams and/or Claude/Codex MCP clients on fleet devices. HTTP servers are added as remote URLs; stdio servers are added as command configs. Required env vars are written to ~/.lab/.env for gateway installs and embedded in the MCP client config for client installs.",
        destructive: true,
        returns: "InstallResults",
        params: &[
            ParamSpec {
                name: "name",
                ty: "string",
                required: true,
                description: "Registry server name (e.g. `io.github.user/my-mcp`)",
            },
            ParamSpec {
                name: "gateway_ids",
                ty: "array",
                required: false,
                description: "Lab gateway names to add this server to — one gateway.add call per entry. Either gateway_ids or client_targets must be provided.",
            },
            ParamSpec {
                name: "client_targets",
                ty: "array",
                required: false,
                description: "Claude/Codex MCP client targets on fleet devices. Each entry is an object with node_id and client (`claude` or `codex`). Either gateway_ids or client_targets must be provided.",
            },
            ParamSpec {
                name: "bearer_token_env",
                ty: "string",
                required: false,
                description: "HTTP only: name of the env var holding the bearer token (not the token value)",
            },
            ParamSpec {
                name: "version",
                ty: "string",
                required: false,
                description: "Registry version to fetch; defaults to `latest`",
            },
            ParamSpec {
                name: "env_values",
                ty: "object",
                required: false,
                description: "Stdio only: map of env var name → value for variables declared by the server's packages[0].environmentVariables. Required vars with no default must be supplied here.",
            },
            ParamSpec {
                name: "confirm",
                ty: "boolean",
                required: true,
                description: "Must be true to confirm the destructive install operation",
            },
        ],
    },
    ActionSpec {
        name: "mcp.uninstall",
        description: "Remove a previously installed MCP server gateway upstream by gateway name",
        destructive: true,
        returns: "GatewayView",
        params: &[
            ParamSpec {
                name: "gateway_name",
                ty: "string",
                required: true,
                description: "Gateway name to remove (as used during install)",
            },
            ParamSpec {
                name: "confirm",
                ty: "boolean",
                required: true,
                description: "Must be true to confirm the destructive uninstall operation",
            },
        ],
    },
    ActionSpec {
        name: "mcp.meta.get",
        description: "Get Lab-owned local metadata for a stored registry server version from the local registry mirror.",
        destructive: false,
        returns: "RegistryLocalMeta",
        params: &[
            ParamSpec {
                name: "name",
                ty: "string",
                required: true,
                description: "Registry server name",
            },
            ParamSpec {
                name: "version",
                ty: "string",
                required: false,
                description: "Version string to read; defaults to `latest` in the local mirror",
            },
        ],
    },
    ActionSpec {
        name: "mcp.meta.set",
        description: "Set Lab-owned local metadata for a stored registry server version under `_meta[\"tv.tootie.lab/registry\"]`.",
        destructive: false,
        returns: "RegistryLocalMeta",
        params: &[
            ParamSpec {
                name: "name",
                ty: "string",
                required: true,
                description: "Registry server name",
            },
            ParamSpec {
                name: "version",
                ty: "string",
                required: false,
                description: "Version string to update; defaults to `latest` in the local mirror",
            },
            ParamSpec {
                name: "metadata",
                ty: "object",
                required: true,
                description: "Lab-owned metadata object stored under `_meta[\"tv.tootie.lab/registry\"]`",
            },
            ParamSpec {
                name: "updated_by",
                ty: "string",
                required: false,
                description: "Audit actor label for this metadata update",
            },
        ],
    },
    ActionSpec {
        name: "mcp.meta.delete",
        description: "Delete Lab-owned local metadata for a stored registry server version under `_meta[\"tv.tootie.lab/registry\"]`.",
        destructive: false,
        returns: "RegistryLocalMetaDeleteResult",
        params: &[
            ParamSpec {
                name: "name",
                ty: "string",
                required: true,
                description: "Registry server name",
            },
            ParamSpec {
                name: "version",
                ty: "string",
                required: false,
                description: "Version string to delete; defaults to `latest` in the local mirror",
            },
        ],
    },
    ActionSpec {
        name: "mcp.sync",
        description: "Trigger an immediate upstream sync of the local registry store. Rate-limited: returns rate_limited if called within 60 seconds of the last sync.",
        destructive: false,
        returns: "SyncResult",
        params: &[],
    },
];
