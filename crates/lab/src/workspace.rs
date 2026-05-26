//! Workspace product runtime seam.
//!
//! This module is the first extraction proof for product-style runtime
//! composition inside the existing `lab` crate. It composes the current
//! workspace filesystem adapters without moving them to an external crate yet.

#[cfg(feature = "fs")]
mod runtime;

#[cfg(feature = "fs")]
pub use runtime::{WorkspaceRuntime, WorkspaceRuntimeBuilder};
