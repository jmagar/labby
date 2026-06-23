use std::path::Path;

use labby_apis::marketplace::{PluginComponent, PluginComponentKind, PluginManifestSummary};
use serde_json::{Map, Value};

/// Replace the user's home-directory prefix with literal `~`.
///
/// lab-zxx5.27: promoted to `dispatch::helpers::redact_home` so the `node/`
/// install paths can call it without reaching into this marketplace module.
/// This is a thin re-export wrapper; the canonical implementation lives in
/// `dispatch/helpers.rs`.
pub use crate::dispatch::helpers::redact_home;

pub fn manifest_summary_from_marketplace_plugin(
    plugin_json: &Value,
) -> Option<PluginManifestSummary> {
    Some(PluginManifestSummary {
        description: plugin_json
            .get("description")
            .and_then(Value::as_str)
            .map(ToString::to_string),
        version: plugin_json
            .get("version")
            .and_then(Value::as_str)
            .map(ToString::to_string),
        interface: None,
    })
}

pub fn manifest_summary_from_codex_manifest(manifest: &Value) -> Option<PluginManifestSummary> {
    Some(PluginManifestSummary {
        description: manifest
            .get("description")
            .or_else(|| manifest.get("metadata").and_then(|m| m.get("description")))
            .and_then(Value::as_str)
            .map(ToString::to_string),
        version: manifest
            .get("version")
            .and_then(Value::as_str)
            .map(ToString::to_string),
        interface: manifest.get("interface").cloned(),
    })
}

pub fn tags_from_marketplace_plugin(plugin_json: &Value) -> Vec<String> {
    let mut tags = Vec::new();
    for key in ["tags", "keywords"] {
        if let Some(arr) = plugin_json.get(key).and_then(Value::as_array) {
            for value in arr {
                if let Some(tag) = value.as_str() {
                    let tag = tag.to_string();
                    if !tags.contains(&tag) {
                        tags.push(tag);
                    }
                }
            }
        }
    }
    if let Some(category) = plugin_json.get("category").and_then(Value::as_str) {
        let category = category.to_string();
        if !tags.contains(&category) {
            tags.insert(0, category);
        }
    }
    tags
}

pub fn components_from_manifest_and_layout(
    root: Option<&Path>,
    manifest: Option<&Value>,
) -> Vec<PluginComponent> {
    let mut out = Vec::new();
    if let Some(manifest) = manifest {
        collect_components_from_value(manifest, &mut out);
    }
    if let Some(root) = root {
        collect_components_from_layout(root, &mut out);
    }
    out.sort_by(|left, right| left.path.cmp(&right.path));
    out.dedup_by(|left, right| left.kind == right.kind && left.path == right.path);
    out
}

fn collect_components_from_value(manifest: &Value, out: &mut Vec<PluginComponent>) {
    // Manifests may use camelCase (Claude Code convention) or snake_case
    // (generic ecosystem convention). Both are collected; dedup in the caller
    // removes duplicates from manifests that include both spellings.
    if let Some(obj) = manifest.as_object() {
        collect_component_array(obj, "skills", PluginComponentKind::Skills, out);
        collect_component_array(obj, "apps", PluginComponentKind::Apps, out);
        collect_component_array(obj, "mcpServers", PluginComponentKind::McpServers, out);
        collect_component_array(obj, "mcp_servers", PluginComponentKind::McpServers, out);
        collect_component_array(obj, "lspServers", PluginComponentKind::LspServers, out);
        collect_component_array(obj, "lsp_servers", PluginComponentKind::LspServers, out);
        collect_component_array(obj, "commands", PluginComponentKind::Commands, out);
        collect_component_array(obj, "agents", PluginComponentKind::Agents, out);
        collect_component_array(obj, "assets", PluginComponentKind::Assets, out);
        collect_component_array(obj, "hooks", PluginComponentKind::Hooks, out);
        collect_component_array(obj, "monitors", PluginComponentKind::Monitors, out);
        collect_component_array(obj, "outputStyles", PluginComponentKind::OutputStyles, out);
        collect_component_array(obj, "output_styles", PluginComponentKind::OutputStyles, out);
        collect_component_array(obj, "themes", PluginComponentKind::Themes, out);
        collect_channel_components(obj, out);
    }
}

