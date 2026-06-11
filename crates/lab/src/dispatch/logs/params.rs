//! Param coercion for logs dispatch actions.

use serde_json::Value;

use super::metrics::MetricsWindow;
use super::types::{LogQuery, LogTailRequest};
use crate::dispatch::error::ToolError;

pub fn parse_search_params(params: Value) -> Result<LogQuery, ToolError> {
    // Accept both `{"query": {...}}` (wrapped) and `{...}` (flat); `null` too.
    let inner = match params {
        Value::Object(mut map) => map.remove("query").unwrap_or(Value::Object(map)),
        Value::Null => Value::Object(serde_json::Map::new()),
        other => other,
    };
    serde_json::from_value::<LogQuery>(inner).map_err(|e| ToolError::InvalidParam {
        message: format!("invalid LogQuery: {e}"),
        param: "query".to_string(),
    })
}

pub fn parse_tail_params(params: Value) -> Result<LogTailRequest, ToolError> {
    let inner = if params.is_null() {
        Value::Object(serde_json::Map::new())
    } else {
        params
    };
    serde_json::from_value::<LogTailRequest>(inner).map_err(|e| ToolError::InvalidParam {
        message: format!("invalid LogTailRequest: {e}"),
        param: "params".to_string(),
    })
}

pub fn parse_metrics_params(params: Value) -> Result<MetricsWindow, ToolError> {
    let window = params
        .get("window")
        .and_then(Value::as_str)
        .unwrap_or("24h");
    MetricsWindow::parse(window).ok_or_else(|| ToolError::InvalidParam {
        message: format!("invalid window `{window}` (expected 1h, 24h, or 7d)"),
        param: "window".to_string(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn search_accepts_wrapped_query() {
        let p = serde_json::json!({"query":{"subsystems":["gateway"],"levels":["warn"]}});
        let q = parse_search_params(p).unwrap();
        assert_eq!(q.subsystems.len(), 1);
        assert_eq!(q.levels.len(), 1);
    }

    #[test]
    fn search_accepts_flat_query() {
        let p = serde_json::json!({"subsystems":["gateway"]});
        let q = parse_search_params(p).unwrap();
        assert_eq!(q.subsystems.len(), 1);
    }

    #[test]
    fn search_accepts_empty_object() {
        let q = parse_search_params(serde_json::json!({})).unwrap();
        assert!(q.subsystems.is_empty());
    }

    #[test]
    fn tail_accepts_flat() {
        let p = serde_json::json!({"after_ts":0,"limit":10});
        let r = parse_tail_params(p).unwrap();
        assert_eq!(r.after_ts, Some(0));
        assert_eq!(r.limit, Some(10));
    }

    #[test]
    fn search_rejects_garbage() {
        let err = parse_search_params(serde_json::json!(42)).unwrap_err();
        assert_eq!(err.kind(), "invalid_param");
    }
}
