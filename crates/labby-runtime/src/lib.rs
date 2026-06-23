#![forbid(unsafe_code)]

//! Surface-neutral contracts, DTOs, and helpers shared across the Lab gateway
//! extraction crates (`labby-codemode`, `labby-gateway`, `labby`).
//!
//! It owns serialization-stable gateway configuration DTOs plus generic helper
//! contracts used by more than one extracted crate. It must not depend on
//! product/transport layers (`axum`, `clap`, `rmcp`, `javy`, `wasmtime`,
//! `utoipa`) or on Labby product registry builders.

pub mod dispatch_helpers;
pub mod error;
pub mod gateway_config;
pub mod helpers;
pub mod path_safety;
pub mod redact;
pub mod security;

pub use helpers::{env_non_empty, home_dir, lab_home};

/// Code Mode runtime configuration, re-exported at the crate root so consumers
/// that must stay free of host/transport vocabulary can name it without the
/// module path.
pub use gateway_config::{CodeModeConfig, CodeModeResultShapePolicy};
