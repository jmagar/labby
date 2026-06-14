//! Pure domain types for the stash service.
//!
//! No I/O, no network, no env reads. All types are plain data structures
//! intended to be serialised to/from JSON on the wire and in storage.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// Workspace shape
// ---------------------------------------------------------------------------

/// Describes whether a component occupies a single file or a directory on the
/// host workspace.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum StashWorkspaceShape {
    /// The component is a single file (e.g. a settings JSON or a shell script).
    File,
    /// The component is a directory tree (e.g. a skill or agent bundle).
    Directory,
}

// ---------------------------------------------------------------------------
// Component kind
// ---------------------------------------------------------------------------

/// Every artifact type that stash tracks.  All 13 kinds are enumerated — do
/// not collapse or merge variants.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "snake_case")]
pub enum StashComponentKind {
    /// A Claude Code skill definition (directory).
    Skill,
    /// An AI agent bundle (directory).
    Agent,
    /// A slash command definition (directory).
    Command,
    /// A notification channel definition (directory).
    Channel,
    /// A process / log monitor definition (directory).
    Monitor,
    /// A lifecycle hook (directory).
    Hook,
    /// An output style / renderer (directory).
    OutputStyle,
    /// A visual theme (directory).
    Theme,
    /// Settings file (file).
    Settings,
    /// MCP configuration file (file).
    McpConfig,
    /// LSP configuration file (file).
    LspConfig,
    /// A shell or script file (file).
    Script,
    /// A compiled binary artefact (file).
    BinFile,
    /// Directory-shaped component representing a whole Marketplace plugin fork.
    Plugin,
}

impl StashComponentKind {
    /// Return the workspace shape for this kind.
    ///
    /// File-shaped kinds live at a single path; directory-shaped kinds occupy a
    /// directory tree.
    #[must_use]
    pub const fn workspace_shape(self) -> StashWorkspaceShape {
        match self {
            // directory-shaped kinds
            Self::Skill
            | Self::Agent
            | Self::Command
            | Self::Channel
            | Self::Monitor
            | Self::Hook
            | Self::OutputStyle
            | Self::Theme
            | Self::Plugin => StashWorkspaceShape::Directory,

            // file-shaped kinds
            Self::Settings | Self::McpConfig | Self::LspConfig | Self::Script | Self::BinFile => {
                StashWorkspaceShape::File
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Provider capabilities
// ---------------------------------------------------------------------------

/// Declares which operations a storage provider supports.
///
/// The spec listed `StashProviderCapabilities` by name without defining its
/// fields.  This struct captures the obvious provider-shape capabilities; the
/// full field set should be revisited when the provider API is specified.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct StashProviderCapabilities {
    /// Provider can retrieve component content.
    pub can_read: bool,
    /// Provider can persist new or updated content.
    pub can_write: bool,
    /// Provider maintains a history of revisions.
    pub supports_revisions: bool,
    /// Provider can lock components to prevent concurrent writes.
    pub supports_locking: bool,
}

// ---------------------------------------------------------------------------
// Limits
// ---------------------------------------------------------------------------

/// Compile-time constants that constrain stash operations.
pub mod limits {
    /// Maximum length of a component name, in bytes.
    pub const MAX_COMPONENT_NAME_LEN: usize = 128;
    /// Maximum length of a component label, in bytes.
    pub const MAX_COMPONENT_LABEL_LEN: usize = 64;
    /// Maximum length of a revision label, in bytes.
    pub const MAX_REVISION_LABEL_LEN: usize = 128;
    /// Maximum size of a single tracked file (50 MiB).
    pub const MAX_FILE_SIZE: u64 = 50 * 1024 * 1024;
    /// Maximum total size of a component workspace (200 MiB).
    pub const MAX_WORKSPACE_SIZE: u64 = 200 * 1024 * 1024;
    /// Maximum number of components tracked per stash instance.
    pub const MAX_COMPONENTS: usize = 10_000;
    /// Maximum number of files in a single component workspace (lab-se5t).
    ///
    /// Prevents DoS via mass-tiny-file imports: 10,000 files × ~1 B each
    /// would consume negligible disk space but flood the inode table and
    /// make every walk O(N) without this guard.
    pub const MAX_FILE_COUNT: usize = 10_000;
    /// Deploy operation timeout in milliseconds.
    pub const DEPLOY_TIMEOUT_MS: u64 = 30_000;
}

// ---------------------------------------------------------------------------
// Deploy target
// ---------------------------------------------------------------------------

/// Destination for a stash deploy operation.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum StashDeployTarget {
    /// A local filesystem path on this host.
    Local {
        /// Stable identifier for this target.
        id: String,
        /// Human-readable name shown in UI/logs.
        name: String,
        /// Absolute path to the deployment root.
        path: PathBuf,
    },
    /// A remote host accessed through a gateway.
    Remote {
        /// Stable identifier for this target.
        id: String,
        /// Human-readable name shown in UI/logs.
        name: String,
        /// Identifier of the gateway that proxies access to this host.
        gateway_id: String,
    },
}

// ---------------------------------------------------------------------------
// Export options
// ---------------------------------------------------------------------------

/// Options that control how a component is exported.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct StashExportOptions {
    /// When `true`, secrets embedded in the component are included in the
    /// export.  Defaults to `false` so that safe-by-default exports never leak
    /// credentials.
    #[serde(default)]
    pub include_secrets: bool,
    /// When `true`, overwrite an existing export at the destination without
    /// prompting.
    #[serde(default)]
    pub force: bool,
}

// ---------------------------------------------------------------------------
// Core structs
// ---------------------------------------------------------------------------

/// Structured origin metadata for components imported from another Lab surface.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum StashOrigin {
    /// Component was forked or adopted from a Marketplace plugin artifact.
    Marketplace(MarketplaceOrigin),
    /// Component was imported directly from a local filesystem path.
    LocalPath {
        /// Original absolute source path at import time.
        source_path: PathBuf,
    },
}

