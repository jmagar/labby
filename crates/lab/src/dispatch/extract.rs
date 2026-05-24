//! Dispatch layer for the `extract` synthetic service.
//!
//! This is the always-on service; no feature flag needed.
//! All real work is delegated to `lab_apis::extract::ExtractClient`.

use std::path::PathBuf;

use lab_apis::core::action::{ActionSpec, ParamSpec};
use lab_apis::extract::{ExtractClient, RedactedExtractReport, ScanTarget, ServiceCreds, Uri};
use serde::Serialize;
use serde_json::Value;

use crate::config::{env_merge, write_service_creds};
use crate::dispatch::error::ToolError;
use crate::dispatch::helpers::{action_schema, help_payload, require_str, to_json};

/// Action catalog — read by `extract.help`, the `lab.help` meta-tool, and
/// the `lab://extract/actions` MCP resource. **One source of truth**.
pub const ACTIONS: &[ActionSpec] = &[
    ActionSpec {
        name: "help",
        description: "Show this action catalog",
        destructive: false,
        params: &[],
        returns: "Catalog",
    },
    ActionSpec {
        name: "schema",
        description: "Return the parameter schema for a named action",
        destructive: false,
        params: &[ParamSpec {
            name: "action",
            ty: "string",
            required: true,
            description: "Action name to describe",
        }],
        returns: "Schema",
    },
    ActionSpec {
        name: "list_hosts",
        description: "Return SSH config host aliases available for fleet scanning",
        destructive: false,
        params: &[],
        returns: "string[]",
    },
    ActionSpec {
        name: "scan",
        description: "Scan an appdata path and return discovered service credentials",
        destructive: false,
        params: &[
            ParamSpec {
                name: "uri",
                ty: "string",
                required: false,
                description: "Local path or 'host:/abs/path' for SSH; omit for fleet scan",
            },
            ParamSpec {
                name: "hosts",
                ty: "string[]",
                required: false,
                description: "Restrict fleet scan to these SSH config aliases; omit for all hosts",
            },
            ParamSpec {
                name: "redact_secrets",
                ty: "bool",
                required: false,
                description: "Return browser-safe results without secret values",
            },
        ],
        returns: "DiscoveredService[]",
    },
    ActionSpec {
        name: "apply",
        description: "Scan and write discovered credentials into ~/.lab/.env (with backup)",
        destructive: true,
        params: &[
            ParamSpec {
                name: "uri",
                ty: "string",
                required: true,
                description: "Same as scan",
            },
            ParamSpec {
                name: "services",
                ty: "string[]",
                required: false,
                description: "Optional filter; defaults to everything found",
            },
            ParamSpec {
                name: "env_path",
                ty: "string",
                required: false,
                description: "Override target env file path",
            },
            ParamSpec {
                name: "force",
                ty: "bool",
                required: false,
                description: "Overwrite conflicting env keys instead of skipping them",
            },
        ],
        returns: "WritePlan",
    },
    ActionSpec {
        name: "diff",
        description: "Show what 'apply' would change vs the current env file (no writes)",
        destructive: false,
        params: &[
            ParamSpec {
                name: "uri",
                ty: "string",
                required: true,
                description: "Local path or 'host:/abs/path' for SSH — same format as scan",
            },
            ParamSpec {
                name: "services",
                ty: "string[]",
                required: false,
                description: "Optional filter; defaults to everything found",
            },
            ParamSpec {
                name: "env_path",
                ty: "string",
                required: false,
                description: "Override target env file path",
            },
            ParamSpec {
                name: "force",
                ty: "bool",
                required: false,
                description: "Show overwrite changes instead of skipped conflicts",
            },
        ],
        returns: "WritePlan",
    },
];

