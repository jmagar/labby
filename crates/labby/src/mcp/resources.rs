#![allow(dead_code)]

//! MCP resource handlers.
//!
//! Exposes `lab://catalog` (the full discovery document) and
//! `lab://<service>/actions` (per-service action list). Resources are
//! read-only and derived from the shared catalog.
use anyhow::Result;
use serde_json::Value;

use crate::{catalog::build_catalog, registry::ToolRegistry};

/// Render the `lab://catalog` resource as JSON.
pub fn catalog_json(registry: &ToolRegistry) -> Result<Value> {
    let filtered;
    let registry = if crate::registry::lab_show_all_enabled() {
        registry
    } else {
        filtered = crate::registry::filter_by_configured_env(registry);
        &filtered
    };
    let catalog = build_catalog(registry);
    Ok(serde_json::to_value(catalog)?)
}

/// Render the `lab://<service>/actions` resource for one service.
pub fn service_actions_json(registry: &ToolRegistry, service: &str) -> Result<Value> {
    let filtered;
    let registry = if crate::registry::lab_show_all_enabled() {
        registry
    } else {
        filtered = crate::registry::filter_by_configured_env(registry);
        &filtered
    };
    let catalog = build_catalog(registry);
    let entry = catalog
        .services
        .into_iter()
        .find(|s| s.name == service)
        .ok_or_else(|| anyhow::anyhow!("unknown service: {service}"))?;
    Ok(serde_json::to_value(entry.actions)?)
}
