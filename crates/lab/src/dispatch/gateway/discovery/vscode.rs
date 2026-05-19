use std::path::{Path, PathBuf};

use super::{DiscoveredServer, scan_paths, xdg_config_home};

/// Scans VS Code MCP configs. Also covers GitHub Copilot, which uses VS Code's
/// mcp.json when running as a VS Code extension.
pub fn discover(home: &Path) -> Vec<DiscoveredServer> {
    let mut paths: Vec<PathBuf> = Vec::new();

    #[cfg(target_os = "macos")]
    {
        paths.push(home.join("Library/Application Support/Code/User/mcp.json"));
        paths.push(home.join("Library/Application Support/Code - Insiders/User/mcp.json"));
        paths.push(home.join("Library/Application Support/Antigravity/User/mcp.json"));
    }

    #[cfg(target_os = "windows")]
    {
        let appdata = std::env::var("APPDATA")
            .map(PathBuf::from)
            .unwrap_or_else(|_| home.join("AppData/Roaming"));
        paths.push(appdata.join("Code/User/mcp.json"));
        paths.push(appdata.join("Code - Insiders/User/mcp.json"));
    }

    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        let xdg = xdg_config_home(home);
        paths.push(xdg.join("Code/User/mcp.json"));
        paths.push(xdg.join("Code - Insiders/User/mcp.json"));
        paths.push(xdg.join("Antigravity/User/mcp.json"));
    }

    scan_paths(&paths, "vscode", true)
}
