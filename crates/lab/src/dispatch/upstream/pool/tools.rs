//! Tool queries on the upstream pool: healthy-tool listings, candidate/owner
//! lookup, schema and exposure rows, cached summaries, runtime metadata, and
//! tool health. `has_healthy_tools_for_upstream` is `pub(super)` because
//! `ensure.rs` calls it across the module boundary (plan §3.0/§2.1).

use futures::StreamExt;
use futures::stream::FuturesUnordered;
use serde_json::Value;

use crate::config::UpstreamConfig;

use super::super::types::{
    UpstreamCapability, UpstreamHealth, UpstreamRuntimeMetadata, UpstreamTool,
    UpstreamToolExposureRow,
};
use super::UpstreamPool;
use super::connect::connect_upstream;
use super::helpers::UpstreamCachedSummary;

/// Hard cap on the total number of tools returned by a single `healthy_tools()` call.
///
/// Prevents runaway allocations when a malicious or misconfigured upstream
/// exposes an extremely large catalog.  A truncation warning is emitted when
/// this limit is hit.  Tests can reference this constant to assert bounds behavior.
pub const MAX_UPSTREAM_TOOLS: usize = 1000;

/// Hard cap on the total number of resources returned by `list_upstream_resources()`.
pub const MAX_UPSTREAM_RESOURCES: usize = 1000;

/// Hard cap on the total number of prompts returned by `collect_upstream_prompts()`.
pub const MAX_UPSTREAM_PROMPTS: usize = 1000;

impl UpstreamPool {
    /// Get all healthy upstream tools, up to [`MAX_UPSTREAM_TOOLS`] total.
    ///
    /// If the combined catalog across all upstreams exceeds the cap, the excess
    /// is dropped and a `tracing::warn!` is emitted.  This prevents a buggy or
    /// malicious upstream from forcing large allocations.
    pub async fn healthy_tools(&self) -> Vec<UpstreamTool> {
        let catalog = self.catalog.read().await;
        let mut tools: Vec<UpstreamTool> = catalog
            .values()
            .filter(|entry| entry.tool_health.is_routable())
            .flat_map(|entry| {
                entry.tools.values().filter_map(|tool| {
                    entry
                        .exposure_policy
                        .matches(tool.tool.name.as_ref())
                        .then(|| tool.clone())
                })
            })
            .take(MAX_UPSTREAM_TOOLS + 1)
            .collect();
        if tools.len() > MAX_UPSTREAM_TOOLS {
            tools.truncate(MAX_UPSTREAM_TOOLS);
            tracing::warn!(
                limit = MAX_UPSTREAM_TOOLS,
                "upstream tool catalog exceeds limit — truncating to cap"
            );
        }
        tools
    }

    pub async fn healthy_tools_for_upstream(&self, upstream: &str) -> Vec<UpstreamTool> {
        let catalog = self.catalog.read().await;
        catalog
            .get(upstream)
            .into_iter()
            .filter(|entry| entry.tool_health.is_routable())
            .flat_map(|entry| {
                entry.tools.values().filter_map(|tool| {
                    entry
                        .exposure_policy
                        .matches(tool.tool.name.as_ref())
                        .then(|| tool.clone())
                })
            })
            .collect()
    }

    pub(super) async fn has_healthy_tools_for_upstream(&self, upstream: &str) -> bool {
        let catalog = self.catalog.read().await;
        catalog.get(upstream).is_some_and(|entry| {
            entry.tool_health.is_routable()
                && entry
                    .tools
                    .values()
                    .any(|tool| entry.exposure_policy.matches(tool.tool.name.as_ref()))
        })
    }

    pub async fn find_tool_candidates(&self, tool_name: &str) -> Vec<(String, UpstreamTool)> {
        let catalog = self.catalog.read().await;
        let mut matches = Vec::new();
        for (upstream_name, entry) in catalog.iter() {
            if !entry.tool_health.is_routable() {
                continue;
            }
            if let Some(tool) = entry.tools.get(tool_name)
                && entry.exposure_policy.matches(tool.tool.name.as_ref())
            {
                matches.push((upstream_name.clone(), tool.clone()));
            }
        }
        matches.sort_by(|a, b| a.0.cmp(&b.0));
        matches
    }

