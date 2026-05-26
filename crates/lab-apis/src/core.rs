//! Cross-cutting primitives shared by every service module.

/// Authentication enum: ApiKey, Token, Basic, Bearer.
pub mod auth;

/// Shared HTTP client with retries, backoff, rate limiting, and tracing.
pub mod http;

/// Canonical error type and `ApiError::kind()` taxonomy.
pub mod error;

/// `ServiceStatus` health-check shape.
pub mod status;

/// `ActionSpec` / `ParamSpec` — discovery metadata.
pub mod action;

/// `PluginMeta` — per-service constants for TUI / install / doctor.
pub mod plugin;

/// `UiSchema` / `FieldKind` / `FieldValidation` / `WizardKind` — Bootstrap wizard + Settings rail.
pub mod plugin_ui;

/// `ServiceClient` trait — common surface every service implements.
pub mod traits;

/// Shared SSH primitives (host config parsing, hardened options) used by
/// `deploy`.
pub mod ssh;

// Convenience re-exports so service modules can `use crate::core::{Auth, HttpClient, ApiError, ...}`.
pub use action::{ActionSpec, ParamSpec};
pub use auth::Auth;
pub use error::ApiError;
pub use http::HttpClient;
pub use plugin::{Category, EnvVar, PluginMeta};
pub use plugin_ui::{
    BOOL_FIELD, FIELD_VALIDATION_DEFAULT, FieldKind, FieldValidation, SECRET_FIELD,
    SECRET_OPTIONAL_FIELD, TEXT_FIELD, TEXT_OPTIONAL_FIELD, UI_SCHEMA_DEFAULT, URL_FIELD,
    URL_OPTIONAL_FIELD, UiSchema, WizardKind, file_path_within_root,
};
pub use status::ServiceStatus;
pub use traits::ServiceClient;
