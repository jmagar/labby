//! Tool queries on the upstream pool: healthy-tool listings, candidate/owner
//! lookup, schema and exposure rows, cached summaries, runtime metadata, and
//! tool health. `has_healthy_tools_for_upstream` is `pub(super)` because
//! `ensure.rs` calls it across the module boundary (plan §3.0/§2.1).

use std::collections::BTreeSet;

use futures::StreamExt;
use futures::stream::FuturesUnordered;
use serde_json::Value;

use labby_runtime::gateway_config::UpstreamConfig;

use super::super::types::{
    UpstreamCapability, UpstreamEnrichmentCatalogEntry, UpstreamHealth, UpstreamRuntimeMetadata,
    UpstreamTool, UpstreamToolExposureRow,
};
use super::UpstreamPool;
use super::helpers::UpstreamCachedSummary;

/// Hard cap on the total number of tools returned by a single `healthy_tools()` call.
///
/// Prevents runaway allocations when a malicious or misconfigured upstream
/// exposes an extremely large catalog.  A truncation warning is emitted when
/// this limit is hit.  Tests can reference this constant to assert bounds behavior.
pub(crate) const MAX_UPSTREAM_TOOLS: usize = 1000;

/// Hard cap on the total number of resources returned by `list_upstream_resources()`.
pub(crate) const MAX_UPSTREAM_RESOURCES: usize = 1000;

/// Hard cap on the total number of prompts returned by `collect_upstream_prompts()`.
pub(crate) const MAX_UPSTREAM_PROMPTS: usize = 1000;

fn upstream_allowed(allowed: Option<&BTreeSet<String>>, upstream: &str) -> bool {
    allowed.is_none_or(|names| names.contains(upstream))
}

impl UpstreamPool {
    /// Get all healthy upstream tools, up to [`MAX_UPSTREAM_TOOLS`] total.
    ///
    /// If the combined catalog across all upstreams exceeds the cap, the excess
    /// is dropped and a `tracing::warn!` is emitted.  This prevents a buggy or
    /// malicious upstream from forcing large allocations.
    pub async fn healthy_tools(&self) -> Vec<UpstreamTool> {
        self.healthy_tools_allowed(None).await
    }

