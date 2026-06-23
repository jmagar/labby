//! Env-file persistence: canonical `.env` path resolution and gateway
//! bearer-token writes, both delegated to the host-owned [`GatewayConfigStore`].
//!
//! The manager owns only the gateway-specific policy (token normalization and
//! env-name validation); the actual `.env` backup/atomic-write and any cached
//! service-client refresh live behind the store seam in the host (`lab`).

use std::path::PathBuf;

use labby_runtime::error::ToolError;

use crate::gateway::config::validate_bearer_token_env_name;

use super::GatewayManager;

impl GatewayManager {
    pub(super) fn env_path(&self) -> PathBuf {
        self.store.env_path()
    }

    pub(super) async fn persist_gateway_bearer_token(
        &self,
        env_name: &str,
        token_value: &str,
    ) -> Result<(), ToolError> {
        validate_bearer_token_env_name(env_name)?;
        let auth_header = normalize_gateway_bearer_token(token_value);
        self.store
            .persist_gateway_bearer_token(env_name, &auth_header)
            .await
    }
}

fn normalize_gateway_bearer_token(token_value: &str) -> String {
    let trimmed = token_value.trim();
    if trimmed
        .get(..7)
        .is_some_and(|s| s.eq_ignore_ascii_case("bearer "))
    {
        format!("Bearer {}", &trimmed[7..])
    } else {
        format!("Bearer {trimmed}")
    }
}
