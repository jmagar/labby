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
use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as B64;

pub const MAX_LOCAL_ATTACHMENTS: usize = 5;
pub const MAX_LOCAL_ATTACHMENT_BYTES: u64 = 48 * 1024;

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

pub fn into_local_prompt_attachments(
    attachments: Vec<PromptAttachmentParam>,
) -> Vec<LocalPromptAttachment> {
    attachments
        .into_iter()
        .filter_map(|attachment| match attachment {
            PromptAttachmentParam::Local {
                id,
                name,
                mime_type,
                size,
                content,
            } => Some(LocalPromptAttachment {
                id,
                name,
                mime_type,
                size,
                content,
            }),
            PromptAttachmentParam::File { .. } => None,
        })
        .collect()
}

pub fn validate_prompt_attachments(attachments: &[PromptAttachmentParam]) -> Result<(), ToolError> {
    let local = local_prompt_attachments(attachments);
    validate_local_attachments(&local)
}

pub fn validate_local_attachments(attachments: &[LocalPromptAttachment]) -> Result<(), ToolError> {
    if attachments.len() > MAX_LOCAL_ATTACHMENTS {
        return Err(ToolError::InvalidParam {
            message: format!("at most {MAX_LOCAL_ATTACHMENTS} attachments are allowed"),
            param: "attachments".into(),
        });
    }

    for attachment in attachments {
        let actual_size = attachment_content_size(attachment)?;
        if attachment.size > MAX_LOCAL_ATTACHMENT_BYTES {
            return Err(ToolError::InvalidParam {
                message: format!("attachment `{}` exceeds the 48 KiB limit", attachment.name),
                param: "attachments".into(),
            });
        }
        if actual_size > MAX_LOCAL_ATTACHMENT_BYTES {
            return Err(ToolError::InvalidParam {
                message: format!(
                    "attachment `{}` content exceeds the 48 KiB limit",
                    attachment.name
                ),
                param: "attachments".into(),
            });
        }
        if attachment.size != actual_size {
            return Err(ToolError::InvalidParam {
                message: format!(
                    "attachment `{}` declared size {} does not match content size {}",
                    attachment.name, attachment.size, actual_size
                ),
                param: "attachments".into(),
            });
        }
        if !is_safe_attachment_name(&attachment.name) {
            return Err(ToolError::InvalidParam {
                message: format!("attachment `{}` has an unsafe name", attachment.name),
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

fn attachment_content_size(attachment: &LocalPromptAttachment) -> Result<u64, ToolError> {
    match &attachment.content {
        LocalAttachmentContent::Text { text } => Ok(text.len() as u64),
        LocalAttachmentContent::Blob { base64 } => {
            let encoded_limit = MAX_LOCAL_ATTACHMENT_BYTES.div_ceil(3) * 4;
            if base64.len() as u64 > encoded_limit {
                return Err(ToolError::InvalidParam {
                    message: format!(
                        "attachment `{}` base64 content exceeds the 48 KiB limit",
                        attachment.name
                    ),
                    param: "attachments".into(),
                });
            }
            B64.decode(base64)
                .map(|bytes| bytes.len() as u64)
                .map_err(|error| ToolError::InvalidParam {
                    message: format!(
                        "attachment `{}` has invalid base64 content: {error}",
                        attachment.name
                    ),
                    param: "attachments".into(),
                })
        }
    }
}

fn is_safe_attachment_name(name: &str) -> bool {
    !name.is_empty()
        && !name.contains('/')
        && !name.contains('\\')
        && !name.contains("..")
        && !name.chars().any(char::is_control)
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

#[cfg(test)]
mod tests {
    use super::*;

    fn text_attachment(size: u64, text: &str) -> LocalPromptAttachment {
        LocalPromptAttachment {
            id: "local-1".to_string(),
            name: "notes.txt".to_string(),
            mime_type: "text/plain".to_string(),
            size,
            content: LocalAttachmentContent::Text {
                text: text.to_string(),
            },
        }
    }

    #[test]
    fn validate_local_attachments_rejects_declared_size_mismatch() {
        let err = validate_local_attachments(&[text_attachment(1, "hello")])
            .expect_err("size mismatch should be rejected");

        assert_eq!(err.kind(), "invalid_param");
    }

    #[test]
    fn validate_local_attachments_rejects_oversized_actual_text() {
        let text = "x".repeat(MAX_LOCAL_ATTACHMENT_BYTES as usize + 1);
        let err = validate_local_attachments(&[text_attachment(text.len() as u64, &text)])
            .expect_err("oversized content should be rejected");

        assert_eq!(err.kind(), "invalid_param");
    }

    #[test]
    fn validate_local_attachments_rejects_unsafe_names() {
        let mut attachment = text_attachment(5, "hello");
        attachment.name = "../notes.txt".to_string();

        let err = validate_local_attachments(&[attachment])
            .expect_err("unsafe attachment name should be rejected");

        assert_eq!(err.kind(), "invalid_param");
    }

    #[test]
    fn validate_local_attachments_checks_decoded_blob_size() {
        let attachment = LocalPromptAttachment {
            id: "local-1".to_string(),
            name: "image.png".to_string(),
            mime_type: "image/png".to_string(),
            size: 3,
            content: LocalAttachmentContent::Blob {
                base64: "AQID".to_string(),
            },
        };

        validate_local_attachments(&[attachment]).expect("valid decoded blob");
    }
}