/// Dispatch one call against the extract service.
///
/// # Errors
/// Returns errors from URI parsing, client scan, or unknown action lookup.
pub async fn dispatch(action: &str, params: Value) -> Result<Value, ToolError> {
    match action {
        "help" => Ok(help_payload("extract", ACTIONS)),
        "schema" => {
            let a = require_str(&params, "action")?;
            action_schema(ACTIONS, a)
        }
        "list_hosts" => {
            let client = ExtractClient::new();
            let hosts = client.list_hosts().map_err(|e| ToolError::Sdk {
                sdk_kind: "internal_error".into(),
                message: e.to_string(),
            })?;
            to_json(hosts)
        }
        "scan" => {
            let redact_secrets = parse_redact_secrets(&params)?;
            let client = ExtractClient::new();
            let report = match parse_scan_target(&params)? {
                ScanTarget::Fleet => {
                    if let Some(hosts) = parse_hosts_filter(&params)? {
                        client.scan_fleet_filtered(&hosts).await
                    } else {
                        client.scan(ScanTarget::Fleet).await
                    }
                }
                targeted => client.scan(targeted).await,
            }
            .map_err(|e| ToolError::Sdk {
                sdk_kind: "internal_error".into(),
                message: e.to_string(),
            })?;
            serialize_scan_report(report, redact_secrets)
        }
        "apply" => {
            // Destructive — the registry has already invoked elicitation
            // before we get here, otherwise dispatch would have short-circuited.
            let uri = parse_uri(&params)?;
            let force = parse_bool_param(&params, "force")?.unwrap_or(false);
            let env_path = parse_env_path(&params)?;
            let report = scan_targeted(uri).await?;
            let creds = filter_creds(report.creds, parse_services_filter(&params)?)?;
            let merge_request = merge_request_from_creds(&creds, force);
            let preview = env_merge::preview(&env_path, &merge_request).map_err(map_merge_err)?;
            let outcome = write_service_creds(&env_path, &creds, force).map_err(map_merge_err)?;
            to_json(WritePlan {
                env_path,
                credentials: creds.len(),
                preview,
                applied: true,
                written: outcome.written,
                skipped: outcome.skipped,
                backup_path: outcome.backup_path,
            })
        }
        "diff" => {
            let uri = parse_uri(&params)?;
            let force = parse_bool_param(&params, "force")?.unwrap_or(false);
            let env_path = parse_env_path(&params)?;
            let report = scan_targeted(uri).await?;
            let creds = filter_creds(report.creds, parse_services_filter(&params)?)?;
            let merge_request = merge_request_from_creds(&creds, force);
            let preview = env_merge::preview(&env_path, &merge_request).map_err(map_merge_err)?;
            to_json(WritePlan {
                env_path,
                credentials: creds.len(),
                written: preview.written,
                skipped: preview.skipped.clone(),
                preview,
                applied: false,
                backup_path: None,
            })
        }
        unknown => Err(ToolError::UnknownAction {
            message: format!("unknown action 'extract.{unknown}'"),
            valid: ACTIONS.iter().map(|a| a.name.to_string()).collect(),
            hint: None,
        }),
    }
}

#[derive(Debug, Serialize)]
struct WritePlan {
    env_path: PathBuf,
    credentials: usize,
    preview: env_merge::MergePreview,
    applied: bool,
    written: usize,
    skipped: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    backup_path: Option<PathBuf>,
}

fn parse_redact_secrets(params: &Value) -> Result<bool, ToolError> {
    Ok(parse_bool_param(params, "redact_secrets")?.unwrap_or(false))
}

fn parse_bool_param(params: &Value, param: &'static str) -> Result<Option<bool>, ToolError> {
    match params.get(param) {
        None => Ok(None),
        Some(value) => value
            .as_bool()
            .map(Some)
            .ok_or_else(|| ToolError::InvalidParam {
                message: format!("parameter `{param}` must be a bool"),
                param: param.into(),
            }),
    }
}

fn serialize_scan_report(
    report: lab_apis::extract::ExtractReport,
    redact_secrets: bool,
) -> Result<Value, ToolError> {
    if redact_secrets {
        return to_json(RedactedExtractReport::from(report));
    }
    to_json(report)
}

fn parse_uri(params: &Value) -> Result<Uri, ToolError> {
    let s = params
        .get("uri")
        .and_then(Value::as_str)
        .ok_or_else(|| ToolError::MissingParam {
            message: "missing required param 'uri'".into(),
            param: "uri".into(),
        })?;
    s.parse()
        .map_err(|e: <Uri as std::str::FromStr>::Err| ToolError::Sdk {
            sdk_kind: "invalid_param".into(),
            message: e.to_string(),
        })
}

