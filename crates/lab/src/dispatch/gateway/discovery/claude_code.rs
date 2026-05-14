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
