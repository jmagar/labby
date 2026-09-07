//! Provider-neutral dispatch catalogs for remote Artifact operations.

use labby_apis::artifact_control::Operation;
use labby_primitives::action::{ActionSpec, ParamSpec};
use serde_json::Value;

use crate::dispatch::error::ToolError;
use crate::dispatch::helpers::{action_schema, help_payload, require_str};

const CONNECTION: ParamSpec = ParamSpec {
    name: "connection_id",
    ty: "string",
    required: false,
    description: "Configured remote Artifact authority; optional when exactly one is configured",
};
const CURSOR: ParamSpec = ParamSpec {
    name: "cursor",
    ty: "string",
    required: false,
    description: "Opaque remote continuation cursor",
};
const LIMIT: ParamSpec = ParamSpec {
    name: "limit",
    ty: "integer",
    required: false,
    description: "Bounded remote page size",
};
const ID: ParamSpec = ParamSpec {
    name: "id",
    ty: "string",
    required: true,
    description: "Stable item identifier",
};
const SLUG: ParamSpec = ParamSpec {
    name: "slug",
    ty: "string",
    required: true,
    description: "Bundle slug",
};

const fn spec(
    name: &'static str,
    description: &'static str,
    destructive: bool,
    requires_admin: bool,
    returns: &'static str,
    params: &'static [ParamSpec],
) -> ActionSpec {
    ActionSpec {
        name,
        description,
        destructive,
        requires_admin,
        returns,
        params,
    }
}

pub(crate) const SOURCE_ACTIONS: &[ActionSpec] = &[
    spec(
        "sources.list",
        "List configured ingestion sources and refresh state",
        false,
        false,
        "Source[]",
        &[CONNECTION],
    ),
    spec(
        "sources.configure",
        "Enable, pause, or schedule a remote ingestion source",
        false,
        true,
        "Source",
        &[
            CONNECTION,
            ID,
            ParamSpec {
                name: "enabled",
                ty: "boolean",
                required: false,
                description: "Whether scheduled refresh is enabled",
            },
            ParamSpec {
                name: "interval_seconds",
                ty: "integer",
                required: false,
                description: "Refresh interval in seconds",
            },
        ],
    ),
    spec(
        "sources.delete",
        "Delete a source while retaining ingested Artifacts",
        false,
        true,
        "DeleteReceipt",
        &[CONNECTION, ID],
    ),
    spec(
        "sources.refresh",
        "Schedule an immediate source refresh",
        false,
        true,
        "SourceRefreshReceipt",
        &[CONNECTION, ID],
    ),
];

pub(crate) const JOB_ACTIONS: &[ActionSpec] = &[
    spec(
        "jobs.start",
        "Start a durable asynchronous Artifact ingestion job",
        false,
        true,
        "IngestJobReceipt",
        &[
            CONNECTION,
            ParamSpec {
                name: "kind",
                ty: "string",
                required: true,
                description: "Supported ingestion source kind",
            },
            ParamSpec {
                name: "arguments",
                ty: "object",
                required: true,
                description: "Kind-specific source selectors; secret values are forbidden",
            },
            ParamSpec {
                name: "idempotency_key",
                ty: "string",
                required: false,
                description: "Stable idempotency key, maximum 256 bytes",
            },
        ],
    ),
    spec(
        "jobs.list",
        "List recent durable ingestion jobs",
        false,
        true,
        "IngestJob[]",
        &[CONNECTION, LIMIT],
    ),
    spec(
        "jobs.get",
        "Get ingestion progress and terminal outcome",
        false,
        true,
        "IngestJob",
        &[CONNECTION, ID],
    ),
    spec(
        "jobs.cancel",
        "Request cooperative cancellation of an active ingestion job",
        false,
        true,
        "IngestJob",
        &[CONNECTION, ID],
    ),
    spec(
        "jobs.retry",
        "Retry a terminal ingestion job from its persisted selectors",
        false,
        true,
        "IngestJob",
        &[CONNECTION, ID],
    ),
];

