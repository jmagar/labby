//! Request/response types for the MCP Registry v0.1 API.
//!
//! These types closely follow the official MCP Registry OpenAPI specification
//! plus the Lab-specific extension metadata stored alongside registry records.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::Value;

// ---------------------------------------------------------------------------
// Core registry types (mirrors the upstream API)
// ---------------------------------------------------------------------------

/// A server record as returned by the registry API.
///
/// `server` holds the serialisable MCP server definition (stored verbatim in
/// the local SQLite mirror). `meta` carries registry-managed extension data
/// such as `is_latest`, publication timestamps, and Lab-specific annotations
/// that are merged in at read time.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServerResponse {
    /// The MCP server definition.
    pub server: ServerJSON,
    /// Registry-managed metadata attached to this response.
    /// `None` when absent in both the upstream response and the local store.
    pub meta: Option<ResponseMeta>,
}

/// Serialisable MCP server definition — stored verbatim in the local registry
/// mirror and re-parsed on each read.
///
/// Fields align with the MCP Registry v0.1 `server` object schema.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServerJSON {
    /// JSON-LD / JSON Schema `$schema` URL.
    #[serde(rename = "$schema", skip_serializing_if = "Option::is_none")]
    pub schema: Option<String>,
    /// Qualified server name, e.g. `io.modelcontextprotocol/everything`.
    pub name: String,
    /// Human-readable display title.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    /// Human-readable description of the server's purpose.
    pub description: String,
    /// Semver version string for this entry.
    pub version: String,
    /// Package distributions available for this server.
    #[serde(default)]
    pub packages: Vec<Package>,
    /// Remote transport endpoints.
    #[serde(default)]
    pub remotes: Vec<Remote>,
    /// Source repository metadata.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub repository: Option<Repository>,
    /// Icon references (URL or data URI).
    #[serde(default)]
    pub icons: Vec<Icon>,
    /// Canonical website URL, if any.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub website_url: Option<String>,
}

impl ServerJSON {
    /// Convenience: look up the first remote URL, if any.
    #[must_use]
    pub fn first_remote_url(&self) -> Option<&str> {
        self.remotes.iter().find_map(|r| r.url.as_deref())
    }
}

/// A package distribution for an MCP server.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Package {
    /// Registry type: `"npm"`, `"pypi"`, `"docker"`, `"mcpb"`, etc.
    pub registry_type: String,
    /// Package identifier within that registry (e.g. `@scope/name`).
    pub identifier: String,
    /// Optional pinned package version.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    /// Transport configuration for this package.
    pub transport: Transport,
    /// Runtime hint: `"npx"`, `"uvx"`, `"docker"`, etc.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub runtime_hint: Option<String>,
    /// Extra arguments prepended before the package identifier.
    #[serde(default)]
    pub runtime_arguments: Vec<Value>,
    /// Extra arguments appended after the package identifier.
    #[serde(default)]
    pub package_arguments: Vec<Value>,
    /// Environment variables accepted or required by this package.
    #[serde(default)]
    pub environment_variables: Vec<EnvironmentVariable>,
    /// SHA-256 hash of the binary artifact (MCPB packages only).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub file_sha256: Option<String>,
    /// Override base URL for the package registry (used by self-hosted npm mirrors).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub registry_base_url: Option<String>,
}

/// Transport configuration attached to a package.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Transport {
    /// Transport type: `"stdio"`, `"sse"`, `"http"`, etc.
    pub transport_type: String,
    /// URL for HTTP-based transports.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    /// Static HTTP headers to send with every request.
    #[serde(default)]
    pub headers: Vec<Header>,
    /// Dynamic variable definitions (template substitution).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub variables: Option<Value>,
}

/// A static HTTP header.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Header {
    /// Header name (e.g. `Authorization`).
    pub name: String,
    /// Header value or template (e.g. `Bearer ${API_KEY}`).
    pub value: String,
}

/// An environment variable declaration for a package.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EnvironmentVariable {
    /// Variable name (e.g. `GITHUB_TOKEN`).
    pub name: String,
    /// Human-readable description.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// Whether this variable must be set.
    pub is_required: bool,
    /// Whether this variable should be treated as a secret.
    pub is_secret: bool,
    /// Default value to use when the caller does not provide one.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub default: Option<String>,
    /// Enumerated choices for the variable value.
    #[serde(default)]
    pub choices: Vec<String>,
    /// Placeholder text shown in UIs.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub placeholder: Option<String>,
    /// Format hint (e.g. `"token"`, `"url"`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub format: Option<String>,
}

/// A remote transport endpoint for an MCP server.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Remote {
    /// Transport type: `"sse"`, `"http"`, etc.
    pub transport_type: String,
    /// URL of the remote endpoint.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    /// Static HTTP headers to send with every request.
    #[serde(default)]
    pub headers: Vec<Header>,
}

/// Source repository metadata.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Repository {
    /// Repository URL (e.g. GitHub URL).
    pub url: String,
    /// Source host type (e.g. `"github"`, `"gitlab"`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
}

