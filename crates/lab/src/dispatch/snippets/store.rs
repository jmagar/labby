use std::fs;
use std::path::{Path, PathBuf};

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::dispatch::error::ToolError;

const SNIPPET_EXTENSIONS: &[&str] = &["md", "js"];
const MAX_SNIPPET_BYTES: usize = 20 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SnippetSource {
    Builtin,
    User,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SnippetInfo {
    pub name: String,
    pub description: Option<String>,
    pub tags: Vec<String>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub inputs: BTreeMap<String, SnippetInputSpec>,
    pub source: SnippetSource,
    pub path: PathBuf,
    pub shadowed: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResolvedSnippet {
    pub name: String,
    pub description: Option<String>,
    pub tags: Vec<String>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub inputs: BTreeMap<String, SnippetInputSpec>,
    pub source: SnippetSource,
    pub path: PathBuf,
    pub body: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SnippetFrontmatter {
    pub name: String,
    pub description: String,
    pub tags: Vec<String>,
    pub inputs: BTreeMap<String, SnippetInputSpec>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct SnippetInputSpec {
    pub ty: SnippetInputType,
    #[serde(default)]
    pub required: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SnippetInputType {
    String,
    Integer,
    Number,
    Boolean,
    Object,
    Array,
    Json,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SnippetRemoveResult {
    pub name: String,
    pub removed: bool,
}

#[must_use]
pub fn user_snippet_dir(lab_home: &Path) -> PathBuf {
    lab_home.join("snippets")
}

#[must_use]
pub fn builtin_snippet_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../docs/snippets")
}

pub fn validate_snippet_name(name: &str) -> Result<(), ToolError> {
    let mut chars = name.chars();
    let Some(first) = chars.next() else {
        return invalid_name(name);
    };
    if !first.is_ascii_lowercase() && !first.is_ascii_digit() {
        return invalid_name(name);
    }
    if !chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-' || c == '_') {
        return invalid_name(name);
    }
    Ok(())
}

fn invalid_name(name: &str) -> Result<(), ToolError> {
    Err(ToolError::InvalidParam {
        message: format!(
            "invalid snippet name `{name}`; use lowercase letters, digits, hyphens, and underscores"
        ),
        param: "name".to_string(),
    })
}

pub fn extract_javascript_block(source: &str) -> Result<String, ToolError> {
    let mut in_fence = false;
    let mut wanted = false;
    let mut body = Vec::new();

    for line in source.lines() {
        let trimmed = line.trim();
        if let Some(info) = trimmed.strip_prefix("```") {
            if in_fence {
                if wanted {
                    return Ok(body.join("\n").trim().to_string());
                }
                in_fence = false;
                wanted = false;
                body.clear();
                continue;
            }

            let language = info.split_whitespace().next().unwrap_or_default();
            in_fence = true;
            wanted = matches!(language, "js" | "javascript");
            body.clear();
            continue;
        }

        if in_fence && wanted {
            body.push(line);
        }
    }

    Err(ToolError::InvalidParam {
        message: "snippet markdown must contain a fenced ```js or ```javascript block".to_string(),
        param: "body".to_string(),
    })
}

pub fn code_for_snippet(snippet: &ResolvedSnippet) -> Result<String, ToolError> {
    let code = if snippet
        .path
        .extension()
        .and_then(|ext| ext.to_str())
        .is_some_and(|ext| ext == "js")
    {
        snippet.body.clone()
    } else if has_frontmatter(&snippet.body) || snippet.body.contains("```") {
        extract_javascript_block(&snippet.body)?
    } else {
        snippet.body.trim().to_string()
    };
    validate_snippet_code(&code)?;
    Ok(code)
}

pub fn create_user_snippet(
    lab_home: &Path,
    name: &str,
    body: &str,
    description: Option<&str>,
    force: bool,
) -> Result<SnippetInfo, ToolError> {
    validate_snippet_name(name)?;
    validate_snippet_body(name, body)?;
    let dir = user_snippet_dir(lab_home);
    fs::create_dir_all(&dir).map_err(|e| io_error("create snippets directory", &dir, e))?;
    let path = dir.join(format!("{name}.md"));
    if path.exists() && !force {
        return Err(ToolError::Conflict {
            message: format!("user snippet `{name}` already exists"),
            existing_id: name.to_string(),
        });
    }
    let body = render_user_snippet_body(name, body, description)?;
    fs::write(&path, &body).map_err(|e| io_error("write snippet", &path, e))?;
    let metadata = frontmatter(&body).ok().flatten();
    Ok(SnippetInfo {
        name: name.to_string(),
        description: metadata.as_ref().map(|m| m.description.clone()),
        tags: metadata
            .as_ref()
            .map(|m| m.tags.clone())
            .unwrap_or_default(),
        inputs: metadata.map(|m| m.inputs).unwrap_or_default(),
        source: SnippetSource::User,
        path,
        shadowed: false,
    })
}

pub fn list_snippets(lab_home: &Path, builtin_dir: &Path) -> Result<Vec<SnippetInfo>, ToolError> {
    let mut snippets = Vec::new();
    let user_dir = user_snippet_dir(lab_home);
    let user_names = collect_snippets(&user_dir, SnippetSource::User, &mut snippets)?;
    collect_snippets(builtin_dir, SnippetSource::Builtin, &mut snippets)?;

    for snippet in &mut snippets {
        snippet.shadowed =
            snippet.source == SnippetSource::Builtin && user_names.contains(&snippet.name);
    }
    snippets.sort_by(|a, b| {
        a.name
            .cmp(&b.name)
            .then_with(|| source_rank(a.source).cmp(&source_rank(b.source)))
    });
    Ok(snippets)
}

pub fn resolve_snippet(
    lab_home: &Path,
    builtin_dir: &Path,
    name: &str,
) -> Result<ResolvedSnippet, ToolError> {
    validate_snippet_name(name)?;
    let user_dir = user_snippet_dir(lab_home);
    if let Some(path) = find_snippet_file(&user_dir, name) {
        return read_resolved(name, SnippetSource::User, path);
    }
    if let Some(path) = find_snippet_file(builtin_dir, name) {
        return read_resolved(name, SnippetSource::Builtin, path);
    }
    Err(ToolError::Sdk {
        sdk_kind: "not_found".to_string(),
        message: format!("snippet `{name}` not found"),
    })
}

pub fn remove_user_snippet(
    lab_home: &Path,
    builtin_dir: &Path,
    name: &str,
) -> Result<SnippetRemoveResult, ToolError> {
    validate_snippet_name(name)?;
    let user_dir = user_snippet_dir(lab_home);
    if let Some(path) = find_snippet_file(&user_dir, name) {
        fs::remove_file(&path).map_err(|e| io_error("remove snippet", &path, e))?;
        return Ok(SnippetRemoveResult {
            name: name.to_string(),
            removed: true,
        });
    }
    if find_snippet_file(builtin_dir, name).is_some() {
        return Err(ToolError::InvalidParam {
            message: format!("snippet `{name}` is built in; only user snippets can be removed"),
            param: "name".to_string(),
        });
    }
    Err(ToolError::Sdk {
        sdk_kind: "not_found".to_string(),
        message: format!("user snippet `{name}` not found"),
    })
}

fn collect_snippets(
    dir: &Path,
    source: SnippetSource,
    out: &mut Vec<SnippetInfo>,
) -> Result<std::collections::HashSet<String>, ToolError> {
    let mut names = std::collections::HashSet::new();
    if !dir.exists() {
        return Ok(names);
    }
    let entries = fs::read_dir(dir).map_err(|e| io_error("read snippets directory", dir, e))?;
    for entry in entries {
        let entry = entry.map_err(|e| io_error("read snippets directory entry", dir, e))?;
        let path = entry.path();
        if !path.is_file() || !has_snippet_extension(&path) {
            continue;
        }
        let Some(stem) = path.file_stem().and_then(|s| s.to_str()) else {
            continue;
        };
        if validate_snippet_name(stem).is_err() {
            continue;
        }
        names.insert(stem.to_string());
        let body = match fs::read_to_string(&path) {
            Ok(body) => body,
            Err(_) => continue,
        };
        if source == SnippetSource::Builtin && frontmatter(&body).ok().flatten().is_none() {
            continue;
        }
        if validate_snippet_body(stem, &body).is_err() {
            continue;
        }
        let metadata = frontmatter(&body).ok().flatten();
        out.push(SnippetInfo {
            name: stem.to_string(),
            description: metadata.as_ref().map(|m| m.description.clone()),
            tags: metadata
                .as_ref()
                .map(|m| m.tags.clone())
                .unwrap_or_default(),
            inputs: metadata.map(|m| m.inputs).unwrap_or_default(),
            source,
            path,
            shadowed: false,
        });
    }
    Ok(names)
}

fn find_snippet_file(dir: &Path, name: &str) -> Option<PathBuf> {
    SNIPPET_EXTENSIONS
        .iter()
        .map(|ext| dir.join(format!("{name}.{ext}")))
        .find(|path| path.is_file())
}

fn has_snippet_extension(path: &Path) -> bool {
    path.extension()
        .and_then(|ext| ext.to_str())
        .is_some_and(|ext| SNIPPET_EXTENSIONS.contains(&ext))
}

fn read_resolved(
    name: &str,
    source: SnippetSource,
    path: PathBuf,
) -> Result<ResolvedSnippet, ToolError> {
    let body = fs::read_to_string(&path).map_err(|e| io_error("read snippet", &path, e))?;
    validate_snippet_body(name, &body)?;
    let metadata = frontmatter(&body)?.filter(|m| m.name == name);
    Ok(ResolvedSnippet {
        name: name.to_string(),
        description: metadata.as_ref().map(|m| m.description.clone()),
        tags: metadata
            .as_ref()
            .map(|m| m.tags.clone())
            .unwrap_or_default(),
        inputs: metadata.map(|m| m.inputs).unwrap_or_default(),
        source,
        path,
        body,
    })
}

const fn source_rank(source: SnippetSource) -> u8 {
    match source {
        SnippetSource::User => 0,
        SnippetSource::Builtin => 1,
    }
}

fn io_error(action: &str, path: &Path, error: std::io::Error) -> ToolError {
    ToolError::Sdk {
        sdk_kind: "internal_error".to_string(),
        message: format!("{action} `{}` failed: {error}", path.display()),
    }
}

pub fn validate_snippet_body(name: &str, body: &str) -> Result<(), ToolError> {
    if body.len() > MAX_SNIPPET_BYTES {
        return Err(ToolError::InvalidParam {
            message: format!("snippet body exceeds {MAX_SNIPPET_BYTES} bytes"),
            param: "body".to_string(),
        });
    }
    if let Some(metadata) = frontmatter(body)? {
        if metadata.name != name {
            return Err(ToolError::InvalidParam {
                message: format!(
                    "frontmatter name `{}` does not match snippet name `{name}`",
                    metadata.name
                ),
                param: "name".to_string(),
            });
        }
    }
    let code = if has_frontmatter(body) || body.contains("```") {
        extract_javascript_block(body)?
    } else {
        body.trim().to_string()
    };
    validate_snippet_code(&code)
}

pub fn validate_snippet_code(code: &str) -> Result<(), ToolError> {
    let code = code.trim();
    if code.is_empty() {
        return Err(ToolError::InvalidParam {
            message: "snippet code is empty".to_string(),
            param: "body".to_string(),
        });
    }
    if !(code.starts_with("async ") && code.contains("=>")) {
        return Err(ToolError::InvalidParam {
            message:
                "snippet code must be an async arrow function, e.g. async () => ({ ok: true })"
                    .to_string(),
            param: "body".to_string(),
        });
    }
    Ok(())
}

pub fn frontmatter(body: &str) -> Result<Option<SnippetFrontmatter>, ToolError> {
    let Some(rest) = body.strip_prefix("---\n") else {
        return Ok(None);
    };
    let Some(raw) = frontmatter_block(rest) else {
        return Err(ToolError::InvalidParam {
            message: "snippet frontmatter starts with --- but is not closed".to_string(),
            param: "body".to_string(),
        });
    };
    let mut name = None;
    let mut description = None;
    let mut tags = Vec::new();
    let lines: Vec<&str> = raw.lines().collect();
    let mut inputs = BTreeMap::new();
    let mut i = 0;
    while i < lines.len() {
        let raw_line = lines[i];
        if raw_line.starts_with("  ") {
            i += 1;
            continue;
        }
        let line = raw_line.trim();
        if line == "inputs:" {
            let (parsed, next) = parse_inputs_block(&lines, i + 1)?;
            inputs = parsed;
            i = next;
            continue;
        }
        let line = line.trim();
        if line.is_empty() {
            i += 1;
            continue;
        }
        let Some((key, value)) = line.split_once(':') else {
            return Err(ToolError::InvalidParam {
                message: format!("invalid frontmatter line `{line}`"),
                param: "body".to_string(),
            });
        };
        let value = value.trim().trim_matches('"');
        match key.trim() {
            "name" => name = Some(value.to_string()),
            "description" => description = Some(value.to_string()),
            "tags" => tags = parse_tags(value)?,
            _ => {}
        }
        i += 1;
    }
    let name = required_frontmatter_field(name, "name")?;
    let description = required_frontmatter_field(description, "description")?;
    Ok(Some(SnippetFrontmatter {
        name,
        description,
        tags,
        inputs,
    }))
}

fn frontmatter_block(rest: &str) -> Option<String> {
    let mut raw = Vec::new();
    for line in rest.lines() {
        if line.trim_end_matches('\r') == "---" {
            return Some(raw.join("\n"));
        }
        raw.push(line.trim_end_matches('\r'));
    }
    None
}

fn has_frontmatter(body: &str) -> bool {
    body.starts_with("---\n")
}

fn required_frontmatter_field(value: Option<String>, field: &str) -> Result<String, ToolError> {
    let Some(value) = value.filter(|v| !v.trim().is_empty()) else {
        return Err(ToolError::InvalidParam {
            message: format!("snippet frontmatter requires `{field}`"),
            param: "body".to_string(),
        });
    };
    Ok(value)
}

fn parse_tags(value: &str) -> Result<Vec<String>, ToolError> {
    let value = value.trim();
    if value.is_empty() || value == "[]" {
        return Ok(Vec::new());
    }
    let Some(inner) = value.strip_prefix('[').and_then(|v| v.strip_suffix(']')) else {
        return Err(ToolError::InvalidParam {
            message: "frontmatter `tags` must be an inline array".to_string(),
            param: "body".to_string(),
        });
    };
    Ok(inner
        .split(',')
        .map(|tag| tag.trim().trim_matches('"').to_string())
        .filter(|tag| !tag.is_empty())
        .collect())
}

fn render_user_snippet_body(
    name: &str,
    body: &str,
    description: Option<&str>,
) -> Result<String, ToolError> {
    if has_frontmatter(body) {
        return Ok(body.to_string());
    }
    let description = description
        .filter(|value| !value.trim().is_empty())
        .map(sanitize_frontmatter_scalar)
        .unwrap_or_else(|| "User snippet".to_string());
    let code = if body.contains("```") {
        extract_javascript_block(body)?
    } else {
        body.trim().to_string()
    };
    validate_snippet_code(&code)?;
    Ok(format!(
        "---\nname: {name}\ndescription: {description}\ntags: []\n---\n\n```js\n{code}\n```\n"
    ))
}

fn sanitize_frontmatter_scalar(value: &str) -> String {
    let sanitized = value.split_whitespace().collect::<Vec<_>>().join(" ");
    if sanitized.is_empty() {
        "User snippet".to_string()
    } else {
        sanitized.replace('"', "'")
    }
}

pub fn merge_snippet_input(snippet: &ResolvedSnippet, caller: Value) -> Result<Value, ToolError> {
    let caller = match caller {
        Value::Null => Value::Object(Map::new()),
        Value::Object(map) => Value::Object(map),
        _ => {
            return Err(ToolError::InvalidParam {
                message: "snippet params must be a JSON object".to_string(),
                param: "params".to_string(),
            });
        }
    };

    if snippet.inputs.is_empty() {
        return Ok(caller);
    }

    let caller = caller.as_object().expect("caller normalized to object");
    for key in caller.keys() {
        if !snippet.inputs.contains_key(key) {
            return Err(ToolError::InvalidParam {
                message: format!("unknown snippet input `{key}`"),
                param: format!("params.{key}"),
            });
        }
    }

    let mut merged = Map::new();
    for (name, spec) in &snippet.inputs {
        let value = caller
            .get(name)
            .cloned()
            .or_else(|| spec.default.clone())
            .or_else(|| (!spec.required).then_some(Value::Null));
        let Some(value) = value else {
            return Err(ToolError::MissingParam {
                message: format!("missing required snippet input `{name}`"),
                param: format!("params.{name}"),
            });
        };
        if !value.is_null() {
            validate_input_type(name, spec.ty, &value)?;
        }
        merged.insert(name.clone(), value);
    }

    Ok(Value::Object(merged))
}

fn parse_inputs_block(
    lines: &[&str],
    mut i: usize,
) -> Result<(BTreeMap<String, SnippetInputSpec>, usize), ToolError> {
    let mut inputs = BTreeMap::new();
    while i < lines.len() {
        let line = lines[i];
        if line.trim().is_empty() {
            i += 1;
            continue;
        }
        if !line.starts_with("  ") {
            break;
        }
        if line.starts_with("    ") {
            return Err(ToolError::InvalidParam {
                message: format!("invalid input declaration line `{}`", line.trim()),
                param: "body".to_string(),
            });
        }
        let Some(input_name) = line.trim().strip_suffix(':') else {
            return Err(ToolError::InvalidParam {
                message: format!("invalid input declaration line `{}`", line.trim()),
                param: "body".to_string(),
            });
        };
        validate_snippet_name(input_name)?;
        i += 1;

        let mut ty = None;
        let mut required = false;
        let mut default = None;
        let mut description = None;
        while i < lines.len() {
            let field_line = lines[i];
            if field_line.trim().is_empty() {
                i += 1;
                continue;
            }
            if !field_line.starts_with("    ") {
                break;
            }
            let Some((key, value)) = field_line.trim().split_once(':') else {
                return Err(ToolError::InvalidParam {
                    message: format!("invalid input field line `{}`", field_line.trim()),
                    param: "body".to_string(),
                });
            };
            let value = value.trim().trim_matches('"');
            match key.trim() {
                "type" => ty = Some(parse_input_type(value)?),
                "required" => required = parse_bool(value, "required")?,
                "default" => default = Some(parse_default_value(value)),
                "description" => description = Some(value.to_string()),
                _ => {}
            }
            i += 1;
        }

        let ty = ty.ok_or_else(|| ToolError::InvalidParam {
            message: format!("snippet input `{input_name}` requires `type`"),
            param: "body".to_string(),
        })?;
        if let Some(default_value) = &default {
            validate_input_type(input_name, ty, default_value)?;
        }
        inputs.insert(
            input_name.to_string(),
            SnippetInputSpec {
                ty,
                required,
                default,
                description,
            },
        );
    }
    Ok((inputs, i))
}

fn parse_input_type(value: &str) -> Result<SnippetInputType, ToolError> {
    match value {
        "string" => Ok(SnippetInputType::String),
        "integer" => Ok(SnippetInputType::Integer),
        "number" => Ok(SnippetInputType::Number),
        "boolean" => Ok(SnippetInputType::Boolean),
        "object" => Ok(SnippetInputType::Object),
        "array" => Ok(SnippetInputType::Array),
        "json" => Ok(SnippetInputType::Json),
        _ => Err(ToolError::InvalidParam {
            message: format!("unsupported snippet input type `{value}`"),
            param: "body".to_string(),
        }),
    }
}

fn parse_bool(value: &str, field: &str) -> Result<bool, ToolError> {
    match value {
        "true" => Ok(true),
        "false" => Ok(false),
        _ => Err(ToolError::InvalidParam {
            message: format!("frontmatter `{field}` must be true or false"),
            param: "body".to_string(),
        }),
    }
}

fn parse_default_value(value: &str) -> Value {
    let value = value.trim();
    if value.eq_ignore_ascii_case("true") {
        return Value::Bool(true);
    }
    if value.eq_ignore_ascii_case("false") {
        return Value::Bool(false);
    }
    if let Ok(n) = value.parse::<i64>()
        && n.to_string() == value
    {
        return Value::Number(n.into());
    }
    if let Ok(n) = value.parse::<f64>()
        && let Some(num) = serde_json::Number::from_f64(n)
        && n.to_string() == value
    {
        return Value::Number(num);
    }
    if let Ok(json) = serde_json::from_str::<Value>(value)
        && (json.is_object() || json.is_array())
    {
        return json;
    }
    Value::String(value.trim_matches('"').to_string())
}

fn validate_input_type(name: &str, ty: SnippetInputType, value: &Value) -> Result<(), ToolError> {
    let ok = match ty {
        SnippetInputType::String => value.is_string(),
        SnippetInputType::Integer => value.as_i64().is_some(),
        SnippetInputType::Number => value.is_number(),
        SnippetInputType::Boolean => value.is_boolean(),
        SnippetInputType::Object => value.is_object(),
        SnippetInputType::Array => value.is_array(),
        SnippetInputType::Json => true,
    };
    if ok {
        return Ok(());
    }
    Err(ToolError::InvalidParam {
        message: format!("snippet input `{name}` has wrong type; expected {ty:?}"),
        param: format!("params.{name}"),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn valid_body() -> &'static str {
        "---\nname: demo\ndescription: Demo snippet\ntags: []\n---\n\n```js\nasync () => ({ ok: true })\n```\n"
    }

    #[test]
    fn generated_frontmatter_description_is_single_line() {
        let body = render_user_snippet_body(
            "demo",
            "async () => ({ ok: true })",
            Some("first line\nname: injected\n---\nsecond line"),
        )
        .expect("rendered");

        let metadata = frontmatter(&body)
            .expect("frontmatter parsed")
            .expect("metadata");
        assert_eq!(metadata.name, "demo");
        assert_eq!(
            metadata.description,
            "first line name: injected --- second line"
        );
        assert!(validate_snippet_body("demo", &body).is_ok());
    }

    #[test]
    fn frontmatter_requires_exact_closing_delimiter_line() {
        let body = "---\nname: demo\ndescription: Demo snippet\n--- trailing\n\n```js\nasync () => ({ ok: true })\n```\n";

        let error = frontmatter(body).expect_err("loose delimiter should be rejected");
        assert!(format!("{error}").contains("not closed"));
    }

    #[test]
    fn validate_snippet_body_accepts_valid_frontmatter() {
        assert!(validate_snippet_body("demo", valid_body()).is_ok());
    }

    #[test]
    fn repo_status_gh_pulse_builtin_is_discoverable_and_executable() {
        let lab_home = tempfile::tempdir().expect("temp lab home");
        let builtin_dir = builtin_snippet_dir();
        let snippets = list_snippets(lab_home.path(), &builtin_dir).expect("list snippets");
        let info = snippets
            .iter()
            .find(|snippet| snippet.name == "repo-status-gh-pulse")
            .expect("repo-status-gh-pulse listed");

        assert_eq!(info.source, SnippetSource::Builtin);
        assert!(info.inputs.contains_key("owner"));
        assert!(info.inputs.contains_key("repo"));
        assert!(info.inputs.contains_key("include_workflow_runs"));

        let resolved = resolve_snippet(lab_home.path(), &builtin_dir, "repo-status-gh-pulse")
            .expect("resolve builtin snippet");
        let code = code_for_snippet(&resolved).expect("extract executable code");

        assert!(code.contains("github::search_issues"));
        assert!(!code.contains("github::list_workflow_runs"));
    }

    #[test]
    fn merge_snippet_input_rejects_unknown_declared_inputs() {
        let body = "---\nname: demo\ndescription: Demo snippet\ninputs:\n  host:\n    type: string\n    default: dookie\n---\n\n```js\nasync (input) => input\n```\n";
        let metadata = frontmatter(body)
            .expect("frontmatter parsed")
            .expect("metadata");
        let snippet = ResolvedSnippet {
            name: "demo".to_string(),
            description: Some(metadata.description),
            tags: metadata.tags,
            inputs: metadata.inputs,
            source: SnippetSource::User,
            path: PathBuf::from("demo.md"),
            body: body.to_string(),
        };

        let error = merge_snippet_input(&snippet, json!({"bogus": true}))
            .expect_err("unknown input should be rejected");
        assert!(format!("{error}").contains("unknown snippet input"));
    }
}
