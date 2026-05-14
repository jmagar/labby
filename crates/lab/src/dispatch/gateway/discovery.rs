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

/// Read a JSON file and return parsed Value, distinguishing missing-file from parse errors.
pub(crate) fn read_json(path: &Path) -> Option<Value> {
    let raw = match std::fs::read_to_string(path) {
        Ok(s) => s,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return None,
        Err(e) => {
            tracing::debug!(
                path = %path.display(),
                kind = "io_error",
                error = %e,
                "discovery: skipping unreadable config"
            );
            return None;
        }
    };
    // Strip JSONC-style comments by using a lenient approach: serde_json doesn't
    // support JSONC natively, so strip line comments and block comments first.
    let stripped = strip_jsonc_comments(&raw);
    match serde_json::from_str(&stripped) {
        Ok(v) => Some(v),
        Err(e) => {
            tracing::warn!(
                path = %path.display(),
                kind = "decode_error",
                error = %e,
                "discovery: skipping malformed config"
            );
            None
        }
    }
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
        tracing::trace!(
            name = %name,
            source_client = %source_client,
            source_path = %source_path,
            reason = "missing command and url",
            "discovery: skipping entry with no command or url"
        );
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

/// Scan a list of candidate paths and return discovered servers from the first one that parses.
pub(crate) fn scan_paths(
    paths: &[PathBuf],
    source_client: &str,
    allow_root_fallback: bool,
) -> Vec<DiscoveredServer> {
    let now = jiff::Timestamp::now().to_string();
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
    fn strip_jsonc_comments_handles_all_cases() {
        let cases: &[(&str, &str)] = &[
            // URL strings with // must not be stripped
            (
                r#"{"url": "https://host/path"}"#,
                r#"{"url": "https://host/path"}"#,
            ),
            // Block comment markers inside string survive
            (
                r#"{"key": "/* not stripped */"}"#,
                r#"{"key": "/* not stripped */"}"#,
            ),
            // Line comment after value is stripped
            ("{\"k\": 1} // comment", "{\"k\": 1} "),
            // Block comment stripped
            ("{/* x */\"k\":1}", "{\"k\":1}"),
            // Unterminated block — no panic, rest is empty
            ("{\"k\": 1} /* never closed", "{\"k\": 1} "),
        ];
        for (input, expected) in cases {
            assert_eq!(
                &super::strip_jsonc_comments(input),
                expected,
                "input: {input:?}"
            );
        }
    }

    #[test]
    fn strip_jsonc_comments_preserves_newlines_in_line_comments() {
        let input = "{\n// comment line\n\"k\":1\n}";
        let output = super::strip_jsonc_comments(input);
        // The newline after the comment should be preserved
        assert!(output.contains('\n'));
        assert!(!output.contains("comment line"));
        assert!(output.contains("\"k\":1"));
    }

    #[test]
    fn strip_jsonc_comments_escaped_backslash_before_closing_quote() {
        // Escaped backslash before closing quote: string ends at the quote after \\
        // The // comment that follows should be stripped
        let input = r#"{"k": "end\\"}//comment"#;
        let output = super::strip_jsonc_comments(input);
        assert!(
            !output.contains("comment"),
            "line comment should be stripped: {output:?}"
        );
        assert!(
            output.contains("end\\\\"),
            "escaped backslash in string should survive: {output:?}"
        );
    }

    #[test]
    fn strip_jsonc_comments_nested_block_comment_markers_in_string() {
        // Block comment markers inside a string must not terminate the string
        let input = r#"{"k": "a /* b */"}"#;
        let output = super::strip_jsonc_comments(input);
        assert_eq!(output, r#"{"k": "a /* b */"}"#);
    }

    #[test]
    fn entry_to_upstream_command_array_extracts_first_as_command() {
        let entry = json!({"command": ["node", "server.js"]});
        let spec = super::entry_to_upstream("x", &entry, "test", "/p", "2026-01-01T00:00:00Z")
            .expect("should produce spec");
        assert_eq!(spec.command.as_deref(), Some("node"));
        assert_eq!(spec.args, vec!["server.js"]);
    }

    #[test]
    fn entry_to_upstream_single_element_array_uses_sibling_args() {
        // len==1 array falls through to read the sibling "args" key
        let entry = json!({"command": ["node"], "args": ["x", "y"]});
        let spec = super::entry_to_upstream("x", &entry, "test", "/p", "2026-01-01T00:00:00Z")
            .expect("should produce spec");
        assert_eq!(spec.command.as_deref(), Some("node"));
        assert_eq!(spec.args, vec!["x", "y"]);
    }

    #[test]
    fn entry_to_upstream_multi_element_array_ignores_sibling_args() {
        // len>1 array: remaining elements ARE the args, sibling ignored
        let entry = json!({"command": ["node", "s.js"], "args": ["ignored"]});
        let spec = super::entry_to_upstream("x", &entry, "test", "/p", "2026-01-01T00:00:00Z")
            .expect("should produce spec");
        assert_eq!(spec.command.as_deref(), Some("node"));
        assert_eq!(spec.args, vec!["s.js"]);
    }

    #[test]
    fn entry_to_upstream_empty_array_yields_none() {
        let entry = json!({"command": []});
        assert!(
            super::entry_to_upstream("x", &entry, "test", "/p", "2026-01-01T00:00:00Z").is_none()
        );
    }

    #[test]
    fn entry_to_upstream_non_string_array_element_yields_none() {
        let entry = json!({"command": [42, "server.js"]});
        assert!(
            super::entry_to_upstream("x", &entry, "test", "/p", "2026-01-01T00:00:00Z").is_none()
        );
    }

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

    #[test]
    fn extract_mcp_entries_prefers_mcp_servers_over_servers() {
        let v = serde_json::json!({
            "mcpServers": {"a": {"command": "x"}},
            "servers":    {"b": {"command": "y"}}
        });
        let entries = super::extract_mcp_entries(&v, false);
        let names: Vec<&str> = entries.iter().map(|(n, _)| n.as_str()).collect();
        assert_eq!(names, vec!["a"]);
    }

    #[test]
    fn extract_mcp_entries_root_fallback_disabled_returns_empty() {
        let v = serde_json::json!({"a": {"command": "x"}});
        assert!(super::extract_mcp_entries(&v, false).is_empty());
    }

    #[test]
    fn extract_mcp_entries_root_fallback_skips_non_server_root() {
        let v = serde_json::json!({"theme": "dark", "version": 1});
        assert!(super::extract_mcp_entries(&v, true).is_empty());
    }

    #[test]
    fn extract_mcp_entries_root_fallback_returns_server_looking_keys() {
        let v = serde_json::json!({
            "a": {"command": "node"},
            "b": {"url": "https://h"}
        });
        let entries = super::extract_mcp_entries(&v, true);
        assert_eq!(entries.len(), 2);
    }

    #[test]
    fn extract_mcp_entries_root_fallback_enabled_but_mcp_servers_key_present_uses_it() {
        // When mcpServers is present, root fallback is never reached
        let v = serde_json::json!({
            "mcpServers": {"canonical": {"command": "c"}},
            "root_server": {"command": "r"}
        });
        let entries = super::extract_mcp_entries(&v, true);
        let names: Vec<&str> = entries.iter().map(|(n, _)| n.as_str()).collect();
        assert_eq!(names, vec!["canonical"]);
    }

    #[test]
    fn discover_all_deduplicates_by_name_first_seen_wins() {
        let dir = tempfile::tempdir().expect("tempdir");
        let home = dir.path();

        // Cursor config with "shared" server — scanned first
        let cursor_dir = home.join(".cursor");
        std::fs::create_dir_all(&cursor_dir).unwrap();
        std::fs::write(
            cursor_dir.join("mcp.json"),
            r#"{"mcpServers": {"shared": {"command": "from-cursor"}}}"#,
        )
        .unwrap();

        // Claude Code settings.json with same "shared" server — scanned second
        let claude_dir = home.join(".claude");
        std::fs::create_dir_all(&claude_dir).unwrap();
        std::fs::write(
            claude_dir.join("settings.json"),
            r#"{"mcpServers": {"shared": {"command": "from-claude-code"}}}"#,
        )
        .unwrap();

        let results = super::discover_all(home);

        // cursor wins (first-seen)
        let shared: Vec<_> = results.iter().filter(|s| s.name == "shared").collect();
        assert_eq!(shared.len(), 1, "shared should appear exactly once");
        assert_eq!(
            shared[0].source_client, "cursor",
            "cursor should win as first-seen"
        );
        assert_eq!(
            shared[0].spec.command.as_deref(),
            Some("from-cursor"),
            "cursor command should be preserved"
        );
    }
}