fn collect_channel_components(obj: &Map<String, Value>, out: &mut Vec<PluginComponent>) {
    let Some(value) = obj.get("channels") else {
        return;
    };
    match value {
        Value::Array(items) => {
            for (index, item) in items.iter().enumerate() {
                out.push(component_from_inline_config(
                    PluginComponentKind::Channels,
                    &index.to_string(),
                    item,
                ));
            }
        }
        Value::Object(items) => {
            for (name, item) in items {
                if let Value::String(path) = item {
                    out.push(PluginComponent {
                        kind: PluginComponentKind::Channels,
                        path: path.clone(),
                        name: name.clone(),
                        metadata: None,
                    });
                } else {
                    out.push(component_from_inline_config(
                        PluginComponentKind::Channels,
                        name,
                        item,
                    ));
                }
            }
        }
        Value::String(path) => out.push(PluginComponent {
            kind: PluginComponentKind::Channels,
            path: path.clone(),
            name: path_name(path),
            metadata: None,
        }),
        _ => {}
    }
}

fn component_from_inline_config(
    kind: PluginComponentKind,
    fallback_name: &str,
    value: &Value,
) -> PluginComponent {
    if let Some(path) = value.as_str() {
        return PluginComponent {
            kind,
            path: path.to_string(),
            name: path_name(path),
            metadata: None,
        };
    }

    let name = value
        .get("name")
        .and_then(Value::as_str)
        .unwrap_or(fallback_name)
        .to_string();
    PluginComponent {
        kind,
        path: value
            .get("path")
            .or_else(|| value.get("file"))
            .and_then(Value::as_str)
            .unwrap_or(&name)
            .to_string(),
        name,
        metadata: Some(value.clone()),
    }
}

fn collect_component_array(
    obj: &Map<String, Value>,
    key: &str,
    kind: PluginComponentKind,
    out: &mut Vec<PluginComponent>,
) {
    let Some(value) = obj.get(key) else {
        return;
    };
    match value {
        Value::Array(items) => {
            for item in items {
                if let Some(component) = component_from_value(kind, item) {
                    out.push(component);
                }
            }
        }
        Value::Object(items) => {
            for (name, item) in items {
                if let Some(component) = component_from_object_entry(kind, name, item) {
                    out.push(component);
                }
            }
        }
        Value::String(path) => out.push(PluginComponent {
            kind,
            path: path.clone(),
            name: path_name(path),
            metadata: None,
        }),
        _ => {}
    }
}

fn component_from_value(kind: PluginComponentKind, value: &Value) -> Option<PluginComponent> {
    match value {
        Value::String(path) => Some(PluginComponent {
            kind,
            path: path.clone(),
            name: path_name(path),
            metadata: None,
        }),
        Value::Object(map) => {
            let path = map
                .get("path")
                .or_else(|| map.get("file"))
                .or_else(|| map.get("entry"))
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string();
            if path.is_empty() {
                return None;
            }
            let name = map
                .get("name")
                .and_then(Value::as_str)
                .map(ToString::to_string)
                .unwrap_or_else(|| path_name(&path));
            Some(PluginComponent {
                kind,
                path,
                name,
                metadata: Some(Value::Object(map.clone())),
            })
        }
        _ => None,
    }
}

fn component_from_object_entry(
    kind: PluginComponentKind,
    name: &str,
    value: &Value,
) -> Option<PluginComponent> {
    match value {
        Value::String(path) => Some(PluginComponent {
            kind,
            path: path.clone(),
            name: name.to_string(),
            metadata: None,
        }),
        Value::Object(map) => {
            let path = map
                .get("path")
                .or_else(|| map.get("file"))
                .or_else(|| map.get("entry"))
                .and_then(Value::as_str)
                .unwrap_or(name)
                .to_string();
            Some(PluginComponent {
                kind,
                path,
                name: name.to_string(),
                metadata: Some(Value::Object(map.clone())),
            })
        }
        _ => None,
    }
}

