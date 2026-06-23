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
        // Distinguish "file absent" from "file present but unparseable".
        // Collapsing both into an empty-map rebuild would silently tear down
        // every configured client on a hot-reload triggered by a malformed
        // `.env`, with no signal to the operator.
        let iter = match dotenvy::from_path_iter(path) {
            Ok(iter) => iter,
            // File absent (or otherwise unopenable) — preserve the prior
            // behavior of rebuilding from the ambient environment only.
            Err(_) => {
                *self.inner.write().await = ServiceClients::from_env_map(HashMap::new());
                #[cfg(test)]
                self.refresh_count
                    .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                return Ok(());
            }
        };

        // The file opened: a per-line error means it parsed partway then hit a
        // malformed entry. Do NOT wipe the live clients in that case — warn and
        // keep prior state so a typo can't take the whole fleet offline.
        let mut values = HashMap::new();
        for entry in iter {
            match entry {
                Ok((key, value)) => {
                    values.insert(key, value);
                }
                Err(error) => {
                    tracing::warn!(
                        path = %path.display(),
                        error = %error,
                        "malformed .env on hot-reload; keeping existing service clients"
                    );
                    return Ok(());
                }
            }
        }

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

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[tokio::test]
    async fn refresh_rebuilds_on_absent_file() {
        let shared = SharedServiceClients::from_clients(ServiceClients::default());
        let dir = tempfile::tempdir().unwrap();
        let missing = dir.path().join("absent-does-not-exist.env");
        shared.refresh_from_env_path(&missing).await.unwrap();
        // File-absent keeps the prior rebuild-from-ambient behavior: the swap runs.
        assert_eq!(shared.refresh_count(), 1);
    }

    #[tokio::test]
    async fn refresh_rebuilds_on_valid_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("good.env");
        std::fs::write(&path, b"FOO=bar\nBAZ=qux\n").unwrap();
        let shared = SharedServiceClients::from_clients(ServiceClients::default());
        shared.refresh_from_env_path(&path).await.unwrap();
        assert_eq!(shared.refresh_count(), 1);
    }

    #[tokio::test]
    async fn refresh_preserves_state_on_malformed_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("bad.env");
        // A line with no `=` and an illegal key is a dotenvy parse error, which
        // surfaces as a per-line `Err` from `from_path_iter`.
        let mut f = std::fs::File::create(&path).unwrap();
        f.write_all(b"VALID=ok\n!!! this is not valid env syntax !!!\n")
            .unwrap();
        f.flush().unwrap();

        let shared = SharedServiceClients::from_clients(ServiceClients::default());
        shared.refresh_from_env_path(&path).await.unwrap();
        // Malformed `.env` must NOT trigger a teardown/rebuild — prior state kept.
        assert_eq!(shared.refresh_count(), 0);
    }
}
