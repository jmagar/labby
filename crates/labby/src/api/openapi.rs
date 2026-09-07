//! OpenAPI 3.1 schema generation for the lab HTTP API.
//!
//! All utoipa coupling is confined to this module. The spec is built
//! programmatically from the `ActionSpec` catalog — no `#[utoipa::path]`
//! annotations on handlers.

use std::sync::Arc;

use serde::{Deserialize, Serialize};
use utoipa::openapi::path::{OperationBuilder, ParameterBuilder, ParameterIn, PathItemBuilder};
use utoipa::openapi::request_body::RequestBodyBuilder;
use utoipa::openapi::schema::SchemaType;
use utoipa::openapi::security::{ApiKey, ApiKeyValue, HttpAuthScheme, HttpBuilder, SecurityScheme};
use utoipa::openapi::{
    Components, ContentBuilder, ObjectBuilder, PathItem, RefOr, Required, Response,
    ResponseBuilder, ResponsesBuilder, Schema, SecurityRequirement, Type,
};
use utoipa::{Modify, OpenApi, ToSchema};

use crate::app_manifest::{APPS_MANIFEST_API_ROUTE, SERVER_LOGS_QUERY_API_ROUTE};
use crate::registry::RegisteredService;

#[derive(Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct AccessBootstrapOwnerRequest {
    /// Display name for the reserved local Organization (1-128 bytes after trimming).
    pub organization_name: String,
    /// Display name for the reserved default Project (1-128 bytes after trimming).
    pub project_name: String,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct AccessBootstrapOwnerResponse {
    /// Redacted outcome: `created` or `already_applied`.
    pub status: String,
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct BootstrapProofConsumeRequest {
    pub version: u8,
    pub installation_id: String,
    pub canonical_issuer: String,
    pub organization_name: String,
    pub project_name: String,
    pub subject: String,
    pub loadout_id: String,
    pub route_id: String,
    pub resource: String,
    pub scopes: Vec<String>,
    pub ttl_seconds: u64,
    pub credential_id: String,
    pub idempotency_key: String,
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct BootstrapPrepareIdRequest {
    pub prepare_id: String,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct BootstrapProofMetadataResponse {
    pub status: String,
    pub prepare_id: String,
    pub credential_id: Option<String>,
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct CredentialIssueRequestDoc {
    pub credential_id: String,
    pub credential_digest_hex: String,
    pub project_id: String,
    pub route_id: String,
    pub resource: String,
    pub audience: String,
    pub scopes: Vec<String>,
    pub expires_at: i64,
    pub idempotency_key: String,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct CredentialMetadataResponseDoc {
    pub status: String,
    pub credential_id: String,
    pub credential_generation: u64,
    pub expires_at: i64,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct CredentialSelfResponseDoc {
    pub status: String,
    pub credential_id: String,
    pub credential_generation: u64,
    pub installation_id: String,
    pub organization_id: String,
    pub project_id: String,
    pub loadout_id: String,
    pub route_id: String,
    pub resource: String,
    pub audience: String,
    pub scopes: Vec<String>,
    pub expires_at: i64,
    pub revocation_generation: u64,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct MutationMetadataResponseDoc {
    pub status: String,
    pub credential_id: Option<String>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct LocalSessionResponseDoc {
    pub csrf_token: String,
    pub expires_at: i64,
}

#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct StashFileDoc {
    pub file_id: String,
    pub uri: String,
    pub display_name: String,
    pub size_bytes: u64,
    pub created_at: i64,
    pub updated_at: i64,
    pub owned: bool,
}

#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct StashPageDoc {
    pub files: Vec<StashFileDoc>,
    pub next_cursor: Option<String>,
}

#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct StashStatsDoc {
    pub owned_file_count: u64,
    pub owned_shared_file_count: u64,
    pub owned_committed_bytes: u64,
    pub owned_reserved_bytes: u64,
}

#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct StashUploadResponseDoc {
    pub file_id: String,
    pub uri: String,
}

#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct StashRenameRequestDoc {
    pub display_name: String,
}

#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct StashGrantRequestDoc {
    pub grantee_principal_id: String,
}

#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct StashGrantDoc {
    pub grant_id: String,
    pub file_id: String,
    pub grantee_principal_id: String,
    pub created_at: i64,
}

#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct StashGrantPageDoc {
    pub grants: Vec<StashGrantDoc>,
    pub next_cursor: Option<String>,
}

#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct StashRecipientQueryDoc {
    pub query: String,
}

#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct StashRecipientDoc {
    pub principal_id: String,
    pub display_name: String,
}

#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct StashRecipientsDoc {
    pub recipients: Vec<StashRecipientDoc>,
}

// ── Documentation-only error schemas ────────────────────────────────────
//
// These mirror the `ToolError` wire format for OpenAPI documentation but
// are NEVER used at runtime. `ToolError` itself must not derive `ToSchema`
// because it has a hand-written `Serialize` impl.

/// Error envelope for `unknown_action` responses.
#[derive(Debug, Serialize, ToSchema)]
pub struct ErrorUnknownAction {
    /// Always `"unknown_action"`.
    pub kind: String,
    /// Human-readable message.
    pub message: String,
    /// Valid action names for this service.
    pub valid: Vec<String>,
    /// Optional fuzzy match suggestion.
    pub hint: Option<String>,
}

/// Error envelope for `missing_param` responses.
#[derive(Debug, Serialize, ToSchema)]
pub struct ErrorMissingParam {
    /// Always `"missing_param"`.
    pub kind: String,
    /// Human-readable message.
    pub message: String,
    /// The missing parameter name.
    pub param: String,
}

/// Error envelope for `invalid_param` responses.
#[derive(Debug, Serialize, ToSchema)]
pub struct ErrorInvalidParam {
    /// Always `"invalid_param"`.
    pub kind: String,
    /// Human-readable message.
    pub message: String,
    /// The invalid parameter name.
    pub param: String,
}

/// Error envelope for `confirmation_required` responses.
#[derive(Debug, Serialize, ToSchema)]
pub struct ErrorConfirmationRequired {
    /// Always `"confirmation_required"`.
    pub kind: String,
    /// Human-readable message.
    pub message: String,
}

/// Structured recovery guidance shared by every agent-facing error response.
#[derive(Debug, Serialize, ToSchema)]
pub struct AgentErrorRecovery {
    /// Recommended next action, such as `revise_and_retry`, `retry_later`,
    /// `reauthenticate`, `rediscover`, or `do_not_retry`.
    pub action: String,
    /// Safety of repeating the exact same request: `safe`, `conditional`,
    /// `discouraged`, or `never`.
    pub same_arguments: String,
    /// Model-readable course-correction instructions.
    pub guidance: String,
    /// Optional retry delay supplied by the failing subsystem.
    pub retry_after_ms: Option<u64>,
}

/// Versioned error contract returned by every HTTP action failure.
#[derive(Debug, Serialize, ToSchema)]
pub struct AgentErrorResponse {
    /// Contract version. Additive version-1 fields may be ignored by clients.
    pub contract_version: u32,
    /// Stable machine-readable error kind.
    pub kind: String,
    /// Standalone model-readable diagnosis.
    pub message: String,
    /// Failure origin such as `validation`, `policy`, `tool_execution`,
    /// `upstream_transport`, `discovery`, or `runtime`.
    pub origin: String,
    /// Recovery action and exact-retry safety.
    pub recovery: AgentErrorRecovery,
    /// Conservative side-effect assessment: `none_expected`, `possible`, or `unknown`.
    pub side_effects: String,
    pub service: Option<String>,
    pub action: Option<String>,
    pub tool: Option<String>,
    pub upstream: Option<String>,
    pub command: Option<String>,
    pub prompt: Option<String>,
    pub resource: Option<String>,
    pub cause: Option<String>,
    pub valid: Option<Vec<String>>,
    pub hint: Option<String>,
    pub param: Option<String>,
    pub required_scopes: Option<Vec<String>>,
    pub existing_id: Option<String>,
}

#[derive(Debug, Serialize, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct CodeModeToolSearchRequest {
    pub query: String,
    #[serde(default = "default_code_mode_tool_search_limit")]
    pub limit: usize,
}

const fn default_code_mode_tool_search_limit() -> usize {
    50
}

