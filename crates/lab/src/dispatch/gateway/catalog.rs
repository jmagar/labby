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
        name: "gateway.list",
        description: "List configured gateways",
        destructive: false,
        requires_admin: true,
        returns: "ServerView[]",
        params: &[],
    },
    ActionSpec {
        name: "gateway.code_mode.get",
        description: "Read gateway-wide Code Mode exposure and execution settings",
        destructive: false,
        requires_admin: true,
        returns: "CodeModeConfig",
        params: &[],
    },
    ActionSpec {
        name: "gateway.code_mode.set",
        description: "Configure gateway code execution limits",
        destructive: false,
        requires_admin: true,
        returns: "CodeModeConfig",
        params: &[
            ParamSpec {
                name: "enabled",
                ty: "boolean",
                required: false,
                description: "Whether the gateway advertises the Code Mode codemode surface",
            },
            ParamSpec {
                name: "trace_params",
                ty: "boolean",
                required: false,
                description: "Whether call traces include redacted and capped upstream tool params",
            },
            ParamSpec {
                name: "timeout_ms",
                ty: "integer",
                required: false,
                description: "Maximum wall-clock time for one Code Mode execution",
            },
            ParamSpec {
                name: "max_response_bytes",
                ty: "integer",
                required: false,
                description: "Maximum serialized response envelope size",
            },
            ParamSpec {
                name: "max_response_tokens",
                ty: "integer",
                required: false,
                description: "Approximate maximum response tokens",
            },
            ParamSpec {
                name: "token_estimate_divisor",
                ty: "integer",
                required: false,
                description: "Byte-to-token divisor used by response limiting",
            },
            ParamSpec {
                name: "max_log_entries",
                ty: "integer",
                required: false,
                description: "Maximum captured console log lines per execution",
            },
            ParamSpec {
                name: "max_log_bytes",
                ty: "integer",
                required: false,
                description: "Maximum captured console log bytes per execution",
            },
        ],
    },
    ActionSpec {
        name: "gateway.server.get",
        description: "Get one unified server row by id",
        destructive: false,
        requires_admin: true,
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
        requires_admin: true,
        returns: "SupportedServiceView[]",
        params: &[],
    },
    ActionSpec {
        name: "gateway.protected_route.list",
        description: "List Gateway-managed public MCP routes protected by Lab OAuth",
        destructive: false,
        requires_admin: true,
        returns: "ProtectedMcpRouteConfig[]",
        params: &[],
    },
    ActionSpec {
        name: "gateway.protected_route.get",
        description: "Get one Gateway-managed protected MCP route",
        destructive: false,
        requires_admin: true,
        returns: "ProtectedMcpRouteConfig",
        params: &[NAME_PARAM],
    },
    ActionSpec {
        name: "gateway.protected_route.add",
        description: "Add a Gateway-managed protected MCP route",
        destructive: false,
        requires_admin: true,
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
        destructive: false,
        requires_admin: true,
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
        destructive: false,
        requires_admin: true,
        returns: "ProtectedMcpRouteConfig",
        params: &[NAME_PARAM],
    },
    ActionSpec {
        name: "gateway.protected_route.test",
        description: "Validate a proposed protected MCP route without saving it",
        destructive: false,
        requires_admin: true,
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
        destructive: false,
        requires_admin: true,
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
        destructive: false,
        requires_admin: true,
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
        destructive: false,
        requires_admin: true,
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
        requires_admin: true,
        returns: "ServerView[]",
        params: &[],
    },
    ActionSpec {
        name: "gateway.virtual_server.quarantine.restore",
        description: "Restore a quarantined Lab-backed virtual server into the active gateway list",
        destructive: false,
        requires_admin: true,
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
        destructive: false,
        requires_admin: true,
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
        requires_admin: true,
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
        destructive: false,
        requires_admin: true,
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
        requires_admin: true,
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
        requires_admin: true,
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
        destructive: false,
        requires_admin: true,
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
        requires_admin: true,
        returns: "GatewayView",
        params: &[NAME_PARAM],
    },
    ActionSpec {
        name: "gateway.client_config.get",
        description: "Get the MCP client configuration JSON for one gateway",
        destructive: false,
        requires_admin: true,
        returns: "McpClientConfigView",
        params: &[NAME_PARAM],
    },
    ActionSpec {
        name: "gateway.test",
        description: "Test a configured or proposed gateway without saving it (probing a stdio gateway runs its local command)",
        destructive: false,
        requires_admin: true,
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
        ],
    },
    ActionSpec {
        name: "gateway.discover",
        description: "Scan the machine for MCP server configs from known editors and tools (cursor, claude-code, claude-desktop, codex, windsurf, opencode, vscode, gemini). Read-only — does not modify config.",
        destructive: false,
        requires_admin: true,
        returns: "DiscoveredServerView[]",
        params: &[
            ParamSpec {
                name: "clients",
                ty: "string[]",
                required: false,
                description: "Limit scan to these client kinds (e.g. [\"cursor\",\"vscode\"]). Empty means scan all.",
            },
            ParamSpec {
                name: "include_existing",
                ty: "boolean",
                required: false,
                description: "Also return servers already present in the gateway config",
            },
        ],
    },
    ActionSpec {
        name: "gateway.import",
        description: "Import discovered MCP servers into the gateway config as disabled-by-default entries. Servers are marked with their discovery source.",
        destructive: false,
        requires_admin: true,
        returns: "ImportResultView",
        params: &[
            ParamSpec {
                name: "all",
                ty: "boolean",
                required: false,
                description: "Import every discovered server not already in the gateway config",
            },
            ParamSpec {
                name: "names",
                ty: "string[]",
                required: false,
                description: "Specific server names to import. Mutually exclusive with `all`.",
            },
            ParamSpec {
                name: "clients",
                ty: "string[]",
                required: false,
                description: "Limit discovery to these client kinds. Empty means scan all.",
            },
        ],
    },
    ActionSpec {
        name: "gateway.import_pending.list",
        description: "List MCP servers discovered but waiting for operator approval (gateway_import_mode=pending)",
        destructive: false,
        requires_admin: true,
        returns: "PendingImportView[]",
        params: &[],
    },
    ActionSpec {
        name: "gateway.import_pending.approve",
        description: "Approve a pending discovered server and add it to the gateway config as disabled-by-default",
        destructive: false,
        requires_admin: true,
        returns: "PendingImportView",
        params: &[ParamSpec {
            name: "name",
            ty: "string",
            required: true,
            description: "Name of the pending server to approve",
        }],
    },
    ActionSpec {
        name: "gateway.import_pending.reject",
        description: "Reject a pending discovered server and tombstone it so it never re-appears",
        destructive: false,
        requires_admin: true,
        returns: "PendingImportView",
        params: &[ParamSpec {
            name: "name",
            ty: "string",
            required: true,
            description: "Name of the pending server to reject",
        }],
    },
    ActionSpec {
        name: "gateway.import_tombstones.list",
        description: "List operator-deleted imported MCP servers that are suppressed from automatic re-import",
        destructive: false,
        requires_admin: true,
        returns: "ImportTombstoneView[]",
        params: &[],
    },
    ActionSpec {
        name: "gateway.import_tombstones.clear",
        description: "Clear one import tombstone so a previously deleted imported server can be imported again",
        destructive: false,
        requires_admin: true,
        returns: "ImportTombstoneView[]",
        params: &[
            NAME_PARAM,
            ParamSpec {
                name: "source_client",
                ty: "string",
                required: false,
                description: "Client kind that originated the import (e.g. \"cursor\", \"vscode\"). Use to disambiguate when multiple tombstones share the same name.",
            },
            ParamSpec {
                name: "source_path",
                ty: "string",
                required: false,
                description: "Path of the client config file the server was imported from. Use to disambiguate when multiple tombstones share the same name.",
            },
            ParamSpec {
                name: "server_name",
                ty: "string",
                required: false,
                description: "Server name as it appeared in the source client config. Use to disambiguate when the gateway name differs from the original config key.",
            },
            ParamSpec {
                name: "transport_fingerprint",
                ty: "string",
                required: false,
                description: "Stable SHA-256 hash of the transport target (URL for HTTP, command+args for stdio). Use to uniquely identify the tombstone when name alone is ambiguous.",
            },
        ],
    },
    ActionSpec {
        name: "gateway.import_tombstones.restore",
        description: "Atomically clear one import tombstone and restore the matching discovered server as disabled",
        destructive: false,
        requires_admin: true,
        returns: "GatewayView",
        params: &[
            NAME_PARAM,
            ParamSpec {
                name: "source_client",
                ty: "string",
                required: false,
                description: "Client kind that originated the import (e.g. \"cursor\", \"vscode\"). Use to disambiguate when multiple tombstones share the same name.",
            },
            ParamSpec {
                name: "source_path",
                ty: "string",
                required: false,
                description: "Path of the client config file the server was imported from. Use to disambiguate when multiple tombstones share the same name.",
            },
            ParamSpec {
                name: "server_name",
                ty: "string",
                required: false,
                description: "Server name as it appeared in the source client config. Use to disambiguate when the gateway name differs from the original config key.",
            },
            ParamSpec {
                name: "transport_fingerprint",
                ty: "string",
                required: false,
                description: "Stable SHA-256 hash of the transport target (URL for HTTP, command+args for stdio). Use to uniquely identify the tombstone when name alone is ambiguous.",
            },
        ],
    },
    ActionSpec {
        name: "gateway.add",
        description: "Add a gateway and reconcile runtime state",
        destructive: false,
        requires_admin: true,
        returns: "GatewayView",
        params: &[
            ParamSpec {
                name: "spec",
                ty: "json",
                required: true,
                description: "Gateway config payload to persist. \
                    TRUST BOUNDARY: when `command` is set (stdio transport), the gateway \
                    will spawn that command as a local subprocess with labby's full process \
                    environment. Only operators with admin access may call this action; \
                    the command and its arguments are validated against a spawn allowlist \
                    before being persisted, but callers must treat this as an admin-level \
                    code-execution primitive.",
            },
            ParamSpec {
                name: "bearer_token_value",
                ty: "string",
                required: false,
                description: "Write-only: raw bearer token to store securely. Never returned on reads. If bearer_token_env is omitted from the spec, a default env var name is derived from the gateway name.",
            },
        ],
    },
    ActionSpec {
        name: "gateway.update",
        description: "Update a gateway and reconcile runtime state",
        destructive: false,
        requires_admin: true,
        returns: "GatewayView",
        params: &[
            NAME_PARAM,
            ParamSpec {
                name: "patch",
                ty: "json",
                required: true,
                description: "Partial gateway update payload. \
                    TRUST BOUNDARY: if the patch sets or changes `command` (stdio transport), \
                    the gateway will spawn that command as a local subprocess. The command is \
                    validated against a spawn allowlist before being persisted, but this \
                    remains an admin-level code-execution primitive.",
            },
            ParamSpec {
                name: "bearer_token_value",
                ty: "string",
                required: false,
                description: "Write-only: raw bearer token to store securely. Never returned on reads. Requires bearer_token_env in patch or existing config.",
            },
        ],
    },
    ActionSpec {
        name: "gateway.remove",
        description: "Remove a gateway and reconcile runtime state",
        destructive: false,
        requires_admin: true,
        returns: "GatewayView",
        params: &[NAME_PARAM],
    },
    ActionSpec {
        name: "gateway.reload",
        description: "Reload gateways from config and reconcile runtime state",
        destructive: false,
        requires_admin: true,
        returns: "GatewayCatalogDiff",
        params: &[],
    },
    ActionSpec {
        name: "gateway.status",
        description: "Get current runtime gateway status",
        destructive: false,
        requires_admin: true,
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
        requires_admin: true,
        returns: "GatewayToolExposureRowView[]",
        params: &[NAME_PARAM],
    },
    ActionSpec {
        name: "gateway.discovered_resources",
        description: "List discovered upstream resources for one gateway",
        destructive: false,
        requires_admin: true,
        returns: "string[]",
        params: &[NAME_PARAM],
    },
    ActionSpec {
        name: "gateway.discovered_prompts",
        description: "List discovered upstream prompts for one gateway",
        destructive: false,
        requires_admin: true,
        returns: "string[]",
        params: &[NAME_PARAM],
    },
    ActionSpec {
        name: "gateway.servers",
        description: "List upstream MCP servers connected to the gateway, with cached \
                       tool/prompt/resource counts and tools-capability health.",
        destructive: false,
        requires_admin: true,
        returns: "GatewayServersDoc",
        params: &[],
    },
    ActionSpec {
        name: "gateway.schema",
        description: "Return the cached tool schemas (input_schema + meta) for one upstream \
                       MCP server, filtered by its exposure policy.",
        destructive: false,
        requires_admin: false,
        returns: "GatewayServerSchema",
        params: &[ParamSpec {
            name: "name",
            ty: "string",
            required: true,
            description: "Upstream server name (as listed by gateway.servers).",
        }],
    },
    ActionSpec {
        name: "gateway.oauth.probe",
        description: "Probe a URL for OAuth support via RFC 8414 AS metadata discovery. \
                       Rejects userinfo, query strings, and fragments. Registers a transient \
                       OAuth manager keyed by URL host, port, and path; it is persisted only \
                       after a successful callback updates gateway config.",
        destructive: false,
        requires_admin: true,
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
        requires_admin: true,
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
        requires_admin: true,
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
        destructive: false,
        requires_admin: true,
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
        name: "gateway.oauth.wait",
        description: "Poll gateway.oauth.status until the upstream is authenticated or timeout elapses. \
                       Returns {authenticated: bool, timed_out: bool}. \
                       Moves the --wait poll loop out of the CLI into shared dispatch (Q-H3).",
        destructive: false,
        requires_admin: true,
        returns: "{authenticated: bool, timed_out: bool}",
        params: &[
            ParamSpec {
                name: "upstream",
                ty: "string",
                required: true,
                description: "Configured upstream name to wait on",
            },
            ParamSpec {
                name: "subject",
                ty: "string",
                required: false,
                description: "Optional credential owner key. Defaults to the shared gateway subject `gateway`.",
            },
            ParamSpec {
                name: "timeout_secs",
                ty: "integer",
                required: false,
                description: "Maximum seconds to wait (default: 120)",
            },
        ],
    },
    ActionSpec {
        name: "gateway.mcp.enable",
        description: "Enable an upstream MCP server so new sessions discover and proxy it again",
        destructive: false,
        requires_admin: true,
        returns: "GatewayView",
        params: &[NAME_PARAM],
    },
    ActionSpec {
        name: "gateway.mcp.list",
        description: "List upstream MCP runtime state, discovery counts, and likely stale process counts",
        destructive: false,
        requires_admin: true,
        returns: "GatewayMcpRuntimeView[]",
        params: &[],
    },
    ActionSpec {
        name: "gateway.mcp.disable",
        description: "Disable an upstream MCP server and optionally clean up running processes",
        destructive: false,
        requires_admin: true,
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
        destructive: false,
        requires_admin: true,
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
    ActionSpec {
        name: "gateway.public_urls.get",
        description: "Read the resolved canonical public URL pair (app and MCP gateway). \
                       Merges LAB_PUBLIC_URL / LAB_MCP_GATEWAY_URL env vars over config.toml \
                       [public_urls] section and the legacy [auth].public_url field.",
        destructive: false,
        requires_admin: true,
        returns: "{app: string?, mcp_gateway: string?, effective_mcp_gateway: string?}",
        params: &[],
    },
];

