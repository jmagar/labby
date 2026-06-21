//! Backward-compat re-export alias for `ts_signatures`.
//!
//! The live TypeScript signature / `.d.ts` generator lived here under the
//! name `types_legacy`; it was renamed to `ts_signatures` in Q-L2 to reflect
//! its actual role. This module delegates entirely to `ts_signatures` so
//! existing callers using the `types_legacy` path continue to compile.
//!
//! Do not add new code here — use `ts_signatures` directly.

// Backward-compat re-exports for any out-of-crate caller still naming the
// `types_legacy` path. Nothing in-crate consumes them — the redundant
// `tests_types_legacy` test file (identical to `tests_ts_signatures` modulo the
// module path) was removed, so this re-export is unused in this crate's own
// builds; keep the `allow` so the public shim does not trip `-D warnings`.
#[allow(unused_imports)]
pub use super::ts_signatures::{ToolTypes, generate_tool_types, json_schema_to_type};
