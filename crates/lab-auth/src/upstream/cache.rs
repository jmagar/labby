//! Per-`(upstream, subject)` `AuthClient` cache.
//!
//! Each entry binds one MCP upstream and one lab subject to a single
//! `AuthClient<reqwest::Client>` so tokens are never shared between users.
//! Entries are built lazily on first use via the upstream's
//! [`UpstreamOauthManager`], cached by `(upstream_name, subject)`, and
//! invalidated when the upstream's OAuth registration changes (e.g.
//! `client_id` rotation) or when the upstream is removed from config at
//! reload time.
//!
//! The cache is injected into both `GatewayManager` (for lifecycle and
//! eviction during config reload) and `UpstreamPool` (for per-request
//! lookup from MCP handlers). Extracting it avoids a circular dependency:
//! the pool does not need a reference to the gateway and the gateway does
//! not need to know how the pool uses the clients.

use std::future::Future;
use std::sync::Arc;

use dashmap::DashMap;
use lab_runtime::gateway_config::{UpstreamConfig, UpstreamOauthRegistration};
use rmcp::transport::AuthClient;
use rmcp::transport::streamable_http_client::StreamableHttpClient;
use rmcp_client as rmcp;
use tokio::sync::Mutex;

use crate::upstream::manager::UpstreamOauthManager;
use crate::upstream::types::OauthError;

/// A cached `AuthClient` plus the OAuth-registration fingerprint it was
/// built from. When the current config's fingerprint differs, the entry
/// is evicted and rebuilt so a stale `client_id` never signs a request.
pub struct CachedAuthClient {
    pub client: Arc<AuthClient<reqwest::Client>>,
    fingerprint: String,
}

/// Per-`(upstream, subject)` `AuthClient` cache.
///
/// Cheap to clone (all state is behind `Arc`). Safe to share between the
/// gateway manager and the upstream pool.
#[derive(Clone)]
pub struct OauthClientCache {
    /// Cached clients keyed by `(upstream_name, subject)`.
    clients: Arc<DashMap<(String, String), Arc<CachedAuthClient>>>,
    /// Per-upstream OAuth managers, owned by the gateway manager and
    /// shared in by `Arc` so the cache can call `build_auth_client`.
    managers: Arc<DashMap<String, UpstreamOauthManager>>,
    /// Per-`(upstream, subject)` build lock so concurrent first-request
    /// tasks don't issue duplicate token exchanges against the AS.
    build_locks: Arc<DashMap<(String, String), Arc<Mutex<()>>>>,
}

impl OauthClientCache {
    /// Create a new cache backed by the gateway's OAuth manager map.
    #[must_use]
    pub fn new(managers: Arc<DashMap<String, UpstreamOauthManager>>) -> Self {
        Self {
            clients: Arc::new(DashMap::new()),
            managers,
            build_locks: Arc::new(DashMap::new()),
        }
    }

    /// Return a cached `AuthClient<reqwest::Client>` for `(upstream, subject)`,
    /// building one on first use.
    ///
    /// Kept for callers that need a shared `Arc<AuthClient<reqwest::Client>>`
    /// (e.g. status-check endpoints).  The MCP connection path uses
    /// `get_or_build_capped` instead so the `BodyCappedHttpClient` cap applies.
    ///
    /// If a cached entry exists but was built from a different OAuth
    /// registration than the current `config`, the entry is evicted and
    /// rebuilt so stale `client_id`s never sign requests.
    ///
    /// For `Dynamic` upstreams the fingerprint includes the stored
    /// `client_id` (fetched from SQLite via the upstream manager) so a
    /// re-registration cycle evicts the cached `AuthClient` (lab-77y5.13).
    ///
    /// Concurrent first-request callers for the same key are serialised
    /// by a per-key mutex so only one token exchange runs.
    #[allow(dead_code)]
    pub async fn get_or_build(
        &self,
        config: &UpstreamConfig,
        subject: &str,
    ) -> Result<Arc<AuthClient<reqwest::Client>>, OauthError> {
        // For Dynamic upstreams, include the stored client_id in the
        // fingerprint so a re-registration is detected (lab-77y5.13).
        let dynamic_client_id: Option<String> = if config
            .oauth
            .as_ref()
            .is_some_and(|o| matches!(o.registration, UpstreamOauthRegistration::Dynamic))
        {
            self.managers
                .get(&config.name)
                .map(|r| r.clone())
                .ok_or_else(|| {
                    OauthError::Internal(format!(
                        "no oauth manager registered for upstream '{}'",
                        config.name
                    ))
                })?
                .stored_dynamic_client_id(subject)
                .await?
        } else {
            None
        };

        self.get_or_insert_with(config, subject, dynamic_client_id.as_deref(), || async {
            let manager = self
                .managers
                .get(&config.name)
                .map(|r| r.clone())
                .ok_or_else(|| {
                    OauthError::Internal(format!(
                        "no oauth manager registered for upstream '{}'",
                        config.name
                    ))
                })?;
            let auth_client = manager.build_auth_client(subject).await?;
            Ok(Arc::new(auth_client))
        })
        .await
    }

