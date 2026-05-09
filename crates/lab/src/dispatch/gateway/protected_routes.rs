use std::collections::HashMap;

use crate::config::ProtectedMcpRouteConfig;

#[derive(Debug, Clone, Default)]
pub struct ProtectedRouteIndex {
    routes: HashMap<(String, String), ProtectedMcpRouteConfig>,
}

impl ProtectedRouteIndex {
    #[must_use]
    pub fn from_routes(routes: &[ProtectedMcpRouteConfig]) -> Self {
        let mut index = Self::default();
        for route in routes.iter().filter(|route| route.enabled) {
            let key = route_key(&route.public_host, &route.public_path);
            index.routes.insert(key, route.clone());
        }
        index
    }

    #[must_use]
    pub fn resolve(&self, host: &str, path: &str) -> Option<ProtectedMcpRouteConfig> {
        let host = normalize_host(host)?;
        let first_segment = first_path_segment(path)?;
        let candidate = format!("/{first_segment}");
        self.routes.get(&(host, candidate)).cloned()
    }

    #[must_use]
    pub fn resolve_exact_metadata_path(
        &self,
        host: &str,
        metadata_path: &str,
    ) -> Option<ProtectedMcpRouteConfig> {
        const PREFIX: &str = "/.well-known/oauth-protected-resource";
        let suffix = metadata_path.strip_prefix(PREFIX)?;
        let public_path = if suffix.is_empty() { "/mcp" } else { suffix };
        let host = normalize_host(host)?;
        self.routes.get(&(host, public_path.to_string())).cloned()
    }
}

fn route_key(host: &str, public_path: &str) -> (String, String) {
    (
        normalize_host(host).unwrap_or_else(|| host.to_ascii_lowercase()),
        public_path.to_string(),
    )
}

fn normalize_host(raw: &str) -> Option<String> {
    let host = raw
        .split(',')
        .next()
        .unwrap_or(raw)
        .trim()
        .trim_end_matches('.');
    if host.is_empty() {
        return None;
    }
    Some(host.split(':').next().unwrap_or(host).to_ascii_lowercase())
}

fn first_path_segment(path: &str) -> Option<&str> {
    path.trim_start_matches('/')
        .split('/')
        .next()
        .filter(|segment| !segment.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn route(name: &str, host: &str, path: &str) -> ProtectedMcpRouteConfig {
        ProtectedMcpRouteConfig {
            name: name.to_string(),
            enabled: true,
            public_host: host.to_string(),
            public_path: path.to_string(),
            upstream: None,
            backend_url: "http://100.88.16.79:3100".to_string(),
            backend_mcp_path: "/mcp".to_string(),
            scopes: vec!["mcp:read".to_string(), "mcp:write".to_string()],
            health_path: None,
        }
    }

    #[test]
    fn resolves_by_host_and_first_path_segment() {
        let index = ProtectedRouteIndex::from_routes(&[
            route("syslog", "mcp.tootie.tv", "/syslog"),
            route("syslog-domain", "syslog.tootie.tv", "/mcp"),
        ]);

        assert_eq!(
            index
                .resolve("mcp.tootie.tv", "/syslog")
                .expect("syslog")
                .name,
            "syslog"
        );
        assert_eq!(
            index
                .resolve("syslog.tootie.tv", "/mcp")
                .expect("domain mcp")
                .name,
            "syslog-domain"
        );
        assert!(index.resolve("mcp.tootie.tv", "/other").is_none());
    }
}
