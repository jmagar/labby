//! Surface-neutral error type for dispatch operations.
//!
//! `ToolError` is the single canonical error type across all three surfaces
//! (MCP, API, CLI). It now lives in the `lab-runtime` crate so standalone
//! product slices can share it; it is re-exported here so existing
//! `crate::dispatch::error::ToolError` import paths keep working.
//!
//! The orphan rule forces `From<ServiceError>` conversions to live next to the
//! `ToolError` definition. Conversions whose source type is in `lab-apis` live
//! in `lab_runtime::error`; conversions whose source type is local to this
//! crate (`RegistryStoreError`) stay here.

pub use lab_runtime::error::ToolError;

// RegistryStore errors mostly represent persistence failures. Invalid cursors
// remain caller-fixable `invalid_param`, and upstream fetch failures surface as
// `network_error`. `RegistryStoreError` is local to the `lab` crate, so this
// `From` impl is legal here (and only here).
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
