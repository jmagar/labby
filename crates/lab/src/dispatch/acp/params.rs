//! Param coercion helpers for ACP dispatch actions.
//!
//! ACP-specific semantic: empty strings are treated as missing/absent. The
//! shared `helpers::require_str` does not filter empties, and
//! `helpers::optional_str` rejects empties as `InvalidParam` — neither matches
//! ACP's contract. These wrappers preserve the empty-as-missing behavior on
//! top of the shared helpers.

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::dispatch::error::ToolError;
use crate::dispatch::helpers;

pub const MAX_LOCAL_ATTACHMENTS: usize = 5;
pub const MAX_LOCAL_ATTACHMENT_BYTES: u64 = 2 * 1024 * 1024;

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

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum PromptAttachmentParam {
    #[serde(rename = "local")]
    Local {
        id: String,
        name: String,
        #[serde(rename = "mimeType")]
        mime_type: String,
        size: u64,
        #[serde(flatten)]
        content: LocalAttachmentContent,
    },
    #[serde(rename = "file")]
    File { path: String },
}

pub fn local_prompt_attachments(
    attachments: &[PromptAttachmentParam],
) -> Vec<LocalPromptAttachment> {
    attachments
        .iter()
        .filter_map(|attachment| match attachment {
            PromptAttachmentParam::Local {
                id,
                name,
                mime_type,
                size,
                content,
            } => Some(LocalPromptAttachment {
                id: id.clone(),
                name: name.clone(),
                mime_type: mime_type.clone(),
                size: *size,
                content: content.clone(),
            }),
            PromptAttachmentParam::File { .. } => None,
        })
        .collect()
}

pub fn validate_prompt_attachments(
    attachments: &[PromptAttachmentParam],
) -> Result<(), ToolError> {
    let local = local_prompt_attachments(attachments);
    validate_local_attachments(&local)
}

pub fn validate_local_attachments(
    attachments: &[LocalPromptAttachment],
) -> Result<(), ToolError> {
    if attachments.len() > MAX_LOCAL_ATTACHMENTS {
        return Err(ToolError::InvalidParam {
            message: format!("at most {MAX_LOCAL_ATTACHMENTS} attachments are allowed"),
            param: "attachments".into(),
        });
    }

    for attachment in attachments {
        if attachment.size > MAX_LOCAL_ATTACHMENT_BYTES {
            return Err(ToolError::InvalidParam {
                message: format!("attachment `{}` exceeds the 2 MiB limit", attachment.name),
                param: "attachments".into(),
            });
        }
        if !is_supported_attachment_mime(&attachment.mime_type) {
            return Err(ToolError::InvalidParam {
                message: format!(
                    "attachment `{}` has unsupported type `{}`",
                    attachment.name, attachment.mime_type
                ),
                param: "attachments".into(),
            });
        }
    }

    Ok(())
}

pub fn is_supported_attachment_mime(mime_type: &str) -> bool {
    let normalized = mime_type.trim().to_ascii_lowercase();
    normalized.starts_with("text/")
        || matches!(
            normalized.as_str(),
            "application/json"
                | "application/pdf"
                | "image/png"
                | "image/jpeg"
                | "image/gif"
                | "image/webp"
        )
}

/// Extract a required string param. Returns `MissingParam` if absent, null, or empty.
pub fn require_str<'a>(params: &'a Value, name: &str) -> Result<&'a str, ToolError> {
    let value = helpers::require_str(params, name)?;
    if value.is_empty() {
        return Err(ToolError::MissingParam {
            message: format!("required param `{name}` is missing or empty"),
            param: name.to_string(),
        });
    }
    Ok(value)
}

/// Extract an optional string param. Returns `None` if absent, null, or empty.
pub fn opt_str<'a>(params: &'a Value, name: &str) -> Option<&'a str> {
    params
        .get(name)
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
}

/// Extract an optional u64 param.
pub fn opt_u64(params: &Value, name: &str) -> Result<Option<u64>, ToolError> {
    match params.get(name) {
        None | Some(Value::Null) => Ok(None),
        Some(v) => v.as_u64().map(Some).ok_or_else(|| ToolError::InvalidParam {
            message: format!("`{name}` must be a non-negative integer"),
            param: name.to_string(),
        }),
    }
}
