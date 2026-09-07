//! Surface-neutral operations and action metadata for principal-scoped File Stash.
// Transport adapters are wired in dependent beads; keep this shared service
// independently reviewable until those callers land.
#![allow(dead_code)]
use crate::dispatch::helpers::{action_schema, help_payload, require_str};
use crate::{
    access::AccessRuntime,
    dispatch::error::ToolError,
    file_stash::{
        FileStashRuntime, FileStashStoreError, OpenedBlob, PrincipalId, StashCursor, StashFile,
        StashGrant, StashUsage, UploadAdmission, UploadReservation,
    },
};
use labby_primitives::action::{ActionSpec, ParamSpec};
use serde::Serialize;
use serde_json::Value;
use std::sync::Arc;
use tokio::io::AsyncRead;
use tokio_util::sync::CancellationToken;
use unicode_casefold::UnicodeCaseFold;
use unicode_normalization::UnicodeNormalization;

pub const META: (&str, &str, &str) = (
    "stash",
    "Store and share principal-scoped files",
    "bootstrap",
);

pub(crate) fn observe_operation(
    surface: &'static str,
    action: &str,
    result: &'static str,
    object_id: Option<&str>,
    grant_id: Option<&str>,
    byte_count: Option<u64>,
    destructive: bool,
) {
    tracing::info!(
        surface,
        service = "stash",
        action,
        result,
        object_id,
        grant_id,
        byte_count,
        destructive,
        "file stash operation"
    );
}

pub const ACTIONS: &[ActionSpec] = &[
    action(
        "stash.list",
        "List files available to the caller",
        false,
        LIST_PARAMS,
        "FilePage",
    ),
    action(
        "stash.search",
        "Search available files by display name",
        false,
        SEARCH_PARAMS,
        "FilePage",
    ),
    action(
        "stash.stats",
        "Read authoritative owned-file usage",
        false,
        OWNER_PARAMS,
        "StashStats",
    ),
    action(
        "stash.metadata",
        "Read metadata for an available file",
        false,
        FILE_PARAM,
        "StashFile",
    ),
    action(
        "stash.rename",
        "Rename an owned file",
        false,
        RENAME_PARAMS,
        "StashFile",
    ),
    action(
        "stash.delete",
        "Permanently delete an owned file",
        true,
        FILE_PARAM,
        "Deleted",
    ),
    action(
        "stash.grants.create",
        "Grant one principal read access to an owned file",
        false,
        GRANT_CREATE_PARAMS,
        "StashGrant",
    ),
    action(
        "stash.grants.list",
        "List active grants for an owned file",
        false,
        GRANT_LIST_PARAMS,
        "GrantPage",
    ),
    action(
        "stash.grants.revoke",
        "Revoke an active grant on an owned file",
        false,
        GRANT_REVOKE_PARAMS,
        "Revoked",
    ),
];

