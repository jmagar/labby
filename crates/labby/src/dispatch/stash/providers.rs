//! Provider registry — resolve a concrete `StashProvider` from a record.
//!
//! Add a new `match` arm here when a new provider driver is added.

pub mod filesystem;

use labby_apis::stash::types::StashProviderRecord;

use crate::dispatch::error::ToolError;
use crate::dispatch::stash::provider::StashProvider;
use crate::dispatch::stash::providers::filesystem::FilesystemProvider;

/// Construct a boxed [`StashProvider`] from a provider record.
///
/// # Errors
/// - `unsupported_provider` — `record.kind` is not a recognised driver name.
/// - `invalid_param` — required config fields are missing or malformed.
pub fn provider_from_record(
    record: &StashProviderRecord,
) -> Result<Box<dyn StashProvider>, ToolError> {
    match record.kind.as_str() {
        "filesystem" => Ok(Box::new(FilesystemProvider::from_record(record)?)),
        other => Err(ToolError::Sdk {
            sdk_kind: "unsupported_provider".into(),
            message: format!("unknown provider kind: {other}"),
        }),
    }
}
