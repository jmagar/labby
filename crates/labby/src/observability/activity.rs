//! Helpers for activity-feed actor correlation.
//!
//! `actor_key` is intentionally derived once at the authenticated session
//! boundary and then cloned into later log/activity records. Do not compute it
//! inside tracing subscriber hot paths.

use std::{path::Path, sync::Arc};

use anyhow::{Context, Result, anyhow};
use chacha20poly1305::aead::{OsRng, rand_core::RngCore};
use hmac::{Hmac, KeyInit, Mac};
use sha2::Sha256;

use crate::config::{dotenv_path, write_env_pairs};
use crate::dispatch::helpers::env_non_empty;

pub const ACTOR_KEY_SECRET_ENV: &str = "LAB_ACTOR_KEY_SECRET";
const GENERATED_SECRET_BYTES: usize = 32;

type HmacSha256 = Hmac<Sha256>;

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct ActorKey(Arc<str>);

impl ActorKey {
    #[must_use]
    #[allow(dead_code)]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    #[must_use]
    pub fn into_arc(self) -> Arc<str> {
        self.0
    }
}

#[derive(Clone, Debug)]
pub struct ActorKeyDeriver {
    secret: Arc<[u8]>,
}

impl ActorKeyDeriver {
    /// Build a deriver from an already loaded secret.
    ///
    /// This constructor is useful for session binding and deterministic tests.
    pub fn from_secret(secret: impl AsRef<[u8]>) -> Result<Self> {
        let secret = secret.as_ref();
        if secret.is_empty() {
            return Err(anyhow!("{ACTOR_KEY_SECRET_ENV} must not be empty"));
        }
        Ok(Self {
            secret: Arc::from(secret.to_vec().into_boxed_slice()),
        })
    }

    /// Load `LAB_ACTOR_KEY_SECRET` from process env or `~/.lab/.env`.
    ///
    /// If absent, a per-installation secret is generated and appended to
    /// `~/.lab/.env`. The returned deriver should be cached by the caller.
    pub fn load_or_create() -> Result<Self> {
        let path = dotenv_path().ok_or_else(|| anyhow!("HOME env var not set"))?;
        Self::load_or_create_from_path(&path)
    }

    pub fn load_or_create_from_path(path: &Path) -> Result<Self> {
        if let Some(secret) = env_non_empty(ACTOR_KEY_SECRET_ENV) {
            return Self::from_secret(secret);
        }

        if let Some(secret) = read_secret_from_dotenv(path)? {
            return Self::from_secret(secret);
        }

        let secret = generate_secret();
        write_env_pairs(
            path,
            &[(ACTOR_KEY_SECRET_ENV.to_string(), secret.clone())],
            false,
        )
        .with_context(|| format!("write {ACTOR_KEY_SECRET_ENV} to {}", path.display()))?;
        Self::from_secret(secret)
    }

    /// Derive a stable, non-reversible actor key for a non-empty authenticated subject.
    #[must_use]
    pub fn derive_subject(&self, subject: &str) -> Option<ActorKey> {
        if subject.is_empty() {
            return None;
        }

        let mut mac =
            HmacSha256::new_from_slice(&self.secret).expect("HMAC-SHA256 accepts keys of any size");
        mac.update(subject.as_bytes());
        let digest = mac.finalize().into_bytes();
        Some(ActorKey(Arc::from(hex::encode(digest))))
    }
}

fn read_secret_from_dotenv(path: &Path) -> Result<Option<String>> {
    if !path.exists() {
        return Ok(None);
    }

    for item in dotenvy::from_path_iter(path).with_context(|| format!("read {}", path.display()))? {
        let (key, value) = item.with_context(|| format!("parse {}", path.display()))?;
        if key == ACTOR_KEY_SECRET_ENV && !value.is_empty() {
            return Ok(Some(value));
        }
    }
    Ok(None)
}

fn generate_secret() -> String {
    let mut bytes = [0_u8; GENERATED_SECRET_BYTES];
    OsRng.fill_bytes(&mut bytes);
    hex::encode(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn actor_key_uses_pinned_hmac_sha256_fixture() {
        let deriver = ActorKeyDeriver::from_secret("test-secret").unwrap();
        let key = deriver.derive_subject("user@example.com").unwrap();

        assert_eq!(
            key.as_str(),
            "01d54a297ba437dea0ea85db3e939dff2f8947abd7925d12d1c46ae3ac4308a4"
        );
        assert_ne!(key.as_str(), "user@example.com");
    }

    #[test]
    fn actor_key_is_deterministic_without_leaking_raw_subject() {
        let deriver = ActorKeyDeriver::from_secret("installation-secret").unwrap();
        let first = deriver.derive_subject("alice").unwrap();
        let second = deriver.derive_subject("alice").unwrap();

        assert_eq!(first, second);
        assert!(!first.as_str().contains("alice"));
        assert_eq!(first.as_str().len(), 64);
    }

    #[test]
    fn empty_subject_has_no_actor_key() {
        let deriver = ActorKeyDeriver::from_secret("installation-secret").unwrap();
        assert!(deriver.derive_subject("").is_none());
    }

    #[test]
    fn load_or_create_generates_secret_in_dotenv_when_absent() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(".env");

        let deriver = ActorKeyDeriver::load_or_create_from_path(&path).unwrap();
        let key = deriver.derive_subject("alice").unwrap();
        let contents = std::fs::read_to_string(&path).unwrap();

        assert!(contents.contains(ACTOR_KEY_SECRET_ENV));
        assert_eq!(key.as_str().len(), 64);
        assert!(!contents.contains("alice"));
    }
}