    /// Build an `AuthClient<C>` wrapping the supplied HTTP client and return it
    /// WITHOUT caching it.
    ///
    /// This is the P-H4 entry point: callers that manage their own connection
    /// cache (e.g. `UpstreamPool::acquire_or_connect_subject` via
    /// `SubjectScopedConnection`) pass a pre-built `BodyCappedHttpClient` so
    /// the OAuth path gets the same streaming response-size cap as the
    /// non-OAuth path.  The caller caches the resulting `AuthClient` at the
    /// pool level, so there is no double-caching here.
    pub async fn get_or_build_capped<C>(
        &self,
        config: &UpstreamConfig,
        subject: &str,
        http_client: C,
    ) -> Result<AuthClient<C>, OauthError>
    where
        C: StreamableHttpClient + Clone,
    {
        let manager = self
            .managers
            .get(&config.name)
            .map(|r| r.clone())
            .ok_or_else(|| {
                OauthError::Internal(format!(
                    "no oauth manager registered for upstream '{}'",
                    config.name
                ))
            })?;
        manager.build_auth_client_with(subject, http_client).await
    }

    #[allow(dead_code)]
    async fn get_or_insert_with<F, Fut>(
        &self,
        config: &UpstreamConfig,
        subject: &str,
        // For `Dynamic` upstreams: the stored `client_id` to fold into the
        // fingerprint. `None` for non-dynamic upstreams.
        dynamic_client_id: Option<&str>,
        builder: F,
    ) -> Result<Arc<AuthClient<reqwest::Client>>, OauthError>
    where
        F: FnOnce() -> Fut,
        Fut: Future<Output = Result<Arc<AuthClient<reqwest::Client>>, OauthError>>,
    {
        let fingerprint = registration_fingerprint(config, dynamic_client_id)?;
        let key = (config.name.clone(), subject.to_string());

        if let Some(entry) = self.clients.get(&key)
            && entry.fingerprint == fingerprint
        {
            return Ok(Arc::clone(&entry.client));
        }

        let lock = self
            .build_locks
            .entry(key.clone())
            .or_insert_with(|| Arc::new(Mutex::new(())))
            .clone();
        let _guard = lock.lock().await;

        // Re-check after acquiring the lock: another caller may have built
        // the entry while we were waiting.
        if let Some(entry) = self.clients.get(&key)
            && entry.fingerprint == fingerprint
        {
            return Ok(Arc::clone(&entry.client));
        }

        let arc_client = builder().await?;

        self.clients.insert(
            key,
            Arc::new(CachedAuthClient {
                client: Arc::clone(&arc_client),
                fingerprint,
            }),
        );

        Ok(arc_client)
    }

    /// Evict the entry for a single `(upstream, subject)` pair.
    ///
    /// Used by API handlers when credentials are cleared or when a refresh
    /// fails terminally and the next request must reauthenticate.
    pub fn evict_subject(&self, upstream: &str, subject: &str) {
        let key = (upstream.to_string(), subject.to_string());
        self.clients.remove(&key);
        // build_locks is intentionally NOT evicted: it serializes concurrent
        // builders for the same (upstream, subject) key. Removing it creates a
        // race window where two concurrent callers both see no cached client,
        // both drop the lock guard, and then both start building in parallel.
    }

