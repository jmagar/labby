#![allow(dead_code)]

#[cfg(any(feature = "marketplace", feature = "acp_registry"))]
use lab_apis::marketplace::PluginSource;
use lab_apis::marketplace::{Artifact, Marketplace, MarketplaceRuntime, Plugin, PluginComponent};
use serde_json::Value;

use crate::config;
use crate::dispatch::error::ToolError;
use crate::dispatch::marketplace::backend::{MarketplaceBackend, PluginFilter};
use crate::dispatch::marketplace::backends::claude::ClaudeMarketplaceBackend;
use crate::dispatch::marketplace::backends::codex::CodexMarketplaceBackend;
use crate::dispatch::marketplace::client::join_err;
use crate::dispatch::marketplace::runtime::{parse_marketplace_runtime, runtime_display_name};

pub fn runtime_from_params(params: &Value) -> Result<Option<MarketplaceRuntime>, ToolError> {
    params
        .get("runtime")
        .and_then(Value::as_str)
        .map(parse_marketplace_runtime)
        .transpose()
}

fn claude_backend() -> ClaudeMarketplaceBackend {
    ClaudeMarketplaceBackend
}

fn codex_backend() -> CodexMarketplaceBackend {
    CodexMarketplaceBackend
}

pub fn list_plugins_sync(
    runtime: Option<MarketplaceRuntime>,
    filter: Option<String>,
) -> Result<Vec<Plugin>, ToolError> {
    let filter = PluginFilter {
        marketplace: filter,
    };
    match runtime {
        Some(MarketplaceRuntime::Claude) => claude_backend().list_plugins(filter),
        Some(MarketplaceRuntime::Codex) => codex_backend().list_plugins(filter),
        Some(MarketplaceRuntime::Gemini) => Ok(Vec::new()),
        None => {
            let mut out = Vec::new();
            let claude = claude_backend();
            if claude.is_available() {
                out.extend(claude.list_plugins(filter.clone())?);
            }
            let codex = codex_backend();
            if codex.is_available() {
                out.extend(codex.list_plugins(filter)?);
            }
            Ok(out)
        }
    }
}

pub async fn sources_list(
    runtime: Option<MarketplaceRuntime>,
) -> Result<Vec<Marketplace>, ToolError> {
    let mut sources = tokio::task::spawn_blocking(move || match runtime {
        Some(MarketplaceRuntime::Claude) => claude_backend().list_sources(),
        Some(MarketplaceRuntime::Codex) => codex_backend().list_sources(),
        Some(MarketplaceRuntime::Gemini) => Ok(Vec::new()),
        None => {
            let mut out = Vec::new();
            let claude = claude_backend();
            if claude.is_available() {
                out.extend(claude.list_sources()?);
            }
            let codex = codex_backend();
            if codex.is_available() {
                out.extend(codex.list_sources()?);
            }
            Ok(out)
        }
    })
    .await
    .map_err(join_err)??;

    append_mcp_registry_source(&mut sources).await;
    #[cfg(feature = "acp_registry")]
    append_acp_registry_source(&mut sources).await;

    Ok(sources)
}

pub async fn plugins_list(
    runtime: Option<MarketplaceRuntime>,
    filter: Option<String>,
) -> Result<Vec<Plugin>, ToolError> {
    tokio::task::spawn_blocking(move || list_plugins_sync(runtime, filter))
        .await
        .map_err(join_err)?
}

pub async fn plugin_get(
    runtime: Option<MarketplaceRuntime>,
    id: &str,
) -> Result<Plugin, ToolError> {
    let id = id.to_string();
    tokio::task::spawn_blocking(move || get_plugin_sync(runtime, &id))
        .await
        .map_err(join_err)?
}

pub async fn plugin_artifacts(
    runtime: Option<MarketplaceRuntime>,
    id: &str,
) -> Result<Vec<Artifact>, ToolError> {
    let id = id.to_string();
    tokio::task::spawn_blocking(move || match runtime {
        Some(MarketplaceRuntime::Claude) => claude_backend().list_artifacts(&id),
        Some(MarketplaceRuntime::Codex) => codex_backend().list_artifacts(&id),
        Some(MarketplaceRuntime::Gemini) => Err(unsupported_runtime_action(
            MarketplaceRuntime::Gemini,
            "plugin.artifacts",
        )),
        None => {
            if let Ok(plugin) = get_plugin_sync(None, &id) {
                match plugin.runtime {
                    Some(MarketplaceRuntime::Claude) => claude_backend().list_artifacts(&id),
                    Some(MarketplaceRuntime::Codex) => codex_backend().list_artifacts(&id),
                    Some(MarketplaceRuntime::Gemini) => Err(unsupported_runtime_action(
                        MarketplaceRuntime::Gemini,
                        "plugin.artifacts",
                    )),
                    None => Err(ToolError::Sdk {
                        sdk_kind: "not_found".into(),
                        message: format!("plugin `{id}` not found"),
                    }),
                }
            } else {
                Err(ToolError::Sdk {
                    sdk_kind: "not_found".into(),
                    message: format!("plugin `{id}` not found"),
                })
            }
        }
    })
    .await
    .map_err(join_err)?
}