fn collect_components_from_layout(root: &Path, out: &mut Vec<PluginComponent>) {
    let specs = [
        ("skills", PluginComponentKind::Skills),
        ("apps", PluginComponentKind::Apps),
        ("commands", PluginComponentKind::Commands),
        ("agents", PluginComponentKind::Agents),
        ("assets", PluginComponentKind::Assets),
        ("hooks", PluginComponentKind::Hooks),
        ("monitors", PluginComponentKind::Monitors),
        ("bin", PluginComponentKind::Bin),
        ("output-styles", PluginComponentKind::OutputStyles),
        ("themes", PluginComponentKind::Themes),
    ];

    for (dir_name, kind) in specs {
        let dir = root.join(dir_name);
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if kind == PluginComponentKind::Agents && path.is_dir() {
                collect_agent_markdown_entries(root, &path, out);
                continue;
            }
            out.push(component_from_layout_path(root, &path, kind));
        }
    }

    let codex_manifest = root.join(".codex-plugin").join("plugin.json");
    if codex_manifest.exists() {
        out.push(PluginComponent {
            kind: PluginComponentKind::Files,
            path: ".codex-plugin/plugin.json".into(),
            name: "plugin.json".into(),
            metadata: None,
        });
    }

    collect_component_file(root, ".mcp.json", PluginComponentKind::McpServers, out);
    collect_component_file(root, ".lsp.json", PluginComponentKind::LspServers, out);
    collect_component_file(root, "settings.json", PluginComponentKind::Settings, out);
    collect_component_file(root, "channels.json", PluginComponentKind::Channels, out);
}

fn path_name(path: &str) -> String {
    Path::new(path)
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or(path)
        .to_string()
}

fn collect_component_file(
    root: &Path,
    rel_path: &str,
    kind: PluginComponentKind,
    out: &mut Vec<PluginComponent>,
) {
    if root.join(rel_path).exists() {
        out.push(PluginComponent {
            kind,
            path: rel_path.into(),
            name: path_name(rel_path),
            metadata: None,
        });
    }
}

fn component_from_layout_path(
    root: &Path,
    path: &Path,
    kind: PluginComponentKind,
) -> PluginComponent {
    // Component paths are stable, cross-platform identifiers compared with
    // forward slashes (e.g. `skills/review`). Normalize the OS-native separator
    // so Windows backslashes don't produce `skills\review`.
    let rel = path
        .strip_prefix(root)
        .unwrap_or(path)
        .to_string_lossy()
        .replace('\\', "/");
    let mut name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or(&rel)
        .to_string();
    let metadata = if kind == PluginComponentKind::Agents {
        let metadata = markdown_frontmatter(path);
        if let Some(frontmatter_name) = metadata
            .as_ref()
            .and_then(|value| value.get("name"))
            .and_then(Value::as_str)
        {
            name = frontmatter_name.to_string();
        }
        metadata.map(Value::Object)
    } else {
        None
    };

    PluginComponent {
        kind,
        path: rel,
        name,
        metadata,
    }
}

fn collect_agent_markdown_entries(root: &Path, dir: &Path, out: &mut Vec<PluginComponent>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_agent_markdown_entries(root, &path, out);
            continue;
        }
        if path.extension().and_then(|ext| ext.to_str()) == Some("md") {
            out.push(component_from_layout_path(
                root,
                &path,
                PluginComponentKind::Agents,
            ));
        }
    }
}