pub(crate) const UPLOAD_ACTIONS: &[ActionSpec] = &[
    spec(
        "uploads.create",
        "Create a short-lived upload slot for Artifact ingestion",
        false,
        true,
        "Upload",
        &[
            CONNECTION,
            ParamSpec {
                name: "filename",
                ty: "string",
                required: true,
                description: "Original archive or manifest filename",
            },
        ],
    ),
    spec(
        "uploads.get",
        "Inspect a principal-owned upload slot",
        false,
        true,
        "Upload",
        &[CONNECTION, ID],
    ),
    spec(
        "uploads.delete",
        "Delete a pending or ready upload slot",
        false,
        true,
        "DeleteReceipt",
        &[CONNECTION, ID],
    ),
];

pub(crate) const BUNDLE_ACTIONS: &[ActionSpec] = &[
    spec(
        "bundles.list",
        "List curated Artifact bundles with publication drift",
        false,
        false,
        "Bundle[]",
        &[CONNECTION],
    ),
    spec(
        "bundles.get",
        "Get a bundle draft and immutable published versions",
        false,
        false,
        "Bundle",
        &[CONNECTION, SLUG],
    ),
    spec(
        "bundles.create",
        "Create an empty curated Artifact bundle",
        false,
        true,
        "Bundle",
        &[
            CONNECTION,
            SLUG,
            ParamSpec {
                name: "description",
                ty: "string",
                required: false,
                description: "Human-readable bundle description",
            },
            ParamSpec {
                name: "visibility",
                ty: "public|bearer|oauth",
                required: false,
                description: "Bundle access mode",
            },
        ],
    ),
    spec(
        "bundles.add",
        "Add an Artifact to a bundle draft",
        false,
        true,
        "Bundle",
        &[
            CONNECTION,
            SLUG,
            ParamSpec {
                name: "namespace",
                ty: "string",
                required: true,
                description: "Artifact namespace",
            },
            ParamSpec {
                name: "name",
                ty: "string",
                required: true,
                description: "Artifact name",
            },
        ],
    ),
    spec(
        "bundles.remove",
        "Remove an Artifact from a bundle draft",
        false,
        true,
        "Bundle",
        &[
            CONNECTION,
            SLUG,
            ParamSpec {
                name: "namespace",
                ty: "string",
                required: true,
                description: "Artifact namespace",
            },
            ParamSpec {
                name: "name",
                ty: "string",
                required: true,
                description: "Artifact name",
            },
        ],
    ),
    spec(
        "bundles.set_visibility",
        "Change bundle access mode",
        false,
        true,
        "Bundle",
        &[
            CONNECTION,
            SLUG,
            ParamSpec {
                name: "visibility",
                ty: "public|bearer|oauth",
                required: true,
                description: "Bundle access mode",
            },
        ],
    ),
    spec(
        "bundles.publish",
        "Publish an immutable bundle snapshot",
        false,
        true,
        "BundlePublishReceipt",
        &[CONNECTION, SLUG],
    ),
    spec(
        "bundles.delete",
        "Delete a bundle and its published manifests",
        true,
        true,
        "DeleteReceipt",
        &[CONNECTION, SLUG],
    ),
];

