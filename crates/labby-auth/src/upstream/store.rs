//! SQLite-backed rmcp `CredentialStore` and `StateStore` adapters.
//!
//! Both adapters are per-`(upstream_name, subject)` — construct a fresh pair for each
//! upstream × subject combination.  The underlying `SqliteStore` is `Clone` so the
//! shared connection pool is cheap to duplicate.
//!
//! # `StateStore::load` is a consuming take
//!
//! `StateStore::load` calls `take_upstream_oauth_state` (DELETE … RETURNING) rather than
//! a plain SELECT.  This is intentional: consuming state on first read closes the replay
//! window.  `StateStore::delete` is therefore a no-op — the row is already gone after
//! a successful `load`.
//!
//! # Lifetime pattern
//!
//! `#[async_trait]` expands `async fn foo(&self)` to a two-lifetime form:
//! `fn foo<'life0, 'async_trait>(&'life0 self) where 'life0: 'async_trait, Self: 'async_trait`.
//! All method impls below match that exact pattern without importing the `async-trait` crate.

use std::future::Future;
use std::pin::Pin;

use oauth2::{CsrfToken, PkceCodeVerifier, TokenResponse as _};
use rmcp::transport::auth::{
    AuthError, CredentialStore, StateStore, StoredAuthorizationState, StoredCredentials,
};
use rmcp_client as rmcp;

use crate::sqlite::SqliteStore;
use crate::types::{UpstreamOauthCredentialRow, UpstreamOauthStateRow};
use crate::upstream::encryption::{self, EncryptionKey};

fn credential_aad(upstream_name: &str, subject: &str, client_id: &str) -> Vec<u8> {
    format!("upstream={upstream_name}\0subject={subject}\0client_id={client_id}").into_bytes()
}

fn now_unix() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64
}

/// Per-`(upstream_name, subject)` credential store backed by SQLite.
///
/// Tokens are encrypted at rest with ChaCha20-Poly1305.  Decryption failure
/// surfaces as `AuthError::AuthorizationRequired` so callers re-initiate the
/// authorization flow rather than hard-failing.
pub struct SqliteCredentialStore {
    store: SqliteStore,
    key: EncryptionKey,
    upstream_name: String,
    subject: String,
}

impl SqliteCredentialStore {
    pub fn new(
        store: SqliteStore,
        key: EncryptionKey,
        upstream_name: impl Into<String>,
        subject: impl Into<String>,
    ) -> Self {
        Self {
            store,
            key,
            upstream_name: upstream_name.into(),
            subject: subject.into(),
        }
    }
}

impl CredentialStore for SqliteCredentialStore {
    fn load<'life0, 'async_trait>(
        &'life0 self,
    ) -> Pin<
        Box<
            dyn Future<Output = Result<Option<StoredCredentials>, AuthError>> + Send + 'async_trait,
        >,
    >
    where
        'life0: 'async_trait,
        Self: 'async_trait,
    {
        Box::pin(async move {
            let row = self
                .store
                .find_upstream_oauth_credentials(&self.upstream_name, &self.subject)
                .await
                .map_err(|e| AuthError::InternalError(e.to_string()))?;

            let Some(row) = row else {
                return Ok(None);
            };

            let aad = credential_aad(&row.upstream_name, &row.subject, &row.client_id);
            let plaintext =
                encryption::open_with_aad(&self.key, &row.token_blob, &row.token_blob_nonce, &aad)
                    .map_err(|_| AuthError::AuthorizationRequired)?;

            let creds: StoredCredentials = serde_json::from_slice(&plaintext)
                .map_err(|e| AuthError::InternalError(format!("deserialize credentials: {e}")))?;

            Ok(Some(creds))
        })
    }