pub async fn plugin_components(
    runtime: Option<MarketplaceRuntime>,
    id: &str,
) -> Result<Vec<PluginComponent>, ToolError> {
    let id = id.to_string();
    tokio::task::spawn_blocking(move || match runtime {
        Some(MarketplaceRuntime::Claude) => claude_backend().list_components(&id),
        Some(MarketplaceRuntime::Codex) => codex_backend().list_components(&id),
        Some(MarketplaceRuntime::Gemini) => Ok(Vec::new()),
        None => {
            let plugin = get_plugin_sync(None, &id)?;
            match plugin.runtime {
                Some(MarketplaceRuntime::Claude) => claude_backend().list_components(&id),
                Some(MarketplaceRuntime::Codex) => codex_backend().list_components(&id),
                Some(MarketplaceRuntime::Gemini) | None => Ok(Vec::new()),
            }
        }
    })
    .await
    .map_err(join_err)?
}

pub fn require_claude_write(
    runtime: Option<MarketplaceRuntime>,
    action: &str,
) -> Result<(), ToolError> {
    match runtime {
        Some(MarketplaceRuntime::Codex) => Err(unsupported_runtime_action(
            MarketplaceRuntime::Codex,
            action,
        )),
        Some(MarketplaceRuntime::Gemini) => Err(unsupported_runtime_action(
            MarketplaceRuntime::Gemini,
            action,
        )),
        _ => Ok(()),
    }
}

fn get_plugin_sync(runtime: Option<MarketplaceRuntime>, id: &str) -> Result<Plugin, ToolError> {
    match runtime {
        Some(MarketplaceRuntime::Claude) => claude_backend().get_plugin(id),
        Some(MarketplaceRuntime::Codex) => codex_backend().get_plugin(id),
        Some(MarketplaceRuntime::Gemini) => Err(ToolError::Sdk {
            sdk_kind: "not_found".into(),
            message: format!("plugin `{id}` not found"),
        }),
        None => {
            let mut matches = Vec::new();
            let claude = claude_backend();
            if claude.is_available() {
                if let Ok(plugin) = claude.get_plugin(id) {
                    matches.push(plugin);
                }
            }
            let codex = codex_backend();
            if codex.is_available() {
                if let Ok(plugin) = codex.get_plugin(id) {
                    matches.push(plugin);
                }
            }
            match matches.len() {
                0 => Err(ToolError::Sdk {
                    sdk_kind: "not_found".into(),
                    message: format!("plugin `{id}` not found"),
                }),
                1 => Ok(matches.remove(0)),
                _ => Err(ToolError::Conflict {
                    message: format!(
                        "plugin `{id}` exists in multiple runtimes; pass `runtime` explicitly"
                    ),
                    existing_id: id.to_string(),
                }),
            }
        }
    }
}

fn unsupported_runtime_action(runtime: MarketplaceRuntime, action: &str) -> ToolError {
    ToolError::InvalidParam {
        message: format!(
            "action `{action}` is not supported for runtime `{}` in Phase 1",
            runtime_display_name(runtime)
        ),
        param: "runtime".into(),
    }
}

const MCP_REGISTRY_SOURCE_ID: &str = "mcp-registry";
const MCP_REGISTRY_SOURCE_NAME: &str = "MCP Registry";
#[cfg(feature = "acp_registry")]
const ACP_REGISTRY_SOURCE_ID: &str = "acp-registry";
#[cfg(feature = "acp_registry")]
const ACP_REGISTRY_SOURCE_NAME: &str = "ACP Registry";

async fn append_mcp_registry_source(sources: &mut Vec<Marketplace>) {
    if sources
        .iter()
        .any(|source| source.id == MCP_REGISTRY_SOURCE_ID)
    {
        return;
    }

    sources.push(mcp_registry_marketplace(
        &configured_mcp_registry_url_or_default(),
        local_mcp_registry_server_count().await,
    ));
}

#[cfg(feature = "acp_registry")]
async fn append_acp_registry_source(sources: &mut Vec<Marketplace>) {
    if sources
        .iter()
        .any(|source| source.id == ACP_REGISTRY_SOURCE_ID)
    {
        return;
    }

    sources.push(acp_registry_marketplace(
        &crate::dispatch::marketplace::acp_client::configured_registry_url(),
        acp_registry_agent_count().await,
    ));
}

