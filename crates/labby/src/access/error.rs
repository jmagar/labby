use std::path::PathBuf;

use thiserror::Error;

#[derive(Debug, Error)]
pub(crate) enum AccessStoreError {
    #[error("access store is locked")]
    Locked,
    #[error("access store is corrupt")]
    Corrupt,
    #[error("access store storage is full")]
    DiskFull,
    #[error("access store is read-only")]
    ReadOnly,
    #[error("access store path is unsafe: {path}")]
    InsecurePath { path: PathBuf },
    #[error("access store parent must already exist: {path}")]
    MissingParent { path: PathBuf },
    #[error("access store file has insecure permissions: {path}")]
    InsecurePermissions { path: PathBuf },
    #[error("access store schema {found} is newer than supported schema {supported}")]
    UnsupportedSchema { found: i64, supported: i64 },
    #[error("access store migration from schema {found} requires explicit operator approval")]
    MigrationApprovalRequired { found: i64 },
    #[error("access store migration evidence is invalid: {reason}")]
    MigrationEvidenceInvalid { reason: String },
    #[error("access store integrity check failed: {check}")]
    IntegrityViolation { check: &'static str },
    #[error("access store relation references a missing parent")]
    ForeignKeyViolation,
    #[error("access store owner bootstrap conflicts with existing state")]
    BootstrapConflict,
    #[error("access store owner bootstrap input is invalid")]
    InvalidBootstrapInput,
    #[error("access identity is unavailable")]
    IdentityUnavailable,
    #[error("project access is unavailable")]
    ProjectAccessUnavailable,
    #[error("not authorized")]
    NotAuthorized,
    #[error("team access input is invalid")]
    InvalidTeamInput,
    #[error("team or membership is unavailable")]
    TeamUnavailable,
    #[error("an active team must retain at least one active owner")]
    LastActiveTeamOwner,
    #[error("project loadout assignment input is invalid")]
    InvalidProjectLoadoutInput,
    #[error("project already has a different loadout assignment")]
    ProjectLoadoutConflict,
    #[error("access store contains malformed vocabulary")]
    MalformedVocabulary,
    #[error("access store is unavailable: {0}")]
    Unavailable(String),
}

pub(crate) type AccessStoreResult<T> = Result<T, AccessStoreError>;