/// An icon reference.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Icon {
    /// MIME type hint.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mime_type: Option<String>,
    /// URL or data URI of the icon.
    pub url: String,
}

// ---------------------------------------------------------------------------
// Registry response envelope
// ---------------------------------------------------------------------------

/// Paginated list of MCP servers from the registry.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServerListResponse {
    /// Servers in this page.
    pub servers: Vec<ServerResponse>,
    /// Pagination metadata.
    pub metadata: PaginationMetadata,
}

/// Pagination metadata returned with list responses.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct PaginationMetadata {
    /// Opaque cursor for fetching the next page, if any.
    pub next_cursor: Option<String>,
}

// ---------------------------------------------------------------------------
// Response meta (registry-managed extensions)
// ---------------------------------------------------------------------------

/// Registry-managed metadata attached to a `ServerResponse`.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ResponseMeta {
    /// Official registry extensions (is_latest, status, timestamps).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub official: Option<RegistryExtensions>,
    /// Arbitrary extension metadata keyed by namespace.
    ///
    /// Lab stores its own curation data here under the `"lab"` key.
    #[serde(flatten)]
    pub extensions: BTreeMap<String, Value>,
}

impl ResponseMeta {
    /// Return true when no fields carry any data (safe to serialize as `None`).
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.official.is_none() && self.extensions.is_empty()
    }

    /// Insert or replace an extension value under a given namespace key.
    pub fn insert_extension(&mut self, namespace: &str, value: Value) {
        self.extensions.insert(namespace.to_owned(), value);
    }
}

/// Official registry-managed extension fields.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RegistryExtensions {
    /// Whether this is the latest published version of the server.
    pub is_latest: bool,
    /// ISO-8601 timestamp when this version was first published.
    pub published_at: String,
    /// Lifecycle status: `"active"`, `"deprecated"`, `"deleted"`, etc.
    pub status: String,
    /// ISO-8601 timestamp when `status` last changed.
    pub status_changed_at: String,
    /// Human-readable message accompanying a non-active status.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status_message: Option<String>,
    /// ISO-8601 timestamp of the most recent upstream update.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub updated_at: Option<String>,
}

// ---------------------------------------------------------------------------
// Validate types
// ---------------------------------------------------------------------------

/// Result from the registry's `/v0.1/validate` endpoint.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ValidationResult {
    /// Whether the provided server JSON is valid.
    pub valid: bool,
    /// Validation error messages, if any.
    #[serde(default)]
    pub errors: Vec<String>,
}

// ---------------------------------------------------------------------------
// Query parameters
// ---------------------------------------------------------------------------

/// Query parameters for the `GET /v0.1/servers` list endpoint.
#[derive(Debug, Clone, Default)]
pub struct ListServersParams {
    /// Optional free-text search query.
    pub search: Option<String>,
    /// Maximum number of results per page.
    pub limit: Option<u32>,
    /// Pagination cursor returned by a prior response.
    pub cursor: Option<String>,
    /// Filter to a specific version string.
    pub version: Option<String>,
    /// Filter to entries updated since this ISO-8601 timestamp.
    pub updated_since: Option<String>,
    /// Filter to Lab-featured entries.
    pub featured: Option<bool>,
    /// Filter to Lab-reviewed entries.
    pub reviewed: Option<bool>,
    /// Filter to Lab-recommended entries.
    pub recommended: Option<bool>,
    /// Filter to hidden entries.
    pub hidden: Option<bool>,
    /// Filter to a single Lab curation tag.
    pub tag: Option<String>,
}

impl ListServersParams {
    /// Encode as URL query pairs for `GET /v0.1/servers`, omitting `None` fields.
    ///
    /// Note: Lab-specific filter fields (featured, reviewed, etc.) are client-side
    /// concepts applied against the local store — they are NOT forwarded upstream.
    #[must_use]
    pub fn to_upstream_query_pairs(&self) -> Vec<(String, String)> {
        let mut pairs = Vec::new();
        if let Some(q) = &self.search {
            pairs.push(("search".to_owned(), q.clone()));
        }
        if let Some(n) = self.limit {
            pairs.push(("limit".to_owned(), n.to_string()));
        }
        if let Some(c) = &self.cursor {
            pairs.push(("cursor".to_owned(), c.clone()));
        }
        pairs
    }
}

// ---------------------------------------------------------------------------
// Lab-specific metadata (stored alongside registry records)
// ---------------------------------------------------------------------------

/// Lab-managed curation metadata attached to a registry record.
///
/// Stored in the local registry SQLite store under the `"lab"` extension
/// namespace. Never accepted from the upstream registry API.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct LabRegistryMetadata {
    /// Lab audit trail (populated by the store, read-only for callers).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub audit: Option<LabRegistryAudit>,
    /// Curation tags and notes.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub curation: Option<LabCuration>,
    /// Trust signals (manual review state).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub trust: Option<LabTrustMeta>,
    /// Installation quality signals.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub quality: Option<LabQualityMeta>,
    /// UX-level annotations.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ux: Option<LabUxMeta>,
}

