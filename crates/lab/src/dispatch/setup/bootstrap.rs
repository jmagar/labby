//! First-run self-bootstrap: create a minimal `~/.lab/.env` so the server can
//! start and the operator can reach `/setup`. Non-destructive — a no-op when
//! the file already exists, so it is safe to call unconditionally at startup.

use std::path::{Path, PathBuf};

use serde_json::{Value, json};

use crate::config::env_merge::{self, EnvEntry, MergeRequest};
use crate::dispatch::error::ToolError;

use super::client::env_path;
use super::dispatch::map_merge_err;
use super::token::generate_mcp_token;

/// Result of a first-run bootstrap attempt.
///
/// `Created` carries the freshly generated token so callers (serve) can make
/// it authoritative in-process without depending on a successful env reload.
/// `AlreadyPresent` means the operator already has a `~/.lab/.env` — it is left
/// byte-for-byte untouched (the don't-clobber-operator-creds safety property).
pub enum BootstrapOutcome {
    Created { env_path: PathBuf, token: String },
    AlreadyPresent { env_path: PathBuf },
}

/// Decide whether `labby serve` should self-bootstrap: only when there is no
/// MCP bearer token configured AND OAuth is not the active mode. `oauth_mode`
/// is `true` when `LAB_AUTH_MODE=oauth`.
#[must_use]
pub fn should_bootstrap(token_configured: bool, oauth_mode: bool) -> bool {
    !token_configured && !oauth_mode
}

/// Create `~/.lab/.env` with a generated bearer token + loopback MCP defaults
/// when it does not exist. Non-destructive — returns
/// [`BootstrapOutcome::AlreadyPresent`] when the file is already there.
pub fn bootstrap() -> Result<BootstrapOutcome, ToolError> {
    bootstrap_at(&env_path())
}

/// MCP/CLI dispatch adapter: run [`bootstrap`] and serialize to the stable JSON
/// envelope `{ "created": bool, "env_path": string, "token": string|null }`.
pub fn bootstrap_action() -> Result<Value, ToolError> {
    Ok(match bootstrap()? {
        BootstrapOutcome::Created { env_path, token } => json!({
            "created": true,
            "env_path": env_path.display().to_string(),
            "token": token,
        }),
        BootstrapOutcome::AlreadyPresent { env_path } => json!({
            "created": false,
            "env_path": env_path.display().to_string(),
            "token": Value::Null,
        }),
    })
}

/// Path-parameterized core of [`bootstrap`]. Kept separate so unit tests can
/// drive it against a temp path without mutating `LAB_HOME` — the crate forbids
/// `unsafe_code`, so env mutation inside tests is unavailable (see `state.rs`).
fn bootstrap_at(env: &Path) -> Result<BootstrapOutcome, ToolError> {
    if env.exists() {
        return Ok(BootstrapOutcome::AlreadyPresent {
            env_path: env.to_path_buf(),
        });
    }

    let token = generate_mcp_token();
    let entries = vec![
        EnvEntry::new("LAB_MCP_HTTP_TOKEN", token.clone()),
        EnvEntry::new("LAB_MCP_TRANSPORT", "http"),
        EnvEntry::new("LAB_MCP_HTTP_HOST", "127.0.0.1"),
        EnvEntry::new("LAB_MCP_HTTP_PORT", "8765"),
        EnvEntry::new("LAB_AUTH_MODE", "bearer"),
    ];

    // `env_merge::merge` creates the parent dir (`create_dir_all`) and applies
    // 0600 perms on Unix, so no manual create_dir_all is needed here. Reuse the
    // canonical merge-error mapper so failures carry the stable `kind` from
    // docs/dev/ERRORS.md (merge_write_conflict, merge_temp_create, …).
    env_merge::merge(
        env,
        MergeRequest {
            entries,
            force: false,
            expected_mtime: None,
        },
    )
    .map_err(map_merge_err)?;

    Ok(BootstrapOutcome::Created {
        env_path: env.to_path_buf(),
        token,
    })
}

#[cfg(test)]
mod tests {
    use super::{BootstrapOutcome, bootstrap_at, should_bootstrap};

    #[test]
    fn should_bootstrap_only_without_token_and_oauth() {
        assert!(should_bootstrap(false, false));
        assert!(!should_bootstrap(true, false));
        assert!(!should_bootstrap(false, true));
        assert!(!should_bootstrap(true, true));
    }

    #[test]
    fn bootstrap_creates_env_with_token_then_is_idempotent() {
        let dir = tempfile::tempdir().expect("tempdir");
        let env_file = dir.path().join(".env");

        let first = bootstrap_at(&env_file).expect("first bootstrap");
        let token = match first {
            BootstrapOutcome::Created { token, .. } => token,
            BootstrapOutcome::AlreadyPresent { .. } => panic!("expected Created on first run"),
        };
        assert_eq!(token.len(), 64);

        let body = std::fs::read_to_string(&env_file).expect("read .env");
        assert!(body.contains("LAB_MCP_HTTP_TOKEN="));
        assert!(body.contains("LAB_AUTH_MODE=bearer"));

        // Second call must be a no-op (file already exists).
        let second = bootstrap_at(&env_file).expect("second bootstrap");
        assert!(
            matches!(second, BootstrapOutcome::AlreadyPresent { .. }),
            "expected AlreadyPresent on second run"
        );
    }

    #[test]
    fn bootstrap_never_clobbers_an_existing_operator_env() {
        let dir = tempfile::tempdir().expect("tempdir");
        let env_file = dir.path().join(".env");
        std::fs::write(&env_file, "LAB_MCP_HTTP_TOKEN=preexisting-operator-token\n")
            .expect("seed operator .env");

        let outcome = bootstrap_at(&env_file).expect("bootstrap over existing file");
        assert!(
            matches!(outcome, BootstrapOutcome::AlreadyPresent { .. }),
            "must not create over an existing operator .env"
        );

        let body = std::fs::read_to_string(&env_file).expect("read .env");
        assert!(
            body.contains("preexisting-operator-token"),
            "operator credentials must be preserved byte-for-byte"
        );
    }
}
