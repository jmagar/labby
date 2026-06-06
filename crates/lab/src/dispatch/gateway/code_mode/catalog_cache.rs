//! Disk cache of the Code Mode upstream catalog for one-shot CLI invocations.
//!
//! `labby gateway code exec` builds the `codemode.*` JS proxy from the upstream
//! tool catalog. The MCP surface refreshes that catalog from a long-lived pool,
//! but a one-shot CLI process would have to connect every configured stdio
//! upstream per invocation just to generate the proxy. This cache persists the
//! per-upstream tool lists (fingerprinted against the upstream config and
//! bounded by a TTL) so repeat CLI invocations connect zero upstreams for proxy
//! generation; tool calls still resolve live via
//! `resolve_code_mode_upstream_tool`, so a stale cache can only omit or
//! over-offer `codemode.*` helpers — never execute against stale state.
//!
//! Concurrency model: one-shot invocations may read/write concurrently. Writes
//! are atomic (temp file + rename) and merge into a freshly loaded copy, so the
//! worst case for a lost race is a redundant refresh on a later run. Any parse
//! failure is treated as a cache miss.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};

use crate::config::UpstreamConfig;
use crate::dispatch::upstream::types::UpstreamTool;

const CACHE_VERSION: u32 = 1;
/// How long a cached upstream catalog stays valid. The fingerprint catches
/// config edits; the TTL catches upstream-side tool drift (server upgrades)
/// that no config change reflects.
const CACHE_TTL: Duration = Duration::from_secs(6 * 60 * 60);

#[derive(Default, Serialize, Deserialize)]
pub(crate) struct CatalogCache {
    version: u32,
    upstreams: HashMap<String, CachedUpstreamCatalog>,
}

#[derive(Clone, Serialize, Deserialize)]
struct CachedUpstreamCatalog {
    /// Fingerprint of the upstream config this entry was captured under.
    fingerprint: String,
    saved_at_unix: u64,
    tools: Vec<CachedTool>,
}

#[derive(Clone, Serialize, Deserialize)]
struct CachedTool {
    tool: rmcp::model::Tool,
    input_schema: Option<Value>,
    output_schema: Option<Value>,
    destructive: bool,
}

/// A pending cache update for one upstream, produced after a live probe.
pub(crate) struct CatalogCacheUpdate {
    pub(crate) upstream_name: String,
    pub(crate) fingerprint: String,
    pub(crate) tools: Vec<UpstreamTool>,
}

pub(crate) fn cache_path() -> PathBuf {
    crate::dispatch::setup::lab_home()
        .join("cache")
        .join("codemode-catalog.json")
}

/// Stable fingerprint of an upstream config entry.
pub(crate) fn fingerprint(config: &UpstreamConfig) -> String {
    let serialized = serde_json::to_string(config).unwrap_or_else(|_| format!("{:?}", config.name));
    let digest = Sha256::digest(serialized.as_bytes());
    digest.iter().map(|b| format!("{b:02x}")).collect()
}

impl CatalogCache {
    /// Load the cache from disk. Missing, unreadable, corrupt, or
    /// version-mismatched files are all treated as an empty cache.
    pub(crate) fn load() -> Self {
        let path = cache_path();
        let Ok(bytes) = std::fs::read(&path) else {
            return Self::default();
        };
        match serde_json::from_slice::<Self>(&bytes) {
            Ok(cache) if cache.version == CACHE_VERSION => cache,
            Ok(_) | Err(_) => {
                tracing::debug!(
                    path = %path.display(),
                    "code_mode catalog cache unreadable or version-mismatched; treating as empty"
                );
                Self::default()
            }
        }
    }

    /// Return the cached tools for `upstream_name` when the entry matches
    /// `fingerprint` and is within the TTL.
    pub(crate) fn fresh_tools(
        &self,
        upstream_name: &str,
        fingerprint: &str,
    ) -> Option<Vec<UpstreamTool>> {
        let entry = self.upstreams.get(upstream_name)?;
        if entry.fingerprint != fingerprint {
            return None;
        }
        let age = now_unix().checked_sub(entry.saved_at_unix)?;
        if age > CACHE_TTL.as_secs() {
            return None;
        }
        let name: Arc<str> = Arc::from(upstream_name);
        Some(
            entry
                .tools
                .iter()
                .cloned()
                .map(|cached| UpstreamTool {
                    tool: cached.tool,
                    input_schema: cached.input_schema,
                    output_schema: cached.output_schema,
                    upstream_name: Arc::clone(&name),
                    destructive: cached.destructive,
                })
                .collect(),
        )
    }
}

