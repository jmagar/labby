use std::collections::{BTreeSet, HashSet};

use serde_json::Value;

use super::code_mode::upstream_tool_id;
use super::code_mode_preamble::tool_name_to_snake;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolTypes {
    pub signature: String,
    pub dts: String,
}

pub fn generate_tool_types(
    upstream: &str,
    tool: &str,
    description: &str,
    input_schema: Option<&Value>,
    output_schema: Option<&Value>,
) -> ToolTypes {
    let base = format!(
        "{}{}",
        to_pascal_identifier(upstream),
        to_pascal_identifier(tool)
    );
    let input_name = format!("{base}Input");
    let output_name = format!("{base}Output");
    let upstream_method = tool_name_to_snake(upstream);
    let tool_method = tool_name_to_snake(tool);
    let tool_id = upstream_tool_id(upstream, tool);
    let tool_id_literal = serde_json::to_string(&tool_id).unwrap_or_else(|_| "\"\"".to_string());
    let input_type = json_schema_to_type(input_schema);
    let output_type = json_schema_to_type(output_schema);
    let signature = format!(
        "codemode.{upstream_method}.{tool_method}(params: {input_name}): Promise<{output_name}>"
    );

    let mut dts = String::new();
    dts.push_str(&format!("type {input_name} = {input_type};\n"));
    dts.push_str(&format!("type {output_name} = {output_type};\n"));
    dts.push_str("declare const codemode: {\n");
    dts.push_str(&format!("  {upstream_method}: {{\n"));
    if let Some(comment) = jsdoc_block(description, 4) {
        dts.push_str(&comment);
    }
    dts.push_str(&format!(
        "    {tool_method}(params: {input_name}): Promise<{output_name}>;\n"
    ));
    dts.push_str("  };\n");
    dts.push_str("};\n");
    dts.push_str(&format!(
        "declare function callTool(id: {tool_id_literal}, params: {input_name}): Promise<{output_name}>;\n"
    ));

    ToolTypes { signature, dts }
}

pub fn json_schema_to_type(schema: Option<&Value>) -> String {
    let Some(schema) = schema else {
        return "unknown".to_string();
    };
    let mut seen_refs = HashSet::new();
    schema_to_type(schema, schema, 0, &mut seen_refs)
}

fn schema_to_type(
    schema: &Value,
    root: &Value,
    depth: usize,
    seen_refs: &mut HashSet<String>,
) -> String {
    if let Some(value) = schema.as_bool() {
        return if value { "unknown" } else { "never" }.to_string();
    }
    if depth > 20 {
        return "unknown".to_string();
    }
    let Some(object) = schema.as_object() else {
        return "unknown".to_string();
    };

    if let Some(reference) = object.get("$ref").and_then(Value::as_str) {
        if !seen_refs.insert(reference.to_string()) {
            return "unknown".to_string();
        }
        let resolved = resolve_ref(root, reference)
            .map(|schema| schema_to_type(schema, root, depth + 1, seen_refs))
            .unwrap_or_else(|| "unknown".to_string());
        seen_refs.remove(reference);
        return resolved;
    }

    if let Some(values) = object.get("anyOf").and_then(Value::as_array) {
        return union(
            values
                .iter()
                .map(|v| schema_to_type(v, root, depth + 1, seen_refs)),
        );
    }
    if let Some(values) = object.get("oneOf").and_then(Value::as_array) {
        return union(
            values
                .iter()
                .map(|v| schema_to_type(v, root, depth + 1, seen_refs)),
        );
    }
    if let Some(values) = object.get("allOf").and_then(Value::as_array) {
        return intersection(
            values
                .iter()
                .map(|v| schema_to_type(v, root, depth + 1, seen_refs)),
        );
    }

    if let Some(value) = object.get("const") {
        return literal_type(value);
    }
    if let Some(values) = object.get("enum").and_then(Value::as_array) {
        return union(values.iter().map(literal_type));
    }

    let mut rendered = match object.get("type") {
        Some(Value::Array(types)) => union(types.iter().map(|value| {
            value
                .as_str()
                .map(|kind| schema_type_to_type(kind, schema, root, depth, seen_refs))
                .unwrap_or_else(|| "unknown".to_string())
        })),
        Some(Value::String(kind)) => schema_type_to_type(kind, schema, root, depth, seen_refs),
        _ if object.contains_key("properties") || object.contains_key("additionalProperties") => {
            object_type(schema, root, depth, seen_refs)
        }
        _ if object.contains_key("items") || object.contains_key("prefixItems") => {
            array_type(schema, root, depth, seen_refs)
        }
        _ => "unknown".to_string(),
    };

    if object
        .get("nullable")
        .and_then(Value::as_bool)
        .unwrap_or(false)
        && !rendered.split('|').any(|part| part.trim() == "null")
    {
        rendered.push_str(" | null");
    }

    rendered
}

