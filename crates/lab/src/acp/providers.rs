//! Installed ACP provider metadata shared by marketplace install and chat runtime.

use std::collections::BTreeMap;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::dispatch::error::ToolError;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AcpProviderEntry {
    pub id: String,
    pub name: String,
    pub version: String,
    pub distribution: String,
    /// Argv[0] for the provider subprocess. Always a single binary name or
    /// absolute path — never a quoted command line. Structured `args` carry
    /// the rest.
    pub command: String,
    /// Argv[1..] for the provider subprocess. Each entry is one literal
    /// argument; spaces and quoting in entries are preserved verbatim.
    /// Empty for legacy entries written before this field existed — readers
    /// fall back to whitespace-splitting `command` in that case (legacy
    /// only, no quote handling).
    #[serde(default)]
    pub args: Vec<String>,
    /// Working directory for the provider subprocess. `None` keeps the
    /// session-level cwd from `StartSessionInput`, which is the previous
    /// behavior.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cwd: Option<PathBuf>,
    /// Per-provider environment overrides applied on top of the global
    /// subprocess allowlist (see `provider_subprocess_env`). Use this for
    /// tokens or settings that must be scoped to one provider rather than
    /// leaking from the lab process environment.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub env: BTreeMap<String, String>,
    pub installed_at: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sha256: Option<String>,
}

pub fn providers_path() -> Result<PathBuf, ToolError> {
    let env_path = crate::config::dotenv_path().ok_or_else(|| ToolError::Sdk {
        sdk_kind: "internal_error".to_string(),
        message: "cannot determine ~/.lab path".to_string(),
    })?;
    let dir = env_path
        .parent()
        .ok_or_else(|| ToolError::internal_message("dotenv path has no parent"))?;
    Ok(dir.join("acp-providers.json"))
}

pub fn read_providers() -> Result<Vec<AcpProviderEntry>, ToolError> {
    let path = providers_path()?;
    if !path.exists() {
        return Ok(Vec::new());
    }
    let bytes = std::fs::read(&path)
        .map_err(|e| ToolError::internal_message(format!("read {}: {e}", path.display())))?;
    if bytes.is_empty() {
        return Ok(Vec::new());
    }
    serde_json::from_slice(&bytes).map_err(|e| ToolError::Sdk {
        sdk_kind: "decode_error".to_string(),
        message: format!("parse {}: {e}", path.display()),
    })
}

pub fn write_providers(entries: &[AcpProviderEntry]) -> Result<(), ToolError> {
    use std::io::Write;

    let path = providers_path()?;
    let dir = path
        .parent()
        .ok_or_else(|| ToolError::internal_message("providers path has no parent"))?;
    std::fs::create_dir_all(dir)
        .map_err(|e| ToolError::internal_message(format!("create {}: {e}", dir.display())))?;
    let mut tmp = tempfile::NamedTempFile::new_in(dir)
        .map_err(|e| ToolError::internal_message(format!("temp file: {e}")))?;
    let body = serde_json::to_vec_pretty(entries)
        .map_err(|e| ToolError::internal_message(format!("serialize providers: {e}")))?;
    tmp.write_all(&body)
        .map_err(|e| ToolError::internal_message(format!("write temp: {e}")))?;
    tmp.flush()
        .map_err(|e| ToolError::internal_message(format!("flush temp: {e}")))?;
    if let Ok(meta) = std::fs::symlink_metadata(&path)
        && meta.file_type().is_symlink()
    {
        return Err(ToolError::Sdk {
            sdk_kind: "invalid_param".to_string(),
            message: format!(
                "refusing to overwrite symlink at {} (acp-providers.json must be a regular file)",
                path.display()
            ),
        });
    }
    match tmp.persist(&path) {
        Ok(_) => {}
        Err(error) if error.error.raw_os_error() == Some(16) => {
            write_mounted_provider_file_in_place(&path, &body)?;
        }
        Err(error) => {
            return Err(ToolError::internal_message(format!(
                "persist {}: {error}",
                path.display()
            )));
        }
    }
    Ok(())
}

