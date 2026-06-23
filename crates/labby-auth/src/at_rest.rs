/// At-rest encryption for upstream provider refresh tokens.
///
/// Provider tokens (e.g. Google refresh tokens) are encrypted with
/// ChaCha20-Poly1305 before being written to SQLite so that a copied
/// `auth.db` cannot be used as an upstream-account pivot.
///
/// # Key management
///
/// The encryption key is a 32-byte value derived from the
/// `{PREFIX}_TOKEN_ENCRYPTION_KEY` environment variable, which must be
/// either 64 hex digits or 43 base64url-no-pad characters.  When the env
/// var is absent the helper functions are no-ops: data is stored as-is
/// (backward-compatible with existing deployments that haven't opted in to
/// at-rest protection yet).
///
/// # Storage format
///
/// Each ciphertext is a `base64url(nonce || ciphertext+tag)` string where:
/// - `nonce` is a randomly generated 12-byte ChaCha20-Poly1305 nonce
/// - `ciphertext+tag` is the output of ChaCha20-Poly1305 AEAD encryption:
///   the ciphertext bytes followed by the 16-byte authentication tag appended
///   by the AEAD library
///
/// The sentineled prefix `"enc:"` is prepended so that legacy plaintext values
/// (or rows from databases without encryption) can be distinguished from
/// encrypted blobs and returned as-is.
use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use chacha20poly1305::aead::{Aead, KeyInit};
use chacha20poly1305::{ChaCha20Poly1305, Nonce};
use getrandom::fill;

use crate::error::AuthError;

/// Sentinel prefix that distinguishes ciphertext blobs from legacy plaintext.
const ENC_PREFIX: &str = "enc:";

/// 32-byte ChaCha20-Poly1305 key.
#[derive(Clone, PartialEq, Eq)]
pub struct TokenEncryptionKey([u8; 32]);

impl std::fmt::Debug for TokenEncryptionKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("TokenEncryptionKey(<redacted>)")
    }
}

impl TokenEncryptionKey {
    /// Parse a 32-byte key from a hex (64 chars) or base64url-no-pad (43 chars) string.
    pub fn from_encoded(s: &str) -> Result<Self, AuthError> {
        let s = s.trim();
        if s.len() == 64 {
            // 64-char hex string
            let mut key = [0u8; 32];
            for (i, chunk) in s.as_bytes().chunks(2).enumerate() {
                let hex = std::str::from_utf8(chunk).map_err(|_| {
                    AuthError::Config(
                        "TOKEN_ENCRYPTION_KEY hex string contains invalid UTF-8".to_string(),
                    )
                })?;
                key[i] = u8::from_str_radix(hex, 16).map_err(|_| {
                    AuthError::Config(format!(
                        "TOKEN_ENCRYPTION_KEY hex string contains invalid character `{hex}`"
                    ))
                })?;
            }
            Ok(Self(key))
        } else {
            // Try base64url decode
            let bytes = URL_SAFE_NO_PAD.decode(s).map_err(|_| {
                AuthError::Config(
                    "TOKEN_ENCRYPTION_KEY must be 64 hex digits or 43 base64url characters"
                        .to_string(),
                )
            })?;
            if bytes.len() != 32 {
                return Err(AuthError::Config(format!(
                    "TOKEN_ENCRYPTION_KEY base64-decoded to {} bytes, expected 32",
                    bytes.len()
                )));
            }
            let mut key = [0u8; 32];
            key.copy_from_slice(&bytes);
            Ok(Self(key))
        }
    }

    /// Derive a deterministic 32-byte key from an arbitrary passphrase using
    /// SHA-256.  Not for production use — provided so tests can create keys
    /// without managing hex strings.
    #[cfg(test)]
    pub fn from_passphrase(passphrase: &str) -> Self {
        use sha2::{Digest, Sha256};
        let hash = Sha256::digest(passphrase.as_bytes());
        let mut key = [0u8; 32];
        key.copy_from_slice(&hash);
        Self(key)
    }
}

/// Encrypt a provider refresh token with ChaCha20-Poly1305.
///
/// Returns `"enc:<base64url(nonce||ciphertext)>"`.
pub fn encrypt_provider_token(
    key: &TokenEncryptionKey,
    plaintext: &str,
) -> Result<String, AuthError> {
    let cipher = ChaCha20Poly1305::new_from_slice(&key.0)
        .map_err(|e| AuthError::Storage(format!("init cipher: {e}")))?;

    let mut nonce_bytes = [0u8; 12];
    fill(&mut nonce_bytes).map_err(|e| AuthError::Storage(format!("generate nonce: {e}")))?;
    let nonce = Nonce::from(nonce_bytes);

    let ciphertext = cipher
        .encrypt(&nonce, plaintext.as_bytes())
        .map_err(|_| AuthError::Storage("token encryption failed".to_string()))?;

    // Layout: nonce (12 bytes) || ciphertext+tag
    let mut blob = Vec::with_capacity(12 + ciphertext.len());
    blob.extend_from_slice(&nonce_bytes);
    blob.extend_from_slice(&ciphertext);

    Ok(format!("{ENC_PREFIX}{}", URL_SAFE_NO_PAD.encode(&blob)))
}

