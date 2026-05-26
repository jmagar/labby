use std::collections::{BTreeMap, HashMap};

use crate::config::EnvCredential;
use crate::dispatch::error::ToolError;

pub(super) fn read_env_values(
    path: &std::path::Path,
) -> Result<HashMap<String, String>, ToolError> {
    Ok(dotenvy::from_path_iter(path)
        .ok()
        .map(|iter| iter.filter_map(Result::ok).collect())
        .unwrap_or_default())
}

pub(super) fn values_to_service_creds(
    service: &str,
    values: &BTreeMap<String, String>,
) -> Vec<EnvCredential> {
    values
        .iter()
        .map(|(field, value)| {
            let url = if field == &format!("{}_URL", service.to_uppercase()) {
                Some(value.clone())
            } else {
                None
            };
            let secret = if url.is_some() {
                None
            } else {
                Some(value.clone())
            };
            EnvCredential {
                service: service.to_string(),
                url,
                secret,
                env_field: field.clone(),
            }
        })
        .collect()
}
