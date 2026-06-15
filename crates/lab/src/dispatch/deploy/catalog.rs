//! Action catalog for the `deploy` service.

use lab_apis::core::action::{ActionSpec, ParamSpec};

pub const ACTIONS: &[ActionSpec] = &[
    ActionSpec {
        name: "help",
        description: "List deploy actions",
        destructive: false,
        requires_admin: false,
        params: &[],
        returns: "Catalog",
    },
    ActionSpec {
        name: "schema",
        description: "Per-action JSON schema",
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
        name: "config.list",
        description: "Lists the current deploy configuration. Note: configuration is loaded \
                      once at startup — restart the lab process to pick up changes to \
                      ~/.ssh/config or deploy preferences.",
        destructive: false,
        requires_admin: false,
        params: &[],
        returns: "ConfigListing",
    },
    ActionSpec {
        name: "deploy.plan",
        description: "Dry-run: resolve targets, hash local artifact, show what would happen",
        destructive: false,
        requires_admin: false,
        params: &[ParamSpec {
            name: "targets",
            ty: "string[]",
            required: true,
            description: "SSH host aliases to include in the plan",
        }],
        returns: "DeployPlan",
    },
    ActionSpec {
        name: "deploy.run",
        description: "Build, transfer, install, restart, verify on targets (destructive)",
        destructive: true,
        requires_admin: false,
        params: &[
            ParamSpec {
                name: "targets",
                ty: "string[]",
                required: true,
                description: "SSH host aliases to deploy to",
            },
            ParamSpec {
                name: "confirm",
                ty: "boolean",
                required: true,
                description: "Must be true to confirm the destructive operation",
            },
            ParamSpec {
                name: "max_parallel",
                ty: "integer",
                required: false,
                description: "Maximum number of hosts to work on concurrently",
            },
            ParamSpec {
                name: "fail_fast",
                ty: "boolean",
                required: false,
                description: "Abort remaining hosts on the first failure",
            },
        ],
        returns: "DeployRunSummary",
    },
    ActionSpec {
        name: "deploy.rollback",
        description: "Restore the most recent timestamped backup on the specified targets (destructive)",
        destructive: true,
        requires_admin: false,
        params: &[
            ParamSpec {
                name: "targets",
                ty: "string[]",
                required: true,
                description: "SSH host aliases to roll back",
            },
            ParamSpec {
                name: "confirm",
                ty: "boolean",
                required: true,
                description: "Must be true to confirm the destructive operation",
            },
        ],
        returns: "DeployRunSummary",
    },
    // ---- Deprecated bare-verb aliases (pre-Arch-M3) ----
    // Kept working for existing callers; route to the same handlers as their
    // dotted canonical names above. Do not document these as primary; remove
    // once no caller depends on the bare form. The catalog lint in
    // `tests/architecture_orchestrator.rs` exempts these via
    // DEPRECATED_ACTION_ALIASES.
    ActionSpec {
        name: "plan",
        description: "Deprecated alias for `deploy.plan`",
        destructive: false,
        requires_admin: false,
        params: &[ParamSpec {
            name: "targets",
            ty: "string[]",
            required: true,
            description: "SSH host aliases to include in the plan",
        }],
        returns: "DeployPlan",
    },
    ActionSpec {
        name: "run",
        description: "Deprecated alias for `deploy.run`",
        destructive: true,
        requires_admin: false,
        params: &[
            ParamSpec {
                name: "targets",
                ty: "string[]",
                required: true,
                description: "SSH host aliases to deploy to",
            },
            ParamSpec {
                name: "confirm",
                ty: "boolean",
                required: true,
                description: "Must be true to confirm the destructive operation",
            },
            ParamSpec {
                name: "max_parallel",
                ty: "integer",
                required: false,
                description: "Maximum number of hosts to work on concurrently",
            },
            ParamSpec {
                name: "fail_fast",
                ty: "boolean",
                required: false,
                description: "Abort remaining hosts on the first failure",
            },
        ],
        returns: "DeployRunSummary",
    },
    ActionSpec {
        name: "rollback",
        description: "Deprecated alias for `deploy.rollback`",
        destructive: true,
        requires_admin: false,
        params: &[
            ParamSpec {
                name: "targets",
                ty: "string[]",
                required: true,
                description: "SSH host aliases to roll back",
            },
            ParamSpec {
                name: "confirm",
                ty: "boolean",
                required: true,
                description: "Must be true to confirm the destructive operation",
            },
        ],
        returns: "DeployRunSummary",
    },
];
