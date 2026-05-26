//! OpenAPI 3.1 schema generation for the lab HTTP API.
//!
//! All utoipa coupling is confined to this module. The spec is built
//! programmatically from the `ActionSpec` catalog — no `#[utoipa::path]`
//! annotations on handlers.

use std::sync::Arc;

use serde::Serialize;
use utoipa::openapi::path::{OperationBuilder, PathItemBuilder};
use utoipa::openapi::request_body::RequestBodyBuilder;
use utoipa::openapi::schema::SchemaType;
use utoipa::openapi::security::{HttpAuthScheme, HttpBuilder, SecurityScheme};
use utoipa::openapi::{
    Components, ContentBuilder, ObjectBuilder, PathItem, RefOr, ResponseBuilder, ResponsesBuilder,
    Schema, SecurityRequirement, Type,
};
use utoipa::{Modify, OpenApi, ToSchema};

use crate::registry::RegisteredService;

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

/// Error envelope for SDK pass-through errors (`auth_failed`, `rate_limited`, etc.).
#[derive(Debug, Serialize, ToSchema)]
pub struct ErrorSdk {
    /// Stable kind tag from the SDK (e.g. `"auth_failed"`, `"rate_limited"`).
    pub kind: String,
    /// Human-readable message.
    pub message: String,
}

// ── Param type → OpenAPI schema conversion ──────────────────────────────

/// Convert a `ParamSpec.ty` string label to an `OpenAPI` `Schema`.
///
/// Handles the 10 known type labels plus unknown fallback:
/// - `"string"`, `"integer"`, `"number"`, `"boolean"`, `"object"`, `"array"`
/// - `"string[]"`, `"integer[]"`
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

// ── PascalCase conversion ───────────────────────────────────────────────

/// Convert a dotted action name to `PascalCase` for schema naming.
///
/// `"movie.search"` → `"MovieSearch"`, `"queue.list"` → `"QueueList"`
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
/// Names follow the pattern `{Service}{Action}Params` — e.g., `RadarrMovieSearchParams`.
#[must_use]
pub fn build_action_schemas(services: &[RegisteredService]) -> Vec<(String, RefOr<Schema>)> {
    let mut schemas = Vec::new();
    for svc in services {
        let svc_pascal = to_pascal_case(svc.name);
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
    let mut paths = service_names
        .iter()
        .map(|svc| {
            let path = format!("/v1/{svc}");
            let operation = OperationBuilder::new()
                .tag(svc)
                .summary(Some(format!("Dispatch action to {svc}")))
                .description(Some(format!(
                    "Execute an action on the {svc} service. Use `action: \"help\"` to list available actions."
                )))
                .request_body(Some(
                    RequestBodyBuilder::new()
                        .content(
                            "application/json",
                            ContentBuilder::new()
                                .schema(Some(RefOr::Ref(utoipa::openapi::Ref::new(
                                    "#/components/schemas/ActionRequest",
                                ))))
                                .build(),
                        )
                        .required(Some(utoipa::openapi::Required::True))
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
                            ResponseBuilder::new()
                                .description("Bad request (unknown action, confirmation required)")
                                .content(
                                    "application/json",
                                    ContentBuilder::new()
                                        .schema(Some(RefOr::Ref(utoipa::openapi::Ref::new(
                                            "#/components/schemas/ErrorUnknownAction",
                                        ))))
                                        .build(),
                                )
                                .build(),
                        )
                        .response(
                            "401",
                            ResponseBuilder::new()
                                .description("Authentication failed")
                                .content(
                                    "application/json",
                                    ContentBuilder::new()
                                        .schema(Some(RefOr::Ref(utoipa::openapi::Ref::new(
                                            "#/components/schemas/ErrorSdk",
                                        ))))
                                        .build(),
                                )
                                .build(),
                        )
                        .response(
                            "422",
                            ResponseBuilder::new()
                                .description("Validation error (missing or invalid param)")
                                .content(
                                    "application/json",
                                    ContentBuilder::new()
                                        .schema(Some(RefOr::Ref(utoipa::openapi::Ref::new(
                                            "#/components/schemas/ErrorMissingParam",
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

            let item = PathItemBuilder::new()
                .operation(utoipa::openapi::HttpMethod::Post, operation)
                .build();
            (path, item)
        })
        .collect::<Vec<_>>();

    if service_names.iter().any(|name| name == "logs") {
        let stream_item = PathItemBuilder::new()
            .operation(
                utoipa::openapi::HttpMethod::Get,
                OperationBuilder::new()
                    .tag("logs")
                    .summary(Some("Subscribe to live local-master log events"))
                    .description(Some(
                        "Server-sent events stream for live local-master logs. API clients use bearer auth here, while the hosted gateway-admin browser consumes the same endpoint with same-origin session auth.",
                    ))
                    .responses(
                        ResponsesBuilder::new()
                            .response(
                                "200",
                                ResponseBuilder::new()
                                    .description("SSE event stream")
                                    .content(
                                        "text/event-stream",
                                        ContentBuilder::new()
                                            .schema(Some(RefOr::T(
                                                ObjectBuilder::new()
                                                    .schema_type(SchemaType::Type(Type::String))
                                                    .build()
                                                    .into(),
                                            )))
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
                    .build(),
            )
            .build();
        paths.push(("/v1/logs/stream".to_string(), stream_item));
    }

    paths
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
        ErrorSdk,
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

    let json = serde_json::to_string_pretty(&spec)?;
    Ok(Arc::new(json))
}

// ── Tests ───────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dispatch::error::ToolError;

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
        assert_eq!(to_pascal_case("movie.search"), "MovieSearch");
        assert_eq!(to_pascal_case("queue.list"), "QueueList");
        assert_eq!(to_pascal_case("help"), "Help");
        assert_eq!(to_pascal_case("movie.add"), "MovieAdd");
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
        let names = vec!["radarr".to_string(), "sonarr".to_string()];
        let paths = build_service_paths(&names);
        assert_eq!(paths.len(), 2);
        assert_eq!(paths[0].0, "/v1/radarr");
        assert_eq!(paths[1].0, "/v1/sonarr");
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

        // At least setup (always-on) should have a /v1/setup path
        assert!(paths.contains_key("/v1/setup"), "missing /v1/setup path");

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
        assert!(schemas.contains_key("ErrorSdk"), "missing ErrorSdk schema");

        // Security scheme
        let security_schemes = spec["components"]["securitySchemes"]
            .as_object()
            .expect("securitySchemes should be an object");
        assert!(
            security_schemes.contains_key("bearer_auth"),
            "missing bearer_auth security scheme"
        );

        // Service paths should have POST operations with security requirement
        for (path, item) in paths {
            if path.starts_with("/v1/") && !path.ends_with("/actions") && path != "/v1/logs/stream"
            {
                let post = item.get("post");
                assert!(
                    post.is_some(),
                    "service path {path} should have a POST operation"
                );
                if let Some(post) = post {
                    assert!(
                        post.get("security").is_some(),
                        "POST {path} should have security requirement"
                    );
                }
            }
        }

        let stream = paths
            .get("/v1/logs/stream")
            .expect("missing /v1/logs/stream path");
        assert!(
            stream.get("get").is_some(),
            "/v1/logs/stream should expose GET for SSE"
        );
    }
}
