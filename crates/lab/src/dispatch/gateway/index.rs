use std::cmp::Ordering;
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::config::UpstreamConfig;
use crate::dispatch::upstream::types::UpstreamTool;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IndexedTool {
    pub name: String,
    pub description: String,
    pub upstream_name: String,
    pub input_schema: Option<Value>,
    name_lower: String,
    haystack: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ToolIndexMetadata {
    pub truncated: bool,
    pub total_discovered: usize,
    pub indexed_count: usize,
    pub catalog_hash: u64,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ToolIndex {
    pub tools: Vec<IndexedTool>,
    pub metadata: ToolIndexMetadata,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchHit {
    pub tool: IndexedTool,
    pub score: f32,
}

impl ToolIndex {
    /// Build an index from an already-fetched healthy-tool snapshot.
    ///
    /// Split out from the async fetch so callers can `spawn_blocking` the CPU work
    /// after awaiting `UpstreamPool::healthy_tools()` on the async runtime.
    pub fn build_from_tools(
        config: &UpstreamConfig,
        healthy_tools: Vec<UpstreamTool>,
        max_tools: usize,
    ) -> Self {
        let matching = healthy_tools
            .into_iter()
            .filter(|tool| tool.upstream_name.as_ref() == config.name)
            .collect::<Vec<_>>();
        let total_discovered = matching.len();

        let tools = matching
            .into_iter()
            .take(max_tools)
            .map(|tool| {
                let description = tool
                    .tool
                    .description
                    .as_ref()
                    .map(|value| value.to_string())
                    .unwrap_or_default();
                let name = tool.tool.name.to_string();
                let name_lower = name.to_ascii_lowercase();
                let haystack = format!("{}\n{}", name_lower, description.to_ascii_lowercase());
                IndexedTool {
                    name,
                    description,
                    upstream_name: tool.upstream_name.as_ref().to_string(),
                    input_schema: tool.input_schema,
                    name_lower,
                    haystack,
                }
            })
            .collect::<Vec<_>>();

        let metadata = ToolIndexMetadata {
            truncated: total_discovered > tools.len(),
            total_discovered,
            indexed_count: tools.len(),
            catalog_hash: catalog_hash(&tools),
        };

        Self { tools, metadata }
    }

    pub fn search(&self, query: &str, top_k: usize) -> Vec<SearchHit> {
        let needle = query.trim().to_ascii_lowercase();
        if needle.is_empty() {
            return Vec::new();
        }

        let mut scored = self
            .tools
            .iter()
            .filter_map(|tool| {
                let score = score_tool(&needle, tool);
                (score > 0.0).then_some((score, tool))
            })
            .collect::<Vec<_>>();

        scored.sort_by(|a, b| {
            b.0.partial_cmp(&a.0)
                .unwrap_or(Ordering::Equal)
                .then_with(|| a.1.name.cmp(&b.1.name))
        });
        scored.truncate(top_k);
        scored
            .into_iter()
            .map(|(score, tool)| SearchHit {
                tool: tool.clone(),
                score,
            })
            .collect()
    }
}

fn score_tool(query: &str, tool: &IndexedTool) -> f32 {
    score_name_haystack(query, &tool.name_lower, &tool.haystack)
}

/// Score a tool given pre-lowercased name and haystack strings.
///
/// Exported so the builtin-tool search path in the MCP server can use the
/// same algorithm without duplicating it.
pub(crate) fn score_name_haystack(query: &str, name_lower: &str, haystack: &str) -> f32 {
    // Exact name match always wins.
    if name_lower == query {
        return 200.0;
    }

    let mut score = 0.0f32;

    // Whole-query match against the name.
    if name_lower.starts_with(query) {
        score += 80.0;
    } else if name_lower.contains(query) {
        score += 25.0;
    }

    // Token-level scoring: split both query and name on word-boundary characters
    // so "weather" in "get_weather" scores as a segment match, not just substring.
    let q_tokens: Vec<&str> = query
        .split(|c: char| c.is_whitespace() || c == '_' || c == '-')
        .filter(|t| t.len() >= 2)
        .collect();
    if !q_tokens.is_empty() {
        let name_segments: Vec<&str> = name_lower
            .split(|c: char| c == '_' || c == '-')
            .filter(|s| !s.is_empty())
            .collect();
        for token in &q_tokens {
            if name_segments.iter().any(|seg| *seg == *token) {
                // Exact segment match: "weather" in ["get","weather"] → strongest token signal.
                score += 20.0;
            } else if name_segments.iter().any(|seg| seg.starts_with(token)) {
                score += 10.0;
            } else if name_lower.contains(token) {
                score += 5.0;
            }
            if haystack.contains(token) {
                score += 2.0;
            }
        }
    }

    // Length normalization: prefer concise, focused names for equal relevance.
    // The divisor is capped at 1.0 so short names get no bonus, only long ones
    // are gently penalized.
    let len_factor = (name_lower.len() as f32 / 12.0).max(1.0).sqrt();
    score / len_factor
}

fn catalog_hash(tools: &[IndexedTool]) -> u64 {
    let mut hasher = DefaultHasher::new();
    for tool in tools {
        tool.upstream_name.hash(&mut hasher);
        tool.name.hash(&mut hasher);
        tool.description.hash(&mut hasher);
        if let Some(schema) = &tool.input_schema {
            schema.to_string().hash(&mut hasher);
        }
    }
    hasher.finish()
}
