//! Request/response types for the ACP Agent Registry CDN endpoint.
//!
//! Serde rules:
//! - No `deny_unknown_fields` on `Agent` — the registry adds fields freely.
//! - Use `#[serde(default)]` liberally for optional arrays/fields.
//! - `Distribution` is a struct, not an enum — agents may ship multiple methods.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};
use serde_json::Value;

// ---------------------------------------------------------------------------
// Top-level response
// ---------------------------------------------------------------------------

/// Top-level response envelope from the ACP Registry CDN.
///
/// Endpoint: `GET /registry/v1/latest/registry.json`
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AcpRegistryResponse {
    /// Schema version, e.g. `"1.0.0"`.
    pub version: String,
    /// All registered ACP agents.
    #[serde(default)]
    pub agents: Vec<Agent>,
    /// Extension entries (reserved for future use; currently empty `[]`).
    #[serde(default)]
    pub extensions: Vec<Value>,
}

// ---------------------------------------------------------------------------
// Agent
// ---------------------------------------------------------------------------

/// A single ACP-compatible agent entry from the registry.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Agent {
    /// Unique agent identifier (e.g. `"anthropic/claude-code"`).
    pub id: String,
    /// Human-readable display name.
    pub name: String,
    /// Semver version string.
    pub version: String,
    /// Short description of the agent.
    #[serde(default)]
    pub description: Option<String>,
    /// How this agent is distributed and run.
    pub distribution: Distribution,
    /// Environment variables the agent accepts.
    #[serde(default)]
    pub env: Vec<AgentEnvVar>,
    /// Any additional fields not captured above.
    #[serde(flatten)]
    pub extra: HashMap<String, Value>,
}

// ---------------------------------------------------------------------------
// Distribution
// ---------------------------------------------------------------------------

/// Distribution methods for an agent. An agent may provide multiple methods
/// simultaneously (e.g. both `binary` and `npx`). All fields are optional;
/// unknown method keys are captured in `extra` for forward-compatibility.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Distribution {
    /// Pre-compiled binary assets keyed by platform triple
    /// (e.g. `"darwin-aarch64"`, `"linux-x86_64"`, `"windows-x86_64"`).
    #[serde(default)]
    pub binary: Option<HashMap<String, BinaryAsset>>,
    /// Run via `npx <package>`.
    #[serde(default)]
    pub npx: Option<NpxAsset>,
    /// Run via `uvx <package>`.
    #[serde(default)]
    pub uvx: Option<UvxAsset>,
    /// Unknown distribution methods (forward-compat).
    #[serde(flatten)]
    pub extra: HashMap<String, Value>,
}

/// A single platform binary asset.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BinaryAsset {
    /// URL to download the archive (`.tar.gz` or `.zip`).
    pub archive: String,
    /// Expected SHA-256 digest for the archive, as 64 hex chars or
    /// `sha256:<hex>`.
    #[serde(default, alias = "sha256sum", alias = "checksum_sha256")]
    pub sha256: Option<String>,
    /// OCI/GitHub-style digest field, usually `sha256:<hex>`.
    #[serde(default)]
    pub digest: Option<String>,
    /// Command to run after extraction (e.g. `"./my-agent"`).
    pub cmd: String,
    /// Extra CLI arguments appended after `cmd`.
    #[serde(default)]
    pub args: Vec<String>,
}

/// npm/npx distribution asset.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NpxAsset {
    /// npm package name (e.g. `"@scope/agent"`).
    pub package: String,
    /// Package version — optional in the live registry.
    #[serde(default)]
    pub version: Option<String>,
    /// Extra CLI arguments passed to npx.
    #[serde(default)]
    pub args: Vec<String>,
    /// Environment variable overrides (key → value).
    #[serde(default)]
    pub env: HashMap<String, String>,
    /// Unknown fields for forward-compatibility.
    #[serde(flatten)]
    pub extra: HashMap<String, Value>,
}

/// uv/uvx distribution asset.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UvxAsset {
    /// PyPI package name.
    pub package: String,
    /// Package version — optional in the live registry.
    #[serde(default)]
    pub version: Option<String>,
    /// Extra CLI arguments passed to uvx.
    #[serde(default)]
    pub args: Vec<String>,
    /// Environment variable overrides (key → value).
    #[serde(default)]
    pub env: HashMap<String, String>,
    /// Unknown fields for forward-compatibility.
    #[serde(flatten)]
    pub extra: HashMap<String, Value>,
}

// ---------------------------------------------------------------------------
// AgentEnvVar
// ---------------------------------------------------------------------------

/// An environment variable declaration from an agent manifest.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentEnvVar {
    /// The environment variable name (e.g. `"ANTHROPIC_API_KEY"`).
    pub name: String,
    /// Optional human-readable description.
    #[serde(default)]
    pub description: Option<String>,
    /// Whether this variable is required for the agent to function.
    #[serde(default)]
    pub required: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn binary_asset_models_sha256_metadata() {
        let asset: BinaryAsset = serde_json::from_value(serde_json::json!({
            "archive": "https://example.com/agent.tar.gz",
            "sha256": "abc",
            "digest": "sha256:def",
            "cmd": "./agent"
        }))
        .expect("deserialize");

        assert_eq!(asset.sha256.as_deref(), Some("abc"));
        assert_eq!(asset.digest.as_deref(), Some("sha256:def"));
    }

    #[test]
    fn binary_asset_accepts_sha256_aliases() {
        let asset: BinaryAsset = serde_json::from_value(serde_json::json!({
            "archive": "https://example.com/agent.tar.gz",
            "sha256sum": "abc",
            "cmd": "./agent"
        }))
        .expect("deserialize");

        assert_eq!(asset.sha256.as_deref(), Some("abc"));
    }
}
