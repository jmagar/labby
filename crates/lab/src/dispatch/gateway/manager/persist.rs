//! Env-file persistence helpers: canonical `.env` path resolution and gateway
//! bearer-token writes (backup-first, idempotent).

use std::path::PathBuf;

use crate::config::{EnvCredential, backup_env, env_is_up_to_date, write_env};
use crate::dispatch::error::ToolError;
use crate::dispatch::gateway::config::validate_bearer_token_env_name;

use super::GatewayManager;

impl GatewayManager {
    pub(super) fn env_path(&self) -> PathBuf {
        if let Some(override_path) = &self.env_path_override {
            return override_path.clone();
        }
        crate::config::home_dir()
            .map(|h| h.join(".lab").join(".env"))
            .unwrap_or_else(|| PathBuf::from(".env"))
    }

    pub(super) async fn persist_gateway_bearer_token(
        &self,
        env_name: &str,
        token_value: &str,
    ) -> Result<(), ToolError> {
        validate_bearer_token_env_name(env_name)?;

        let auth_header = normalize_gateway_bearer_token(token_value);
        let env_path = self.env_path();
        let creds = [EnvCredential {
            service: "gateway".to_string(),
            url: None,
            secret: Some(auth_header),
            env_field: env_name.to_string(),
        }];

        if !env_is_up_to_date(&env_path, &creds) {
            drop(backup_env(&env_path).map_err(|e| {
                ToolError::internal_message(format!("failed to back up env file: {e}"))
            })?);
            write_env(&env_path, &creds, true).map_err(|e| {
                ToolError::internal_message(format!("failed to write env file: {e}"))
            })?;
        }

        if let Some(service_clients) = &self.service_clients {
            service_clients
                .refresh_from_env_path(&env_path)
                .await
                .map_err(|e| {
                    ToolError::internal_message(format!(
                        "failed to refresh service clients from {}: {e}",
                        env_path.display()
                    ))
                })?;
        }

        Ok(())
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
