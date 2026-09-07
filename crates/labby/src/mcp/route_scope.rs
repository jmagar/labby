use std::collections::{BTreeMap, BTreeSet};

use sha2::{Digest, Sha256};

use crate::config::{GatewayLoadoutConfig, ProtectedGatewaySubsetTarget, ProtectedMcpRouteConfig};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct McpRouteCapabilityGates {
    expose_tools: bool,
    expose_resources: bool,
    expose_prompts: bool,
    expose_skills: bool,
    expose_code_mode: bool,
}

impl McpRouteCapabilityGates {
    fn all(expose_code_mode: bool) -> Self {
        Self {
            expose_tools: true,
            expose_resources: true,
            expose_prompts: true,
            expose_skills: true,
            expose_code_mode,
        }
    }

    fn from_loadout(loadout: &GatewayLoadoutConfig) -> Self {
        Self {
            expose_tools: loadout.expose_tools,
            expose_resources: loadout.expose_resources,
            expose_prompts: loadout.expose_prompts,
            expose_skills: loadout.expose_skills,
            expose_code_mode: loadout.expose_code_mode,
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) enum McpRouteScope {
    #[default]
    Root,
    ProtectedSubset {
        route_name: String,
        upstreams: BTreeSet<String>,
        services: BTreeSet<String>,
        expose_tools: bool,
        expose_resources: bool,
        expose_prompts: bool,
        expose_skills: bool,
        expose_code_mode: bool,
        authority_partition: String,
        team_id: Option<String>,
        credential_bindings: BTreeMap<String, (String, u64)>,
    },
}

impl McpRouteScope {
    pub(crate) fn protected_subset<I, J, S, T>(
        route_name: impl Into<String>,
        upstreams: I,
        services: J,
        expose_code_mode: bool,
    ) -> Self
    where
        I: IntoIterator<Item = S>,
        J: IntoIterator<Item = T>,
        S: AsRef<str>,
        T: AsRef<str>,
    {
        Self::protected_subset_with_capabilities(
            route_name,
            upstreams,
            services,
            McpRouteCapabilityGates::all(expose_code_mode),
        )
    }

    pub(crate) fn protected_subset_with_capabilities<I, J, S, T>(
        route_name: impl Into<String>,
        upstreams: I,
        services: J,
        capabilities: McpRouteCapabilityGates,
    ) -> Self
    where
        I: IntoIterator<Item = S>,
        J: IntoIterator<Item = T>,
        S: AsRef<str>,
        T: AsRef<str>,
    {
        Self::ProtectedSubset {
            route_name: route_name.into(),
            upstreams: upstreams
                .into_iter()
                .map(|name| name.as_ref().to_string())
                .collect(),
            services: services
                .into_iter()
                .map(|name| name.as_ref().to_string())
                .collect(),
            expose_tools: capabilities.expose_tools,
            expose_resources: capabilities.expose_resources,
            expose_prompts: capabilities.expose_prompts,
            expose_skills: capabilities.expose_skills,
            expose_code_mode: capabilities.expose_code_mode,
            authority_partition: "unbound".to_owned(),
            team_id: None,
            credential_bindings: BTreeMap::new(),
        }
    }

    pub(crate) fn from_protected_route(
        route: &ProtectedMcpRouteConfig,
        loadouts: &[GatewayLoadoutConfig],
    ) -> Result<Option<Self>, String> {
        let Some(target): Option<&ProtectedGatewaySubsetTarget> = route.gateway_subset_target()
        else {
            return Ok(None);
        };
        if let Some(loadout_name) = target.loadout.as_deref() {
            let loadout = loadouts
                .iter()
                .find(|loadout| loadout.name == loadout_name)
                .ok_or_else(|| {
                    format!(
                        "protected MCP route `{}` references missing loadout `{loadout_name}`; create the loadout or update the route",
                        route.name
                    )
                })?;
            let effective = loadout.intersect_gateway_subset(target).map_err(|_| {
                format!(
                    "protected MCP route `{}` loadout binding is inconsistent",
                    route.name
                )
            })?;
            let mut scope = Self::protected_subset_with_capabilities(
                route.name.clone(),
                effective.upstreams.iter().map(String::as_str),
                effective.services.iter().map(String::as_str),
                McpRouteCapabilityGates::from_loadout(&effective),
            );
            scope.bind_loadout_authority(&effective);
            return Ok(Some(scope));
        }
        Ok(Some(Self::protected_subset(
            route.name.clone(),
            target.upstreams.iter().map(String::as_str),
            target.services.iter().map(String::as_str),
            target.expose_code_mode,
        )))
    }

    pub(crate) fn label(&self) -> String {
        match self {
            Self::Root => "root".to_string(),
            Self::ProtectedSubset { route_name, .. } => format!("protected:{route_name}"),
        }
    }

    fn bind_loadout_authority(&mut self, loadout: &GatewayLoadoutConfig) {
        let Self::ProtectedSubset {
            authority_partition,
            team_id,
            credential_bindings,
            ..
        } = self
        else {
            return;
        };
        let mut digest = Sha256::new();
        *team_id = loadout
            .name
            .strip_prefix("team:")
            .and_then(|rest| rest.split_once(':'))
            .map(|(team, _)| team.to_owned());
        for binding in &loadout.credential_bindings {
            for value in [
                binding.upstream_name.as_bytes(),
                binding.binding_id.as_bytes(),
                &binding.generation.to_be_bytes(),
            ] {
                digest.update((value.len() as u64).to_be_bytes());
                digest.update(value);
            }
            credential_bindings.insert(
                binding.upstream_name.clone(),
                (binding.binding_id.clone(), binding.generation),
            );
        }
        *authority_partition = hex::encode(digest.finalize());
    }

    pub(crate) fn protected_history_label(&self) -> Option<String> {
        match self {
            Self::Root => None,
            Self::ProtectedSubset { .. } => Some(self.label()),
        }
    }

    pub(crate) fn allows_service(&self, service: &str) -> bool {
        match self {
            Self::Root => true,
            Self::ProtectedSubset { services, .. } => services.contains(service),
        }
    }

    pub(crate) fn allows_upstream(&self, upstream: &str) -> bool {
        match self {
            Self::Root => true,
            Self::ProtectedSubset { upstreams, .. } => upstreams.contains(upstream),
        }
    }

    pub(crate) fn exposes_tools(&self) -> bool {
        match self {
            Self::Root => true,
            Self::ProtectedSubset { expose_tools, .. } => *expose_tools,
        }
    }

    pub(crate) fn exposes_resources(&self) -> bool {
        match self {
            Self::Root => true,
            Self::ProtectedSubset {
                expose_resources, ..
            } => *expose_resources,
        }
    }

    pub(crate) fn exposes_prompts(&self) -> bool {
        match self {
            Self::Root => true,
            Self::ProtectedSubset { expose_prompts, .. } => *expose_prompts,
        }
    }

    pub(crate) fn exposes_skills(&self) -> bool {
        match self {
            Self::Root => true,
            Self::ProtectedSubset { expose_skills, .. } => *expose_skills,
        }
    }

    pub(crate) fn is_root(&self) -> bool {
        matches!(self, Self::Root)
    }

    pub(crate) fn matches_product_route(&self, route_id: &str) -> bool {
        matches!(self, Self::ProtectedSubset { route_name, .. } if route_name == route_id)
    }

    pub(crate) fn exposes_code_mode(&self) -> bool {
        match self {
            Self::Root => true,
            Self::ProtectedSubset {
                expose_code_mode, ..
            } => *expose_code_mode,
        }
    }

    pub(crate) fn allowed_upstreams(&self) -> Option<&BTreeSet<String>> {
        match self {
            Self::Root => None,
            Self::ProtectedSubset { upstreams, .. } => Some(upstreams),
        }
    }

    pub(crate) fn team_credential_subject(&self) -> Option<String> {
        match self {
            Self::ProtectedSubset { team_id, .. } => {
                team_id.as_ref().map(|team| format!("team:{team}"))
            }
            Self::Root => None,
        }
    }

    pub(crate) fn team_credential_binding(&self, upstream: &str) -> Option<(&str, &str, u64)> {
        match self {
            Self::ProtectedSubset {
                team_id: Some(team_id),
                credential_bindings,
                ..
            } => credential_bindings
                .get(upstream)
                .map(|(binding_id, generation)| {
                    (team_id.as_str(), binding_id.as_str(), *generation)
                }),
            _ => None,
        }
    }

    #[cfg(feature = "gateway")]
    pub(crate) fn task_authorization(
        &self,
    ) -> labby_gateway::upstream::pool::TaskRouteAuthorization {
        let route_key = match self {
            Self::Root => self.label(),
            Self::ProtectedSubset {
                authority_partition,
                ..
            } => format!("{}:{authority_partition}", self.label()),
        };
        labby_gateway::upstream::pool::TaskRouteAuthorization::new(
            route_key,
            self.allowed_upstreams().cloned(),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn root_allows_everything() {
        let scope = McpRouteScope::Root;
        assert!(scope.allows_service("gateway"));
        assert!(scope.allows_upstream("gateway-alpha"));
        assert!(scope.exposes_tools());
        assert!(scope.exposes_resources());
        assert!(scope.exposes_prompts());
        assert!(scope.exposes_skills());
        assert!(scope.exposes_code_mode());
        assert!(scope.is_root());
        assert_eq!(scope.label(), "root");
    }

    #[test]
    fn protected_subset_allows_only_configured_names() {
        let scope = McpRouteScope::protected_subset(
            "ops",
            ["gateway-alpha", "gateway-beta"],
            ["gateway"],
            true,
        );
        assert!(scope.allows_service("gateway"));
        assert!(!scope.allows_service("logs"));
        assert!(scope.allows_upstream("gateway-alpha"));
        assert!(!scope.allows_upstream("hidden-upstream"));
        assert!(scope.exposes_tools());
        assert!(scope.exposes_resources());
        assert!(scope.exposes_prompts());
        assert!(scope.exposes_skills());
        assert!(scope.exposes_code_mode());
        assert!(!scope.is_root());
        assert_eq!(scope.label(), "protected:ops");
    }

    #[test]
    fn product_route_binding_matches_only_the_exact_protected_route() {
        let scope = McpRouteScope::protected_subset("team", ["depot"], ["skills"], false);
        assert!(scope.matches_product_route("team"));
        assert!(!scope.matches_product_route("other"));
        assert!(!McpRouteScope::Root.matches_product_route("team"));
    }

    #[test]
    fn protected_subset_can_hide_code_mode() {
        let scope = McpRouteScope::protected_subset("ops", ["unifi"], ["device"], false);
        assert!(!scope.exposes_code_mode());
    }

    #[test]
    fn loadout_resolves_capability_gates_and_names() {
        let route = ProtectedMcpRouteConfig {
            name: "ops-route".to_string(),
            enabled: true,
            public_host: "mcp.example.com".to_string(),
            public_path: "/ops".to_string(),
            upstream: None,
            backend_url: String::new(),
            backend_mcp_path: "/mcp".to_string(),
            scopes: vec![],
            health_path: None,
            target: Some(crate::config::ProtectedMcpRouteTarget::GatewaySubset(
                ProtectedGatewaySubsetTarget {
                    loadout: Some("ops".to_string()),
                    ..Default::default()
                },
            )),
        };
        let loadouts = vec![GatewayLoadoutConfig {
            name: "ops".to_string(),
            upstreams: vec!["axon".to_string()],
            services: vec!["device".to_string()],
            expose_tools: false,
            expose_resources: true,
            expose_prompts: false,
            expose_skills: true,
            expose_code_mode: true,
            ..GatewayLoadoutConfig::default()
        }];
        let scope = McpRouteScope::from_protected_route(&route, &loadouts)
            .expect("loadout resolution")
            .expect("gateway subset");
        assert!(scope.allows_upstream("axon"));
        assert!(!scope.allows_upstream("hidden"));
        assert!(scope.allows_service("device"));
        assert!(!scope.exposes_tools());
        assert!(scope.exposes_resources());
        assert!(!scope.exposes_prompts());
        assert!(scope.exposes_skills());
        assert!(scope.exposes_code_mode());
    }

    #[cfg(feature = "gateway")]
    #[test]
    fn credential_rotation_invalidates_retained_task_authority() {
        use labby_runtime::gateway_config::GatewayCredentialBindingRef;
        let route = ProtectedMcpRouteConfig {
            name: "team-route".into(),
            enabled: true,
            public_host: "mcp.example.com".into(),
            public_path: "/team".into(),
            upstream: None,
            backend_url: String::new(),
            backend_mcp_path: "/mcp".into(),
            scopes: vec![],
            health_path: None,
            target: Some(crate::config::ProtectedMcpRouteTarget::GatewaySubset(
                ProtectedGatewaySubsetTarget {
                    loadout: Some("team:alpha:prod".into()),
                    ..Default::default()
                },
            )),
        };
        let loadout = |generation| GatewayLoadoutConfig {
            name: "team:alpha:prod".into(),
            upstreams: vec!["shared".into()],
            credential_bindings: vec![GatewayCredentialBindingRef {
                upstream_name: "shared".into(),
                binding_id: "alpha-binding".into(),
                generation,
            }],
            ..GatewayLoadoutConfig::default()
        };
        let before = McpRouteScope::from_protected_route(&route, &[loadout(1)])
            .unwrap()
            .unwrap();
        let after = McpRouteScope::from_protected_route(&route, &[loadout(2)])
            .unwrap()
            .unwrap();
        assert_ne!(before.task_authorization(), after.task_authorization());
        assert_eq!(before.label(), after.label());
        assert_eq!(
            before.team_credential_subject().as_deref(),
            Some("team:alpha")
        );
        assert_eq!(
            before.team_credential_binding("shared"),
            Some(("alpha", "alpha-binding", 1))
        );
        assert_eq!(before.team_credential_binding("other"), None);
    }

    #[test]
    fn missing_loadout_returns_course_correcting_error() {
        let route = ProtectedMcpRouteConfig {
            name: "ops-route".to_string(),
            enabled: true,
            public_host: "mcp.example.com".to_string(),
            public_path: "/ops".to_string(),
            upstream: None,
            backend_url: String::new(),
            backend_mcp_path: "/mcp".to_string(),
            scopes: vec![],
            health_path: None,
            target: Some(crate::config::ProtectedMcpRouteTarget::GatewaySubset(
                ProtectedGatewaySubsetTarget {
                    loadout: Some("missing".to_string()),
                    ..Default::default()
                },
            )),
        };
        let error = McpRouteScope::from_protected_route(&route, &[]).expect_err("missing loadout");
        assert!(error.contains("missing loadout"));
        assert!(error.contains("create the loadout or update the route"));
    }
}
