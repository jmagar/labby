//! Shared catalog module — single source of truth for service + action
//! discovery, feeding three surfaces: the `lab.help` MCP meta-tool, the
//! `lab://catalog` MCP resource, and the `labby help` CLI subcommand.

use serde::{Deserialize, Serialize};

use crate::registry::ToolRegistry;

/// Top-level discovery document.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Catalog {
    /// One entry per registered service.
    pub services: Vec<ServiceCatalog>,
}

#[allow(dead_code)]
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Transport {
    Stdio,
    Http,
}

/// Per-service slice of the discovery document.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServiceCatalog {
    /// Service identifier (matches the MCP tool name and CLI subcommand).
    pub name: String,
    /// Short human description from `PluginMeta::description`.
    pub description: String,
    /// Category slug (Network, Notifications, Bootstrap, etc.).
    pub category: String,
    /// Implementation status: `"available"` or `"stub"`.
    ///
    /// Filter on `status == "available"` to find services that are callable.
    /// `"stub"` means compiled-in but not yet dispatching real actions.
    pub status: String,
    /// Whether invocation requires a transport-resolved caller identity.
    #[serde(default)]
    pub caller_bound: bool,
    /// True when the service requires an authenticated HTTP request context and
    /// therefore must be hidden from stdio catalogs.
    #[serde(default)]
    pub requires_http_subject: bool,
    /// List of actions exposed by the service.
    pub actions: Vec<ActionEntry>,
}

/// One action inside a service's catalog.
///
/// Includes the full parameter list so agents can plan from a single
/// `lab://catalog` resource read without issuing per-action `schema` calls.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ActionEntry {
    /// Dotted action name (e.g., `status.get`).
    pub name: String,
    /// Short description.
    pub description: String,
    /// Whether the action can cause permanent, hard-to-recreate data loss and
    /// therefore requires destructive-action confirmation.
    pub destructive: bool,
    /// Whether this action requires an administrator authorization context.
    #[serde(default)]
    pub requires_admin: bool,
    /// Additional OAuth scopes required by the action.
    #[serde(default)]
    pub required_scopes: Vec<String>,
    /// Declared parameters for this action. Empty when the action takes no params.
    pub params: Vec<ParamEntry>,
    /// Type-name hint for the return shape, e.g. `"Movie[]"`. Informational only.
    pub returns: String,
}

/// One declared parameter in an action's catalog entry.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ParamEntry {
    /// Parameter name.
    pub name: String,
    /// Free-form type label: `"string"`, `"integer"`, `"boolean"`, `"object"`,
    /// `"array"`, union literals like `"string|null"`, or enum literals like
    /// `"queued|running|done"`.
    pub ty: String,
    /// Whether this parameter must be present for the action to succeed.
    pub required: bool,
    /// Human-readable description of the parameter.
    pub description: String,
}

/// Build a [`Catalog`] from the current tool registry.
#[must_use]
pub fn build_catalog(registry: &ToolRegistry) -> Catalog {
    let services = registry
        .services()
        .iter()
        .map(|svc| ServiceCatalog {
            name: svc.name.to_string(),
            description: svc.description.to_string(),
            category: svc.category.to_string(),
            status: svc.status.to_string(),
            caller_bound: registry.dispatch_capability(svc.name)
                == Some(crate::registry::DispatchCapability::CallerBound),
            requires_http_subject: false,
            actions: svc
                .actions
                .iter()
                .map(|a| ActionEntry {
                    name: a.name.into(),
                    description: a.description.into(),
                    destructive: a.destructive,
                    requires_admin: a.requires_admin,
                    required_scopes: if a.requires_admin {
                        vec!["lab:admin".to_string()]
                    } else {
                        Vec::new()
                    },
                    returns: a.returns.into(),
                    params: a
                        .params
                        .iter()
                        .map(|p| ParamEntry {
                            name: p.name.into(),
                            ty: p.ty.into(),
                            required: p.required,
                            description: p.description.into(),
                        })
                        .collect(),
                })
                .collect(),
        })
        .collect();

    Catalog { services }
}

#[allow(dead_code)]
#[must_use]
pub fn actions_for(catalog: &Catalog, service: &str, transport: Transport) -> Vec<ActionEntry> {
    let Some(entry) = catalog.services.iter().find(|entry| entry.name == service) else {
        return Vec::new();
    };

    if matches!(transport, Transport::Stdio) && entry.requires_http_subject {
        return Vec::new();
    }

    entry.actions.clone()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn catalog_hides_oauth_upstreams_on_stdio() {
        let catalog = Catalog {
            services: vec![ServiceCatalog {
                name: "oauth-upstream".to_string(),
                description: "OAuth protected upstream".to_string(),
                category: "Gateway".to_string(),
                status: "available".to_string(),
                caller_bound: false,
                requires_http_subject: true,
                actions: vec![ActionEntry {
                    name: "tool.call".to_string(),
                    description: "Call upstream tool".to_string(),
                    destructive: false,
                    requires_admin: false,
                    required_scopes: vec![],
                    params: vec![],
                    returns: String::new(),
                }],
            }],
        };

        assert!(actions_for(&catalog, "oauth-upstream", Transport::Stdio).is_empty());
        assert_eq!(
            actions_for(&catalog, "oauth-upstream", Transport::Http).len(),
            1
        );
    }
}
