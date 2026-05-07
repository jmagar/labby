//! Surface-neutral error type for dispatch operations.
//!
//! `ToolError` is the single canonical error type across all three surfaces
//! (MCP, API, CLI). It lives here in `dispatch/` because it is
//! surface-neutral — no surface module should own it.

use serde::Serialize;

/// Error variants that dispatchers can produce on top of SDK errors.
///
/// **Serialization contract:** `Serialize` is hand-written so the `Sdk` variant
/// promotes `sdk_kind` to the top-level `kind` field. `Deserialize` is NOT
/// derived — the derived impl would expect `{"kind":"sdk","sdk_kind":"..."}`,
/// which disagrees with the wire format `{"kind":"auth_failed","message":"..."}`.
/// If you need deserialization, deserialize into `serde_json::Value` and
/// construct variants manually.
#[derive(Debug, Clone)]
pub enum ToolError {
    /// Action name not recognized for this service.
    UnknownAction {
        /// Human-readable message.
        message: String,
        /// Valid action names for this service.
        valid: Vec<String>,
        /// Optional fuzzy suggestion.
        hint: Option<String>,
    },
    /// Required parameter missing.
    MissingParam {
        /// Human-readable message.
        message: String,
        /// Parameter name.
        param: String,
    },
    /// Parameter present but wrong type or value.
    InvalidParam {
        /// Human-readable message.
        message: String,
        /// Parameter name.
        param: String,
    },
    /// Multi-instance label not found.
    #[allow(dead_code)]
    UnknownInstance {
        /// Human-readable message.
        message: String,
        /// Known instance labels.
        valid: Vec<String>,
    },
    /// Tool name matched multiple upstream tools; caller must qualify it
    /// with the upstream prefix (e.g. `upstream::tool_name`).
    AmbiguousTool {
        /// Human-readable message.
        message: String,
        /// Fully-qualified candidate names the caller should choose from.
        valid: Vec<String>,
    },
    /// Destructive action invoked without the required confirmation signal.
    ConfirmationRequired {
        /// Human-readable message.
        message: String,
    },
    /// Resource already exists with the given identifier.
    Conflict {
        /// Human-readable message.
        message: String,
        /// The identifier of the conflicting resource.
        existing_id: String,
    },
    /// Pass-through of an `ApiError::kind()` tag from the SDK.
    Sdk {
        /// Stable kind tag (`auth_failed`, `rate_limited`, …).
        sdk_kind: String,
        /// Human-readable message.
        message: String,
    },
}

impl Serialize for ToolError {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let v = match self {
            Self::UnknownAction {
                message,
                valid,
                hint,
            } => serde_json::json!({
                "kind": "unknown_action",
                "message": message,
                "valid": valid,
                "hint": hint,
            }),
            Self::MissingParam { message, param } => serde_json::json!({
                "kind": "missing_param",
                "message": message,
                "param": param,
            }),
            Self::InvalidParam { message, param } => serde_json::json!({
                "kind": "invalid_param",
                "message": message,
                "param": param,
            }),
            Self::UnknownInstance { message, valid } => serde_json::json!({
                "kind": "unknown_instance",
                "message": message,
                "valid": valid,
            }),
            Self::AmbiguousTool { message, valid } => serde_json::json!({
                "kind": "ambiguous_tool",
                "message": message,
                "valid": valid,
            }),
            Self::ConfirmationRequired { message } => serde_json::json!({
                "kind": "confirmation_required",
                "message": message,
            }),
            Self::Conflict {
                message,
                existing_id,
            } => serde_json::json!({
                "kind": "conflict",
                "message": message,
                "existing_id": existing_id,
            }),
            Self::Sdk { sdk_kind, message } => serde_json::json!({
                "kind": sdk_kind,
                "message": message,
            }),
        };
        v.serialize(serializer)
    }
}

impl std::fmt::Display for ToolError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Serialize to the stable JSON envelope so callers get a machine-readable string.
        match serde_json::to_string(self) {
            Ok(s) => f.write_str(&s),
            Err(_) => write!(f, "{self:?}"),
        }
    }
}

