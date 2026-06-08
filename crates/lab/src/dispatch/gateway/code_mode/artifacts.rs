//! Host-brokered artifact writes for Code Mode.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tokio::io::AsyncWriteExt;

use crate::dispatch::error::ToolError;
use crate::dispatch::helpers::{lab_home, redact_home, reject_path_traversal};

const DEFAULT_CONTENT_TYPE: &str = "text/plain";
const MAX_ARTIFACT_BYTES: usize = 1024 * 1024;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(in crate::dispatch::gateway::code_mode) struct CodeModeArtifactWrite {
    pub path: String,
    pub content: String,
    #[serde(default)]
    pub content_type: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(in crate::dispatch::gateway::code_mode) struct CodeModeArtifactReceipt {
    pub path: String,
    pub absolute_path: String,
    pub content_type: String,
    pub bytes: usize,
    pub sha256: String,
}

#[must_use]
pub(in crate::dispatch::gateway::code_mode) fn code_mode_artifact_root(run_id: &str) -> PathBuf {
    lab_home().join("code-mode-artifacts").join(run_id)
}

pub(in crate::dispatch::gateway::code_mode) async fn write_code_mode_artifact(
    root: &Path,
    request: &CodeModeArtifactWrite,
) -> Result<CodeModeArtifactReceipt, ToolError> {
    let rel_path = normalize_artifact_path(&request.path)?;
    let bytes = request.content.as_bytes();
    if bytes.len() > MAX_ARTIFACT_BYTES {
        return Err(ToolError::InvalidParam {
            message: format!(
                "artifact content is {} bytes; maximum is {} bytes",
                bytes.len(),
                MAX_ARTIFACT_BYTES
            ),
            param: "content".to_string(),
        });
    }

    let destination = root.join(&rel_path);
    if let Some(parent) = destination.parent() {
        tokio::fs::create_dir_all(parent)
            .await
            .map_err(|err| ToolError::Sdk {
                sdk_kind: "artifact_write_failed".to_string(),
                message: format!("failed to create artifact directory: {err}"),
            })?;
    }

    let mut file = tokio::fs::File::create(&destination)
        .await
        .map_err(|err| ToolError::Sdk {
            sdk_kind: "artifact_write_failed".to_string(),
            message: format!("failed to create artifact file: {err}"),
        })?;
    file.write_all(bytes).await.map_err(|err| ToolError::Sdk {
        sdk_kind: "artifact_write_failed".to_string(),
        message: format!("failed to write artifact file: {err}"),
    })?;
    file.flush().await.map_err(|err| ToolError::Sdk {
        sdk_kind: "artifact_write_failed".to_string(),
        message: format!("failed to flush artifact file: {err}"),
    })?;

    let sha256 = Sha256::digest(bytes);
    let content_type = request
        .content_type
        .as_deref()
        .filter(|value| !value.trim().is_empty())
        .unwrap_or(DEFAULT_CONTENT_TYPE)
        .to_string();

    Ok(CodeModeArtifactReceipt {
        path: rel_path,
        absolute_path: redact_home(&destination.display().to_string()),
        content_type,
        bytes: bytes.len(),
        sha256: hex::encode(sha256),
    })
}

fn normalize_artifact_path(raw: &str) -> Result<String, ToolError> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Err(ToolError::InvalidParam {
            message: "artifact path must be a non-empty relative path".to_string(),
            param: "path".to_string(),
        });
    }
    let path = Path::new(trimmed);
    if path.is_absolute() {
        return Err(ToolError::InvalidParam {
            message: "artifact path must be a relative path".to_string(),
            param: "path".to_string(),
        });
    }
    reject_path_traversal(trimmed)?;
    Ok(trimmed.replace('\\', "/"))
}
