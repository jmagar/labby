//! Native MCP adapter for Agent Skills (SEP-2640).
//!
//! Canonical list/get/read semantics live in crate::skills::facade. This module
//! owns only MCP authorization, JSON-RPC shapes, request-context projection,
//! and native extension observability.

pub(crate) use crate::skills::is_skill_uri;
use std::future::Future;
use std::time::Instant;

use labby_runtime::error::ToolError;
use labby_runtime::skills::wire::{
    CACHE_SCOPE_PRIVATE, SKILLS_GET_METHOD, SkillsGetParams, SkillsGetResult, SkillsListResult,
};
use rmcp::RoleServer;
use rmcp::model::{CustomRequest, CustomResult, ErrorData};
use rmcp::service::RequestContext;

use crate::mcp::context::{auth_context_from_extensions, code_mode_read_scope_allowed};
use crate::mcp::server::LabMcpServer;
use crate::skills::aggregate::ToolAccess;
use crate::skills::facade::{
    SkillCallerScope, SkillRegistryContext, get_visible_skill, list_visible_skills,
};

fn optional_header_str<'a>(
    headers: &'a axum::http::HeaderMap,
    name: &'static str,
) -> Result<Option<&'a str>, ToolError> {
    headers
        .get(name)
        .map(|value| {
            value.to_str().map_err(|_| ToolError::InvalidParam {
                message: "Skill Library request header is invalid".to_owned(),
                param: name.to_owned(),
            })
        })
        .transpose()
}

#[cfg(test)]
pub(crate) async fn dispatch_at_in_process_boundary(
    registry: &SkillRegistryContext,
    action: &str,
    params: serde_json::Value,
) -> Result<serde_json::Value, ToolError> {
    crate::dispatch::skills::dispatch_with_context(registry, action, params).await
}

#[cfg(feature = "skills")]
fn parse_public_import_params(
    params: serde_json::Value,
) -> Result<crate::dispatch::skill_library::params::ImportParams, ToolError> {
    serde_json::from_value(params).map_err(|_| ToolError::InvalidParam {
        message: "Skill Library import parameters are invalid".to_owned(),
        param: "params".to_owned(),
    })
}

#[cfg(feature = "skills")]
fn parse_public_import_batch_params(
    params: serde_json::Value,
) -> Result<crate::dispatch::skill_library::params::ImportBatchParams, ToolError> {
    serde_json::from_value(params).map_err(|_| ToolError::InvalidParam {
        message: "Artifact batch import parameters are invalid".to_owned(),
        param: "params".to_owned(),
    })
}

#[cfg(feature = "skills")]
async fn dispatch_public_import<F, Fut>(
    params: serde_json::Value,
    execute: F,
) -> Result<serde_json::Value, ToolError>
where
    F: FnOnce(crate::dispatch::skill_library::params::ImportParams) -> Fut,
    Fut: Future<Output = Result<serde_json::Value, ToolError>>,
{
    execute(parse_public_import_params(params)?).await
}

impl LabMcpServer {
    fn product_credential_bound_for_skills(
        &self,
        parts: &axum::http::request::Parts,
        project_id: &str,
    ) -> bool {
        let Some(identity) = parts.extensions.get::<labby_auth::VerifiedIdentity>() else {
            return false;
        };
        if identity.authenticator() != labby_auth::Authenticator::ProductCredential {
            return false;
        }
        let source = parts
            .extensions
            .get::<labby_primitives::product_credential::ProductCredentialGrant>();
        let bound = parts
            .extensions
            .get::<labby_primitives::product_credential::BoundAccessGrant>();
        source.zip(bound).is_some_and(|(source, bound)| {
            crate::dispatch::skill_library::auth::product_grants_are_route_bound(source, bound)
                && bound.project_id == project_id
                && self.route_scope.matches_product_route(&bound.route_id)
                && self.route_scope.allows_service("skills")
                && self.route_scope.exposes_skills()
        })
    }

