//! Path resolution helpers + cached registry views for the `setup`
//! dispatch service.
//!
//! Honors `LABBY_HOME` for tests; defaults to `~/.labby/` in production. The
//! registry-derived caches live here so dispatch and secret_mask don't
//! rebuild them on every call.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::OnceLock;

use labby_apis::core::EnvVar;

use crate::registry::{ToolRegistry, build_default_registry, service_meta};

/// Re-exported from `dispatch::helpers` (the canonical home for this leaf path
/// helper) so the `env_path`/`draft_path` callers below and `plugin_hook`'s
/// `client::lab_home` import keep resolving.
pub use crate::dispatch::helpers::lab_home;

#[must_use]
pub fn env_path() -> PathBuf {
    lab_home().join(".env")
}

#[must_use]
pub fn draft_path() -> PathBuf {
    lab_home().join(".env.draft")
}

// ─── cached registry views ──────────────────────────────────────────────

static CACHED_REGISTRY: OnceLock<ToolRegistry> = OnceLock::new();
static CACHED_SECRET_KEYS: OnceLock<HashSet<&'static str>> = OnceLock::new();
static CACHED_ENV_VAR_INDEX: OnceLock<HashMap<&'static str, &'static EnvVar>> = OnceLock::new();

/// Returns the lazy-initialized default registry. Built once per process.
pub fn cached_registry() -> &'static ToolRegistry {
    CACHED_REGISTRY.get_or_init(build_default_registry)
}

/// Returns a `HashSet` of every env var name where the registered
/// `EnvVar.secret == true`. O(1) lookup replaces the per-call registry walk.
pub fn cached_secret_keys() -> &'static HashSet<&'static str> {
    CACHED_SECRET_KEYS.get_or_init(|| {
        let mut keys = HashSet::new();
        for entry in cached_registry().services() {
            if let Some(meta) = service_meta(entry.name) {
                for var in meta.required_env.iter().chain(meta.optional_env.iter()) {
                    if var.secret {
                        keys.insert(var.name);
                    }
                }
            }
        }
        keys
    })
}

/// Suffixes that mark a key as secret-by-naming-convention. Used by
/// [`super::secret_mask::is_secret_key`] to mask values for keys that are
/// not in the explicit registry — third-party env vars pasted into the
/// draft, or services compiled out via feature flags whose secret flag
/// would otherwise be lost.
pub const SECRET_SUFFIX_DEFAULT_MASK: &[&str] = &["_API_KEY", "_TOKEN", "_PASSWORD", "_SECRET"];

/// Returns `true` when `key` ends with any of [`SECRET_SUFFIX_DEFAULT_MASK`].
#[must_use]
pub fn key_matches_secret_suffix(key: &str) -> bool {
    SECRET_SUFFIX_DEFAULT_MASK
        .iter()
        .any(|suffix| key.ends_with(suffix))
}

/// Returns a `HashMap` from env var name to the registered `EnvVar` declaration.
/// O(1) lookup replaces the per-entry registry rebuild in
/// `validate_against_registry`.
pub fn cached_env_var_index() -> &'static HashMap<&'static str, &'static EnvVar> {
    CACHED_ENV_VAR_INDEX.get_or_init(|| {
        let mut idx = HashMap::new();
        for entry in cached_registry().services() {
            if let Some(meta) = service_meta(entry.name) {
                for var in meta.required_env.iter().chain(meta.optional_env.iter()) {
                    idx.insert(var.name, var);
                }
            }
        }
        idx
    })
}