fn parse_hosts_filter(params: &Value) -> Result<Option<Vec<String>>, ToolError> {
    let Some(value) = params.get("hosts") else {
        return Ok(None);
    };
    let arr = value.as_array().ok_or_else(|| ToolError::InvalidParam {
        message: "parameter `hosts` must be an array of strings".into(),
        param: "hosts".into(),
    })?;
    let hosts = arr
        .iter()
        .map(|v| {
            v.as_str()
                .map(str::to_owned)
                .ok_or_else(|| ToolError::InvalidParam {
                    message: "each element of `hosts` must be a string".into(),
                    param: "hosts".into(),
                })
        })
        .collect::<Result<Vec<_>, _>>()?;
    Ok(if hosts.is_empty() { None } else { Some(hosts) })
}

fn parse_services_filter(params: &Value) -> Result<Option<Vec<String>>, ToolError> {
    let Some(value) = params.get("services") else {
        return Ok(None);
    };
    let arr = value.as_array().ok_or_else(|| ToolError::InvalidParam {
        message: "parameter `services` must be an array of strings".into(),
        param: "services".into(),
    })?;
    let services = arr
        .iter()
        .map(|v| {
            v.as_str()
                .map(|s| s.to_ascii_lowercase())
                .ok_or_else(|| ToolError::InvalidParam {
                    message: "each element of `services` must be a string".into(),
                    param: "services".into(),
                })
        })
        .collect::<Result<Vec<_>, _>>()?;
    Ok(if services.is_empty() {
        None
    } else {
        Some(services)
    })
}

fn parse_env_path(params: &Value) -> Result<PathBuf, ToolError> {
    match params.get("env_path") {
        None => default_env_path(),
        Some(Value::String(path)) if env_path_override_allowed() => Ok(PathBuf::from(path)),
        Some(Value::String(_)) => Err(ToolError::InvalidParam {
            message:
                "parameter `env_path` is only accepted when LAB_ALLOW_EXTRACT_ENV_PATH_OVERRIDE=1"
                    .into(),
            param: "env_path".into(),
        }),
        Some(_) => Err(ToolError::InvalidParam {
            message: "parameter `env_path` must be a string".into(),
            param: "env_path".into(),
        }),
    }
}

fn default_env_path() -> Result<PathBuf, ToolError> {
    let home = std::env::var("HOME").map_err(|_| ToolError::Sdk {
        sdk_kind: "internal_error".into(),
        message: "$HOME not set".into(),
    })?;
    Ok(PathBuf::from(home).join(".lab/.env"))
}

fn env_path_override_allowed() -> bool {
    std::env::var("LAB_ALLOW_EXTRACT_ENV_PATH_OVERRIDE").is_ok_and(|value| value == "1")
}

async fn scan_targeted(uri: Uri) -> Result<lab_apis::extract::ExtractReport, ToolError> {
    ExtractClient::new()
        .scan(ScanTarget::Targeted(uri))
        .await
        .map_err(|e| ToolError::Sdk {
            sdk_kind: "internal_error".into(),
            message: e.to_string(),
        })
}

fn filter_creds(
    creds: Vec<ServiceCreds>,
    services: Option<Vec<String>>,
) -> Result<Vec<ServiceCreds>, ToolError> {
    let Some(services) = services else {
        return Ok(creds);
    };
    let filtered: Vec<ServiceCreds> = creds
        .into_iter()
        .filter(|cred| services.iter().any(|service| service == &cred.service))
        .collect();
    if filtered.is_empty() {
        return Err(ToolError::InvalidParam {
            message: "parameter `services` matched no discovered credentials".into(),
            param: "services".into(),
        });
    }
    Ok(filtered)
}

fn merge_request_from_creds(creds: &[ServiceCreds], force: bool) -> env_merge::MergeRequest {
    let mut entries = Vec::new();
    for cred in creds {
        let svc_upper = cred.service.to_uppercase();
        if let Some(url) = &cred.url {
            entries.push(env_merge::EnvEntry::new(
                format!("{svc_upper}_URL"),
                url.clone(),
            ));
        }
        if let Some(secret) = &cred.secret {
            entries.push(env_merge::EnvEntry::new(
                cred.env_field.clone(),
                secret.clone(),
            ));
        }
    }
    env_merge::MergeRequest {
        entries,
        force,
        expected_mtime: None,
    }
}

