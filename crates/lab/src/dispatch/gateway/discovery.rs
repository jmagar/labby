pub mod claude_code;
pub mod claude_desktop;
pub mod codex;
pub mod cursor;
pub mod gemini;
pub mod opencode;
pub mod vscode;
pub mod windsurf;

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use serde_json::Value;

use crate::config::{ImportSource, UpstreamConfig};

/// A single MCP server discovered from an external config file.
#[derive(Debug, Clone)]
pub struct DiscoveredServer {
    pub name: String,
    pub spec: UpstreamConfig,
    pub source_client: String,
    pub source_path: String,
    pub env_key_count: usize,
}

/// Scan all known MCP config locations and return deduplicated discovered servers.
///
/// Deduplication is by name; first-seen wins. Clients are scanned in a stable
/// order: cursor, claude-code, claude-desktop, codex, windsurf, opencode, vscode, gemini.
/// GitHub Copilot is covered by the vscode scanner (Copilot uses VS Code's mcp.json).
pub fn discover_all(home: &Path) -> Vec<DiscoveredServer> {
    let mut seen: HashMap<String, DiscoveredServer> = HashMap::new();
    let mut ordered: Vec<String> = Vec::new();

    let all: Vec<DiscoveredServer> = [
        cursor::discover(home),
        claude_code::discover(home),
        claude_desktop::discover(home),
        codex::discover(home),
        windsurf::discover(home),
        opencode::discover(home),
        vscode::discover(home),
        gemini::discover(home),
    ]
    .into_iter()
    .flatten()
    .collect();

    for server in all {
        if !seen.contains_key(&server.name) {
            ordered.push(server.name.clone());
            seen.insert(server.name.clone(), server);
        }
    }

    ordered
        .into_iter()
        .filter_map(|name| seen.remove(&name))
        .collect()
}

/// Resolve home directory from env, falling back to dirs crate.
pub(crate) fn home_dir() -> Option<PathBuf> {
    dirs::home_dir()
}

/// Return an XDG config dir if set, else ~/.config.
pub(crate) fn xdg_config_home(home: &Path) -> PathBuf {
    std::env::var("XDG_CONFIG_HOME")
        .ok()
        .filter(|s| !s.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| home.join(".config"))
}

/// Parse `mcpServers`, `servers`, `mcp`, or root-level keys from a JSON value.
/// Returns a vec of (name, entry_json) pairs.
pub(crate) fn extract_mcp_entries(
    value: &Value,
    allow_root_fallback: bool,
) -> Vec<(String, &Value)> {
    for key in &["mcpServers", "servers", "mcp"] {
        if let Some(map) = value.get(key).and_then(|v| v.as_object()) {
            return map.iter().map(|(k, v)| (k.clone(), v)).collect();
        }
    }
    if allow_root_fallback {
        if let Some(map) = value.as_object() {
            let looks_like_servers = map.values().any(|v| {
                v.as_object()
                    .map(|o| o.contains_key("command") || o.contains_key("url"))
                    .unwrap_or(false)
            });
            if looks_like_servers {
                return map.iter().map(|(k, v)| (k.clone(), v)).collect();
            }
        }
    }
    vec![]
}

/// Read a JSON file and return parsed Value, ignoring missing-file and parse errors.
pub(crate) fn read_json(path: &Path) -> Option<Value> {
    let raw = std::fs::read_to_string(path).ok()?;
    // Strip JSONC-style comments by using a lenient approach: serde_json doesn't
    // support JSONC natively, so strip line comments and block comments first.
    let stripped = strip_jsonc_comments(&raw);
    serde_json::from_str(&stripped).ok()
}

/// Strip `//` line comments and `/* */` block comments from JSON-like text.
fn strip_jsonc_comments(src: &str) -> String {
    let mut out = String::with_capacity(src.len());
    let mut chars = src.chars().peekable();
    let mut in_string = false;

    while let Some(ch) = chars.next() {
        match ch {
            '"' if !in_string => {
                in_string = true;
                out.push(ch);
            }
            '"' if in_string => {
                in_string = false;
                out.push(ch);
            }
            '\\' if in_string => {
                out.push(ch);
                if let Some(next) = chars.next() {
                    out.push(next);
                }
            }
            '/' if !in_string => match chars.peek() {
                Some('/') => {
                    chars.next();
                    for c in chars.by_ref() {
                        if c == '\n' {
                            out.push('\n');
                            break;
                        }
                    }
                }
                Some('*') => {
                    chars.next();
                    let mut prev = ' ';
                    for c in chars.by_ref() {
                        if prev == '*' && c == '/' {
                            break;
                        }
                        prev = c;
                    }
                }
                _ => out.push(ch),
            },
            _ => out.push(ch),
        }
    }
    out
}

