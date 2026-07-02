//! UI schema types for the Bootstrap wizard and Settings rail.
//!
//! Re-exported from the dependency-free `labby-primitives` leaf crate so that
//! `labby-apis` and `labby-gateway` share the exact same `UiSchema` types
//! without depending on each other. See `labby_primitives::plugin_ui` for the
//! type definitions and docs.

pub use labby_primitives::plugin_ui::{
    BOOL_FIELD, FIELD_VALIDATION_DEFAULT, FieldKind, FieldValidation, SECRET_FIELD,
    SECRET_OPTIONAL_FIELD, TEXT_FIELD, TEXT_OPTIONAL_FIELD, UI_SCHEMA_DEFAULT, UiSchema,
    URL_FIELD, URL_OPTIONAL_FIELD, WizardKind, file_path_within_root,
};