/// Audit trail automatically populated by Lab.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LabRegistryAudit {
    /// ISO-8601 timestamp of the last metadata write.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub updated_at: Option<String>,
    /// Agent or user identifier that last wrote the metadata.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub updated_by: Option<String>,
}

/// Lab curator tags and notes for a server.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LabCuration {
    /// Curation tags (sorted, deduplicated by the store).
    #[serde(default)]
    pub tags: Vec<String>,
    /// Optional curator notes.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub notes: Option<String>,
    /// Whether Lab features this server in curated listings.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub featured: Option<bool>,
    /// Whether Lab has reviewed this server.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reviewed: Option<bool>,
    /// Whether Lab recommends this server.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub recommended: Option<bool>,
    /// Whether this server is hidden from default listings.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hidden: Option<bool>,
}

/// Trust signals.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LabTrustMeta {
    /// ISO-8601 timestamp when a human last reviewed this server.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reviewed_at: Option<String>,
}

/// Installation quality signals.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LabQualityMeta {
    /// ISO-8601 timestamp of the last successful install test.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_install_tested_at: Option<String>,
    /// Observed transport reliability score.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub transport_score: Option<LabRegistryTransportScore>,
}

/// UX-level annotations.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LabUxMeta {
    /// Subjective setup difficulty rating.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub setup_difficulty: Option<LabRegistrySetupDifficulty>,
}

/// Transport reliability score.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LabRegistryTransportScore {
    /// Transport works reliably.
    Good,
    /// Transport has known issues in some configurations.
    Mixed,
    /// Transport is unreliable or broken.
    Poor,
}

/// Subjective setup difficulty.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LabRegistrySetupDifficulty {
    /// Minimal configuration required.
    Easy,
    /// Some configuration steps required.
    Medium,
    /// Complex configuration or prerequisites required.
    Hard,
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn server_list_response_deserializes_minimal() {
        let json = serde_json::json!({
            "servers": [
                {
                    "server": {
                        "name": "io.example/hello",
                        "description": "A hello world server",
                        "version": "1.0.0"
                    },
                    "meta": null
                }
            ],
            "metadata": {
                "next_cursor": null
            }
        });

        let resp: ServerListResponse = serde_json::from_value(json).expect("should deserialize");
        assert_eq!(resp.servers.len(), 1);
        assert_eq!(resp.servers[0].server.name, "io.example/hello");
        assert!(resp.metadata.next_cursor.is_none());
        assert!(resp.servers[0].server.packages.is_empty());
        assert!(resp.servers[0].server.remotes.is_empty());
    }

    #[test]
    fn server_response_meta_default_is_empty() {
        let meta = ResponseMeta::default();
        assert!(meta.is_empty());
    }

    #[test]
    fn server_response_meta_insert_extension() {
        let mut meta = ResponseMeta::default();
        meta.insert_extension("lab", serde_json::json!({"featured": true}));
        assert!(!meta.is_empty());
        assert!(meta.extensions.contains_key("lab"));
    }

    #[test]
    fn list_servers_params_to_upstream_query_pairs_omits_lab_fields() {
        let p = ListServersParams {
            search: Some("test".into()),
            limit: Some(25),
            cursor: Some("cur1".into()),
            featured: Some(true), // Lab-only — must NOT appear in upstream pairs
            ..Default::default()
        };
        let pairs = p.to_upstream_query_pairs();
        assert_eq!(pairs.len(), 3);
        assert!(pairs.iter().any(|(k, v)| k == "search" && v == "test"));
        assert!(pairs.iter().any(|(k, v)| k == "limit" && v == "25"));
        assert!(pairs.iter().any(|(k, v)| k == "cursor" && v == "cur1"));
        // Lab-only fields must be absent
        assert!(!pairs.iter().any(|(k, _)| k == "featured"));
    }

    #[test]
    fn lab_registry_metadata_audit_field_roundtrips() {
        let meta = LabRegistryMetadata {
            audit: Some(LabRegistryAudit {
                updated_at: Some("2025-01-01T00:00:00Z".into()),
                updated_by: Some("lab-agent".into()),
            }),
            ..Default::default()
        };
        let v = serde_json::to_value(&meta).unwrap();
        let back: LabRegistryMetadata = serde_json::from_value(v).unwrap();
        assert_eq!(
            back.audit.as_ref().unwrap().updated_at.as_deref(),
            Some("2025-01-01T00:00:00Z")
        );
    }

    #[test]
    fn package_deserializes_with_defaults() {
        let json = serde_json::json!({
            "registry_type": "npm",
            "identifier": "@example/server",
            "transport": {
                "transport_type": "stdio",
                "headers": []
            },
            "is_required": false,
            "is_secret": false
        });
        let pkg: Package = serde_json::from_value(json).expect("should deserialize");
        assert_eq!(pkg.registry_type, "npm");
        assert!(pkg.runtime_hint.is_none());
        assert!(pkg.environment_variables.is_empty());
        assert!(pkg.runtime_arguments.is_empty());
    }
}
