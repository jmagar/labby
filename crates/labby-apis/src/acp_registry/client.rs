//! `AcpRegistryClient` — read-only ACP Agent Registry client.

use std::time::Duration;

use reqwest::redirect;

use crate::core::{ApiError, Auth, HttpClient};

use super::error::AcpRegistryError;
use super::types::{AcpRegistryResponse, Agent};

/// Default ACP Registry CDN base URL; overridden by `ACP_REGISTRY_URL` env var.
pub const REGISTRY_DEFAULT_URL: &str = "https://cdn.agentclientprotocol.com";

/// Path to the full registry manifest.
const REGISTRY_PATH: &str = "/registry/v1/latest/registry.json";

/// Client for the ACP Agent Registry CDN.
///
/// All operations are unauthenticated read-only. Uses a custom `reqwest::Client` with:
/// - 20 s request timeout
/// - 5 s connect timeout
/// - No redirect following (prevents SSRF via registry-hosted redirect chains)
pub struct AcpRegistryClient {
    pub(crate) http: HttpClient,
}

impl AcpRegistryClient {
    /// Construct a new client targeting `base_url`.
    ///
    /// # Errors
    /// Returns [`AcpRegistryError::Request`] wrapping [`ApiError::Internal`] if the TLS
    /// backend fails to initialise.
    pub fn new(base_url: &str) -> Result<Self, AcpRegistryError> {
        let inner = reqwest::Client::builder()
            .user_agent(concat!("lab-apis/", env!("CARGO_PKG_VERSION")))
            .connect_timeout(Duration::from_secs(5))
            .timeout(Duration::from_secs(20))
            .redirect(redirect::Policy::none())
            .build()
            .map_err(|e| {
                AcpRegistryError::Request(ApiError::Internal(format!(
                    "reqwest::Client::build: {e}"
                )))
            })?;
        Ok(Self {
            http: HttpClient::from_parts(base_url, Auth::None, inner),
        })
    }

    /// Fetch the full registry manifest and return all agents.
    ///
    /// # Errors
    /// Returns [`AcpRegistryError`] on HTTP or decode failure. Non-2xx upstream
    /// statuses surface as [`AcpRegistryError::Api`] carrying the status + body
    /// so callers can see the upstream failure rather than an opaque transport
    /// error.
    pub async fn list_agents(&self) -> Result<Vec<Agent>, AcpRegistryError> {
        let response: AcpRegistryResponse = self
            .http
            .get_json(REGISTRY_PATH)
            .await
            .map_err(map_api_error)?;
        Ok(response.agents)
    }

    /// Fetch the registry and find an agent by `id` (client-side filter).
    ///
    /// Returns `None` if no agent with that id exists.
    ///
    /// # Errors
    /// Returns [`AcpRegistryError`] on HTTP or decode failure.
    pub async fn get_agent(&self, id: &str) -> Result<Option<Agent>, AcpRegistryError> {
        let agents = self.list_agents().await?;
        Ok(agents.into_iter().find(|a| a.id == id))
    }

    pub(super) async fn health_probe(&self) -> Result<AcpRegistryResponse, AcpRegistryError> {
        self.http
            .get_json(REGISTRY_PATH)
            .await
            .map_err(map_api_error)
    }
}

/// Map a transport-layer [`ApiError`] into an [`AcpRegistryError`].
///
/// A non-success HTTP status that `HttpClient` already classified (5xx →
/// [`ApiError::Server`], 401/403 → [`ApiError::Auth`], 404 → [`ApiError::NotFound`])
/// is promoted to the richer [`AcpRegistryError::Api`] envelope so callers see the
/// upstream status/body. Every other failure (network, decode, internal) folds
/// into the opaque [`AcpRegistryError::Request`] wrapper.
fn map_api_error(e: ApiError) -> AcpRegistryError {
    match e {
        ApiError::Server { status, body } => AcpRegistryError::Api { status, body },
        ApiError::Auth => AcpRegistryError::Api {
            status: 401,
            body: "authentication failed".to_string(),
        },
        ApiError::NotFound => AcpRegistryError::Api {
            status: 404,
            body: "not found".to_string(),
        },
        other => AcpRegistryError::Request(other),
    }
}

#[cfg(test)]
mod tests {
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    use super::*;