    pub(crate) async fn artifact_access_for_request(
        &self,
        context: &RequestContext<RoleServer>,
    ) -> Result<Option<crate::skills::facade::ArtifactAccessSnapshot>, ToolError> {
        let Some(parts) = context.extensions.get::<axum::http::request::Parts>() else {
            return Ok(None);
        };
        let (Some(identity), Some(auth), Some(project_header)) = (
            parts.extensions.get::<labby_auth::VerifiedIdentity>(),
            parts
                .extensions
                .get::<labby_auth::auth_context::AuthContext>(),
            parts.headers.get("x-labby-project-id"),
        ) else {
            return Ok(None);
        };
        let project_id = project_header
            .to_str()
            .map_err(|_| ToolError::InvalidParam {
                message: "Skill Library project context is invalid".to_owned(),
                param: "x-labby-project-id".to_owned(),
            })?;
        let transport = if self.product_credential_bound_for_skills(parts, project_id) {
            crate::dispatch::skill_library::auth::SkillLibraryTransport::product_bearer(
                crate::dispatch::skill_library::auth::SkillLibrarySurface::Mcp,
            )
        } else {
            crate::dispatch::skill_library::auth::SkillLibraryTransport::bearer(
                crate::dispatch::skill_library::auth::SkillLibrarySurface::Mcp,
                true,
            )
        };
        let selected_team_id =
            optional_header_str(&parts.headers, "x-labby-team-id")?.map(str::to_owned);
        let caller = crate::dispatch::skill_library::auth::SkillLibraryCaller::new(
            identity.clone(),
            auth.scopes.clone(),
            transport,
        )
        .with_selected_team_id(selected_team_id.clone());
        let request_id =
            optional_header_str(&parts.headers, "x-request-id")?.unwrap_or("mcp-skills-read");
        let correlation =
            crate::dispatch::skill_library::audit::SkillLibraryCorrelationId::parse(request_id)
                .map_err(|_| ToolError::InvalidParam {
                    message: "Skill Library request correlation is invalid".to_owned(),
                    param: "x-request-id".to_owned(),
                })?;
        let decision = crate::dispatch::skill_library::auth::authorize_at_boundary(
            &self.access_runtime,
            caller,
            project_id,
            crate::dispatch::skill_library::auth::SkillLibraryAction::List,
            &crate::dispatch::skill_library::audit::CanonicalArtifactId::parse("library").map_err(
                |_| ToolError::Sdk {
                    sdk_kind: "internal_error".to_owned(),
                    message: "Skill Library authorization request is invalid".to_owned(),
                },
            )?,
            crate::dispatch::skill_library::auth::SkillLibraryTarget::SharedActive,
            &correlation,
        )
        .await
        .map_err(|error| {
            crate::dispatch::skill_library::map_dispatch_error(
                crate::dispatch::skill_library::dispatch::SkillLibraryDispatchError::Authorization(
                    error,
                ),
            )
        })?;
        Ok(Some(decision.artifact_access_snapshot()))
    }

