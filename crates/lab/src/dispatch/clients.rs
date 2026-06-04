use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;

use tokio::sync::RwLock;

/// Pre-built service clients, constructed once at startup from environment variables.
///
/// Fields are added here as services are onboarded. Each optional field is `None`
/// when the required env vars are absent. Surfaces extract the pre-built client to
/// avoid per-request env reads and `reqwest::Client` (connection pool) construction.
///
/// # Client-resolution patterns
///
/// Two supported patterns (see `dispatch/CLAUDE.md` for the canonical templates):
///
/// 1. **`AppState`-wired (preferred):** add an `Option<FooClient>` field here,
///    populate it in `from_env()` via `dispatch::foo::client::client_from_env()`.
///    API handlers receive the pre-built client from `AppState`.
///
/// 2. **`require_client()` fallback:** used by MCP/CLI dispatch when `AppState`
///    is not available (e.g. CLI invocations without a running server). Each
///    service's `dispatch/<service>/client.rs` exposes `require_client()` which
///    reads env vars on demand. This is the only permitted per-request env read.
///
/// Multi-instance services use `InstancePool<C>` (from `dispatch::helpers`) instead
/// of a bespoke `OnceLock`. `InstancePool::build(prefix, factory)` scans for
/// `{PREFIX}_URL` (default) and `{PREFIX}_{LABEL}_URL` (named) at first call.
///
/// Do NOT create per-service bespoke pools, per-method sub-dispatchers that
/// re-read env vars, or inline `std::env::var` calls outside `client.rs`.
#[derive(Clone, Default)]
pub struct ServiceClients {
    // [lab-scaffold: state-fields]
}

impl ServiceClients {
    /// Build all service clients from environment variables.
    ///
    /// Called once at startup. Returns `None` per field when env vars are missing.
    #[must_use]
    pub fn from_env() -> Self {
        Self {
            // [lab-scaffold: state-from-env]
        }
    }

    #[must_use]
    pub fn from_env_map(values: HashMap<String, String>) -> Self {
        crate::dispatch::helpers::with_env_override(values, Self::from_env)
    }
}

#[derive(Clone, Default)]
pub struct SharedServiceClients {
    inner: Arc<RwLock<ServiceClients>>,
    #[cfg(test)]
    refresh_count: Arc<std::sync::atomic::AtomicUsize>,
}

impl SharedServiceClients {
    #[must_use]
    pub fn from_clients(clients: ServiceClients) -> Self {
        Self {
            inner: Arc::new(RwLock::new(clients)),
            #[cfg(test)]
            refresh_count: Arc::new(std::sync::atomic::AtomicUsize::new(0)),
        }
    }

    #[must_use]
    #[allow(dead_code)]
    pub fn from_env() -> Self {
        Self::from_clients(ServiceClients::from_env())
    }

    #[allow(dead_code)]
    pub async fn snapshot(&self) -> ServiceClients {
        self.inner.read().await.clone()
    }

    pub async fn refresh_from_env_path(&self, path: &Path) -> anyhow::Result<()> {
        let values = dotenvy::from_path_iter(path)
            .ok()
            .map(|iter| iter.filter_map(Result::ok).collect())
            .unwrap_or_default();
        *self.inner.write().await = ServiceClients::from_env_map(values);
        #[cfg(test)]
        self.refresh_count
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        Ok(())
    }

    #[cfg(test)]
    pub fn refresh_count(&self) -> usize {
        self.refresh_count.load(std::sync::atomic::Ordering::SeqCst)
    }
}
