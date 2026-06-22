//! Single-flight refresh coordination for upstream OAuth clients.
//!
//! `RefreshLocks` prevents concurrent callers for the same `(upstream, subject)` pair
//! from issuing simultaneous token refresh requests against the authorization server.
//! One caller wins the lock and executes `get_access_token()` (which internally handles
//! proactive refresh); all others wait and then return the already-refreshed token.
//!
//! **Scope:** This module handles *proactive* refresh triggered before making an MCP call.
//! Reactive 401-retry logic is wired in Task 4 (`dispatch/gateway/`).
//!
//! ## rmcp refresh semantics
//!
//! `AuthorizationManager::get_access_token()` refreshes the token when fewer than 30 s
//! remain before expiry.  It does **not** react to 401 responses from the resource server.
//! A 401 with a locally-still-valid token requires an explicit `refresh_token()` call
//! followed by a retry — that is the Task 4 responsibility.

use std::sync::Arc;

use dashmap::DashMap;
use tokio::sync::Mutex;

/// Per-`(upstream_name, subject)` mutex pool.
///
/// Entries are created lazily on first access and are never removed (the number of
/// distinct `(upstream, subject)` pairs is bounded by the number of configured upstreams
/// times the number of users, which is small in a homelab context).
#[derive(Default)]
pub struct RefreshLocks(DashMap<(String, String), Arc<Mutex<()>>>);

impl RefreshLocks {
    pub fn new() -> Self {
        Self(DashMap::new())
    }

    /// Return the mutex for `(upstream_name, subject)`, creating it if absent.
    pub fn acquire(&self, upstream_name: &str, subject: &str) -> Arc<Mutex<()>> {
        let key = (upstream_name.to_string(), subject.to_string());
        self.0
            .entry(key)
            .or_insert_with(|| Arc::new(Mutex::new(())))
            .clone()
    }
}
