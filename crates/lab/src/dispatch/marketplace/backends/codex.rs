#![allow(dead_code)]

use std::path::{Path, PathBuf};

use lab_apis::marketplace::{
    Artifact, Marketplace, MarketplaceRuntime, Plugin, PluginInstallState, PluginSource,
};
use serde_json::Value;
use toml::Value as TomlValue;

use crate::dispatch::error::ToolError;
use crate::dispatch::marketplace::backend::{MarketplaceBackend, PluginFilter};
use crate::dispatch::marketplace::client;
use crate::dispatch::marketplace::package::{
    components_from_manifest_and_layout, manifest_summary_from_codex_manifest,
};
use crate::dispatch::marketplace::params::parse_plugin_id;

pub struct CodexMarketplaceBackend;

#[cfg(test)]
static TEST_CODEX_HOME_OVERRIDE: std::sync::Mutex<Option<PathBuf>> = std::sync::Mutex::new(None);
#[cfg(test)]
static TEST_CODEX_HOME_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

#[derive(Clone)]
struct CatalogSource {
    marketplace: Marketplace,
    plugins: Vec<Value>,
    base_dir: Option<PathBuf>,
}

impl CodexMarketplaceBackend {
    fn home_dir() -> Result<PathBuf, ToolError> {
        #[cfg(test)]
        if let Some(home) = TEST_CODEX_HOME_OVERRIDE.lock().unwrap().clone() {
            return Ok(home);
        }

        client::home_dir()
    }

    fn codex_config_path() -> Result<PathBuf, ToolError> {
        Ok(Self::home_dir()?.join(".codex").join("config.toml"))
    }

    fn codex_cache_root() -> Result<PathBuf, ToolError> {
        Ok(Self::home_dir()?.join(".codex").join("cache"))
    }

    fn absolute_path(path: PathBuf) -> PathBuf {
        if path.is_absolute() {
            path
        } else {
            std::env::current_dir()
                .unwrap_or_else(|_| PathBuf::from("."))
                .join(path)
        }
    }

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

    fn installed_plugin_names(&self) -> Vec<String> {
        let Ok(path) = Self::codex_config_path() else {
            return Vec::new();
        };
        let Ok(data) = std::fs::read_to_string(path) else {
            return Vec::new();
        };
        let Ok(table) = toml::from_str::<TomlValue>(&data) else {
            return Vec::new();
        };
        match table {
            TomlValue::Table(table) => table.keys().cloned().collect(),
            _ => Vec::new(),
        }
    }

    fn read_catalog_sources(&self) -> Result<Vec<CatalogSource>, ToolError> {
        let mut out = Vec::new();
        let home_catalog = Self::home_dir()?.join(".agents/plugins/marketplace.json");
        if home_catalog.exists() {
            out.push(self.catalog_from_path(
                "codex-personal",
                "Codex Personal",
                &home_catalog,
                None,
            )?);
        }
        if let Ok(cwd) = std::env::current_dir() {
            let repo_catalog = cwd.join(".agents/plugins/marketplace.json");
            if repo_catalog.exists() {
                out.push(self.catalog_from_path(
                    "codex-repo",
                    "Codex Repo",
                    &repo_catalog,
                    Some(cwd.clone()),
                )?);
            }
            let compat_catalog = cwd.join(".claude-plugin/marketplace.json");
            if compat_catalog.exists() {
                out.push(self.catalog_from_path(
                    "codex-compat",
                    "Codex Compatibility",
                    &compat_catalog,
                    Some(cwd),
                )?);
            }
        }
        Ok(out)
    }