const fn action(
    name: &'static str,
    description: &'static str,
    destructive: bool,
    params: &'static [ParamSpec],
    returns: &'static str,
) -> ActionSpec {
    ActionSpec {
        name,
        description,
        destructive,
        requires_admin: false,
        params,
        returns,
    }
}
const OWNER_KIND_PARAM: ParamSpec = ParamSpec {
    name: "owner_kind",
    ty: "personal|team",
    required: false,
    description: "Explicit owner scope; defaults to personal",
};
const OWNER_ID_PARAM: ParamSpec = ParamSpec {
    name: "owner_id",
    ty: "string",
    required: false,
    description: "Required with Team ownership",
};
const OWNER_PARAMS: &[ParamSpec] = &[OWNER_KIND_PARAM, OWNER_ID_PARAM];
const FILE_PARAM: &[ParamSpec] = &[
    ParamSpec {
        name: "file_id",
        ty: "string",
        required: true,
        description: "Opaque immutable file ID",
    },
    OWNER_KIND_PARAM,
    OWNER_ID_PARAM,
];
const LIST_PARAMS: &[ParamSpec] = &[
    ParamSpec {
        name: "cursor",
        ty: "string",
        required: false,
        description: "Opaque continuation cursor",
    },
    ParamSpec {
        name: "limit",
        ty: "integer",
        required: false,
        description: "Page size from 1 to the configured maximum",
    },
    OWNER_KIND_PARAM,
    OWNER_ID_PARAM,
];
const SEARCH_PARAMS: &[ParamSpec] = &[
    ParamSpec {
        name: "query",
        ty: "string",
        required: true,
        description: "Case-insensitive display-name substring",
    },
    ParamSpec {
        name: "cursor",
        ty: "string",
        required: false,
        description: "Opaque continuation cursor",
    },
    ParamSpec {
        name: "limit",
        ty: "integer",
        required: false,
        description: "Page size from 1 to the configured maximum",
    },
    OWNER_KIND_PARAM,
    OWNER_ID_PARAM,
];
const RENAME_PARAMS: &[ParamSpec] = &[
    ParamSpec {
        name: "file_id",
        ty: "string",
        required: true,
        description: "Opaque immutable file ID",
    },
    ParamSpec {
        name: "display_name",
        ty: "string",
        required: true,
        description: "New display filename",
    },
    OWNER_KIND_PARAM,
    OWNER_ID_PARAM,
];
const GRANT_CREATE_PARAMS: &[ParamSpec] = &[
    ParamSpec {
        name: "file_id",
        ty: "string",
        required: true,
        description: "Opaque immutable file ID",
    },
    ParamSpec {
        name: "grantee_principal_id",
        ty: "string",
        required: true,
        description: "AccessStore-resolved durable PrincipalId",
    },
    OWNER_KIND_PARAM,
    OWNER_ID_PARAM,
];
const GRANT_LIST_PARAMS: &[ParamSpec] = &[
    ParamSpec {
        name: "file_id",
        ty: "string",
        required: true,
        description: "Opaque immutable file ID",
    },
    ParamSpec {
        name: "cursor",
        ty: "string",
        required: false,
        description: "Opaque continuation cursor",
    },
    ParamSpec {
        name: "limit",
        ty: "integer",
        required: false,
        description: "Page size from 1 to the configured maximum",
    },
    OWNER_KIND_PARAM,
    OWNER_ID_PARAM,
];
const GRANT_REVOKE_PARAMS: &[ParamSpec] = &[
    ParamSpec {
        name: "file_id",
        ty: "string",
        required: true,
        description: "Opaque immutable file ID",
    },
    ParamSpec {
        name: "grant_id",
        ty: "string",
        required: true,
        description: "Opaque grant ID",
    },
    OWNER_KIND_PARAM,
    OWNER_ID_PARAM,
];

#[derive(Clone)]
pub(crate) struct FileStashService {
    runtime: Arc<FileStashRuntime>,
    access_runtime: Arc<AccessRuntime>,
    page_limit: usize,
    max_query_bytes: usize,
}

#[derive(Debug, Serialize)]
pub(crate) struct FileView {
    pub file_id: String,
    pub uri: String,
    pub display_name: String,
    pub size_bytes: u64,
    pub created_at: i64,
    pub updated_at: i64,
    pub owned: bool,
}
#[derive(Debug, Serialize)]
pub(crate) struct FilePage {
    pub files: Vec<FileView>,
    pub next_cursor: Option<String>,
}
#[derive(Debug, Serialize)]
pub(crate) struct GrantView {
    pub grant_id: String,
    pub file_id: String,
    pub grantee_principal_id: String,
    pub created_at: i64,
}
#[derive(Debug, Serialize)]
pub(crate) struct GrantPage {
    pub grants: Vec<GrantView>,
    pub next_cursor: Option<String>,
}
#[derive(Debug, Serialize)]
pub(crate) struct StatsView {
    pub owned_file_count: u64,
    pub owned_shared_file_count: u64,
    pub owned_committed_bytes: u64,
    pub owned_reserved_bytes: u64,
}

impl From<StashFile> for FileView {
    fn from(file: StashFile) -> Self {
        Self {
            uri: file.uri(),
            file_id: file.file_id,
            display_name: file.display_name,
            size_bytes: file.size_bytes,
            created_at: file.created_at,
            updated_at: file.updated_at,
            owned: file.owned,
        }
    }
}
impl From<StashGrant> for GrantView {
    fn from(g: StashGrant) -> Self {
        Self {
            grant_id: g.grant_id,
            file_id: g.file_id,
            grantee_principal_id: g.grantee_principal_id,
            created_at: g.created_at,
        }
    }
}
impl From<StashUsage> for StatsView {
    fn from(s: StashUsage) -> Self {
        Self {
            owned_file_count: s.live_files,
            owned_shared_file_count: s.owned_shared_file_count,
            owned_committed_bytes: s.committed_bytes,
            owned_reserved_bytes: s.reserved_bytes,
        }
    }
}

