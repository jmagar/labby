//! Code Mode snippet ENGINE (store, parse, validate, resolve, render).
//!
//! This is the storage/resolution engine only. The snippet SURFACE (the MCP
//! tool, HTTP route, CLI command, and `ACTIONS` catalog) lives in the host
//! binary as a thin adapter over this module.

pub mod store;
