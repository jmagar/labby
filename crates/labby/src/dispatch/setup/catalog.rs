//! Action catalog for the `setup` Bootstrap orchestrator.

use labby_apis::core::action::{ActionSpec, ParamSpec};

/// Plugin-lifecycle action names — canonical dotted forms paired with their
/// deprecated snake_case aliases. **Single source of truth** for the HTTP
/// loopback restriction enforced in `crate::api::services::setup`.
///
/// Invariant: every name here MUST have (a) a catalog `ActionSpec` below and
/// (b) a dispatch arm in `dispatch.rs`. The gate consumes this list directly,
/// so a name that the dispatcher can route but that is missing here would be a
/// loopback-restriction bypass. The `plugin_lifecycle_actions_*` tests enforce
/// the catalog membership and the dispatch routing so the three locations
/// cannot silently drift.
///
/// Pairs are ordered (canonical, alias) so tests can assert metadata parity.
pub const PLUGIN_LIFECYCLE_ACTIONS: &[&str] = &[
    "plugins.installed",
    "installed_plugins",
    "services.status",
    "services_status",
    "plugin.install",
    "install_plugin",
    "plugin.uninstall",
    "uninstall_plugin",
];

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
    ActionSpec {
        name: "state",
        description: "First-run + draft snapshot for the wizard / settings UI",
        destructive: false,
        requires_admin: false,
        returns: "SetupSnapshot",
        params: &[],
    },
    ActionSpec {
        name: "bootstrap",
        description: "Create ~/.labby/.env with a generated token + loopback defaults when absent (first-run)",
        destructive: false,
        requires_admin: false,
        returns: "BootstrapOutcome",
        params: &[],
    },
    ActionSpec {
        name: "schema.get",
        description: "UiSchema projection for all (or filtered) services",
        destructive: false,
        requires_admin: false,
        returns: "ServiceSchemaMap",
        params: &[ParamSpec {
            name: "services",
            ty: "string[]",
            required: false,
            description: "Optional filter; defaults to every service in the registry",
        }],
    },
    ActionSpec {
        name: "draft.get",
        description: "Read .env.draft with secret values masked to '***'",
        destructive: false,
        requires_admin: false,
        returns: "DraftEntry[]",
        params: &[],
    },
    ActionSpec {
        name: "draft.set",
        description: "Write a key (or section) into .env.draft (validated server-side)",
        destructive: false,
        requires_admin: false,
        returns: "DraftSetOutcome",
        params: &[
            ParamSpec {
                name: "entries",
                ty: "DraftEntry[]",
                required: true,
                description: "Key/value pairs to write into the draft",
            },
            ParamSpec {
                name: "force",
                ty: "boolean",
                required: false,
                description: "Overwrite conflicting draft keys (default false)",
            },
        ],
    },
    ActionSpec {
        name: "draft.discard",
        description: "Discard .env.draft without modifying .env",
        destructive: true,
        requires_admin: false,
        returns: "DraftDiscardOutcome",
        params: &[],
    },
    ActionSpec {
        name: "draft.commit",
        description: "Run audit and atomically merge .env.draft into .env",
        destructive: true,
        requires_admin: false,
        returns: "CommitOutcome",
        params: &[ParamSpec {
            name: "force",
            ty: "boolean",
            required: false,
            description: "Overwrite conflicting .env keys (default false)",
        }],
    },
    ActionSpec {
        name: "settings.state",
        description: "Return section-scoped safe settings values and source metadata",
        destructive: false,
        requires_admin: false,
        returns: "SettingsState",
        params: &[ParamSpec {
            name: "section",
            ty: "string",
            required: false,
            description: "Settings section id; defaults to core",
        }],
    },
    ActionSpec {
        name: "settings.schema",
        description: "Return the safe settings schema with risk and write-policy metadata",
        destructive: false,
        requires_admin: false,
        returns: "SettingsSchema",
        params: &[],
    },
    ActionSpec {
        name: "settings.env_schema",
        description: "Return generated and registry-derived environment variable inventory",
        destructive: false,
        requires_admin: false,
        returns: "EnvSettingSpec[]",
        params: &[],
    },
    ActionSpec {
        name: "settings.advanced_state",
        description: "Return redacted advanced settings state",
        destructive: false,
        requires_admin: false,
        returns: "SettingsState",
        params: &[],
    },
    ActionSpec {
        name: "settings.update",
        description: "Update non-secret operator settings with validation",
        destructive: true,
        requires_admin: true,
        returns: "SettingsState",
        params: &[ParamSpec {
            name: "services.built_in_upstream_apis_enabled",
            ty: "boolean",
            required: true,
            description: "Enable built-in upstream API service integrations",
        }],
    },
    ActionSpec {
        name: "settings.config.update",
        description: "Admin-only scalar config.toml settings update",
        destructive: true,
        requires_admin: true,
        returns: "SettingsMutationOutcome",
        params: &[ParamSpec {
            name: "entries",
            ty: "SettingsUpdateEntry[]",
            required: true,
            description: "Schema-approved config scalar updates",
        }],
    },
    ActionSpec {
        name: "settings.env.update",
        description: "Admin-only targeted .env settings update for known low-risk LAB_* keys",
        destructive: true,
        requires_admin: true,
        returns: "SettingsState",
        params: &[ParamSpec {
            name: "entries",
            ty: "SettingsUpdateEntry[]",
            required: true,
            description: "Schema-approved env scalar updates",
        }],
    },
    ActionSpec {
        name: "plugin_hook",
        description: "Run binary-owned local setup checks for Claude plugin hooks; in repair mode also syncs CLAUDE_PLUGIN_OPTION_* and probes server connectivity",
        destructive: true,
        requires_admin: false,
        // Composite payload: { setup: SetupReport, sync: PluginSyncOutcome|null, connectivity: ConnectivityOutcome }.
        // `sync` is null when called with repair=false (check mode is guaranteed non-mutating).
        returns: "PluginHookReport",
        params: &[ParamSpec {
            name: "repair",
            ty: "boolean",
            required: false,
            description: "Create missing local Lab setup files and sync plugin env; defaults to true",
        }],
    },
    ActionSpec {
        name: "plugin_sync",
        description: "Sync CLAUDE_PLUGIN_OPTION_* env vars into ~/.labby/.env as LAB_* vars",
        destructive: true,
        requires_admin: false,
        returns: "PluginSyncOutcome",
        params: &[],
    },
    ActionSpec {
        name: "plugin_export",
        description: "Read ~/.labby/.env and return current values keyed by userConfig field name",
        destructive: false,
        requires_admin: false,
        returns: "PluginExportOutcome",
        params: &[],
    },
    ActionSpec {
        name: "plugin_connectivity",
        description: "Validate connectivity to the lab MCP server at {server_url}/health",
        destructive: false,
        requires_admin: false,
        returns: "ConnectivityOutcome",
        params: &[ParamSpec {
            name: "server_url",
            ty: "string",
            required: false,
            description: "Override server URL; defaults to CLAUDE_PLUGIN_OPTION_SERVER_URL or http://localhost:8765",
        }],
    },
    ActionSpec {
        name: "check",
        description: "Check local Lab setup prerequisites without mutating the filesystem",
        destructive: false,
        requires_admin: false,
        returns: "SetupReport",
        params: &[],
    },
    ActionSpec {
        name: "repair",
        description: "Repair missing local Lab setup prerequisites without contacting external services",
        destructive: true,
        requires_admin: true,
        returns: "SetupReport",
        params: &[],
    },
    // -- Plugin-lifecycle actions ------------------------------------------
    //
    // These actions are HTTP loopback-gated in
    // `crate::api::services::setup::plugin_lifecycle_action`, which reads its
    // name set from `PLUGIN_LIFECYCLE_ACTIONS` above. The canonical names are
    // the dotted `<resource>.<verb>` forms below; the snake_case entries that
    // follow each one are deprecated aliases retained only for backward
    // compatibility with external callers using the historical names — no
    // in-tree caller depends on them (the CLI uses the dotted forms). Both
    // forms route to the same handler in `dispatch.rs`. Every name in
    // `PLUGIN_LIFECYCLE_ACTIONS` must have an entry here and a dispatch arm;
    // the `plugin_lifecycle_actions_*` tests enforce that lockstep.
    ActionSpec {
        name: "plugins.installed",
        description: "List installed Claude Code lab plugins",
        destructive: false,
        requires_admin: false,
        returns: "InstalledPlugin[]",
        params: &[ParamSpec {
            name: "force",
            ty: "boolean",
            required: false,
            description: "Bypass the short in-process cache",
        }],
    },
    // Deprecated alias for `plugins.installed`.
    ActionSpec {
        name: "installed_plugins",
        description: "Deprecated alias for `plugins.installed`",
        destructive: false,
        requires_admin: false,
        returns: "InstalledPlugin[]",
        params: &[ParamSpec {
            name: "force",
            ty: "boolean",
            required: false,
            description: "Bypass the short in-process cache",
        }],
    },
    ActionSpec {
        name: "services.status",
        description: "Join service configuration, draft, and Claude plugin state",
        destructive: false,
        requires_admin: false,
        returns: "ServiceStatus[]",
        params: &[],
    },
    // Deprecated alias for `services.status`.
    ActionSpec {
        name: "services_status",
        description: "Deprecated alias for `services.status`",
        destructive: false,
        requires_admin: false,
        returns: "ServiceStatus[]",
        params: &[],
    },
    ActionSpec {
        name: "plugin.install",
        description: "Install the Claude Code plugin for one configured service",
        destructive: true,
        requires_admin: false,
        returns: "PluginMutationResult",
        params: &[ParamSpec {
            name: "service",
            ty: "string",
            required: true,
            description: "Registered service name",
        }],
    },
    // Deprecated alias for `plugin.install`.
    ActionSpec {
        name: "install_plugin",
        description: "Deprecated alias for `plugin.install`",
        destructive: true,
        requires_admin: false,
        returns: "PluginMutationResult",
        params: &[ParamSpec {
            name: "service",
            ty: "string",
            required: true,
            description: "Registered service name",
        }],
    },
    ActionSpec {
        name: "plugin.uninstall",
        description: "Uninstall the Claude Code plugin for one service",
        destructive: true,
        requires_admin: false,
        returns: "PluginMutationResult",
        params: &[ParamSpec {
            name: "service",
            ty: "string",
            required: true,
            description: "Registered service name",
        }],
    },
    // Deprecated alias for `plugin.uninstall`.
    ActionSpec {
        name: "uninstall_plugin",
        description: "Deprecated alias for `plugin.uninstall`",
        destructive: true,
        requires_admin: false,
        returns: "PluginMutationResult",
        params: &[ParamSpec {
            name: "service",
            ty: "string",
            required: true,
            description: "Registered service name",
        }],
    },
    ActionSpec {
        name: "finalize",
        description: "Alias for draft.commit; same params, same returns",
        destructive: true,
        requires_admin: false,
        returns: "CommitOutcome",
        params: &[ParamSpec {
            name: "force",
            ty: "boolean",
            required: false,
            description: "Overwrite conflicting .env keys (default false)",
        }],
    },
];