impl FileStashService {
    pub(crate) fn new(
        runtime: Arc<FileStashRuntime>,
        access_runtime: Arc<AccessRuntime>,
        page_limit: usize,
        max_query_bytes: usize,
    ) -> Self {
        Self {
            runtime,
            access_runtime,
            page_limit: page_limit.clamp(1, 200),
            max_query_bytes: max_query_bytes.clamp(1, 1_024),
        }
    }
    async fn stores(
        &self,
    ) -> Result<
        (
            crate::file_stash::FileStashStore,
            crate::file_stash::BlobStore,
        ),
        ToolError,
    > {
        Ok((
            self.runtime
                .store()
                .await
                .map_err(|_| service_error("service_unavailable", "File Stash is unavailable"))?,
            self.runtime
                .blob_store()
                .await
                .map_err(|_| service_error("service_unavailable", "File Stash is unavailable"))?,
        ))
    }
    pub(crate) async fn list(
        &self,
        principal: &PrincipalId,
        cursor: Option<&str>,
        limit: Option<usize>,
    ) -> Result<FilePage, ToolError> {
        self.list_inner(principal, None, cursor, limit).await
    }
    pub(crate) async fn search(
        &self,
        principal: &PrincipalId,
        query: &str,
        cursor: Option<&str>,
        limit: Option<usize>,
    ) -> Result<FilePage, ToolError> {
        if query.is_empty() || query.len() > self.max_query_bytes {
            return Err(invalid("query"));
        }
        self.list_inner(principal, Some(search_key(query)), cursor, limit)
            .await
    }
    async fn list_inner(
        &self,
        principal: &PrincipalId,
        query: Option<String>,
        cursor: Option<&str>,
        limit: Option<usize>,
    ) -> Result<FilePage, ToolError> {
        let limit = validated_limit(limit, self.page_limit)?;
        let cursor = cursor.map(parse_cursor).transpose()?;
        let (store, _) = self.stores().await?;
        let mut rows = store
            .list_files(principal.as_str().to_owned(), query, cursor, limit + 1)
            .await
            .map_err(map_error)?;
        let next_cursor = if rows.len() > limit {
            rows.truncate(limit);
            rows.last().map(cursor_for)
        } else {
            None
        };
        Ok(FilePage {
            files: rows.into_iter().map(Into::into).collect(),
            next_cursor,
        })
    }
    pub(crate) async fn stats(&self, principal: &PrincipalId) -> Result<StatsView, ToolError> {
        let (store, _) = self.stores().await?;
        store
            .usage(principal.as_str().to_owned())
            .await
            .map(Into::into)
            .map_err(map_error)
    }
    pub(crate) async fn metadata(
        &self,
        principal: &PrincipalId,
        file_id: &str,
    ) -> Result<FileView, ToolError> {
        validate_id(file_id, "file_id")?;
        let (store, _) = self.stores().await?;
        store
            .authorized_file(principal.as_str().to_owned(), file_id.to_owned())
            .await
            .map(Into::into)
            .map_err(map_error)
    }
    pub(crate) async fn rename(
        &self,
        principal: &PrincipalId,
        file_id: &str,
        name: &str,
    ) -> Result<FileView, ToolError> {
        validate_id(file_id, "file_id")?;
        let (name, key) = normalize_name(name)?;
        let (store, _) = self.stores().await?;
        store
            .rename_file(principal.as_str().to_owned(), file_id.to_owned(), name, key)
            .await
            .map(Into::into)
            .map_err(map_error)
    }
    pub(crate) async fn delete(
        &self,
        principal: &PrincipalId,
        file_id: &str,
    ) -> Result<(), ToolError> {
        validate_id(file_id, "file_id")?;
        let (store, blobs) = self.stores().await?;
        let key = store
            .delete_file(principal.as_str().to_owned(), file_id.to_owned())
            .await
            .map_err(map_error)?;
        if let Err(error) = blobs.remove_blob(&key) {
            tracing::warn!(error_kind=%error,"file stash deleted metadata; blob reclamation deferred");
        }
        Ok(())
    }
    pub(crate) async fn create_grant(
        &self,
        owner: &PrincipalId,
        file_id: &str,
        grantee: &PrincipalId,
    ) -> Result<GrantView, ToolError> {
        validate_id(file_id, "file_id")?;
        let _recipient_lease = self
            .access_runtime
            .lease_active_file_stash_principal(grantee.clone())
            .await
            .map_err(|_| service_error("not_found", "File Stash operation failed"))?;
        let (store, _) = self.stores().await?;
        store
            .create_grant(
                owner.as_str().to_owned(),
                file_id.to_owned(),
                grantee.as_str().to_owned(),
            )
            .await
            .map(Into::into)
            .map_err(map_error)
    }
    pub(crate) async fn create_grant_for_recipient_id(
        &self,
        owner: &PrincipalId,
        file_id: &str,
        recipient_id: String,
    ) -> Result<GrantView, ToolError> {
        validate_id(file_id, "file_id")?;
        let (grantee, _recipient_lease) = self
            .access_runtime
            .resolve_active_file_stash_recipient(recipient_id)
            .await
            .map_err(|_| service_error("not_found", "File Stash operation failed"))?;
        let (store, _) = self.stores().await?;
        store
            .create_grant(
                owner.as_str().to_owned(),
                file_id.to_owned(),
                grantee.as_str().to_owned(),
            )
            .await
            .map(Into::into)
            .map_err(map_error)
    }
    pub(crate) async fn revoke_grant(
        &self,
        owner: &PrincipalId,
        file_id: &str,
        grant_id: &str,
    ) -> Result<(), ToolError> {
        validate_id(file_id, "file_id")?;
        validate_id(grant_id, "grant_id")?;
        let (store, _) = self.stores().await?;
        store
            .revoke_grant(
                owner.as_str().to_owned(),
                file_id.to_owned(),
                grant_id.to_owned(),
            )
            .await
            .map_err(map_error)
    }
    pub(crate) async fn grants(
        &self,
        owner: &PrincipalId,
        file_id: &str,
        cursor: Option<&str>,
        limit: Option<usize>,
    ) -> Result<GrantPage, ToolError> {
        validate_id(file_id, "file_id")?;
        let limit = validated_limit(limit, self.page_limit)?;
        let after = cursor.unwrap_or_default();
        if !after.is_empty() {
            validate_id(after, "cursor")?
        };
        let (store, _) = self.stores().await?;
        let mut rows = store
            .list_grants(
                owner.as_str().to_owned(),
                file_id.to_owned(),
                after.to_owned(),
                limit + 1,
            )
            .await
            .map_err(map_error)?;
        let next_cursor = if rows.len() > limit {
            rows.truncate(limit);
            rows.last().map(|g| g.grant_id.clone())
        } else {
            None
        };
        Ok(GrantPage {
            grants: rows.into_iter().map(Into::into).collect(),
            next_cursor,
        })
    }
    pub(crate) async fn reserve_upload(
        &self,
        owner: &PrincipalId,
        name: &str,
        bytes: u64,
    ) -> Result<(UploadReservation, UploadAdmission), ToolError> {
        let (name, key) = normalize_name(name)?;
        let (_, blobs) = self.stores().await?;
        blobs
            .reserve(owner, name, key, bytes)
            .await
            .map_err(map_error)
    }
    pub(crate) async fn finalize_upload<R: AsyncRead + Unpin>(
        &self,
        reservation: UploadReservation,
        admission: UploadAdmission,
        reader: R,
        cancel: CancellationToken,
    ) -> Result<String, ToolError> {
        let (_, blobs) = self.stores().await?;
        blobs
            .write_reserved(reservation, admission, reader, cancel)
            .await
            .map_err(map_error)
    }
    pub(crate) async fn open_download(
        &self,
        principal: &PrincipalId,
        file_id: &str,
        mcp: bool,
    ) -> Result<(FileView, OpenedBlob), ToolError> {
        validate_id(file_id, "file_id")?;
        let (store, blobs) = self.stores().await?;
        let file = store
            .authorized_file(principal.as_str().to_owned(), file_id.to_owned())
            .await
            .map_err(map_error)?;
        let opened = blobs
            .open_blob(&file.blob_key, file.size_bytes, mcp)
            .await
            .map_err(map_error)?;
        // Authorization after the regular-file handle is open is the
        // linearization point. If revoke/delete won, this second read denies
        // and `opened` is dropped; otherwise the snapshot may finish.
        let authorized = store
            .authorized_file(principal.as_str().to_owned(), file_id.to_owned())
            .await
            .map_err(map_error)?;
        Ok((authorized.into(), opened))
    }
}