fn write_mounted_provider_file_in_place(path: &PathBuf, body: &[u8]) -> Result<(), ToolError> {
    use std::io::Write;

    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .truncate(true)
        .open(path)
        .map_err(|e| {
            ToolError::internal_message(format!(
                "open mounted provider file {} for fallback write: {e}",
                path.display()
            ))
        })?;
    file.write_all(body).map_err(|e| {
        ToolError::internal_message(format!(
            "write mounted provider file {}: {e}",
            path.display()
        ))
    })?;
    file.flush().map_err(|e| {
        ToolError::internal_message(format!(
            "flush mounted provider file {}: {e}",
            path.display()
        ))
    })?;
    file.sync_all().map_err(|e| {
        ToolError::internal_message(format!(
            "fsync mounted provider file {}: {e}",
            path.display()
        ))
    })?;
    Ok(())
}

pub fn upsert_provider(entry: &AcpProviderEntry) -> Result<(), ToolError> {
    let mut entries = read_providers()?;
    if let Some(existing) = entries.iter_mut().find(|e| e.id == entry.id) {
        *existing = entry.clone();
    } else {
        entries.push(entry.clone());
    }
    write_providers(&entries)
}

pub fn remove_provider(id: &str) -> Result<bool, ToolError> {
    let mut entries = read_providers()?;
    let before = entries.len();
    entries.retain(|e| e.id != id);
    let removed = entries.len() != before;
    if removed {
        write_providers(&entries)?;
    }
    Ok(removed)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn structured_entry_round_trips_quoted_args_and_spaces() {
        let mut env = BTreeMap::new();
        env.insert(
            "CODEX_TOKEN".to_string(),
            "sk-with-spaces and quotes".to_string(),
        );

        let entry = AcpProviderEntry {
            id: "test-provider".to_string(),
            name: "Test".to_string(),
            version: "1.0.0".to_string(),
            distribution: "binary".to_string(),
            command: "/opt/with spaces/bin/codex".to_string(),
            args: vec![
                "--config".to_string(),
                "value with spaces".to_string(),
                "--quoted=\"already-quoted\"".to_string(),
                "".to_string(),
            ],
            cwd: Some(PathBuf::from("/var/lib/with spaces")),
            env,
            installed_at: "2026-04-30T00:00:00Z".to_string(),
            sha256: None,
        };

        let json = serde_json::to_string(&entry).expect("serialize");
        let round: AcpProviderEntry = serde_json::from_str(&json).expect("deserialize");

        assert_eq!(round.command, "/opt/with spaces/bin/codex");
        assert_eq!(round.args, entry.args);
        assert_eq!(
            round.cwd.as_deref(),
            Some(PathBuf::from("/var/lib/with spaces").as_path())
        );
        assert_eq!(
            round.env.get("CODEX_TOKEN").map(String::as_str),
            Some("sk-with-spaces and quotes")
        );
    }

    #[test]
    fn legacy_entry_without_args_field_deserializes_with_empty_args() {
        // Legacy on-disk shape: no args/cwd/env keys.
        let legacy = serde_json::json!({
            "id": "old-provider",
            "name": "Old",
            "version": "0.9.0",
            "distribution": "npx",
            "command": "npx -y @scope/old-acp --flag",
            "installed_at": "2026-01-01T00:00:00Z",
        });

        let entry: AcpProviderEntry = serde_json::from_value(legacy).expect("deserialize legacy");
        assert!(
            entry.args.is_empty(),
            "legacy entries default to empty args"
        );
        assert!(entry.cwd.is_none());
        assert!(entry.env.is_empty());
        // The whitespace-joined command survives verbatim — it is the
        // launcher's responsibility to fall back to whitespace-splitting.
        assert_eq!(entry.command, "npx -y @scope/old-acp --flag");
    }

    #[test]
    fn structured_entry_omits_optional_fields_when_empty() {
        let entry = AcpProviderEntry {
            id: "min".to_string(),
            name: "Minimal".to_string(),
            version: "1.0".to_string(),
            distribution: "binary".to_string(),
            command: "/usr/bin/min".to_string(),
            args: Vec::new(),
            cwd: None,
            env: BTreeMap::new(),
            installed_at: "2026-04-30T00:00:00Z".to_string(),
            sha256: None,
        };
        let json = serde_json::to_value(&entry).expect("serialize");
        // cwd/env/sha256 are skip_if_empty/None — must not appear in JSON.
        assert!(json.get("cwd").is_none(), "cwd must be omitted when None");
        assert!(json.get("env").is_none(), "env must be omitted when empty");
        assert!(
            json.get("sha256").is_none(),
            "sha256 must be omitted when None"
        );
        // args is `default` not `skip_if_empty` — empty vec serializes as []
        // so explicit consumers can distinguish "no args" from "legacy".
        assert_eq!(json.get("args"), Some(&serde_json::json!([])));
    }
}
