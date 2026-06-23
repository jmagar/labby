//! Action catalog for the `acp` (Agent Client Protocol) service.
//!
//! Single authoritative source for MCP, CLI, and API adapters.
//! `session.cancel`, `session.close`, and `session.bulk_close` are marked destructive.

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
        name: "provider.list",
        description: "List available providers with health status",
        destructive: false,
        requires_admin: false,
        returns: "Value",
        params: &[],
    },
    ActionSpec {
        name: "provider.get",
        description: "Get one provider's health and capabilities",
        destructive: false,
        requires_admin: false,
        returns: "Value",
        params: &[ParamSpec {
            name: "provider",
            ty: "string",
            required: true,
            description: "Provider name (e.g. 'codex')",
        }],
    },
    ActionSpec {
        name: "provider.select",
        description: "Validate a provider name (note: does not persist a default — planned feature)",
        destructive: false,
        requires_admin: false,
        returns: "Value",
        params: &[ParamSpec {
            name: "provider",
            ty: "string",
            required: true,
            description: "Provider name to set as default",
        }],
    },
    ActionSpec {
        name: "session.list",
        description: "List all sessions owned by the caller",
        destructive: false,
        requires_admin: false,
        returns: "Value",
        params: &[ParamSpec {
            name: "principal",
            ty: "string",
            required: false,
            description: "Filter sessions by principal (defaults to caller identity)",
        }],
    },
    ActionSpec {
        name: "session.get",
        description: "Get one session's summary and state",
        destructive: false,
        requires_admin: false,
        returns: "Value",
        params: &[ParamSpec {
            name: "session_id",
            ty: "string",
            required: true,
            description: "Session ID to retrieve",
        }],
    },
    ActionSpec {
        name: "session.start",
        description: "Create and start a new agent session",
        destructive: false,
        requires_admin: false,
        returns: "Value",
        params: &[
            ParamSpec {
                name: "provider",
                ty: "string",
                required: false,
                description: "Provider to use (default: 'codex')",
            },
            ParamSpec {
                name: "title",
                ty: "string",
                required: false,
                description: "Human-readable session title",
            },
            ParamSpec {
                name: "cwd",
                ty: "string",
                required: false,
                description: "Working directory for the session",
            },
            ParamSpec {
                name: "principal",
                ty: "string",
                required: false,
                description: "Caller principal (defaults to empty = anonymous)",
            },
        ],
    },
    ActionSpec {
        name: "session.start_and_prompt",
        description: "Atomically create an ACP session and queue its first prompt. Returns session metadata + SSE stream ticket. Closes the orphan-session window of separate create+prompt calls.",
        destructive: false,
        requires_admin: false,
        returns: "Value",
        params: &[
            ParamSpec {
                name: "provider",
                ty: "string",
                required: false,
                description: "Provider id (defaults to gateway default)",
            },
            ParamSpec {
                name: "model",
                ty: "string",
                required: false,
                description: "Model id; provider's default if omitted",
            },
            ParamSpec {
                name: "title",
                ty: "string",
                required: false,
                description: "Human-readable session title",
            },
            ParamSpec {
                name: "cwd",
                ty: "string",
                required: false,
                description: "Working directory for the session",
            },
            ParamSpec {
                name: "prompt",
                ty: "string",
                required: true,
                description: "First user prompt text",
            },
            ParamSpec {
                name: "page_context",
                ty: "object",
                required: false,
                description: "Optional page context: {route, entityType?, entityId?}",
            },
            ParamSpec {
                name: "principal",
                ty: "string",
                required: true,
                description: "Caller principal for ownership of the new session",
            },
        ],
    },
    ActionSpec {
        name: "session.prompt",
        description: "Send a prompt to a session. Optional provider switches the active runtime inside the same Lab session before dispatch.",
        destructive: false,
        requires_admin: false,
        returns: "Value",
        params: &[
            ParamSpec {
                name: "session_id",
                ty: "string",
                required: true,
                description: "Target session ID",
            },
            ParamSpec {
                name: "text",
                ty: "string",
                required: true,
                description: "Prompt text to send",
            },
            ParamSpec {
                name: "principal",
                ty: "string",
                required: true,
                description: "Caller principal for ownership verification",
            },
            ParamSpec {
                name: "provider",
                ty: "string",
                required: false,
                description: "Provider to use for this prompt; if different from the current session provider, Lab switches runtime before dispatch",
            },
            ParamSpec {
                name: "continuity_mode",
                ty: "string",
                required: false,
                description: "Provider switch continuity mode: 'handoff' (bounded transcript) or 'reset'",
            },
            ParamSpec {
                name: "page_context",
                ty: "object",
                required: false,
                description: "Optional page context: {route, entityType?, entityId?} — prepends a compact context prefix to the prompt",
            },
        ],
    },
    ActionSpec {
        name: "session.cancel",
        description: "Cancel a running session [destructive]",
        destructive: true,
        requires_admin: false,
        returns: "Value",
        params: &[
            ParamSpec {
                name: "session_id",
                ty: "string",
                required: true,
                description: "Session ID to cancel",
            },
            ParamSpec {
                name: "principal",
                ty: "string",
                required: false,
                description: "Caller principal for ownership verification",
            },
        ],
    },
    ActionSpec {
        name: "session.permission.approve",
        description: "Approve a pending provider permission request [destructive]",
        destructive: true,
        requires_admin: false,
        returns: "Value",
        params: &[
            ParamSpec {
                name: "session_id",
                ty: "string",
                required: true,
                description: "Session ID that owns the permission request",
            },
            ParamSpec {
                name: "request_id",
                ty: "string",
                required: true,
                description: "Pending permission request ID from the permission_request event",
            },
            ParamSpec {
                name: "option_id",
                ty: "string",
                required: true,
                description: "Allow option ID to select for this request",
            },
            ParamSpec {
                name: "principal",
                ty: "string",
                required: false,
                description: "Caller principal for ownership verification",
            },
            ParamSpec {
                name: "confirm",
                ty: "boolean",
                required: true,
                description: "Must be true because approval grants provider access",
            },
        ],
    },
    ActionSpec {
        name: "session.permission.reject",
        description: "Reject a pending provider permission request",
        destructive: false,
        requires_admin: false,
        returns: "Value",
        params: &[
            ParamSpec {
                name: "session_id",
                ty: "string",
                required: true,
                description: "Session ID that owns the permission request",
            },
            ParamSpec {
                name: "request_id",
                ty: "string",
                required: true,
                description: "Pending permission request ID from the permission_request event",
            },
            ParamSpec {
                name: "principal",
                ty: "string",
                required: false,
                description: "Caller principal for ownership verification",
            },
        ],
    },
    ActionSpec {
        name: "session.close",
        description: "Close a session permanently [destructive]",
        destructive: true,
        requires_admin: false,
        returns: "Value",
        params: &[
            ParamSpec {
                name: "session_id",
                ty: "string",
                required: true,
                description: "Session ID to close",
            },
            ParamSpec {
                name: "principal",
                ty: "string",
                required: false,
                description: "Caller principal for ownership verification",
            },
        ],
    },
    ActionSpec {
        name: "session.bulk_close",
        description: "Bulk close sessions matching a typed selector. Self-service only — \
                      only the caller's own sessions are touched. [destructive]",
        destructive: true,
        requires_admin: false,
        returns: r#"{ "closed": string[], "failed": [{ "id": string, "kind": string, "message": string }] }"#,
        params: &[
            ParamSpec {
                name: "selector",
                ty: "object",
                required: true,
                description: "BulkCloseSelector { states?: AcpSessionState[], max_age_days?: number, max_count?: number (default 500) }",
            },
            ParamSpec {
                name: "principal",
                ty: "string",
                required: true,
                description: "Caller principal; only the caller's sessions are touched",
            },
        ],
    },
    ActionSpec {
        name: "session.events",
        description: "Get stored events for a session. ProviderInfo events of type \
                     'tool_call_metadata' carry an optional '_meta' object relayed transparently \
                     from the originating agent; the key is absent (not null) when the agent did \
                     not inject it. ToolCallUpdate events carry merged '_meta' (outer wrapper \
                     wins over any '_meta' already present in raw_output).",
        destructive: false,
        requires_admin: false,
        returns: r#"{ "events": AcpEvent[], "count": number }"#,
        params: &[
            ParamSpec {
                name: "session_id",
                ty: "string",
                required: true,
                description: "Session ID to fetch events for",
            },
            ParamSpec {
                name: "since",
                ty: "integer",
                required: false,
                description: "Return events after this sequence number (default 0)",
            },
        ],
    },
    ActionSpec {
        name: "session.subscribe_ticket",
        description: "Issue a short-lived SSE auth ticket for browser EventSource clients",
        destructive: false,
        requires_admin: false,
        returns: "Value",
        params: &[
            ParamSpec {
                name: "session_id",
                ty: "string",
                required: true,
                description: "Session ID to subscribe to",
            },
            ParamSpec {
                name: "principal",
                ty: "string",
                required: false,
                description: "Caller principal for ownership verification",
            },
        ],
    },
];