#[derive(Debug, Serialize, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct CodeModeToolDescribeRequest {
    pub target: String,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct CodeModeToolSafetyDoc {
    pub read_only: Option<bool>,
    pub destructive: Option<bool>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct CodeModeToolSearchHitDoc {
    pub path: String,
    pub id: String,
    pub kind: String,
    pub namespace: String,
    pub name: String,
    pub description: String,
    pub signature: String,
    pub tags: Vec<String>,
    pub score: u32,
    pub safety: Option<CodeModeToolSafetyDoc>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct CodeModeToolSearchResponseDoc {
    pub results: Vec<CodeModeToolSearchHitDoc>,
    pub total: usize,
    pub truncated: bool,
    pub hint: Option<String>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct CodeModeToolDescribeResponseDoc {
    pub path: String,
    pub id: String,
    pub namespace: String,
    pub name: String,
    pub description: String,
    pub helper: String,
    pub signature: String,
    pub tags: Vec<String>,
    pub safety: Option<CodeModeToolSafetyDoc>,
    pub typescript: Option<String>,
    pub typescript_omitted: Option<String>,
}

/// Compatibility schema name for SDK pass-through errors. Runtime responses use
/// the full `AgentErrorResponse` contract.
pub type ErrorSdk = AgentErrorResponse;

fn agent_error_response(description: &str) -> Response {
    ResponseBuilder::new()
        .description(description)
        .content(
            "application/json",
            ContentBuilder::new()
                .schema(Some(RefOr::Ref(utoipa::openapi::Ref::new(
                    "#/components/schemas/AgentErrorResponse",
                ))))
                .build(),
        )
        .build()
}

// ── Param type → OpenAPI schema conversion ──────────────────────────────

/// Convert a `ParamSpec.ty` string label to an `OpenAPI` `Schema`.
///
/// Handles the 10 known type labels plus unknown fallback:
/// - `"string"`, `"integer"`, `"number"`, `"boolean"`, `"object"`, `"array"`
/// - `"string[]"`, `"integer[]"`, `"SettingsUpdateEntry[]"`
/// - `"string|null"`
/// - Enum literals like `"queued|running|done"` (pipe-separated, no `null`)
/// - Unknown → string fallback
#[must_use]
pub fn param_type_to_schema(ty: &str) -> Schema {
    match ty {
        "string" => ObjectBuilder::new()
            .schema_type(SchemaType::Type(Type::String))
            .build()
            .into(),
        "integer" => ObjectBuilder::new()
            .schema_type(SchemaType::Type(Type::Integer))
            .build()
            .into(),
        "number" => ObjectBuilder::new()
            .schema_type(SchemaType::Type(Type::Number))
            .build()
            .into(),
        "boolean" => ObjectBuilder::new()
            .schema_type(SchemaType::Type(Type::Boolean))
            .build()
            .into(),
        "object" => ObjectBuilder::new()
            .schema_type(SchemaType::Type(Type::Object))
            .build()
            .into(),
        "array" | "string[]" => utoipa::openapi::ArrayBuilder::new()
            .items(ObjectBuilder::new().schema_type(SchemaType::Type(Type::String)))
            .build()
            .into(),
        "integer[]" => utoipa::openapi::ArrayBuilder::new()
            .items(ObjectBuilder::new().schema_type(SchemaType::Type(Type::Integer)))
            .build()
            .into(),
        "SettingsUpdateEntry[]" => utoipa::openapi::ArrayBuilder::new()
            .items(settings_update_entry_schema())
            .build()
            .into(),
        "string|null" => utoipa::openapi::schema::AnyOfBuilder::new()
            .item(
                ObjectBuilder::new()
                    .schema_type(SchemaType::Type(Type::String))
                    .build(),
            )
            .item(
                ObjectBuilder::new()
                    .schema_type(SchemaType::Type(Type::Null))
                    .build(),
            )
            .build()
            .into(),
        other if other.contains('|') => {
            // Pipe-separated enum: "queued|running|done"
            let variants: Vec<serde_json::Value> = other
                .split('|')
                .map(|s| serde_json::Value::String(s.to_string()))
                .collect();
            ObjectBuilder::new()
                .schema_type(SchemaType::Type(Type::String))
                .enum_values(Some(variants))
                .build()
                .into()
        }
        // Unknown type label → string fallback
        _ => ObjectBuilder::new()
            .schema_type(SchemaType::Type(Type::String))
            .build()
            .into(),
    }
}

fn settings_update_entry_schema() -> ObjectBuilder {
    ObjectBuilder::new()
        .schema_type(SchemaType::Type(Type::Object))
        .property(
            "key",
            ObjectBuilder::new().schema_type(SchemaType::Type(Type::String)),
        )
        .required("key")
        .property("value", settings_update_value_schema())
        .required("value")
        .property("previous", settings_update_value_schema())
        .required("previous")
        .property(
            "unset",
            ObjectBuilder::new().schema_type(SchemaType::Type(Type::Boolean)),
        )
}

fn settings_update_value_schema() -> Schema {
    utoipa::openapi::schema::AnyOfBuilder::new()
        .item(
            ObjectBuilder::new()
                .schema_type(SchemaType::Type(Type::String))
                .build(),
        )
        .item(
            ObjectBuilder::new()
                .schema_type(SchemaType::Type(Type::Integer))
                .build(),
        )
        .item(
            ObjectBuilder::new()
                .schema_type(SchemaType::Type(Type::Number))
                .build(),
        )
        .item(
            ObjectBuilder::new()
                .schema_type(SchemaType::Type(Type::Boolean))
                .build(),
        )
        .item(
            ObjectBuilder::new()
                .schema_type(SchemaType::Type(Type::Array))
                .build(),
        )
        .item(
            ObjectBuilder::new()
                .schema_type(SchemaType::Type(Type::Object))
                .build(),
        )
        .item(
            ObjectBuilder::new()
                .schema_type(SchemaType::Type(Type::Null))
                .build(),
        )
        .build()
        .into()
}

// ── PascalCase conversion ───────────────────────────────────────────────

/// Convert a dotted action name to `PascalCase` for schema naming.
///
/// `"status.get"` → `"StatusGet"`, `"health.list"` → `"HealthList"`
#[must_use]
pub fn to_pascal_case(dotted: &str) -> String {
    dotted
        .split('.')
        .map(|seg| {
            let mut chars = seg.chars();
            chars.next().map_or_else(String::new, |c| {
                let mut s = c.to_uppercase().to_string();
                s.extend(chars);
                s
            })
        })
        .collect()
}

// ── Action schema generation ────────────────────────────────────────────

/// Build named schemas for each service's actions.
///
/// Returns `(name, Schema)` pairs suitable for injection into `OpenAPI` components.
/// Names follow the pattern `{Service}{Action}Params` — e.g., `GatewayListParams`.
#[must_use]
pub fn build_action_schemas(services: &[RegisteredService]) -> Vec<(String, RefOr<Schema>)> {
    let mut schemas = Vec::new();
    for svc in services {
        let svc_pascal = to_pascal_case(svc.name);
        let action_names = svc
            .actions
            .iter()
            .map(|action| serde_json::Value::String(action.name.to_string()))
            .collect::<Vec<_>>();
        let request = ObjectBuilder::new()
            .property(
                "action",
                ObjectBuilder::new()
                    .schema_type(SchemaType::Type(Type::String))
                    .enum_values(Some(action_names)),
            )
            .required("action")
            .property(
                "params",
                ObjectBuilder::new().schema_type(SchemaType::Type(Type::Object)),
            )
            .build();
        schemas.push((
            format!("{svc_pascal}ActionRequest"),
            RefOr::T(request.into()),
        ));
        for action in svc.actions {
            if action.params.is_empty() {
                continue;
            }
            let action_pascal = to_pascal_case(action.name);
            let name = format!("{svc_pascal}{action_pascal}Params");

            let mut builder = ObjectBuilder::new();
            for p in action.params {
                builder = builder.property(p.name, param_type_to_schema(p.ty));
                if p.required {
                    builder = builder.required(p.name);
                }
            }
            schemas.push((name, RefOr::T(builder.build().into())));
        }
    }
    schemas
}

// ── utoipa::Modify implementations ──────────────────────────────────────

/// Injects all action parameter schemas into the `OpenAPI` components.
pub struct ActionSchemaInjector {
    schemas: Vec<(String, RefOr<Schema>)>,
}

impl ActionSchemaInjector {
    #[must_use]
    pub fn new(services: &[RegisteredService]) -> Self {
        Self {
            schemas: build_action_schemas(services),
        }
    }
}

impl Modify for ActionSchemaInjector {
    fn modify(&self, openapi: &mut utoipa::openapi::OpenApi) {
        let components = openapi.components.get_or_insert_with(Components::default);
        for (name, schema) in &self.schemas {
            components.schemas.insert(name.clone(), schema.clone());
        }
    }
}

/// Adds Bearer auth security scheme to the `OpenAPI` spec.
pub struct SecurityAddon;

impl Modify for SecurityAddon {
    fn modify(&self, openapi: &mut utoipa::openapi::OpenApi) {
        let components = openapi.components.get_or_insert_with(Components::default);
        components.security_schemes.insert(
            "bearer_auth".to_string(),
            SecurityScheme::Http(
                HttpBuilder::new()
                    .scheme(HttpAuthScheme::Bearer)
                    .bearer_format("token")
                    .build(),
            ),
        );
        components.security_schemes.insert(
            "browser_session".to_string(),
            SecurityScheme::ApiKey(ApiKey::Cookie(ApiKeyValue::new("lab_session"))),
        );
        components.security_schemes.insert(
            "LabbyBootstrapProof".to_string(),
            SecurityScheme::ApiKey(ApiKey::Header(ApiKeyValue::new("X-Labby-Bootstrap-Proof"))),
        );
    }
}

// ── Path builders ───────────────────────────────────────────────────────

/// Build `OpenAPI` paths for health endpoints.
#[must_use]
pub fn build_health_paths() -> Vec<(String, PathItem)> {
    let health_response = ResponseBuilder::new()
        .description("Service is alive")
        .content(
            "application/json",
            ContentBuilder::new()
                .schema(Some(RefOr::Ref(utoipa::openapi::Ref::new(
                    "#/components/schemas/HealthResponse",
                ))))
                .build(),
        )
        .build();

    let ready_response = ResponseBuilder::new()
        .description("Service is ready to serve traffic")
        .content(
            "application/json",
            ContentBuilder::new()
                .schema(Some(RefOr::Ref(utoipa::openapi::Ref::new(
                    "#/components/schemas/HealthResponse",
                ))))
                .build(),
        )
        .build();

    vec![
        (
            "/health".to_string(),
            PathItemBuilder::new()
                .operation(
                    utoipa::openapi::HttpMethod::Get,
                    OperationBuilder::new()
                        .tag("health")
                        .summary(Some("Liveness probe"))
                        .description(Some("Returns 200 as long as the process is running."))
                        .responses(
                            ResponsesBuilder::new()
                                .response("200", health_response)
                                .build(),
                        )
                        .build(),
                )
                .build(),
        ),
        (
            "/ready".to_string(),
            PathItemBuilder::new()
                .operation(
                    utoipa::openapi::HttpMethod::Get,
                    OperationBuilder::new()
                        .tag("health")
                        .summary(Some("Readiness probe"))
                        .description(Some(
                            "Returns 200 once app state is fully constructed, 503 otherwise.",
                        ))
                        .responses(
                            ResponsesBuilder::new()
                                .response("200", ready_response)
                                .response(
                                    "503",
                                    ResponseBuilder::new()
                                        .description("Service not ready")
                                        .build(),
                                )
                                .build(),
                        )
                        .build(),
                )
                .build(),
        ),
    ]
}

/// Build `OpenAPI` paths for all service endpoints.
///
/// Each service gets `POST /v1/{service}` with the `ActionRequest` body schema.
#[must_use]
pub fn build_service_paths(service_names: &[String]) -> Vec<(String, PathItem)> {
    let paths = service_names
        .iter()
        .map(|svc| {
            let path = format!("/v1/{svc}");
            let operation = OperationBuilder::new()
                .tag(svc)
                .summary(Some(format!("Dispatch action to {svc}")))
                .description(Some(format!(
                    "Execute an action on the {svc} service. Use `action: \"help\"` to list available actions. Actions whose schema reports `requires_admin: true` require the `lab:admin` scope. Every non-2xx response uses AgentErrorResponse; follow `recovery.guidance` and do not repeat requests unchanged when `side_effects` is `possible` or `unknown`."
                )))
                .request_body(Some(
                    RequestBodyBuilder::new()
                        .content(
                            "application/json",
                            ContentBuilder::new()
                                .schema(Some(RefOr::Ref(utoipa::openapi::Ref::new(format!(
                                    "#/components/schemas/{}ActionRequest",
                                    to_pascal_case(svc)
                                )))))
                                .build(),
                        )
                        .required(Some(Required::True))
                        .build(),
                ))
                .responses(
                    ResponsesBuilder::new()
                        .response(
                            "200",
                            ResponseBuilder::new()
                                .description("Successful action response")
                                .content(
                                    "application/json",
                                    ContentBuilder::new()
                                        .schema(Some(RefOr::T(
                                            ObjectBuilder::new()
                                                .schema_type(SchemaType::Type(Type::Object))
                                                .build()
                                                .into(),
                                        )))
                                        .build(),
                                )
                                .build(),
                        )
                        .response(
                            "400",
                            agent_error_response("Unknown action or malformed request"),
                        )
                        .response(
                            "401",
                            agent_error_response("Authentication is missing, invalid, or must be renewed"),
                        )
                        .response(
                            "403",
                            agent_error_response("Authenticated caller lacks required scope or policy permission"),
                        )
                        .response(
                            "404",
                            agent_error_response("Requested service, action target, or upstream was not found"),
                        )
                        .response(
                            "409",
                            agent_error_response("Conflict, ambiguity, stale state, or restart required"),
                        )
                        .response(
                            "413",
                            agent_error_response("Request or generated content exceeds the configured limit"),
                        )
                        .response(
                            "422",
                            agent_error_response("Validation, confirmation, path-safety, or tool-execution error"),
                        )
                        .response(
                            "429",
                            agent_error_response("Rate limit or queue saturation; honor recovery.retry_after_ms when present"),
                        )
                        .response(
                            "500",
                            agent_error_response("Internal failure requiring diagnostic inspection"),
                        )
                        .response(
                            "502",
                            agent_error_response("Upstream gateway, provider, OAuth-resource, or transport failure"),
                        )
                        .response(
                            "503",
                            agent_error_response("Service or provider temporarily unavailable"),
                        )
                        .response(
                            "504",
                            agent_error_response("Operation timed out; use recovery guidance before retrying"),
                        )
                        .build(),
                )
                .security(SecurityRequirement::new::<&str, [&str; 0], &str>(
                    "bearer_auth",
                    [],
                ))
                .build();

            let item = PathItemBuilder::new()
                .operation(utoipa::openapi::HttpMethod::Post, operation)
                .build();
            (path, item)
        })
        .collect::<Vec<_>>();

    paths
}

/// Build the dedicated streaming and browser-facing File Stash routes.
#[must_use]
pub fn build_stash_paths() -> Vec<(String, PathItem)> {
    use utoipa::openapi::HttpMethod;

    let response = |description: &str, schema: &str| {
        ResponseBuilder::new()
            .description(description)
            .content(
                "application/json",
                ContentBuilder::new()
                    .schema(Some(RefOr::Ref(utoipa::openapi::Ref::new(schema))))
                    .build(),
            )
            .build()
    };
    let responses = |success_code: &str, description: &str, schema: Option<&str>| {
        let success = schema.map_or_else(
            || ResponseBuilder::new().description(description).build(),
            |schema| response(description, schema),
        );
        ResponsesBuilder::new()
            .response(success_code, success)
            .response("400", agent_error_response("Malformed File Stash request"))
            .response(
                "401",
                agent_error_response("Authentication is missing or invalid"),
            )
            .response("404", agent_error_response("File or grant was not found"))
            .response(
                "409",
                agent_error_response("Filename or grant conflicts with existing state"),
            )
            .response(
                "413",
                agent_error_response("File or request exceeds a configured limit"),
            )
            .response("422", agent_error_response("File Stash validation failed"))
            .response("429", agent_error_response("File Stash capacity is busy"))
            .response("503", agent_error_response("File Stash is unavailable"))
            .build()
    };
    let security = || SecurityRequirement::new::<&str, [&str; 0], &str>("bearer_auth", []);
    let query_parameter = |name: &'static str, description: &'static str| {
        ParameterBuilder::new()
            .name(name)
            .parameter_in(ParameterIn::Query)
            .required(Required::False)
            .description(Some(description))
            .schema(Some(param_type_to_schema("string")))
            .build()
    };
    let path_parameter = |name: &'static str| {
        ParameterBuilder::new()
            .name(name)
            .parameter_in(ParameterIn::Path)
            .required(Required::True)
            .schema(Some(param_type_to_schema("string")))
            .build()
    };
    let json_body = |schema: &'static str| {
        RequestBodyBuilder::new()
            .content(
                "application/json",
                ContentBuilder::new()
                    .schema(Some(RefOr::Ref(utoipa::openapi::Ref::new(schema))))
                    .build(),
            )
            .required(Some(Required::True))
            .build()
    };
    let operation = |method: HttpMethod,
                     summary: &'static str,
                     success_code: &'static str,
                     success_description: &'static str,
                     success_schema: Option<&'static str>,
                     parameters: Vec<utoipa::openapi::path::Parameter>,
                     body: Option<utoipa::openapi::request_body::RequestBody>| {
        let operation = OperationBuilder::new()
            .tag("stash")
            .summary(Some(summary))
            .parameters(Some(parameters))
            .request_body(body)
            .responses(responses(success_code, success_description, success_schema))
            .security(security())
            .build();
        PathItemBuilder::new().operation(method, operation).build()
    };

    let mut root = build_service_paths(&["stash".to_owned()])
        .pop()
        .expect("stash service path")
        .1;
    root.get = operation(
        HttpMethod::Get,
        "List or search File Stash files",
        "200",
        "Caller-authorized file page",
        Some("#/components/schemas/StashPageDoc"),
        vec![
            query_parameter("cursor", "Opaque page cursor"),
            query_parameter("query", "Page-local filename substring filter"),
            ParameterBuilder::new()
                .name("limit")
                .parameter_in(ParameterIn::Query)
                .required(Required::False)
                .schema(Some(param_type_to_schema("integer")))
                .build(),
        ],
        None,
    )
    .get;

    let file_path = || vec![path_parameter("file_id")];
    vec![
        ("/v1/stash".to_owned(), root),
        (
            "/v1/stash/stats".to_owned(),
            operation(
                HttpMethod::Get,
                "Read File Stash usage",
                "200",
                "Owned-file usage",
                Some("#/components/schemas/StashStatsDoc"),
                vec![],
                None,
            ),
        ),
        (
            "/v1/stash/recipients".to_owned(),
            operation(
                HttpMethod::Post,
                "Search eligible grant recipients",
                "200",
                "Recipient matches",
                Some("#/components/schemas/StashRecipientsDoc"),
                vec![],
                Some(json_body("#/components/schemas/StashRecipientQueryDoc")),
            ),
        ),
        (
            "/v1/stash/uploads".to_owned(),
            operation(
                HttpMethod::Post,
                "Upload a File Stash object",
                "201",
                "Created file identity",
                Some("#/components/schemas/StashUploadResponseDoc"),
                vec![
                    ParameterBuilder::new()
                        .name("Content-Length")
                        .parameter_in(ParameterIn::Header)
                        .required(Required::True)
                        .description(Some("Exact raw body length in bytes"))
                        .schema(Some(param_type_to_schema("integer")))
                        .build(),
                    ParameterBuilder::new()
                        .name("X-Labby-Stash-Filename")
                        .parameter_in(ParameterIn::Header)
                        .required(Required::True)
                        .description(Some("Percent-encoded display filename"))
                        .schema(Some(param_type_to_schema("string")))
                        .build(),
                    ParameterBuilder::new()
                        .name("X-CSRF-Token")
                        .parameter_in(ParameterIn::Header)
                        .required(Required::False)
                        .description(Some("Required for cookie-authenticated mutations"))
                        .schema(Some(param_type_to_schema("string")))
                        .build(),
                ],
                Some(
                    RequestBodyBuilder::new()
                        .content(
                            "application/octet-stream",
                            ContentBuilder::new()
                                .schema(Some(RefOr::T(param_type_to_schema("string"))))
                                .build(),
                        )
                        .required(Some(Required::True))
                        .build(),
                ),
            ),
        ),
        ("/v1/stash/files/{file_id}".to_owned(), {
            let mut item = operation(
                HttpMethod::Get,
                "Read File Stash metadata",
                "200",
                "File metadata",
                Some("#/components/schemas/StashFileDoc"),
                file_path(),
                None,
            );
            item.patch = operation(
                HttpMethod::Patch,
                "Rename an owned File Stash object",
                "200",
                "Renamed file metadata",
                Some("#/components/schemas/StashFileDoc"),
                file_path(),
                Some(json_body("#/components/schemas/StashRenameRequestDoc")),
            )
            .patch;
            item.delete = operation(
                HttpMethod::Delete,
                "Delete an owned File Stash object",
                "204",
                "File deleted",
                None,
                file_path(),
                None,
            )
            .delete;
            item
        }),
        ("/v1/stash/files/{file_id}/content".to_owned(), {
            let mut item = operation(
                HttpMethod::Get,
                "Download File Stash content",
                "200",
                "Raw file bytes",
                None,
                file_path(),
                None,
            );
            if let Some(get) = item.get.as_mut() {
                get.responses.responses.insert(
                    "200".to_owned(),
                    RefOr::T(
                        ResponseBuilder::new()
                            .description(
                                "Raw file bytes with private no-store and attachment headers",
                            )
                            .content(
                                "application/octet-stream",
                                ContentBuilder::new()
                                    .schema(Some(RefOr::T(param_type_to_schema("string"))))
                                    .build(),
                            )
                            .build(),
                    ),
                );
            }
            item
        }),
        ("/v1/stash/files/{file_id}/grants".to_owned(), {
            let mut item = operation(
                HttpMethod::Get,
                "List active File Stash grants",
                "200",
                "Grant page",
                Some("#/components/schemas/StashGrantPageDoc"),
                vec![
                    path_parameter("file_id"),
                    query_parameter("cursor", "Opaque grant page cursor"),
                ],
                None,
            );
            item.post = operation(
                HttpMethod::Post,
                "Grant read access to a File Stash object",
                "201",
                "Created read grant",
                Some("#/components/schemas/StashGrantDoc"),
                file_path(),
                Some(json_body("#/components/schemas/StashGrantRequestDoc")),
            )
            .post;
            item
        }),
        (
            "/v1/stash/files/{file_id}/grants/{grant_id}".to_owned(),
            operation(
                HttpMethod::Delete,
                "Revoke a File Stash grant",
                "204",
                "Grant revoked",
                None,
                vec![path_parameter("file_id"), path_parameter("grant_id")],
                None,
            ),
        ),
    ]
}

#[must_use]
pub fn build_app_paths() -> Vec<(String, PathItem)> {
    let generic_json = || {
        ContentBuilder::new()
            .schema(Some(RefOr::T(
                ObjectBuilder::new()
                    .schema_type(SchemaType::Type(Type::Object))
                    .build()
                    .into(),
            )))
            .build()
    };
    let ok_response = |description: &str| {
        ResponseBuilder::new()
            .description(description)
            .content("application/json", generic_json())
            .build()
    };
    let auth_response = || {
        ResponseBuilder::new()
            .description("Authentication failed")
            .content(
                "application/json",
                ContentBuilder::new()
                    .schema(Some(RefOr::Ref(utoipa::openapi::Ref::new(
                        "#/components/schemas/AgentErrorResponse",
                    ))))
                    .build(),
            )
            .build()
    };
    let manifest = OperationBuilder::new()
        .tag("apps")
        .summary(Some("List Labby operator apps"))
        .description(Some(
            "Returns app metadata resolved against the live ActionSpec registry.",
        ))
        .responses(
            ResponsesBuilder::new()
                .response("200", ok_response("Operator app manifest"))
                .response("401", auth_response())
                .build(),
        )
        .security(SecurityRequirement::new::<&str, [&str; 0], &str>(
            "bearer_auth",
            [],
        ))
        .build();
    let server_logs_query = OperationBuilder::new()
        .tag("apps")
        .summary(Some("Query Labby server process logs"))
        .description(Some(
            "Browser-friendly data route for the Server Logs operator app. Mirrors `server_logs.query`.",
        ))
        .parameters(Some(server_logs_query_parameters()))
        .responses(
            ResponsesBuilder::new()
                .response("200", ok_response("Server log query result"))
                .response("401", auth_response())
                .response(
                    "403",
                    ResponseBuilder::new()
                        .description("Admin scope required")
                        .content(
                            "application/json",
                            ContentBuilder::new()
                                .schema(Some(RefOr::Ref(utoipa::openapi::Ref::new(
                                    "#/components/schemas/AgentErrorResponse",
                                ))))
                                .build(),
                        )
                        .build(),
                )
                .build(),
        )
        .security(SecurityRequirement::new::<&str, [&str; 0], &str>(
            "bearer_auth",
            [],
        ))
        .build();
    let admin_tool_operation =
        |summary: &'static str, request_schema: &'static str, response_schema: &'static str| {
            OperationBuilder::new()
                .tag("gateway")
                .summary(Some(summary))
                .description(Some(
                    "Private admin browser projection of the live Code Mode catalog.",
                ))
                .request_body(Some(
                    RequestBodyBuilder::new()
                        .content(
                            "application/json",
                            ContentBuilder::new()
                                .schema(Some(RefOr::Ref(utoipa::openapi::Ref::new(request_schema))))
                                .build(),
                        )
                        .required(Some(Required::True))
                        .build(),
                ))
                .responses(
                    ResponsesBuilder::new()
                        .response(
                            "200",
                            ResponseBuilder::new()
                                .description("Code Mode tool discovery result")
                                .content(
                                    "application/json",
                                    ContentBuilder::new()
                                        .schema(Some(RefOr::Ref(utoipa::openapi::Ref::new(
                                            response_schema,
                                        ))))
                                        .build(),
                                )
                                .build(),
                        )
                        .response("401", auth_response())
                        .response("403", agent_error_response("Admin scope required"))
                        .response("404", agent_error_response("Tool not found"))
                        .response(
                            "413",
                            agent_error_response("Response exceeds the bounded payload limit"),
                        )
                        .response("422", agent_error_response("Invalid request"))
                        .response("500", agent_error_response("Catalog discovery failed"))
                        .build(),
                )
                .security(SecurityRequirement::new::<&str, [&str; 0], &str>(
                    "bearer_auth",
                    [],
                ))
                .build()
        };

    vec![
        (
            APPS_MANIFEST_API_ROUTE.to_string(),
            PathItemBuilder::new()
                .operation(utoipa::openapi::HttpMethod::Get, manifest)
                .build(),
        ),
        (
            SERVER_LOGS_QUERY_API_ROUTE.to_string(),
            PathItemBuilder::new()
                .operation(utoipa::openapi::HttpMethod::Get, server_logs_query)
                .build(),
        ),
        (
            "/v1/gateway/codemode/tools/search".to_string(),
            PathItemBuilder::new()
                .operation(
                    utoipa::openapi::HttpMethod::Post,
                    admin_tool_operation(
                        "Search live Code Mode tools",
                        "#/components/schemas/CodeModeToolSearchRequest",
                        "#/components/schemas/CodeModeToolSearchResponseDoc",
                    ),
                )
                .build(),
        ),
        (
            "/v1/gateway/codemode/tools/describe".to_string(),
            PathItemBuilder::new()
                .operation(
                    utoipa::openapi::HttpMethod::Post,
                    admin_tool_operation(
                        "Describe a live Code Mode tool",
                        "#/components/schemas/CodeModeToolDescribeRequest",
                        "#/components/schemas/CodeModeToolDescribeResponseDoc",
                    ),
                )
                .build(),
        ),
    ]
}

