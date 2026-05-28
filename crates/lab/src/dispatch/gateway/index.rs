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
    /// Score multiplier inherited from `UpstreamConfig.priority` (default 1.0).
    /// Applied after all lexical scoring so the floor cut sees priority-adjusted scores.
    #[serde(default = "default_priority")]
    pub priority: f32,
    name_lower: String,
    haystack: String,
}

fn default_priority() -> f32 {
    1.0
}

impl IndexedTool {
    /// Synthesize an `IndexedTool` from a Qdrant semantic-search payload,
    /// inheriting the upstream's configured priority.
    ///
    /// Used by `rrf_fuse` to surface semantic-only hits — tools that the
    /// lexical scorer didn't find but the embedding model retrieved. Callers
    /// must drop hits where `priority == 0.0` rather than constructing a
    /// zero-priority entry, so operator suppression survives fusion.
    /// `input_schema` is unavailable from the payload (not indexed); callers
    /// needing it must look the tool up by `(upstream, name)` in the live
    /// catalog.
    #[allow(dead_code)]
    pub(crate) fn from_semantic_payload_with_priority(
        name: &str,
        upstream: &str,
        description: &str,
        priority: f32,
    ) -> Self {
        let name_lower = name.to_ascii_lowercase();
        let haystack = format!("{}\n{}", name_lower, description.to_ascii_lowercase());
        Self {
            name: name.to_string(),
            description: description.to_string(),
            upstream_name: upstream.to_string(),
            input_schema: None,
            priority,
            name_lower,
            haystack,
        }
    }
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

#[allow(dead_code)]
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

        let priority = config.priority.max(0.0);
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
                    priority,
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

    /// Construct a single-tool index for use in tests.
    #[cfg(test)]
    pub fn build_for_test(name: &str, upstream: &str, description: &str) -> Self {
        let name_lower = name.to_ascii_lowercase();
        let haystack = format!("{}\n{}", name_lower, description.to_ascii_lowercase());
        let tool = IndexedTool {
            name: name.to_string(),
            description: description.to_string(),
            upstream_name: upstream.to_string(),
            input_schema: None,
            priority: 1.0,
            name_lower,
            haystack,
        };
        Self {
            tools: vec![tool],
            metadata: ToolIndexMetadata::default(),
        }
    }

