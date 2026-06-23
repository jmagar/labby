//! Surface-neutral redaction helpers.
//!
//! These now live in the `lab-runtime` crate so the standalone gateway crates
//! can share them. They are re-exported here so existing
//! `crate::dispatch::redact::*` import paths keep working.

pub use labby_runtime::redact::*;
