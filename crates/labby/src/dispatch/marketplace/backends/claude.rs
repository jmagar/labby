#![allow(dead_code)]

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use labby_apis::marketplace::{
    Artifact, Marketplace, MarketplaceRuntime, Plugin, PluginInstallState, PluginSource,
};
use serde_json::{Map, Value};

use crate::dispatch::error::ToolError;
use crate::dispatch::marketplace::backend::{MarketplaceBackend, PluginFilter};
use crate::dispatch::marketplace::client;
use crate::dispatch::marketplace::package::{
    components_from_manifest_and_layout, manifest_summary_from_marketplace_plugin, redact_home,
    tags_from_marketplace_plugin,
};
use crate::dispatch::marketplace::params::parse_plugin_id;

pub struct ClaudeMarketplaceBackend;

struct MarketplaceManifest {
    display_name: Option<String>,
    owner_name: Option<String>,
    description: Option<String>,
    plugins: Vec<Value>,
}

struct InstalledRecord {
    install_path: PathBuf,
    installed_at: String,
    last_updated: String,
}

impl ClaudeMarketplaceBackend {
    fn read_json(path: &Path) -> Result<Value, ToolError> {
        let bytes = std::fs::read(path).map_err(|e| ToolError::Sdk {
            sdk_kind: "internal_error".into(),
            message: format!("read {}: {e}", path.display()),
        })?;
        serde_json::from_slice(&bytes).map_err(|e| ToolError::Sdk {
            sdk_kind: "decode_error".into(),
            message: format!("parse {}: {e}", path.display()),
        })
    }

    fn parse_source(
        m: &Map<String, Value>,
    ) -> (
        PluginSource,
        Option<String>,
        Option<String>,
        Option<String>,
        String,
    ) {
        let kind = m.get("source").and_then(Value::as_str).unwrap_or("local");
        let url = m
            .get("url")
            .and_then(Value::as_str)
            .map(ToString::to_string);
        let repo = m
            .get("repo")
            .and_then(Value::as_str)
            .map(ToString::to_string);
        let path = m
            .get("path")
            .and_then(Value::as_str)
            .map(ToString::to_string);
        let gh_user = repo
            .as_deref()
            .and_then(|r| r.split('/').next())
            .unwrap_or("")
            .to_string();
        let source = match kind {
            "github" => PluginSource::Github,
            "git" => PluginSource::Git,
            _ => PluginSource::Local,
        };
        (source, url, repo, path, gh_user)
    }

    fn read_marketplace_manifest(install_loc: &Path) -> Option<MarketplaceManifest> {
        let candidates = [
            install_loc.join(".claude-plugin").join("marketplace.json"),
            install_loc.join("marketplace.json"),
        ];
        for path in candidates {
            if !path.exists() {
                continue;
            }
            let v = match Self::read_json(&path) {
                Ok(v) => v,
                Err(e) => {
                    tracing::warn!(
                        service = "marketplace",
                        event = "manifest.parse_failed",
                        path = %path.display(),
                        error = %e,
                        "marketplace.json parse error; skipping"
                    );
                    continue;
                }
            };
            let plugins = v
                .get("plugins")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default();
            let display_name = v
                .get("name")
                .and_then(Value::as_str)
                .map(ToString::to_string);
            let owner_name = v
                .get("owner")
                .and_then(|o| o.get("name"))
                .and_then(Value::as_str)
                .map(ToString::to_string);
            let description = v
                .get("metadata")
                .and_then(|m| m.get("description"))
                .and_then(Value::as_str)
                .map(ToString::to_string);
            return Some(MarketplaceManifest {
                display_name,
                owner_name,
                description,
                plugins,
            });
        }
        None
    }

