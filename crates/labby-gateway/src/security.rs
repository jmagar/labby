//! Shared security preflight guards used by the gateway runtime.
//!
//! Owned here (not in `labby-runtime`) because both modules are gateway-only:
//! neither `labby-auth` nor `labby-codemode` ever call into them.

pub mod spawn_guard;
pub mod ssrf;
