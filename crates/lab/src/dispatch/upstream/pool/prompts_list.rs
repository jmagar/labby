//! Prompt listing and ownership lookup.
//!
//! `collect_upstream_prompts` is the single shared RPC pass; `list_upstream_prompts`,
//! `prompt_ownership_map`, and `find_prompt_owner` are built on it, plus the cached
//! name/owner snapshot helpers. `cached_prompt_owner` is private and stays
//! co-located with its only caller (`find_prompt_owner`) — no `pub(super)` needed
//! (plan §2.1 drop note).

use std::collections::HashMap;

use futures::StreamExt;
use futures::stream::FuturesUnordered;
use rmcp::model::Prompt;

use super::super::types::UpstreamCapability;
use super::UpstreamPool;
use super::discover::routable_upstream_peers;
use super::helpers::merge_upstream_prompts;
use super::logging::is_capability_unsupported;
use super::tools::MAX_UPSTREAM_PROMPTS;

impl UpstreamPool {
    /// Fetch prompts from all healthy upstreams and merge them, returning both the
    /// deduplicated prompt list and the ownership map (prompt_name -> upstream_name).
    ///
    /// This is the single RPC pass shared by all prompt-related queries.
    async fn collect_upstream_prompts(
        &self,
        builtin_names: &[&str],
    ) -> (Vec<Prompt>, HashMap<String, String>) {
        let peers = routable_upstream_peers(self, UpstreamCapability::Prompts).await;

        // Issue RPCs in parallel. merge_upstream_prompts sorts internally,
        // so completion order does not affect the final result.
        let mut futures = FuturesUnordered::new();
        for (name, peer) in peers {
            futures.push(async move {
                let result = peer.list_prompts(None).await;
                (name, result)
            });
        }

        let mut upstream_prompts = Vec::new();
        let mut prompt_name_updates: HashMap<String, Vec<String>> = HashMap::new();
        while let Some((name, result)) = futures.next().await {
            match result {
                Ok(result) => {
                    self.record_success_for(&name, UpstreamCapability::Prompts)
                        .await;
                    prompt_name_updates.insert(name.clone(), Vec::new());
                    {
                        let mut catalog = self.catalog.write().await;
                        if let Some(entry) = catalog.get_mut(&name) {
                            entry.prompt_count = result.prompts.len();
                        }
                    }
                    upstream_prompts.push((name, result.prompts));
                }
                Err(e) if is_capability_unsupported(&e) => {
                    // The upstream simply doesn't implement `prompts/list`
                    // (JSON-RPC -32601). This is expected capability negotiation,
                    // not a failure: treat it like an empty, successful listing so
                    // the upstream stays routable and accrues no phantom failures.
                    self.record_success_for(&name, UpstreamCapability::Prompts)
                        .await;
                    prompt_name_updates.insert(name.clone(), Vec::new());
                    {
                        let mut catalog = self.catalog.write().await;
                        if let Some(entry) = catalog.get_mut(&name) {
                            entry.prompt_count = 0;
                        }
                    }
                    tracing::debug!(
                        upstream = %name,
                        error = %e,
                        "upstream does not implement prompts/list — capability absent"
                    );
                }
                Err(e) => {
                    self.record_failure_for(
                        &name,
                        UpstreamCapability::Prompts,
                        format!("failed to list prompts from upstream: {e}"),
                    )
                    .await;
                    {
                        let mut catalog = self.catalog.write().await;
                        if let Some(entry) = catalog.get_mut(&name) {
                            entry.prompt_count = 0;
                        }
                    }
                    tracing::warn!(
                        upstream = %name,
                        error = %e,
                        "failed to list prompts from upstream"
                    );
                }
            }
        }

        let (mut prompts, owners) = merge_upstream_prompts(builtin_names, upstream_prompts);
        if prompts.len() > MAX_UPSTREAM_PROMPTS {
            prompts.truncate(MAX_UPSTREAM_PROMPTS);
            tracing::warn!(
                limit = MAX_UPSTREAM_PROMPTS,
                "upstream prompt catalog exceeds limit — truncating to cap"
            );
        }
        if !prompt_name_updates.is_empty() {
            for prompt in &prompts {
                if let Some(upstream_name) = owners.get(prompt.name.as_str())
                    && let Some(names) = prompt_name_updates.get_mut(upstream_name)
                {
                    names.push(prompt.name.to_string());
                }
            }
            let mut catalog = self.catalog.write().await;
            for (upstream_name, names) in prompt_name_updates {
                if let Some(entry) = catalog.get_mut(&upstream_name) {
                    entry.prompt_names = names;
                }
            }
        }

        (prompts, owners)
    }