pub(crate) const REMOTE_ARTIFACT_ACTIONS: &[ActionSpec] = &[
    spec(
        "artifacts.list_connections",
        "List safe configured Artifact authority identifiers",
        false,
        false,
        "ArtifactAuthorityConnectionPage",
        &[],
    ),
    crate::dispatch::artifact_control::CALLBACK_REMOTE_ACTIONS[0],
    crate::dispatch::artifact_control::CALLBACK_REMOTE_ACTIONS[1],
    crate::dispatch::artifact_control::CALLBACK_REMOTE_ACTIONS[2],
    crate::dispatch::artifact_control::CALLBACK_REMOTE_ACTIONS[3],
    spec(
        "artifacts.intake_candidate",
        "Validate and persist bounded Artifact candidate evidence",
        false,
        true,
        "ArtifactCandidate",
        &[
            CONNECTION,
            ParamSpec {
                name: "candidate",
                ty: "object",
                required: true,
                description: "dinglebear.artifact-candidate/v1 payload",
            },
        ],
    ),
    spec(
        "artifacts.follow",
        "Follow or unfollow an upstream Artifact while preserving revision identity",
        false,
        true,
        "RemoteArtifact",
        &[
            CONNECTION,
            ID,
            ParamSpec {
                name: "upstream_artifact_id",
                ty: "string",
                required: false,
                description: "Upstream Artifact identifier when following",
            },
            ParamSpec {
                name: "upstream_revision_id",
                ty: "string",
                required: false,
                description: "Optional exact upstream revision or digest",
            },
            ParamSpec {
                name: "following",
                ty: "boolean",
                required: false,
                description: "True to follow; false to unfollow",
            },
        ],
    ),
    spec(
        "artifacts.fork",
        "Fork an exact Artifact revision into a new hosted Artifact",
        false,
        true,
        "RemoteArtifact",
        &[
            CONNECTION,
            ParamSpec {
                name: "source_artifact_id",
                ty: "string",
                required: true,
                description: "Source Artifact identifier",
            },
            ParamSpec {
                name: "revision_id",
                ty: "string",
                required: false,
                description: "Optional exact source revision or digest",
            },
            ParamSpec {
                name: "namespace",
                ty: "string",
                required: true,
                description: "Target namespace",
            },
            ParamSpec {
                name: "name",
                ty: "string",
                required: true,
                description: "Target Artifact name",
            },
            ParamSpec {
                name: "following",
                ty: "boolean",
                required: false,
                description: "Whether the fork follows its source",
            },
        ],
    ),
    spec(
        "artifacts.set_publication",
        "Change publication state under the authority's redistribution policy",
        false,
        true,
        "RemoteArtifact",
        &[
            CONNECTION,
            ID,
            ParamSpec {
                name: "state",
                ty: "draft|listed|published|withdrawn",
                required: false,
                description: "Publication state",
            },
            ParamSpec {
                name: "visibility",
                ty: "private|unlisted|public",
                required: false,
                description: "Publication visibility",
            },
            ParamSpec {
                name: "distribution",
                ty: "metadata|bytes",
                required: false,
                description: "Distribution mode",
            },
        ],
    ),
    spec(
        "artifacts.set_license",
        "Set authoritative license review, redistribution, and takedown policy",
        false,
        true,
        "RemoteArtifact",
        &[
            CONNECTION,
            ID,
            ParamSpec {
                name: "declared",
                ty: "string|null",
                required: false,
                description: "Declared license; null clears it",
            },
            ParamSpec {
                name: "detected",
                ty: "array",
                required: false,
                description: "Detected license evidence",
            },
            ParamSpec {
                name: "notices",
                ty: "array",
                required: false,
                description: "License notice evidence",
            },
            ParamSpec {
                name: "redistribution",
                ty: "string",
                required: false,
                description: "Authoritative redistribution class",
            },
            ParamSpec {
                name: "review_state",
                ty: "unreviewed|reviewed|disputed",
                required: false,
                description: "License review state",
            },
            ParamSpec {
                name: "takedown_state",
                ty: "none|requested|restricted|removed",
                required: false,
                description: "Takedown state",
            },
            ParamSpec {
                name: "evidence_at",
                ty: "string",
                required: false,
                description: "RFC 3339 evidence timestamp",
            },
            ParamSpec {
                name: "metadata",
                ty: "object",
                required: false,
                description: "Bounded policy metadata",
            },
        ],
    ),
    spec(
        "artifacts.search_skills_sh",
        "Search the public skills.sh catalog without ingesting",
        false,
        false,
        "RemoteArtifactSearch",
        &[
            CONNECTION,
            ParamSpec {
                name: "query",
                ty: "string",
                required: true,
                description: "Search query",
            },
            LIMIT,
        ],
    ),
    spec(
        "artifacts.search_ard",
        "Search an Agentic Resource Discovery registry without ingesting",
        false,
        false,
        "RemoteArtifactSearch",
        &[
            CONNECTION,
            ParamSpec {
                name: "registry",
                ty: "string",
                required: true,
                description: "ARD registry base URL or domain",
            },
            ParamSpec {
                name: "query",
                ty: "string",
                required: true,
                description: "Search query",
            },
            ParamSpec {
                name: "page_token",
                ty: "string",
                required: false,
                description: "Opaque registry page token",
            },
        ],
    ),
    spec(
        "artifacts.search_marketplace",
        "Resolve a plugin marketplace into the Artifact targets it exposes",
        false,
        false,
        "RemoteArtifactSearch",
        &[
            CONNECTION,
            ParamSpec {
                name: "source",
                ty: "string",
                required: true,
                description: "Marketplace repository or manifest URL",
            },
            ParamSpec {
                name: "ref",
                ty: "string",
                required: false,
                description: "Optional Git ref",
            },
            ParamSpec {
                name: "only",
                ty: "array",
                required: false,
                description: "Optional plugin names to include",
            },
        ],
    ),
    spec(
        "artifacts.list_mcp_registry",
        "List MCP Registry servers available for Artifact ingestion",
        false,
        false,
        "McpRegistryPage",
        &[
            CONNECTION,
            ParamSpec {
                name: "query",
                ty: "string",
                required: false,
                description: "Search query",
            },
            ParamSpec {
                name: "category",
                ty: "string",
                required: false,
                description: "Category filter",
            },
            ParamSpec {
                name: "tag",
                ty: "string",
                required: false,
                description: "Tag filter",
            },
            ParamSpec {
                name: "version",
                ty: "string",
                required: false,
                description: "Exact version or latest",
            },
            ParamSpec {
                name: "updated_since",
                ty: "string",
                required: false,
                description: "RFC 3339 update watermark",
            },
            ParamSpec {
                name: "include_deleted",
                ty: "boolean",
                required: false,
                description: "Include deleted registry entries",
            },
            CURSOR,
            LIMIT,
        ],
    ),
    spec(
        "artifacts.list_acp_registry",
        "List ACP Registry agents available for Artifact ingestion",
        false,
        false,
        "AcpRegistryPage",
        &[CONNECTION],
    ),
    spec(
        "artifacts.authority_status",
        "Return the configured Artifact authority's health and catalog status",
        false,
        false,
        "ArtifactAuthorityStatus",
        &[CONNECTION],
    ),
];

