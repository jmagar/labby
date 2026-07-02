//! `PluginMeta` — per-service compile-time constants.
//!
//! Re-exported from the dependency-free `labby-primitives` leaf crate so that
//! `labby-apis` and `labby-gateway` share the exact same `PluginMeta`/`EnvVar`
//! types without depending on each other. See `labby_primitives::plugin` for
//! the type definitions and docs.

pub use labby_primitives::plugin::{Category, EnvVar, PluginMeta};
