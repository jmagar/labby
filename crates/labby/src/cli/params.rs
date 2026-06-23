//! CLI key=value param parsing for action-style subcommands.
//!
//! Both `ByteStash` and `UniFi` (and future services) expose an `action + params`
//! CLI surface. This module owns the canonical parse/coerce logic so it
//! is not duplicated per service.
//!
//! MCP and HTTP surfaces receive JSON directly from the protocol and never
//! call this function — it belongs in cli/, not dispatch/.

use anyhow::Result;
use serde_json::{Map, Value};

/// Parse a list of `key=value` strings into a JSON object.
///
/// Each string must contain exactly one `=`. The value portion is coerced to
/// a JSON boolean, integer, float, or string — in that precedence order.
///
/// # Errors
/// Returns an error if any item does not contain `=`.
pub fn parse_kv_params(params: Vec<String>) -> Result<Value> {
    let mut map = Map::new();
    for item in params {
        let Some((key, raw)) = item.split_once('=') else {
            anyhow::bail!("invalid param `{item}`; expected key=value");
        };
        if map.contains_key(key) {
            anyhow::bail!("duplicate param key: `{key}`");
        }
        map.insert(key.to_string(), coerce_value(raw));
    }
    Ok(Value::Object(map))
}

/// Coerce a raw string into the most specific JSON scalar type.
fn coerce_value(raw: &str) -> Value {
    if raw.eq_ignore_ascii_case("true") {
        return Value::Bool(true);
    }
    if raw.eq_ignore_ascii_case("false") {
        return Value::Bool(false);
    }
    if let Ok(n) = raw.parse::<i64>() {
        // Round-trip check: '01' parses as 1 but must stay a string.
        if n.to_string() == raw {
            return Value::Number(n.into());
        }
    }
    if let Ok(n) = raw.parse::<f64>()
        && let Some(num) = serde_json::Number::from_f64(n)
    {
        // Round-trip check for floats too: '01234' parses as 1234.0 but must stay a string.
        if n.to_string() == raw {
            return Value::Number(num);
        }
    }
    Value::String(raw.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_kv_params_coerces_scalars() {
        let value = parse_kv_params(vec![
            "enabled=true".to_string(),
            "count=7".to_string(),
            "ratio=1.5".to_string(),
            "name=alice".to_string(),
        ])
        .unwrap();
        assert_eq!(value["enabled"], true);
        assert_eq!(value["count"], 7);
        assert_eq!(value["ratio"], 1.5);
        assert_eq!(value["name"], "alice");
    }

    #[test]
    fn parse_kv_params_rejects_missing_equals() {
        let err = parse_kv_params(vec!["broken".to_string()]).unwrap_err();
        assert!(err.to_string().contains("expected key=value"));
    }

    #[test]
    fn coerce_preserves_leading_zeros_as_strings() {
        let value = parse_kv_params(vec![
            "zip=01234".to_string(),
            "code=007".to_string(),
            "padded=01".to_string(),
            "plain=7".to_string(),
        ])
        .unwrap();
        assert_eq!(value["zip"], "01234");
        assert_eq!(value["code"], "007");
        assert_eq!(value["padded"], "01");
        assert_eq!(value["plain"], 7);
    }

    #[test]
    fn parse_kv_params_rejects_duplicate_keys() {
        let err = parse_kv_params(vec!["k=1".to_string(), "k=2".to_string()]).unwrap_err();
        assert!(err.to_string().contains("duplicate param key: `k`"));
    }

    #[test]
    fn parse_kv_params_empty_returns_empty_object() {
        let value = parse_kv_params(vec![]).unwrap();
        assert!(value.as_object().unwrap().is_empty());
    }

    #[test]
    fn empty_value_yields_empty_string() {
        let value = parse_kv_params(vec!["key=".to_string()]).unwrap();
        assert_eq!(value["key"], "");
    }

    #[test]
    fn empty_key_yields_empty_string_key() {
        let value = parse_kv_params(vec!["=value".to_string()]).unwrap();
        assert_eq!(value[""], "value");
    }

    #[test]
    fn value_containing_equals_uses_split_once() {
        let value = parse_kv_params(vec!["key=foo=bar".to_string()]).unwrap();
        assert_eq!(value["key"], "foo=bar");
    }

    #[test]
    fn boolean_case_insensitive() {
        let value = parse_kv_params(vec![
            "a=TRUE".to_string(),
            "b=False".to_string(),
            "c=FALSE".to_string(),
            "d=True".to_string(),
        ])
        .unwrap();
        assert_eq!(value["a"], true);
        assert_eq!(value["b"], false);
        assert_eq!(value["c"], false);
        assert_eq!(value["d"], true);
    }

    #[test]
    fn nan_inf_stay_as_strings() {
        let value = parse_kv_params(vec![
            "a=NaN".to_string(),
            "b=inf".to_string(),
            "c=-inf".to_string(),
        ])
        .unwrap();
        assert_eq!(value["a"], "NaN");
        assert_eq!(value["b"], "inf");
        assert_eq!(value["c"], "-inf");
    }
}