pub(crate) async fn dispatch(
    service: &str,
    action: &str,
    params: Value,
) -> Result<Value, ToolError> {
    dispatch_with_context(service, action, params, None).await
}

pub(crate) async fn dispatch_with_context(
    service: &str,
    action: &str,
    params: Value,
    context: Option<&crate::dispatch::artifact_control::AuthorityContext>,
) -> Result<Value, ToolError> {
    if !crate::registry::runtime_built_in_upstream_apis_enabled() {
        return Err(ToolError::Sdk {
            sdk_kind: "source_unavailable".to_owned(),
            message: "Built-in upstream Artifact APIs are disabled".to_owned(),
        });
    }
    match action {
        "help" => return Ok(help_payload(service, actions(service))),
        "schema" => return action_schema(actions(service), require_str(&params, "action")?),
        _ => {}
    }
    let mut object = params
        .as_object()
        .cloned()
        .ok_or_else(|| ToolError::InvalidParam {
            message: "Control-plane parameters must be an object".to_owned(),
            param: "params".to_owned(),
        })?;
    let connection_id = take_connection_id(&mut object)?;
    reject_secret_values(&object)?;
    if context.is_none() {
        return Err(ToolError::Forbidden {
            message: "Remote Artifact operations require verified actor and project context"
                .to_owned(),
            required_scopes: vec!["lab:read".to_owned()],
        });
    }
    let controls =
        crate::dispatch::skill_library::process_controls().ok_or_else(|| ToolError::Sdk {
            sdk_kind: "source_unavailable".to_owned(),
            message: "Remote Artifact control plane is unavailable".to_owned(),
        })?;
    if (service, action) == ("artifacts", "artifacts.list_connections") {
        return Ok(controls.connections());
    }
    normalize_provider_params(service, action, &mut object)?;
    let operation = operation(service, action).ok_or_else(|| ToolError::UnknownAction {
        message: format!("unknown {service} action"),
        valid: actions(service)
            .iter()
            .map(|item| item.name.to_owned())
            .collect(),
        hint: None,
    })?;
    controls
        .execute(
            connection_id.as_deref(),
            operation,
            &Value::Object(object),
            context,
        )
        .await
}

