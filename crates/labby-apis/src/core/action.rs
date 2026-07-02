//! Action discovery metadata.
//!
//! Re-exported from the dependency-free `labby-primitives` leaf crate so that
//! `labby-apis` and the gateway-extraction crates (`labby-gateway`, and
//! transitively `labby-auth` / `labby-codemode`) share the exact same
//! `ActionSpec`/`ParamSpec` types without depending on each other. See
//! `labby_primitives::action` for the type definitions and docs.

pub use labby_primitives::action::{ActionSpec, ParamSpec};