/// Convert a JSON server entry to an `UpstreamConfig`.
/// Returns None if the entry has neither command nor url.
pub(crate) fn entry_to_upstream(
    name: &str,
    entry: &Value,
    source_client: &str,
    source_path: &str,
    imported_at: &str,
) -> Option<UpstreamConfig> {
    let command = entry
        .get("command")
        .and_then(|v| match v {
            Value::String(s) => Some(s.clone()),
            Value::Array(arr) => arr.first().and_then(|v| v.as_str()).map(String::from),
            _ => None,
        })
        .or_else(|| {
            entry
                .get("executable")
                .and_then(Value::as_str)
                .map(String::from)
        });

    let url = entry
        .get("url")
        .or_else(|| entry.get("baseUrl"))
        .or_else(|| entry.get("base_url"))
        .or_else(|| entry.get("serverUrl"))
        .or_else(|| entry.get("server_url"))
        .and_then(Value::as_str)
        .map(String::from);

    if command.is_none() && url.is_none() {
        return None;
    }

    let args: Vec<String> = if command.is_some() {
        // If command was an array, remaining elements are args
        match entry.get("command") {
            Some(Value::Array(arr)) if arr.len() > 1 => arr[1..]
                .iter()
                .filter_map(|v| v.as_str().map(String::from))
                .collect(),
            _ => entry
                .get("args")
                .and_then(Value::as_array)
                .map(|arr| {
                    arr.iter()
                        .filter_map(|v| v.as_str().map(String::from))
                        .collect()
                })
                .unwrap_or_default(),
        }
    } else {
        vec![]
    };

    Some(UpstreamConfig {
        name: name.to_string(),
        enabled: false,
        url,
        command,
        args,
        env: std::collections::BTreeMap::new(),
        bearer_token_env: None,
        proxy_resources: true,
        proxy_prompts: true,
        expose_tools: None,
        expose_resources: None,
        expose_prompts: None,
        oauth: None,
        imported_from: Some(ImportSource::new(source_client, source_path, imported_at)),
        tool_search: Default::default(),
    })
}

pub(crate) fn env_key_count(entry: &Value) -> usize {
    entry
        .get("env")
        .and_then(Value::as_object)
        .map_or(0, serde_json::Map::len)
}

/// ISO 8601 timestamp for right now (seconds precision).
pub(crate) fn now_iso8601() -> String {
    jiff::Timestamp::now().to_string()
}

/// Scan a list of candidate paths and return discovered servers from the first one that parses.
pub(crate) fn scan_paths(
    paths: &[PathBuf],
    source_client: &str,
    allow_root_fallback: bool,
) -> Vec<DiscoveredServer> {
    let now = now_iso8601();
    let mut results = Vec::new();

    for path in paths {
        let Some(value) = read_json(path) else {
            continue;
        };
        let path_str = path.to_string_lossy().to_string();
        let entries = extract_mcp_entries(&value, allow_root_fallback);
        for (name, entry) in entries {
            if let Some(spec) = entry_to_upstream(&name, entry, source_client, &path_str, &now) {
                results.push(DiscoveredServer {
                    name: name.clone(),
                    spec,
                    source_client: source_client.to_string(),
                    source_path: path_str.clone(),
                    env_key_count: env_key_count(entry),
                });
            }
        }
        // Don't stop at first path — scan all paths and let caller dedup if needed.
    }

    results
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    #[test]
    fn imported_upstream_does_not_copy_raw_env_values() {
        let entry = json!({
            "command": "example-mcp",
            "args": ["--serve"],
            "env": {
                "API_TOKEN": "secret-token",
                "MODE": "local"
            }
        });

        let spec = super::entry_to_upstream(
            "example",
            &entry,
            "test-client",
            "/home/alice/.config/test/mcp.json",
            "2026-05-14T00:00:00Z",
        )
        .expect("upstream spec");

        assert_eq!(super::env_key_count(&entry), 2);
        assert!(spec.env.is_empty());
        assert!(!spec.enabled);
        assert!(spec.imported_from.is_some());
    }
}
