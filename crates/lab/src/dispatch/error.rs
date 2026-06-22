//! Surface-neutral error type for dispatch operations.
//!
//! `ToolError` is the single canonical error type across all three surfaces
//! (MCP, API, CLI). It now lives in `lab_runtime::error` so the
//! gateway-extraction crates can share it; this module re-exports it for the
//! existing `crate::dispatch::error::ToolError` import path.
//!
//! Most `From<ServiceError> for ToolError` impls moved into `lab-runtime`
//! alongside the type. The one impl that remains here is the conversion from
//! `RegistryStoreError`, whose source type is local to the `lab` binary — the
//! orphan rule forbids implementing that conversion in `lab-runtime`.

pub use lab_runtime::error::ToolError;

// RegistryStore errors mostly represent persistence failures. Invalid cursors
// remain caller-fixable `invalid_param`, and upstream fetch failures surface as
// `network_error`. This impl stays in `lab` because `RegistryStoreError` is a
// local type (orphan rule); the foreign-source impls live in `lab-runtime`.
#[cfg(feature = "marketplace")]
impl From<crate::dispatch::marketplace::store::RegistryStoreError> for ToolError {
    fn from(e: crate::dispatch::marketplace::store::RegistryStoreError) -> Self {
        use crate::dispatch::marketplace::store::RegistryStoreError;
        let sdk_kind = match &e {
            RegistryStoreError::Upstream(_) => "network_error",
            RegistryStoreError::InvalidCursor(_) => "invalid_param",
            _ => "internal_error",
        };
        Self::Sdk {
            sdk_kind: sdk_kind.to_string(),
            message: format!("registry store: {e}"),
        }
    }
}
