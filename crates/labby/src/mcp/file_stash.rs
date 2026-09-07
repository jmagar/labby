use std::sync::Arc;

use axum::http::request::Parts;
use base64::Engine as _;
use rmcp::model::{ReadResourceResult, Resource, ResourceContents, ResourceTemplate};
use rmcp::service::RequestContext;
use rmcp::{ErrorData, RoleServer};
use tokio::io::AsyncReadExt;

use crate::dispatch::error::ToolError;
use crate::dispatch::file_stash::{FileStashService, FileView, parse_stash_uri};
use crate::file_stash::PrincipalId;
use crate::mcp::context::{
    auth_context_from_extensions, propagated_caller_auth, resolve_caller_authorization,
};
use crate::mcp::server::LabMcpServer;

pub(crate) const TEMPLATE_URI: &str = "stash://me/files/{file_id}";
const PRIVATE_IN_PROCESS_TRANSPORT: &str = "in-process";

pub(crate) struct ResolvedStashPrincipal {
    id: PrincipalId,
    _lease: crate::access::ActiveFileStashPrincipalLease,
}

impl std::ops::Deref for ResolvedStashPrincipal {
    type Target = PrincipalId;
    fn deref(&self) -> &Self::Target {
        &self.id
    }
}

impl LabMcpServer {
    pub(crate) fn file_stash_caller_bound(&self) -> bool {
        self.registry.dispatch_capability("stash")
            == Some(crate::registry::DispatchCapability::CallerBound)
    }

    pub(crate) async fn dispatch_caller_bound_service(
        &self,
        service: &str,
        action: &str,
        params: serde_json::Value,
        context: &RequestContext<RoleServer>,
        meta: Option<&rmcp::model::RequestMetaObject>,
    ) -> Result<serde_json::Value, ToolError> {
        match service {
            "stash" => {
                let (principal, validated_grantee) = if action == "stash.grants.create" {
                    let recipient = params
                        .get("grantee_principal_id")
                        .and_then(serde_json::Value::as_str)
                        .ok_or_else(|| ToolError::InvalidParam {
                            param: "grantee_principal_id".into(),
                            message: "invalid File Stash parameter".into(),
                        })?
                        .to_owned();
                    let (owner, recipient, lease) = self
                        .file_stash_participants(context, meta, recipient)
                        .await?;
                    (
                        ResolvedStashPrincipal {
                            id: owner,
                            _lease: lease,
                        },
                        Some(recipient),
                    )
                } else {
                    (self.file_stash_principal(context, meta).await?, None)
                };
                crate::dispatch::file_stash::dispatch_for_principal(
                    &self.file_stash_service(),
                    &principal,
                    "mcp",
                    action,
                    params,
                    validated_grantee.as_ref(),
                )
                .await
            }
            _ => Err(ToolError::Sdk {
                sdk_kind: "service_unavailable".to_owned(),
                message: "caller-bound service adapter is unavailable".to_owned(),
            }),
        }
    }

    pub(crate) fn file_stash_service(&self) -> FileStashService {
        let page_limit = self.file_stash_runtime.page_limit();
        let max_query_bytes = self.file_stash_runtime.max_query_bytes();
        FileStashService::new(
            Arc::clone(&self.file_stash_runtime),
            Arc::clone(&self.access_runtime),
            page_limit,
            max_query_bytes,
        )
    }

    pub(crate) async fn file_stash_principal(
        &self,
        context: &RequestContext<RoleServer>,
        meta: Option<&rmcp::model::RequestMetaObject>,
    ) -> Result<ResolvedStashPrincipal, ToolError> {
        let caller = resolve_caller_authorization(
            auth_context_from_extensions(&context.extensions),
            self.absent_auth_trust(),
            propagated_caller_auth(meta),
        );
        if !caller.can_read() {
            return Err(forbidden());
        }
        if let Some(parts) = context.extensions.get::<Parts>()
            && let Some(identity) = parts.extensions.get::<labby_auth::VerifiedIdentity>()
        {
            return self
                .access_runtime
                .resolve_and_lease_file_stash_principal(identity.clone())
                .await
                .map(|(id, lease)| ResolvedStashPrincipal { id, _lease: lease })
                .map_err(|_| forbidden());
        }
        // Serialized principal IDs are trusted on only the private in-process
        // peer. Network and stdio routes must resolve a VerifiedIdentity.
        if let Some(principal) =
            propagated_file_stash_principal(self.transport_label, propagated_caller_auth(meta))
        {
            return self
                .access_runtime
                .lease_active_file_stash_principal(principal.clone())
                .await
                .map(|lease| ResolvedStashPrincipal {
                    id: principal,
                    _lease: lease,
                })
                .map_err(|_| forbidden());
        }
        Err(forbidden())
    }

