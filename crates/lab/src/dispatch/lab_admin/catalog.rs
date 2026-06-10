use lab_apis::core::action::{ActionSpec, ParamSpec};

/// Action catalog for the internal `lab_admin` tool.
///
/// This is the single authoritative source. MCP, CLI, and API re-export
/// or reference it.
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
        name: "onboarding.audit",
        description: "Audit service onboarding against the current repo contract",
        destructive: false,
        requires_admin: false,
        params: &[ParamSpec {
            name: "services",
            ty: "string[]",
            required: true,
            description: "Services to audit",
        }],
        returns: "AuditReport",
    },
];
