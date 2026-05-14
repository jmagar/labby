use std::path::{Path, PathBuf};

use super::{DiscoveredServer, entry_to_upstream, env_key_count, now_iso8601};

pub fn discover(home: &Path) -> Vec<DiscoveredServer> {
    let paths: &[PathBuf] = &[home.join(".codex").join("config.toml")];
    let now = now_iso8601();
    let mut results = Vec::new();

    for path in paths {
        let Ok(raw) = std::fs::read_to_string(path) else {
            continue;
        };
        let Ok(table) = toml::from_str::<toml::Value>(&raw) else {
            continue;
        };
        let path_str = path.to_string_lossy().to_string();
        let Some(servers) = table.get("mcp_servers").and_then(|v| v.as_table()) else {
            continue;
        };
        for (name, entry) in servers {
            // Convert TOML value to serde_json::Value for shared entry_to_upstream
            let Ok(json_entry) = serde_json::to_value(entry) else {
                continue;
            };
            if let Some(spec) = entry_to_upstream(name, &json_entry, "codex", &path_str, &now) {
                results.push(DiscoveredServer {
                    name: name.clone(),
                    spec,
                    source_client: "codex".to_string(),
                    source_path: path_str.clone(),
                    env_key_count: env_key_count(&json_entry),
                });
            }
        }
    }

    results
}