    pub async fn healthy_tools_allowed(
        &self,
        allowed: Option<&BTreeSet<String>>,
    ) -> Vec<UpstreamTool> {
        let catalog = self.catalog.read().await;
        let mut tools: Vec<UpstreamTool> = catalog
            .iter()
            .filter(|(name, _)| upstream_allowed(allowed, name))
            .filter(|(_, entry)| entry.tool_health.is_routable())
            .flat_map(|(_, entry)| {
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

    pub async fn healthy_ui_tools_allowed(
        &self,
        allowed: Option<&BTreeSet<String>>,
    ) -> Vec<UpstreamTool> {
        let catalog = self.catalog.read().await;
        let mut tools: Vec<UpstreamTool> = catalog
            .iter()
            .filter(|(name, _)| upstream_allowed(allowed, name))
            .filter(|(_, entry)| entry.tool_health.is_routable())
            .filter(|(_, entry)| entry.proxy_resources)
            .flat_map(|(_, entry)| {
                entry.tools.values().filter_map(|tool| {
                    (entry.exposure_policy.matches(tool.tool.name.as_ref())
                        && tool_has_mcp_app_ui_resource(tool))
                    .then(|| tool.clone())
                })
            })
            .take(MAX_UPSTREAM_TOOLS + 1)
            .collect();
        if tools.len() > MAX_UPSTREAM_TOOLS {
            tools.truncate(MAX_UPSTREAM_TOOLS);
            tracing::warn!(
                limit = MAX_UPSTREAM_TOOLS,
                "upstream MCP App tool catalog exceeds limit — truncating to cap"
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

    /// Like [`find_tool_candidates`](Self::find_tool_candidates) but constrained
    /// to the route's allowed upstreams.
    ///
    /// Returns every exposed, routable, route-scope-allowed upstream that exposes
    /// `tool_name`, sorted by upstream name. The Code Mode MCP App callback gate
    /// uses this to detect ambiguity (a tool name exposed by more than one allowed
    /// upstream) and fail closed instead of proxying an arbitrary, hash-order
    /// dependent upstream.
    pub async fn find_exposed_tool_candidates_allowed(
        &self,
        tool_name: &str,
        allowed: Option<&BTreeSet<String>>,
    ) -> Vec<(String, UpstreamTool)> {
        let catalog = self.catalog.read().await;
        let mut matches = Vec::new();
        for (upstream_name, entry) in catalog.iter() {
            if !upstream_allowed(allowed, upstream_name) || !entry.tool_health.is_routable() {
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

    /// Return exposed tools whose upstream also exposes at least one MCP App UI tool.
    ///
    /// Code Mode keeps ordinary raw tools out of `list_tools`, but a rendered MCP
    /// App can only talk back to its server through host `callServerTool`
    /// callbacks. This lookup is the narrow callback allowlist: the requested
    /// tool must still be exposed by its upstream, and that same upstream must
    /// expose an MCP App UI tool.
    pub async fn find_mcp_app_sibling_tool_candidates(
        &self,
        tool_name: &str,
        allowed: Option<&BTreeSet<String>>,
    ) -> Vec<(String, UpstreamTool)> {
        let catalog = self.catalog.read().await;
        let mut matches = Vec::new();
        for (upstream_name, entry) in catalog.iter() {
            if !upstream_allowed(allowed, upstream_name) || !entry.tool_health.is_routable() {
                continue;
            }
            let Some(tool) = entry.tools.get(tool_name) else {
                continue;
            };
            if !entry.exposure_policy.matches(tool.tool.name.as_ref()) {
                continue;
            }
            let has_ui_sibling = entry.tools.values().any(|candidate| {
                entry.exposure_policy.matches(candidate.tool.name.as_ref())
                    && tool_has_mcp_app_ui_resource(candidate)
            });
            if has_ui_sibling {
                matches.push((upstream_name.clone(), tool.clone()));
            }
        }
        matches.sort_by(|a, b| a.0.cmp(&b.0));
        matches
    }

    /// Return tool lists for all OAuth upstreams visible to `subject`.
    ///
    /// P-C1 fix: uses `acquire_or_connect_subject` so the per-(upstream,subject)
    /// connection and tool list are cached — the expensive TLS + initialize +
    /// tools/list is paid at most once per idle-TTL window, not on every call.
    pub async fn subject_scoped_tools(
        &self,
        configs: &[UpstreamConfig],
        subject: &str,
    ) -> Vec<(String, Vec<rmcp::model::Tool>)> {
        let mut futures = FuturesUnordered::new();
        for config in configs.iter().filter(|config| config.oauth.is_some()) {
            let config = config.clone();
            let subject = subject.to_string();
            let pool = self.clone();
            futures.push(async move {
                let result = pool.acquire_or_connect_subject(&config, &subject).await;
                (config.name.clone(), result)
            });
        }

        let mut discovered = Vec::new();
        while let Some((name, result)) = futures.next().await {
            match result {
                Ok((_peer, tools)) => discovered.push((name, tools)),
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
        self.find_tool_allowed(tool_name, None).await
    }

    #[allow(clippy::significant_drop_tightening)]
    pub async fn find_tool_allowed(
        &self,
        tool_name: &str,
        allowed: Option<&BTreeSet<String>>,
    ) -> Option<(String, UpstreamTool)> {
        let catalog = self.catalog.read().await;
        catalog
            .iter()
            .filter(|(name, _)| upstream_allowed(allowed, name))
            .map(|(_, entry)| entry)
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

    /// Return one deterministic cached catalog snapshot for enrichment previews.
    ///
    /// This never connects, probes, reads resources/prompts, or calls upstream
    /// tools. It clones bounded metadata from the in-memory catalog under a
    /// single read lock, allowing callers to filter and cap outside the lock.
    pub async fn cached_enrichment_snapshot(
        &self,
        allowed: Option<&BTreeSet<String>>,
    ) -> Vec<UpstreamEnrichmentCatalogEntry> {
        let catalog = self.catalog.read().await;
        let mut entries = catalog
            .iter()
            .filter(|(name, _)| upstream_allowed(allowed, name))
            .map(|(name, entry)| {
                let mut tool_rows = entry
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
                    .collect::<Vec<_>>();
                tool_rows.sort_by(|left, right| left.name.cmp(&right.name));
                UpstreamEnrichmentCatalogEntry {
                    upstream: name.clone(),
                    tool_rows,
                    resource_count: if entry.resource_health.is_routable() {
                        entry.resource_count
                    } else {
                        0
                    },
                    prompt_count: if entry.prompt_health.is_routable() {
                        entry.prompt_count
                    } else {
                        0
                    },
                }
            })
            .collect::<Vec<_>>();
        entries.sort_by(|left, right| left.upstream.cmp(&right.upstream));
        entries
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

    /// Return just the names of all healthy exposed upstream tools.
    ///
    /// Cheaper than `healthy_tools()` for callers that only need tool names
    /// (e.g. `snapshot_catalog` change-detection): avoids deep-cloning every
    /// tool schema just to extract the name field.
    pub async fn healthy_tool_names(&self) -> Vec<String> {
        let catalog = self.catalog.read().await;
        let mut names: Vec<String> = catalog
            .values()
            .filter(|entry| entry.tool_health.is_routable())
            .flat_map(|entry| {
                entry.tools.values().filter_map(|tool| {
                    entry
                        .exposure_policy
                        .matches(tool.tool.name.as_ref())
                        .then(|| tool.tool.name.to_string())
                })
            })
            .take(MAX_UPSTREAM_TOOLS + 1)
            .collect();
        if names.len() > MAX_UPSTREAM_TOOLS {
            names.truncate(MAX_UPSTREAM_TOOLS);
        }
        names
    }
}

pub fn tool_has_mcp_app_ui_resource(tool: &UpstreamTool) -> bool {
    tool.tool
        .meta
        .as_ref()
        .and_then(|meta| meta.0.get("ui"))
        .and_then(|ui| ui.get("resourceUri"))
        .and_then(Value::as_str)
        .is_some_and(|uri| uri.starts_with("ui://"))
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use rmcp::model::Meta;

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

    #[tokio::test]
    async fn mcp_app_sibling_lookup_requires_exposed_ui_tool_on_same_upstream() {
        let pool = UpstreamPool::new();

        let apps_name: Arc<str> = Arc::from("apps");
        let mut apps_tools =
            test_upstream_tools(&apps_name, &["youtube_search_ui", "youtube_probe"]);
        let ui_meta = Meta(serde_json::Map::from_iter([(
            "ui".to_string(),
            serde_json::json!({ "resourceUri": "ui://apps/youtube-search.html" }),
        )]));
        apps_tools
            .get_mut("youtube_search_ui")
            .expect("ui tool")
            .tool
            .meta = Some(ui_meta);
        let apps_entry = healthy_in_process_entry(Arc::clone(&apps_name), apps_tools);
        pool.catalog
            .write()
            .await
            .insert("apps".to_string(), apps_entry);

        let plain_name: Arc<str> = Arc::from("plain");
        let plain_tools = test_upstream_tools(&plain_name, &["youtube_probe"]);
        let plain_entry = healthy_in_process_entry(Arc::clone(&plain_name), plain_tools);
        pool.catalog
            .write()
            .await
            .insert("plain".to_string(), plain_entry);

        let candidates = pool
            .find_mcp_app_sibling_tool_candidates("youtube_probe", None)
            .await;
        let upstreams = candidates
            .iter()
            .map(|(upstream, _)| upstream.as_str())
            .collect::<Vec<_>>();

        assert_eq!(upstreams, vec!["apps"]);

        let allowed = BTreeSet::from(["plain".to_string()]);
        assert!(
            pool.find_mcp_app_sibling_tool_candidates("youtube_probe", Some(&allowed))
                .await
                .is_empty(),
            "route scope must still constrain MCP App callback siblings"
        );
    }

    #[tokio::test]
    async fn mcp_app_sibling_lookup_returns_all_candidate_upstreams() {
        // When a hidden tool name is exposed by more than one UI-bearing upstream,
        // the lookup must surface every candidate so the call gate can detect the
        // ambiguity and fail closed (rather than silently picking one).
        let pool = UpstreamPool::new();
        for upstream in ["apps_a", "apps_b"] {
            let name: Arc<str> = Arc::from(upstream);
            let mut tools = test_upstream_tools(&name, &["search_ui", "youtube_probe"]);
            tools.get_mut("search_ui").expect("ui tool").tool.meta =
                Some(Meta(serde_json::Map::from_iter([(
                    "ui".to_string(),
                    serde_json::json!({ "resourceUri": format!("ui://{upstream}/s.html") }),
                )])));
            let entry = healthy_in_process_entry(Arc::clone(&name), tools);
            pool.catalog
                .write()
                .await
                .insert(upstream.to_string(), entry);
        }

        let candidates = pool
            .find_mcp_app_sibling_tool_candidates("youtube_probe", None)
            .await;
        let upstreams = candidates
            .iter()
            .map(|(upstream, _)| upstream.as_str())
            .collect::<Vec<_>>();
        assert_eq!(
            upstreams,
            vec!["apps_a", "apps_b"],
            "both UI-bearing upstreams must be returned so the gate can detect ambiguity"
        );
    }

    #[tokio::test]
    async fn find_exposed_tool_candidates_allowed_filters_by_scope_and_exposure() {
        let pool = UpstreamPool::new();

        // Upstream "a" exposes `probe`.
        let a: Arc<str> = Arc::from("a");
        let a_tools = test_upstream_tools(&a, &["probe"]);
        pool.catalog.write().await.insert(
            "a".to_string(),
            healthy_in_process_entry(Arc::clone(&a), a_tools),
        );

        // Upstream "b" has `probe` but hides it via exposure policy.
        let b: Arc<str> = Arc::from("b");
        let b_tools = test_upstream_tools(&b, &["probe", "other"]);
        let mut b_entry = healthy_in_process_entry(Arc::clone(&b), b_tools);
        b_entry.exposure_policy =
            ToolExposurePolicy::from_patterns(vec!["other".into()]).expect("policy");
        pool.catalog.write().await.insert("b".to_string(), b_entry);

        // No route scope: only "a" exposes `probe` ("b" hides it).
        let all = pool
            .find_exposed_tool_candidates_allowed("probe", None)
            .await;
        assert_eq!(
            all.iter().map(|(u, _)| u.as_str()).collect::<Vec<_>>(),
            vec!["a"],
            "exposure policy must hide `probe` on upstream b"
        );

        // Route scope excluding "a" yields nothing.
        let scoped = BTreeSet::from(["b".to_string()]);
        assert!(
            pool.find_exposed_tool_candidates_allowed("probe", Some(&scoped))
                .await
                .is_empty(),
            "route scope must exclude `probe` on a non-allowed upstream"
        );
    }

    #[tokio::test]
    async fn mcp_app_sibling_lookup_respects_exposure_policy() {
        let pool = UpstreamPool::new();
        let upstream_name: Arc<str> = Arc::from("apps");
        let mut tools = test_upstream_tools(
            &upstream_name,
            &["youtube_search_ui", "youtube_probe", "internal_delete"],
        );
        tools
            .get_mut("youtube_search_ui")
            .expect("ui tool")
            .tool
            .meta = Some(Meta(serde_json::Map::from_iter([(
            "ui".to_string(),
            serde_json::json!({ "resourceUri": "ui://apps/youtube-search.html" }),
        )])));
        let mut entry = healthy_in_process_entry(Arc::clone(&upstream_name), tools);
        entry.exposure_policy = ToolExposurePolicy::from_patterns(vec![
            "youtube_search_ui".to_string(),
            "youtube_probe".to_string(),
        ])
        .expect("policy");
        pool.catalog.write().await.insert("apps".to_string(), entry);

        assert_eq!(
            pool.find_mcp_app_sibling_tool_candidates("youtube_probe", None)
                .await
                .len(),
            1
        );
        assert!(
            pool.find_mcp_app_sibling_tool_candidates("internal_delete", None)
                .await
                .is_empty(),
            "unexposed sibling tools must remain uncallable"
        );
    }

    // --- lab-tad5: oversized catalog bounds regression tests ---

    /// A gateway pool that receives more than `MAX_UPSTREAM_TOOLS` tools must cap
    /// the result at exactly the limit and not panic or allocate unboundedly.
    #[tokio::test]
    async fn gateway_upstream_tool_cap_truncates_oversized_catalog() {
        let pool = UpstreamPool::new();
        let upstream_name: Arc<str> = Arc::from("big-upstream");

        // Build more tools than the cap.
        let tool_names: Vec<String> = (0..MAX_UPSTREAM_TOOLS + 50)
            .map(|i| format!("tool_{i:04}"))
            .collect();
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

        let tool_names: Vec<String> = (0..MAX_UPSTREAM_TOOLS)
            .map(|i| format!("tool_{i:04}"))
            .collect();
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