fn schema_type_to_type(
    kind: &str,
    schema: &Value,
    root: &Value,
    depth: usize,
    seen_refs: &mut HashSet<String>,
) -> String {
    match kind {
        "object" => object_type(schema, root, depth, seen_refs),
        "array" => array_type(schema, root, depth, seen_refs),
        "string" => "string".to_string(),
        "integer" | "number" => "number".to_string(),
        "boolean" => "boolean".to_string(),
        "null" => "null".to_string(),
        _ => "unknown".to_string(),
    }
}

fn object_type(
    schema: &Value,
    root: &Value,
    depth: usize,
    seen_refs: &mut HashSet<String>,
) -> String {
    let Some(object) = schema.as_object() else {
        return "Record<string, unknown>".to_string();
    };

    let required = object
        .get("required")
        .and_then(Value::as_array)
        .map(|values| {
            values
                .iter()
                .filter_map(Value::as_str)
                .collect::<BTreeSet<_>>()
        })
        .unwrap_or_default();

    let mut lines = Vec::new();
    if let Some(properties) = object.get("properties").and_then(Value::as_object) {
        for key in properties.keys() {
            let property = &properties[key];
            if let Some(comment) = property_jsdoc(property, 2) {
                lines.push(comment.trim_end().to_string());
            }
            let optional = if required.contains(key.as_str()) {
                ""
            } else {
                "?"
            };
            lines.push(format!(
                "  {}{}: {};",
                quote_prop(key),
                optional,
                schema_to_type(property, root, depth + 1, seen_refs)
            ));
        }
    }

    match object.get("additionalProperties") {
        Some(Value::Object(_)) => lines.push(format!(
            "  [key: string]: {};",
            schema_to_type(&object["additionalProperties"], root, depth + 1, seen_refs)
        )),
        Some(Value::Bool(true)) => lines.push("  [key: string]: unknown;".to_string()),
        Some(Value::Bool(false)) => {}
        _ => {}
    }

    if lines.is_empty() {
        if object.get("additionalProperties").and_then(Value::as_bool) == Some(false) {
            return "{}".to_string();
        }
        return "Record<string, unknown>".to_string();
    }

    format!("{{\n{}\n}}", lines.join("\n"))
}

fn array_type(
    schema: &Value,
    root: &Value,
    depth: usize,
    seen_refs: &mut HashSet<String>,
) -> String {
    let Some(object) = schema.as_object() else {
        return "unknown[]".to_string();
    };

    if let Some(items) = object.get("prefixItems").and_then(Value::as_array) {
        let items = items
            .iter()
            .map(|item| schema_to_type(item, root, depth + 1, seen_refs))
            .collect::<Vec<_>>()
            .join(", ");
        return format!("[{items}]");
    }

    if let Some(items) = object.get("items").and_then(Value::as_array) {
        let items = items
            .iter()
            .map(|item| schema_to_type(item, root, depth + 1, seen_refs))
            .collect::<Vec<_>>()
            .join(", ");
        return format!("[{items}]");
    }

    let item_type = object
        .get("items")
        .map(|items| schema_to_type(items, root, depth + 1, seen_refs))
        .unwrap_or_else(|| "unknown".to_string());
    format!("Array<{item_type}>")
}

fn resolve_ref<'a>(root: &'a Value, reference: &str) -> Option<&'a Value> {
    reference
        .strip_prefix('#')
        .and_then(|pointer| root.pointer(pointer))
}

fn union(types: impl Iterator<Item = String>) -> String {
    let mut seen = BTreeSet::new();
    let types = types
        .filter(|ty| seen.insert(ty.clone()))
        .collect::<Vec<_>>();
    if types.is_empty() {
        "unknown".to_string()
    } else {
        types.join(" | ")
    }
}

fn intersection(types: impl Iterator<Item = String>) -> String {
    let types = types.collect::<Vec<_>>();
    if types.is_empty() {
        "unknown".to_string()
    } else {
        types.join(" & ")
    }
}

fn literal_type(value: &Value) -> String {
    match value {
        Value::String(text) => serde_json::to_string(text).unwrap_or_else(|_| "string".to_string()),
        Value::Number(number) => number.to_string(),
        Value::Bool(value) => value.to_string(),
        Value::Null => "null".to_string(),
        Value::Array(_) | Value::Object(_) => {
            serde_json::to_string(value).unwrap_or_else(|_| "unknown".to_string())
        }
    }
}

fn quote_prop(key: &str) -> String {
    if is_identifier(key) {
        key.to_string()
    } else {
        serde_json::to_string(key).unwrap_or_else(|_| "\"\"".to_string())
    }
}