    #[cfg(feature = "skills")]
    async fn dispatch_skill_library_management(
        &self,
        context: &RequestContext<RoleServer>,
        action: &str,
        params: serde_json::Value,
    ) -> Result<serde_json::Value, ToolError> {
        if matches!(action, "help" | "schema") {
            return crate::dispatch::artifacts::dispatch(action, params).await;
        }
        let service =
            crate::dispatch::skill_library::process_service().ok_or_else(|| ToolError::Sdk {
                sdk_kind: "service_unavailable".to_owned(),
                message: "Skill Library is unavailable".to_owned(),
            })?;
        let parts = context
            .extensions
            .get::<axum::http::request::Parts>()
            .ok_or_else(|| ToolError::Forbidden {
                message: "Skill Library requires an authenticated transport context".to_owned(),
                required_scopes: Vec::new(),
            })?;
        let boundary = super::call_tool::skill_library_callback_boundary(parts)?;
        let project_id =
            optional_header_str(&parts.headers, "x-labby-project-id")?.ok_or_else(|| {
                ToolError::Forbidden {
                    message: "Skill Library project context is required".to_owned(),
                    required_scopes: Vec::new(),
                }
            })?;
        let product_credential_bound = self.product_credential_bound_for_skills(parts, project_id);
        let request_id = optional_header_str(&parts.headers, "x-request-id")?;
        let correlation = super::call_tool::skill_library_callback_correlation(request_id)?;
        let transport = if boundary.product_credential_bound && product_credential_bound {
            crate::dispatch::skill_library::auth::SkillLibraryTransport::product_app_callback()
        } else {
            crate::dispatch::skill_library::auth::SkillLibraryTransport::app_callback(true, true)
        };
        let selected_team_id =
            optional_header_str(&parts.headers, "x-labby-team-id")?.map(str::to_owned);
        let caller = crate::dispatch::skill_library::auth::SkillLibraryCaller::new(
            boundary.identity.clone(),
            boundary.scopes.clone(),
            transport,
        )
        .with_selected_team_id(selected_team_id.clone());
        if crate::dispatch::remote_control::REMOTE_ARTIFACT_ACTIONS
            .iter()
            .any(|candidate| candidate.name == action)
        {
            let operation = crate::dispatch::remote_control::operation("artifacts", action)
                .ok_or_else(|| ToolError::UnknownAction {
                    message: format!("Unknown action: {action}"),
                    valid: Vec::new(),
                    hint: None,
                })?;
            let permission = crate::dispatch::artifact_control::operation_permission(operation);
            let authority = crate::dispatch::artifact_control::authorize_authority_context(
                &self.access_runtime,
                boundary.identity,
                project_id,
                selected_team_id.as_deref(),
                permission,
            )
            .await?;
            return crate::dispatch::remote_control::dispatch_with_context(
                "artifacts",
                action,
                params,
                Some(&authority),
            )
            .await;
        }
        if action == "artifacts.import" {
            let imports = crate::dispatch::skill_library::process_imports().ok_or_else(|| {
                ToolError::Sdk {
                    sdk_kind: "source_unavailable".to_owned(),
                    message: "Skill import sources are not configured".to_owned(),
                }
            })?;
            return dispatch_public_import(params, |import_params| async move {
                imports
                    .import_selected(
                        &service,
                        &self.access_runtime,
                        caller,
                        project_id,
                        import_params.source,
                        import_params.expected_library_version,
                        import_params.idempotency_key,
                        &correlation,
                    )
                    .await
                    .map_err(crate::dispatch::skill_library::map_import_error)
            })
            .await;
        }
        if action == "artifacts.import_batch" {
            let imports = crate::dispatch::skill_library::process_imports().ok_or_else(|| {
                ToolError::Sdk {
                    sdk_kind: "source_unavailable".to_owned(),
                    message: "Artifact import sources are not configured".to_owned(),
                }
            })?;
            let import_params = parse_public_import_batch_params(params)?;
            return imports
                .import_batch_selected(
                    &service,
                    &self.access_runtime,
                    caller,
                    project_id,
                    import_params.sources,
                    import_params.expected_library_version,
                    import_params.idempotency_key,
                    &correlation,
                )
                .await
                .map_err(crate::dispatch::skill_library::map_import_error);
        }
        service
            .dispatch(
                &self.access_runtime,
                caller,
                project_id,
                action,
                params,
                &correlation,
            )
            .await
            .map_err(crate::dispatch::skill_library::map_dispatch_error)
    }

    /// Project MCP route/auth state into the transport-neutral Skills context.
    pub(crate) async fn skill_registry_context(
        &self,
        context: &RequestContext<RoleServer>,
    ) -> Result<SkillRegistryContext, ToolError> {
        #[cfg(feature = "gateway")]
        {
            let Some(manager) = self.gateway_manager.as_ref() else {
                return Ok(SkillRegistryContext::first_party_only());
            };
            let access = if manager.code_mode_enabled().await {
                ToolAccess::CodeModeOnly
            } else {
                ToolAccess::Direct
            };
            let subject = self.request_subject(context).map(str::to_string);
            let scope = match self.route_scope.allowed_upstreams() {
                None => SkillCallerScope::root(subject, access),
                Some(allowed) => {
                    SkillCallerScope::restricted(allowed.iter().cloned(), subject, access)
                }
            };
            let registry =
                SkillRegistryContext::with_manager(std::sync::Arc::clone(manager), scope);
            return Ok(match self.artifact_access_for_request(context).await? {
                Some(access) => registry.with_artifact_access(access),
                None => registry,
            });
        }

        #[cfg(not(feature = "gateway"))]
        {
            let _ = context;
            let registry = SkillRegistryContext::first_party_only();
            Ok(match self.artifact_access_for_request(context).await? {
                Some(access) => registry.with_artifact_access(access),
                None => registry,
            })
        }
    }

