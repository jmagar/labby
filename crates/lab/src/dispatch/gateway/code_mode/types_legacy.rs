//! Backward-compat re-export alias for `ts_signatures`.
//!
//! The live TypeScript signature / `.d.ts` generator lived here under the
//! name `types_legacy`; it was renamed to `ts_signatures` in Q-L2 to reflect
//! its actual role. This module delegates entirely to `ts_signatures` so
//! existing callers using the `types_legacy` path continue to compile.
//!
//! Do not add new code here — use `ts_signatures` directly.

// These re-exports are used by `tests_types_legacy` (which calls into
// `super::types_legacy::*` to verify backward-compat paths). They are
// intentionally `pub` for that test surface and unused in the non-test build.
#[allow(unused_imports)]
pub use super::ts_signatures::{ToolTypes, generate_tool_types, json_schema_to_type};
