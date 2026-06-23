//! MCP service adapter for the `stash` service.
//!
//! The stash service is always-on and registered in `crate::registry` directly
//! from the shared dispatch layer. This module exists as a declared module for
//! consistency with the `mcp/services/` surface layout. No extra dispatch
//! logic is needed here.
