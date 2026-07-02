//! Stash root resolution for dispatch.
//!
//! Stash stores component directories under a root path derived as follows:
//! 1. Read `[workspace].root` from `config.toml` (CWD → `~/.labby/` → `~/.config/labby/`).
//! 2. Append `"stash"` to get the stash root.
//! 3. Fall back to `~/.labby/stash` if no workspace root is configured.
//!
//! The resolved root is cached in a process-global `OnceLock` — resolution
//! happens on first call and is never re-read from disk.

use std::path::PathBuf;
use std::sync::OnceLock;

use crate::dispatch::error::ToolError;

/// Structured error for callers when the stash root cannot be resolved.
pub fn not_configured_error() -> ToolError {
    ToolError::Sdk {
        sdk_kind: "workspace_not_configured".to_string(),
        message: "stash root could not be resolved — set [workspace].root in config.toml or ensure ~/.labby/stash is writable".to_string(),
    }
}

/// Resolve the stash root directory without caching.
///
/// Exposed for startup-time pre-warming (`cli::serve`). Use
/// `require_stash_root()` in dispatch paths.
pub fn resolve_stash_root() -> Option<PathBuf> {
    // Prefer workspace.root from config.toml + "stash" subdirectory.
    if let Ok(cfg) = crate::config::load_toml(&crate::config::toml_candidates()) {
        if let Ok(root) = crate::config::workspace_root_path(&cfg) {
            return Some(root.join("stash"));
        }
    }
    // Fall back to ~/.labby/stash using HOME env var.
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .map(|h| h.join(".labby").join("stash"))
}

static STASH_ROOT: OnceLock<Option<PathBuf>> = OnceLock::new();

#[cfg(test)]
static TEST_STASH_ROOT_OVERRIDE: std::sync::Mutex<Option<PathBuf>> = std::sync::Mutex::new(None);
#[cfg(test)]
static TEST_STASH_ROOT_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// Return the cached stash root path, or a structured `workspace_not_configured`
/// error if the root cannot be determined.
///
/// The first call resolves and caches; subsequent calls return the cached value.
pub fn require_stash_root() -> Result<&'static PathBuf, ToolError> {
    #[cfg(test)]
    if let Some(path) = TEST_STASH_ROOT_OVERRIDE.lock().unwrap().clone() {
        return Ok(Box::leak(Box::new(path)));
    }

    let cached = STASH_ROOT.get_or_init(resolve_stash_root);
    cached.as_ref().ok_or_else(not_configured_error)
}

#[cfg(test)]
pub fn with_test_stash_root<T>(root: PathBuf, run: impl FnOnce() -> T) -> T {
    let _guard = TEST_STASH_ROOT_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let previous = {
        let mut slot = TEST_STASH_ROOT_OVERRIDE
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        (*slot).replace(root)
    };

    struct RestoreGuard(Option<PathBuf>);
    impl Drop for RestoreGuard {
        fn drop(&mut self) {
            *TEST_STASH_ROOT_OVERRIDE
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner) = self.0.take();
        }
    }
    let _restore = RestoreGuard(previous);

    run()
}