    /// List prompts from all healthy upstreams, filtering built-in and cross-upstream collisions.
    pub async fn list_upstream_prompts(&self, builtin_names: &[&str]) -> Vec<Prompt> {
        let (prompts, _) = self.collect_upstream_prompts(builtin_names).await;
        prompts
    }

    /// Return cached prompt names from all upstreams, excluding any that clash with builtins.
    pub async fn cached_upstream_prompt_names(&self, builtins: &[&str]) -> Vec<String> {
        let catalog = self.catalog.read().await;
        catalog
            .values()
            .flat_map(|entry| entry.prompt_names.iter().cloned())
            .filter(|name| !builtins.contains(&name.as_str()))
            .collect()
    }

    /// Return cached prompt names keyed by upstream name.
    ///
    /// Used by inspection actions (e.g. `gateway.discovered_prompts`) to answer
    /// from already-populated catalog data without issuing live RPCs to all upstreams.
    /// Entries whose `prompt_health` is not routable are excluded.
    pub async fn cached_upstream_prompt_names_by_upstream(&self) -> Vec<(String, Vec<String>)> {
        let catalog = self.catalog.read().await;
        catalog
            .iter()
            .filter(|(_, entry)| {
                entry.prompt_health.is_routable() && !entry.prompt_names.is_empty()
            })
            .map(|(name, entry)| (name.clone(), entry.prompt_names.clone()))
            .collect()
    }

    async fn cached_prompt_owner(
        &self,
        prompt_name: &str,
        require_routable: bool,
    ) -> Option<String> {
        let catalog = self.catalog.read().await;
        let mut entries = catalog.values().collect::<Vec<_>>();
        entries.sort_unstable_by(|left, right| left.name.cmp(&right.name));
        entries.into_iter().find_map(|entry| {
            if require_routable && !entry.prompt_health.is_routable() {
                return None;
            }
            entry
                .prompt_names
                .iter()
                .any(|name| name == prompt_name)
                .then(|| entry.name.to_string())
        })
    }

    /// Build prompt ownership map: prompt_name -> upstream_name.
    ///
    /// Makes M RPCs (one per healthy upstream), not M*N. Use this when you need
    /// to look up ownership for multiple prompts.
    pub async fn prompt_ownership_map(&self, builtin_names: &[&str]) -> HashMap<String, String> {
        let (_, owners) = self.collect_upstream_prompts(builtin_names).await;
        owners
    }

    /// Resolve which upstream owns a given prompt name.
    ///
    /// Prefer `prompt_ownership_map()` when resolving ownership for multiple
    /// prompts to avoid an N+1 RPC pattern.
    pub async fn find_prompt_owner(&self, prompt_name: &str) -> Option<String> {
        if let Some(owner) = self.cached_prompt_owner(prompt_name, true).await {
            return Some(owner);
        }

        let (_, owners) = self.collect_upstream_prompts(&[]).await;
        if let Some(owner) = owners.get(prompt_name) {
            return Some(owner.clone());
        }

        self.cached_prompt_owner(prompt_name, false).await
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicU8, Ordering};

    use rmcp::model::{GetPromptRequestParams, LoggingLevel, Prompt};
    use tokio::sync::RwLock;

    use crate::mcp::logging::logging_level_rank;
    use crate::mcp::server::LabMcpServer;
    use crate::registry::ToolRegistry;

    use super::super::super::types;
    use super::super::helpers::merge_upstream_prompts;
    use super::super::testsupport::*;
    use super::*;

    #[test]
    fn merge_upstream_prompts_is_deterministic() {
        let left = Prompt::new("shared", Some("left"), None);
        let right = Prompt::new("shared", Some("right"), None);
        let left_only = Prompt::new("left-only", Some("left-only"), None);
        let right_only = Prompt::new("right-only", Some("right-only"), None);

        let (prompts, owners) = merge_upstream_prompts(
            &["builtin"],
            vec![
                ("zeta".into(), vec![right.clone(), right_only]),
                ("alpha".into(), vec![left.clone(), left_only]),
            ],
        );

        // Every prompt is namespaced by its owning upstream, so the two `shared`
        // prompts no longer collide — both survive with distinct names. Ordering
        // is deterministic: upstreams sorted (alpha < zeta), prompts in order.
        let names: Vec<_> = prompts.iter().map(|prompt| prompt.name.as_str()).collect();
        assert_eq!(
            names,
            vec![
                "alpha/shared",
                "alpha/left-only",
                "zeta/shared",
                "zeta/right-only",
            ]
        );
        assert_eq!(
            owners.get("alpha/shared").map(String::as_str),
            Some("alpha")
        );
        assert_eq!(owners.get("zeta/shared").map(String::as_str), Some("zeta"));
        assert_eq!(
            owners.get("alpha/left-only").map(String::as_str),
            Some("alpha")
        );
        assert_eq!(
            owners.get("zeta/right-only").map(String::as_str),
            Some("zeta")
        );
    }