    /// Minimal fixture matching the real CDN response shape.
    fn fixture_registry() -> serde_json::Value {
        serde_json::json!({
            "version": "1.0.0",
            "agents": [
                {
                    "id": "anthropic/claude-code",
                    "name": "Claude Code",
                    "version": "1.2.3",
                    "description": "AI coding agent by Anthropic",
                    "distribution": {
                        "binary": {
                            "darwin-aarch64": { "archive": "https://example.com/darwin-arm.tar.gz", "cmd": "./claude-code" },
                            "linux-x86_64":   { "archive": "https://example.com/linux-x64.tar.gz",  "cmd": "./claude-code" }
                        }
                    },
                    "env": [
                        { "name": "ANTHROPIC_API_KEY", "description": "Anthropic API key", "required": true }
                    ]
                },
                {
                    "id": "openai/codex-cli",
                    "name": "Codex CLI",
                    "version": "0.9.0",
                    "distribution": {
                        "npx": { "package": "@openai/codex", "args": ["--acp"] }
                    },
                    "env": []
                }
            ],
            "extensions": []
        })
    }

    fn make_client(base_url: &str) -> AcpRegistryClient {
        AcpRegistryClient::new(base_url).expect("client construction should succeed")
    }

    // -----------------------------------------------------------------------
    // 1. list_agents() returns deserialized agents from wrapper response
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_list_agents_parses_response() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/registry/v1/latest/registry.json"))
            .respond_with(ResponseTemplate::new(200).set_body_json(fixture_registry()))
            .mount(&server)
            .await;

