use std::path::{Path, PathBuf};

use super::{DiscoveredServer, scan_paths};

/// Gemini CLI stores MCP servers in ~/.gemini/mcp.json and ~/.gemini/settings.json,
/// both using the top-level `mcpServers` key.
pub fn discover(home: &Path) -> Vec<DiscoveredServer> {
    let paths: Vec<PathBuf> = vec![
        home.join(".gemini").join("mcp.json"),
        home.join(".gemini").join("settings.json"),
    ];
    scan_paths(&paths, "gemini", false)
}