    fn catalog_from_path(
        &self,
        id: &str,
        name: &str,
        path: &Path,
        base_dir: Option<PathBuf>,
    ) -> Result<CatalogSource, ToolError> {
        let base_dir = base_dir.or_else(|| path.parent().map(Path::to_path_buf));
        let value = Self::read_json(path)?;
        let (plugins, desc) = match &value {
            Value::Array(items) => (items.clone(), String::from("Codex plugin catalog")),
            Value::Object(map) => {
                let plugins = map
                    .get("plugins")
                    .and_then(Value::as_array)
                    .cloned()
                    .unwrap_or_default();
                let desc = map
                    .get("metadata")
                    .and_then(|m| m.get("description"))
                    .and_then(Value::as_str)
                    .unwrap_or("Codex plugin catalog")
                    .to_string();
                (plugins, desc)
            }
            _ => (Vec::new(), String::from("Codex plugin catalog")),
        };
        Ok(CatalogSource {
            marketplace: Marketplace {
                id: id.to_string(),
                name: name.to_string(),
                owner: "Local".into(),
                gh_user: String::new(),
                repo: None,
                source: PluginSource::Local,
                url: None,
                path: Some(path.to_string_lossy().into_owned()),
                desc,
                auto_update: false,
                total_plugins: plugins.len() as u32,
                last_updated: String::new(),
                runtime: Some(MarketplaceRuntime::Codex),
            },
            plugins,
            base_dir,
        })
    }

    fn source_dir_for_plugin(
        &self,
        marketplace: &str,
        plugin_json: &Value,
        base_dir: Option<&Path>,
    ) -> Option<PathBuf> {
        let explicit = plugin_json
            .get("path")
            .and_then(Value::as_str)
            .map(PathBuf::from);
        if let Some(path) = explicit {
            let root = base_dir.unwrap_or_else(|| Path::new("."));
            let joined = if path.is_absolute() {
                path
            } else {
                root.join(path)
            };
            // Reject traversal: the resolved path must stay within the catalog root.
            let canonical = std::fs::canonicalize(&joined).ok()?;
            let canonical_root = std::fs::canonicalize(root).ok()?;
            if !canonical.starts_with(&canonical_root) {
                tracing::warn!(
                    service = "marketplace",
                    event = "plugin.path.traversal_rejected",
                    path = %joined.display(),
                    root = %canonical_root.display(),
                    "plugin path resolves outside catalog root; skipping"
                );
                return None;
            }
            return Some(canonical);
        }
        if marketplace == "codex-repo" || marketplace == "codex-compat" {
            if let Some(base_dir) = base_dir {
                if base_dir.join(".codex-plugin/plugin.json").exists() {
                    return Some(Self::absolute_path(base_dir.to_path_buf()));
                }
            }
        }
        None
    }

    fn codex_manifest_for_source(&self, source: &Path) -> Option<Value> {
        let manifest = source.join(".codex-plugin").join("plugin.json");
        if !manifest.exists() {
            return None;
        }
        Self::read_json(&manifest).ok()
    }

    fn cache_path_for_plugin(
        &self,
        marketplace: &str,
        name: &str,
        version: Option<&str>,
    ) -> Option<PathBuf> {
        let cache_root = Self::codex_cache_root().ok()?;
        let base = cache_root.join(marketplace).join(name);
        if let Some(version) = version {
            let candidate = base.join(version);
            if candidate.exists() {
                return Some(Self::absolute_path(candidate));
            }
        }
        let Ok(entries) = std::fs::read_dir(&base) else {
            return None;
        };
        let mut dirs: Vec<PathBuf> = entries
            .flatten()
            .map(|e| e.path())
            .filter(|p| p.is_dir())
            .collect();
        // Sort by parsed semantic version so "1.10.0" sorts after "1.9.0".
        // Fall back to lexical order for non-semver names.
        dirs.sort_by(|a, b| {
            let va = a.file_name().and_then(|n| n.to_str()).map(parse_semver_key);
            let vb = b.file_name().and_then(|n| n.to_str()).map(parse_semver_key);
            va.cmp(&vb)
        });
        dirs.pop().map(Self::absolute_path)
    }

