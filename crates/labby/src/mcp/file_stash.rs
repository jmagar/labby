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
/// Request-scoped Stash owner selection. This value chooses a scope; it grants
/// no authority and is always re-evaluated against the verified caller.
pub(crate) const OWNER_META_KEY: &str = "ai.dinglebear.labby/stashOwner";
const PRIVATE_IN_PROCESS_TRANSPORT: &str = "in-process";

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct StashOwnerSelection {
    kind: String,
    #[serde(default)]
    id: Option<String>,
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
        _meta: Option<&rmcp::model::RequestMetaObject>,
    ) -> Result<serde_json::Value, ToolError> {
        match service {
            "agents" | "tasks" => {
                let identity = context
                    .extensions
                    .get::<labby_auth::VerifiedIdentity>()
                    .cloned()
                    .or_else(|| {
                        context
                            .extensions
                            .get::<Parts>()
                            .and_then(|parts| {
                                parts.extensions.get::<labby_auth::VerifiedIdentity>()
                            })
                            .cloned()
                    })
                    .ok_or_else(forbidden)?;
                let store = self.access_runtime.store().await.map_err(|_| forbidden())?;
                let auth = auth_context_from_extensions(&context.extensions);
                let ceiling = auth.map_or_else(
                    crate::access::AuthorityCeiling::trusted_local,
                    crate::access::AuthorityCeiling::from_auth_context,
                );
                if service == "agents" {
                    crate::dispatch::agents::dispatch(
                        crate::dispatch::agents::AgentDispatchContext {
                            store,
                            identity,
                            ceiling,
                        },
                        action,
                        params,
                    )
                    .await
                } else {
                    crate::dispatch::tasks::dispatch(
                        crate::dispatch::tasks::TaskDispatchContext {
                            store,
                            identity,
                            ceiling,
                        },
                        action,
                        params,
                    )
                    .await
                }
            }
            "stash" => {
                let identity = context
                    .extensions
                    .get::<labby_auth::VerifiedIdentity>()
                    .cloned()
                    .or_else(|| {
                        context
                            .extensions
                            .get::<Parts>()
                            .and_then(|p| p.extensions.get::<labby_auth::VerifiedIdentity>())
                            .cloned()
                    })
                    .ok_or_else(forbidden)?;
                let owner =
                    stash_owner_from_params(&params, &identity, &self.access_runtime).await?;
                let capability = stash_capability(action);
                let ceiling = auth_context_from_extensions(&context.extensions).map_or_else(
                    crate::access::AuthorityCeiling::trusted_local,
                    crate::access::AuthorityCeiling::from_auth_context,
                );
                let principal = self
                    .access_runtime
                    .authorize_file_stash_owner(
                        identity,
                        ceiling,
                        owner,
                        action,
                        capability,
                        unix_millis(),
                    )
                    .await
                    .map_err(|_| forbidden())?;
                principal
                    .validate_before_commit()
                    .await
                    .map_err(|_| forbidden())?;
                crate::dispatch::file_stash::dispatch_for_principal(
                    &self.file_stash_service(),
                    &principal,
                    "mcp",
                    action,
                    params,
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
    ) -> Result<AuthorizedStashPrincipal, ToolError> {
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
            let owner = if let Some(owner) =
                selected_stash_owner(meta, identity, &self.access_runtime).await?
            {
                owner
            } else {
                use labby_primitives::access::OwnerScope;
                let principal = self
                    .access_runtime
                    .resolve_file_stash_principal(identity.clone())
                    .await
                    .map_err(|_| forbidden())?;
                OwnerScope::Personal(
                    labby_primitives::access::PrincipalId::new(principal.as_str())
                        .map_err(|_| forbidden())?,
                )
            };
            let ceiling = auth_context_from_extensions(&context.extensions).map_or_else(
                crate::access::AuthorityCeiling::trusted_local,
                crate::access::AuthorityCeiling::from_auth_context,
            );
            return self
                .access_runtime
                .authorize_file_stash_owner(
                    identity.clone(),
                    ceiling,
                    owner,
                    "stash.resources.read",
                    labby_primitives::access::Capability::ScopeRead,
                    unix_millis(),
                )
                .await
                .map(AuthorizedStashPrincipal::sealed)
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
                .map(|_| AuthorizedStashPrincipal::trusted(principal))
                .map_err(|_| forbidden());
        }
        Err(forbidden())
    }

    pub(crate) async fn file_stash_resources(
        &self,
        context: &RequestContext<RoleServer>,
    ) -> Vec<Resource> {
        if !self.file_stash_caller_bound()
            || !self.route_scope.allows_service("stash")
            || !self.service_visible_on_mcp("stash").await
        {
            return Vec::new();
        }
        let Ok(principal) = self
            .file_stash_principal(context, Some(&context.meta))
            .await
        else {
            return Vec::new();
        };
        if principal.validate_before_commit().await.is_err() {
            return Vec::new();
        }
        collect_file_stash_resources(
            &self.file_stash_service(),
            &principal,
            self.file_stash_runtime.page_limit(),
        )
        .await
        .unwrap_or_default()
    }

    pub(crate) async fn read_file_stash_resource(
        &self,
        uri: &str,
        context: &RequestContext<RoleServer>,
    ) -> Result<rmcp::model::ReadResourceResponse, ErrorData> {
        let file_id = parse_stash_uri(uri).map_err(|_| unknown(uri))?;
        let principal = self
            .file_stash_principal(context, Some(&context.meta))
            .await
            .map_err(|_| unknown(uri))?;
        let (_metadata, mut blob) = self
            .file_stash_service()
            .open_download(&principal, &file_id, true)
            .await
            .map_err(|error| match error.kind() {
                "quota_exceeded" => quota_exceeded(uri),
                "not_found" => unknown(uri),
                "busy" => busy(uri),
                _ => unavailable(uri),
            })?;
        principal
            .validate_before_commit()
            .await
            .map_err(|_| unknown(uri))?;
        let capacity = usize::try_from(blob.size).map_err(|_| quota_exceeded(uri))?;
        let mut bytes = Vec::with_capacity(capacity);
        (&mut blob.file)
            .take(blob.size.saturating_add(1))
            .read_to_end(&mut bytes)
            .await
            .map_err(|_| unavailable(uri))?;
        if bytes.len() != capacity {
            return Err(unavailable(uri));
        }
        let contents = ResourceContents::blob(
            base64::engine::general_purpose::STANDARD.encode(bytes),
            uri.to_owned(),
        )
        .with_mime_type("application/octet-stream");
        Ok(ReadResourceResult::new(vec![contents]).into())
    }
}

pub(crate) struct AuthorizedStashPrincipal {
    principal: PrincipalId,
    authority: Option<crate::access::FileStashOwnerAuthorization>,
}

impl AuthorizedStashPrincipal {
    fn sealed(authority: crate::access::FileStashOwnerAuthorization) -> Self {
        Self {
            principal: (*authority).clone(),
            authority: Some(authority),
        }
    }

    fn trusted(principal: PrincipalId) -> Self {
        Self {
            principal,
            authority: None,
        }
    }

    async fn validate_before_commit(&self) -> Result<(), ToolError> {
        if let Some(authority) = &self.authority {
            authority
                .validate_before_commit()
                .await
                .map_err(|_| forbidden())?;
        }
        Ok(())
    }
}

impl std::ops::Deref for AuthorizedStashPrincipal {
    type Target = PrincipalId;

    fn deref(&self) -> &Self::Target {
        &self.principal
    }
}

async fn selected_stash_owner(
    meta: Option<&rmcp::model::RequestMetaObject>,
    identity: &labby_auth::VerifiedIdentity,
    runtime: &crate::access::AccessRuntime,
) -> Result<Option<labby_primitives::access::OwnerScope>, ToolError> {
    use labby_primitives::access::{OwnerScope, PrincipalId, TeamId};
    let Some(value) = meta.and_then(|meta| meta.get(OWNER_META_KEY)) else {
        return Ok(None);
    };
    let selection: StashOwnerSelection =
        serde_json::from_value(value.clone()).map_err(|_| forbidden())?;
    match selection.kind.as_str() {
        "personal" if selection.id.is_none() => {
            let principal = runtime
                .resolve_file_stash_principal(identity.clone())
                .await
                .map_err(|_| forbidden())?;
            Ok(Some(OwnerScope::Personal(
                PrincipalId::new(principal.as_str()).map_err(|_| forbidden())?,
            )))
        }
        "team" => Ok(Some(OwnerScope::Team(
            TeamId::new(selection.id.as_deref().ok_or_else(forbidden)?).map_err(|_| forbidden())?,
        ))),
        _ => Err(forbidden()),
    }
}

async fn stash_owner_from_params(
    params: &serde_json::Value,
    identity: &labby_auth::VerifiedIdentity,
    runtime: &crate::access::AccessRuntime,
) -> Result<labby_primitives::access::OwnerScope, ToolError> {
    use labby_primitives::access::{OwnerScope, PrincipalId, TeamId};
    match params.get("owner_kind").and_then(serde_json::Value::as_str) {
        None | Some("personal") => {
            let principal = runtime
                .resolve_file_stash_principal(identity.clone())
                .await
                .map_err(|_| forbidden())?;
            Ok(OwnerScope::Personal(
                PrincipalId::new(principal.as_str()).map_err(|_| forbidden())?,
            ))
        }
        Some("team") => Ok(OwnerScope::Team(
            TeamId::new(
                params
                    .get("owner_id")
                    .and_then(serde_json::Value::as_str)
                    .ok_or_else(forbidden)?,
            )
            .map_err(|_| forbidden())?,
        )),
        _ => Err(forbidden()),
    }
}
fn stash_capability(action: &str) -> labby_primitives::access::Capability {
    use labby_primitives::access::Capability;
    match action {
        "stash.list" | "stash.search" | "stash.stats" | "stash.metadata" => Capability::ScopeRead,
        "stash.delete" => Capability::ScopeDelete,
        "stash.rename" | "stash.grants.create" | "stash.grants.list" | "stash.grants.revoke" => {
            Capability::ScopeManage
        }
        _ => Capability::ScopeRead,
    }
}
fn unix_millis() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |v| u64::try_from(v.as_millis()).unwrap_or(u64::MAX))
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

    #[cfg(any(target_os = "linux", target_os = "android"))]
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