impl std::error::Error for ToolError {}

impl ToolError {
    /// Canonical stable string tag.
    #[must_use]
    pub const fn kind(&self) -> &str {
        match self {
            Self::UnknownAction { .. } => "unknown_action",
            Self::MissingParam { .. } => "missing_param",
            Self::InvalidParam { .. } => "invalid_param",
            Self::UnknownInstance { .. } => "unknown_instance",
            Self::AmbiguousTool { .. } => "ambiguous_tool",
            Self::ConfirmationRequired { .. } => "confirmation_required",
            Self::Conflict { .. } => "conflict",
            Self::Sdk { sdk_kind, .. } => sdk_kind.as_str(),
        }
    }

    /// Whether this error represents an internal/fatal failure (ERROR level)
    /// vs a caller/user error (WARN level).
    ///
    /// Per OBSERVABILITY.md:
    /// - WARN: user/caller errors the caller can fix
    /// - ERROR: unhandled/fatal errors requiring operator investigation
    #[must_use]
    pub fn is_internal(&self) -> bool {
        matches!(
            self.kind(),
            "internal_error" | "server_error" | "decode_error"
        )
    }

    #[must_use]
    pub fn internal_message(message: impl Into<String>) -> Self {
        Self::Sdk {
            sdk_kind: "internal_error".to_string(),
            message: message.into(),
        }
    }
}

// ── From<ServiceError> for ToolError ─────────────────────────────────────
//
// All SDK error → ToolError conversions live here (not in MCP or HTTP
// surface modules) so both surfaces share one conversion path.
// Each impl is feature-gated to match its service.
//
// Adding a new service requires one macro invocation:
//
//   impl_tool_error_from!(
//       "myservice",
//       lab_apis::myservice::error::MyServiceError,
//       Api(api) => api.kind()         // standard arm — covers ApiError wrapper
//   );
//
// For services with additional error variants:
//
//   impl_tool_error_from!(
//       "radarr",
//       lab_apis::radarr::error::RadarrError,
//       Api(api) => api.kind(),
//       NotFound { .. } => "not_found"
//   );

/// Generate a feature-gated `From<$err> for ToolError` impl.
///
/// The macro imports all variants of `$err` via `use $err::*` so arms need
/// not be fully qualified.  All arms must evaluate to `&str`.
macro_rules! impl_tool_error_from {
    ($feature:literal, $err:path, $($arm:pat => $kind:expr),+ $(,)?) => {
        #[cfg(feature = $feature)]
        impl From<$err> for ToolError {
            fn from(e: $err) -> Self {
                #[allow(unused_imports)]
                use $err::*;
                let kind: &str = match &e {
                    $($arm => $kind,)+
                };
                Self::Sdk {
                    sdk_kind: kind.to_string(),
                    message: e.to_string(),
                }
            }
        }
    };
}

impl_tool_error_from!(
    "bytestash",
    lab_apis::bytestash::error::ByteStashError,
    Api(api) => api.kind()
);

impl_tool_error_from!(
    "radarr",
    lab_apis::radarr::error::RadarrError,
    Api(api) => api.kind(),
    NotFound { .. } => "not_found"
);

impl_tool_error_from!(
    "sabnzbd",
    lab_apis::sabnzbd::error::SabnzbdError,
    Api(api) => api.kind()
);

impl_tool_error_from!(
    "unifi",
    lab_apis::unifi::error::UnifiError,
    Api(api) => api.kind()
);

// unraid uses Http variant (not Api) as the ApiError wrapper.
impl_tool_error_from!(
    "unraid",
    lab_apis::unraid::UnraidError,
    Http(api) => api.kind()
);

impl_tool_error_from!(
    "gotify",
    lab_apis::gotify::error::GotifyError,
    Api(api) => api.kind()
);

