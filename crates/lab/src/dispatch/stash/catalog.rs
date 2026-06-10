//! Action catalog for the `stash` service.
//!
//! Single authoritative source for MCP, CLI, and API adapters.
//! Destructive actions are marked `destructive: true` to drive MCP elicitation
//! and CLI `--yes` requirements.

use lab_apis::core::action::{ActionSpec, ParamSpec};

pub const ACTIONS: &[ActionSpec] = &[
    ActionSpec {
        name: "help",
        description: "Show this action catalog",
        destructive: false,
        requires_admin: false,
        returns: "Catalog",
        params: &[],
    },
    ActionSpec {
        name: "schema",
        description: "Return the parameter schema for a named action",
        destructive: false,
        requires_admin: false,
        returns: "Schema",
        params: &[ParamSpec {
            name: "action",
            ty: "string",
            required: true,
            description: "Action name to describe",
        }],
    },
    // ─── Component lifecycle ────────────────────────────────────────────────
    ActionSpec {
        name: "components.list",
        description: "List all components in the stash",
        destructive: false,
        requires_admin: false,
        returns: "ComponentSummary[]",
        params: &[],
    },
    ActionSpec {
        name: "component.get",
        description: "Get details for a single component",
        destructive: false,
        requires_admin: false,
        returns: "ComponentDetail",
        params: &[ParamSpec {
            name: "id",
            ty: "string",
            required: true,
            description: "Component ID (lowercase ULID, e.g. '01aryz6s41tpz5x11k39dv3r2g')",
        }],
    },
    ActionSpec {
        name: "component.create",
        description: "Create a new component in the stash",
        destructive: false,
        requires_admin: false,
        returns: "ComponentDetail",
        params: &[
            ParamSpec {
                name: "kind",
                ty: "string",
                required: true,
                description: "Component kind: skill, agent, command, channel, monitor, hook, output_style, theme, settings, mcp_config, lsp_config, script, bin_file",
            },
            ParamSpec {
                name: "name",
                ty: "string",
                required: true,
                description: "Component name (used as directory slug)",
            },
            ParamSpec {
                name: "label",
                ty: "string",
                required: false,
                description: "Optional human-readable display label",
            },
        ],
    },
    ActionSpec {
        name: "component.import",
        description: "Import a local path into the stash as a new or updated component [destructive]",
        destructive: true,
        requires_admin: false,
        returns: "ImportResult",
        params: &[
            ParamSpec {
                name: "id",
                ty: "string",
                required: true,
                description: "Target component ID to import into",
            },
            ParamSpec {
                name: "source_path",
                ty: "string",
                required: true,
                description: "Absolute path to the source directory or file",
            },
            ParamSpec {
                name: "kind",
                ty: "string",
                required: false,
                description: "Override component kind detected from source",
            },
        ],
    },
    ActionSpec {
        name: "component.workspace",
        description: "Get the workspace (local checkout) path for a component",
        destructive: false,
        requires_admin: false,
        returns: "WorkspacePath",
        params: &[ParamSpec {
            name: "id",
            ty: "string",
            required: true,
            description: "Component ID",
        }],
    },
    ActionSpec {
        name: "component.save",
        description: "Save (snapshot) the current workspace state for a component",
        destructive: false,
        requires_admin: false,
        returns: "SaveResult",
        params: &[
            ParamSpec {
                name: "id",
                ty: "string",
                required: true,
                description: "Component ID",
            },
            ParamSpec {
                name: "label",
                ty: "string",
                required: false,
                description: "Optional revision label / commit message",
            },
        ],
    },
    ActionSpec {
        name: "component.revisions",
        description: "List saved revisions for a component",
        destructive: false,
        requires_admin: false,
        returns: "Revision[]",
        params: &[ParamSpec {
            name: "id",
            ty: "string",
            required: true,
            description: "Component ID",
        }],
    },
    ActionSpec {
        name: "component.export",
        description: "Export a component to a local path [destructive]",
        // Marked destructive because the action writes files to the filesystem.
        // All exports require confirm: true — the dispatcher does not inspect
        // include_secrets at runtime; destructive: true is the single gate.
        destructive: true,
        requires_admin: false,
        returns: "ExportResult",
        params: &[
            ParamSpec {
                name: "id",
                ty: "string",
                required: true,
                description: "Component ID",
            },
            ParamSpec {
                name: "output_path",
                ty: "string",
                required: true,
                description: "Absolute destination path to export into",
            },
            ParamSpec {
                name: "include_secrets",
                ty: "boolean",
                required: false,
                description: "Include secret env values in the export (default: false)",
            },
            ParamSpec {
                name: "force",
                ty: "boolean",
                required: false,
                description: "Overwrite output_path if it already exists (default: false)",
            },
        ],
    },
    ActionSpec {
        name: "component.deploy",
        description: "Deploy a component to a registered target [destructive]",
        destructive: true,
        requires_admin: false,
        returns: "DeployResult",
        params: &[
            ParamSpec {
                name: "id",
                ty: "string",
                required: true,
                description: "Component ID",
            },
            ParamSpec {
                name: "target_id",
                ty: "string",
                required: true,
                description: "Registered deploy target ID",
            },
            ParamSpec {
                name: "revision_id",
                ty: "string",
                required: false,
                description: "Specific revision to deploy (default: latest saved)",
            },
        ],
    },
    // ─── Provider sync ──────────────────────────────────────────────────────
    ActionSpec {
        name: "providers.list",
        description: "List registered sync providers",
        destructive: false,
        requires_admin: false,
        returns: "Provider[]",
        params: &[],
    },
    ActionSpec {
        name: "provider.link",
        description: "Register a sync provider for a component",
        destructive: true,
        requires_admin: false,
        returns: "Provider",
        params: &[
            ParamSpec {
                name: "id",
                ty: "string",
                required: true,
                description: "Component ID to link the provider to",
            },
            ParamSpec {
                name: "kind",
                ty: "string",
                required: true,
                description: "Provider kind (e.g. 'github', 's3', 'local')",
            },
            ParamSpec {
                name: "label",
                ty: "string",
                required: true,
                description: "Human-readable provider label",
            },
            ParamSpec {
                name: "config",
                ty: "object",
                required: true,
                description: "Provider-specific configuration object (e.g. {repo, branch})",
            },
        ],
    },
    ActionSpec {
        name: "provider.push",
        description: "Push the latest component revision to a provider [destructive]",
        destructive: true,
        requires_admin: false,
        returns: "SyncResult",
        params: &[
            ParamSpec {
                name: "id",
                ty: "string",
                required: true,
                description: "Component ID",
            },
            ParamSpec {
                name: "provider_id",
                ty: "string",
                required: true,
                description: "Provider ID to push to",
            },
        ],
    },
    ActionSpec {
        name: "provider.pull",
        description: "Pull the latest state from a provider into the component [destructive]",
        destructive: true,
        requires_admin: false,
        returns: "SyncResult",
        params: &[
            ParamSpec {
                name: "id",
                ty: "string",
                required: true,
                description: "Component ID",
            },
            ParamSpec {
                name: "provider_id",
                ty: "string",
                required: true,
                description: "Provider ID to pull from",
            },
        ],
    },
    // ─── Deploy targets ─────────────────────────────────────────────────────
    ActionSpec {
        name: "targets.list",
        description: "List registered deploy targets",
        destructive: false,
        requires_admin: false,
        returns: "Target[]",
        params: &[],
    },
    ActionSpec {
        name: "target.add",
        description: "Register a new deploy target for future component.deploy writes",
        destructive: false,
        requires_admin: false,
        returns: "Target",
        params: &[
            ParamSpec {
                name: "name",
                ty: "string",
                required: true,
                description: "Human-readable target name",
            },
            ParamSpec {
                name: "kind",
                ty: "string",
                required: true,
                description: "Target kind (e.g. 'local', 'gateway')",
            },
            ParamSpec {
                name: "path",
                ty: "string",
                required: false,
                description: "Filesystem path (for kind=local)",
            },
            ParamSpec {
                name: "gateway_id",
                ty: "string",
                required: false,
                description: "Gateway connection ID (for kind=gateway)",
            },
        ],
    },
    ActionSpec {
        name: "target.remove",
        description: "Remove a registered deploy target [destructive]",
        destructive: true,
        requires_admin: false,
        returns: "RemoveResult",
        params: &[ParamSpec {
            name: "id",
            ty: "string",
            required: true,
            description: "Target ID to remove",
        }],
    },
];