        let client = make_client(&server.uri());
        let agents = client.list_agents().await.unwrap();
        assert_eq!(agents.len(), 2);
        assert_eq!(agents[0].id, "anthropic/claude-code");
        assert_eq!(agents[0].name, "Claude Code");
        assert_eq!(agents[0].version, "1.2.3");
        assert_eq!(agents[1].id, "openai/codex-cli");
    }

    // -----------------------------------------------------------------------
    // 2. get_agent(id) finds by id, returns None for unknown
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_get_agent_found() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/registry/v1/latest/registry.json"))
            .respond_with(ResponseTemplate::new(200).set_body_json(fixture_registry()))
            .mount(&server)
            .await;

        let client = make_client(&server.uri());
        let agent = client.get_agent("openai/codex-cli").await.unwrap();
        assert!(agent.is_some());
        let agent = agent.unwrap();
        assert_eq!(agent.id, "openai/codex-cli");
    }

    #[tokio::test]
    async fn test_get_agent_not_found_returns_none() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/registry/v1/latest/registry.json"))
            .respond_with(ResponseTemplate::new(200).set_body_json(fixture_registry()))
            .mount(&server)
            .await;

        let client = make_client(&server.uri());
        let agent = client.get_agent("nonexistent/agent").await.unwrap();
        assert!(agent.is_none());
    }

    // -----------------------------------------------------------------------
    // 3. HTTP 4xx → AcpRegistryError
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_4xx_returns_error() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/registry/v1/latest/registry.json"))
            .respond_with(ResponseTemplate::new(404).set_body_string("not found"))
            .mount(&server)
            .await;

        let client = make_client(&server.uri());
        let result = client.list_agents().await;
        // 404 is promoted to the richer `Api` envelope carrying the status.
        let err = result.expect_err("expected error on 404");
        assert!(
            matches!(err, AcpRegistryError::Api { status: 404, .. }),
            "expected Api {{ status: 404 }}, got {err:?}"
        );
    }

    // -----------------------------------------------------------------------
    // 4. HTTP 5xx → AcpRegistryError
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_5xx_returns_error() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/registry/v1/latest/registry.json"))
            .respond_with(ResponseTemplate::new(503).set_body_string("service unavailable"))
            .mount(&server)
            .await;

        let client = make_client(&server.uri());
        let result = client.list_agents().await;
        // 5xx is promoted to the richer `Api` envelope carrying status + body.
        let err = result.expect_err("expected error on 503");
        assert!(
            matches!(
                &err,
                AcpRegistryError::Api { status: 503, body } if body.contains("service unavailable")
            ),
            "expected Api {{ status: 503, body: \"…service unavailable…\" }}, got {err:?}"
        );
    }

    // -----------------------------------------------------------------------
    // 5. Network failure → error propagated
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_network_failure_propagated() {
        // Use an invalid address that will refuse connections immediately
        let client = AcpRegistryClient::new("http://127.0.0.1:1").unwrap();
        let result = client.list_agents().await;
        assert!(result.is_err(), "expected network error, got Ok");
    }

    // -----------------------------------------------------------------------
    // 6. Snapshot test with minimal fixture of the real JSON shape
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_snapshot_minimal_fixture() {
        let server = MockServer::start().await;
        let minimal = serde_json::json!({
            "version": "1.0.0",
            "agents": [
                {
                    "id": "test/agent",
                    "name": "Test Agent",
                    "version": "0.1.0",
                    "distribution": {
                        "uvx": { "package": "test-agent" }
                    }
                }
            ],
            "extensions": []
        });
        Mock::given(method("GET"))
            .and(path("/registry/v1/latest/registry.json"))
            .respond_with(ResponseTemplate::new(200).set_body_json(minimal))
            .mount(&server)
            .await;

        let client = make_client(&server.uri());
        let agents = client.list_agents().await.unwrap();
        assert_eq!(agents.len(), 1);
        assert_eq!(agents[0].id, "test/agent");
        assert_eq!(agents[0].version, "0.1.0");
        // description defaults to None when absent
        assert!(agents[0].description.is_none());
        // env defaults to empty vec
        assert!(agents[0].env.is_empty());
    }

    // -----------------------------------------------------------------------
    // 7. Hybrid distribution (binary + npx) decodes without error
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_hybrid_distribution_decodes() {
        let server = MockServer::start().await;
        let hybrid = serde_json::json!({
            "version": "1.0.0",
            "agents": [
                {
                    "id": "acme/hybrid",
                    "name": "Hybrid Agent",
                    "version": "1.0.0",
                    "distribution": {
                        "binary": {
                            "linux-x86_64": {
                                "archive": "https://example.com/linux.tar.gz",
                                "cmd": "./agent",
                                "args": ["--acp"]
                            }
                        },
                        "npx": { "package": "@acme/hybrid-acp" }
                    }
                }
            ],
            "extensions": []
        });
        Mock::given(method("GET"))
            .and(path("/registry/v1/latest/registry.json"))
            .respond_with(ResponseTemplate::new(200).set_body_json(hybrid))
            .mount(&server)
            .await;

        let client = make_client(&server.uri());
        let result = client.list_agents().await;
        assert!(
            result.is_ok(),
            "hybrid distribution should decode: {result:?}"
        );
        let agents = result.unwrap();
        assert_eq!(agents[0].id, "acme/hybrid");
        assert!(agents[0].distribution.binary.is_some());
        assert!(agents[0].distribution.npx.is_some());
        let bin = agents[0].distribution.binary.as_ref().unwrap();
        assert_eq!(bin["linux-x86_64"].args, vec!["--acp"]);
    }

    #[tokio::test]
    async fn test_npx_and_uvx_assets_preserve_optional_args_and_env() {
        let server = MockServer::start().await;
        let manifest = serde_json::json!({
            "version": "1.0.0",
            "agents": [
                {
                    "id": "acme/npm",
                    "name": "NPM Agent",
                    "version": "1.0.0",
                    "distribution": {
                        "npx": {
                            "package": "@acme/npm-agent",
                            "args": ["--stdio"],
                            "env": { "ACME_MODE": "npx" }
                        }
                    }
                },
                {
                    "id": "acme/python",
                    "name": "Python Agent",
                    "version": "1.0.0",
                    "distribution": {
                        "uvx": {
                            "package": "acme-agent",
                            "args": ["--stdio"],
                            "env": { "ACME_MODE": "uvx" }
                        }
                    }
                }
            ],
            "extensions": []
        });
        Mock::given(method("GET"))
            .and(path("/registry/v1/latest/registry.json"))
            .respond_with(ResponseTemplate::new(200).set_body_json(manifest))
            .mount(&server)
            .await;

        let client = make_client(&server.uri());
        let agents = client.list_agents().await.unwrap();

        let npx = agents[0].distribution.npx.as_ref().unwrap();
        assert_eq!(npx.version, None);
        assert_eq!(npx.args, vec!["--stdio"]);
        assert_eq!(npx.env["ACME_MODE"], "npx");

        let uvx = agents[1].distribution.uvx.as_ref().unwrap();
        assert_eq!(uvx.version, None);
        assert_eq!(uvx.args, vec!["--stdio"]);
        assert_eq!(uvx.env["ACME_MODE"], "uvx");
    }

    // -----------------------------------------------------------------------
    // Extra: unknown fields in Agent are silently ignored via flatten
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_unknown_agent_fields_ignored() {
        let server = MockServer::start().await;
        let with_extra = serde_json::json!({
            "version": "1.0.0",
            "agents": [
                {
                    "id": "acme/tool",
                    "name": "Acme Tool",
                    "version": "2.0.0",
                    "distribution": {
                        "npx": { "package": "@acme/tool", "version": "2.0.0" }
                    },
                    "future_field": "should not cause a panic",
                    "another_unknown": 99
                }
            ],
            "extensions": []
        });
        Mock::given(method("GET"))
            .and(path("/registry/v1/latest/registry.json"))
            .respond_with(ResponseTemplate::new(200).set_body_json(with_extra))
            .mount(&server)
            .await;

        let client = make_client(&server.uri());
        let result = client.list_agents().await;
        assert!(
            result.is_ok(),
            "unknown fields should be silently captured: {result:?}"
        );
        let agents = result.unwrap();
        assert_eq!(agents[0].id, "acme/tool");
        // Extra fields end up in the `extra` HashMap
        assert!(agents[0].extra.contains_key("future_field"));
    }
}
