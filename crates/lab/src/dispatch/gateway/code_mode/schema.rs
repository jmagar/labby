//! JSON Schema validation for Code Mode callTool params and upstream-result unwrapping.

use std::collections::{BTreeSet, HashSet};

use rmcp::model::CallToolResult;
use serde_json::{Map, Value, json};

use crate::dispatch::error::ToolError;

/// Serialize `value` with object keys sorted recursively, so two `Value`-equal
/// inputs always produce the same string. Used to key `uniqueItems` dedup in a
/// `HashSet` without depending on object key insertion order (serde_json runs
/// with `preserve_order` in this crate).
fn canonical_json(value: &Value) -> String {
    fn write(value: &Value, out: &mut String) {
        match value {
            Value::Object(map) => {
                out.push('{');
                let mut keys: Vec<&String> = map.keys().collect();
                keys.sort_unstable();
                for (index, key) in keys.into_iter().enumerate() {
                    if index > 0 {
                        out.push(',');
                    }
                    // Quote the key via serde so it is escaped identically to a
                    // normal JSON string.
                    out.push_str(&Value::String(key.clone()).to_string());
                    out.push(':');
                    write(&map[key], out);
                }
                out.push('}');
            }
            Value::Array(items) => {
                out.push('[');
                for (index, item) in items.iter().enumerate() {
                    if index > 0 {
                        out.push(',');
                    }
                    write(item, out);
                }
                out.push(']');
            }
            // Scalars serialize unambiguously; reuse serde for correct escaping
            // and number formatting.
            other => out.push_str(&other.to_string()),
        }
    }
    let mut out = String::new();
    write(value, &mut out);
    out
}

pub(in crate::dispatch::gateway::code_mode) fn validate_code_mode_params_against_schema(
    params: &Value,
    schema: Option<&Value>,
) -> Result<(), ToolError> {
    if let Some(schema) = schema {
        validate_json_schema_value(params, schema, "params")?;
    }
    Ok(())
}

fn json_value_matches_schema_type(value: &Value, expected: &str) -> bool {
    match expected {
        "string" => value.is_string(),
        "integer" => value.as_i64().is_some() || value.as_u64().is_some(),
        "number" => value.is_number(),
        "boolean" => value.is_boolean(),
        "object" => value.is_object(),
        "array" => value.is_array(),
        "null" => value.is_null(),
        _ => true,
    }
}

fn validate_json_schema_value(value: &Value, schema: &Value, path: &str) -> Result<(), ToolError> {
    let mut seen_refs = BTreeSet::new();
    validate_json_schema_value_inner(value, schema, schema, path, &mut seen_refs)
}

