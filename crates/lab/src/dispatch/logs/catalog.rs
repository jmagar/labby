//! Action catalog for the local-master `logs` service.
//!
//! Single authoritative source for MCP, CLI, and API adapters. All actions
//! are read-only — `destructive: false` everywhere.

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
    ActionSpec {
        name: "logs.search",
        description: "Search persisted log events with filters",
        destructive: false,
        requires_admin: false,
        returns: "Value",
        params: &[ParamSpec {
            name: "query",
            ty: "json",
            required: false,
            description: "LogQuery filter object (text, levels, subsystems, surfaces, ts range, …)",
        }],
    },
    ActionSpec {
        name: "logs.tail",
        description: "Bounded follow-up read from the persisted store",
        destructive: false,
        requires_admin: false,
        returns: "Value",
        params: &[
            ParamSpec {
                name: "after_ts",
                ty: "integer",
                required: false,
                description: "Return events strictly after this ms-since-epoch timestamp",
            },
            ParamSpec {
                name: "since_event_id",
                ty: "string",
                required: false,
                description: "Return events strictly after this event_id cursor",
            },
            ParamSpec {
                name: "limit",
                ty: "integer",
                required: false,
                description: "Max events to return (default 500, max 10000)",
            },
        ],
    },
    ActionSpec {
        name: "logs.stats",
        description: "Return retention metadata and drop counters",
        destructive: false,
        requires_admin: false,
        returns: "Value",
        params: &[],
    },
    ActionSpec {
        name: "logs.metrics",
        description: "Aggregate usage metrics (tool calls, tokens, latency, surfaces, fan-out, …) over a rolling window",
        destructive: false,
        requires_admin: false,
        returns: "Value",
        params: &[ParamSpec {
            name: "window",
            ty: "string",
            required: false,
            description: "Rolling window: 1h, 24h (default), or 7d",
        }],
    },
    ActionSpec {
        name: "logs.stream",
        description: "Live push is HTTP SSE only; dispatch returns guidance",
        destructive: false,
        requires_admin: false,
        returns: "Value",
        params: &[],
    },
];