    fn save<'life0, 'async_trait>(
        &'life0 self,
        credentials: StoredCredentials,
    ) -> Pin<Box<dyn Future<Output = Result<(), AuthError>> + Send + 'async_trait>>
    where
        'life0: 'async_trait,
        Self: 'async_trait,
    {
        Box::pin(async move {
            let token_received_at = credentials
                .token_received_at
                .map(|t| t as i64)
                .unwrap_or_else(now_unix);

            let (access_token_expires_at, refresh_token_present) =
                if let Some(ref token) = credentials.token_response {
                    let expires_in = token.expires_in().map(|d| d.as_secs()).unwrap_or(3600) as i64;
                    (
                        token_received_at + expires_in,
                        token.refresh_token().is_some(),
                    )
                } else {
                    (0, false)
                };

            let granted_scopes_json = serde_json::to_string(&credentials.granted_scopes)
                .map_err(|e| AuthError::InternalError(format!("serialize scopes: {e}")))?;

            let plaintext = serde_json::to_vec(&credentials)
                .map_err(|e| AuthError::InternalError(format!("serialize credentials: {e}")))?;

            let aad = credential_aad(&self.upstream_name, &self.subject, &credentials.client_id);
            let (token_blob, token_blob_nonce) =
                encryption::seal_with_aad(&self.key, &plaintext, &aad)
                    .map_err(|e| AuthError::InternalError(format!("encrypt credentials: {e}")))?;

            let row = UpstreamOauthCredentialRow {
                upstream_name: self.upstream_name.clone(),
                subject: self.subject.clone(),
                client_id: credentials.client_id.clone(),
                granted_scopes_json,
                token_blob,
                token_blob_nonce,
                token_received_at,
                access_token_expires_at,
                refresh_token_present,
            };

            self.store
                .upsert_upstream_oauth_credentials(row)
                .await
                .map_err(|e| AuthError::InternalError(e.to_string()))
        })
    }

    fn clear<'life0, 'async_trait>(
        &'life0 self,
    ) -> Pin<Box<dyn Future<Output = Result<(), AuthError>> + Send + 'async_trait>>
    where
        'life0: 'async_trait,
        Self: 'async_trait,
    {
        Box::pin(async move {
            self.store
                .delete_upstream_oauth_credentials(&self.upstream_name, &self.subject)
                .await
                .map_err(|e| AuthError::InternalError(e.to_string()))
        })
    }
}

/// Per-`(upstream_name, subject)` state store backed by SQLite.
///
/// The `load` method uses `take_upstream_oauth_state` (atomic DELETE … RETURNING)
/// rather than a SELECT, consuming the row on first read.  `delete` is a no-op.
pub struct SqliteStateStore {
    store: SqliteStore,
    upstream_name: String,
    subject: String,
}

impl SqliteStateStore {
    pub fn new(
        store: SqliteStore,
        upstream_name: impl Into<String>,
        subject: impl Into<String>,
    ) -> Self {
        Self {
            store,
            upstream_name: upstream_name.into(),
            subject: subject.into(),
        }
    }
}

/// TTL for pending authorization state (5 minutes).
const STATE_TTL_SECS: i64 = 300;

impl StateStore for SqliteStateStore {
    fn save<'life0, 'life1, 'async_trait>(
        &'life0 self,
        csrf_token: &'life1 str,
        state: StoredAuthorizationState,
    ) -> Pin<Box<dyn Future<Output = Result<(), AuthError>> + Send + 'async_trait>>
    where
        'life0: 'async_trait,
        'life1: 'async_trait,
        Self: 'async_trait,
    {
        Box::pin(async move {
            let now = now_unix();
            let row = UpstreamOauthStateRow {
                upstream_name: self.upstream_name.clone(),
                subject: self.subject.clone(),
                csrf_token: csrf_token.to_string(),
                pkce_verifier: state.pkce_verifier,
                created_at: now,
                expires_at: now + STATE_TTL_SECS,
            };
            self.store
                .save_upstream_oauth_state(row)
                .await
                .map_err(|e| AuthError::InternalError(e.to_string()))
        })
    }

    fn load<'life0, 'life1, 'async_trait>(
        &'life0 self,
        csrf_token: &'life1 str,
    ) -> Pin<
        Box<
            dyn Future<Output = Result<Option<StoredAuthorizationState>, AuthError>>
                + Send
                + 'async_trait,
        >,
    >
    where
        'life0: 'async_trait,
        'life1: 'async_trait,
        Self: 'async_trait,
    {
        Box::pin(async move {
            let now = now_unix();
            let row = self
                .store
                .take_upstream_oauth_state(&self.upstream_name, &self.subject, csrf_token, now)
                .await
                .map_err(|e| AuthError::InternalError(e.to_string()))?;

            Ok(row.map(|r| {
                StoredAuthorizationState::new(
                    &PkceCodeVerifier::new(r.pkce_verifier),
                    &CsrfToken::new(r.csrf_token),
                )
            }))
        })
    }

    fn delete<'life0, 'life1, 'async_trait>(
        &'life0 self,
        _csrf_token: &'life1 str,
    ) -> Pin<Box<dyn Future<Output = Result<(), AuthError>> + Send + 'async_trait>>
    where
        'life0: 'async_trait,
        'life1: 'async_trait,
        Self: 'async_trait,
    {
        // `load` already performs an atomic DELETE … RETURNING; a separate
        // delete call would be a double-delete with no effect.
        Box::pin(async move { Ok(()) })
    }
}

