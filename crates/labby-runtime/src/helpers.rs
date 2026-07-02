//! Pure, dependency-free runtime helpers shared by the gateway-extraction
//! crates.
//!
//! These mirror the small leaf helpers in the `lab` binary's
//! `dispatch::helpers` module, but without the binary-only test override hooks
//! (`TEST_LABBY_HOME` / thread-local `ENV_OVERRIDE`). `lab-gateway` and friends
//! use these production-path versions; the `lab` binary keeps its own copies
//! with the test seams its unit tests rely on.

use std::path::PathBuf;

/// Resolve the lab home directory: `$LABBY_HOME` if set and non-empty, else
/// `$HOME/.labby/`.
///
/// Falls back to a relative `.lab` only when neither variable is set — callers
/// that store secrets/state should ensure `HOME` is present.
#[must_use]
pub fn lab_home() -> PathBuf {
    if let Ok(home) = std::env::var("LABBY_HOME")
        && !home.is_empty()
    {
        return PathBuf::from(home);
    }
    match std::env::var("HOME") {
        Ok(home) if !home.is_empty() => PathBuf::from(home).join(".labby"),
        _ => PathBuf::from(".labby"),
    }
}

/// The user's home directory (`$HOME`), or `None` when unset/empty.
#[must_use]
pub fn home_dir() -> Option<PathBuf> {
    match std::env::var("HOME") {
        Ok(home) if !home.is_empty() => Some(PathBuf::from(home)),
        _ => None,
    }
}

/// Read an environment variable, returning `None` if absent or empty.
#[must_use]
pub fn env_non_empty(name: &str) -> Option<String> {
    std::env::var(name).ok().filter(|v| !v.is_empty())
}