    fn load_known_marketplaces(&self) -> Result<Vec<Marketplace>, ToolError> {
        let path = client::claude_plugins_root()?.join("known_marketplaces.json");
        if !path.exists() {
            return Ok(Vec::new());
        }
        let v = Self::read_json(&path)?;
        let Some(obj) = v.as_object() else {
            return Ok(Vec::new());
        };
        let mut out = Vec::with_capacity(obj.len());
        for (id, entry) in obj {
            let src = entry.get("source").cloned().unwrap_or(Value::Null);
            let (source, url, repo, path_val, gh_user) = match src {
                Value::Object(m) => Self::parse_source(&m),
                _ => (PluginSource::Local, None, None, None, String::new()),
            };
            let auto_update = entry
                .get("autoUpdate")
                .and_then(Value::as_bool)
                .unwrap_or(false);
            let last_updated = entry
                .get("lastUpdated")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string();
            let install_loc = entry
                .get("installLocation")
                .and_then(Value::as_str)
                .map(PathBuf::from);
            let manifest = install_loc
                .as_deref()
                .and_then(Self::read_marketplace_manifest);
            let plugin_count = manifest.as_ref().map(|m| m.plugins.len()).unwrap_or(0);
            let display_name = manifest
                .as_ref()
                .and_then(|m| m.display_name.clone())
                .unwrap_or_else(|| id.clone());
            let owner = manifest
                .as_ref()
                .and_then(|m| m.owner_name.clone())
                .unwrap_or_else(|| gh_user.clone());
            let desc = manifest
                .as_ref()
                .and_then(|m| m.description.clone())
                .unwrap_or_default();
            out.push(Marketplace {
                id: id.clone(),
                name: display_name,
                owner,
                gh_user,
                repo,
                source,
                url,
                path: path_val,
                desc,
                auto_update,
                total_plugins: plugin_count as u32,
                last_updated,
                runtime: Some(MarketplaceRuntime::Claude),
            });
        }
        Ok(out)
    }

    fn load_installed(&self) -> Result<HashMap<String, InstalledRecord>, ToolError> {
        let path = client::claude_plugins_root()?.join("installed_plugins.json");
        if !path.exists() {
            return Ok(HashMap::new());
        }
        let v = Self::read_json(&path)?;
        let Some(obj) = v.get("plugins").and_then(Value::as_object) else {
            return Ok(HashMap::new());
        };
        let mut out = HashMap::new();
        for (id, list) in obj {
            if let Some(first) = list.as_array().and_then(|a| a.first()) {
                out.insert(
                    id.clone(),
                    InstalledRecord {
                        install_path: first
                            .get("installPath")
                            .and_then(Value::as_str)
                            .map(PathBuf::from)
                            .unwrap_or_default(),
                        installed_at: first
                            .get("installedAt")
                            .and_then(Value::as_str)
                            .unwrap_or("")
                            .to_string(),
                        last_updated: first
                            .get("lastUpdated")
                            .and_then(Value::as_str)
                            .unwrap_or("")
                            .to_string(),
                    },
                );
            }
        }
        Ok(out)
    }

    fn source_path_for_plugin(&self, id: &str) -> Result<PathBuf, ToolError> {
        let (name, marketplace) = parse_plugin_id(id)?;
        let root = client::claude_plugins_root()?;
        let candidate = root.join("marketplaces").join(marketplace).join(name);
        if candidate.exists() {
            let canonical = std::fs::canonicalize(&candidate).map_err(client::io_internal)?;
            let canonical_root = std::fs::canonicalize(&root).map_err(client::io_internal)?;
            if !canonical.starts_with(&canonical_root) {
                return Err(ToolError::InvalidParam {
                    message: format!("plugin id `{id}` resolves outside the marketplace root"),
                    param: "id".into(),
                });
            }
            return Ok(candidate);
        }
        let installed = self.load_installed()?;
        let rec = installed.get(id).ok_or_else(|| ToolError::Sdk {
            sdk_kind: "not_found".into(),
            message: format!("plugin `{id}` is not installed"),
        })?;
        Ok(rec.install_path.clone())
    }

