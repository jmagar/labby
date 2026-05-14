use std::path::{Path, PathBuf};

use super::{DiscoveredServer, entry_to_upstream, env_key_count, now_iso8601, read_json};

/// OpenCode uses the `mcp` key only (no root fallback, no `mcpServers`).
pub fn discover(home: &Path) -> Vec<DiscoveredServer> {
    let now = now_iso8601();
    let mut results = Vec::new();

    for path in candidate_paths(home) {
        let Some(value) = read_json(&path) else {
            continue;
        };
        let Some(mcp_obj) = value.get("mcp").and_then(|v| v.as_object()) else {
            continue;
        };
        let path_str = path.to_string_lossy().to_string();
        for (name, entry) in mcp_obj {
            if let Some(spec) = entry_to_upstream(name, entry, "opencode", &path_str, &now) {
                results.push(DiscoveredServer {
                    name: name.clone(),
                    spec,
                    source_client: "opencode".to_string(),
                    source_path: path_str.clone(),
                    env_key_count: env_key_count(entry),
                });
            }
        }
    }

    results
}

fn candidate_paths(home: &Path) -> Vec<PathBuf> {
    let mut paths = Vec::new();

    // Env override
    if let Ok(p) = std::env::var("OPENCODE_CONFIG") {
        paths.push(PathBuf::from(p));
        return paths; // explicit override wins
    }

    let config_dir = std::env::var("OPENCODE_CONFIG_DIR").ok().map(PathBuf::from);

    if let Some(ref dir) = config_dir {
        paths.push(dir.join("opencode.jsonc"));
        paths.push(dir.join("opencode.json"));
    }

    // XDG / default config dirs
    let xdg = std::env::var("XDG_CONFIG_HOME")
        .ok()
        .filter(|s| !s.is_empty())
        .map(PathBuf::from);

    #[cfg(target_os = "windows")]
    let default_config_dir = std::env::var("APPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(|_| home.join("AppData/Roaming"))
        .join("opencode");

    #[cfg(not(target_os = "windows"))]
    let default_config_dir = xdg
        .clone()
        .map(|x| x.join("opencode"))
        .unwrap_or_else(|| home.join(".config/opencode"));

    paths.push(default_config_dir.join("opencode.jsonc"));
    paths.push(default_config_dir.join("opencode.json"));

    paths
}
