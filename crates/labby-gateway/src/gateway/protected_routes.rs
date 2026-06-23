use std::collections::HashMap;

use labby_runtime::gateway_config::ProtectedMcpRouteConfig;

#[derive(Debug, Clone, Default)]
pub struct ProtectedRouteIndex {
    routes: HashMap<String, Vec<ProtectedMcpRouteConfig>>,
}

impl ProtectedRouteIndex {
    #[must_use]
    pub fn from_routes(routes: &[ProtectedMcpRouteConfig]) -> Self {
        let mut index = Self::default();
        for route in routes.iter().filter(|route| route.enabled) {
            let host = normalize_host(&route.public_host)
                .unwrap_or_else(|| route.public_host.to_ascii_lowercase());
            index.routes.entry(host).or_default().push(route.clone());
        }
        for routes in index.routes.values_mut() {
            routes.sort_by(|left, right| right.public_path.len().cmp(&left.public_path.len()));
        }
        index
    }

    #[must_use]
    pub fn resolve(&self, host: &str, path: &str) -> Option<ProtectedMcpRouteConfig> {
        let host = normalize_host(host)?;
        let path = normalize_request_path(path);
        self.routes.get(&host).and_then(|routes| {
            routes
                .iter()
                .find(|route| path_matches_prefix(&path, &route.public_path))
                .cloned()
        })
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
        self.routes.get(&host).and_then(|routes| {
            routes
                .iter()
                .find(|route| route.public_path == public_path)
                .cloned()
        })
    }
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

fn normalize_request_path(path: &str) -> String {
    let path = path.split('?').next().unwrap_or(path).trim();
    if path.starts_with('/') {
        path.to_string()
    } else {
        format!("/{path}")
    }
}

fn path_matches_prefix(path: &str, prefix: &str) -> bool {
    if path == prefix {
        return true;
    }
    let prefix = prefix.trim_end_matches('/');
    path.strip_prefix(prefix)
        .is_some_and(|rest| rest.starts_with('/'))
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
            target: None,
        }
    }

    #[test]
    fn resolves_by_host_and_longest_path_prefix() {
        let index = ProtectedRouteIndex::from_routes(&[
            route("mcp", "mcp.tootie.tv", "/mcp"),
            route("openapi", "mcp.tootie.tv", "/mcp/openapi/foo"),
            route("other-host", "other.tootie.tv", "/mcp/openapi/foo"),
        ]);

        assert_eq!(
            index
                .resolve("mcp.tootie.tv", "/mcp/openapi/foo")
                .expect("exact nested")
                .name,
            "openapi"
        );
        assert_eq!(
            index
                .resolve("mcp.tootie.tv", "/mcp/openapi/foo/sse")
                .expect("nested prefix")
                .name,
            "openapi"
        );
        assert_eq!(
            index
                .resolve("mcp.tootie.tv", "/mcp/other")
                .expect("root prefix")
                .name,
            "mcp"
        );
        assert_eq!(
            index
                .resolve("other.tootie.tv", "/mcp/openapi/foo")
                .expect("host scoped")
                .name,
            "other-host"
        );
        assert!(index.resolve("mcp.tootie.tv", "/mcproxy").is_none());
    }
}