    async fn file_stash_participants(
        &self,
        context: &RequestContext<RoleServer>,
        meta: Option<&rmcp::model::RequestMetaObject>,
        recipient: String,
    ) -> Result<
        (
            PrincipalId,
            PrincipalId,
            crate::access::ActiveFileStashPrincipalLease,
        ),
        ToolError,
    > {
        let caller = resolve_caller_authorization(
            auth_context_from_extensions(&context.extensions),
            self.absent_auth_trust(),
            propagated_caller_auth(meta),
        );
        if !caller.can_read() {
            return Err(forbidden());
        }
        if let Some(parts) = context.extensions.get::<Parts>()
            && let Some(identity) = parts.extensions.get::<labby_auth::VerifiedIdentity>()
        {
            return self
                .access_runtime
                .resolve_and_lease_file_stash_participants(identity.clone(), recipient)
                .await
                .map_err(|_| forbidden());
        }
        if let Some(owner) =
            propagated_file_stash_principal(self.transport_label, propagated_caller_auth(meta))
        {
            let (recipient, lease) = self
                .access_runtime
                .lease_file_stash_participants(owner.clone(), recipient)
                .await
                .map_err(|_| forbidden())?;
            return Ok((owner, recipient, lease));
        }
        Err(forbidden())
    }

    pub(crate) async fn file_stash_resources(
        &self,
        context: &RequestContext<RoleServer>,
    ) -> Result<Vec<Resource>, ErrorData> {
        let caller = resolve_caller_authorization(
            auth_context_from_extensions(&context.extensions),
            self.absent_auth_trust(),
            propagated_caller_auth(Some(&context.meta)),
        );
        // Resource listing is additive across services. A caller without the
        // Stash read scope should simply not see Stash resources; once the
        // caller is authorized, identity/storage failures remain observable.
        if !caller.can_read() {
            return Ok(Vec::new());
        }
        let verified_identity = context
            .extensions
            .get::<Parts>()
            .and_then(|parts| parts.extensions.get::<labby_auth::VerifiedIdentity>())
            .cloned();
        let has_private_principal = propagated_file_stash_principal(
            self.transport_label,
            propagated_caller_auth(Some(&context.meta)),
        )
        .is_some();
        if verified_identity.is_none() && !has_private_principal {
            return Ok(Vec::new());
        }
        crate::dispatch::file_stash::observe_result(
            "mcp",
            "stash.resources.list",
            None,
            None,
            None,
            false,
            async {
                if !self.file_stash_caller_bound()
                    || !self.route_scope.allows_service("stash")
                    || !self.service_visible_on_mcp("stash").await
                {
                    return Ok(Vec::new());
                }
                let principal = if let Some(identity) = verified_identity {
                    match self
                        .access_runtime
                        .resolve_and_lease_file_stash_principal(identity)
                        .await
                    {
                        Ok((id, lease)) => ResolvedStashPrincipal { id, _lease: lease },
                        // Resource listing is additive. A verified identity
                        // without a durable Stash principal (including a host
                        // where the access store is not configured) contributes
                        // no Stash resources without hiding other providers.
                        Err(_) => return Ok(Vec::new()),
                    }
                } else {
                    self.file_stash_principal(context, Some(&context.meta))
                        .await?
                };
                collect_file_stash_resources(
                    &self.file_stash_service(),
                    &principal,
                    self.file_stash_runtime.page_limit(),
                )
                .await
            },
        )
        .await
        .map_err(|error| list_error(&error))
    }

