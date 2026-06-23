use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct NodeLogEvent {
    pub node_id: String,
    pub source: String,
    pub timestamp_unix_ms: i64,
    pub level: Option<String>,
    pub message: String,
    pub fields: serde_json::Map<String, serde_json::Value>,
}
