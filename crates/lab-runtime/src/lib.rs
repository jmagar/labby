#![forbid(unsafe_code)]

//! Surface-neutral runtime contracts and helpers shared across the Lab
//! gateway-extraction crates (`lab-codemode`, `lab-gateway`, `lab-gatewayd`).
//!
//! This crate holds contracts, DTOs, and pure helpers only. It must not depend
//! on transport/runtime layers (`axum`, `clap`, `rmcp`, `javy`, `wasmtime`,
//! `utoipa`) or on Labby product registry builders.

pub mod error;
pub mod gateway_config;
pub mod path_safety;
pub mod redact;

/// Code Mode runtime configuration, re-exported at the crate root so consumers
/// that must stay free of host/transport vocabulary can name it without the
/// module path.
pub use gateway_config::CodeModeConfig;
