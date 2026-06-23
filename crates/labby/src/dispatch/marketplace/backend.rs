#![allow(dead_code)]

use labby_apis::marketplace::{Artifact, Marketplace, Plugin, PluginComponent};

use crate::dispatch::error::ToolError;

#[derive(Debug, Clone, Default)]
pub struct PluginFilter {
    pub marketplace: Option<String>,
}

pub trait MarketplaceBackend {
    fn is_available(&self) -> bool;
    fn list_sources(&self) -> Result<Vec<Marketplace>, ToolError>;
    fn list_plugins(&self, filter: PluginFilter) -> Result<Vec<Plugin>, ToolError>;
    fn get_plugin(&self, id: &str) -> Result<Plugin, ToolError>;
    fn list_artifacts(&self, id: &str) -> Result<Vec<Artifact>, ToolError>;
    fn list_components(&self, id: &str) -> Result<Vec<PluginComponent>, ToolError>;
}
