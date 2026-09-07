#![forbid(unsafe_code)]

//! Leaf crate of shared vocabulary types with **no internal workspace
//! dependencies**.
//!
//! Both the `labby-apis` product SDK (service catalog metadata) and the
//! gateway-extraction crates (`labby-gateway`, and transitively `labby-auth`
//! / `labby-codemode` through `labby-runtime`) need to agree on the same
//! `ActionSpec`/`ParamSpec` types and the same static SSRF preflight checks,
//! without either side depending on the other. This crate is the shared
//! bottom of that graph: `labby-apis` re-exports from here for backward
//! compatibility, and `labby-gateway` depends on it directly.

pub mod access;
pub mod action;
pub mod agent;
pub mod authority_projection;
pub mod dev_container;
pub mod mcp;
pub mod plugin;
pub mod plugin_ui;
pub mod product_credential;
pub mod ssrf;
pub mod task;