    fn plugin_from_catalog(&self, catalog: &CatalogSource, plugin_json: &Value) -> Option<Plugin> {
        let name = plugin_json.get("name").and_then(Value::as_str)?.to_string();
        let version = plugin_json
            .get("version")
            .and_then(Value::as_str)
            .map(ToString::to_string)
            .unwrap_or_default();
        let description = plugin_json
            .get("description")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
        let source_path = self.source_dir_for_plugin(
            &catalog.marketplace.id,
            plugin_json,
            catalog.base_dir.as_deref(),
        );
        let manifest_json = source_path
            .as_deref()
            .and_then(|path| self.codex_manifest_for_source(path));
        let components =
            components_from_manifest_and_layout(source_path.as_deref(), manifest_json.as_ref());
        let installed_names = self.installed_plugin_names();
        let installed = installed_names
            .iter()
            .any(|entry| entry.eq_ignore_ascii_case(&name));
        let cache_path = self.cache_path_for_plugin(&catalog.marketplace.id, &name, Some(&version));
        Some(Plugin {
            id: format!("{name}@{}", catalog.marketplace.id),
            name,
            mkt: catalog.marketplace.id.clone(),
            ver: version.clone(),
            desc: description.clone(),
            tags: Vec::new(),
            installed,
            has_update: None,
            installed_at: None,
            updated_at: None,
            runtime: Some(MarketplaceRuntime::Codex),
            enabled: Some(installed),
            marketplace_id: Some(catalog.marketplace.id.clone()),
            version: Some(version),
            description: Some(description),
            manifest: manifest_json
                .as_ref()
                .and_then(manifest_summary_from_codex_manifest),
            components: Some(components),
            install_state: Some(PluginInstallState {
                installed,
                enabled: Some(installed),
                installed_at: None,
                updated_at: None,
            }),
            // Store raw absolute filesystem paths — `list_artifacts` reads these
            // back as `PathBuf`. Display/log callers should apply `redact_home`
            // before exposing paths.
            source_path: source_path.map(|p| p.to_string_lossy().into_owned()),
            cache_path: cache_path.map(|p| p.to_string_lossy().into_owned()),
        })
    }

    fn find_plugin(&self, id: &str) -> Result<Plugin, ToolError> {
        let (name, marketplace_id) = parse_plugin_id(id)?;
        let catalogs = self.read_catalog_sources()?;
        for catalog in catalogs {
            if catalog.marketplace.id != marketplace_id {
                continue;
            }
            for plugin_json in &catalog.plugins {
                if plugin_json.get("name").and_then(Value::as_str) != Some(name) {
                    continue;
                }
                if let Some(plugin) = self.plugin_from_catalog(&catalog, plugin_json) {
                    return Ok(plugin);
                }
            }
        }
        Err(ToolError::Sdk {
            sdk_kind: "not_found".into(),
            message: format!("plugin `{id}` not found"),
        })
    }
}

impl MarketplaceBackend for CodexMarketplaceBackend {
    fn is_available(&self) -> bool {
        Self::home_dir().is_ok_and(|home| {
            home.join(".agents/plugins/marketplace.json").exists()
                || home.join(".codex/config.toml").exists()
                || std::env::current_dir().ok().is_some_and(|cwd| {
                    cwd.join(".agents/plugins/marketplace.json").exists()
                        || cwd.join(".claude-plugin/marketplace.json").exists()
                })
        })
    }

    fn list_sources(&self) -> Result<Vec<Marketplace>, ToolError> {
        Ok(self
            .read_catalog_sources()?
            .into_iter()
            .map(|source| source.marketplace)
            .collect())
    }

    fn list_plugins(&self, filter: PluginFilter) -> Result<Vec<Plugin>, ToolError> {
        let mut out = Vec::new();
        for catalog in self.read_catalog_sources()? {
            if let Some(ref marketplace) = filter.marketplace {
                if &catalog.marketplace.id != marketplace {
                    continue;
                }
            }
            for plugin_json in &catalog.plugins {
                if let Some(plugin) = self.plugin_from_catalog(&catalog, plugin_json) {
                    out.push(plugin);
                }
            }
        }
        Ok(out)
    }

    fn get_plugin(&self, id: &str) -> Result<Plugin, ToolError> {
        self.find_plugin(id)
    }

    fn list_artifacts(&self, id: &str) -> Result<Vec<Artifact>, ToolError> {
        let plugin = self.find_plugin(id)?;
        if let Some(source_path) = plugin.source_path.as_deref() {
            let root = PathBuf::from(source_path);
            return client::walk_artifacts(&root, &root);
        }
        if let Some(cache_path) = plugin.cache_path.as_deref() {
            let root = PathBuf::from(cache_path);
            return client::walk_artifacts(&root, &root);
        }
        Err(ToolError::Sdk {
            sdk_kind: "not_found".into(),
            message: format!("plugin `{id}` has no readable source or cache path"),
        })
    }

