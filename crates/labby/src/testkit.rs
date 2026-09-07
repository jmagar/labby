//! Explicit production-owned fixtures for live integration tests.

use std::path::PathBuf;

/// Provision an active, same-organization File Stash recipient using the
/// AccessStore authority and its current-schema validation.
pub async fn provision_file_stash_recipient(
    access_store_path: PathBuf,
    owner_credential_id: String,
    principal_id: String,
    display_name: String,
    recipient_credential_id: String,
) -> Result<(), String> {
    let store = crate::access::AccessStore::open_existing_current(access_store_path)
        .await
        .map_err(|error| error.to_string())?;
    store
        .provision_file_stash_recipient_fixture(
            owner_credential_id,
            principal_id,
            display_name,
            recipient_credential_id,
        )
        .await
        .map_err(|error| error.to_string())
}