fn take_connection_id(
    object: &mut serde_json::Map<String, Value>,
) -> Result<Option<String>, ToolError> {
    match object.remove("connection_id") {
        None => Ok(None),
        Some(Value::String(value)) if !value.trim().is_empty() => Ok(Some(value)),
        Some(_) => Err(ToolError::InvalidParam {
            message: "connection_id must be a non-empty string".to_owned(),
            param: "connection_id".to_owned(),
        }),
    }
}

fn normalize_provider_params(
    service: &str,
    action: &str,
    object: &mut serde_json::Map<String, Value>,
) -> Result<(), ToolError> {
    let rename = |object: &mut serde_json::Map<String, Value>, from: &str, to: &str| {
        if let Some(value) = object.remove(from) {
            object.insert(to.to_owned(), value);
        }
    };
    match (service, action) {
        ("sources", "sources.configure" | "sources.delete" | "sources.refresh") => {
            rename(object, "id", "sourceId")
        }
        ("jobs", "jobs.get" | "jobs.cancel" | "jobs.retry") => rename(object, "id", "jobId"),
        ("jobs", "jobs.start") => rename(object, "idempotency_key", "idempotencyKey"),
        ("uploads", "uploads.get" | "uploads.delete") => rename(object, "id", "uploadId"),
        ("artifacts", "artifacts.get_remote") => rename(object, "id", "artifactId"),
        (
            "artifacts",
            "artifacts.follow" | "artifacts.set_publication" | "artifacts.set_license",
        ) => rename(object, "id", "artifactId"),
        _ => {}
    }
    if service == "artifacts" {
        for (from, to) in [
            ("source_artifact_id", "sourceArtifactId"),
            ("revision_id", "revisionId"),
            ("upstream_artifact_id", "upstreamArtifactId"),
            ("upstream_revision_id", "upstreamRevisionId"),
            ("review_state", "reviewState"),
            ("takedown_state", "takedownState"),
            ("evidence_at", "evidenceAt"),
            ("page_token", "pageToken"),
            ("updated_since", "updatedSince"),
            ("include_deleted", "includeDeleted"),
        ] {
            rename(object, from, to);
        }
    }
    if service == "sources" && action == "sources.configure" {
        rename(object, "interval_seconds", "intervalSeconds");
    }
    if service == "jobs" && action == "jobs.start" {
        let allowed = [
            "repo",
            "well_known",
            "ard_catalog",
            "marketplace",
            "skills_sh",
            "mcp",
            "mcp_registry",
            "acp_registry",
            "archive",
        ];
        let kind =
            object
                .get("kind")
                .and_then(Value::as_str)
                .ok_or_else(|| ToolError::MissingParam {
                    message: "kind is required".to_owned(),
                    param: "kind".to_owned(),
                })?;
        if !allowed.contains(&kind) {
            return Err(ToolError::InvalidParam {
                message: "unsupported ingestion kind".to_owned(),
                param: "kind".to_owned(),
            });
        }
        if let Some(Value::Object(arguments)) = object.get_mut("arguments") {
            for (from, to) in [
                ("upload_id", "uploadId"),
                ("base_source", "baseSource"),
                ("repo_url", "repoUrl"),
                ("skill_id", "skillId"),
                ("registry_url", "registryUrl"),
                ("source_url", "sourceUrl"),
            ] {
                rename(arguments, from, to);
            }
        }
    }
    Ok(())
}

