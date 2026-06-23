use serde::{Deserialize, Serialize};

use crate::node::config_scan::DiscoveredMcpConfigFile;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NodeHello {
    #[serde(alias = "device_id")]
    pub node_id: String,
    pub role: String,
    pub version: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NodeStatus {
    pub node_id: String,
    pub connected: bool,
    pub cpu_percent: Option<f32>,
    pub memory_used_bytes: Option<u64>,
    pub storage_used_bytes: Option<u64>,
    pub os: Option<String>,
    pub ips: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub health: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub uptime_seconds: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cores: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cpu_clock_mhz: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cpu_temp_c: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub total_memory_bytes: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub total_storage_bytes: Option<u64>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub doctor_issues: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub active_claude_sessions: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub active_codex_sessions: Option<u32>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NodeMetadataUpload {
    pub node_id: String,
    pub discovered_configs: Vec<DiscoveredMcpConfigFile>,
}
