use lab_apis::core::action::{ActionSpec, ParamSpec};

use super::mcp_catalog::MCP_ACTIONS;

pub const ACTIONS: &[ActionSpec] = &[
    ActionSpec {
        name: "help",
        description: "Show this action catalog",
        destructive: false,
        requires_admin: false,
        params: &[],
        returns: "Catalog",
    },
    ActionSpec {
        name: "schema",
        description: "Return the parameter schema for a named action",
        destructive: false,
        requires_admin: false,
        params: &[ParamSpec {
            name: "action",
            ty: "string",
            required: true,
            description: "Action name to describe",
        }],
        returns: "Schema",
    },
    ActionSpec {
        name: "sources.list",
        description: "List configured marketplaces",
        destructive: false,
        requires_admin: false,
        params: &[],
        returns: "Marketplace[]",
    },
    ActionSpec {
        name: "plugins.list",
        description: "List plugins across marketplaces. Supports server-side filtering by kind, installed state, and text query. All filter params are optional and additive.",
        destructive: false,
        requires_admin: false,
        params: &[
            ParamSpec {
                name: "marketplace",
                ty: "string",
                required: false,
                description: "Filter to a single marketplace id",
            },
            ParamSpec {
                name: "kind",
                ty: "string",
                required: false,
                description: "Filter by component kind (plugin, mcp_server, acp_agent, source, agent, skill, command, app, hook, channel, executable, theme, asset, file, config, settings, monitor, output_style, lsp_server)",
            },
            ParamSpec {
                name: "installed",
                ty: "bool",
                required: false,
                description: "When true, return only installed items; when false, return only uninstalled items",
            },
            ParamSpec {
                name: "query",
                ty: "string",
                required: false,
                description: "Case-insensitive substring filter applied to name, description, and tags",
            },
        ],
        returns: "Plugin[]",
    },
    ActionSpec {
        name: "plugin.get",
        description: "Return a single plugin by id (`name@marketplace`)",
        destructive: false,
        requires_admin: false,
        params: &[ParamSpec {
            name: "id",
            ty: "string",
            required: true,
            description: "Plugin id in `name@marketplace` form",
        }],
        returns: "Plugin",
    },
    ActionSpec {
        name: "plugin.artifacts",
        description: "List artifact files shipped with an installed plugin",
        destructive: false,
        requires_admin: false,
        params: &[ParamSpec {
            name: "id",
            ty: "string",
            required: true,
            description: "Plugin id in `name@marketplace` form",
        }],
        returns: "Artifact[]",
    },
    ActionSpec {
        name: "plugin.workspace",
        description: "Load or create an app-managed editable workspace mirror for a plugin",
        destructive: false,
        requires_admin: false,
        params: &[ParamSpec {
            name: "id",
            ty: "string",
            required: true,
            description: "Plugin id in `name@marketplace` form",
        }],
        returns: "PluginWorkspace",
    },
    ActionSpec {
        name: "plugin.save",
        description: "Save a file into the plugin workspace mirror",
        destructive: false,
        requires_admin: false,
        params: &[
            ParamSpec {
                name: "id",
                ty: "string",
                required: true,
                description: "Plugin id in `name@marketplace` form",
            },
            ParamSpec {
                name: "path",
                ty: "string",
                required: true,
                description: "Relative file path inside the plugin workspace",
            },
            ParamSpec {
                name: "content",
                ty: "string",
                required: true,
                description: "Updated file contents",
            },
        ],
        returns: "SaveResult",
    },
    ActionSpec {
        name: "plugin.deploy",
        description: "Deploy the saved plugin workspace to the local Claude Code install target",
        destructive: true,
        requires_admin: true,
        params: &[ParamSpec {
            name: "id",
            ty: "string",
            required: true,
            description: "Plugin id in `name@marketplace` form",
        }],
        returns: "DeployResult",
    },
    ActionSpec {
        name: "plugin.deploy.preview",
        description: "Preview changed, skipped, and removed files before deploying the workspace",
        destructive: false,
        requires_admin: false,
        params: &[ParamSpec {
            name: "id",
            ty: "string",
            required: true,
            description: "Plugin id in `name@marketplace` form",
        }],
        returns: "DeployPreviewResult",
    },
    ActionSpec {
        name: "artifact.fork",
        description: "Fork Marketplace artifact(s) into Stash [destructive]",
        destructive: true,
        requires_admin: true,
        params: &[
            ParamSpec {
                name: "plugin_id",
                ty: "string",
                required: true,
                description: "Plugin id in `name@marketplace` form",
            },
            ParamSpec {
                name: "artifacts",
                ty: "array",
                required: false,
                description: "Relative artifact paths to fork; omit to fork the whole plugin",
            },
        ],
        returns: "ForkResponse",
    },
    ActionSpec {
        name: "artifact.list",
        description: "List forked marketplace artifact stashes with drift status.",
        destructive: false,
        requires_admin: false,
        params: &[
            ParamSpec {
                name: "plugin_id",
                ty: "string",
                required: false,
                description: "Optional plugin id in `name@marketplace` form",
            },
            ParamSpec {
                name: "instance",
                ty: "string",
                required: false,
                description: "Multi-instance label",
            },
        ],
        returns: "ForkedPluginStatus[]",
    },
    ActionSpec {
        name: "artifact.unfork",
        description: "Remove fork tracking metadata for artifact(s) or a plugin stash.",
        destructive: true,
        requires_admin: true,
        params: &[
            ParamSpec {
                name: "plugin_id",
                ty: "string",
                required: true,
                description: "Plugin id in `name@marketplace` form",
            },
            ParamSpec {
                name: "artifacts",
                ty: "array",
                required: false,
                description: "Relative artifact paths to unfork; omit for the entire plugin fork",
            },
            ParamSpec {
                name: "instance",
                ty: "string",
                required: false,
                description: "Multi-instance label",
            },
        ],
        returns: "UnforkResult",
    },
    ActionSpec {
        name: "artifact.reset",
        description: "Reset forked artifact(s) back to their upstream base snapshot.",
        destructive: true,
        requires_admin: true,
        params: &[
            ParamSpec {
                name: "plugin_id",
                ty: "string",
                required: true,
                description: "Plugin id in `name@marketplace` form",
            },
            ParamSpec {
                name: "artifacts",
                ty: "array",
                required: false,
                description: "Relative artifact paths to reset; omit for all forked artifacts",
            },
            ParamSpec {
                name: "instance",
                ty: "string",
                required: false,
                description: "Multi-instance label",
            },
        ],
        returns: "ResetResult",
    },
    ActionSpec {
        name: "artifact.diff",
        description: "Show diffs between forked artifact content and upstream/base snapshots.",
        destructive: false,
        requires_admin: false,
        params: &[
            ParamSpec {
                name: "plugin_id",
                ty: "string",
                required: true,
                description: "Plugin id in `name@marketplace` form",
            },
            ParamSpec {
                name: "artifact_path",
                ty: "string",
                required: false,
                description: "Optional relative artifact path; omitted returns all fork diffs",
            },
            ParamSpec {
                name: "instance",
                ty: "string",
                required: false,
                description: "Multi-instance label",
            },
        ],
        returns: "ArtifactDiffResult",
    },
    ActionSpec {
        name: "artifact.patch",
        description: "Apply a patch to one forked artifact in the marketplace stash.",
        destructive: false,
        requires_admin: true,
        params: &[
            ParamSpec {
                name: "plugin_id",
                ty: "string",
                required: true,
                description: "Plugin id in `name@marketplace` form",
            },
            ParamSpec {
                name: "artifact_path",
                ty: "string",
                required: true,
                description: "Relative artifact path inside the plugin stash",
            },
            ParamSpec {
                name: "patch",
                ty: "string",
                required: true,
                description: "Unified patch content to apply",
            },
            ParamSpec {
                name: "description",
                ty: "string",
                required: false,
                description: "Optional patch record description",
            },
            ParamSpec {
                name: "instance",
                ty: "string",
                required: false,
                description: "Multi-instance label",
            },
        ],
        returns: "PatchResult",
    },
    ActionSpec {
        name: "artifact.update.check",
        description: "Check whether a forked plugin artifact stash has an upstream update",
        destructive: false,
        requires_admin: false,
        params: &[ParamSpec {
            name: "plugin_id",
            ty: "string",
            required: false,
            description: "Optional plugin id in `name@marketplace` form; omitted scans all forked artifact stashes",
        }],
        returns: "UpdateCheckResult[]",
    },
    ActionSpec {
        name: "artifact.update.preview",
        description: "Preview artifact update changes and conflicts for a forked plugin stash",
        destructive: false,
        requires_admin: false,
        params: &[
            ParamSpec {
                name: "plugin_id",
                ty: "string",
                required: true,
                description: "Plugin id in `name@marketplace` form",
            },
            ParamSpec {
                name: "artifact_path",
                ty: "string",
                required: false,
                description: "Optional relative artifact path when the plugin has multiple artifact forks",
            },
        ],
        returns: "UpdatePreviewResult",
    },
    ActionSpec {
        name: "artifact.update.apply",
        description: "Apply a pending upstream artifact update to a forked plugin stash",
        destructive: true,
        requires_admin: true,
        params: &[
            ParamSpec {
                name: "plugin_id",
                ty: "string",
                required: true,
                description: "Plugin id in `name@marketplace` form",
            },
            ParamSpec {
                name: "strategy",
                ty: "string",
                required: false,
                description: "Merge strategy: keep_mine, take_upstream, always_ask, ai_suggest",
            },
            ParamSpec {
                name: "artifact_path",
                ty: "string",
                required: false,
                description: "Optional relative artifact path when the plugin has multiple artifact forks",
            },
        ],
        returns: "ApplyResult",
    },
    ActionSpec {
        name: "artifact.merge.suggest",
        description: "Request an AI merge suggestion for one conflicted artifact file",
        destructive: false,
        requires_admin: false,
        params: &[
            ParamSpec {
                name: "plugin_id",
                ty: "string",
                required: true,
                description: "Plugin id in `name@marketplace` form",
            },
            ParamSpec {
                name: "artifact_path",
                ty: "string",
                required: true,
                description: "Relative artifact path inside the plugin stash",
            },
        ],
        returns: "MergeSuggestResult",
    },
    ActionSpec {
        name: "artifact.config.set",
        description: "Update artifact update preferences for a forked plugin stash",
        destructive: false,
        requires_admin: true,
        params: &[
            ParamSpec {
                name: "plugin_id",
                ty: "string",
                required: true,
                description: "Plugin id in `name@marketplace` form",
            },
            ParamSpec {
                name: "strategy",
                ty: "string",
                required: false,
                description: "Merge strategy: keep_mine, take_upstream, always_ask, ai_suggest",
            },
            ParamSpec {
                name: "notify",
                ty: "boolean",
                required: false,
                description: "Whether to notify when updates are available",
            },
            ParamSpec {
                name: "artifact_path",
                ty: "string",
                required: false,
                description: "Optional relative artifact path when the plugin has multiple artifact forks",
            },
        ],
        returns: "ConfigSetResult",
    },
    ActionSpec {
        name: "sources.add",
        description: "Register a new marketplace via `claude plugin marketplace add`",
        destructive: true,
        requires_admin: true,
        params: &[
            ParamSpec {
                name: "repo",
                ty: "string",
                required: false,
                description: "GitHub `owner/repo` slug (mutually exclusive with `url`)",
            },
            ParamSpec {
                name: "url",
                ty: "string",
                required: false,
                description: "Git URL (mutually exclusive with `repo`)",
            },
            ParamSpec {
                name: "autoUpdate",
                ty: "boolean",
                required: false,
                description: "Persist whether this marketplace should auto-update",
            },
        ],
        returns: "AddResult",
    },
    ActionSpec {
        name: "plugin.install",
        description: "Install a plugin via `claude plugin install`",
        destructive: true,
        requires_admin: true,
        params: &[ParamSpec {
            name: "id",
            ty: "string",
            required: true,
            description: "Plugin id in `name@marketplace` form",
        }],
        returns: "InstallResult",
    },
    ActionSpec {
        name: "plugin.uninstall",
        description: "Uninstall a plugin via `claude plugin uninstall`",
        destructive: true,
        requires_admin: true,
        params: &[ParamSpec {
            name: "id",
            ty: "string",
            required: true,
            description: "Plugin id in `name@marketplace` form",
        }],
        returns: "UninstallResult",
    },
    // ── ACP agent actions (lab-zxx5.3) ───────────────────────────────────
    // Mirrors `acp_catalog::ACP_ACTIONS`; this catalog drives `help`/`schema`
    // for the marketplace MCP/CLI/API surface.
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
        requires_admin: true,
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
                description: "Node ids to install on (`\"local\"` for the controller host)",
            },
            ParamSpec {
                name: "platform",
                ty: "string",
                required: false,
                description: "Override platform triple for binary lookup (e.g. `linux-x86_64`)",
            },
            ParamSpec {
                name: "confirm",
                ty: "boolean",
                required: true,
                description: "Must be true to confirm the destructive install operation",
            },
        ],
    },
    // ── Plugin cherry-pick (lab-zxx5.6) ──────────────────────────────────────
    ActionSpec {
        name: "plugin.cherry_pick",
        description: "Install selected components from a plugin to one or more devices",
        destructive: true,
        requires_admin: true,
        params: &[
            ParamSpec {
                name: "plugin_id",
                ty: "string",
                required: true,
                description: "Plugin id in `name@marketplace` form",
            },
            ParamSpec {
                name: "components",
                ty: "array",
                required: true,
                description: "Component paths to install (e.g. `agents/my-agent.md`)",
            },
            ParamSpec {
                name: "node_ids",
                ty: "array",
                required: true,
                description: "Target node ids (`\"local\"` for the controller host)",
            },
            ParamSpec {
                name: "scope",
                ty: "string",
                required: true,
                description: "`global` (to `~/.claude/`) or `project` (to `project_path/.claude/`)",
            },
            ParamSpec {
                name: "project_path",
                ty: "string",
                required: false,
                description: "Absolute project path — required when `scope` is `project`",
            },
            ParamSpec {
                name: "confirm",
                ty: "boolean",
                required: true,
                description: "Must be `true` to confirm this destructive operation",
            },
        ],
        returns: "CherryPickResults",
    },
    ActionSpec {
        name: "agent.uninstall",
        description: "Remove an installed ACP agent entry from `~/.lab/acp-providers.json`",
        destructive: true,
        requires_admin: true,
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

pub fn actions() -> &'static [ActionSpec] {
    static ACTIONS: std::sync::LazyLock<&'static [ActionSpec]> = std::sync::LazyLock::new(|| {
        let mut all = Vec::new();
        all.extend_from_slice(self::ACTIONS);
        all.extend_from_slice(MCP_ACTIONS);
        Vec::leak(all)
    });
    &ACTIONS
}

#[allow(dead_code)]
pub fn action_requires_admin(action: &str) -> bool {
    actions()
        .iter()
        .any(|spec| spec.name == action && spec.destructive)
        || matches!(
            action,
            "plugin.workspace"
                | "plugin.save"
                | "artifact.fork"
                | "artifact.patch"
                | "artifact.config.set"
        )
}
