//! Agent Client Protocol (ACP) — shared types, error taxonomy, persistence
//! trait, and provider types for the ACP runtime.
//!
//! This module is always compiled in `labby-apis` (no cargo feature here).
//! The service surface in the `labby` binary, however, is gated behind the
//! `acp` cargo feature (a member of `all`). It owns the stable public
//! surface that both `lab-apis` consumers and the `lab` binary depend on:
//!
//! - [`types`] — session, agent, and message types (populated by lab-jwbg.2)
//! - [`error`] — `AcpError` (thiserror)
//! - [`persistence`] — `AcpPersistence` trait; implementation (SQLite) lives in `lab`
//! - [`session`] — `SessionHandle` and related provider types
//!
//! Stateful runtime (registry, SQLite persistence implementation, subprocess
//! launch) lives in `crates/lab/src/acp/`, not here.

/// ACP request/response and domain types.
pub mod types;

/// `AcpError` — typed errors (thiserror).
pub mod error;

/// `AcpPersistence` — storage trait (implementation lives in `lab`).
pub mod persistence;

/// `SessionHandle` and related provider types.
pub mod session;

use crate::core::plugin::{Category, EnvVar, PluginMeta};
use crate::core::plugin_ui::{SECRET_OPTIONAL_FIELD, TEXT_OPTIONAL_FIELD};

/// Compile-time metadata for the ACP service.
pub const META: PluginMeta = PluginMeta {
    name: "acp",
    display_name: "ACP",
    description: "Agent Client Protocol — session management and provider orchestration",
    category: Category::Ai,
    docs_url: "",
    required_env: &[],
    optional_env: &[
        EnvVar {
            name: "LAB_ACP_DB",
            description: "Path to ACP SQLite database",
            example: "~/.labby/acp.db",
            secret: false,
            ui: Some(&TEXT_OPTIONAL_FIELD),
        },
        EnvVar {
            name: "LAB_ACP_HMAC_SECRET",
            description: "HMAC key for permission outcome signing; auto-generated if absent",
            example: "",
            secret: true,
            ui: Some(&SECRET_OPTIONAL_FIELD),
        },
    ],
    default_port: None,
    supports_multi_instance: false,
};

// Convenience re-exports of the canonical public surface.
pub use error::{AcpError, PersistenceError};
pub use session::{SessionCommand, SessionError, SessionHandle};
pub use types::{
    AcpContentBlock, AcpEvent, AcpPermissionOption, AcpProviderHealth, AcpSessionState,
    AcpSessionSummary,
};