    /// Dispatch the Artifact Library tool behind a heap boundary.
    ///
    /// `call_tool_impl` is already a large async state machine. Returning an
    /// erased boxed future here prevents the concrete Skills list/get/read
    /// future from inflating that parent stack frame while preserving the same
    /// caller-scoped registry and authorization semantics.
    pub(crate) fn dispatch_artifact_tool_boxed<'a>(
        &'a self,
        context: &'a RequestContext<RoleServer>,
        meta: Option<&'a rmcp::model::RequestMetaObject>,
        action: &'a str,
        params: serde_json::Value,
    ) -> std::pin::Pin<Box<dyn Future<Output = Result<serde_json::Value, ToolError>> + Send + 'a>>
    {
        Box::pin(async move {
            let _ = meta;
            self.dispatch_skill_library_management(context, action, params)
                .await
        })
    }

    /// Answer native Skills extension list/get requests.
    pub(crate) async fn handle_skills_request(
        &self,
        request: &CustomRequest,
        context: &RequestContext<RoleServer>,
    ) -> Result<CustomResult, ErrorData> {
        let start = Instant::now();
        let action = if request.method == SKILLS_GET_METHOD {
            "skills.get"
        } else {
            "skills.list"
        };
        let subject_log = self.request_subject_log_tag(context);
        let outcome = self.dispatch_skills_request(request, context).await;

        match &outcome {
            Ok(_) => tracing::info!(
                surface = "mcp",
                service = "labby",
                action,
                subject = %subject_log,
                elapsed_ms = start.elapsed().as_millis(),
                "dispatch finish"
            ),
            Err(error) => tracing::warn!(
                surface = "mcp",
                service = "labby",
                action,
                subject = %subject_log,
                elapsed_ms = start.elapsed().as_millis(),
                kind = %error.code.0,
                "dispatch error"
            ),
        }
        outcome
    }

    async fn dispatch_skills_request(
        &self,
        request: &CustomRequest,
        context: &RequestContext<RoleServer>,
    ) -> Result<CustomResult, ErrorData> {
        let auth = auth_context_from_extensions(&context.extensions);
        if !code_mode_read_scope_allowed(auth) {
            return Err(ErrorData::new(
                rmcp::model::ErrorCode::INVALID_REQUEST,
                "reading skills requires the lab:read scope".to_string(),
                None,
            ));
        }
        if !self.route_scope.exposes_skills() {
            if request.method == SKILLS_GET_METHOD {
                return Err(ErrorData::new(
                    rmcp::model::ErrorCode::INVALID_REQUEST,
                    "Agent Skills are disabled by this loadout; ask the operator to enable Skills and Resources for this loadout".to_string(),
                    None,
                ));
            }
            tracing::info!(
                surface = "mcp",
                service = "labby",
                action = "skills.list",
                route_scope = %self.route_scope.label(),
                "Skills catalog hidden by loadout"
            );
            return serde_json::to_value(SkillsListResult {
                result_type: Default::default(),
                skills: Vec::new(),
                next_cursor: None,
                ttl_ms: Some(0),
                cache_scope: Some(CACHE_SCOPE_PRIVATE.to_string()),
                meta: None,
            })
            .map(CustomResult::new)
            .map_err(|error| ErrorData::internal_error(error.to_string(), None));
        }

        let registry = self
            .skill_registry_context(context)
            .await
            .map_err(skill_read_error)?;
        tracing::debug!(
            surface = "mcp",
            service = "labby",
            method = %request.method,
            skill_generation = registry.generation_id(),
            skill_generation_digest = registry.generation_digest(),
            "captured Skill generation"
        );
        dispatch_native_with_registry(request, &registry).await
    }
}

async fn dispatch_native_with_registry(
    request: &CustomRequest,
    registry: &SkillRegistryContext,
) -> Result<CustomResult, ErrorData> {
    if request.method == SKILLS_GET_METHOD {
        let params = request
            .params_as::<SkillsGetParams>()
            .map_err(|error| ErrorData::invalid_params(error.to_string(), None))?
            .ok_or_else(|| ErrorData::invalid_params("skills/get requires uri", None))?;
        let entry = get_visible_skill(registry, &params.uri)
            .await
            .map_err(skill_read_error)?
            .ok_or_else(|| {
                ErrorData::invalid_params(
                    format!("'{}' is not a skill this server serves", params.uri),
                    None,
                )
            })?;
        let result = SkillsGetResult {
            result_type: Default::default(),
            skill: entry,
        };
        return serde_json::to_value(result)
            .map(CustomResult::new)
            .map_err(|error| ErrorData::internal_error(error.to_string(), None));
    }

    serde_json::to_value(list_visible_skills(registry).await)
        .map(CustomResult::new)
        .map_err(|error| ErrorData::internal_error(error.to_string(), None))
}