fn validate_json_schema_value_inner(
    value: &Value,
    schema: &Value,
    root_schema: &Value,
    path: &str,
    seen_refs: &mut BTreeSet<String>,
) -> Result<(), ToolError> {
    let Some(schema_object) = schema.as_object() else {
        return Ok(());
    };

    if let Some(reference) = schema_object.get("$ref").and_then(Value::as_str) {
        let pointer = reference.strip_prefix('#').ok_or_else(|| {
            invalid_schema_param(path, "uses an unsupported non-local $ref in inputSchema")
        })?;
        if !seen_refs.insert(reference.to_string()) {
            return Err(invalid_schema_param(
                path,
                "contains a cyclic $ref in inputSchema",
            ));
        }
        let referenced_schema = root_schema.pointer(pointer).ok_or_else(|| {
            invalid_schema_param(path, "uses an unresolved local $ref in inputSchema")
        })?;
        validate_json_schema_value_inner(value, referenced_schema, root_schema, path, seen_refs)?;
        seen_refs.remove(reference);
    }

    if let Some(values) = schema_object.get("enum").and_then(Value::as_array)
        && !values.iter().any(|candidate| candidate == value)
    {
        return Err(invalid_schema_param(path, "must match enum"));
    }
    if let Some(const_value) = schema_object.get("const")
        && const_value != value
    {
        return Err(invalid_schema_param(path, "must match const"));
    }

    if let Some(variants) = schema_object.get("anyOf").and_then(Value::as_array) {
        if !variants.iter().any(|variant| {
            validate_json_schema_value_inner(
                value,
                variant,
                root_schema,
                path,
                &mut seen_refs.clone(),
            )
            .is_ok()
        }) {
            return Err(invalid_schema_param(path, "must match at least one schema"));
        }
    }
    if let Some(variants) = schema_object.get("oneOf").and_then(Value::as_array) {
        let matches = variants
            .iter()
            .filter(|variant| {
                validate_json_schema_value_inner(
                    value,
                    variant,
                    root_schema,
                    path,
                    &mut seen_refs.clone(),
                )
                .is_ok()
            })
            .count();
        if matches != 1 {
            return Err(invalid_schema_param(path, "must match exactly one schema"));
        }
    }
    if let Some(variants) = schema_object.get("allOf").and_then(Value::as_array) {
        for variant in variants {
            validate_json_schema_value_inner(value, variant, root_schema, path, seen_refs)?;
        }
    }

    if let Some(type_value) = schema_object.get("type") {
        let matches_type = match type_value {
            Value::String(expected) => {
                json_value_matches_schema_type(value, expected)
                    || schema_accepts_binary_sentinel(value, schema_object, expected)
            }
            Value::Array(types) => types.iter().filter_map(Value::as_str).any(|expected| {
                json_value_matches_schema_type(value, expected)
                    || schema_accepts_binary_sentinel(value, schema_object, expected)
            }),
            _ => true,
        };
        if !matches_type {
            return Err(invalid_schema_param(path, "has wrong type"));
        }
    }

    if let Some(minimum) = schema_object.get("minimum").and_then(Value::as_f64)
        && value.as_f64().is_some_and(|actual| actual < minimum)
    {
        return Err(invalid_schema_param(path, "is below minimum"));
    }
    if let Some(maximum) = schema_object.get("maximum").and_then(Value::as_f64)
        && value.as_f64().is_some_and(|actual| actual > maximum)
    {
        return Err(invalid_schema_param(path, "is above maximum"));
    }

    if let Some(actual) = value.as_str() {
        if let Some(min_length) = schema_object.get("minLength").and_then(Value::as_u64)
            && actual.chars().count() < min_length as usize
        {
            return Err(invalid_schema_param(path, "is shorter than minLength"));
        }
        if let Some(max_length) = schema_object.get("maxLength").and_then(Value::as_u64)
            && actual.chars().count() > max_length as usize
        {
            return Err(invalid_schema_param(path, "is longer than maxLength"));
        }
        if let Some(pattern) = schema_object.get("pattern").and_then(Value::as_str) {
            let regex = regex::Regex::new(pattern)
                .map_err(|_| invalid_schema_param(path, "has an invalid pattern in inputSchema"))?;
            if !regex.is_match(actual) {
                return Err(invalid_schema_param(path, "does not match pattern"));
            }
        }
    }

    if let Some(object) = value.as_object() {
        if let Some(required) = schema_object.get("required").and_then(Value::as_array) {
            for key in required.iter().filter_map(Value::as_str) {
                if !object.contains_key(key) {
                    return Err(if path == "params" {
                        ToolError::Sdk {
                            sdk_kind: "missing_param".to_string(),
                            message: format!("callTool params missing required field `{key}`"),
                        }
                    } else {
                        invalid_schema_param(&format!("{path}.{key}"), "is required")
                    });
                }
            }
        }
        let properties = schema_object.get("properties").and_then(Value::as_object);
        let pattern_properties = schema_object
            .get("patternProperties")
            .and_then(Value::as_object);
        let mut matched_pattern_keys = BTreeSet::new();
        if let Some(pattern_properties) = pattern_properties {
            for (pattern, pattern_schema) in pattern_properties {
                let regex = regex::Regex::new(pattern).map_err(|_| {
                    invalid_schema_param(
                        path,
                        "has an invalid patternProperties key in inputSchema",
                    )
                })?;
                for (key, property_value) in object {
                    if regex.is_match(key) {
                        matched_pattern_keys.insert(key.clone());
                        validate_json_schema_value_inner(
                            property_value,
                            pattern_schema,
                            root_schema,
                            &format!("{path}.{key}"),
                            seen_refs,
                        )?;
                    }
                }
            }
        }
        let additional_properties = schema_object.get("additionalProperties");
        if additional_properties.and_then(Value::as_bool) == Some(false) {
            for key in object.keys() {
                if properties.is_none_or(|properties| !properties.contains_key(key))
                    && !matched_pattern_keys.contains(key)
                {
                    return Err(invalid_schema_param(
                        &format!("{path}.{key}"),
                        "is not allowed by inputSchema",
                    ));
                }
            }
        }
        if let Some(properties) = properties {
            for (key, property_schema) in properties {
                if let Some(property_value) = object.get(key) {
                    validate_json_schema_value_inner(
                        property_value,
                        property_schema,
                        root_schema,
                        &format!("{path}.{key}"),
                        seen_refs,
                    )?;
                }
            }
        }
        if let Some(additional_schema) = additional_properties.filter(|value| value.is_object()) {
            for (key, property_value) in object {
                if properties.is_some_and(|properties| properties.contains_key(key))
                    || matched_pattern_keys.contains(key)
                {
                    continue;
                }
                validate_json_schema_value_inner(
                    property_value,
                    additional_schema,
                    root_schema,
                    &format!("{path}.{key}"),
                    seen_refs,
                )?;
            }
        }
    }

    if let Some(array) = value.as_array() {
        if let Some(min_items) = schema_object.get("minItems").and_then(Value::as_u64)
            && array.len() < min_items as usize
        {
            return Err(invalid_schema_param(path, "has fewer items than minItems"));
        }
        if let Some(max_items) = schema_object.get("maxItems").and_then(Value::as_u64)
            && array.len() > max_items as usize
        {
            return Err(invalid_schema_param(path, "has more items than maxItems"));
        }
        if schema_object
            .get("uniqueItems")
            .and_then(Value::as_bool)
            .unwrap_or(false)
        {
            // O(n) dedup via a set of canonical (sorted-key) serializations.
            // Canonicalization makes the string key match `Value` equality even
            // though `serde_json` preserves object key insertion order, so this
            // is semantically identical to the previous O(n²) `right == left`
            // pairwise scan (same accept/reject, same error).
            let mut seen = HashSet::with_capacity(array.len());
            for item in array {
                if !seen.insert(canonical_json(item)) {
                    return Err(invalid_schema_param(path, "must contain unique items"));
                }
            }
        }
        if let Some(items) = schema_object.get("items") {
            if let Some(tuple_items) = items.as_array() {
                for (index, item_schema) in tuple_items.iter().enumerate() {
                    if let Some(item_value) = array.get(index) {
                        validate_json_schema_value_inner(
                            item_value,
                            item_schema,
                            root_schema,
                            &format!("{path}[{index}]"),
                            seen_refs,
                        )?;
                    }
                }
            } else {
                for (index, item_value) in array.iter().enumerate() {
                    validate_json_schema_value_inner(
                        item_value,
                        items,
                        root_schema,
                        &format!("{path}[{index}]"),
                        seen_refs,
                    )?;
                }
            }
        }
    }

    Ok(())
}

