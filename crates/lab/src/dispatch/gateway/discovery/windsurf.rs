use std::path::{Path, PathBuf};

use super::{DiscoveredServer, scan_paths, xdg_config_home};

pub fn discover(home: &Path) -> Vec<DiscoveredServer> {
    let mut paths: Vec<PathBuf> = vec![
        home.join(".codeium/windsurf/mcp_config.json"),
        home.join(".codeium/windsurf-next/mcp_config.json"),
        home.join(".windsurf/mcp_config.json"),
    ];

    #[cfg(not(target_os = "windows"))]
    paths.push(xdg_config_home(home).join(".codeium/windsurf/mcp_config.json"));

    #[cfg(target_os = "windows")]
    {
        let appdata = std::env::var("APPDATA")
            .map(PathBuf::from)
            .unwrap_or_else(|_| home.join("AppData/Roaming"));
        paths.push(appdata.join("Codeium/windsurf/mcp_config.json"));
    }

    // Also check VS Code-like user dirs that Windsurf Next uses on Linux/Mac
    #[cfg(not(target_os = "windows"))]
    {
        paths.push(
            xdg_config_home(home)
                .join("Windsurf - Next")
                .join("User")
                .join("mcp.json"),
        );
        paths.push(
            xdg_config_home(home)
                .join("Windsurf")
                .join("User")
                .join("mcp.json"),
        );
    }

    scan_paths(&paths, "windsurf", true)
}