fn reject_secret_values(object: &serde_json::Map<String, Value>) -> Result<(), ToolError> {
    fn contains(value: &Value) -> bool {
        match value {
            Value::Object(map) => map.iter().any(|(key, value)| {
                let normalized = key.to_ascii_lowercase();
                let compact = normalized
                    .chars()
                    .filter(|character| character.is_ascii_alphanumeric())
                    .collect::<String>();
                let safe_opaque_token = matches!(compact.as_str(), "pagetoken" | "nextpagetoken");
                normalized.contains("authorization")
                    || normalized.contains("password")
                    || normalized.contains("secret")
                    || normalized.contains("credentialvalue")
                    || normalized.contains("credential_value")
                    || normalized.contains("apikey")
                    || normalized.contains("api_key")
                    || normalized.contains("privatekey")
                    || normalized.contains("private_key")
                    || normalized.contains("cookie")
                    || (normalized.contains("token") && !safe_opaque_token)
                    || contains(value)
            }),
            Value::Array(values) => values.iter().any(contains),
            _ => false,
        }
    }
    if contains(&Value::Object(object.clone())) {
        Err(ToolError::InvalidParam {
            message: "Secret values are not accepted; use a server-configured credential reference"
                .to_owned(),
            param: "params".to_owned(),
        })
    } else {
        Ok(())
    }
}

pub(crate) fn operation(service: &str, action: &str) -> Option<Operation> {
    Some(match (service, action) {
        ("artifacts", "artifacts.search_remote") => Operation::ArtifactsSearch,
        ("artifacts", "artifacts.list_remote") => Operation::ArtifactsList,
        ("artifacts", "artifacts.get_remote") => Operation::ArtifactsGet,
        ("artifacts", "artifacts.list_candidates") => Operation::CandidatesList,
        ("artifacts", "artifacts.intake_candidate") => Operation::CandidatesIntake,
        ("artifacts", "artifacts.follow") => Operation::ArtifactsFollow,
        ("artifacts", "artifacts.fork") => Operation::ArtifactsFork,
        ("artifacts", "artifacts.set_publication") => Operation::ArtifactsSetPublication,
        ("artifacts", "artifacts.set_license") => Operation::ArtifactsSetLicense,
        ("artifacts", "artifacts.search_skills_sh") => Operation::SearchSkillsSh,
        ("artifacts", "artifacts.search_ard") => Operation::SearchArd,
        ("artifacts", "artifacts.search_marketplace") => Operation::SearchMarketplace,
        ("artifacts", "artifacts.list_mcp_registry") => Operation::McpRegistryList,
        ("artifacts", "artifacts.list_acp_registry") => Operation::AcpRegistryList,
        ("artifacts", "artifacts.authority_status") => Operation::AuthorityStatus,
        ("sources", "sources.list") => Operation::SourcesList,
        ("sources", "sources.configure") => Operation::SourcesConfigure,
        ("sources", "sources.delete") => Operation::SourcesDelete,
        ("sources", "sources.refresh") => Operation::SourcesRefresh,
        ("jobs", "jobs.start") => Operation::JobsStart,
        ("jobs", "jobs.list") => Operation::JobsList,
        ("jobs", "jobs.get") => Operation::JobsGet,
        ("jobs", "jobs.cancel") => Operation::JobsCancel,
        ("jobs", "jobs.retry") => Operation::JobsRetry,
        ("uploads", "uploads.create") => Operation::UploadsCreate,
        ("uploads", "uploads.get") => Operation::UploadsGet,
        ("uploads", "uploads.delete") => Operation::UploadsDelete,
        ("bundles", "bundles.list") => Operation::BundlesList,
        ("bundles", "bundles.get") => Operation::BundlesGet,
        ("bundles", "bundles.create") => Operation::BundlesCreate,
        ("bundles", "bundles.add") => Operation::BundlesAddArtifact,
        ("bundles", "bundles.remove") => Operation::BundlesRemoveArtifact,
        ("bundles", "bundles.set_visibility") => Operation::BundlesSetVisibility,
        ("bundles", "bundles.publish") => Operation::BundlesPublish,
        ("bundles", "bundles.delete") => Operation::BundlesDelete,
        _ => return None,
    })
}

pub(crate) fn actions(service: &str) -> &'static [ActionSpec] {
    match service {
        "artifacts" => REMOTE_ARTIFACT_ACTIONS,
        "sources" => SOURCE_ACTIONS,
        "jobs" => JOB_ACTIONS,
        "uploads" => UPLOAD_ACTIONS,
        "bundles" => BUNDLE_ACTIONS,
        _ => &[],
    }
}