#[cfg(test)]
mod tests {
    use super::ACTIONS;

    #[test]
    fn gateway_actions_are_not_destructive_under_data_loss_definition() {
        for spec in ACTIONS {
            assert!(
                !spec.destructive,
                "{} must not be destructive unless it risks permanent, hard-to-recreate data loss",
                spec.name
            );
        }
    }

    #[test]
    fn gateway_code_mode_actions_are_primary_catalog_entries() {
        let get = ACTIONS
            .iter()
            .find(|spec| spec.name == "gateway.code_mode.get")
            .expect("gateway.code_mode.get catalog entry");
        assert!(!get.destructive);

        let set = ACTIONS
            .iter()
            .find(|spec| spec.name == "gateway.code_mode.set")
            .expect("gateway.code_mode.set catalog entry");
        assert!(!set.destructive);
        let params: Vec<&str> = set.params.iter().map(|param| param.name).collect();
        for param in [
            "enabled",
            "trace_params",
            "timeout_ms",
            "max_response_bytes",
            "max_response_tokens",
            "token_estimate_divisor",
            "max_log_entries",
            "max_log_bytes",
        ] {
            assert!(
                params.contains(&param),
                "gateway.code_mode.set catalog missing {param}; have {params:?}"
            );
        }
    }

    #[test]
    fn gateway_code_mode_actions_are_catalog_entries() {
        let get = ACTIONS
            .iter()
            .find(|spec| spec.name == "gateway.code_mode.get")
            .expect("gateway.code_mode.get catalog entry");
        assert!(!get.destructive);

        let set = ACTIONS
            .iter()
            .find(|spec| spec.name == "gateway.code_mode.set")
            .expect("gateway.code_mode.set catalog entry");
        assert!(!set.destructive);
    }