fn map_merge_err(err: env_merge::MergeError) -> ToolError {
    ToolError::Sdk {
        sdk_kind: err.kind().into(),
        message: err.to_string(),
    }
}

fn parse_scan_target(params: &Value) -> Result<ScanTarget, ToolError> {
    match params.get("uri") {
        Some(Value::String(_)) => Ok(ScanTarget::Targeted(parse_uri(params)?)),
        Some(_) => Err(ToolError::Sdk {
            sdk_kind: "invalid_param".into(),
            message: "param 'uri' must be a string".into(),
        }),
        None => Ok(ScanTarget::Fleet),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use lab_apis::extract::ScanTarget;
    use serde_json::json;

    #[test]
    fn scan_without_uri_maps_to_fleet() {
        let target = parse_scan_target(&json!({})).expect("scan target");
        assert!(matches!(target, ScanTarget::Fleet));
    }

    #[test]
    fn scan_with_uri_maps_to_targeted() {
        let target = parse_scan_target(&json!({"uri": "/tmp/appdata"})).expect("scan target");
        assert!(matches!(target, ScanTarget::Targeted(_)));
    }

    #[test]
    fn apply_and_diff_still_require_uri() {
        assert!(parse_uri(&json!({})).is_err());
    }

    #[test]
    fn scan_rejects_non_string_uri() {
        let error = parse_scan_target(&json!({"uri": {"host": "squirts"}}))
            .expect_err("non-string uri should be rejected");
        assert!(matches!(
            error,
            ToolError::Sdk {
                sdk_kind,
                message
            } if sdk_kind == "invalid_param" && message == "param 'uri' must be a string"
        ));
    }

    #[test]
    fn scan_rejects_non_boolean_redact_flag() {
        let error = parse_redact_secrets(&json!({"redact_secrets": "yes"}))
            .expect_err("non-bool redact flag should be rejected");
        assert!(matches!(
            error,
            ToolError::InvalidParam { param, .. } if param == "redact_secrets"
        ));
    }

    #[test]
    fn env_path_override_is_rejected_by_default() {
        let error = parse_env_path(&json!({"env_path": "/tmp/custom.env"}))
            .expect_err("api env_path override should be gated");
        assert!(matches!(
            error,
            ToolError::InvalidParam { param, .. } if param == "env_path"
        ));
    }

    #[test]
    fn services_filter_is_case_insensitive_and_rejects_misses() {
        let creds = vec![ServiceCreds {
            service: "radarr".to_owned(),
            url: Some("http://localhost:7878".to_owned()),
            secret: Some("secret-key".to_owned()),
            env_field: "RADARR_API_KEY".to_owned(),
            source_host: None,
            probe_host: None,
            runtime: None,
            url_verified: false,
        }];

        let filtered = filter_creds(
            creds.clone(),
            parse_services_filter(&json!({"services": ["RADARR"]})).expect("services"),
        )
        .expect("filter");
        assert_eq!(filtered.len(), 1);

        assert!(
            filter_creds(
                creds,
                parse_services_filter(&json!({"services": ["sonarr"]})).expect("services"),
            )
            .is_err()
        );
    }

    #[test]
    fn redacted_scan_serialization_omits_secret_values() {
        let report = lab_apis::extract::ExtractReport {
            target: ScanTarget::Fleet,
            uri: None,
            found: vec!["radarr".to_owned()],
            creds: vec![ServiceCreds {
                service: "radarr".to_owned(),
                url: Some("http://100.64.0.12:7878".to_owned()),
                secret: Some("secret-key".to_owned()),
                env_field: "RADARR_API_KEY".to_owned(),
                source_host: Some("media-node".to_owned()),
                probe_host: Some("100.64.0.12".to_owned()),
                runtime: None,
                url_verified: true,
            }],
            warnings: vec![],
        };

        let value = serialize_scan_report(report, true).expect("serialize redacted report");

        assert_eq!(value["creds"][0]["secret_present"], true);
        assert!(value["creds"][0].get("secret").is_none());
    }
}