pub async fn dispatch_sources(action: &str, params: Value) -> Result<Value, ToolError> {
    dispatch("sources", action, params).await
}
pub async fn dispatch_jobs(action: &str, params: Value) -> Result<Value, ToolError> {
    dispatch("jobs", action, params).await
}
pub async fn dispatch_uploads(action: &str, params: Value) -> Result<Value, ToolError> {
    dispatch("uploads", action, params).await
}
pub async fn dispatch_bundles(action: &str, params: Value) -> Result<Value, ToolError> {
    dispatch("bundles", action, params).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn normalizes_public_names_without_exposing_provider_vocabulary() {
        let mut params = json!({"id":"job-1"}).as_object().unwrap().clone();
        normalize_provider_params("jobs", "jobs.get", &mut params).unwrap();
        assert_eq!(params["jobId"], "job-1");
        assert!(params.get("id").is_none());

        let mut lifecycle = json!({
            "id": "artifact-1",
            "upstream_artifact_id": "source-1",
            "upstream_revision_id": "sha256:abc",
            "review_state": "reviewed",
            "takedown_state": "none",
            "evidence_at": "2026-09-03T00:00:00Z"
        })
        .as_object()
        .unwrap()
        .clone();
        normalize_provider_params("artifacts", "artifacts.set_license", &mut lifecycle).unwrap();
        assert_eq!(lifecycle["artifactId"], "artifact-1");
        assert_eq!(lifecycle["upstreamArtifactId"], "source-1");
        assert_eq!(lifecycle["reviewState"], "reviewed");
        assert!(lifecycle.get("review_state").is_none());
    }

    #[test]
    fn rejects_renderer_supplied_secrets_recursively() {
        let params = json!({"arguments":{"authorization":"Bearer nope"}});
        assert!(reject_secret_values(params.as_object().unwrap()).is_err());
        for key in [
            "accessToken",
            "refresh-token",
            "apiKey",
            "privateKey",
            "session_cookie",
            "dbPassword",
        ] {
            let params = json!({"arguments": {key: "nope"}});
            assert!(
                reject_secret_values(params.as_object().unwrap()).is_err(),
                "{key} must be rejected"
            );
        }
    }

    #[test]
    fn accepts_opaque_pagination_tokens_but_not_credentials() {
        let mut params = json!({"page_token":"opaque-next-page"})
            .as_object()
            .unwrap()
            .clone();
        reject_secret_values(&params).unwrap();
        normalize_provider_params("artifacts", "artifacts.search_ard", &mut params).unwrap();
        assert_eq!(params["pageToken"], "opaque-next-page");
    }

    #[test]
    fn connection_id_must_be_a_non_empty_string() {
        for value in [json!(42), json!("   ")] {
            let mut params = json!({"connection_id": value}).as_object().unwrap().clone();
            assert!(matches!(
                take_connection_id(&mut params),
                Err(ToolError::InvalidParam { ref param, .. }) if param == "connection_id"
            ));
        }
    }

    #[test]
    fn every_published_control_action_has_a_sealed_operation() {
        for (service, specs) in [
            ("artifacts", REMOTE_ARTIFACT_ACTIONS),
            ("sources", SOURCE_ACTIONS),
            ("jobs", JOB_ACTIONS),
            ("uploads", UPLOAD_ACTIONS),
            ("bundles", BUNDLE_ACTIONS),
        ] {
            for spec in specs {
                if spec.name == "artifacts.list_connections" {
                    continue;
                }
                assert!(
                    operation(service, spec.name).is_some(),
                    "{} has no sealed provider operation",
                    spec.name
                );
            }
        }
    }

    #[tokio::test]
    async fn every_remote_operation_fails_closed_without_authorized_context() {
        for (service, action) in [
            ("artifacts", "artifacts.list_connections"),
            ("artifacts", "artifacts.search_remote"),
            ("bundles", "bundles.list"),
            ("jobs", "jobs.start"),
        ] {
            let error = dispatch_with_context(service, action, json!({}), None)
                .await
                .unwrap_err();
            assert!(
                matches!(error, ToolError::Forbidden { .. }),
                "{service}.{action}"
            );
        }
    }
}