#[must_use]
pub fn build_access_paths() -> Vec<(String, PathItem)> {
    let response = |description: &str, schema: &str| {
        ResponseBuilder::new()
            .description(description)
            .content(
                "application/json",
                ContentBuilder::new()
                    .schema(Some(RefOr::Ref(utoipa::openapi::Ref::new(schema))))
                    .build(),
            )
            .build()
    };
    let operation = OperationBuilder::new()
        .tag("access")
        .summary(Some("Explicitly bootstrap the access-control owner"))
        .description(Some(
            "OAuth-browser-only, one-time owner bootstrap. The /v1 middleware must supply an authenticated browser session, valid CSRF token, and canonical VerifiedIdentity; the handler additionally requires lab:admin and an email matching LABBY_AUTH_ADMIN_EMAIL. Bearer automation, MCP, CLI, stdio, local credentials, and loopback origin do not bypass these gates. Success reveals only created or already_applied and responses are private, no-store.",
        ))
        .parameter(
            ParameterBuilder::new()
                .name("x-csrf-token")
                .parameter_in(ParameterIn::Header)
                .required(Required::True)
                .description(Some("CSRF token issued with the authenticated browser session."))
                .schema(Some(RefOr::T(param_type_to_schema("string"))))
                .build(),
        )
        .request_body(Some(
            RequestBodyBuilder::new()
                .content(
                    "application/json",
                    ContentBuilder::new()
                        .schema(Some(RefOr::Ref(utoipa::openapi::Ref::new(
                            "#/components/schemas/AccessBootstrapOwnerRequest",
                        ))))
                        .build(),
                )
                .required(Some(Required::True))
                .build(),
        ))
        .responses(
            ResponsesBuilder::new()
                .response("200", response("Bootstrap was already applied", "#/components/schemas/AccessBootstrapOwnerResponse"))
                .response("201", response("Bootstrap created the owner state", "#/components/schemas/AccessBootstrapOwnerResponse"))
                .response("401", agent_error_response("Browser session is missing or invalid"))
                .response("403", agent_error_response("Browser-admin identity gate failed"))
                .response(
                    "404",
                    ResponseBuilder::new()
                        .description("Route is not mounted when OAuth browser mode is unavailable")
                        .build(),
                )
                .response("409", agent_error_response("Bootstrap conflicts with existing state"))
                .response(
                    "422",
                    ResponseBuilder::new()
                        .description("CSRF, JSON body, Organization name, or Project name is invalid; CSRF rejection uses the auth middleware error envelope")
                        .build(),
                )
                .response("503", agent_error_response("Access store is busy, unavailable, or failed integrity validation"))
                .build(),
        )
        .security(SecurityRequirement::new::<&str, [&str; 0], &str>(
            "browser_session",
            [],
        ))
        .build();
    vec![(
        "/v1/access/bootstrap-owner".to_string(),
        PathItemBuilder::new()
            .operation(utoipa::openapi::HttpMethod::Post, operation)
            .build(),
    )]
}

