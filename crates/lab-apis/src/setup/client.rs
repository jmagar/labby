//! Pure setup-side helpers.
//!
//! `SetupClient` is a synthetic marker — `setup` has no remote API to wrap
//! (all fs I/O lives in `crates/lab/src/dispatch/setup/`). This file holds
//! validation primitives that other crates can call without pulling in fs
//! or env machinery.

use crate::core::plugin_ui::{FieldKind, UiSchema};

use super::error::SetupError;

/// Marker type. Construct via `SetupClient::new()` to call validation
/// helpers without touching the filesystem.
#[derive(Debug, Default, Clone, Copy)]
pub struct SetupClient;

impl SetupClient {
    pub const fn new() -> Self {
        Self
    }

    /// Validate `value` against a [`UiSchema`] declaration. This is the
    /// defense-in-depth checker used by `setup.draft.set` to block client-side
    /// bypasses (the frontend also validates with Zod, but we never trust it).
    ///
    /// Returns `Ok(())` if the value passes; otherwise `Err` with the
    /// failure reason. fs-related checks (`FieldKind::FilePath` traversal)
    /// are handled in dispatch where Path semantics live.
    pub fn validate_against_ui_schema(
        field: &str,
        value: &str,
        schema: &UiSchema,
    ) -> Result<(), SetupError> {
        // Length / required gating from FieldValidation.
        let v = &schema.validation;
        if v.required && value.is_empty() {
            return Err(SetupError::InvalidValue {
                field: field.to_owned(),
                reason: "required".into(),
            });
        }
        if let Some(min) = v.min_length
            && value.chars().count() < min
        {
            return Err(SetupError::InvalidValue {
                field: field.to_owned(),
                reason: format!("shorter than min_length={min}"),
            });
        }
        if let Some(max) = v.max_length
            && value.chars().count() > max
        {
            return Err(SetupError::InvalidValue {
                field: field.to_owned(),
                reason: format!("longer than max_length={max}"),
            });
        }

        match schema.kind {
            FieldKind::Text | FieldKind::Secret => Ok(()),
            FieldKind::Url => {
                let parsed = url::Url::parse(value).map_err(|e| SetupError::InvalidValue {
                    field: field.to_owned(),
                    reason: format!("not a URL: {e}"),
                })?;
                let scheme = parsed.scheme();
                if scheme != "http" && scheme != "https" {
                    return Err(SetupError::InvalidValue {
                        field: field.to_owned(),
                        reason: format!("scheme must be http or https, got {scheme}"),
                    });
                }
                Ok(())
            }
            FieldKind::Bool => match value {
                "true" | "false" | "1" | "0" => Ok(()),
                _ => Err(SetupError::InvalidValue {
                    field: field.to_owned(),
                    reason: "not a boolean (true|false|1|0)".into(),
                }),
            },
            FieldKind::Number => {
                value.parse::<f64>().map_err(|e| SetupError::InvalidValue {
                    field: field.to_owned(),
                    reason: format!("not a number: {e}"),
                })?;
                Ok(())
            }
            FieldKind::FilePath => {
                use std::path::{Component, Path};
                let path = Path::new(value);
                let mut saw_root = false;
                let mut saw_prefix = false;
                for component in path.components() {
                    match component {
                        Component::ParentDir => {
                            return Err(SetupError::InvalidValue {
                                field: field.to_owned(),
                                reason: "path traversal (..) is not allowed".into(),
                            });
                        }
                        Component::RootDir => saw_root = true,
                        Component::Prefix(_) => saw_prefix = true,
                        _ => {}
                    }
                }
                // Absolute paths are only allowed when the schema explicitly
                // declares a safe_root (the wizard validates the result lives
                // inside that root). Otherwise reject defensively.
                if (saw_root || saw_prefix) && schema.validation.safe_root.is_none() {
                    return Err(SetupError::InvalidValue {
                        field: field.to_owned(),
                        reason: "absolute paths require a configured safe_root".into(),
                    });
                }
                Ok(())
            }
            FieldKind::Enum { values } => {
                if values.iter().any(|allowed| *allowed == value) {
                    Ok(())
                } else {
                    Err(SetupError::InvalidValue {
                        field: field.to_owned(),
                        reason: format!("must be one of: {}", values.join(", ")),
                    })
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn schema(kind: FieldKind) -> UiSchema {
        UiSchema {
            kind,
            ..Default::default()
        }
    }

    #[test]
    fn url_must_parse() {
        SetupClient::validate_against_ui_schema(
            "FOO_URL",
            "https://example.com",
            &schema(FieldKind::Url),
        )
        .unwrap();
        assert!(
            SetupClient::validate_against_ui_schema(
                "FOO_URL",
                "not a url",
                &schema(FieldKind::Url)
            )
            .is_err()
        );
    }

    #[test]
    fn url_rejects_unknown_scheme() {
        let err = SetupClient::validate_against_ui_schema(
            "FOO_URL",
            "ftp://example.com",
            &schema(FieldKind::Url),
        )
        .unwrap_err();
        let msg = format!("{err}");
        assert!(msg.contains("scheme"), "got: {msg}");
    }

    #[test]
    fn file_path_rejects_traversal() {
        let err = SetupClient::validate_against_ui_schema(
            "DATA_DIR",
            "../etc/passwd",
            &schema(FieldKind::FilePath),
        )
        .unwrap_err();
        assert!(format!("{err}").contains("traversal"));
    }

    #[test]
    fn enum_accepts_allowlist() {
        let kind = FieldKind::Enum {
            values: &["a", "b"],
        };
        SetupClient::validate_against_ui_schema("MODE", "a", &schema(kind)).unwrap();
        assert!(SetupClient::validate_against_ui_schema("MODE", "c", &schema(kind)).is_err());
    }

    #[test]
    fn bool_accepts_canonical_forms() {
        for v in &["true", "false", "1", "0"] {
            SetupClient::validate_against_ui_schema("FLAG", v, &schema(FieldKind::Bool)).unwrap();
        }
        assert!(
            SetupClient::validate_against_ui_schema("FLAG", "yes", &schema(FieldKind::Bool))
                .is_err()
        );
    }
}
