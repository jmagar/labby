use std::path::{Path, PathBuf};

use super::{DiscoveredServer, scan_paths};
#[cfg(not(any(target_os = "macos", target_os = "windows")))]
use super::xdg_config_home;

pub fn discover(home: &Path) -> Vec<DiscoveredServer> {
    let mut paths: Vec<PathBuf> = vec![home.join(".cursor").join("mcp.json")];

    #[cfg(target_os = "macos")]
    paths.push(home.join("Library/Application Support/Cursor/User/mcp.json"));

    #[cfg(target_os = "windows")]
    paths.push(home.join("AppData/Roaming/Cursor/User/mcp.json"));

    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    paths.push(xdg_config_home(home).join("Cursor/User/mcp.json"));

    scan_paths(&paths, "cursor", true)
}
