use std::path::{Path, PathBuf};

use serde_json::Value;

use super::{DiscoveredServer, entry_to_upstream, env_key_count, extract_mcp_entries, read_json};

/// Claude Code stores MCP servers in several locations.
/// Settings files (`settings.json`, `settings.local.json`) use strict key lookup
/// (no root fallback). Legacy files (`.claude.json`, `.claude/mcp.json`) allow
/// root fallback and also support per-project entries under `projects[path].mcpServers`.
pub fn discover(home: &Path) -> Vec<DiscoveredServer> {
    let now = jiff::Timestamp::now().to_string();
    let mut results = Vec::new();

    // Strict-lookup settings files (no root fallback)
    let strict_paths: &[PathBuf] = &[
        home.join(".claude").join("settings.local.json"),
        home.join(".claude").join("settings.json"),
    ];
    for path in strict_paths {
        if let Some(value) = read_json(path) {
            let path_str = path.to_string_lossy().to_string();
            harvest(&value, false, "claude-code", &path_str, &now, &mut results);
        }
    }

    // Legacy / mcp.json files — root fallback allowed
    let legacy_paths: &[PathBuf] = &[
        home.join(".claude").join("mcp.json"),
        home.join(".claude.json"),
    ];
    for path in legacy_paths {
        if let Some(value) = read_json(path) {
            let path_str = path.to_string_lossy().to_string();
            harvest(&value, true, "claude-code", &path_str, &now, &mut results);
        }
    }

    results
}

fn harvest(
    value: &Value,
    allow_root_fallback: bool,
    source_client: &str,
    source_path: &str,
    now: &str,
    out: &mut Vec<DiscoveredServer>,
) {
    for (name, entry) in extract_mcp_entries(value, allow_root_fallback) {
        if let Some(spec) = entry_to_upstream(name.as_str(), entry, source_client, source_path, now)
        {
            out.push(DiscoveredServer {
                name,
                spec,
                source_client: source_client.to_string(),
                source_path: source_path.to_string(),
                env_key_count: env_key_count(entry),
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;
    use tempfile::TempDir;

    fn write(dir: &Path, rel: &str, content: &str) {
        let p = dir.join(rel);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, content).unwrap();
    }

    #[test]
    fn settings_json_strict_no_root_fallback() {
        let dir = TempDir::new().unwrap();
        // Root-level server-looking key in settings.json — must NOT be harvested
        write(
            dir.path(),
            ".claude/settings.json",
            r#"{"my-server": {"command": "x"}}"#,
        );
        let results = super::discover(dir.path());
        assert!(
            results.is_empty(),
            "settings.json must not allow root fallback"
        );
    }

    #[test]
    fn mcp_servers_key_in_settings_json_is_harvested() {
        let dir = TempDir::new().unwrap();
        write(
            dir.path(),
            ".claude/settings.json",
            r#"{"mcpServers": {"my-server": {"command": "node"}}}"#,
        );
        let results = super::discover(dir.path());
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].name, "my-server");
        assert_eq!(results[0].source_client, "claude-code");
    }

    #[test]
    fn dot_claude_json_allows_root_fallback() {
        let dir = TempDir::new().unwrap();
        // Root-level server key in .claude.json — SHOULD be harvested
        write(
            dir.path(),
            ".claude.json",
            r#"{"my-server": {"command": "x"}}"#,
        );
        let results = super::discover(dir.path());
        assert_eq!(results.len(), 1, ".claude.json must allow root fallback");
        assert_eq!(results[0].name, "my-server");
    }
}