/// Decrypt a provider refresh token that was encrypted by [`encrypt_provider_token`].
///
/// If the stored value does not carry the `"enc:"` prefix it is returned
/// as-is — this handles legacy plaintext rows and the case where no
/// encryption key is configured.
pub fn decrypt_provider_token(key: &TokenEncryptionKey, stored: &str) -> Result<String, AuthError> {
    let Some(encoded) = stored.strip_prefix(ENC_PREFIX) else {
        // Not encrypted (legacy row or no-key path) — return plaintext.
        return Ok(stored.to_string());
    };

    let blob = URL_SAFE_NO_PAD.decode(encoded).map_err(|_| {
        AuthError::Storage("provider token ciphertext is not valid base64".to_string())
    })?;

    if blob.len() < 12 {
        return Err(AuthError::Storage(
            "provider token ciphertext blob too short".to_string(),
        ));
    }

    let (nonce_bytes, ciphertext) = blob.split_at(12);
    let nonce = Nonce::from_slice(nonce_bytes);

    let cipher = ChaCha20Poly1305::new_from_slice(&key.0)
        .map_err(|e| AuthError::Storage(format!("init cipher: {e}")))?;

    let plaintext_bytes = cipher.decrypt(nonce, ciphertext).map_err(|_| {
        AuthError::Storage(
            "provider token decryption failed (wrong key or corrupted data)".to_string(),
        )
    })?;

    String::from_utf8(plaintext_bytes)
        .map_err(|_| AuthError::Storage("decrypted provider token is not valid UTF-8".to_string()))
}

/// Attempt to encrypt `value` if a key is available, returning the stored
/// representation.  If `key` is `None` the value is stored as plaintext.
pub fn maybe_encrypt(key: Option<&TokenEncryptionKey>, value: &str) -> Result<String, AuthError> {
    match key {
        Some(k) => encrypt_provider_token(k, value),
        None => Ok(value.to_string()),
    }
}

/// Attempt to decrypt `stored` if it carries the `"enc:"` prefix.  If no
/// key is provided but the value is encrypted, return an error — the caller
/// should not silently return ciphertext as the token value.
pub fn maybe_decrypt(key: Option<&TokenEncryptionKey>, stored: &str) -> Result<String, AuthError> {
    if stored.starts_with(ENC_PREFIX) {
        let k = key.ok_or_else(|| {
            AuthError::Config(
                "provider token is encrypted but no TOKEN_ENCRYPTION_KEY is configured".to_string(),
            )
        })?;
        decrypt_provider_token(k, stored)
    } else {
        // Plaintext — return as-is regardless of whether a key is present.
        Ok(stored.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_key() -> TokenEncryptionKey {
        TokenEncryptionKey::from_passphrase("test-key-for-unit-tests")
    }

    #[test]
    fn encrypt_decrypt_round_trip() {
        let key = test_key();
        let token = "fake-google-refresh-token-for-tests";
        let encrypted = encrypt_provider_token(&key, token).unwrap();
        assert!(
            encrypted.starts_with(ENC_PREFIX),
            "ciphertext should carry enc: prefix"
        );
        assert_ne!(encrypted, token, "ciphertext must not equal plaintext");
        let decrypted = decrypt_provider_token(&key, &encrypted).unwrap();
        assert_eq!(decrypted, token);
    }

    #[test]
    fn different_encryptions_of_same_token_differ() {
        let key = test_key();
        let token = "test-provider-token-same-value";
        let enc1 = encrypt_provider_token(&key, token).unwrap();
        let enc2 = encrypt_provider_token(&key, token).unwrap();
        assert_ne!(
            enc1, enc2,
            "fresh nonce per encrypt must produce distinct ciphertexts"
        );
    }

    #[test]
    fn legacy_plaintext_returned_unchanged_via_decrypt() {
        let key = test_key();
        let plaintext = "legacy-plaintext-no-prefix";
        let result = decrypt_provider_token(&key, plaintext).unwrap();
        assert_eq!(result, plaintext);
    }

    #[test]
    fn maybe_encrypt_without_key_is_identity() {
        let value = "some-token";
        let result = maybe_encrypt(None, value).unwrap();
        assert_eq!(result, value);
    }

    #[test]
    fn maybe_decrypt_without_key_returns_plaintext() {
        let result = maybe_decrypt(None, "plaintext-token").unwrap();
        assert_eq!(result, "plaintext-token");
    }

    #[test]
    fn maybe_decrypt_without_key_fails_on_encrypted_value() {
        let key = test_key();
        let encrypted = encrypt_provider_token(&key, "secret").unwrap();
        let err = maybe_decrypt(None, &encrypted).unwrap_err();
        assert!(
            err.to_string().contains("TOKEN_ENCRYPTION_KEY"),
            "should tell operator to configure key, got: {err}"
        );
    }

    #[test]
    fn key_from_str_accepts_hex() {
        let hex = "0102030405060708090a0b0c0d0e0f101112131415161718191a1b1c1d1e1f20";
        let key = TokenEncryptionKey::from_encoded(hex).unwrap();
        assert_eq!(key.0[0], 0x01);
        assert_eq!(key.0[31], 0x20);
    }

    #[test]
    fn key_from_str_rejects_short_hex() {
        let err = TokenEncryptionKey::from_encoded("aabbcc").unwrap_err();
        assert!(err.to_string().contains("TOKEN_ENCRYPTION_KEY"));
    }

    #[test]
    fn tampered_ciphertext_fails_decryption() {
        let key = test_key();
        let encrypted = encrypt_provider_token(&key, "secret").unwrap();
        // Decode the base64url payload, flip a byte in the binary blob, re-encode.
        let encoded = encrypted.strip_prefix(ENC_PREFIX).unwrap();
        let mut blob = URL_SAFE_NO_PAD.decode(encoded).unwrap();
        if blob.len() > 20 {
            blob[20] ^= 0xff;
        }
        let tampered = format!("{ENC_PREFIX}{}", URL_SAFE_NO_PAD.encode(&blob));
        let result = decrypt_provider_token(&key, &tampered);
        assert!(
            result.is_err(),
            "tampered ciphertext should fail to decrypt"
        );
    }
}