fn schema_accepts_binary_sentinel(
    value: &Value,
    schema_object: &Map<String, Value>,
    expected_type: &str,
) -> bool {
    expected_type == "string"
        && schema_object.get("format").and_then(Value::as_str) == Some("binary")
        && is_lab_binary_sentinel(value)
}

fn is_lab_binary_sentinel(value: &Value) -> bool {
    let Some(object) = value.as_object() else {
        return false;
    };
    object.get("__labBinary").and_then(Value::as_str) == Some("base64")
        && object.get("data").and_then(Value::as_str).is_some()
        && matches!(
            object.get("type").and_then(Value::as_str),
            Some("Uint8Array" | "ArrayBuffer")
        )
}

fn invalid_schema_param(path: &str, detail: &str) -> ToolError {
    ToolError::Sdk {
        sdk_kind: "invalid_param".to_string(),
        message: format!("callTool params `{path}` {detail}"),
    }
}

pub(in crate::dispatch::gateway::code_mode) fn unwrap_code_mode_upstream_result(
    result: CallToolResult,
) -> Value {
    if let Some(value) = result.structured_content {
        return value;
    }

    let all_text = !result.content.is_empty()
        && result
            .content
            .iter()
            .all(|content| content.as_text().is_some());
    if all_text {
        let text = result
            .content
            .iter()
            .filter_map(|content| content.as_text())
            .map(|content| content.text.as_str())
            .collect::<Vec<_>>()
            .join("\n");
        return serde_json::from_str(&text).unwrap_or_else(|_| Value::String(text));
    }

    if result.content.is_empty() {
        Value::Null
    } else {
        json!(result)
    }
}