fn private_json_response(description: &str, schema: &str) -> Response {
    ResponseBuilder::new()
        .description(description)
        .content(
            "application/json",
            ContentBuilder::new()
                .schema(Some(RefOr::Ref(utoipa::openapi::Ref::new(schema))))
                .build(),
        )
        .build()
}

fn bootstrap_proof_operation(
    summary: &'static str,
    request_schema: &'static str,
) -> utoipa::openapi::path::Operation {
    OperationBuilder::new()
        .tag("access")
        .summary(Some(summary))
        .description(Some(
            "Direct-local proof-authenticated operation. Unknown, malformed, expired, or mismatched proofs receive one uniform non-enumerating denial. Responses are private, no-store and no-referrer.",
        ))
        .request_body(Some(
            RequestBodyBuilder::new()
                .content(
                    "application/json",
                    ContentBuilder::new()
                        .schema(Some(RefOr::Ref(utoipa::openapi::Ref::new(
                            request_schema,
                        ))))
                        .build(),
                )
                .required(Some(Required::True))
                .build(),
        ))
        .responses(
            ResponsesBuilder::new()
                .response(
                    "200",
                    private_json_response(
                        "Metadata-only operation outcome",
                        "#/components/schemas/BootstrapProofMetadataResponse",
                    ),
                )
                .response("403", agent_error_response("Uniform bootstrap proof denial"))
                .response("503", agent_error_response("Bootstrap service unavailable"))
                .build(),
        )
        .security(SecurityRequirement::new::<&str, [&str; 0], &str>(
            "LabbyBootstrapProof",
            [],
        ))
        .build()
}