impl_tool_error_from!(
    "qdrant",
    lab_apis::qdrant::error::QdrantError,
    Api(api) => api.kind()
);

impl_tool_error_from!(
    "tei",
    lab_apis::tei::error::TeiError,
    Api(api) => api.kind()
);

impl_tool_error_from!(
    "apprise",
    lab_apis::apprise::error::AppriseError,
    Api(api) => api.kind()
);

impl_tool_error_from!(
    "linkding",
    lab_apis::linkding::error::LinkdingError,
    Api(api) => api.kind()
);

impl_tool_error_from!(
    "beads",
    lab_apis::beads::error::BeadsError,
    NotConfigured { .. } => "internal_error",
    Connect { .. } => "network_error",
    Query { .. } => "upstream_error",
    InvalidIdentifier { .. } => "invalid_param",
    Decode { .. } => "decode_error"
);

impl_tool_error_from!(
    "dozzle",
    lab_apis::dozzle::error::DozzleError,
    Api(api) => api.kind(),
    InvalidResponse(_) => "decode_error",
    StreamTimeout(_) => "timeout"
);

impl_tool_error_from!(
    "immich",
    lab_apis::immich::error::ImmichError,
    Api(api) => api.kind(),
    InvalidParam(_) => "invalid_param"
);

impl_tool_error_from!(
    "jellyfin",
    lab_apis::jellyfin::error::JellyfinError,
    Api(api) => api.kind(),
    InvalidParam(_) => "invalid_param"
);

impl_tool_error_from!(
    "adguard",
    lab_apis::adguard::error::AdguardError,
    Api(api) => api.kind(),
    InvalidParam(_) => "invalid_param"
);

impl_tool_error_from!(
    "glances",
    lab_apis::glances::error::GlancesError,
    Api(api) => api.kind(),
    InvalidParam(_) => "invalid_param"
);

impl_tool_error_from!(
    "pihole",
    lab_apis::pihole::error::PiholeError,
    Api(api) => api.kind(),
    InvalidParam(_) => "invalid_param"
);

impl_tool_error_from!(
    "neo4j",
    lab_apis::neo4j::error::Neo4jError,
    Api(api) => api.kind(),
    InvalidParam(_) => "invalid_param",
    InsecureScheme(_) => "validation_failed",
    Timeout(_) => "timeout"
);

impl_tool_error_from!(
    "uptime_kuma",
    lab_apis::uptime_kuma::error::UptimeKumaError,
    Api(api) => api.kind(),
    Socket(_) => "network_error",
    Auth(_) => "auth_failed",
    Timeout(_) => "timeout",
    InvalidParam(_) => "invalid_param",
    Unsupported(_) => "server_error"
);

impl_tool_error_from!(
    "navidrome",
    lab_apis::navidrome::error::NavidromeError,
    Api(api) => api.kind(),
    InvalidParam(_) => "invalid_param"
);

impl_tool_error_from!(
    "scrutiny",
    lab_apis::scrutiny::error::ScrutinyError,
    Api(api) => api.kind()
);

impl_tool_error_from!(
    "freshrss",
    lab_apis::freshrss::error::FreshrssError,
    Api(api) => api.kind(),
    InvalidParam(_) => "invalid_param",
    MissingAuthToken => "auth_failed"
);

impl_tool_error_from!(
    "loggifly",
    lab_apis::loggifly::error::LoggiflyError,
    Api(api) => api.kind(),
    Io(_) => "internal_error"
);

impl_tool_error_from!(
    "paperless",
    lab_apis::paperless::error::PaperlessError,
    Api(api) => api.kind()
);

impl_tool_error_from!(
    "prowlarr",
    lab_apis::prowlarr::error::ProwlarrError,
    Api(api) => api.kind()
);

impl_tool_error_from!(
    "plex",
    lab_apis::plex::PlexError,
    Api(api) => api.kind()
);

impl_tool_error_from!(
    "sonarr",
    lab_apis::sonarr::error::SonarrError,
    Api(api) => api.kind()
);

