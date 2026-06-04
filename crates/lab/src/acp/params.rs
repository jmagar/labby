//! ACP parameter types shared between the registry and the dispatch layer.
//!
//! Types in this module are referenced by `acp/registry.rs` — keeping them
//! here avoids an upward dependency from the registry into `dispatch/acp/`.
//!
//! `dispatch/acp/params.rs` re-exports these types so existing dispatch
//! callsites are not broken.

use serde::{Deserialize, Serialize};

use lab_apis::acp::types::AcpSessionState;

use crate::dispatch::error::ToolError;

pub const MAX_LOCAL_ATTACHMENTS: usize = 5;
pub const MAX_LOCAL_ATTACHMENT_BYTES: u64 = 48 * 1024;

/// Hard cap on the number of sessions a single `session.bulk_close` call can match.
/// Protects against accidental delete-all and bounds the concurrent-close fan-out.
pub const DEFAULT_BULK_CLOSE_MAX_COUNT: u32 = 500;

/// Typed selector for `session.bulk_close`. At least one of `states` or
/// `max_age_days` must be set — `validate_non_empty` enforces it so an
/// empty selector cannot be used as a delete-all shortcut.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BulkCloseSelector {
    #[serde(default)]
    pub states: Vec<AcpSessionState>,
    #[serde(default)]
    pub max_age_days: Option<u32>,
    #[serde(default = "default_bulk_close_max_count")]
    pub max_count: u32,
}

fn default_bulk_close_max_count() -> u32 {
    DEFAULT_BULK_CLOSE_MAX_COUNT
}

impl BulkCloseSelector {
    pub fn validate_non_empty(&self) -> Result<(), ToolError> {
        if self.states.is_empty() && self.max_age_days.is_none() {
            return Err(ToolError::InvalidParam {
                message: "selector must specify at least one of: states, max_age_days".to_string(),
                param: "selector".to_string(),
            });
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", tag = "contentKind")]
pub enum LocalAttachmentContent {
    #[serde(rename = "text")]
    Text { text: String },
    #[serde(rename = "blob")]
    Blob { base64: String },
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LocalPromptAttachment {
    pub id: String,
    pub name: String,
    pub mime_type: String,
    pub size: u64,
    #[serde(flatten)]
    pub content: LocalAttachmentContent,
}