    pub(crate) async fn read_file_stash_resource(
        &self,
        uri: &str,
        context: &RequestContext<RoleServer>,
    ) -> Result<rmcp::model::ReadResourceResponse, ErrorData> {
        let observed_file_id = parse_stash_uri(uri).ok();
        crate::dispatch::file_stash::observe_result(
            "mcp",
            "stash.resource.read",
            observed_file_id.as_deref(),
            None,
            None,
            false,
            async {
                let file_id = parse_stash_uri(uri)?;
                let principal = self
                    .file_stash_principal(context, Some(&context.meta))
                    .await?;
                let stash = self.file_stash_service();
                let (_metadata, mut blob) = stash.open_download(&principal, &file_id, true).await?;
                let capacity = usize::try_from(blob.size).map_err(|_| ToolError::Sdk {
                    sdk_kind: "quota_exceeded".to_owned(),
                    message: "File Stash operation failed".to_owned(),
                })?;
                let mut bytes = Vec::with_capacity(capacity);
                let read_limit = blob.size.saturating_add(1);
                (&mut blob)
                    .take(read_limit)
                    .read_to_end(&mut bytes)
                    .await
                    .map_err(|_| ToolError::Sdk {
                        sdk_kind: "service_unavailable".to_owned(),
                        message: "File Stash operation failed".to_owned(),
                    })?;
                if bytes.len() != capacity {
                    return Err(ToolError::Sdk {
                        sdk_kind: "integrity_error".to_owned(),
                        message: "File Stash operation failed".to_owned(),
                    });
                }
                crate::dispatch::file_stash::capture_observation_details(
                    Some(&file_id),
                    None,
                    Some(u64::try_from(bytes.len()).unwrap_or(u64::MAX)),
                );
                let contents = ResourceContents::blob(
                    base64::engine::general_purpose::STANDARD.encode(bytes),
                    uri.to_owned(),
                )
                .with_mime_type("application/octet-stream");
                Ok(ReadResourceResult::new(vec![contents]).into())
            },
        )
        .await
        .map_err(|error| map_resource_read_error(&error, uri))
    }
}

fn list_error(error: &ToolError) -> ErrorData {
    ErrorData::internal_error(
        "File Stash resources could not be listed",
        Some(serde_json::json!({"kind": error.kind()})),
    )
}

fn map_resource_read_error(error: &ToolError, uri: &str) -> ErrorData {
    match error.kind() {
        "invalid_param" | "forbidden" | "not_found" => unknown(uri),
        "quota_exceeded" => quota_exceeded(uri),
        "busy" => busy(uri),
        _ => unavailable(uri),
    }
}

async fn collect_file_stash_resources(
    service: &FileStashService,
    principal: &PrincipalId,
    page_limit: usize,
) -> Result<Vec<Resource>, ToolError> {
    let mut cursor = None;
    let mut files = Vec::new();
    while files.len() < crate::mcp::pagination::MCP_RETAINED_LIST_ITEM_CAP {
        let remaining = crate::mcp::pagination::MCP_RETAINED_LIST_ITEM_CAP - files.len();
        let limit = remaining.min(page_limit);
        let page = service
            .list(principal, cursor.as_deref(), Some(limit))
            .await?;
        files.extend(page.files);
        let Some(next) = page.next_cursor else { break };
        if cursor.as_deref() == Some(next.as_str()) {
            return Err(ToolError::Sdk {
                sdk_kind: "invalid_cursor".to_owned(),
                message: "File Stash returned a non-advancing cursor".to_owned(),
            });
        }
        cursor = Some(next);
    }
    Ok(files.into_iter().map(resource_for_file).collect())
}

fn propagated_file_stash_principal(
    transport: &str,
    auth: Option<labby_runtime::caller_auth::PropagatedCallerAuth>,
) -> Option<PrincipalId> {
    (transport == PRIVATE_IN_PROCESS_TRANSPORT)
        .then_some(auth?)?
        .access_principal_id
        .and_then(PrincipalId::from_propagated)
}

fn resource_for_file(file: FileView) -> Resource {
    Resource::new(file.uri, file.display_name)
        .with_description(if file.owned {
            "File Stash file owned by the caller"
        } else {
            "File Stash file shared with the caller"
        })
        .with_mime_type("application/octet-stream")
        .with_size(file.size_bytes)
}

pub(crate) fn template() -> ResourceTemplate {
    ResourceTemplate::new(TEMPLATE_URI, "stash/file")
        .with_description("Caller-authorized File Stash object by opaque ULID")
        .with_mime_type("application/octet-stream")
}

fn forbidden() -> ToolError {
    ToolError::Forbidden {
        message: "File Stash requires a verified caller identity".to_owned(),
        required_scopes: vec![
            "lab:read".to_owned(),
            "lab".to_owned(),
            "lab:admin".to_owned(),
        ],
    }
}

fn unknown(uri: &str) -> ErrorData {
    ErrorData::resource_not_found(
        "File Stash resource is unavailable",
        Some(serde_json::json!({"uri": uri})),
    )
}

fn unavailable(uri: &str) -> ErrorData {
    ErrorData::internal_error(
        "File Stash resource could not be read",
        Some(serde_json::json!({"uri": uri, "kind": "service_unavailable"})),
    )
}

fn busy(uri: &str) -> ErrorData {
    ErrorData::internal_error(
        "File Stash resource capacity is busy",
        Some(serde_json::json!({"uri": uri, "kind": "busy"})),
    )
}