impl_tool_error_from!(
    "overseerr",
    lab_apis::overseerr::OverseerrError,
    Api(api) => api.kind()
);

impl_tool_error_from!(
    "openai",
    lab_apis::openai::OpenAiError,
    Api(api) => api.kind()
);

impl_tool_error_from!("openacp", lab_apis::openacp::OpenAcpError, Api(api) => api.kind());

impl_tool_error_from!(
    "notebooklm",
    lab_apis::notebooklm::NotebookLmError,
    Api(api) => api.kind()
);

impl_tool_error_from!(
    "memos",
    lab_apis::memos::MemosError,
    Api(api) => api.kind()
);

impl_tool_error_from!(
    "tailscale",
    lab_apis::tailscale::TailscaleError,
    Api(api) => api.kind()
);

impl_tool_error_from!(
    "qbittorrent",
    lab_apis::qbittorrent::QbittorrentError,
    Api(api) => api.kind(),
    CommandFailed(_) => "server_error"
);

impl_tool_error_from!(
    "tautulli",
    lab_apis::tautulli::TautulliError,
    Api(api) => api.kind()
);

impl_tool_error_from!(
    "arcane",
    lab_apis::arcane::ArcaneError,
    Api(api) => api.kind()
);

// mcpregistry has an InvalidInput variant in addition to the standard Api wrapper.
#[cfg(feature = "mcpregistry")]
impl From<lab_apis::mcpregistry::error::RegistryError> for ToolError {
    fn from(e: lab_apis::mcpregistry::error::RegistryError) -> Self {
        use lab_apis::mcpregistry::error::RegistryError;
        match e {
            RegistryError::InvalidInput { ref message } => Self::Sdk {
                sdk_kind: "invalid_param".to_string(),
                message: message.clone(),
            },
            RegistryError::Api(ref api) => Self::Sdk {
                sdk_kind: api.kind().to_string(),
                message: e.to_string(),
            },
        }
    }
}

// RegistryStore errors mostly represent persistence failures. Invalid cursors
// remain caller-fixable `invalid_param`, and upstream fetch failures surface as
// `network_error`.
#[cfg(feature = "mcpregistry")]
impl From<crate::dispatch::marketplace::store::RegistryStoreError> for ToolError {
    fn from(e: crate::dispatch::marketplace::store::RegistryStoreError) -> Self {
        use crate::dispatch::marketplace::store::RegistryStoreError;
        let sdk_kind = match &e {
            RegistryStoreError::Upstream(_) => "network_error",
            RegistryStoreError::InvalidCursor(_) => "invalid_param",
            _ => "internal_error",
        };
        Self::Sdk {
            sdk_kind: sdk_kind.to_string(),
            message: format!("registry store: {e}"),
        }
    }
}

// acp_registry has an `Api { status, body }` variant in addition to the
// standard `Request(ApiError)` wrapper, so it gets a hand-rolled impl
// rather than going through the macro.
#[cfg(feature = "acp_registry")]
impl From<lab_apis::acp_registry::AcpRegistryError> for ToolError {
    fn from(e: lab_apis::acp_registry::AcpRegistryError) -> Self {
        use lab_apis::acp_registry::AcpRegistryError;
        let sdk_kind = match &e {
            AcpRegistryError::Request(api) => api.kind().to_string(),
            AcpRegistryError::Api { .. } => "server_error".to_string(),
        };
        Self::Sdk {
            sdk_kind,
            message: e.to_string(),
        }
    }
}

// Deploy uses a hand-rolled impl instead of the macro so it can call
// `redacted_message()` rather than `Display` (which includes host/reason detail
// that must not escape to MCP or HTTP envelopes).
#[cfg(feature = "deploy")]
impl From<lab_apis::deploy::DeployError> for ToolError {
    fn from(e: lab_apis::deploy::DeployError) -> Self {
        Self::Sdk {
            sdk_kind: e.kind().to_string(),
            message: e.redacted_message(),
        }
    }
}