    pub async fn subject_scoped_tools(
        &self,
        configs: &[UpstreamConfig],
        subject: &str,
    ) -> Vec<(String, Vec<rmcp::model::Tool>)> {
        let mut futures = FuturesUnordered::new();
        let oauth_client_cache = self.oauth_client_cache.clone();
        for config in configs.iter().filter(|config| config.oauth.is_some()) {
            let config = config.clone();
            let subject = subject.to_string();
            let oauth_client_cache = oauth_client_cache.clone();
            futures.push(async move {
                let result = connect_upstream(
                    &config,
                    Some(subject.as_str()),
                    oauth_client_cache.as_ref(),
                    None,
                    None,
                )
                .await;
                (config.name.clone(), result)
            });
        }

        let mut discovered = Vec::new();
        while let Some((name, result)) = futures.next().await {
            match result {
                Ok((_conn, tools)) => discovered.push((name, tools)),
                Err(error) => {
                    tracing::warn!(
                        upstream = %name,
                        error = %error,
                        "subject-scoped upstream tool discovery failed"
                    );
                }
            }
        }
        discovered
    }

    /// Return the names of upstreams currently routable for a capability.
    pub async fn routable_upstream_names(&self, capability: UpstreamCapability) -> Vec<String> {
        let catalog = self.catalog.read().await;
        let mut names: Vec<String> = match capability {
            UpstreamCapability::Resources => {
                let resource_names = self.resource_upstreams.read().await;
                resource_names
                    .iter()
                    .filter(|name| {
                        catalog
                            .get(*name)
                            .is_some_and(|entry| entry.health_for(capability).is_routable())
                    })
                    .cloned()
                    .collect()
            }
            UpstreamCapability::Tools | UpstreamCapability::Prompts => catalog
                .iter()
                .filter(|(_, entry)| entry.health_for(capability).is_routable())
                .map(|(name, _)| name.clone())
                .collect(),
        };
        names.sort_unstable();
        names.dedup();
        names
    }

    /// Look up which upstream owns a given tool name.
    #[allow(clippy::significant_drop_tightening)]
    pub async fn find_tool(&self, tool_name: &str) -> Option<(String, UpstreamTool)> {
        let catalog = self.catalog.read().await;
        catalog
            .values()
            .filter(|entry| entry.tool_health.is_routable())
            .find_map(|entry| {
                entry.tools.get(tool_name).and_then(|tool| {
                    entry
                        .exposure_policy
                        .matches(tool_name)
                        .then(|| (entry.name.to_string(), tool.clone()))
                })
            })
    }

    /// Get the cached schema for a specific upstream tool.
    #[allow(clippy::significant_drop_tightening)]
    pub async fn tool_schema(&self, tool_name: &str) -> Option<Value> {
        let catalog = self.catalog.read().await;
        catalog.values().find_map(|entry| {
            entry.tools.get(tool_name).and_then(|tool| {
                entry
                    .exposure_policy
                    .matches(tool_name)
                    .then(|| tool.input_schema.clone())
                    .flatten()
            })
        })
    }

    /// Return all discovered tools for one upstream, including hidden tools and exposure metadata.
    pub async fn tool_exposure_rows(&self, upstream_name: &str) -> Vec<UpstreamToolExposureRow> {
        let catalog = self.catalog.read().await;
        let Some(entry) = catalog.get(upstream_name) else {
            return Vec::new();
        };

        let mut rows: Vec<UpstreamToolExposureRow> = entry
            .tools
            .values()
            .map(|tool| {
                let matched_by = entry.exposure_policy.matched_by(tool.tool.name.as_ref());
                UpstreamToolExposureRow {
                    name: tool.tool.name.to_string(),
                    description: tool
                        .tool
                        .description
                        .as_ref()
                        .map(ToString::to_string)
                        .filter(|text| !text.trim().is_empty()),
                    exposed: matched_by.is_some(),
                    matched_by,
                }
            })
            .collect();
        rows.sort_by(|left, right| left.name.cmp(&right.name));
        rows
    }

    pub async fn cached_upstream_summary(
        &self,
        upstream_name: &str,
    ) -> Option<UpstreamCachedSummary> {
        let catalog = self.catalog.read().await;
        let entry = catalog.get(upstream_name)?;
        let discovered_tool_count = entry.tools.len();
        let exposed_tool_count = entry
            .tools
            .values()
            .filter(|tool| entry.exposure_policy.matches(tool.tool.name.as_ref()))
            .count();
        let discovered_resource_count = entry.resource_count;
        let exposed_resource_count = if entry.resource_health.is_routable() {
            entry.resource_count
        } else {
            0
        };
        let discovered_prompt_count = entry.prompt_count;
        let exposed_prompt_count = if entry.prompt_health.is_routable() {
            entry.prompt_count
        } else {
            0
        };

        Some(UpstreamCachedSummary {
            discovered_tool_count,
            exposed_tool_count,
            discovered_resource_count,
            exposed_resource_count,
            discovered_prompt_count,
            exposed_prompt_count,
        })
    }

    pub async fn upstream_runtime_metadata(
        &self,
        upstream_name: &str,
    ) -> Option<UpstreamRuntimeMetadata> {
        self.connections
            .read()
            .await
            .get(upstream_name)
            .map(|conn| conn.runtime.clone())
    }