/// Marketplace-specific component origin.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct MarketplaceOrigin {
    /// Plugin id in `name@marketplace` form.
    pub plugin_id: String,
    /// Relative artifact path inside the plugin. `None` means whole-plugin fork.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub artifact_path: Option<String>,
    /// Version string from the plugin or marketplace manifest at fork time.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source_version: Option<String>,
    /// Source tree fingerprint or upstream commit at fork time.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source_fingerprint: Option<String>,
}

/// A tracked component — the top-level unit of versioning in stash.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StashComponent {
    /// Stable opaque identifier (UUID or similar).
    pub id: String,
    /// Artifact type of this component.
    pub kind: StashComponentKind,
    /// Short name used in CLI and MCP surface (`lab stash get <name>`).
    pub name: String,
    /// Optional human-readable label / description.
    pub label: Option<String>,
    /// Revision ID of the currently checked-out revision, if any.
    pub head_revision_id: Option<String>,
    /// Upstream origin URI, if this component was installed from a registry or
    /// remote stash.
    pub origin: Option<String>,
    /// Structured origin metadata for behavior; optional for older records.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub origin_meta: Option<StashOrigin>,
    /// Absolute path to the workspace root on the local host.
    pub workspace_root: PathBuf,
    /// Whether the workspace root is a file or a directory.
    pub workspace_shape: StashWorkspaceShape,
    /// Unix permission bits for `BinFile` components only.
    ///
    /// Stored as `mode & 0o0755` (execute bits only; setuid/setgid/sticky
    /// always stripped). `None` for non-`BinFile` components.
    pub unix_mode: Option<u32>,
    /// ISO-8601 creation timestamp.
    pub created_at: String,
    /// ISO-8601 last-updated timestamp.
    pub updated_at: String,
}

/// An immutable snapshot of a component's content at a point in time.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StashRevision {
    /// Stable opaque identifier for this revision.
    pub id: String,
    /// ID of the component this revision belongs to.
    pub component_id: String,
    /// Optional human-readable label (e.g. `"v1.2.3"` or `"initial"`).
    pub label: Option<String>,
    /// SHA-256 hex digest of the revision's content archive.
    pub content_digest: String,
    /// ISO-8601 creation timestamp.
    pub created_at: String,
    /// Number of files captured in this revision.
    pub file_count: usize,
    /// Unix permission bits for `BinFile` components only.
    ///
    /// Stored as `mode & 0o0755` (execute bits only; setuid/setgid/sticky
    /// always stripped). `None` for non-`BinFile` revisions.
    pub unix_mode: Option<u32>,
}