    fn list_components(
        &self,
        id: &str,
    ) -> Result<Vec<lab_apis::marketplace::PluginComponent>, ToolError> {
        let plugin = self.find_plugin(id)?;
        Ok(plugin.components.unwrap_or_default())
    }
}

/// Parse a version string into a comparable tuple so that `1.10.0` sorts after
/// `1.9.0`. Non-numeric components fall back to the raw string comparison.
fn parse_semver_key(v: &str) -> (u64, u64, u64, &str) {
    let mut parts = v.splitn(4, '.');
    let major = parts.next().and_then(|p| p.parse().ok()).unwrap_or(0);
    let minor = parts.next().and_then(|p| p.parse().ok()).unwrap_or(0);
    let patch = parts.next().and_then(|p| p.parse().ok()).unwrap_or(0);
    (major, minor, patch, v)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use tempfile::tempdir;

    fn with_home<T>(home: &Path, run: impl FnOnce() -> T) -> T {
        let _guard = TEST_CODEX_HOME_LOCK.lock().unwrap();
        let previous = {
            let mut slot = TEST_CODEX_HOME_OVERRIDE.lock().unwrap();
            std::mem::replace(&mut *slot, Some(home.to_path_buf()))
        };
        let result = run();
        let mut slot = TEST_CODEX_HOME_OVERRIDE.lock().unwrap();
        *slot = previous;
        result
    }

    fn seed_home_catalog(home: &Path, plugins: Value) {
        let catalog = home.join(".agents/plugins/marketplace.json");
        std::fs::create_dir_all(catalog.parent().unwrap()).unwrap();
        std::fs::write(catalog, json!({ "plugins": plugins }).to_string()).unwrap();
    }

    #[test]
    fn codex_source_path_is_absolute_and_usable_for_artifacts() {
        let dir = tempdir().unwrap();
        let home = dir.path();
        let source = home.join(".agents/plugins/demo-plugin");
        std::fs::create_dir_all(source.join(".codex-plugin")).unwrap();
        std::fs::write(
            source.join(".codex-plugin/plugin.json"),
            json!({ "name": "demo-plugin", "version": "1.0.0" }).to_string(),
        )
        .unwrap();
        std::fs::write(source.join("README.md"), "# Demo\n").unwrap();
        seed_home_catalog(
            home,
            json!([
                {
                    "name": "demo-plugin",
                    "version": "1.0.0",
                    "description": "demo",
                    "path": source
                }
            ]),
        );

        with_home(home, || {
            let backend = CodexMarketplaceBackend;
            let plugin = backend.get_plugin("demo-plugin@codex-personal").unwrap();
            let source_path = plugin.source_path.as_deref().unwrap();

            assert!(Path::new(source_path).is_absolute());
            assert!(!source_path.starts_with("~/"));

            let artifacts = backend
                .list_artifacts("demo-plugin@codex-personal")
                .unwrap();
            assert!(
                artifacts
                    .iter()
                    .any(|artifact| artifact.path == "README.md")
            );
        });
    }

    #[test]
    fn codex_cache_path_is_absolute_and_usable_for_artifacts() {
        let dir = tempdir().unwrap();
        let home = dir.path();
        let cache = home.join(".codex/cache/codex-personal/cache-plugin/1.0.0");
        std::fs::create_dir_all(&cache).unwrap();
        std::fs::write(cache.join("plugin.toml"), "name = \"cache-plugin\"\n").unwrap();
        seed_home_catalog(
            home,
            json!([
                {
                    "name": "cache-plugin",
                    "version": "1.0.0",
                    "description": "cache only"
                }
            ]),
        );

        with_home(home, || {
            let backend = CodexMarketplaceBackend;
            let plugin = backend.get_plugin("cache-plugin@codex-personal").unwrap();
            let cache_path = plugin.cache_path.as_deref().unwrap();

            assert!(Path::new(cache_path).is_absolute());
            assert!(!cache_path.starts_with("~/"));

            let artifacts = backend
                .list_artifacts("cache-plugin@codex-personal")
                .unwrap();
            assert!(
                artifacts
                    .iter()
                    .any(|artifact| artifact.path == "plugin.toml")
            );
        });
    }
}