    /// Return the current tool health for one upstream.
    pub async fn upstream_tool_health(&self, upstream_name: &str) -> Option<UpstreamHealth> {
        let catalog = self.catalog.read().await;
        catalog.get(upstream_name).map(|entry| entry.tool_health)
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::super::super::types::ToolExposurePolicy;
    use super::super::entries::healthy_in_process_entry;
    use super::super::testsupport::*;
    use super::*;

    #[tokio::test]
    async fn empty_pool_has_no_tools() {
        let pool = UpstreamPool::new();
        assert!(pool.healthy_tools().await.is_empty());
        assert_eq!(pool.upstream_count().await, 0);
    }

    #[tokio::test]
    async fn hidden_upstream_tools_do_not_appear_in_listings() {
        let pool = UpstreamPool::new();
        let upstream_name: Arc<str> = Arc::from("github");
        let tools = test_upstream_tools(
            &upstream_name,
            &["search_repos", "github_create_issue", "delete_repo"],
        );
        let mut entry = healthy_in_process_entry(Arc::clone(&upstream_name), tools);
        entry.exposure_policy =
            ToolExposurePolicy::from_patterns(vec!["search_repos".into(), "github_*".into()])
                .expect("policy");

        pool.catalog
            .write()
            .await
            .insert("github".to_string(), entry);

        let names: Vec<String> = pool
            .healthy_tools()
            .await
            .into_iter()
            .map(|t| t.tool.name.to_string())
            .collect();
        assert!(names.contains(&"search_repos".to_string()));
        assert!(names.contains(&"github_create_issue".to_string()));
        assert!(!names.contains(&"delete_repo".to_string()));
    }

    #[tokio::test]
    async fn hidden_upstream_tools_cannot_be_called_directly() {
        let pool = UpstreamPool::new();
        let upstream_name: Arc<str> = Arc::from("github");
        let tools = test_upstream_tools(&upstream_name, &["search_repos", "delete_repo"]);
        let mut entry = healthy_in_process_entry(Arc::clone(&upstream_name), tools);
        entry.exposure_policy =
            ToolExposurePolicy::from_patterns(vec!["search_repos".into()]).expect("policy");

        pool.catalog
            .write()
            .await
            .insert("github".to_string(), entry);

        assert!(pool.find_tool("search_repos").await.is_some());
        assert!(pool.find_tool("delete_repo").await.is_none());
    }

    // --- lab-tad5: oversized catalog bounds regression tests ---

    /// A gateway pool that receives more than `MAX_UPSTREAM_TOOLS` tools must cap
    /// the result at exactly the limit and not panic or allocate unboundedly.
    #[tokio::test]
    async fn gateway_upstream_tool_cap_truncates_oversized_catalog() {
        let pool = UpstreamPool::new();
        let upstream_name: Arc<str> = Arc::from("big-upstream");

        // Build more tools than the cap.
        let tool_names: Vec<String> =
            (0..MAX_UPSTREAM_TOOLS + 50).map(|i| format!("tool_{i:04}")).collect();
        let tool_name_refs: Vec<&str> = tool_names.iter().map(String::as_str).collect();
        let tools = test_upstream_tools(&upstream_name, &tool_name_refs);

        let entry = healthy_in_process_entry(Arc::clone(&upstream_name), tools);
        pool.catalog
            .write()
            .await
            .insert("big-upstream".to_string(), entry);

        let result = pool.healthy_tools().await;
        assert_eq!(
            result.len(),
            MAX_UPSTREAM_TOOLS,
            "healthy_tools() must cap at MAX_UPSTREAM_TOOLS={MAX_UPSTREAM_TOOLS}"
        );
    }

    /// A pool with exactly `MAX_UPSTREAM_TOOLS` tools must NOT be truncated.
    #[tokio::test]
    async fn gateway_upstream_tool_cap_allows_exactly_limit_tools() {
        let pool = UpstreamPool::new();
        let upstream_name: Arc<str> = Arc::from("exact-upstream");

        let tool_names: Vec<String> =
            (0..MAX_UPSTREAM_TOOLS).map(|i| format!("tool_{i:04}")).collect();
        let tool_name_refs: Vec<&str> = tool_names.iter().map(String::as_str).collect();
        let tools = test_upstream_tools(&upstream_name, &tool_name_refs);

        let entry = healthy_in_process_entry(Arc::clone(&upstream_name), tools);
        pool.catalog
            .write()
            .await
            .insert("exact-upstream".to_string(), entry);

        let result = pool.healthy_tools().await;
        assert_eq!(
            result.len(),
            MAX_UPSTREAM_TOOLS,
            "healthy_tools() must not truncate exactly MAX_UPSTREAM_TOOLS tools"
        );
    }
}
