use std::collections::BTreeSet;

use crate::config::{ProtectedGatewaySubsetTarget, ProtectedMcpRouteConfig};

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) enum McpRouteScope {
    #[default]
    Root,
    ProtectedSubset {
        route_name: String,
        upstreams: BTreeSet<String>,
        services: BTreeSet<String>,
        expose_code_mode: bool,
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
            expose_code_mode,
        }
    }

    pub(crate) fn from_protected_route(route: &ProtectedMcpRouteConfig) -> Option<Self> {
        let target: &ProtectedGatewaySubsetTarget = route.gateway_subset_target()?;
        Some(Self::protected_subset(
            route.name.clone(),
            target.upstreams.iter().map(String::as_str),
            target.services.iter().map(String::as_str),
            target.expose_code_mode,
        ))
    }

    pub(crate) fn label(&self) -> String {
        match self {
            Self::Root => "root".to_string(),
            Self::ProtectedSubset { route_name, .. } => format!("protected:{route_name}"),
        }
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
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn root_allows_everything() {
        let scope = McpRouteScope::Root;
        assert!(scope.allows_service("gateway"));
        assert!(scope.allows_upstream("sonarr"));
        assert!(scope.exposes_code_mode());
        assert_eq!(scope.label(), "root");
    }

    #[test]
    fn protected_subset_allows_only_configured_names() {
        let scope =
            McpRouteScope::protected_subset("media", ["sonarr", "radarr"], ["gateway"], true);
        assert!(scope.allows_service("gateway"));
        assert!(!scope.allows_service("logs"));
        assert!(scope.allows_upstream("sonarr"));
        assert!(!scope.allows_upstream("github"));
        assert!(scope.exposes_code_mode());
        assert_eq!(scope.label(), "protected:media");
    }

    #[test]
    fn protected_subset_can_hide_code_mode() {
        let scope = McpRouteScope::protected_subset("ops", ["unifi"], ["device"], false);
        assert!(!scope.exposes_code_mode());
    }
}