/// Preserve native resources/read wire semantics while the canonical reader
/// returns the shared ToolError contract.
pub(crate) fn skill_read_error(error: ToolError) -> ErrorData {
    let payload = serde_json::to_string(&error).unwrap_or_else(|_| error.to_string());
    match error.kind() {
        labby_runtime::skills::KIND_SKILL_DIGEST_MISMATCH
        | labby_runtime::skills::KIND_SKILL_MANIFEST_STALE => {
            ErrorData::internal_error(payload, None)
        }
        _ => ErrorData::invalid_params(payload, None),
    }
}

#[cfg(test)]
mod serve_tests {
    use super::*;

    #[tokio::test]
    async fn mcp_import_rejects_acquisition_bytes_and_routes_exact_selector() {
        use std::sync::atomic::{AtomicUsize, Ordering};

        let calls = AtomicUsize::new(0);
        let raw = serde_json::json!({
            "acquisition": { "interchange": {}, "files": [] },
            "expected_library_version": 0,
            "idempotency_key": "raw-bytes"
        });
        assert!(
            dispatch_public_import(raw, |_| async {
                calls.fetch_add(1, Ordering::SeqCst);
                Ok(serde_json::Value::Null)
            })
            .await
            .is_err()
        );
        assert_eq!(calls.load(Ordering::SeqCst), 0);
        let selector = serde_json::json!({
            "source": {
                "kind": "depot",
                "connection_id": "configured-depot",
                "artifact_id": "artifact",
                "revision_id": format!("sha256:{}", "0".repeat(64))
            },
            "expected_library_version": 0,
            "idempotency_key": "selector"
        });
        let result = dispatch_public_import(selector, |params| async {
            calls.fetch_add(1, Ordering::SeqCst);
            match params.source {
                crate::dispatch::skill_library::params::SourceSelector::Depot {
                    connection_id,
                    artifact_id,
                    revision_id,
                } => {
                    assert_eq!(connection_id, "configured-depot");
                    assert_eq!(artifact_id, "artifact");
                    assert_eq!(revision_id, format!("sha256:{}", "0".repeat(64)));
                }
                _ => panic!("MCP selector changed source family"),
            }
            Ok(serde_json::json!({"outcome": "committed"}))
        })
        .await
        .unwrap();
        assert_eq!(result["outcome"], "committed");
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn malformed_skill_library_headers_are_not_treated_as_missing() {
        let mut headers = axum::http::HeaderMap::new();
        headers.insert(
            "x-labby-project-id",
            axum::http::HeaderValue::from_bytes(b"\xff").unwrap(),
        );
        headers.insert(
            "x-request-id",
            axum::http::HeaderValue::from_bytes(b"\xfe").unwrap(),
        );

        let project_error = optional_header_str(&headers, "x-labby-project-id").unwrap_err();
        assert_eq!(project_error.kind(), "invalid_param");
        assert_eq!(project_error.extra_fields()["param"], "x-labby-project-id");
        let request_error = optional_header_str(&headers, "x-request-id").unwrap_err();
        assert_eq!(request_error.kind(), "invalid_param");
        assert_eq!(request_error.extra_fields()["param"], "x-request-id");
    }

    fn write_native_skill(root: &std::path::Path, version: &str) {
        let dir = root.join("native-race");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("SKILL.md"),
            format!("---\nname: native-race\ndescription: {version}\n---\n\n{version}\n"),
        )
        .unwrap();
        std::fs::write(dir.join("notes.md"), format!("support-{version}\n")).unwrap();
    }