    fn build_plugin(
        &self,
        mkt_id: &str,
        plugin_json: &Value,
        installed: &HashMap<String, InstalledRecord>,
    ) -> Option<Plugin> {
        let name = plugin_json.get("name").and_then(Value::as_str)?.to_string();
        let id = format!("{name}@{mkt_id}");
        let ver = plugin_json
            .get("version")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
        let desc = plugin_json
            .get("description")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
        let tags = tags_from_marketplace_plugin(plugin_json);
        let rec = installed.get(&id);
        Some(Plugin {
            id,
            name,
            mkt: mkt_id.to_string(),
            ver: ver.clone(),
            desc: desc.clone(),
            tags,
            installed: rec.is_some(),
            has_update: None,
            installed_at: rec.map(|r| r.installed_at.clone()),
            updated_at: rec.map(|r| r.last_updated.clone()),
            runtime: Some(MarketplaceRuntime::Claude),
            enabled: rec.map(|_| true),
            marketplace_id: Some(mkt_id.to_string()),
            version: Some(ver),
            description: Some(desc),
            manifest: manifest_summary_from_marketplace_plugin(plugin_json),
            components: None,
            install_state: Some(PluginInstallState {
                installed: rec.is_some(),
                enabled: rec.map(|_| true),
                installed_at: rec.map(|r| r.installed_at.clone()),
                updated_at: rec.map(|r| r.last_updated.clone()),
            }),
            source_path: None,
            cache_path: rec.map(|r| redact_home(&r.install_path.to_string_lossy())),
        })
    }
}

impl MarketplaceBackend for ClaudeMarketplaceBackend {
    fn is_available(&self) -> bool {
        client::claude_plugins_root()
            .ok()
            .is_some_and(|path| path.exists())
    }

    fn list_sources(&self) -> Result<Vec<Marketplace>, ToolError> {
        self.load_known_marketplaces()
    }

    fn list_plugins(&self, filter: PluginFilter) -> Result<Vec<Plugin>, ToolError> {
        let markets = self.load_known_marketplaces()?;
        let installed = self.load_installed()?;
        let mut out = Vec::new();
        for market in markets {
            if let Some(ref requested) = filter.marketplace {
                if &market.id != requested {
                    continue;
                }
            }
            let install_loc = client::claude_plugins_root()?
                .join("marketplaces")
                .join(&market.id);
            let Some(manifest) = Self::read_marketplace_manifest(&install_loc) else {
                continue;
            };
            for plugin_json in &manifest.plugins {
                if let Some(mut plugin) = self.build_plugin(&market.id, plugin_json, &installed) {
                    // The frontend catalog expands each plugin into one item
                    // per component, so list_plugins must populate components.
                    // Per-plugin disk walk is acknowledged perf hazard — see
                    // build_plugin_leaves_cache_path_and_components_none test
                    // for the no-walk invariant on the base path.
                    let source = install_loc.join(&plugin.name);
                    plugin.components =
                        Some(components_from_manifest_and_layout(Some(&source), None));
                    out.push(plugin);
                }
            }
        }
        Ok(out)
    }

    fn get_plugin(&self, id: &str) -> Result<Plugin, ToolError> {
        let (name, marketplace) = parse_plugin_id(id)?;
        let installed = self.load_installed()?;
        let install_loc = client::claude_plugins_root()?
            .join("marketplaces")
            .join(marketplace);
        let Some(manifest) = Self::read_marketplace_manifest(&install_loc) else {
            return Err(ToolError::Sdk {
                sdk_kind: "not_found".into(),
                message: format!("plugin `{id}` not found"),
            });
        };
        for plugin_json in &manifest.plugins {
            if plugin_json.get("name").and_then(Value::as_str) != Some(name) {
                continue;
            }
            let mut plugin = self
                .build_plugin(marketplace, plugin_json, &installed)
                .ok_or_else(|| ToolError::Sdk {
                    sdk_kind: "not_found".into(),
                    message: format!("plugin `{id}` not found"),
                })?;
            let source = self.source_path_for_plugin(id)?;
            plugin.source_path = Some(source.to_string_lossy().into_owned());
            plugin.components = Some(components_from_manifest_and_layout(Some(&source), None));
            return Ok(plugin);
        }
        Err(ToolError::Sdk {
            sdk_kind: "not_found".into(),
            message: format!("plugin `{id}` not found"),
        })
    }

    fn list_artifacts(&self, id: &str) -> Result<Vec<Artifact>, ToolError> {
        let source = self.source_path_for_plugin(id)?;
        client::walk_artifacts(&source, &source)
    }

    fn list_components(
        &self,
        id: &str,
    ) -> Result<Vec<labby_apis::marketplace::PluginComponent>, ToolError> {
        let source = self.source_path_for_plugin(id)?;
        Ok(components_from_manifest_and_layout(Some(&source), None))
    }
}
