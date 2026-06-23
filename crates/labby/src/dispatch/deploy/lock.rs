//! Per-host advisory lock registry.
//!
//! Concurrent deploy runs from the same process must not touch the same host
//! simultaneously. The registry keeps one `tokio::sync::Mutex` per alias;
//! acquires wait up to a caller-chosen timeout, after which they return
//! `DeployError::Conflict`.

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Instant;

use dashmap::DashMap;
use labby_apis::deploy::DeployError;
use tokio::sync::{Mutex, OwnedMutexGuard};

/// Advisory lock registry keyed by SSH alias.
#[derive(Default)]
pub struct HostLockRegistry {
    inner: DashMap<String, Arc<Mutex<()>>>,
}

impl HostLockRegistry {
    /// Acquire an advisory lock on `host`, waiting up to `timeout` before
    /// surfacing `Conflict`.
    ///
    /// Returns `Pin<Box<dyn Future + Send + 'static>>` so callers can await
    /// it inside `Box::pin(... + 'static)` contexts without HRTB failures
    /// (Rust issue #100013). All `&self` access is synchronous; only owned
    /// values are captured by the returned future.
    pub fn acquire(
        &self,
        host: &str,
        timeout: std::time::Duration,
    ) -> Pin<Box<dyn Future<Output = Result<OwnedMutexGuard<()>, DeployError>> + Send + 'static>>
    {
        let mutex = self
            .inner
            .entry(host.to_string())
            .or_insert_with(|| Arc::new(Mutex::new(())))
            .clone();
        let host = host.to_string();
        Box::pin(async move {
            let wait_started = Instant::now();
            tracing::debug!(
                surface = "dispatch", service = "deploy.lock", action = "lock.wait",
                host = %host, timeout_ms = timeout.as_millis(),
                "waiting for deploy host lock",
            );
            match tokio::time::timeout(timeout, mutex.lock_owned()).await {
                Ok(guard) => {
                    tracing::debug!(
                        surface = "dispatch", service = "deploy.lock", action = "lock.acquired",
                        host = %host,
                        actor = "operator",
                        outcome = "success",
                        entity_kind = "deploy_host",
                        entity_id = %host,
                        timeout_ms = timeout.as_millis(),
                        wait_ms = wait_started.elapsed().as_millis(),
                        "deploy host lock acquired",
                    );
                    Ok(guard)
                }
                Err(_) => {
                    tracing::warn!(
                        surface = "dispatch", service = "deploy.lock", action = "lock.conflict",
                        host = %host,
                        actor = "operator",
                        outcome = "timeout",
                        entity_kind = "deploy_host",
                        entity_id = %host,
                        timeout_ms = timeout.as_millis(),
                        wait_ms = wait_started.elapsed().as_millis(),
                        kind = "conflict",
                        "deploy host lock contention timeout — another deploy is in progress",
                    );
                    Err(DeployError::Conflict { host })
                }
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    #[tokio::test]
    async fn first_lock_on_host_succeeds() {
        let reg = HostLockRegistry::default();
        let _g = reg
            .acquire("mini1", std::time::Duration::from_millis(50))
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn concurrent_lock_on_same_host_returns_conflict() {
        let reg = Arc::new(HostLockRegistry::default());
        let reg2 = reg.clone();
        let _held = reg
            .acquire("mini1", std::time::Duration::from_millis(50))
            .await
            .unwrap();
        let err = reg2
            .acquire("mini1", std::time::Duration::from_millis(25))
            .await
            .unwrap_err();
        assert_eq!(err.kind(), "conflict");
    }

    #[tokio::test]
    async fn different_hosts_do_not_conflict() {
        let reg = Arc::new(HostLockRegistry::default());
        let _a = reg
            .acquire("mini1", std::time::Duration::from_millis(50))
            .await
            .unwrap();
        let _b = reg
            .acquire("mini2", std::time::Duration::from_millis(50))
            .await
            .unwrap();
    }
}
