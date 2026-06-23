//! ACP registry resolution for dispatch.
//!
//! Follows the gateway manager pattern: `install_registry()` at startup,
//! `require_registry()` in dispatch. The registry is process-global shared
//! state — dispatch does NOT construct a new registry per call.

use std::sync::{Arc, OnceLock, RwLock};

use crate::acp::registry::AcpSessionRegistry;
use crate::dispatch::error::ToolError;

fn registry_slot() -> &'static RwLock<Option<Arc<AcpSessionRegistry>>> {
    static REGISTRY: OnceLock<RwLock<Option<Arc<AcpSessionRegistry>>>> = OnceLock::new();
    REGISTRY.get_or_init(|| RwLock::new(None))
}

/// Install the shared registry into the process-global slot.
///
/// Called once at startup (e.g. `cli/serve.rs`) with the same `Arc<AcpSessionRegistry>`
/// that is stored in `AppState`. Panics if called a second time; tests that need a fresh
/// registry should call `reset_registry_for_testing()` between runs.
pub fn install_registry(registry: Arc<AcpSessionRegistry>) {
    let mut slot = registry_slot().write().expect("ACP registry lock poisoned");
    assert!(slot.is_none(), "ACP registry installed twice");
    *slot = Some(registry);
}

/// Clear the process-global registry slot.
///
/// **For tests only.** Allows a test that called `install_registry` to reset state so
/// subsequent calls in other tests do not panic on the double-install assert.
#[cfg(test)]
#[allow(dead_code)]
pub fn reset_registry_for_testing() {
    *registry_slot().write().expect("ACP registry lock poisoned") = None;
}

/// Return the installed registry, or a structured error if not yet installed.
pub fn require_registry() -> Result<Arc<AcpSessionRegistry>, ToolError> {
    registry_slot()
        .read()
        .expect("ACP registry lock poisoned")
        .clone()
        .ok_or_else(not_configured_error)
}

pub fn not_configured_error() -> ToolError {
    ToolError::Sdk {
        sdk_kind: "internal_error".to_string(),
        message: "ACP registry not wired — start the server with ACP support".to_string(),
    }
}
