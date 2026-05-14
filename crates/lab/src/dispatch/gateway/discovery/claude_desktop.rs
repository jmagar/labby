use std::path::{Path, PathBuf};

use super::{DiscoveredServer, scan_paths};

pub fn discover(home: &Path) -> Vec<DiscoveredServer> {
    let path: PathBuf;

    #[cfg(target_os = "macos")]
    {
        path = home.join("Library/Application Support/Claude/claude_desktop_config.json");
    }

    #[cfg(target_os = "windows")]
    {
        path = home.join("AppData/Roaming/Claude/claude_desktop_config.json");
    }

    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        path = home.join(".config/Claude/claude_desktop_config.json");
    }

    scan_paths(&[path], "claude-desktop", true)
}