fn markdown_frontmatter(path: &Path) -> Option<Map<String, Value>> {
    let content = std::fs::read_to_string(path).ok()?;
    let rest = content
        .strip_prefix("---\n")
        .or_else(|| content.strip_prefix("---\r\n"))?;
    let end = rest
        .find("\n---\n")
        .or_else(|| rest.find("\n---\r\n"))
        .or_else(|| rest.find("\r\n---\r\n"))?;
    let block = &rest[..end];
    let mut metadata = Map::new();
    for line in block.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let Some((key, value)) = line.split_once(':') else {
            continue;
        };
        let key = key.trim();
        if key.is_empty() {
            continue;
        }
        metadata.insert(
            key.to_string(),
            Value::String(unquote_yaml_scalar(value.trim())),
        );
    }
    (!metadata.is_empty()).then_some(metadata)
}

fn unquote_yaml_scalar(value: &str) -> String {
    value
        .strip_prefix('"')
        .and_then(|value| value.strip_suffix('"'))
        .or_else(|| {
            value
                .strip_prefix('\'')
                .and_then(|value| value.strip_suffix('\''))
        })
        .unwrap_or(value)
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn components_from_layout_matches_claude_plugin_component_locations() {
        let dir = tempfile::tempdir().expect("tempdir");
        let root = dir.path();

        std::fs::create_dir_all(root.join("skills/review")).expect("skill dir");
        std::fs::write(
            root.join("skills/review/SKILL.md"),
            "---\ndescription: Review\n---",
        )
        .expect("skill");
        std::fs::create_dir_all(root.join("commands")).expect("commands dir");
        std::fs::write(root.join("commands/ship.md"), "Ship").expect("command");
        std::fs::create_dir_all(root.join("agents")).expect("agents dir");
        std::fs::write(
            root.join("agents/reviewer.md"),
            "---\nname: code-reviewer\ndescription: Reviews code changes\nmodel: sonnet\n---\nAgent",
        )
        .expect("agent");
        std::fs::create_dir_all(root.join("agents/nested")).expect("nested agents dir");
        std::fs::write(
            root.join("agents/nested/debugger.md"),
            "---\nname: debugger\ndescription: Debugs failures\n---\nAgent",
        )
        .expect("nested agent");
        std::fs::create_dir_all(root.join("hooks")).expect("hooks dir");
        std::fs::write(root.join("hooks/hooks.json"), "{}").expect("hooks");
        std::fs::create_dir_all(root.join("monitors")).expect("monitors dir");
        std::fs::write(root.join("monitors/monitors.json"), "[]").expect("monitors");
        std::fs::create_dir_all(root.join("bin")).expect("bin dir");
        std::fs::write(root.join("bin/tool"), "#!/bin/sh\n").expect("bin");
        std::fs::create_dir_all(root.join("output-styles")).expect("output styles dir");
        std::fs::write(
            root.join("output-styles/reviewer.md"),
            "---\nname: Reviewer\n---",
        )
        .expect("output style");
        std::fs::create_dir_all(root.join("themes")).expect("themes dir");
        std::fs::write(root.join("themes/dim.json"), "{}").expect("theme");
        std::fs::write(root.join(".mcp.json"), "{\"mcpServers\":{}}").expect("mcp");
        std::fs::write(root.join(".lsp.json"), "{}").expect("lsp");
        std::fs::write(root.join("settings.json"), "{}").expect("settings");
        std::fs::write(root.join("channels.json"), "[]").expect("channels");

        let components = components_from_manifest_and_layout(Some(root), None);
        let observed: std::collections::HashSet<_> = components
            .iter()
            .map(|component| (component.kind, component.path.as_str()))
            .collect();

        assert!(observed.contains(&(PluginComponentKind::Skills, "skills/review")));
        assert!(observed.contains(&(PluginComponentKind::Commands, "commands/ship.md")));
        assert!(observed.contains(&(PluginComponentKind::Agents, "agents/reviewer.md")));
        assert!(observed.contains(&(PluginComponentKind::Agents, "agents/nested/debugger.md")));
        assert!(observed.contains(&(PluginComponentKind::Hooks, "hooks/hooks.json")));
        assert!(observed.contains(&(PluginComponentKind::Monitors, "monitors/monitors.json")));
        assert!(observed.contains(&(PluginComponentKind::Bin, "bin/tool")));
        assert!(observed.contains(&(PluginComponentKind::McpServers, ".mcp.json")));
        assert!(observed.contains(&(PluginComponentKind::LspServers, ".lsp.json")));
        assert!(observed.contains(&(PluginComponentKind::Settings, "settings.json")));
        assert!(observed.contains(&(PluginComponentKind::Channels, "channels.json")));
        assert!(observed.contains(&(
            PluginComponentKind::OutputStyles,
            "output-styles/reviewer.md"
        )));
        assert!(observed.contains(&(PluginComponentKind::Themes, "themes/dim.json")));

        let reviewer = components
            .iter()
            .find(|component| component.path == "agents/reviewer.md")
            .expect("reviewer component");
        assert_eq!(reviewer.name, "code-reviewer");
        assert_eq!(
            reviewer
                .metadata
                .as_ref()
                .and_then(|metadata| metadata.get("description"))
                .and_then(Value::as_str),
            Some("Reviews code changes")
        );
    }

    #[test]
    fn components_from_manifest_accepts_claude_component_fields() {
        let manifest = serde_json::json!({
            "skills": ["skills/review"],
            "commands": ["commands/ship.md"],
            "agents": ["agents/reviewer.md"],
            "mcpServers": ".mcp.json",
            "lspServers": ".lsp.json",
            "monitors": "monitors/monitors.json",
            "outputStyles": "output-styles/reviewer.md",
            "themes": "themes/dim.json",
            "channels": [{ "name": "team-chat", "server": "chat" }]
        });

        let components = components_from_manifest_and_layout(None, Some(&manifest));
        let observed: std::collections::HashSet<_> = components
            .iter()
            .map(|component| (component.kind, component.path.as_str()))
            .collect();

        assert!(observed.contains(&(PluginComponentKind::Skills, "skills/review")));
        assert!(observed.contains(&(PluginComponentKind::Commands, "commands/ship.md")));
        assert!(observed.contains(&(PluginComponentKind::Agents, "agents/reviewer.md")));
        assert!(observed.contains(&(PluginComponentKind::McpServers, ".mcp.json")));
        assert!(observed.contains(&(PluginComponentKind::LspServers, ".lsp.json")));
        assert!(observed.contains(&(PluginComponentKind::Monitors, "monitors/monitors.json")));
        assert!(observed.contains(&(
            PluginComponentKind::OutputStyles,
            "output-styles/reviewer.md"
        )));
        assert!(observed.contains(&(PluginComponentKind::Themes, "themes/dim.json")));
        assert!(
            components
                .iter()
                .any(|component| component.kind == PluginComponentKind::Channels
                    && component.name == "team-chat")
        );
    }

    #[test]
    fn components_from_manifest_preserves_string_channel_entries() {
        let manifest = serde_json::json!({
            "channels": [
                "channels/stable.json",
                { "name": "team-chat", "path": "channels/team.json" }
            ]
        });

        let components = components_from_manifest_and_layout(None, Some(&manifest));
        let channel = components
            .iter()
            .find(|component| component.path == "channels/stable.json")
            .expect("channel component");

        assert_eq!(channel.path, "channels/stable.json");
        assert_eq!(channel.name, "stable.json");

        let object_channel = components
            .iter()
            .find(|component| component.path == "channels/team.json")
            .expect("object channel component");
        assert_eq!(object_channel.name, "team-chat");
    }

    #[test]
    fn components_from_manifest_preserves_string_channel_map_values() {
        let manifest = serde_json::json!({
            "channels": {
                "team": "channels/team.json",
                "alerts": { "path": "channels/alerts.json" }
            }
        });

        let components = components_from_manifest_and_layout(None, Some(&manifest));

        let team = components
            .iter()
            .find(|component| component.path == "channels/team.json")
            .expect("string channel map value");
        assert_eq!(team.name, "team");

        let alerts = components
            .iter()
            .find(|component| component.path == "channels/alerts.json")
            .expect("object channel map value");
        assert_eq!(alerts.name, "alerts");
    }
}
