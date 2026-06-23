//! Secret masking for `setup.draft.get` responses.
//!
//! Looks up each key against the cached secret-key set built from the
//! runtime registry. Secret values are replaced with [`SECRET_SENTINEL`]
//! before they leave the dispatch layer.

use labby_apis::setup::SECRET_SENTINEL;

use super::client::{cached_secret_keys, key_matches_secret_suffix};

/// Returns `true` when `key` is either registered as a secret env var on
/// some PluginMeta, OR matches the default-mask secret suffix list
/// (`*_API_KEY`, `*_TOKEN`, `*_PASSWORD`, `*_SECRET`). The default mask
/// covers third-party keys the user pasted into the draft and services
/// compiled out via feature flags.
#[must_use]
pub fn is_secret_key(key: &str) -> bool {
    cached_secret_keys().contains(key) || key_matches_secret_suffix(key)
}

/// Replace the value with the sentinel if the key is registered as a secret.
#[must_use]
pub fn mask_value(key: &str, value: &str) -> String {
    if value.is_empty() {
        return value.to_owned();
    }
    if is_secret_key(key) {
        SECRET_SENTINEL.to_owned()
    } else {
        value.to_owned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unknown_keys_are_not_masked() {
        assert_eq!(mask_value("NOT_A_REAL_KEY", "hello"), "hello");
    }

    #[test]
    fn empty_value_passes_through() {
        // Even for known secrets, an empty value should not become "***".
        // The wizard relies on empty as "unset".
        assert_eq!(mask_value("ANYTHING", ""), "");
    }

    #[test]
    fn suffix_default_mask_catches_unregistered_secrets() {
        // Keys with secret-suffix names should mask even if no registered
        // service declares them — covers third-party env vars and
        // feature-gated services.
        for key in &["FOO_API_KEY", "BAR_TOKEN", "BAZ_PASSWORD", "QUX_SECRET"] {
            assert_eq!(mask_value(key, "supersecret"), "***", "key={key}");
        }
        assert_eq!(mask_value("FOO_URL", "https://x"), "https://x");
    }
}
