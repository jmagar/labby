//! UI schema types for the Bootstrap wizard and Settings rail.
//!
//! `UiSchema` is attached to `EnvVar` as a compile-time `&'static` reference.
//! The SDK owns only const-friendly metadata. Projection into JSON Schema or a
//! concrete frontend widget model belongs at the binary/UI boundary.

use std::path::{Component, Path};

/// How a single environment-variable field should be rendered and validated.
#[derive(Debug, Clone, Copy, Default)]
pub struct UiSchema {
    /// Widget kind (text box, password field, URL bar, toggle, etc.).
    pub kind: FieldKind,
    /// Inline validation constraints applied before submit.
    pub validation: FieldValidation,
    /// True when the field should be hidden behind advanced settings by default.
    pub advanced: bool,
    /// Optional external help URL. Audit allows `https://`, plus localhost HTTP.
    pub help_url: Option<&'static str>,
    /// Optional single parent field dependency.
    pub depends_on: Option<&'static str>,
    /// Optional custom wizard hint. `None` means the standard wizard.
    pub wizard_kind: Option<WizardKind>,
    /// Optional action name used to fetch dynamic enum values.
    pub dynamic_source: Option<&'static str>,
}

/// Default validation constraints for const struct update syntax.
pub const FIELD_VALIDATION_DEFAULT: FieldValidation = FieldValidation {
    min_length: None,
    max_length: None,
    pattern: None,
    required: false,
    safe_root: None,
};

/// Default UI schema for const struct update syntax.
pub const UI_SCHEMA_DEFAULT: UiSchema = UiSchema {
    kind: FieldKind::Text,
    validation: FIELD_VALIDATION_DEFAULT,
    advanced: false,
    help_url: None,
    depends_on: None,
    wizard_kind: None,
    dynamic_source: None,
};

/// Input widget variant.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum FieldKind {
    /// Free-form single-line text.
    #[default]
    Text,
    /// Masked password / API key / token input.
    Secret,
    /// URL input with scheme validation.
    Url,
    /// Boolean toggle / checkbox.
    Bool,
    /// Numeric input.
    Number,
    /// File or directory path constrained by `FieldValidation::safe_root`.
    FilePath,
    /// Static enum values. Dynamic enums use `UiSchema::dynamic_source`.
    Enum { values: &'static [&'static str] },
}

/// Client-side validation rules applied before the wizard advances.
#[derive(Debug, Clone, Copy, Default)]
pub struct FieldValidation {
    /// Minimum string length (inclusive).
    pub min_length: Option<usize>,
    /// Maximum string length (inclusive).
    pub max_length: Option<usize>,
    /// ECMAScript-compatible regex the value must match.
    pub pattern: Option<&'static str>,
    /// Whether the field accepts empty / missing values.
    pub required: bool,
    /// Safe root for `FieldKind::FilePath` validation.
    pub safe_root: Option<&'static str>,
}

/// Optional per-service wizard customisation hint.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WizardKind {
    /// Custom multi-step wizard identified by a static slug.
    Custom(&'static str),
}

/// Validate a user-provided file path under an explicit safe root.
///
/// This does not canonicalize. It rejects any `..` component before joining,
/// then verifies the final path still starts with the supplied safe root.
#[must_use]
pub fn file_path_within_root(input: &str, safe_root: &Path) -> bool {
    let path = Path::new(input);
    if path
        .components()
        .any(|component| component == Component::ParentDir)
    {
        return false;
    }

    let final_path = if path.is_absolute() {
        path.to_path_buf()
    } else {
        safe_root.join(path)
    };

    final_path.starts_with(safe_root)
}

/// Standard URL field schema (http/https, required).
pub const URL_FIELD: UiSchema = UiSchema {
    kind: FieldKind::Url,
    validation: FieldValidation {
        min_length: Some(7),
        max_length: None,
        pattern: Some("^https?://"),
        required: true,
        safe_root: None,
    },
    ..UI_SCHEMA_DEFAULT
};

/// Standard API-key / token field schema (masked, required).
pub const SECRET_FIELD: UiSchema = UiSchema {
    kind: FieldKind::Secret,
    validation: FieldValidation {
        min_length: Some(1),
        max_length: None,
        pattern: None,
        required: true,
        safe_root: None,
    },
    ..UI_SCHEMA_DEFAULT
};

/// Standard optional API-key / token field schema (masked, not required).
pub const SECRET_OPTIONAL_FIELD: UiSchema = UiSchema {
    kind: FieldKind::Secret,
    validation: FIELD_VALIDATION_DEFAULT,
    ..UI_SCHEMA_DEFAULT
};

/// Standard optional URL field schema (http/https, not required).
pub const URL_OPTIONAL_FIELD: UiSchema = UiSchema {
    kind: FieldKind::Url,
    validation: FieldValidation {
        min_length: None,
        max_length: None,
        pattern: Some("^https?://"),
        required: false,
        safe_root: None,
    },
    ..UI_SCHEMA_DEFAULT
};

/// Standard free-form text field (required).
pub const TEXT_FIELD: UiSchema = UiSchema {
    validation: FieldValidation {
        min_length: Some(1),
        max_length: None,
        pattern: None,
        required: true,
        safe_root: None,
    },
    ..UI_SCHEMA_DEFAULT
};

/// Standard optional free-form text field.
pub const TEXT_OPTIONAL_FIELD: UiSchema = UI_SCHEMA_DEFAULT;

/// Boolean toggle field.
pub const BOOL_FIELD: UiSchema = UiSchema {
    kind: FieldKind::Bool,
    ..UI_SCHEMA_DEFAULT
};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_schema_is_not_advanced() {
        assert!(!UiSchema::default().advanced);
    }

    #[test]
    fn file_path_rejects_parent_dir_components() {
        let root = Path::new("/safe/root");
        assert!(!file_path_within_root("../escape", root));
        assert!(!file_path_within_root("nested/../../escape", root));
    }

    #[test]
    fn file_path_accepts_relative_path_under_root() {
        let root = Path::new("/safe/root");
        assert!(file_path_within_root("nested/file.txt", root));
    }

    #[test]
    fn file_path_rejects_absolute_path_outside_root() {
        let root = Path::new("/safe/root");
        assert!(!file_path_within_root("/tmp/file.txt", root));
    }
}