pub(crate) async fn dispatch_for_principal(
    service: &FileStashService,
    principal: &PrincipalId,
    surface: &'static str,
    action: &str,
    params: Value,
) -> Result<Value, ToolError> {
    let object_id = params
        .get("file_id")
        .and_then(Value::as_str)
        .map(str::to_owned);
    let grant_id = params
        .get("grant_id")
        .and_then(Value::as_str)
        .map(str::to_owned);
    let object = params.as_object();
    let string = |name: &str| {
        object
            .and_then(|value| value.get(name))
            .and_then(Value::as_str)
            .ok_or_else(|| ToolError::MissingParam {
                message: format!("missing required parameter `{name}`"),
                param: name.to_owned(),
            })
    };
    let optional_string = |name: &str| {
        object
            .and_then(|value| value.get(name))
            .map(|value| value.as_str().ok_or_else(|| invalid(name)))
            .transpose()
    };
    let optional_limit = || {
        object
            .and_then(|value| value.get("limit"))
            .map(|value| {
                value
                    .as_u64()
                    .and_then(|value| usize::try_from(value).ok())
                    .ok_or_else(|| invalid("limit"))
            })
            .transpose()
    };
    let result = async {
        let value = match action {
            "stash.list" => serde_json::to_value(
                service
                    .list(principal, optional_string("cursor")?, optional_limit()?)
                    .await?,
            ),
            "stash.search" => serde_json::to_value(
                service
                    .search(
                        principal,
                        string("query")?,
                        optional_string("cursor")?,
                        optional_limit()?,
                    )
                    .await?,
            ),
            "stash.stats" => serde_json::to_value(service.stats(principal).await?),
            "stash.metadata" => {
                serde_json::to_value(service.metadata(principal, string("file_id")?).await?)
            }
            "stash.rename" => serde_json::to_value(
                service
                    .rename(principal, string("file_id")?, string("display_name")?)
                    .await?,
            ),
            "stash.delete" => {
                service.delete(principal, string("file_id")?).await?;
                Ok(serde_json::json!({"deleted": true}))
            }
            "stash.grants.create" => serde_json::to_value(
                service
                    .create_grant(
                        principal,
                        string("file_id")?,
                        &PrincipalId::from_propagated(string("grantee_principal_id")?.to_owned())
                            .ok_or_else(|| invalid("grantee_principal_id"))?,
                    )
                    .await?,
            ),
            "stash.grants.list" => serde_json::to_value(
                service
                    .grants(
                        principal,
                        string("file_id")?,
                        optional_string("cursor")?,
                        optional_limit()?,
                    )
                    .await?,
            ),
            "stash.grants.revoke" => {
                service
                    .revoke_grant(principal, string("file_id")?, string("grant_id")?)
                    .await?;
                Ok(serde_json::json!({"revoked": true}))
            }
            _ => {
                return Err(ToolError::UnknownAction {
                    message: format!("unknown File Stash action `{action}`"),
                    valid: ACTIONS.iter().map(|item| item.name.to_owned()).collect(),
                    hint: None,
                });
            }
        };
        value.map_err(|_| service_error("internal_error", "File Stash response failed"))
    }
    .await;
    observe_operation(
        surface,
        action,
        if result.is_ok() { "success" } else { "error" },
        object_id.as_deref(),
        grant_id.as_deref(),
        None,
        action == "stash.delete",
    );
    result
}