#[must_use]
pub fn build_project_credential_paths() -> Vec<(String, PathItem)> {
    let bearer = || SecurityRequirement::new::<&str, [&str; 0], &str>("bearer_auth", []);
    let credential_response = private_json_response(
        "Credential metadata only; plaintext credential secrets are never returned",
        "#/components/schemas/CredentialMetadataResponseDoc",
    );
    vec![
        (
            "/auth/bootstrap/consume".into(),
            PathItemBuilder::new()
                .operation(
                    utoipa::openapi::HttpMethod::Post,
                    bootstrap_proof_operation(
                        "Consume an offline-prepared bootstrap proof",
                        "#/components/schemas/BootstrapProofConsumeRequest",
                    ),
                )
                .build(),
        ),
        (
            "/auth/bootstrap/status".into(),
            PathItemBuilder::new()
                .operation(
                    utoipa::openapi::HttpMethod::Post,
                    bootstrap_proof_operation(
                        "Read bootstrap prepare status",
                        "#/components/schemas/BootstrapPrepareIdRequest",
                    ),
                )
                .build(),
        ),
        (
            "/auth/bootstrap/cleanup".into(),
            PathItemBuilder::new()
                .operation(
                    utoipa::openapi::HttpMethod::Post,
                    bootstrap_proof_operation(
                        "Tombstone and clean an exact bootstrap prepare",
                        "#/components/schemas/BootstrapPrepareIdRequest",
                    ),
                )
                .build(),
        ),
        (
            "/v1/access/credentials".into(),
            PathItemBuilder::new()
                .operation(
                    utoipa::openapi::HttpMethod::Post,
                    OperationBuilder::new()
                        .tag("access")
                        .summary(Some("Issue a narrower project credential"))
                        .request_body(Some(
                            RequestBodyBuilder::new()
                                .content(
                                    "application/json",
                                    ContentBuilder::new()
                                        .schema(Some(RefOr::Ref(utoipa::openapi::Ref::new(
                                            "#/components/schemas/CredentialIssueRequestDoc",
                                        ))))
                                        .build(),
                                )
                                .required(Some(Required::True))
                                .build(),
                        ))
                        .response("201", credential_response.clone())
                        .security(bearer())
                        .build(),
                )
                .build(),
        ),
        (
            "/v1/access/credentials/self".into(),
            PathItemBuilder::new()
                .operation(
                    utoipa::openapi::HttpMethod::Get,
                    OperationBuilder::new()
                        .tag("access")
                        .summary(Some("Introspect the exact source credential"))
                        .response(
                            "200",
                            private_json_response(
                                "Exact active credential binding",
                                "#/components/schemas/CredentialSelfResponseDoc",
                            ),
                        )
                        .security(bearer())
                        .build(),
                )
                .build(),
        ),
        (
            "/v1/access/credentials/{credential_id}".into(),
            PathItemBuilder::new()
                .operation(
                    utoipa::openapi::HttpMethod::Delete,
                    OperationBuilder::new()
                        .tag("access")
                        .summary(Some("Revoke a project credential"))
                        .response(
                            "200",
                            private_json_response(
                                "Idempotent revocation outcome",
                                "#/components/schemas/MutationMetadataResponseDoc",
                            ),
                        )
                        .security(bearer())
                        .build(),
                )
                .build(),
        ),
        (
            "/auth/local-session".into(),
            PathItemBuilder::new()
                .operation(
                    utoipa::openapi::HttpMethod::Post,
                    OperationBuilder::new()
                        .tag("access")
                        .summary(Some("Create a source-bound local browser session"))
                        .response(
                            "201",
                            private_json_response(
                                "Source-bound browser session metadata",
                                "#/components/schemas/LocalSessionResponseDoc",
                            ),
                        )
                        .security(bearer())
                        .build(),
                )
                .operation(
                    utoipa::openapi::HttpMethod::Delete,
                    OperationBuilder::new()
                        .tag("access")
                        .summary(Some("Revoke the current local browser session"))
                        .response(
                            "204",
                            ResponseBuilder::new()
                                .description("Browser session revoked and cookie cleared")
                                .build(),
                        )
                        .security(SecurityRequirement::new::<&str, [&str; 0], &str>(
                            "browser_session",
                            [],
                        ))
                        .build(),
                )
                .build(),
        ),
    ]
}

