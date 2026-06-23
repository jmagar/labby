use serde::Serialize;

#[derive(Debug, Clone, Serialize)]
pub struct SearchLogsRequest {
    pub node_id: String,
    pub query: String,
}