fn is_identifier(value: &str) -> bool {
    let mut chars = value.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    (first == '_' || first == '$' || first.is_ascii_alphabetic())
        && chars.all(|ch| ch == '_' || ch == '$' || ch.is_ascii_alphanumeric())
}

fn jsdoc_block(text: &str, indent: usize) -> Option<String> {
    let text = escape_jsdoc(text.trim());
    if text.is_empty() {
        return None;
    }
    let pad = " ".repeat(indent);
    Some(format!("{pad}/** {text} */\n"))
}

fn property_jsdoc(schema: &Value, indent: usize) -> Option<String> {
    let object = schema.as_object()?;
    let description = object.get("description").and_then(Value::as_str);
    let format = object.get("format").and_then(Value::as_str);
    if description.is_none() && format.is_none() {
        return None;
    }
    if let (Some(description), None) = (description, format) {
        return jsdoc_block(description, indent);
    }
    let pad = " ".repeat(indent);
    let mut lines = Vec::new();
    lines.push(format!("{pad}/**"));
    if let Some(description) = description {
        lines.push(format!("{pad} * {}", escape_jsdoc(description.trim())));
    }
    if let Some(format) = format {
        lines.push(format!("{pad} * @format {}", escape_jsdoc(format.trim())));
    }
    lines.push(format!("{pad} */\n"));
    Some(lines.join("\n"))
}

fn escape_jsdoc(text: &str) -> String {
    text.replace("*/", "* /").replace('\n', " ")
}

fn to_pascal_identifier(value: &str) -> String {
    let mut out = String::new();
    for segment in value
        .split(|ch: char| !ch.is_ascii_alphanumeric())
        .filter(|segment| !segment.is_empty())
    {
        let mut chars = segment.chars();
        if let Some(first) = chars.next() {
            out.push(first.to_ascii_uppercase());
            out.extend(chars.map(|ch| ch.to_ascii_lowercase()));
        }
    }
    if out.is_empty() {
        "Tool".to_string()
    } else if out.starts_with(|ch: char| ch.is_ascii_digit()) {
        format!("_{out}")
    } else {
        out
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    #[test]
    fn json_schema_to_type_handles_refs_unions_arrays_and_required_properties() {
        let schema = json!({
            "$defs": {
                "Issue": {
                    "type": "object",
                    "properties": {
                        "title": { "type": "string" },
                        "number": { "type": "integer" }
                    },
                    "required": ["title"]
                }
            },
            "type": "object",
            "properties": {
                "items": {
                    "type": "array",
                    "items": { "$ref": "#/$defs/Issue" }
                },
                "state": {
                    "enum": ["open", "closed"]
                },
                "cursor": {
                    "anyOf": [{ "type": "string" }, { "type": "null" }]
                }
            },
            "required": ["items"]
        });

        let ts = super::json_schema_to_type(Some(&schema));

        assert!(ts.contains("items: Array<{"));
        assert!(ts.contains("title: string;"));
        assert!(ts.contains("number?: number;"));
        assert!(ts.contains("state?: \"open\" | \"closed\";"));
        assert!(ts.contains("cursor?: string | null;"));
    }

    #[test]
    fn json_schema_to_type_matches_cloudflare_edge_cases() {
        let schema = json!({
            "type": "object",
            "properties": {
                "tuple": {
                    "type": "array",
                    "items": [{ "type": "string" }, { "type": "integer" }]
                },
                "exact": {
                    "type": "object",
                    "additionalProperties": false
                },
                "when": {
                    "type": "string",
                    "format": "date-time",
                    "description": "Timestamp"
                },
                "anything": true,
                "nothing": false
            }
        });

        let ts = super::json_schema_to_type(Some(&schema));

        assert!(ts.contains("tuple?: [string, number];"), "{ts}");
        assert!(ts.contains("exact?: {};"), "{ts}");
        assert!(ts.contains("* Timestamp"), "{ts}");
        assert!(ts.contains("* @format date-time"), "{ts}");
        assert!(ts.contains("anything?: unknown;"), "{ts}");
        assert!(ts.contains("nothing?: never;"), "{ts}");
    }

    #[test]
    fn generate_tool_types_quotes_sanitized_namespace_and_method_names() {
        let types = super::generate_tool_types(
            "github chat",
            "list tags",
            "List tags",
            Some(&json!({"type": "object"})),
            None,
        );

        assert!(
            types.signature.contains("codemode.github_chat.list_tags"),
            "{types:?}"
        );
        assert!(types.dts.contains("github_chat"), "{types:?}");
        assert!(types.dts.contains("list_tags(params:"), "{types:?}");
        assert!(!types.dts.contains("github chat: {"), "{types:?}");
        assert!(!types.dts.contains("list tags(params:"), "{types:?}");
    }
}
