//! ActionSpec catalog for `agent.*` actions in the marketplace dispatch.
//!
//! These actions wrap the `lab-apis::acp_registry` SDK to discover, install,
//! and remove ACP-compatible AI coding agents.

use labby_apis::core::action::{ActionSpec, ParamSpec};

pub const ACP_ACTIONS: &[ActionSpec] = &[
    ActionSpec {
        name: "agent.list",
        description: "List ACP-compatible agents from the registry CDN",
        destructive: false,
        requires_admin: false,
        returns: "Agent[]",
        params: &[],
    },
    ActionSpec {
        name: "agent.get",
        description: "Get details for a single ACP agent by id",
        destructive: false,
        requires_admin: false,
        returns: "Agent",
        params: &[ParamSpec {
            name: "id",
            ty: "string",
            required: true,
            description: "Agent id (e.g. `anthropic/claude-code`)",
        }],
    },
    ActionSpec {
        name: "agent.install",
        description: "Install an ACP agent on one or more devices. Local installs write a provider entry to `~/.lab/acp-providers.json`; binary archives are downloaded only over HTTPS, SHA-256 verified, size-limited, and installed atomically.",
        destructive: true,
        requires_admin: false,
        returns: "InstallResults",
        params: &[
            ParamSpec {
                name: "id",
                ty: "string",
                required: true,
                description: "Agent id from the registry",
            },
            ParamSpec {
                name: "node_ids",
                ty: "array",
                required: true,
                description: "Node ids to install on. Use `\"local\"` for the controller host. Remote installs are not yet implemented and return per-node errors.",
            },
            ParamSpec {
                name: "platform",
                ty: "string",
                required: false,
                description: "Override platform triple for binary lookup (e.g. `linux-x86_64`); auto-detected from the host when omitted.",
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
        name: "agent.uninstall",
        description: "Remove an installed ACP agent entry from `~/.lab/acp-providers.json`",
        destructive: true,
        requires_admin: false,
        returns: "UninstallResult",
        params: &[
            ParamSpec {
                name: "id",
                ty: "string",
                required: true,
                description: "Agent id to uninstall",
            },
            ParamSpec {
                name: "confirm",
                ty: "boolean",
                required: true,
                description: "Must be true to confirm the destructive uninstall operation",
            },
        ],
    },
];