// ── T10: OAuth callback/state error-path tests ────────────────────────────────
//
// Verify that the `SqliteStateStore` adapter correctly rejects unknown,
// replayed (already-consumed), and expired state tokens.  These paths were
// previously un-asserted at the adapter layer; the underlying `SqliteStore`
// primitive is tested in `lab-auth/src/sqlite.rs` but those tests exercise the
// raw SQL, not the rmcp `StateStore` adapter behaviour.

#[cfg(test)]
mod tests {
    use oauth2::{CsrfToken, PkceCodeVerifier};
    use rmcp_client::transport::auth::{StateStore, StoredAuthorizationState};

    use super::SqliteStateStore;

    /// Open a disposable in-memory SQLite store for testing.
    async fn temp_store() -> crate::sqlite::SqliteStore {
        let path = tempfile::tempdir()
            .expect("tempdir")
            .keep()
            .join("upstream_oauth_test.db");
        crate::sqlite::SqliteStore::open(path)
            .await
            .expect("open test store")
    }

    fn make_state_store(
        store: crate::sqlite::SqliteStore,
        upstream: &str,
        subject: &str,
    ) -> SqliteStateStore {
        SqliteStateStore::new(store, upstream, subject)
    }

    fn sample_stored_state(csrf: &str) -> StoredAuthorizationState {
        StoredAuthorizationState::new(
            &PkceCodeVerifier::new("verifier-value".to_string()),
            &CsrfToken::new(csrf.to_string()),
        )
    }

    /// Loading a state token that was never saved returns `None`.
    #[tokio::test]
    async fn unknown_state_returns_none() {
        let store = make_state_store(temp_store().await, "acme", "alice");
        let result = store
            .load("nonexistent-csrf")
            .await
            .expect("load should not error");
        assert!(result.is_none(), "unknown state token must return None");
    }

    /// Loading a state token a second time returns `None` (replay prevention).
    ///
    /// `SqliteStateStore::load` uses an atomic DELETE … RETURNING so the first
    /// successful load consumes the row.  A subsequent call for the same token
    /// must return `None` rather than re-authorizing the flow.
    #[tokio::test]
    async fn replayed_state_is_rejected() {
        let sqlite = temp_store().await;
        let store = make_state_store(sqlite, "acme", "alice");
        let csrf = "csrf-replay-test";

        // Save state once.
        store
            .save(csrf, sample_stored_state(csrf))
            .await
            .expect("save should succeed");

        // First load consumes the row.
        let first = store.load(csrf).await.expect("first load should not error");
        assert!(first.is_some(), "first load must return the stored state");

        // Second load must return None — the row no longer exists.
        let second = store
            .load(csrf)
            .await
            .expect("second load should not error");
        assert!(
            second.is_none(),
            "replayed (already-consumed) state token must return None"
        );
    }

    /// Loading a state token whose TTL has expired returns `None`.
    ///
    /// The underlying `take_upstream_oauth_state` query filters by `expires_at`,
    /// so an expired row is treated the same as a missing row.  We simulate this
    /// by writing a row with `expires_at = now - 1` directly via the raw store.
    #[tokio::test]
    async fn expired_state_is_rejected() {
        use crate::types::UpstreamOauthStateRow;

        let sqlite = temp_store().await;

        // Insert an already-expired row directly (bypassing the adapter's TTL).
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs() as i64;
        let row = UpstreamOauthStateRow {
            upstream_name: "acme".to_string(),
            subject: "alice".to_string(),
            csrf_token: "csrf-expired".to_string(),
            pkce_verifier: "verifier".to_string(),
            created_at: now - 400,
            expires_at: now - 1, // already expired
        };
        sqlite
            .save_upstream_oauth_state(row)
            .await
            .expect("save expired row");

        // The adapter must refuse to return an expired state.
        let store = make_state_store(sqlite, "acme", "alice");
        let result = store
            .load("csrf-expired")
            .await
            .expect("load should not error on expired state");
        assert!(
            result.is_none(),
            "expired state token must return None, not the stored state"
        );
    }
}
