use labby_apis::core::action::{ActionSpec, ParamSpec};

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
        name: "system.checks",
        description: "Run local system probes: env vars, Docker, disk, ports, config files",
        destructive: false,
        requires_admin: false,
        returns: "DoctorReport",
        params: &[],
    },
    ActionSpec {
        name: "service.probe",
        description: "Probe a single named service via its health endpoint",
        destructive: false,
        requires_admin: false,
        returns: "Finding",
        params: &[
            ParamSpec {
                name: "service",
                ty: "string",
                required: true,
                description: "Service name to probe (e.g. \"radarr\", \"sonarr\")",
            },
            ParamSpec {
                name: "instance",
                ty: "string",
                required: false,
                description: "Named instance label for multi-instance services",
            },
        ],
    },
    ActionSpec {
        name: "audit.full",
        description: "Probe all configured services plus system checks; streams results",
        destructive: false,
        requires_admin: false,
        returns: "stream<Finding>",
        params: &[],
    },
    ActionSpec {
        name: "auth.check",
        description: "Check auth/OAuth configuration: env vars, file presence, and Unix file permissions",
        destructive: false,
        requires_admin: false,
        returns: "DoctorReport",
        params: &[],
    },
    ActionSpec {
        name: "proxy.check",
        description: "Check public Lab and protected MCP proxy endpoints from caller-visible URLs. \
                       Probes: app health, protected-resource metadata, OAuth bearer challenge, \
                       wrong-path 404 behavior, and (when backend_url is provided) backend-leak \
                       redaction.",
        destructive: false,
        requires_admin: false,
        returns: "DoctorReport",
        params: &[
            ParamSpec {
                name: "app_url",
                ty: "string",
                required: true,
                description: "Public Lab app URL, e.g. https://lab.example.com",
            },
            ParamSpec {
                name: "mcp_url",
                ty: "string",
                required: true,
                description: "Public MCP gateway URL, e.g. https://mcp.example.com",
            },
            ParamSpec {
                name: "route",
                ty: "string",
                required: true,
                description: "Protected MCP public route path, e.g. /syslog",
            },
            ParamSpec {
                name: "backend_url",
                ty: "string",
                required: false,
                description: "Optional private backend origin, e.g. http://mcp-backend:3100. \
                               When provided, enables the backend-leak redaction probe.",
            },
        ],
    },
];