/// Merge `updates` into the on-disk cache and persist atomically.
///
/// Loads a fresh copy first so concurrent invocations updating different
/// upstreams do not clobber each other's entries (last-writer-wins per file,
/// but each write carries the latest visible merge). Failed-to-probe upstreams
/// must NOT be passed here — leaving them absent means the next run retries.
pub(crate) fn merge_and_store(updates: Vec<CatalogCacheUpdate>) {
    if updates.is_empty() {
        return;
    }
    let mut cache = CatalogCache::load();
    cache.version = CACHE_VERSION;
    let saved_at_unix = now_unix();
    for update in updates {
        let tools = update
            .tools
            .into_iter()
            .map(|tool| CachedTool {
                tool: tool.tool,
                input_schema: tool.input_schema,
                output_schema: tool.output_schema,
                destructive: tool.destructive,
            })
            .collect();
        cache.upstreams.insert(
            update.upstream_name,
            CachedUpstreamCatalog {
                fingerprint: update.fingerprint,
                saved_at_unix,
                tools,
            },
        );
    }

    let path = cache_path();
    if let Err(error) = persist_atomic(&path, &cache) {
        tracing::warn!(
            path = %path.display(),
            error = %error,
            "failed to persist code_mode catalog cache"
        );
    }
}

fn persist_atomic(path: &std::path::Path, cache: &CatalogCache) -> std::io::Result<()> {
    let parent = path
        .parent()
        .ok_or_else(|| std::io::Error::other("cache path has no parent directory"))?;
    std::fs::create_dir_all(parent)?;
    let bytes = serde_json::to_vec(cache).map_err(std::io::Error::other)?;
    let tmp = parent.join(format!(".codemode-catalog.{}.tmp", std::process::id()));
    std::fs::write(&tmp, bytes)?;
    let renamed = std::fs::rename(&tmp, path);
    if renamed.is_err() {
        drop(std::fs::remove_file(&tmp));
    }
    renamed
}

fn now_unix() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_tool(name: &str) -> UpstreamTool {
        UpstreamTool {
            tool: rmcp::model::Tool::new(
                name.to_string(),
                "test tool",
                Arc::new(serde_json::Map::new()),
            ),
            input_schema: Some(serde_json::json!({"type": "object"})),
            output_schema: None,
            upstream_name: Arc::from("alpha"),
            destructive: true,
        }
    }

    #[test]
    fn fresh_tools_round_trips_through_serde() {
        let mut cache = CatalogCache {
            version: CACHE_VERSION,
            upstreams: HashMap::new(),
        };
        cache.upstreams.insert(
            "alpha".to_string(),
            CachedUpstreamCatalog {
                fingerprint: "fp".to_string(),
                saved_at_unix: now_unix(),
                tools: vec![CachedTool {
                    tool: test_tool("ping").tool,
                    input_schema: Some(serde_json::json!({"type": "object"})),
                    output_schema: None,
                    destructive: true,
                }],
            },
        );

        let bytes = serde_json::to_vec(&cache).expect("serializes");
        let restored: CatalogCache = serde_json::from_slice(&bytes).expect("deserializes");
        let tools = restored.fresh_tools("alpha", "fp").expect("entry is fresh");
        assert_eq!(tools.len(), 1);
        assert_eq!(tools[0].tool.name.as_ref(), "ping");
        assert!(tools[0].destructive, "destructive flag survives the cache");
        assert_eq!(tools[0].upstream_name.as_ref(), "alpha");
    }

    #[test]
    fn fresh_tools_rejects_fingerprint_mismatch_and_expired_entries() {
        let mut cache = CatalogCache {
            version: CACHE_VERSION,
            upstreams: HashMap::new(),
        };
        cache.upstreams.insert(
            "alpha".to_string(),
            CachedUpstreamCatalog {
                fingerprint: "fp".to_string(),
                saved_at_unix: now_unix() - CACHE_TTL.as_secs() - 60,
                tools: Vec::new(),
            },
        );
        cache.upstreams.insert(
            "beta".to_string(),
            CachedUpstreamCatalog {
                fingerprint: "old-fp".to_string(),
                saved_at_unix: now_unix(),
                tools: Vec::new(),
            },
        );

        assert!(cache.fresh_tools("alpha", "fp").is_none(), "expired");
        assert!(
            cache.fresh_tools("beta", "new-fp").is_none(),
            "fingerprint mismatch"
        );
        assert!(cache.fresh_tools("missing", "fp").is_none());
    }

    #[test]
    fn fingerprint_is_stable_and_config_sensitive() {
        let config = UpstreamConfig {
            enabled: true,
            name: "alpha".to_string(),
            url: None,
            bearer_token_env: None,
            command: Some("true".to_string()),
            args: vec![],
            env: std::collections::BTreeMap::new(),
            proxy_resources: false,
            proxy_prompts: false,
            expose_tools: None,
            expose_resources: None,
            expose_prompts: None,
            oauth: None,
            imported_from: None,
            priority: 1.0,
        };
        let mut changed = config.clone();
        changed.args = vec!["--flag".to_string()];

        assert_eq!(fingerprint(&config), fingerprint(&config));
        assert_ne!(fingerprint(&config), fingerprint(&changed));
    }
}
