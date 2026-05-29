//! Core Rust library for the `lab` MCP gateway.
//!
//! Provides cross-cutting primitives (HTTP client, auth, errors), gateway-adjacent
//! operator modules (marketplace, mcpregistry, device_runtime, deploy, doctor, setup,
//! stash, acp, acp_registry).

#![cfg_attr(docsrs, feature(doc_cfg))]

/// Cross-cutting primitives: HTTP client, auth, errors, status, action specs.
pub mod core;

/// Marketplace: browse and install Claude Code plugins.
pub mod marketplace;

/// Device-runtime control-plane client shared by CLI and runtime code.
pub mod device_runtime;

/// Agent Client Protocol (ACP) — types, error, persistence trait, and provider types.
pub mod acp;

/// Doctor — bootstrap health audit: env vars, system probes, service reachability.
pub mod doctor;

/// Setup — first-run + draft-commit configuration flow (Bootstrap orchestrator).
pub mod setup;

/// Stash — component versioning and deployment (skills, agents, configs, binaries).
pub mod stash;

/// Deploy service — push local release binary to SSH targets.
#[cfg(feature = "deploy")]
pub mod deploy;

/// MCP Registry client — browse and search the official MCP server registry.
#[cfg(feature = "mcpregistry")]
pub mod mcpregistry;

/// ACP Agent Registry client — discover and install ACP-compatible AI coding agents.
#[cfg(feature = "acp_registry")]
pub mod acp_registry;