fn server_logs_query_parameters() -> Vec<utoipa::openapi::path::Parameter> {
    [
        (
            "limit",
            "integer",
            "Maximum number of log entries to return.",
        ),
        ("level", "string", "Filter by log level."),
        ("target", "string", "Filter by tracing target."),
        ("service", "string", "Filter by service field."),
        ("action", "string", "Filter by action field."),
        ("kind", "string", "Filter by error/event kind field."),
        ("query", "string", "Case-insensitive text search."),
        ("file", "string", "Filter by log file name."),
        (
            "max_scan_bytes",
            "integer",
            "Maximum number of log bytes to scan.",
        ),
    ]
    .into_iter()
    .map(|(name, ty, description)| {
        ParameterBuilder::new()
            .name(name)
            .parameter_in(ParameterIn::Query)
            .required(Required::False)
            .description(Some(description))
            .schema(Some(param_type_to_schema(ty)))
            .build()
    })
    .collect()
}

// ── Top-level spec builder ──────────────────────────────────────────────

/// The `OpenApi` derive target. Component schemas are registered here;
/// paths are injected programmatically.
#[derive(OpenApi)]
#[openapi(
    info(
        title = "lab API",
        version = "0.3.2",
        description = "Homelab service orchestration API"
    ),
    components(schemas(
        super::ActionRequest,
        super::health::HealthResponse,
        ErrorUnknownAction,
        ErrorMissingParam,
        ErrorInvalidParam,
        ErrorConfirmationRequired,
        AgentErrorRecovery,
        AgentErrorResponse,
        CodeModeToolSearchRequest,
        CodeModeToolDescribeRequest,
        CodeModeToolSafetyDoc,
        CodeModeToolSearchHitDoc,
        CodeModeToolSearchResponseDoc,
        CodeModeToolDescribeResponseDoc,
        AccessBootstrapOwnerRequest,
        AccessBootstrapOwnerResponse,
        BootstrapProofConsumeRequest,
        BootstrapPrepareIdRequest,
        BootstrapProofMetadataResponse,
        CredentialIssueRequestDoc,
        CredentialMetadataResponseDoc,
        CredentialSelfResponseDoc,
        MutationMetadataResponseDoc,
        LocalSessionResponseDoc,
        StashFileDoc,
        StashPageDoc,
        StashStatsDoc,
        StashUploadResponseDoc,
        StashRenameRequestDoc,
        StashGrantRequestDoc,
        StashGrantDoc,
        StashGrantPageDoc,
        StashRecipientQueryDoc,
        StashRecipientDoc,
        StashRecipientsDoc,
    )),
    modifiers(&SecurityAddon),
)]
struct ApiDoc;

/// Build the complete `OpenAPI` 3.1 JSON spec.
///
/// Pure function — called once at startup, result wrapped in `Arc<String>`.
///
/// # Errors
///
/// Returns `Err` if JSON serialization fails (should never happen).
pub fn build_openapi_spec(
    services: &[RegisteredService],
) -> Result<Arc<String>, serde_json::Error> {
    let service_names: Vec<String> = services.iter().map(|s| s.name.to_string()).collect();

    let injector = ActionSchemaInjector::new(services);

    let mut spec = ApiDoc::openapi();

    // Apply modifiers
    injector.modify(&mut spec);

    // Inject programmatic paths
    for (path, item) in build_health_paths() {
        spec.paths.paths.insert(path, item);
    }
    for (path, item) in build_service_paths(&service_names) {
        spec.paths.paths.insert(path, item);
    }
    if service_names.iter().any(|service| service == "stash") {
        for (path, item) in build_stash_paths() {
            spec.paths.paths.insert(path, item);
        }
    }
    for (path, item) in build_app_paths() {
        spec.paths.paths.insert(path, item);
    }
    for (path, item) in build_access_paths() {
        spec.paths.paths.insert(path, item);
    }
    for (path, item) in build_project_credential_paths() {
        spec.paths.paths.insert(path, item);
    }

    let mut value = serde_json::to_value(&spec)?;
    annotate_access_contracts(&mut value);
    let json = serde_json::to_string_pretty(&value)?;
    Ok(Arc::new(json))
}

