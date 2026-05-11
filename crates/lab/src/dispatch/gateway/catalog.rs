use lab_apis::core::action::{ActionSpec, ParamSpec};

const NAME_PARAM: ParamSpec = ParamSpec {
    name: "name",
    ty: "string",
    required: true,
    description: "Gateway name",
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
        name: "gateway.list",
        description: "List configured gateways",
        destructive: false,
        returns: "ServerView[]",
        params: &[],
    },
    ActionSpec {
        name: "gateway.tool_search.get",
        description: "Read the gateway-wide tool-search settings",
        destructive: false,
        returns: "ToolSearchConfig",
        params: &[],
    },
    ActionSpec {
        name: "gateway.tool_search.set",
        description: "Enable or disable gateway-wide tool-search mode for all exposed upstream tools",
        destructive: true,
        returns: "ToolSearchConfig",
        params: &[
            ParamSpec {
                name: "enabled",
                ty: "boolean",
                required: true,
                description: "Whether tool_search/tool_invoke mode is enabled for the gateway",
            },
            ParamSpec {
                name: "top_k_default",
                ty: "integer",
                required: false,
                description: "Default result count for tool_search when top_k is omitted",
            },
            ParamSpec {
                name: "max_tools",
                ty: "integer",
                required: false,
                description: "Maximum number of tools to index per rebuild",
            },
        ],
    },
    ActionSpec {
        name: "gateway.server.get",
        description: "Get one unified server row by id",
        destructive: false,
        returns: "ServerView",
        params: &[ParamSpec {
            name: "id",
            ty: "string",
            required: true,
            description: "Unified server id",
        }],
    },
    ActionSpec {
        name: "gateway.supported_services",
        description: "List metadata-backed Lab services that can be added as virtual servers",
        destructive: false,
        returns: "SupportedServiceView[]",
        params: &[],
    },
    ActionSpec {
        name: "gateway.protected_route.list",
        description: "List Gateway-managed public MCP routes protected by Lab OAuth",
        destructive: false,
        returns: "ProtectedMcpRouteConfig[]",
        params: &[],
    },
    ActionSpec {
        name: "gateway.protected_route.get",
        description: "Get one Gateway-managed protected MCP route",
        destructive: false,
        returns: "ProtectedMcpRouteConfig",
        params: &[NAME_PARAM],
    },
    ActionSpec {
        name: "gateway.protected_route.add",
        description: "Add a Gateway-managed protected MCP route",
        destructive: true,
        returns: "ProtectedMcpRouteConfig",
        params: &[ParamSpec {
            name: "route",
            ty: "json",
            required: true,
            description: "Protected MCP route config to validate and persist",
        }],
    },
    ActionSpec {
        name: "gateway.protected_route.update",
        description: "Replace a Gateway-managed protected MCP route",
        destructive: true,
        returns: "ProtectedMcpRouteConfig",
        params: &[
            NAME_PARAM,
            ParamSpec {
                name: "route",
                ty: "json",
                required: true,
                description: "Replacement protected MCP route config",
            },
        ],
    },
    ActionSpec {
        name: "gateway.protected_route.remove",
        description: "Remove a Gateway-managed protected MCP route",
        destructive: true,
        returns: "ProtectedMcpRouteConfig",
        params: &[NAME_PARAM],
    },
    ActionSpec {
        name: "gateway.protected_route.test",
        description: "Validate a proposed protected MCP route without saving it",
        destructive: false,
        returns: "ProtectedMcpRouteTestResult",
        params: &[ParamSpec {
            name: "route",
            ty: "json",
            required: true,
            description: "Protected MCP route config to validate",
        }],
    },
    ActionSpec {
        name: "gateway.virtual_server.enable",
        description: "Enable a configured Lab-backed service as a virtual server",
        destructive: true,
        returns: "ServerView",
        params: &[ParamSpec {
            name: "id",
            ty: "string",
            required: true,
            description: "Virtual server id",
        }],
    },
    ActionSpec {
        name: "gateway.virtual_server.disable",
        description: "Disable a Lab-backed virtual server without removing its config",
        destructive: true,
        returns: "ServerView",
        params: &[ParamSpec {
            name: "id",
            ty: "string",
            required: true,
            description: "Virtual server id",
        }],
    },
    ActionSpec {
        name: "gateway.virtual_server.remove",
        description: "Remove a Lab-backed virtual server config entry",
        destructive: true,
        returns: "ServerView",
        params: &[ParamSpec {
            name: "id",
            ty: "string",
            required: true,
            description: "Virtual server id",
        }],
    },
    ActionSpec {
        name: "gateway.virtual_server.quarantine.list",
        description: "List Lab-backed virtual servers quarantined during config migration",
        destructive: false,
        returns: "ServerView[]",
        params: &[],
    },
    ActionSpec {
        name: "gateway.virtual_server.quarantine.restore",
        description: "Restore a quarantined Lab-backed virtual server into the active gateway list",
        destructive: true,
        returns: "ServerView",
        params: &[ParamSpec {
            name: "id",
            ty: "string",
            required: true,
            description: "Quarantined virtual server id",
        }],
    },
    ActionSpec {
        name: "gateway.virtual_server.set_surface",
        description: "Enable or disable one surface on a Lab-backed virtual server",
        destructive: true,
        returns: "ServerView",
        params: &[
            ParamSpec {
                name: "id",
                ty: "string",
                required: true,
                description: "Virtual server id",
            },
            ParamSpec {
                name: "surface",
                ty: "string",
                required: true,
                description: "Surface name: cli, api, mcp, or webui",
            },
            ParamSpec {
                name: "enabled",
                ty: "boolean",
                required: true,
                description: "Whether the surface should be enabled",
            },
        ],
    },
    ActionSpec {
        name: "gateway.virtual_server.get_mcp_policy",
        description: "Read the MCP action allowlist for a Lab-backed virtual server",
        destructive: false,
        returns: "VirtualServerMcpPolicyView",
        params: &[ParamSpec {
            name: "id",
            ty: "string",
            required: true,
            description: "Virtual server id",
        }],
    },
    ActionSpec {
        name: "gateway.virtual_server.set_mcp_policy",
        description: "Replace the MCP action allowlist for a Lab-backed virtual server",
        destructive: true,
        returns: "VirtualServerMcpPolicyView",
        params: &[
            ParamSpec {
                name: "id",
                ty: "string",
                required: true,
                description: "Virtual server id",
            },
            ParamSpec {
                name: "allowed_actions",
                ty: "string[]",
                required: true,
                description: "Exact Lab action names to expose. Empty means expose all actions.",
            },
        ],
    },
    ActionSpec {
        name: "gateway.service_config.get",
        description: "Read canonical config for one Lab-backed service",
        destructive: false,
        returns: "ServiceConfigView",
        params: &[ParamSpec {
            name: "service",
            ty: "string",
            required: true,
            description: "Service key",
        }],
    },
    ActionSpec {
        name: "gateway.service_actions",
        description: "List compiled action metadata for one Lab-backed service",
        destructive: false,
        returns: "ServiceActionView[]",
        params: &[ParamSpec {
            name: "service",
            ty: "string",
            required: true,
            description: "Service key",
        }],
    },
    ActionSpec {
        name: "gateway.service_config.set",
        description: "Write canonical config for one Lab-backed service",
        destructive: true,
        returns: "ServiceConfigView",
        params: &[
            ParamSpec {
                name: "service",
                ty: "string",
                required: true,
                description: "Service key",
            },
            ParamSpec {
                name: "values",
                ty: "json",
                required: true,
                description: "Env-field map to persist for this service",
            },
        ],
    },
    ActionSpec {
        name: "gateway.get",
        description: "Get one configured gateway",
        destructive: false,
        returns: "GatewayView",
        params: &[NAME_PARAM],
    },
    ActionSpec {
        name: "gateway.client_config.get",
        description: "Get the MCP client configuration JSON for one gateway",
        destructive: false,
        returns: "McpClientConfigView",
        params: &[NAME_PARAM],
    },
    ActionSpec {
        name: "gateway.test",
        description: "Test a configured or proposed gateway without saving it",
        destructive: false,
        returns: "GatewayTestResult",
        params: &[
            ParamSpec {
                name: "name",
                ty: "string",
                required: false,
                description: "Configured gateway name to test",
            },
            ParamSpec {
                name: "spec",
                ty: "json",
                required: false,
                description: "Proposed gateway config payload to test without saving",
            },
            ParamSpec {
                name: "allow_stdio",
                ty: "boolean",
                required: false,
                description: "Deprecated compatibility flag; ignored by current Lab versions",
            },
        ],
    },
    ActionSpec {
        name: "gateway.add",
        description: "Add a gateway and reconcile runtime state",
        destructive: true,
        returns: "GatewayView",
        params: &[
            ParamSpec {
                name: "spec",
                ty: "json",
                required: true,
                description: "Gateway config payload to persist",
            },
            ParamSpec {
                name: "bearer_token_value",
                ty: "string",
                required: false,
                description: "Write-only: raw bearer token to store securely. Never returned on reads. If bearer_token_env is omitted from the spec, a default env var name is derived from the gateway name.",
            },
            ParamSpec {
                name: "allow_stdio",
                ty: "boolean",
                required: false,
                description: "Deprecated compatibility flag; ignored by current Lab versions",
            },
        ],
    },
    ActionSpec {
        name: "gateway.update",
        description: "Update a gateway and reconcile runtime state",
        destructive: true,
        returns: "GatewayView",
        params: &[
            NAME_PARAM,
            ParamSpec {
                name: "patch",
                ty: "json",
                required: true,
                description: "Partial gateway update payload",
            },
            ParamSpec {
                name: "bearer_token_value",
                ty: "string",
                required: false,
                description: "Write-only: raw bearer token to store securely. Never returned on reads. Requires bearer_token_env in patch or existing config.",
            },
            ParamSpec {
                name: "allow_stdio",
                ty: "boolean",
                required: false,
                description: "Deprecated compatibility flag; ignored by current Lab versions",
            },
        ],
    },
    ActionSpec {
        name: "gateway.remove",
        description: "Remove a gateway and reconcile runtime state",
        destructive: true,
        returns: "GatewayView",
        params: &[NAME_PARAM],
    },
    ActionSpec {
        name: "gateway.reload",
        description: "Reload gateways from config and reconcile runtime state",
        destructive: true,
        returns: "GatewayCatalogDiff",
        params: &[],
    },
    ActionSpec {
        name: "gateway.status",
        description: "Get current runtime gateway status",
        destructive: false,
        returns: "GatewayRuntimeView[]",
        params: &[ParamSpec {
            name: "name",
            ty: "string",
            required: false,
            description: "Optional gateway name filter",
        }],
    },
    ActionSpec {
        name: "gateway.discovered_tools",
        description: "List discovered upstream tool metadata and exposure state for one gateway",
        destructive: false,
        returns: "GatewayToolExposureRowView[]",
        params: &[NAME_PARAM],
    },
    ActionSpec {
        name: "gateway.discovered_resources",
        description: "List discovered upstream resources for one gateway",
        destructive: false,
        returns: "string[]",
        params: &[NAME_PARAM],
    },
    ActionSpec {
        name: "gateway.discovered_prompts",
        description: "List discovered upstream prompts for one gateway",
        destructive: false,
        returns: "string[]",
        params: &[NAME_PARAM],
    },
    ActionSpec {
        name: "gateway.oauth.probe",
        description: "Probe a URL for OAuth support via RFC 8414 AS metadata discovery. \
                       Rejects userinfo, query strings, and fragments. Registers a transient \
                       OAuth manager keyed by URL host, port, and path; it is persisted only \
                       after a successful callback updates gateway config.",
        destructive: true,
        returns: "ProbeResult",
        params: &[ParamSpec {
            name: "url",
            ty: "string",
            required: true,
            description: "HTTPS URL of the upstream MCP server to probe for OAuth support",
        }],
    },
    ActionSpec {
        name: "gateway.oauth.start",
        description: "Start the upstream OAuth flow for the shared gateway credential and return the browser authorization URL",
        destructive: false,
        returns: "BeginAuthorization",
        params: &[
            ParamSpec {
                name: "upstream",
                ty: "string",
                required: true,
                description: "Configured upstream name",
            },
            ParamSpec {
                name: "subject",
                ty: "string",
                required: false,
                description: "Optional credential owner key. Defaults to the shared gateway subject `gateway`.",
            },
        ],
    },
    ActionSpec {
        name: "gateway.oauth.status",
        description: "Read upstream OAuth status for the shared gateway credential",
        destructive: false,
        returns: "UpstreamOauthStatusView",
        params: &[
            ParamSpec {
                name: "upstream",
                ty: "string",
                required: true,
                description: "Configured upstream name",
            },
            ParamSpec {
                name: "subject",
                ty: "string",
                required: false,
                description: "Optional credential owner key. Defaults to the shared gateway subject `gateway`.",
            },
        ],
    },
    ActionSpec {
        name: "gateway.oauth.clear",
        description: "Clear stored upstream OAuth credentials for the shared gateway credential",
        destructive: true,
        returns: "ok",
        params: &[
            ParamSpec {
                name: "upstream",
                ty: "string",
                required: true,
                description: "Configured upstream name",
            },
            ParamSpec {
                name: "subject",
                ty: "string",
                required: false,
                description: "Optional credential owner key. Defaults to the shared gateway subject `gateway`.",
            },
        ],
    },
    ActionSpec {
        name: "gateway.mcp.enable",
        description: "Enable an upstream MCP server so new sessions discover and proxy it again",
        destructive: true,
        returns: "GatewayView",
        params: &[NAME_PARAM],
    },
    ActionSpec {
        name: "gateway.mcp.list",
        description: "List upstream MCP runtime state, discovery counts, and likely stale process counts",
        destructive: false,
        returns: "GatewayMcpRuntimeView[]",
        params: &[],
    },
    ActionSpec {
        name: "gateway.mcp.disable",
        description: "Disable an upstream MCP server and optionally clean up running processes",
        destructive: true,
        returns: "GatewayView + optional cleanup result",
        params: &[
            NAME_PARAM,
            ParamSpec {
                name: "cleanup",
                ty: "boolean",
                required: false,
                description: "When true, run runtime cleanup after disabling",
            },
            ParamSpec {
                name: "aggressive",
                ty: "boolean",
                required: false,
                description: "When true, use broader host-wide process matching during cleanup",
            },
        ],
    },
    ActionSpec {
        name: "gateway.mcp.cleanup",
        description: "Kill or preview running processes associated with one upstream MCP server",
        destructive: true,
        returns: "GatewayCleanupView",
        params: &[
            NAME_PARAM,
            ParamSpec {
                name: "aggressive",
                ty: "boolean",
                required: false,
                description: "When true, use broader host-wide process matching during cleanup",
            },
            ParamSpec {
                name: "dry_run",
                ty: "boolean",
                required: false,
                description: "When true, preview cleanup matches without killing anything",
            },
        ],
    },
];

#[cfg(test)]
mod tests {
    use super::ACTIONS;

    #[test]
    fn gateway_reconciliation_actions_are_marked_destructive() {
        for action in [
            "gateway.add",
            "gateway.update",
            "gateway.remove",
            "gateway.reload",
            "gateway.oauth.probe",
        ] {
            let spec = ACTIONS
                .iter()
                .find(|spec| spec.name == action)
                .expect("gateway action");
            assert!(spec.destructive, "{action} should be destructive");
        }
    }

    #[test]
    fn gateway_read_actions_remain_non_destructive() {
        for action in [
            "gateway.list",
            "gateway.get",
            "gateway.test",
            "gateway.status",
            "gateway.virtual_server.quarantine.list",
            "gateway.discovered_tools",
            "gateway.discovered_resources",
            "gateway.discovered_prompts",
            "gateway.mcp.list",
        ] {
            let spec = ACTIONS
                .iter()
                .find(|spec| spec.name == action)
                .expect("gateway action");
            assert!(!spec.destructive, "{action} should remain non-destructive");
        }
    }
}
