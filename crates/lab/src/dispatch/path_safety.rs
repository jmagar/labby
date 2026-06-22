//! Surface-neutral path-safety helpers for dispatch modules that operate on the
//! local filesystem.
//!
//! These now live in the `lab-runtime` crate so the standalone gateway crates
//! can share them. They are re-exported here so existing
//! `crate::dispatch::path_safety::*` import paths keep working.

pub use lab_runtime::path_safety::*;