fn annotate_access_contracts(spec: &mut serde_json::Value) {
    let contracts = [
        (
            "/auth/bootstrap/consume",
            "post",
            "atomic owner and credential creation; exact retry idempotent",
        ),
        ("/auth/bootstrap/status", "post", "none_expected"),
        (
            "/auth/bootstrap/cleanup",
            "post",
            "tombstone before exact-file cleanup; exact retry idempotent",
        ),
        (
            "/v1/access/credentials",
            "post",
            "credential creation; exact retry idempotent",
        ),
        ("/v1/access/credentials/self", "get", "none_expected"),
        (
            "/v1/access/credentials/{credential_id}",
            "delete",
            "immediate credential revocation; exact retry idempotent",
        ),
        (
            "/auth/local-session",
            "post",
            "source-bound browser session creation",
        ),
        (
            "/auth/local-session",
            "delete",
            "browser session revocation",
        ),
    ];
    for (path, method, side_effects) in contracts {
        let Some(operation) = spec["paths"][path][method].as_object_mut() else {
            continue;
        };
        operation.insert(
            "x-labby-cache-posture".into(),
            serde_json::json!("private, no-store"),
        );
        operation.insert(
            "x-labby-failure-disclosure".into(),
            serde_json::json!("uniform non-enumerating denial"),
        );
        operation.insert(
            "x-labby-side-effects".into(),
            serde_json::json!(side_effects),
        );
    }
}

