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
    /// with the upstream-qualified name (e.g. `<upstream>::tool_name`).
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
    /// Caller lacks the required OAuth scopes to invoke this tool.
    ///
    /// Replaces the bare `build_error_extra(..., "forbidden", ...)` path so
    /// scope denials flow through the canonical `ToolError` envelope (lab-l9n0n).
    Forbidden {
        /// Human-readable message.
        message: String,
        /// The scopes the caller would need to proceed.
        required_scopes: Vec<String>,
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
            Self::Forbidden {
                message,
                required_scopes,
            } => serde_json::json!({
                "kind": "forbidden",
                "message": message,
                "required_scopes": required_scopes,
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
            Self::Forbidden { .. } => "forbidden",
            Self::Sdk { sdk_kind, .. } => sdk_kind.as_str(),
        }
    }

    /// Human-readable message text. `Display` on `ToolError` emits the full
    /// JSON envelope; this returns only the message field so callers building
    /// nested error payloads (e.g. `BulkCloseFailure`) can avoid double-encoding.
    #[must_use]
    pub fn user_message(&self) -> &str {
        match self {
            Self::UnknownAction { message, .. }
            | Self::MissingParam { message, .. }
            | Self::InvalidParam { message, .. }
            | Self::UnknownInstance { message, .. }
            | Self::AmbiguousTool { message, .. }
            | Self::ConfirmationRequired { message }
            | Self::Conflict { message, .. }
            | Self::Forbidden { message, .. }
            | Self::Sdk { message, .. } => message.as_str(),
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
// All SDK error → ToolError conversions live here so all surfaces share one
// conversion path. Post gateway-pivot the per-service macro is gone; remaining
// impls are hand-written for the surviving feature-gated SDK error types.

// mcpregistry has an InvalidInput variant in addition to the standard Api wrapper.
#[cfg(feature = "marketplace")]
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
#[cfg(feature = "marketplace")]
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
            // The `Api { status, body }` envelope carries the upstream status,
            // so map it onto the canonical kind vocabulary rather than always
            // flattening to `server_error`.
            AcpRegistryError::Api { status, .. } => match status {
                401 | 403 => "auth_failed",
                404 => "not_found",
                429 => "rate_limited",
                _ => "server_error",
            }
            .to_string(),
        };
        Self::Sdk {
            sdk_kind,
            message: e.to_string(),
        }
    }
}

// The ACP archive installer (download/verify/extract/install primitive in
// lab-apis) carries its own stable `kind()` taxonomy that already matches the
// dispatcher vocabulary (`ssrf_blocked`, `integrity_missing`, `path_traversal`,
// `content_too_large`, …), so the conversion is a straight kind+message
// pass-through. Messages are built only from non-secret URL/host forms.
#[cfg(feature = "acp_registry")]
impl From<lab_apis::acp_registry::AcpInstallerError> for ToolError {
    fn from(e: lab_apis::acp_registry::AcpInstallerError) -> Self {
        Self::Sdk {
            sdk_kind: e.kind().to_string(),
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