/// Context-free entrypoint used by catalog machinery. Caller-bound actions are
/// deliberately unavailable here and are routed by the MCP/HTTP adapters only
/// after they resolve an AccessStore principal.
pub(crate) async fn dispatch(action: &str, params: Value) -> Result<Value, ToolError> {
    match action {
        "help" => Ok(help_payload("stash", ACTIONS)),
        "schema" => action_schema(ACTIONS, require_str(&params, "action")?),
        _ => Err(ToolError::Forbidden {
            message: "File Stash requires a resolved caller identity".to_owned(),
            required_scopes: vec![
                "lab:read".to_owned(),
                "lab".to_owned(),
                "lab:admin".to_owned(),
            ],
        }),
    }
}

fn validated_limit(limit: Option<usize>, max: usize) -> Result<usize, ToolError> {
    let n = limit.unwrap_or(max);
    if n == 0 || n > max {
        Err(invalid("limit"))
    } else {
        Ok(n)
    }
}
fn validate_id(value: &str, param: &str) -> Result<(), ToolError> {
    if value.len() == 26 && value.bytes().all(|b| b.is_ascii_alphanumeric()) {
        Ok(())
    } else {
        Err(invalid(param))
    }
}
fn normalize_name(raw: &str) -> Result<(String, String), ToolError> {
    let leaf = raw.rsplit(['/', '\\']).next().unwrap_or_default();
    let name: String = leaf.nfc().collect();
    if name.is_empty()
        || name == "."
        || name == ".."
        || name.len() > 255
        || name
            .chars()
            .any(|c| c.is_control() || matches!(c, '/' | '\\' | '\0'))
    {
        return Err(invalid("display_name"));
    }
    let key = search_key(&name);
    Ok((name, key))
}
fn search_key(value: &str) -> String {
    value.nfc().case_fold().collect::<String>().nfc().collect()
}

