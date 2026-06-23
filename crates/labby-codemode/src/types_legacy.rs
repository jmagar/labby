//! Test-only re-export alias for the old `types_legacy` module path.
//!
//! The live TypeScript signature / `.d.ts` generator lived here under the
//! name `types_legacy`; it was renamed to `ts_signatures` in Q-L2 to reflect
//! its actual role. This module delegates entirely to `ts_signatures` so the
//! legacy-path regression tests can keep proving equivalent behavior.
//!
//! Do not add new code here — use `ts_signatures` directly.

// These re-exports are used by `tests_types_legacy` (which calls into
// `super::types_legacy::*` to verify backward-compat paths).
#[allow(unused_imports)]
pub(super) use super::ts_signatures::{ToolTypes, generate_tool_types, json_schema_to_type};