    /// Evict every entry for `upstream`.
    ///
    /// Used at config reload when an upstream is removed or its OAuth
    /// registration changes, and when the whole server shuts down the
    /// upstream's sessions.
    pub fn evict_upstream(&self, upstream: &str) {
        self.clients.retain(|(name, _), _| name != upstream);
        // build_locks intentionally preserved — see comment in evict_subject.
    }

    /// Evict every entry whose upstream is not in `known`.
    ///
    /// Used at config reload to drop cached clients for upstreams that no
    /// longer exist in config.
    pub fn evict_upstreams_not_in(&self, known: &std::collections::HashSet<&str>) {
        self.clients
            .retain(|(name, _), _| known.contains(name.as_str()));
    }

    /// Number of cached clients. Intended for tests and observability.
    #[allow(dead_code)]
    #[must_use]
    pub fn len(&self) -> usize {
        self.clients.len()
    }

    /// True when the cache holds no clients.
    #[allow(dead_code)]
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.clients.is_empty()
    }

    /// Insert a pre-built `AuthClient` directly into the cache.
    ///
    /// Test-only seam: available in `lab-auth`'s own tests and to downstream
    /// crates that enable the `test` feature (e.g. `lab`'s gateway manager
    /// tests). Not part of the production API.
    #[cfg(any(test, feature = "test"))]
    pub fn insert_for_tests(
        &self,
        upstream: &str,
        subject: &str,
        fingerprint: &str,
        client: Arc<AuthClient<reqwest::Client>>,
    ) {
        self.clients.insert(
            (upstream.to_string(), subject.to_string()),
            Arc::new(CachedAuthClient {
                client,
                fingerprint: fingerprint.to_string(),
            }),
        );
    }
}