fn configured_mcp_registry_url_or_default() -> String {
    match config::load_toml(&config::toml_candidates()) {
        Ok(cfg) => config::mcpregistry_url(&cfg).to_string(),
        Err(error) => {
            tracing::warn!(
                service = "marketplace",
                source = MCP_REGISTRY_SOURCE_ID,
                error = %error,
                "falling back to default MCP Registry URL for marketplace source"
            );
            config::DEFAULT_MCPREGISTRY_URL.to_string()
        }
    }
}

async fn local_mcp_registry_server_count() -> u32 {
    let db_path = config::registry_db_path();
    if !db_path.exists() {
        return 0;
    }

    let store = match crate::dispatch::marketplace::store::RegistryStore::open(&db_path).await {
        Ok(store) => store,
        Err(error) => {
            tracing::warn!(
                service = "marketplace",
                source = MCP_REGISTRY_SOURCE_ID,
                path = %db_path.display(),
                error = %error,
                "could not open local MCP Registry store for marketplace source count"
            );
            return 0;
        }
    };

    match store.count_latest_servers().await {
        Ok(count) => count,
        Err(error) => {
            tracing::warn!(
                service = "marketplace",
                source = MCP_REGISTRY_SOURCE_ID,
                path = %db_path.display(),
                error = %error,
                "could not count local MCP Registry servers for marketplace source"
            );
            0
        }
    }
}

fn mcp_registry_marketplace(url: &str, total_plugins: u32) -> Marketplace {
    Marketplace {
        id: MCP_REGISTRY_SOURCE_ID.to_string(),
        name: MCP_REGISTRY_SOURCE_NAME.to_string(),
        owner: "Model Context Protocol".to_string(),
        gh_user: "modelcontextprotocol".to_string(),
        repo: None,
        source: PluginSource::Git,
        url: Some(url.to_string()),
        path: None,
        desc: "Official MCP server registry mirrored into Marketplace.".to_string(),
        auto_update: true,
        total_plugins,
        last_updated: String::new(),
        runtime: None,
    }
}

#[cfg(feature = "acp_registry")]
async fn acp_registry_agent_count() -> u32 {
    let client = match crate::dispatch::marketplace::acp_client::require_acp_client() {
        Ok(client) => client,
        Err(error) => {
            tracing::warn!(
                service = "marketplace",
                source = ACP_REGISTRY_SOURCE_ID,
                error = %error,
                "could not build ACP Registry client for marketplace source count"
            );
            return 0;
        }
    };

    match client.list_agents().await {
        Ok(agents) => u32::try_from(agents.len()).unwrap_or(u32::MAX),
        Err(error) => {
            tracing::warn!(
                service = "marketplace",
                source = ACP_REGISTRY_SOURCE_ID,
                error = %error,
                "could not count ACP Registry agents for marketplace source"
            );
            0
        }
    }
}

#[cfg(feature = "acp_registry")]
fn acp_registry_marketplace(url: &str, total_plugins: u32) -> Marketplace {
    Marketplace {
        id: ACP_REGISTRY_SOURCE_ID.to_string(),
        name: ACP_REGISTRY_SOURCE_NAME.to_string(),
        owner: "Agent Client Protocol".to_string(),
        gh_user: String::new(),
        repo: None,
        source: PluginSource::Git,
        url: Some(url.to_string()),
        path: None,
        desc: "Official ACP agent registry mirrored into Marketplace.".to_string(),
        auto_update: true,
        total_plugins,
        last_updated: String::new(),
        runtime: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mcp_registry_marketplace_uses_marketplace_source_identity() {
        let source = mcp_registry_marketplace("https://registry.modelcontextprotocol.io", 42);

        assert_eq!(source.id, MCP_REGISTRY_SOURCE_ID);
        assert_eq!(source.name, MCP_REGISTRY_SOURCE_NAME);
        assert_eq!(source.owner, "Model Context Protocol");
        assert_eq!(source.gh_user, "modelcontextprotocol");
        assert_eq!(source.source, PluginSource::Git);
        assert_eq!(
            source.url.as_deref(),
            Some("https://registry.modelcontextprotocol.io")
        );
        assert!(source.auto_update);
        assert_eq!(source.total_plugins, 42);
    }

    #[cfg(feature = "acp_registry")]
    #[test]
    fn acp_registry_marketplace_uses_marketplace_source_identity() {
        let source = acp_registry_marketplace("https://cdn.agentclientprotocol.com", 7);

        assert_eq!(source.id, ACP_REGISTRY_SOURCE_ID);
        assert_eq!(source.name, ACP_REGISTRY_SOURCE_NAME);
        assert_eq!(source.owner, "Agent Client Protocol");
        assert_eq!(source.gh_user, "");
        assert_eq!(source.source, PluginSource::Git);
        assert_eq!(
            source.url.as_deref(),
            Some("https://cdn.agentclientprotocol.com")
        );
        assert!(source.auto_update);
        assert_eq!(source.total_plugins, 7);
    }
}