    #[tokio::test]
    async fn successful_prompt_listing_populates_snapshot_cache() {
        let pool = static_catalog_pool("static").await;

        let prompts = pool.list_upstream_prompts(&[]).await;
        let prompt_names: Vec<&str> = prompts.iter().map(|prompt| prompt.name.as_str()).collect();
        // Prompt names are namespaced by their owning upstream.
        assert_eq!(
            prompt_names,
            vec!["static/upstream.prompt.one", "static/upstream.prompt.two"]
        );
        assert_eq!(
            pool.cached_upstream_prompt_names(&[]).await,
            vec![
                "static/upstream.prompt.one".to_string(),
                "static/upstream.prompt.two".to_string()
            ]
        );

        let runtime = crate::dispatch::gateway::manager::GatewayRuntimeHandle::default();
        runtime.swap(Some(Arc::clone(&pool))).await;
        let manager = Arc::new(crate::dispatch::gateway::manager::GatewayManager::new(
            std::path::PathBuf::from("config.toml"),
            runtime,
        ));
        let server = LabMcpServer {
            registry: Arc::new(ToolRegistry::new()),
            gateway_manager: Some(manager),
            node_role: None,
            peers: Arc::new(RwLock::new(Vec::new())),
            logging_level: Arc::new(AtomicU8::new(logging_level_rank(LoggingLevel::Info))),
        };

        let snapshot = server.snapshot_catalog().await;
        assert!(snapshot.prompts.contains("static/upstream.prompt.one"));
        assert!(snapshot.prompts.contains("static/upstream.prompt.two"));
    }

    #[tokio::test]
    async fn prompt_owner_lookup_uses_cache_without_listing_upstreams() {
        let server = StaticCatalogServer::default();
        let list_prompts_count = Arc::clone(&server.list_prompts_count);
        let get_prompt_count = Arc::clone(&server.get_prompt_count);
        let pool = static_catalog_pool_with_server("static", server).await;

        let prompts = pool.list_upstream_prompts(&[]).await;
        assert_eq!(prompts.len(), 2);
        assert_eq!(list_prompts_count.load(Ordering::SeqCst), 1);

        let owner = pool.find_prompt_owner("static/upstream.prompt.one").await;
        assert_eq!(owner.as_deref(), Some("static"));
        assert_eq!(list_prompts_count.load(Ordering::SeqCst), 1);

        // The gateway-facing name is namespaced; `get_prompt` strips the
        // `{upstream}/` prefix before forwarding the bare name to the upstream.
        let result = pool
            .get_prompt(
                "static",
                GetPromptRequestParams::new("static/upstream.prompt.one"),
            )
            .await
            .expect("upstream remains connected")
            .expect("prompt get succeeds");
        assert_eq!(result.messages.len(), 1);
        assert_eq!(get_prompt_count.load(Ordering::SeqCst), 1);
        assert_eq!(list_prompts_count.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn prompt_owner_lookup_falls_back_to_stale_cache_after_listing_miss() {
        let server = StaticCatalogServer::default();
        let list_prompts_count = Arc::clone(&server.list_prompts_count);
        let fail_list_prompts = Arc::clone(&server.fail_list_prompts);
        let pool = static_catalog_pool_with_server("static", server).await;

        let prompts = pool.list_upstream_prompts(&[]).await;
        assert_eq!(prompts.len(), 2);
        assert_eq!(list_prompts_count.load(Ordering::SeqCst), 1);

        for _ in 0..types::CIRCUIT_BREAKER_THRESHOLD {
            pool.record_failure_for(
                "static",
                UpstreamCapability::Prompts,
                "prompt listing failed for test",
            )
            .await;
        }
        fail_list_prompts.store(true, Ordering::SeqCst);

        let owner = pool.find_prompt_owner("static/upstream.prompt.one").await;
        assert_eq!(owner.as_deref(), Some("static"));
        assert_eq!(list_prompts_count.load(Ordering::SeqCst), 1);
        assert_eq!(
            pool.cached_upstream_prompt_names(&[]).await,
            vec![
                "static/upstream.prompt.one".to_string(),
                "static/upstream.prompt.two".to_string()
            ]
        );
    }
}