    /// Search this index for tools matching `query`.
    ///
    /// `score_floor_fraction`: drop results whose score is below this fraction
    /// of the top result's score. 0.0 disables the floor (keeps all positive
    /// scores). Applied per-upstream so each source's floor cut is relative to
    /// its own best match, not a global mixed-source maximum.
    #[allow(dead_code)]
    pub fn search(&self, query: &str, top_k: usize, score_floor_fraction: f32) -> Vec<SearchHit> {
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

        // Apply score floor relative to this upstream's top result.
        if score_floor_fraction > 0.0 {
            if let Some(&(top_score, _)) = scored.first() {
                let floor = top_score * score_floor_fraction;
                scored.retain(|(score, _)| *score >= floor);
            }
        }

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

#[allow(dead_code)]
fn score_tool(query: &str, tool: &IndexedTool) -> f32 {
    score_name_haystack(query, &tool.name_lower, &tool.haystack) * tool.priority
}

#[allow(dead_code)]
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
            .split(['_', '-'])
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
    // `name_lower.len()` is bounded by tool-name length (well under f32 mantissa range);
    // cast precision loss is irrelevant for this length normalization heuristic.
    #[allow(clippy::cast_precision_loss)]
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

#[cfg(test)]
mod tests {
    use super::*;

    fn approx_eq(a: f32, b: f32) -> bool {
        (a - b).abs() < 0.05
    }

    fn make_tool(name: &str, description: &str, priority: f32) -> IndexedTool {
        let name_lower = name.to_ascii_lowercase();
        let haystack = format!("{}\n{}", name_lower, description.to_ascii_lowercase());
        IndexedTool {
            name: name.to_string(),
            description: description.to_string(),
            upstream_name: "test".to_string(),
            input_schema: None,
            priority,
            name_lower,
            haystack,
        }
    }

    // Golden cases from issue #64 — verify exact arithmetic of the current scorer.
    // These lock in baseline behavior; changes to the scorer must update these first.

    #[test]
    fn golden_synapse_compose_docker_stats_exec() {
        // Only `stats` hits in `zsh_alan_stats`; other tokens miss entirely.
        // raw = 22.0 (segment-exact +20, haystack +2); len_factor = sqrt(14/12) ≈ 1.0801
        let score = score_name_haystack(
            "synapse compose docker stats exec",
            "zsh_alan_stats",
            "zsh_alan_stats\nget alan learning database statistics",
        );
        assert!(approx_eq(score, 20.37), "expected ≈20.37, got {score}");
    }

    #[test]
    fn golden_docker_container_inspect_logs_dookie() {
        // `logs` tool: segment-exact for "logs" (+20), haystack (+2) = 22.0;
        // name len = 4 so len_factor = max(4/12, 1)^0.5 = 1.0.
        // Description must not contain "container" or other query tokens to isolate the score.
        let score = score_name_haystack(
            "docker container inspect logs dookie",
            "logs",
            "logs\nlive log streaming",
        );
        assert!(approx_eq(score, 22.0), "expected 22.0, got {score}");
    }

    #[test]
    fn golden_scout_flux_ssh_inspect_host() {
        // `scanner` tool: no name/segment hits; 2 tokens found in haystack (+2 each) = 4.0
        let score = score_name_haystack(
            "scout flux ssh inspect host",
            "scanner",
            "scanner\nscan local and ssh hosts for service status",
        );
        assert!(approx_eq(score, 4.0), "expected 4.0, got {score}");
    }

    #[test]
    fn exact_name_match_always_wins() {
        let score = score_name_haystack("radarr", "radarr", "radarr\nmovie manager");
        assert_eq!(score, 200.0);
    }

    #[test]
    fn noise_floor_tool_dropped_by_floor_fraction() {
        // `scanner` scores 4.0 (noise floor only via 2 haystack hits) vs `logs` at 22.0.
        // With floor fraction 0.25: floor = 22.0 * 0.25 = 5.5 → scanner (4.0) is dropped.
        // logs description contains none of the query tokens → score is exactly 22.0.
        // scanner description contains "docker" and "container" → 2×2.0 = 4.0 noise floor.
        let logs_tool = make_tool("logs", "live log streaming", 1.0);
        let scanner_tool = make_tool("scanner", "scan docker containers for service status", 1.0);
        let index = ToolIndex {
            tools: vec![logs_tool, scanner_tool],
            metadata: ToolIndexMetadata::default(),
        };
        let results = index.search("docker container inspect logs dookie", 10, 0.25);
        assert_eq!(results.len(), 1, "scanner should be dropped by score floor");
        assert_eq!(results[0].tool.name, "logs");
    }

    #[test]
    fn floor_disabled_returns_all_positive_scores() {
        let logs_tool = make_tool("logs", "live log streaming", 1.0);
        let scanner_tool = make_tool("scanner", "scan docker containers for service status", 1.0);
        let index = ToolIndex {
            tools: vec![logs_tool, scanner_tool],
            metadata: ToolIndexMetadata::default(),
        };
        let results = index.search("docker container inspect logs dookie", 10, 0.0);
        assert_eq!(
            results.len(),
            2,
            "both tools should appear when floor is disabled"
        );
    }

    #[test]
    fn upstream_priority_scales_scores() {
        // Two tools with equal lexical scores; higher-priority one should rank first.
        let low = make_tool("get_weather", "current weather data", 0.5);
        let high = make_tool("get_weather", "current weather data", 2.0);
        let low_score = score_tool("weather", &low);
        let high_score = score_tool("weather", &high);
        assert!(
            high_score > low_score,
            "higher priority must produce higher score"
        );
        assert!(
            approx_eq(high_score, low_score * 4.0),
            "priority is a linear multiplier"
        );
    }

    #[test]
    fn priority_zero_suppresses_tool_completely() {
        // priority=0.0 should produce score=0.0 regardless of lexical match,
        // so the tool is excluded (score > 0.0 filter in search).
        let tool = make_tool("radarr", "movie manager", 0.0);
        let score = score_tool("radarr", &tool);
        assert_eq!(score, 0.0, "priority=0.0 must suppress the tool");
    }
}