    // ── A-H2 / S5: requires_admin field tests ────────────────────────────────

    #[test]
    fn help_and_schema_do_not_require_admin() {
        for name in ["help", "schema", "gateway.help", "gateway.schema"] {
            if let Some(spec) = ACTIONS.iter().find(|s| s.name == name) {
                assert!(
                    !spec.requires_admin,
                    "`{name}` should not require admin — it is a discovery action"
                );
            }
        }
        // The two that are definitely present
        let help = ACTIONS.iter().find(|s| s.name == "help").expect("help");
        assert!(!help.requires_admin);
        let schema = ACTIONS.iter().find(|s| s.name == "schema").expect("schema");
        assert!(!schema.requires_admin);
    }

    #[test]
    fn all_non_discovery_gateway_actions_require_admin() {
        for spec in ACTIONS {
            if matches!(
                spec.name,
                "help" | "schema" | "gateway.help" | "gateway.schema"
            ) {
                assert!(
                    !spec.requires_admin,
                    "`{}` should NOT require admin (discovery action)",
                    spec.name
                );
            } else {
                assert!(
                    spec.requires_admin,
                    "`{}` should require admin but requires_admin=false",
                    spec.name
                );
            }
        }
    }

    // ── Q-H3: gateway.oauth.wait catalog entry ─────────────────────────────

    #[test]
    fn gateway_oauth_wait_is_in_catalog() {
        let spec = ACTIONS
            .iter()
            .find(|s| s.name == "gateway.oauth.wait")
            .expect("gateway.oauth.wait should be in the catalog");
        // Not destructive (read-like poll), but requires admin.
        assert!(
            !spec.destructive,
            "gateway.oauth.wait should not be destructive"
        );
        assert!(
            spec.requires_admin,
            "gateway.oauth.wait should require admin"
        );
        let param_names: Vec<&str> = spec.params.iter().map(|p| p.name).collect();
        assert!(param_names.contains(&"upstream"), "missing upstream param");
        assert!(
            param_names.contains(&"timeout_secs"),
            "missing timeout_secs param"
        );
    }
}