pub(crate) fn parse_stash_uri(uri: &str) -> Result<String, ToolError> {
    const PREFIX: &str = "stash://me/files/";
    let Some(file_id) = uri.strip_prefix(PREFIX) else {
        return Err(invalid("uri"));
    };
    validate_id(file_id, "uri")?;
    let parsed = ulid::Ulid::from_string(file_id).map_err(|_| invalid("uri"))?;
    if parsed.to_string() != file_id {
        return Err(invalid("uri"));
    }
    Ok(file_id.to_owned())
}
fn cursor_for(f: &StashFile) -> String {
    format!("{}.{}", f.created_at, f.file_id)
}
fn parse_cursor(raw: &str) -> Result<StashCursor, ToolError> {
    let Some((created, id)) = raw.split_once('.') else {
        return Err(invalid("cursor"));
    };
    validate_id(id, "cursor")?;
    Ok(StashCursor {
        created_at: created.parse().map_err(|_| invalid("cursor"))?,
        id: id.to_owned(),
    })
}
fn invalid(param: &str) -> ToolError {
    ToolError::InvalidParam {
        param: param.to_owned(),
        message: "invalid File Stash parameter".to_owned(),
    }
}
fn service_error(kind: &str, message: &str) -> ToolError {
    ToolError::Sdk {
        sdk_kind: kind.to_owned(),
        message: message.to_owned(),
    }
}
fn map_error(error: FileStashStoreError) -> ToolError {
    let kind = match error {
        FileStashStoreError::NotFound => "not_found",
        FileStashStoreError::Conflict => "conflict",
        FileStashStoreError::QuotaExceeded => "quota_exceeded",
        FileStashStoreError::LengthMismatch => "invalid_param",
        FileStashStoreError::Busy => "busy",
        FileStashStoreError::Integrity
        | FileStashStoreError::Corrupt
        | FileStashStoreError::BackupMismatch => "integrity_error",
        FileStashStoreError::NewerSchema(_) | FileStashStoreError::Unavailable => {
            "service_unavailable"
        }
    };
    service_error(kind, "File Stash operation failed")
}

#[cfg(test)]
mod tests {
    use super::*;
    use tracing_subscriber::prelude::*;
    #[test]
    fn action_metadata_classifies_only_delete_as_destructive() {
        assert_eq!(
            ACTIONS
                .iter()
                .filter(|a| a.destructive)
                .map(|a| a.name)
                .collect::<Vec<_>>(),
            vec!["stash.delete"]
        )
    }