// ── Tests ───────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dispatch::error::ToolError;

    fn assert_agent_error_fields(object: &serde_json::Map<String, serde_json::Value>) {
        for required in [
            "contract_version",
            "kind",
            "message",
            "origin",
            "recovery",
            "side_effects",
        ] {
            assert!(
                object.contains_key(required),
                "missing agent error field {required}"
            );
        }
        assert!(object["recovery"].get("action").is_some());
        assert!(object["recovery"].get("same_arguments").is_some());
        assert!(object["recovery"].get("guidance").is_some());
    }

    /// Verify doc-only error schemas stay in sync with `ToolError` wire format.
    ///
    /// If a field is added/removed from `ToolError`'s hand-written `Serialize`,
    /// this test must be updated to match.
    #[test]
    fn drift_test_error_schemas_match_tool_error_wire() {
        // UnknownAction
        let err = ToolError::UnknownAction {
            message: "test".into(),
            valid: vec!["a".into()],
            hint: Some("b".into()),
        };
        let v: serde_json::Value = serde_json::to_value(&err).unwrap();
        let obj = v.as_object().unwrap();
        assert_agent_error_fields(obj);
        assert!(obj.contains_key("kind"), "UnknownAction missing 'kind'");
        assert!(
            obj.contains_key("message"),
            "UnknownAction missing 'message'"
        );
        assert!(obj.contains_key("valid"), "UnknownAction missing 'valid'");
        assert!(obj.contains_key("hint"), "UnknownAction missing 'hint'");

        // MissingParam
        let err = ToolError::MissingParam {
            message: "test".into(),
            param: "q".into(),
        };
        let v: serde_json::Value = serde_json::to_value(&err).unwrap();
        let obj = v.as_object().unwrap();
        assert_agent_error_fields(obj);
        assert!(obj.contains_key("kind"), "MissingParam missing 'kind'");
        assert!(
            obj.contains_key("message"),
            "MissingParam missing 'message'"
        );
        assert!(obj.contains_key("param"), "MissingParam missing 'param'");

        // InvalidParam
        let err = ToolError::InvalidParam {
            message: "test".into(),
            param: "q".into(),
        };
        let v: serde_json::Value = serde_json::to_value(&err).unwrap();
        let obj = v.as_object().unwrap();
        assert_agent_error_fields(obj);
        assert!(obj.contains_key("kind"), "InvalidParam missing 'kind'");
        assert!(
            obj.contains_key("message"),
            "InvalidParam missing 'message'"
        );
        assert!(obj.contains_key("param"), "InvalidParam missing 'param'");

        // ConfirmationRequired
        let err = ToolError::ConfirmationRequired {
            message: "test".into(),
        };
        let v: serde_json::Value = serde_json::to_value(&err).unwrap();
        let obj = v.as_object().unwrap();
        assert_agent_error_fields(obj);
        assert!(
            obj.contains_key("kind"),
            "ConfirmationRequired missing 'kind'"
        );
        assert!(
            obj.contains_key("message"),
            "ConfirmationRequired missing 'message'"
        );

        // Sdk (pass-through)
        let err = ToolError::Sdk {
            sdk_kind: "auth_failed".into(),
            message: "test".into(),
        };
        let v: serde_json::Value = serde_json::to_value(&err).unwrap();
        let obj = v.as_object().unwrap();
        assert_agent_error_fields(obj);
        assert!(obj.contains_key("kind"), "Sdk missing 'kind'");
        assert!(obj.contains_key("message"), "Sdk missing 'message'");
        // Verify kind promotion: should be "auth_failed", not "sdk"
        assert_eq!(obj["kind"], "auth_failed", "Sdk kind not promoted");
    }

    #[test]
    fn param_type_string() {
        let schema = param_type_to_schema("string");
        let json = serde_json::to_value(&schema).unwrap();
        assert_eq!(json["type"], "string");
    }

    #[test]
    fn param_type_integer() {
        let schema = param_type_to_schema("integer");
        let json = serde_json::to_value(&schema).unwrap();
        assert_eq!(json["type"], "integer");
    }

    #[test]
    fn param_type_number() {
        let schema = param_type_to_schema("number");
        let json = serde_json::to_value(&schema).unwrap();
        assert_eq!(json["type"], "number");
    }

    #[test]
    fn param_type_boolean() {
        let schema = param_type_to_schema("boolean");
        let json = serde_json::to_value(&schema).unwrap();
        assert_eq!(json["type"], "boolean");
    }

    #[test]
    fn param_type_object() {
        let schema = param_type_to_schema("object");
        let json = serde_json::to_value(&schema).unwrap();
        assert_eq!(json["type"], "object");
    }

    #[test]
    fn param_type_array() {
        let schema = param_type_to_schema("array");
        let json = serde_json::to_value(&schema).unwrap();
        assert_eq!(json["type"], "array");
    }

    #[test]
    fn param_type_string_array() {
        let schema = param_type_to_schema("string[]");
        let json = serde_json::to_value(&schema).unwrap();
        assert_eq!(json["type"], "array");
        assert_eq!(json["items"]["type"], "string");
    }

    #[test]
    fn param_type_integer_array() {
        let schema = param_type_to_schema("integer[]");
        let json = serde_json::to_value(&schema).unwrap();
        assert_eq!(json["type"], "array");
        assert_eq!(json["items"]["type"], "integer");
    }

    #[test]
    fn param_type_settings_update_entry_array() {
        let schema = param_type_to_schema("SettingsUpdateEntry[]");
        let json = serde_json::to_value(&schema).unwrap();
        assert_eq!(json["type"], "array");
        assert_eq!(json["items"]["type"], "object");
        assert_eq!(json["items"]["properties"]["key"]["type"], "string");
        assert!(json["items"]["properties"]["value"].get("anyOf").is_some());
        assert!(
            json["items"]["properties"]["previous"]
                .get("anyOf")
                .is_some()
        );
        assert_eq!(json["items"]["properties"]["unset"]["type"], "boolean");
        assert!(
            json["items"]["required"]
                .as_array()
                .unwrap()
                .contains(&serde_json::json!("previous"))
        );
    }

    #[test]
    fn param_type_nullable_string() {
        let schema = param_type_to_schema("string|null");
        let json = serde_json::to_value(&schema).unwrap();
        // OpenAPI 3.1 nullable: anyOf with string and null
        assert!(json.get("anyOf").is_some(), "nullable should use anyOf");
    }

    #[test]
    fn param_type_enum_literals() {
        let schema = param_type_to_schema("queued|running|done");
        let json = serde_json::to_value(&schema).unwrap();
        assert_eq!(json["type"], "string");
        let enums = json["enum"].as_array().unwrap();
        assert_eq!(enums.len(), 3);
        assert_eq!(enums[0], "queued");
        assert_eq!(enums[1], "running");
        assert_eq!(enums[2], "done");
    }

    #[test]
    fn param_type_unknown_fallback() {
        let schema = param_type_to_schema("foobar");
        let json = serde_json::to_value(&schema).unwrap();
        assert_eq!(json["type"], "string");
    }

    #[test]
    fn to_pascal_case_basic() {
        assert_eq!(to_pascal_case("status.get"), "StatusGet");
        assert_eq!(to_pascal_case("health.list"), "HealthList");
        assert_eq!(to_pascal_case("help"), "Help");
        assert_eq!(to_pascal_case("status.update"), "StatusUpdate");
    }

    #[test]
    fn build_action_schemas_empty_services() {
        let schemas = build_action_schemas(&[]);
        assert!(schemas.is_empty());
    }

    #[test]
    fn build_health_paths_has_two_entries() {
        let paths = build_health_paths();
        assert_eq!(paths.len(), 2);
        assert_eq!(paths[0].0, "/health");
        assert_eq!(paths[1].0, "/ready");
    }

    #[test]
    fn build_service_paths_generates_per_service() {
        let names = vec!["gateway-alpha".to_string(), "gateway-beta".to_string()];
        let paths = build_service_paths(&names);
        assert_eq!(paths.len(), 2);
        assert_eq!(paths[0].0, "/v1/gateway-alpha");
        assert_eq!(paths[1].0, "/v1/gateway-beta");
    }

    #[test]
    fn stash_openapi_covers_every_dedicated_route_descriptor() {
        let paths = build_stash_paths();
        let documented = paths
            .iter()
            .flat_map(|(path, item)| {
                let json = serde_json::to_value(item).expect("path item json");
                ["get", "post", "patch", "delete"]
                    .into_iter()
                    .filter(move |method| json.get(*method).is_some())
                    .map(move |method| (method.to_ascii_uppercase(), path.clone()))
            })
            .collect::<std::collections::BTreeSet<_>>();
        let expected = crate::api::services::file_stash::descriptors()
            .into_iter()
            .map(|descriptor| {
                (
                    descriptor.method.to_owned(),
                    format!("/v1/stash{}", descriptor.path.trim_end_matches('/')),
                )
            })
            .collect::<std::collections::BTreeSet<_>>();
        assert_eq!(documented, expected);
    }

    #[test]
    fn build_app_paths_generates_operator_app_routes() {
        let paths = build_app_paths();
        assert!(
            paths
                .iter()
                .any(|(path, _)| path == APPS_MANIFEST_API_ROUTE)
        );
        assert!(
            paths
                .iter()
                .any(|(path, _)| path == SERVER_LOGS_QUERY_API_ROUTE)
        );
    }

    #[test]
    fn server_logs_query_openapi_lists_query_parameters() {
        let paths = build_app_paths();
        let (_, query_path) = paths
            .iter()
            .find(|(path, _)| path == SERVER_LOGS_QUERY_API_ROUTE)
            .expect("server logs query path");
        let json = serde_json::to_value(query_path).expect("path json");
        let parameters = json["get"]["parameters"]
            .as_array()
            .expect("query route should declare parameters");
        let names: std::collections::BTreeSet<_> = parameters
            .iter()
            .filter_map(|param| param["name"].as_str())
            .collect();

        for expected in [
            "limit",
            "level",
            "target",
            "service",
            "action",
            "kind",
            "query",
            "file",
            "max_scan_bytes",
        ] {
            assert!(
                names.contains(expected),
                "missing query parameter {expected}"
            );
        }
    }

    /// Round-trip integration test: build the full spec from the default registry
    /// and validate its top-level structure.
    #[test]
    fn full_spec_round_trip() {
        use crate::registry::build_default_registry;

        let registry = build_default_registry();
        let spec_json =
            build_openapi_spec(registry.services()).expect("spec serialization should succeed");

        let spec: serde_json::Value =
            serde_json::from_str(&spec_json).expect("spec should be valid JSON");

        // OpenAPI version
        assert_eq!(spec["openapi"], "3.1.0", "should be OpenAPI 3.1");

        // Info block
        assert_eq!(spec["info"]["title"], "lab API");
        assert!(spec["info"]["version"].as_str().is_some());

        // Paths must include health endpoints
        let paths = spec["paths"]
            .as_object()
            .expect("paths should be an object");
        assert!(paths.contains_key("/health"), "missing /health path");
        assert!(paths.contains_key("/ready"), "missing /ready path");
        assert!(
            paths.contains_key(APPS_MANIFEST_API_ROUTE),
            "missing {APPS_MANIFEST_API_ROUTE} path"
        );
        assert!(
            paths.contains_key(SERVER_LOGS_QUERY_API_ROUTE),
            "missing {SERVER_LOGS_QUERY_API_ROUTE} path"
        );

        // At least setup (always-on) should have a /v1/setup path
        assert!(paths.contains_key("/v1/setup"), "missing /v1/setup path");
        let bootstrap = &paths["/v1/access/bootstrap-owner"]["post"];
        assert_eq!(
            bootstrap["security"][0]["browser_session"],
            serde_json::json!([])
        );
        assert!(
            bootstrap["parameters"]
                .as_array()
                .is_some_and(|parameters| parameters.iter().any(|parameter| {
                    parameter["name"] == "x-csrf-token" && parameter["in"] == "header"
                }))
        );

        // Components must include our error schemas
        let schemas = spec["components"]["schemas"]
            .as_object()
            .expect("schemas should be an object");
        assert!(
            schemas.contains_key("ActionRequest"),
            "missing ActionRequest schema"
        );
        assert!(
            schemas.contains_key("HealthResponse"),
            "missing HealthResponse schema"
        );
        assert!(
            schemas.contains_key("ErrorUnknownAction"),
            "missing ErrorUnknownAction schema"
        );
        assert!(
            schemas.contains_key("ErrorMissingParam"),
            "missing ErrorMissingParam schema"
        );
        assert!(
            schemas.contains_key("AgentErrorResponse"),
            "missing AgentErrorResponse schema"
        );
        assert!(
            schemas.contains_key("AgentErrorRecovery"),
            "missing AgentErrorRecovery schema"
        );
        let setup_update = schemas
            .get("SetupSettingsConfigUpdateParams")
            .expect("missing settings config update params");
        assert_eq!(setup_update["properties"]["entries"]["type"], "array");
        assert_eq!(
            setup_update["properties"]["entries"]["items"]["type"],
            "object"
        );
        // Destructive actions no longer get an injected `confirm` schema param —
        // HTTP dispatch no longer requires or generates one.
        assert!(setup_update["properties"].get("confirm").is_none());

        for (schema_name, schema) in schemas {
            let Some(required) = schema.get("required").and_then(|value| value.as_array()) else {
                continue;
            };
            let mut seen = std::collections::BTreeSet::new();
            for required_field in required {
                let Some(required_field) = required_field.as_str() else {
                    continue;
                };
                assert!(
                    seen.insert(required_field),
                    "schema {schema_name} has duplicate required field `{required_field}`"
                );
            }
        }

        // Security scheme
        let security_schemes = spec["components"]["securitySchemes"]
            .as_object()
            .expect("securitySchemes should be an object");
        assert!(
            security_schemes.contains_key("bearer_auth"),
            "missing bearer_auth security scheme"
        );
        assert!(
            security_schemes.contains_key("browser_session"),
            "missing browser_session security scheme"
        );
        assert!(
            security_schemes.contains_key("LabbyBootstrapProof"),
            "missing LabbyBootstrapProof security scheme"
        );
        for path in [
            "/auth/bootstrap/consume",
            "/auth/bootstrap/status",
            "/auth/bootstrap/cleanup",
        ] {
            let operation = &paths[path]["post"];
            assert_eq!(
                operation["security"][0]["LabbyBootstrapProof"],
                serde_json::json!([])
            );
            assert_eq!(operation["x-labby-cache-posture"], "private, no-store");
            assert_eq!(
                operation["x-labby-failure-disclosure"],
                "uniform non-enumerating denial"
            );
            assert!(operation.get("x-labby-side-effects").is_some());
        }
        for path in [
            "/v1/access/credentials",
            "/v1/access/credentials/self",
            "/v1/access/credentials/{credential_id}",
            "/auth/local-session",
        ] {
            assert!(paths.contains_key(path), "missing access path {path}");
        }

        // Service dispatch paths should have POST operations with security requirement.
        // Non-dispatch app routes under /v1 (for example /v1/apps/manifest)
        // are documented separately and may use GET.
        for (path, item) in paths {
            if path.starts_with("/v1/")
                && let Some(post) = item.get("post")
            {
                assert!(
                    post.get("security").is_some(),
                    "POST {path} should have security requirement"
                );
            }
        }
        for path in [APPS_MANIFEST_API_ROUTE, SERVER_LOGS_QUERY_API_ROUTE] {
            let get = paths
                .get(path)
                .and_then(|item| item.get("get"))
                .unwrap_or_else(|| panic!("{path} should have a GET operation"));
            assert!(
                get.get("security").is_some(),
                "GET {path} should have security requirement"
            );
        }
    }
}
