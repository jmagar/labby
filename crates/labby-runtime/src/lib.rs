#![forbid(unsafe_code)]

//! Surface-neutral contracts, DTOs, and helpers shared across the Labby gateway
//! extraction crates (`labby-codemode`, `labby-gateway`, `labby`).
//!
//! It owns serialization-stable gateway configuration DTOs plus generic helper
//! contracts used by more than one extracted crate. It must not depend on
//! product/transport layers (`axum`, `clap`, `rmcp`, `javy`, `wasmtime`,
//! `utoipa`) or on Labby product registry builders.
//!
//! `dispatch_helpers` and the stdio-spawn/SSRF security guards moved to
//! `labby-gateway` — they're gateway-only concerns, and keeping them here
//! would pull the dependency-free `labby-primitives` types they use into
//! `labby-auth` and `labby-codemode`, which never touch them.

pub mod agent_error;
pub mod agent_runtime;
/// Surface-neutral Artifact domain and local runtime implementation.
pub mod artifacts;
/// Typed, versioned authority epochs and short-lived execution leases.
pub mod authority;
/// Shared retry backoff and deterministic jitter helpers.
pub mod backoff;
pub mod caller_auth;
pub mod catalog_notify;
pub mod client_registry;
/// Shared runtime state for the optional Code Mode MCP App surface.
pub mod code_mode_app;
/// Dev Container admission and lifecycle validation without host execution.
pub mod dev_container;
/// Pluggable Dev Container execution and restart recovery.
pub mod dev_container_runtime;
pub mod error;
pub mod gateway_authority;
pub mod gateway_config;
pub mod helpers;
pub mod path_safety;
/// Versioned Phabby/Depot delivery wire contracts and fail-closed validation.
pub mod phabby_delivery;
pub mod redact;
pub mod response_body;
pub mod secure_atomic_file;
pub mod skills;
pub mod task_runtime;

pub use helpers::{env_non_empty, home_dir, lab_home};

/// Code Mode runtime configuration, re-exported at the crate root so consumers
/// that must stay free of host/transport vocabulary can name it without the
/// module path.
pub use code_mode_app::CodeModeAppState;
pub use gateway_config::{CodeModeConfig, CodeModeResultShapePolicy};