    #[tokio::test]
    async fn native_list_and_get_are_pinned_during_refresh() {
        use crate::skills::registry::{FirstPartyGenerationManager, GenerationLimits};
        use labby_runtime::skills::wire::SKILLS_LIST_METHOD;

        let temp = tempfile::tempdir().unwrap();
        write_native_skill(temp.path(), "old");
        let manager = FirstPartyGenerationManager::new(
            temp.path().to_path_buf(),
            GenerationLimits::default(),
        );
        let pinned = SkillRegistryContext::from_generation(manager.generation());
        write_native_skill(temp.path(), "new");
        manager.refresh(None).unwrap();

        let listed =
            dispatch_native_with_registry(&CustomRequest::new(SKILLS_LIST_METHOD, None), &pinned)
                .await
                .unwrap();
        let listing: SkillsListResult = serde_json::from_value(listed.0).unwrap();
        let entry = listing
            .skills
            .iter()
            .find(|entry| entry.uri == "skill://labby/native-race/SKILL.md")
            .unwrap();
        assert_eq!(entry.frontmatter["description"], "old");

        let got = dispatch_native_with_registry(
            &CustomRequest::new(
                SKILLS_GET_METHOD,
                Some(serde_json::json!({ "uri": entry.uri })),
            ),
            &pinned,
        )
        .await
        .unwrap();
        let result: SkillsGetResult = serde_json::from_value(got.0).unwrap();
        let resource = result
            .skill
            .resources
            .as_ref()
            .unwrap()
            .iter()
            .find(|resource| resource.uri.ends_with("/notes.md"))
            .unwrap();
        let file = crate::skills::facade::read_visible_skill_file(&pinned, &resource.uri)
            .await
            .unwrap();
        assert_eq!(resource.digest, file.digest);
        assert!(
            labby_runtime::skills::parse_digest(&resource.digest)
                .unwrap()
                .matches(file.text().unwrap().as_bytes())
        );
        let resource_file = crate::mcp::handlers_resources::read_skill_resource_with_registry(
            &pinned,
            &resource.uri,
        )
        .await
        .unwrap();
        assert_eq!(resource_file.digest, resource.digest);
        assert_eq!(resource_file.text(), file.text());
        assert_eq!(resource_file.text(), Some("support-old\n"));

        let current = SkillRegistryContext::from_generation(manager.generation());
        let current_file = crate::mcp::handlers_resources::read_skill_resource_with_registry(
            &current,
            &resource.uri,
        )
        .await
        .unwrap();
        assert_eq!(current_file.text(), Some("support-new\n"));
        assert_ne!(current_file.digest, resource_file.digest);
    }

    #[test]
    fn the_capability_is_advertised_with_no_optional_features() {
        let extensions = crate::mcp::server::mcp_extensions_for_test();
        let declared = extensions
            .get(labby_runtime::skills::wire::SKILLS_EXTENSION_KEY)
            .expect("skills extension is advertised when the feature is on");
        assert!(declared.is_empty(), "directoryRead must not be advertised");
    }

    #[tokio::test]
    async fn first_party_get_rejects_a_supporting_file_uri() {
        let uri = "skill://labby/creating-snippets/README.md";
        let registry = SkillRegistryContext::first_party_only();
        assert!(get_visible_skill(&registry, uri).await.unwrap().is_none());
    }

    #[tokio::test]
    async fn every_first_party_manifest_file_verifies_against_served_bytes() {
        let registry = SkillRegistryContext::first_party_only();
        let listing = list_visible_skills(&registry).await;
        assert!(!listing.skills.is_empty());
        for entry in &listing.skills {
            for resource in entry.resources.as_ref().expect("manifest") {
                let file = crate::skills::facade::read_visible_skill_file(&registry, &resource.uri)
                    .await
                    .expect("every listed file is served");
                let digest =
                    labby_runtime::skills::parse_digest(&resource.digest).expect("valid digest");
                assert!(
                    digest.matches(file.text().unwrap().as_bytes()),
                    "{} failed",
                    resource.uri
                );
            }
        }
    }

    #[tokio::test]
    async fn unknown_first_party_skill_uris_are_not_served() {
        let registry = SkillRegistryContext::first_party_only();
        let invalid = "skill://labby/using-labby/../escape.md";
        assert!(get_visible_skill(&registry, invalid).await.is_err());
        assert!(
            crate::skills::facade::read_visible_skill_file(&registry, invalid)
                .await
                .is_err()
        );

        for uri in ["skill://labby/nonexistent/SKILL.md"] {
            assert!(get_visible_skill(&registry, uri).await.unwrap().is_none());
            assert!(
                crate::skills::facade::read_visible_skill_file(&registry, uri)
                    .await
                    .is_err()
            );
        }
    }

    #[test]
    fn proxied_uri_reconstruction_removes_the_gateway_label() {
        assert_eq!(
            labby_runtime::skills::parse_skill_uri("skill://gh/skill/acme/refunds/SKILL.md")
                .expect("published URI")
                .upstream_uri_for_origin("gh")
                .expect("reconstructable skill URI"),
            "skill://acme/refunds/SKILL.md"
        );
        assert!(
            labby_runtime::skills::parse_skill_uri("skill://other/skill/acme/refunds/SKILL.md")
                .expect("published URI")
                .upstream_uri_for_origin("gh")
                .is_none()
        );
    }
}
