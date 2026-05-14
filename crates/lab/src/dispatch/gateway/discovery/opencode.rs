use std::path::{Path, PathBuf};

use crate::dispatch::helpers::env_non_empty;

use super::{DiscoveredServer, entry_to_upstream, env_key_count, read_json};

/// OpenCode uses the `mcp` key only (no root fallback, no `mcpServers`).
pub fn discover(home: &Path) -> Vec<DiscoveredServer> {
    let now = jiff::Timestamp::now().to_string();
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

#[cfg(test)]
mod tests {
    use std::path::Path;
    use tempfile::TempDir;

    fn write(dir: &Path, rel: &str, content: &str) {
        let p = dir.join(rel);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, content).unwrap();
    }

    /// Return the opencode config dir that `candidate_paths` will use for the
    /// given `home`, replicating the same XDG / default logic so the tests are
    /// not broken by an ambient `XDG_CONFIG_HOME` on the test runner.
    fn opencode_config_dir(home: &Path) -> std::path::PathBuf {
        let xdg = std::env::var("XDG_CONFIG_HOME")
            .ok()
            .filter(|s| !s.is_empty())
            .map(std::path::PathBuf::from);
        xdg.map(|x| x.join("opencode"))
            .unwrap_or_else(|| home.join(".config/opencode"))
    }

    #[test]
    fn discovers_from_default_config_dir() {
        let dir = TempDir::new().unwrap();
        let config_dir = opencode_config_dir(dir.path());
        let rel = config_dir
            .strip_prefix(dir.path())
            .expect("config dir must be under home")
            .join("opencode.json");
        write(
            dir.path(),
            rel.to_str().unwrap(),
            r#"{"mcp": {"my-server": {"command": "node", "args": ["server.js"]}}}"#,
        );
        let results = super::discover(dir.path());
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].name, "my-server");
        assert_eq!(results[0].source_client, "opencode");
    }

    #[test]
    fn mcp_key_only_no_mcp_servers_fallback() {
        let dir = TempDir::new().unwrap();
        let config_dir = opencode_config_dir(dir.path());
        let rel = config_dir
            .strip_prefix(dir.path())
            .expect("config dir must be under home")
            .join("opencode.json");
        // opencode only uses "mcp" key, NOT "mcpServers"
        write(
            dir.path(),
            rel.to_str().unwrap(),
            r#"{"mcpServers": {"wrong-key": {"command": "node"}}}"#,
        );
        let results = super::discover(dir.path());
        assert!(results.is_empty(), "opencode must not use mcpServers key");
    }
}

fn candidate_paths(home: &Path) -> Vec<PathBuf> {
    let mut paths = Vec::new();

    // Env override — reject empty strings; an empty OPENCODE_CONFIG would produce
    // PathBuf::from("") which silently returns ENOENT, suppressing all discovery.
    if let Some(p) = env_non_empty("OPENCODE_CONFIG") {
        paths.push(PathBuf::from(p));
        return paths; // explicit override wins
    }

    let config_dir = std::env::var("OPENCODE_CONFIG_DIR")
        .ok()
        .filter(|s| !s.is_empty())
        .map(PathBuf::from);

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
