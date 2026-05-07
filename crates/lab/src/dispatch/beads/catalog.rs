use lab_apis::core::action::{ActionSpec, ParamSpec};

const PROJECT_PARAM: ParamSpec = ParamSpec {
    name: "project",
    ty: "string",
    required: false,
    description: "Dolt database name. Falls back to BEADS_DEFAULT_PROJECT if omitted.",
};

const ID_PARAM: ParamSpec = ParamSpec {
    name: "id",
    ty: "string",
    required: true,
    description: "Beads issue id",
};

const LIMIT_PARAM: ParamSpec = ParamSpec {
    name: "limit",
    ty: "integer",
    required: false,
    description: "Maximum issues to return, capped at 500",
};

pub const ACTIONS: &[ActionSpec] = &[
    ActionSpec {
        name: "help",
        description: "Show this action catalog",
        destructive: false,
        returns: "Catalog",
        params: &[],
    },
    ActionSpec {
        name: "schema",
        description: "Return the parameter schema for a named action",
        destructive: false,
        returns: "Schema",
        params: &[ParamSpec {
            name: "action",
            ty: "string",
            required: true,
            description: "Action name to describe",
        }],
    },
    ActionSpec {
        name: "contract.status",
        description: "Return the Beads/Dolt integration contract",
        destructive: false,
        returns: "ContractStatus",
        params: &[],
    },
    ActionSpec {
        name: "health.status",
        description: "Check Dolt reachability and report the server version",
        destructive: false,
        returns: "BeadsHealth",
        params: &[],
    },
    ActionSpec {
        name: "version.get",
        description: "Return the Dolt SQL server version",
        destructive: false,
        returns: "DoltVersion",
        params: &[],
    },
    ActionSpec {
        name: "project.list",
        description: "List the Dolt databases visible on the server (Beads projects)",
        destructive: false,
        returns: "Project[]",
        params: &[],
    },
    ActionSpec {
        name: "context.get",
        description: "Return headline counters for the requested project",
        destructive: false,
        returns: "BeadsContext",
        params: &[PROJECT_PARAM],
    },
    ActionSpec {
        name: "status.summary",
        description: "Return per-status issue counts for the requested project",
        destructive: false,
        returns: "StatusSummary",
        params: &[PROJECT_PARAM],
    },
    ActionSpec {
        name: "issue.list",
        description: "List issues, optionally filtered by stored status",
        destructive: false,
        returns: "Issue[]",
        params: &[
            PROJECT_PARAM,
            ParamSpec {
                name: "status",
                ty: "string",
                required: false,
                description: "Optional stored status filter: open, in_progress, blocked, deferred, closed",
            },
            LIMIT_PARAM,
        ],
    },
    ActionSpec {
        name: "issue.ready",
        description: "List ready (unblocked) issues",
        destructive: false,
        returns: "Issue[]",
        params: &[PROJECT_PARAM, LIMIT_PARAM],
    },
    ActionSpec {
        name: "issue.show",
        description: "Show one Beads issue plus dependencies and comments",
        destructive: false,
        returns: "IssueDetail",
        params: &[PROJECT_PARAM, ID_PARAM],
    },
    ActionSpec {
        name: "graph.show",
        description: "Walk the dependency graph from a root issue (capped at 100 nodes)",
        destructive: false,
        returns: "DependencyGraph",
        params: &[PROJECT_PARAM, ID_PARAM],
    },
];

#[cfg(test)]
mod tests {
    use super::ACTIONS;

    #[test]
    fn beads_actions_are_read_only() {
        for spec in ACTIONS {
            assert!(!spec.destructive, "{} should remain read-only", spec.name);
        }
    }

    #[test]
    fn beads_catalog_includes_project_list() {
        assert!(ACTIONS.iter().any(|a| a.name == "project.list"));
    }
}
