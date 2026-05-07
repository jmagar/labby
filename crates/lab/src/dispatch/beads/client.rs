use lab_apis::beads::{BeadsClient, DoltConnection};
use lab_apis::core::Auth;

use crate::dispatch::error::ToolError;
use crate::dispatch::helpers::env_non_empty;

/// Build a `BeadsClient` from `BEADS_DOLT_*` env vars.
///
/// Returns `None` if `BEADS_DOLT_URL` is absent so `AppState` can leave the
/// `clients.beads` slot empty when Beads is not configured. URL parse failures
/// also surface as `None` here; callers that need to distinguish should use
/// `require_client()` instead.
pub fn client_from_env() -> Option<BeadsClient> {
    let url = env_non_empty("BEADS_DOLT_URL")?;
    let connection = DoltConnection {
        url,
        auth: auth_from_env(),
        default_project: env_non_empty("BEADS_DEFAULT_PROJECT"),
    };
    BeadsClient::new(connection).ok()
}

/// Return a configured client or a structured error.
pub fn require_client() -> Result<BeadsClient, ToolError> {
    let url = env_non_empty("BEADS_DOLT_URL").ok_or_else(not_configured_error)?;
    let connection = DoltConnection {
        url,
        auth: auth_from_env(),
        default_project: env_non_empty("BEADS_DEFAULT_PROJECT"),
    };
    BeadsClient::new(connection).map_err(|err| ToolError::Sdk {
        sdk_kind: "internal_error".into(),
        message: format!("Beads Dolt client init failed: {err}"),
    })
}

pub fn not_configured_error() -> ToolError {
    ToolError::Sdk {
        sdk_kind: "not_configured".into(),
        message: "BEADS_DOLT_URL not configured".into(),
    }
}

fn auth_from_env() -> Auth {
    let user = env_non_empty("BEADS_DOLT_USER");
    let password = env_non_empty("BEADS_DOLT_PASSWORD");
    match (user, password) {
        (None, None) => Auth::None,
        (user, password) => Auth::Basic {
            username: user.unwrap_or_default(),
            password: password.unwrap_or_default(),
        },
    }
}