    #[test]
    fn transfer_length_mismatch_is_not_reported_as_quota_exhaustion() {
        assert_eq!(
            map_error(FileStashStoreError::LengthMismatch).kind(),
            "invalid_param"
        );
        assert_eq!(
            map_error(FileStashStoreError::QuotaExceeded).kind(),
            "quota_exceeded"
        );
    }
    #[test]
    fn canonical_uri_is_id_based() {
        let f = StashFile {
            file_id: "01ARZ3NDEKTSV4RRFFQ69G5FAV".into(),
            display_name: "a".into(),
            size_bytes: 1,
            blob_key: "x".into(),
            created_at: 1,
            updated_at: 1,
            owned: true,
        };
        assert_eq!(f.uri(), "stash://me/files/01ARZ3NDEKTSV4RRFFQ69G5FAV")
    }
    #[test]
    fn filenames_are_leaf_normalized() {
        assert_eq!(
            normalize_name(r"C:\\fakepath\\Report.txt").unwrap().0,
            "Report.txt"
        );
        assert!(normalize_name("../").is_err())
    }
    #[test]
    fn search_keys_are_unicode_normalized_and_case_folded_for_accents() {
        assert_eq!(search_key("CAFÉ"), search_key("cafe\u{301}"));
        assert_eq!(search_key("Straße"), search_key("STRASSE"));
        assert_eq!(search_key("ΟΣ"), search_key("ος"));
        assert_eq!(search_key("ΟΣ"), search_key("οσ"));
    }
    #[tokio::test]
    async fn search_honors_the_configured_query_byte_limit() {
        let service = FileStashService::new(
            Arc::new(FileStashRuntime::blocked()),
            Arc::new(AccessRuntime::blocked_unavailable()),
            50,
            4,
        );
        let principal = PrincipalId::for_test("principal");
        assert!(matches!(
            service.search(&principal, "12345", None, None).await,
            Err(ToolError::InvalidParam { .. })
        ));
        assert_eq!(
            service
                .search(&principal, "1234", None, None)
                .await
                .unwrap_err()
                .kind(),
            "service_unavailable"
        );
    }

    #[tokio::test]
    async fn operation_event_is_structured_and_does_not_log_sensitive_params() {
        let _lock = crate::test_support::TRACING_TEST_LOCK.lock().unwrap();
        let logs = crate::test_support::SharedBuf::default();
        let subscriber = tracing_subscriber::registry().with(
            tracing_subscriber::fmt::layer()
                .json()
                .with_ansi(false)
                .without_time()
                .with_writer(logs.clone()),
        );
        let dispatch = tracing::Dispatch::new(subscriber);
        let service = FileStashService::new(
            Arc::new(FileStashRuntime::blocked()),
            Arc::new(AccessRuntime::blocked_unavailable()),
            50,
            128,
        );
        let principal = PrincipalId::for_test("principal-secret");
        let secret_name = "payroll-secret-name.txt";
        let _subscriber = tracing::dispatcher::set_default(&dispatch);
        crate::test_support::rebuild_tracing_interest_cache();
        let result = dispatch_for_principal(
            &service,
            &principal,
            "mcp",
            "stash.rename",
            serde_json::json!({"file_id":"01ARZ3NDEKTSV4RRFFQ69G5FAV","display_name":secret_name}),
        )
        .await;
        assert!(result.is_err());
        let output = crate::test_support::captured_logs(&logs);
        assert!(output.contains("\"surface\":\"mcp\""));
        assert!(output.contains("\"service\":\"stash\""));
        assert!(output.contains("\"action\":\"stash.rename\""));
        assert!(output.contains("\"result\":\"error\""));
        assert!(output.contains("\"object_id\":\"01ARZ3NDEKTSV4RRFFQ69G5FAV\""));
        assert!(output.contains("\"destructive\":false"));
        assert!(!output.contains(secret_name));
        assert!(!output.contains("principal-secret"));
    }
    #[test]
    fn stash_uri_parser_accepts_only_the_canonical_shape() {
        let id = "01ARZ3NDEKTSV4RRFFQ69G5FAV";
        assert_eq!(
            parse_stash_uri(&format!("stash://me/files/{id}")).unwrap(),
            id
        );
        for invalid_uri in [
            "http://me/files/01ARZ3NDEKTSV4RRFFQ69G5FAV",
            "stash://you/files/01ARZ3NDEKTSV4RRFFQ69G5FAV",
            "stash://me/file/01ARZ3NDEKTSV4RRFFQ69G5FAV",
            "stash://me/files/01ARZ3NDEKTSV4RRFFQ69G5FAV/extra",
            "stash://me/files/01ARZ3NDEKTSV4RRFFQ69G5FAV?q=1",
            "stash://me/files/01ARZ3NDEKTSV4RRFFQ69G5FAV#fragment",
            "stash://me/files/01arz3ndektsv4rrffq69g5fav",
        ] {
            assert!(parse_stash_uri(invalid_uri).is_err(), "{invalid_uri}");
        }
    }
}