fn quota_exceeded(uri: &str) -> ErrorData {
    ErrorData::invalid_request(
        "File Stash resource exceeds the MCP read limit",
        Some(serde_json::json!({"uri": uri, "kind": "quota_exceeded"})),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use labby_runtime::caller_auth::PropagatedCallerAuth;

    #[test]
    fn propagated_principal_is_honored_only_on_private_in_process_transport() {
        let propagated = PropagatedCallerAuth::scoped(vec!["lab:read".into()], Some("sub".into()))
            .with_access_principal_id("principal-1".into());
        assert_eq!(
            propagated_file_stash_principal("in-process", Some(propagated.clone()))
                .map(|value| value.as_str().to_owned()),
            Some("principal-1".into())
        );
        assert!(propagated_file_stash_principal("http", Some(propagated.clone())).is_none());
        assert!(propagated_file_stash_principal("stdio", Some(propagated)).is_none());
        assert!(propagated_file_stash_principal("in-process", None).is_none());
    }

    #[test]
    fn malformed_and_noncanonical_stash_uris_are_rejected_before_dispatch() {
        for uri in [
            "stash://me/files/not-an-id",
            "stash://me/files/01arz3ndektsv4rrffq69g5fav",
            "stash://other/files/01ARZ3NDEKTSV4RRFFQ69G5FAV",
            "stash://me/files/01ARZ3NDEKTSV4RRFFQ69G5FAV/extra",
        ] {
            assert!(parse_stash_uri(uri).is_err(), "{uri}");
        }
    }

    #[test]
    fn oversized_resource_preserves_quota_exceeded_kind() {
        let error = quota_exceeded("stash://me/files/01ARZ3NDEKTSV4RRFFQ69G5FAV");
        assert_eq!(
            error.data.as_ref().and_then(|data| data["kind"].as_str()),
            Some("quota_exceeded")
        );
    }

    #[test]
    fn resource_read_validation_auth_and_absence_are_non_enumerating() {
        let uri = "stash://me/files/01ARZ3NDEKTSV4RRFFQ69G5FAV";
        let errors = [
            ToolError::InvalidParam {
                param: "uri".to_owned(),
                message: "invalid File Stash parameter".to_owned(),
            },
            forbidden(),
            ToolError::Sdk {
                sdk_kind: "not_found".to_owned(),
                message: "File Stash operation failed".to_owned(),
            },
        ];
        for error in errors {
            let response = map_resource_read_error(&error, uri);
            assert_eq!(response.code, rmcp::model::ErrorCode::RESOURCE_NOT_FOUND);
            assert_eq!(response.message, "File Stash resource is unavailable");
            assert_eq!(
                response.data.as_ref().and_then(|data| data["uri"].as_str()),
                Some(uri)
            );
        }
    }

    #[cfg(target_os = "linux")]
    #[tokio::test]
    async fn resource_snapshot_walks_beyond_the_service_page_limit() {
        use std::os::unix::fs::PermissionsExt as _;
        let directory = tempfile::tempdir().expect("temporary directory");
        std::fs::set_permissions(directory.path(), std::fs::Permissions::from_mode(0o700))
            .expect("secure temporary directory");
        let preferences = crate::config::FileStashPreferences {
            page_size: 2,
            ..crate::config::FileStashPreferences::default()
        };
        let runtime = Arc::new(
            crate::file_stash::FileStashRuntime::initialize_with_preferences(
                directory.path().join("stash"),
                preferences,
            )
            .await,
        );
        let store = runtime.store().await.expect("ready stash store");
        for index in 0..3 {
            let reservation = store
                .reserve_upload(
                    "principal-1".into(),
                    format!("file-{index}"),
                    format!("file-{index}"),
                    0,
                    i64::MAX,
                    u64::MAX,
                    u64::MAX,
                    10,
                )
                .await
                .expect("reserve");
            store
                .mark_blob_published(reservation.upload_id.clone())
                .await
                .expect("publish");
            store
                .commit_upload(reservation.upload_id)
                .await
                .expect("commit");
        }
        let service = FileStashService::new(
            Arc::clone(&runtime),
            Arc::new(crate::access::AccessRuntime::blocked_unavailable()),
            2,
            64,
        );
        let principal = PrincipalId::from_propagated("principal-1".into()).expect("principal");
        let resources = collect_file_stash_resources(&service, &principal, 2)
            .await
            .expect("resources");
        assert_eq!(resources.len(), 3, "must not freeze the first service page");
        runtime.shutdown().await;
    }
}