/// Compute a stable fingerprint of the OAuth registration.
///
/// When the fingerprint changes, the cached `AuthClient` is discarded.
/// `Preregistered` changes when `client_id` rotates; `ClientMetadataDocument`
/// changes when its URL moves; `Dynamic` includes the stored per-subject
/// `client_id` (lab-77y5.13) so a re-registration cycle evicts the stale entry.
#[allow(dead_code)]
fn registration_fingerprint(
    config: &UpstreamConfig,
    dynamic_client_id: Option<&str>,
) -> Result<String, OauthError> {
    let oauth = config
        .oauth
        .as_ref()
        .ok_or_else(|| OauthError::Internal("upstream has no oauth config".to_string()))?;

    Ok(match &oauth.registration {
        UpstreamOauthRegistration::Preregistered { client_id, .. } => {
            format!("preregistered:{client_id}")
        }
        UpstreamOauthRegistration::ClientMetadataDocument { url } => {
            format!("client_metadata_document:{url}")
        }
        UpstreamOauthRegistration::Dynamic => {
            // Include the stored client_id so a re-registration evicts the
            // stale cached AuthClient (lab-77y5.13). Fall back to "none" when
            // no client_id has been persisted yet (first-time registration
            // in-flight) so the initial build is not blocked.
            format!("dynamic:{}", dynamic_client_id.unwrap_or("none"))
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use lab_runtime::gateway_config::{UpstreamOauthConfig, UpstreamOauthMode};
    use rmcp_client::transport::AuthorizationManager;
    use std::sync::atomic::{AtomicUsize, Ordering};

    fn cfg(name: &str, client_id: &str) -> UpstreamConfig {
        UpstreamConfig {
            enabled: true,
            name: name.to_string(),
            url: Some(format!("https://{name}.example/mcp")),
            command: None,
            args: vec![],
            bearer_token_env: None,
            env: std::collections::BTreeMap::new(),
            proxy_resources: false,
            proxy_prompts: false,
            expose_tools: None,
            expose_resources: None,
            expose_prompts: None,
            oauth: Some(UpstreamOauthConfig {
                mode: UpstreamOauthMode::AuthorizationCodePkce,
                registration: UpstreamOauthRegistration::Preregistered {
                    client_id: client_id.to_string(),
                    client_secret_env: None,
                },
                scopes: None,
                prefer_client_metadata_document: None,
            }),
            imported_from: None,
            priority: 1.0,
        }
    }

    #[test]
    fn fingerprint_differs_on_client_id_change() {
        let a = registration_fingerprint(&cfg("acme", "id-1"), None).unwrap();
        let b = registration_fingerprint(&cfg("acme", "id-2"), None).unwrap();
        assert_ne!(a, b);
    }

    #[test]
    fn fingerprint_stable_for_identical_config() {
        let a = registration_fingerprint(&cfg("acme", "id-1"), None).unwrap();
        let b = registration_fingerprint(&cfg("acme", "id-1"), None).unwrap();
        assert_eq!(a, b);
    }

    #[test]
    fn empty_cache_is_empty() {
        let cache = OauthClientCache::new(Arc::new(DashMap::new()));
        assert!(cache.is_empty());
        assert_eq!(cache.len(), 0);
    }

    async fn dummy_auth_client() -> Arc<AuthClient<reqwest::Client>> {
        let manager = AuthorizationManager::new("http://localhost")
            .await
            .expect("authorization manager");
        Arc::new(AuthClient::new(reqwest::Client::new(), manager))
    }

    #[tokio::test]
    async fn cache_atomic_first_request_no_double_build() {
        let cache = OauthClientCache::new(Arc::new(DashMap::new()));
        let config = cfg("acme", "id-1");
        let builds = Arc::new(AtomicUsize::new(0));

        let left = {
            let cache = cache.clone();
            let config = config.clone();
            let builds = Arc::clone(&builds);
            tokio::spawn(async move {
                cache
                    .get_or_insert_with(&config, "alice", None, || {
                        let builds = Arc::clone(&builds);
                        async move {
                            builds.fetch_add(1, Ordering::SeqCst);
                            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
                            Ok(dummy_auth_client().await)
                        }
                    })
                    .await
                    .expect("left client")
            })
        };
        let right = {
            let cache = cache.clone();
            let config = config.clone();
            let builds = Arc::clone(&builds);
            tokio::spawn(async move {
                cache
                    .get_or_insert_with(&config, "alice", None, || {
                        let builds = Arc::clone(&builds);
                        async move {
                            builds.fetch_add(1, Ordering::SeqCst);
                            Ok(dummy_auth_client().await)
                        }
                    })
                    .await
                    .expect("right client")
            })
        };

        let left = left.await.expect("join left");
        let right = right.await.expect("join right");

        assert_eq!(builds.load(Ordering::SeqCst), 1);
        assert!(Arc::ptr_eq(&left, &right));
    }

    #[tokio::test]
    async fn cache_refuses_stale_client_id_after_config_change() {
        let cache = OauthClientCache::new(Arc::new(DashMap::new()));
        let old = cfg("acme", "id-1");
        let new = cfg("acme", "id-2");
        let old_fingerprint = registration_fingerprint(&old, None).expect("old fingerprint");
        cache.insert_for_tests("acme", "alice", &old_fingerprint, dummy_auth_client().await);

        let rebuilt = Arc::new(AtomicUsize::new(0));
        let client = cache
            .get_or_insert_with(&new, "alice", None, || {
                let rebuilt = Arc::clone(&rebuilt);
                async move {
                    rebuilt.fetch_add(1, Ordering::SeqCst);
                    Ok(dummy_auth_client().await)
                }
            })
            .await
            .expect("rebuilt client");

        assert_eq!(rebuilt.load(Ordering::SeqCst), 1);
        assert_eq!(cache.len(), 1);
        let stored = cache
            .clients
            .get(&(String::from("acme"), String::from("alice")))
            .expect("stored client");
        assert_eq!(
            stored.fingerprint,
            registration_fingerprint(&new, None).unwrap()
        );
        assert!(Arc::ptr_eq(&stored.client, &client));
    }

    // End-to-end eviction tests live in the Task 4 Step 7 suite where a real
    // `UpstreamOauthManager` and credential store are set up; constructing an
    // `AuthClient` here requires an async network-touching call to
    // `AuthorizationManager::new`, which is inappropriate for a unit test.
}