/// Associates a storage provider with a component.
///
/// `config` is intentionally typed as `serde_json::Value` — provider-specific
/// fields vary and should never include secret values; secrets live in the
/// provider's credential store, not here.
///
/// `Debug` is implemented manually to redact `config`, preventing accidental
/// credential exposure if a caller ever stores sensitive values there.
#[derive(Clone, Serialize, Deserialize)]
pub struct StashProviderRecord {
    /// Stable opaque identifier for this provider record.
    pub id: String,
    /// ID of the component this record is attached to.
    pub component_id: String,
    /// Provider driver name (e.g. `"filesystem"`).
    pub kind: String,
    /// Human-readable label for this provider instance.
    pub label: String,
    /// Provider-specific non-secret configuration.
    pub config: serde_json::Value,
}

impl std::fmt::Debug for StashProviderRecord {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("StashProviderRecord")
            .field("id", &self.id)
            .field("component_id", &self.component_id)
            .field("kind", &self.kind)
            .field("label", &self.label)
            .field("config", &"<redacted>")
            .finish()
    }
}

/// Lightweight summary of a storage provider, used in list responses.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StashProviderSummary {
    /// Stable opaque identifier.
    pub id: String,
    /// Provider driver name (e.g. `"filesystem"`).
    pub kind: String,
    /// Human-readable label.
    pub label: String,
    /// Number of components currently managed by this provider.
    pub component_count: usize,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn marketplace_origin_round_trips() {
        let origin = StashOrigin::Marketplace(MarketplaceOrigin {
            plugin_id: "demo@labby".to_string(),
            artifact_path: Some("skills/demo/SKILL.md".to_string()),
            source_version: Some("abc123".to_string()),
            source_fingerprint: Some("def456".to_string()),
        });

        let encoded = serde_json::to_value(&origin).unwrap();
        assert_eq!(
            encoded,
            json!({
                "kind": "marketplace",
                "plugin_id": "demo@labby",
                "artifact_path": "skills/demo/SKILL.md",
                "source_version": "abc123",
                "source_fingerprint": "def456"
            })
        );

        let decoded: StashOrigin = serde_json::from_value(encoded).unwrap();
        assert_eq!(decoded, origin);
    }

    #[test]
    fn local_path_origin_round_trips() {
        let origin = StashOrigin::LocalPath {
            source_path: PathBuf::from("/tmp/demo"),
        };

        let encoded = serde_json::to_value(&origin).unwrap();
        assert_eq!(
            encoded,
            json!({
                "kind": "local_path",
                "source_path": "/tmp/demo"
            })
        );

        let decoded: StashOrigin = serde_json::from_value(encoded).unwrap();
        assert_eq!(decoded, origin);
    }

    #[test]
    fn component_origin_meta_is_optional_for_existing_records() {
        let value = json!({
            "id": "01aryz6s41tpz5x11k39dv3r2g",
            "kind": "skill",
            "name": "demo",
            "label": null,
            "head_revision_id": null,
            "origin": null,
            "workspace_root": "/tmp/demo",
            "workspace_shape": "directory",
            "unix_mode": null,
            "created_at": "2026-06-13T00:00:00Z",
            "updated_at": "2026-06-13T00:00:00Z"
        });

        let component: StashComponent = serde_json::from_value(value).unwrap();
        assert!(component.origin_meta.is_none());
    }

    #[test]
    fn plugin_kind_round_trips_for_whole_plugin_forks() {
        let encoded = serde_json::to_value(StashComponentKind::Plugin).unwrap();
        assert_eq!(encoded, json!("plugin"));

        let decoded: StashComponentKind = serde_json::from_value(encoded).unwrap();
        assert_eq!(decoded, StashComponentKind::Plugin);
    }
}
