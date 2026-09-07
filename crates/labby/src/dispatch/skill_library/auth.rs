//! Final-boundary Skill Library authorization.

#![allow(
    dead_code,
    reason = "commit-bound policy seam is consumed by the Wave 2 Skill Library dispatcher"
)]

use std::collections::BTreeSet;

use labby_auth::{Authenticator, VerifiedIdentity};
use labby_primitives::product_credential::{BoundAccessGrant, ProductCredentialGrant};
use labby_runtime::artifacts::{
    LibraryActorId, LibraryAuthorization, LibraryGrant, LibraryOwnerKind, LibraryOwnership,
    LibraryTenantId, SkillVisibility,
};

use crate::access::{AccessRuntime, AccessStoreError, Permission, ProjectRole};

use super::audit::{
    CanonicalArtifactId, SkillLibraryAuditEvent, SkillLibraryAuditOutcome, SkillLibraryAuditStage,
    SkillLibraryCorrelationId, skill_library_audit_sink,
};

/// Every library operation classified by its policy needs.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub(crate) enum SkillLibraryAction {
    List,
    Search,
    Get,
    Read,
    History,
    Validate,
    Create,
    Save,
    Activate,
    Deactivate,
    Archive,
    Rollback,
    Import,
    ImportBatch,
    Refresh,
}

impl SkillLibraryAction {
    const ALL: [Self; 15] = [
        Self::List,
        Self::Search,
        Self::Get,
        Self::Read,
        Self::History,
        Self::Validate,
        Self::Create,
        Self::Save,
        Self::Activate,
        Self::Deactivate,
        Self::Archive,
        Self::Rollback,
        Self::Import,
        Self::ImportBatch,
        Self::Refresh,
    ];

    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::List => "artifacts.list",
            Self::Search => "artifacts.search",
            Self::Get => "artifacts.get",
            Self::Read => "artifacts.read",
            Self::History => "artifacts.history",
            Self::Validate => "artifacts.validate",
            Self::Create => "artifacts.create",
            Self::Save => "artifacts.save",
            Self::Activate => "artifacts.activate",
            Self::Deactivate => "artifacts.deactivate",
            Self::Archive => "artifacts.archive",
            Self::Rollback => "artifacts.rollback",
            Self::Import => "artifacts.import",
            Self::ImportBatch => "artifacts.import_batch",
            Self::Refresh => "artifacts.refresh",
        }
    }

    const fn is_mutation(self) -> bool {
        matches!(
            self,
            Self::Create
                | Self::Save
                | Self::Activate
                | Self::Deactivate
                | Self::Archive
                | Self::Rollback
                | Self::Import
                | Self::ImportBatch
                | Self::Refresh
        )
    }
}

/// Product surface where the already-authenticated request entered.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub(crate) enum SkillLibrarySurface {
    ApiCookie,
    ApiBearer,
    Mcp,
    Cli,
    CodeMode,
    AppCallback,
    Resource,
}

impl SkillLibrarySurface {
    const ALL: [Self; 7] = [
        Self::ApiCookie,
        Self::ApiBearer,
        Self::Mcp,
        Self::Cli,
        Self::CodeMode,
        Self::AppCallback,
        Self::Resource,
    ];

    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::ApiCookie => "api",
            Self::ApiBearer => "api",
            Self::Mcp => "mcp",
            Self::Cli => "cli",
            Self::CodeMode => "mcp",
            Self::AppCallback => "mcp",
            Self::Resource => "mcp",
        }
    }
}

/// Transport facts established by a trusted adapter.
///
/// No constructor accepts owner, role, tenant, provider subject, email, `_meta`, or client-supplied
/// authorization decisions.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct SkillLibraryTransport {
    pub(crate) surface: SkillLibrarySurface,
    pub(crate) same_origin: bool,
    pub(crate) csrf_verified: bool,
    pub(crate) audience_bound: bool,
    pub(crate) host_established_callback: bool,
    pub(crate) product_credential_bound: bool,
}

impl SkillLibraryTransport {
    pub(crate) const fn browser(same_origin: bool, csrf_verified: bool) -> Self {
        Self {
            surface: SkillLibrarySurface::ApiCookie,
            same_origin,
            csrf_verified,
            audience_bound: false,
            host_established_callback: false,
            product_credential_bound: false,
        }
    }

    pub(crate) const fn bearer(surface: SkillLibrarySurface, audience_bound: bool) -> Self {
        Self {
            surface,
            same_origin: false,
            csrf_verified: false,
            audience_bound,
            host_established_callback: false,
            product_credential_bound: false,
        }
    }

    pub(crate) const fn product_bearer(surface: SkillLibrarySurface) -> Self {
        Self {
            surface,
            same_origin: false,
            csrf_verified: false,
            audience_bound: true,
            host_established_callback: false,
            product_credential_bound: true,
        }
    }

    pub(crate) const fn app_callback(audience_bound: bool, host_established: bool) -> Self {
        Self {
            surface: SkillLibrarySurface::AppCallback,
            same_origin: false,
            csrf_verified: false,
            audience_bound,
            host_established_callback: host_established,
            product_credential_bound: false,
        }
    }

    pub(crate) const fn product_app_callback() -> Self {
        Self {
            surface: SkillLibrarySurface::AppCallback,
            same_origin: false,
            csrf_verified: false,
            audience_bound: true,
            host_established_callback: true,
            product_credential_bound: true,
        }
    }
}

/// Authenticated request facts supplied by a trusted surface adapter.
///
/// Deliberately does not implement `Debug`: the verified identity remains opaque in diagnostics.
#[derive(Clone)]
pub(crate) struct SkillLibraryCaller {
    identity: VerifiedIdentity,
    scopes: BTreeSet<String>,
    transport: SkillLibraryTransport,
    selected_team_id: Option<String>,
}

impl SkillLibraryCaller {
    pub(crate) fn new(
        identity: VerifiedIdentity,
        scopes: impl IntoIterator<Item = String>,
        transport: SkillLibraryTransport,
    ) -> Self {
        Self {
            identity,
            scopes: scopes.into_iter().collect(),
            transport,
            selected_team_id: None,
        }
    }

    /// Bind an untrusted selector to this request. The access snapshot remains authoritative:
    /// an unknown, suspended, unassigned, or non-member Team is denied below.
    pub(crate) fn with_selected_team_id(mut self, selected_team_id: Option<String>) -> Self {
        self.selected_team_id = selected_team_id;
        self
    }
}

pub(crate) fn product_grants_match(
    source: &ProductCredentialGrant,
    bound: &BoundAccessGrant,
) -> bool {
    source.issuer == bound.issuer
        && source.subject == bound.subject
        && source.credential_id == bound.credential_id
        && source.credential_generation == bound.credential_generation
        && source.scopes == bound.scopes
        && source.resource == bound.resource
        && source.audience == bound.audience
        && source.expires_at == bound.expires_at
}

pub(crate) fn product_grants_are_route_bound(
    source: &ProductCredentialGrant,
    bound: &BoundAccessGrant,
) -> bool {
    product_grants_match(source, bound) && bound.audience == bound.resource
}

/// Target visibility relevant to non-enumerating read policy.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum SkillLibraryTarget<'a> {
    SharedActive,
    Personal(&'a LibraryOwnership),
    Mutation(&'a LibraryOwnership),
    CreateForCaller,
    LibraryRoot,
}

/// One current access snapshot projected into the runtime authority vocabulary.
pub(crate) struct SkillLibraryAuthorizationDecision {
    pub(crate) authorization: LibraryAuthorization,
    pub(crate) ownership: LibraryOwnership,
    pub(crate) audit: SkillLibraryAuditEvent,
    actor_id: LibraryActorId,
    tenant_id: LibraryTenantId,
    project_id: LibraryActorId,
    team_ids: BTreeSet<LibraryActorId>,
    team_management_ids: BTreeSet<LibraryActorId>,
    authority_generation: u64,
    is_admin: bool,
    is_platform_admin: bool,
}

impl SkillLibraryAuthorizationDecision {
    pub(crate) fn tenant_id(&self) -> &LibraryTenantId {
        &self.tenant_id
    }

    pub(crate) fn cursor_binding(&self) -> (&str, &str, &str, u64) {
        (
            self.tenant_id.as_str(),
            self.actor_id.as_str(),
            self.project_id.as_str(),
            self.authority_generation,
        )
    }

    pub(crate) fn cursor_team_ids(&self) -> impl Iterator<Item = &str> {
        self.team_ids.iter().map(LibraryActorId::as_str)
    }

    pub(crate) fn into_shared_create(
        self,
    ) -> Result<
        (
            LibraryAuthorization,
            LibraryOwnership,
            SkillLibraryAuditEvent,
        ),
        SkillLibraryAuthorizationError,
    > {
        if self.team_ids.len() == 1
            && !self
                .team_ids
                .iter()
                .all(|team| self.team_management_ids.contains(team))
        {
            return Err(SkillLibraryAuthorizationError::Denied);
        }
        let (owner_kind, scope_id) = self
            .team_ids
            .iter()
            .next()
            .filter(|_| self.team_ids.len() == 1)
            .map_or(
                (LibraryOwnerKind::Project, self.project_id.clone()),
                |team_id| (LibraryOwnerKind::Team, team_id.clone()),
            );
        let ownership =
            LibraryOwnership::scoped(self.tenant_id.clone(), owner_kind, scope_id.clone());
        let authorization = LibraryAuthorization::from_authorized_scope_projection(
            self.tenant_id,
            self.actor_id,
            owner_kind,
            scope_id,
        );
        Ok((authorization, ownership, self.audit))
    }

    pub(crate) fn artifact_access_snapshot(&self) -> crate::skills::facade::ArtifactAccessSnapshot {
        crate::skills::facade::ArtifactAccessSnapshot::new(
            self.tenant_id.clone(),
            self.actor_id.clone(),
            self.project_id.clone(),
            self.team_ids.clone(),
            self.is_admin,
            self.is_platform_admin,
        )
    }
    /// Filter a previously loaded personal-record collection locally after one request snapshot.
    /// This makes list/get collision handling O(1) access queries rather than one query per Skill.
    pub(crate) fn permits_personal(&self, ownership: &LibraryOwnership) -> bool {
        ownership.tenant_id == self.tenant_id
            && ownership.owner_kind() == LibraryOwnerKind::Personal
            && (ownership.owner_id == self.actor_id || self.is_platform_admin)
    }

    /// Privacy-safe ownership relationship for response projection.
    #[allow(
        clippy::suspicious_operation_groupings,
        reason = "the two differently named canonical id dimensions are intentionally compared"
    )]
    pub(crate) fn owns(&self, ownership: &LibraryOwnership) -> bool {
        (ownership.tenant_id == self.tenant_id)
            && ownership.owner_kind() == LibraryOwnerKind::Personal
            && (ownership.owner_id == self.actor_id)
    }

    /// Apply record visibility after one current membership snapshot without another access read.
    ///
    /// Private records are visible only to their owner or a current project administrator. Tenant
    /// records become shared only while active. Every cross-tenant record is rejected identically.
    pub(crate) fn permits_record(
        &self,
        ownership: &LibraryOwnership,
        visibility: SkillVisibility,
        is_active: bool,
    ) -> bool {
        ownership.tenant_id == self.tenant_id
            && match ownership.owner_kind() {
                LibraryOwnerKind::Personal => {
                    self.permits_personal(ownership)
                        || (visibility == SkillVisibility::Tenant && is_active)
                }
                LibraryOwnerKind::Project => {
                    (ownership.owner_id == self.project_id || self.is_platform_admin)
                        && (visibility == SkillVisibility::Tenant || self.is_admin)
                        && is_active
                }
                LibraryOwnerKind::Team => {
                    self.is_platform_admin
                        || (self.team_ids.len() == 1
                            && self.team_ids.contains(&ownership.owner_id)
                            && visibility == SkillVisibility::Tenant)
                            && is_active
                }
            }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, thiserror::Error)]
pub(crate) enum SkillLibraryAuthorizationError {
    /// Unknown targets, absent identities, insufficient scope/role, and cross-tenant access share
    /// one public denial so private records cannot be enumerated.
    #[error("skill library access denied")]
    Denied,
    #[error("skill library authorization is unavailable")]
    Unavailable,
}

/// Resolve exactly one uncached membership snapshot and authorize this operation.
///
/// Mutation dispatchers must call this immediately before `mutate_library`; validation-time
/// decisions are not reusable commit grants. Every invocation queries the current AccessRuntime,
/// so membership or role revocation between validation and commit wins.
pub(crate) async fn authorize_at_boundary(
    runtime: &AccessRuntime,
    caller: SkillLibraryCaller,
    project_id: &str,
    action: SkillLibraryAction,
    target_id: &CanonicalArtifactId,
    target: SkillLibraryTarget<'_>,
    correlation_id: &SkillLibraryCorrelationId,
) -> Result<SkillLibraryAuthorizationDecision, SkillLibraryAuthorizationError> {
    debug_assert!(SkillLibraryAction::ALL.contains(&action));
    debug_assert!(SkillLibrarySurface::ALL.contains(&caller.transport.surface));
    let target = if action == SkillLibraryAction::Create {
        SkillLibraryTarget::CreateForCaller
    } else {
        target
    };
    let audit_sink = skill_library_audit_sink();
    let surface = caller.transport.surface;
    validate_transport(&caller, action).inspect_err(|_| {
        let event = SkillLibraryAuditEvent::new(
            correlation_id.clone(),
            target_id,
            action,
            surface,
            SkillLibraryAuditOutcome::Deny,
            SkillLibraryAuditStage::Transport,
        );
        audit_sink.record(event);
    })?;
    validate_target_kind(action, target).inspect_err(|_| {
        let event = SkillLibraryAuditEvent::new(
            correlation_id.clone(),
            target_id,
            action,
            surface,
            SkillLibraryAuditOutcome::Deny,
            SkillLibraryAuditStage::Ownership,
        );
        audit_sink.record(event);
    })?;

    let store = runtime.store().await.map_err(|_| {
        let event = SkillLibraryAuditEvent::new(
            correlation_id.clone(),
            target_id,
            action,
            surface,
            SkillLibraryAuditOutcome::Unavailable,
            SkillLibraryAuditStage::AccessSnapshot,
        );
        audit_sink.record(event);
        SkillLibraryAuthorizationError::Unavailable
    })?;
    let permission = if action.is_mutation() {
        Permission::AssetUse
    } else {
        Permission::AssetDiscover
    };
    let selected_team_id = caller.selected_team_id.clone();
    let mut snapshot = store
        .authorize_skill_library(caller.identity, project_id.to_owned(), permission)
        .await
        .map_err(|error| {
            let (outcome, policy_error) = match error {
                AccessStoreError::IdentityUnavailable
                | AccessStoreError::ProjectAccessUnavailable
                | AccessStoreError::NotAuthorized => (
                    SkillLibraryAuditOutcome::Deny,
                    SkillLibraryAuthorizationError::Denied,
                ),
                // `Locked`, `Corrupt`, `DiskFull`, `ReadOnly`, and
                // `Unavailable(String)` all collapse to one opaque agent-facing
                // kind. `map_sqlite_error` separated them for a reason, so record
                // the concrete cause before it is erased — otherwise "retry" and
                // "page an operator" are indistinguishable in the logs too.
                _ => {
                    tracing::error!(
                        surface = ?surface,
                        project_id,
                        action = ?action,
                        error = %error,
                        "Skill Library authorization unavailable"
                    );
                    (
                        SkillLibraryAuditOutcome::Unavailable,
                        SkillLibraryAuthorizationError::Unavailable,
                    )
                }
            };
            let event = SkillLibraryAuditEvent::new(
                correlation_id.clone(),
                target_id,
                action,
                surface,
                outcome,
                SkillLibraryAuditStage::AccessSnapshot,
            );
            audit_sink.record(event);
            policy_error
        })?;

    narrow_to_selected_team(&mut snapshot, selected_team_id.as_deref())?;

    decision_from_snapshot(snapshot, action, target_id, target, correlation_id, surface)
}

fn decision_from_snapshot(
    snapshot: crate::access::LibraryAccessSnapshot,
    action: SkillLibraryAction,
    target_id: &CanonicalArtifactId,
    target: SkillLibraryTarget<'_>,
    correlation_id: &SkillLibraryCorrelationId,
    surface: SkillLibrarySurface,
) -> Result<SkillLibraryAuthorizationDecision, SkillLibraryAuthorizationError> {
    let audit_sink = skill_library_audit_sink();
    // Every other failure below records an audit event. A malformed persisted
    // identifier is exactly the condition `MalformedVocabulary` names, and it
    // fails every Skill Library call for the tenant — it must not be the one
    // path that leaves no trace. Log the field name only, never the value.
    let projection_failure = |field: &'static str| {
        tracing::error!(
            surface = ?surface,
            action = ?action,
            field,
            "Skill Library access snapshot carries a malformed canonical identifier"
        );
        audit_sink.record(SkillLibraryAuditEvent::new(
            correlation_id.clone(),
            target_id,
            action,
            surface,
            SkillLibraryAuditOutcome::Unavailable,
            SkillLibraryAuditStage::AccessSnapshot,
        ));
        SkillLibraryAuthorizationError::Unavailable
    };
    let tenant_id = LibraryTenantId::from_canonical_projection(snapshot.organization_id)
        .map_err(|_| projection_failure("organization_id"))?;
    let actor_id = LibraryActorId::from_canonical_projection(snapshot.principal_id)
        .map_err(|_| projection_failure("principal_id"))?;
    let project_id = LibraryActorId::from_canonical_projection(snapshot.project_id)
        .map_err(|_| projection_failure("project_id"))?;
    let team_ids = snapshot
        .team_ids
        .into_iter()
        .map(LibraryActorId::from_canonical_projection)
        .collect::<Result<BTreeSet<_>, _>>()
        .map_err(|_| projection_failure("team_id"))?;
    let team_management_ids = snapshot
        .team_management_ids
        .into_iter()
        .map(LibraryActorId::from_canonical_projection)
        .collect::<Result<BTreeSet<_>, _>>()
        .map_err(|_| projection_failure("team_id"))?;
    let is_admin = snapshot.is_platform_admin
        || matches!(snapshot.role, ProjectRole::Owner | ProjectRole::Admin);
    let (grant, ownership) = resolve_grant(
        &snapshot.role,
        &tenant_id,
        &actor_id,
        &project_id,
        &team_ids,
        &team_management_ids,
        snapshot.is_platform_admin,
        action,
        target,
    )
    .ok_or_else(|| {
        audit_sink.record(
            SkillLibraryAuditEvent::new(
                correlation_id.clone(),
                target_id,
                action,
                surface,
                SkillLibraryAuditOutcome::Deny,
                SkillLibraryAuditStage::Ownership,
            )
            .with_canonical_actor(
                tenant_id.clone(),
                actor_id.clone(),
                snapshot.global_revision,
            ),
        );
        SkillLibraryAuthorizationError::Denied
    })?;
    let authorization = if ownership.owner_kind() == LibraryOwnerKind::Personal {
        LibraryAuthorization::from_authorized_access_projection(
            tenant_id.clone(),
            actor_id.clone(),
            grant,
        )
    } else {
        LibraryAuthorization::from_authorized_scope_projection(
            tenant_id.clone(),
            actor_id.clone(),
            ownership.owner_kind(),
            ownership.owner_id.clone(),
        )
    };
    let audit = SkillLibraryAuditEvent::new(
        correlation_id.clone(),
        target_id,
        action,
        surface,
        SkillLibraryAuditOutcome::Allow,
        SkillLibraryAuditStage::Ownership,
    )
    .with_canonical_actor(
        tenant_id.clone(),
        actor_id.clone(),
        snapshot.global_revision,
    );
    audit_sink.record(audit.clone());
    Ok(SkillLibraryAuthorizationDecision {
        authorization,
        ownership,
        audit,
        actor_id,
        tenant_id,
        project_id,
        team_ids,
        team_management_ids,
        authority_generation: snapshot.global_revision,
        is_admin,
        is_platform_admin: snapshot.is_platform_admin,
    })
}

fn narrow_to_selected_team(
    snapshot: &mut crate::access::LibraryAccessSnapshot,
    selected_team_id: Option<&str>,
) -> Result<(), SkillLibraryAuthorizationError> {
    if let Some(selected_team_id) = selected_team_id {
        if !snapshot.team_ids.iter().any(|id| id == selected_team_id) {
            return Err(SkillLibraryAuthorizationError::Denied);
        }
        snapshot.team_ids.retain(|id| id == selected_team_id);
        snapshot
            .team_management_ids
            .retain(|id| id == selected_team_id);
    } else {
        // Absence of an explicit Team context means Project context. Never infer a Team from
        // membership, since doing so makes ownership change as assignments are added or removed.
        snapshot.team_ids.clear();
        snapshot.team_management_ids.clear();
    }
    Ok(())
}

/// Fail-closed adapter for surfaces where authentication may be absent.
pub(crate) async fn authorize_optional_at_boundary(
    runtime: &AccessRuntime,
    caller: Option<SkillLibraryCaller>,
    project_id: &str,
    action: SkillLibraryAction,
    target_id: &CanonicalArtifactId,
    target: SkillLibraryTarget<'_>,
    correlation_id: &SkillLibraryCorrelationId,
) -> Result<SkillLibraryAuthorizationDecision, SkillLibraryAuthorizationError> {
    let caller = caller.ok_or_else(|| {
        skill_library_audit_sink().record(SkillLibraryAuditEvent::new(
            correlation_id.clone(),
            target_id,
            action,
            SkillLibrarySurface::ApiBearer,
            SkillLibraryAuditOutcome::Deny,
            SkillLibraryAuditStage::Transport,
        ));
        SkillLibraryAuthorizationError::Denied
    })?;
    authorize_at_boundary(
        runtime,
        caller,
        project_id,
        action,
        target_id,
        target,
        correlation_id,
    )
    .await
}

/// Reauthorize against the current AccessRuntime and immediately execute one commit/replay.
///
/// The executor receives the sealed authorization only after the uncached membership read. A
/// validation-time decision cannot be supplied here, so revocation before this call prevents the
/// executor and therefore prevents any ArtifactStore mutation or idempotent replay.
pub(crate) async fn authorize_and_commit<T, E>(
    runtime: &AccessRuntime,
    caller: SkillLibraryCaller,
    project_id: &str,
    action: SkillLibraryAction,
    target_id: &CanonicalArtifactId,
    target: SkillLibraryTarget<'_>,
    correlation_id: &SkillLibraryCorrelationId,
    executor: impl FnOnce(&LibraryAuthorization, &LibraryOwnership) -> Result<T, E> + Send + 'static,
) -> Result<T, SkillLibraryCommitError<E>>
where
    T: Send + 'static,
    E: Send + 'static,
{
    // `Permission::AssetUse` below is hardcoded on the strength of this
    // invariant. A `debug_assert` lets a release build silently take a path the
    // caller believed impossible, so check it for real.
    if !action.is_mutation() {
        tracing::error!(
            action = ?action,
            "authorize_and_commit called with a non-mutating action"
        );
        return Err(SkillLibraryCommitError::Authorization(
            SkillLibraryAuthorizationError::Unavailable,
        ));
    }
    let target = if action == SkillLibraryAction::Create {
        SkillLibraryTarget::CreateForCaller
    } else {
        target
    };
    let surface = caller.transport.surface;
    let audit_sink = skill_library_audit_sink();
    validate_transport(&caller, action)
        .inspect_err(|_| {
            audit_sink.record(SkillLibraryAuditEvent::new(
                correlation_id.clone(),
                target_id,
                action,
                surface,
                SkillLibraryAuditOutcome::Deny,
                SkillLibraryAuditStage::Transport,
            ));
        })
        .map_err(SkillLibraryCommitError::Authorization)?;
    validate_target_kind(action, target)
        .inspect_err(|_| {
            audit_sink.record(SkillLibraryAuditEvent::new(
                correlation_id.clone(),
                target_id,
                action,
                surface,
                SkillLibraryAuditOutcome::Deny,
                SkillLibraryAuditStage::Ownership,
            ));
        })
        .map_err(SkillLibraryCommitError::Authorization)?;
    let owned_target = OwnedSkillLibraryTarget::from(target);
    let selected_team_id = caller.selected_team_id.clone();
    let commit_target_id = target_id.clone();
    let commit_correlation_id = correlation_id.clone();
    let failure_target_id = target_id.clone();
    let failure_correlation_id = correlation_id.clone();
    let store = runtime.store().await.map_err(|_| {
        audit_sink.record(SkillLibraryAuditEvent::new(
            correlation_id.clone(),
            target_id,
            action,
            surface,
            SkillLibraryAuditOutcome::Unavailable,
            SkillLibraryAuditStage::AccessSnapshot,
        ));
        SkillLibraryCommitError::Authorization(SkillLibraryAuthorizationError::Unavailable)
    })?;
    let guarded = store
        .authorize_skill_library_and_execute(
            caller.identity,
            project_id.to_owned(),
            Permission::AssetUse,
            move |mut snapshot| {
                narrow_to_selected_team(&mut snapshot, selected_team_id.as_deref())?;
                let decision = decision_from_snapshot(
                    snapshot,
                    action,
                    &commit_target_id,
                    owned_target.as_target(),
                    &commit_correlation_id,
                    surface,
                )?;
                Ok::<Result<T, E>, SkillLibraryAuthorizationError>(executor(
                    &decision.authorization,
                    &decision.ownership,
                ))
            },
        )
        .await
        .map_err(|error| {
            let (outcome, policy_error) = match error {
                AccessStoreError::IdentityUnavailable
                | AccessStoreError::ProjectAccessUnavailable
                | AccessStoreError::NotAuthorized => (
                    SkillLibraryAuditOutcome::Deny,
                    SkillLibraryAuthorizationError::Denied,
                ),
                // `Locked`, `Corrupt`, `DiskFull`, `ReadOnly`, and
                // `Unavailable(String)` all collapse to one opaque agent-facing
                // kind. `map_sqlite_error` separated them for a reason, so record
                // the concrete cause before it is erased — otherwise "retry" and
                // "page an operator" are indistinguishable in the logs too.
                _ => {
                    tracing::error!(
                        surface = ?surface,
                        project_id,
                        action = ?action,
                        error = %error,
                        "Skill Library authorization unavailable"
                    );
                    (
                        SkillLibraryAuditOutcome::Unavailable,
                        SkillLibraryAuthorizationError::Unavailable,
                    )
                }
            };
            audit_sink.record(SkillLibraryAuditEvent::new(
                failure_correlation_id,
                &failure_target_id,
                action,
                surface,
                outcome,
                SkillLibraryAuditStage::AccessSnapshot,
            ));
            SkillLibraryCommitError::Authorization(policy_error)
        })?;
    guarded
        .map_err(SkillLibraryCommitError::Authorization)?
        .map_err(SkillLibraryCommitError::Execution)
}

enum OwnedSkillLibraryTarget {
    SharedActive,
    Personal(LibraryOwnership),
    Mutation(LibraryOwnership),
    CreateForCaller,
    LibraryRoot,
}

impl From<SkillLibraryTarget<'_>> for OwnedSkillLibraryTarget {
    fn from(target: SkillLibraryTarget<'_>) -> Self {
        match target {
            SkillLibraryTarget::SharedActive => Self::SharedActive,
            SkillLibraryTarget::Personal(ownership) => Self::Personal(ownership.clone()),
            SkillLibraryTarget::Mutation(ownership) => Self::Mutation(ownership.clone()),
            SkillLibraryTarget::CreateForCaller => Self::CreateForCaller,
            SkillLibraryTarget::LibraryRoot => Self::LibraryRoot,
        }
    }
}

impl OwnedSkillLibraryTarget {
    fn as_target(&self) -> SkillLibraryTarget<'_> {
        match self {
            Self::SharedActive => SkillLibraryTarget::SharedActive,
            Self::Personal(ownership) => SkillLibraryTarget::Personal(ownership),
            Self::Mutation(ownership) => SkillLibraryTarget::Mutation(ownership),
            Self::CreateForCaller => SkillLibraryTarget::CreateForCaller,
            Self::LibraryRoot => SkillLibraryTarget::LibraryRoot,
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub(crate) enum SkillLibraryCommitError<E> {
    #[error(transparent)]
    Authorization(SkillLibraryAuthorizationError),
    #[error("skill library commit failed")]
    Execution(E),
}

fn validate_transport(
    caller: &SkillLibraryCaller,
    action: SkillLibraryAction,
) -> Result<(), SkillLibraryAuthorizationError> {
    let identity_transport = caller.identity.authenticator();
    let scope_allowed = if action.is_mutation() {
        caller
            .scopes
            .iter()
            .any(|scope| matches!(scope.as_str(), "lab" | "lab:admin"))
    } else {
        caller
            .scopes
            .iter()
            .any(|scope| matches!(scope.as_str(), "lab:read" | "lab" | "lab:admin"))
    };
    let valid = match caller.transport.surface {
        SkillLibrarySurface::ApiCookie => {
            identity_transport == Authenticator::BrowserSession
                && caller.transport.same_origin
                && (!action.is_mutation() || caller.transport.csrf_verified)
        }
        SkillLibrarySurface::ApiBearer
        | SkillLibrarySurface::Mcp
        | SkillLibrarySurface::CodeMode
        | SkillLibrarySurface::Resource => {
            (matches!(
                identity_transport,
                Authenticator::OauthBearer | Authenticator::StaticBearer
            ) || (identity_transport == Authenticator::ProductCredential
                && caller.transport.surface != SkillLibrarySurface::ApiBearer
                && caller.transport.product_credential_bound))
                && caller.transport.audience_bound
                && scope_allowed
        }
        SkillLibrarySurface::AppCallback => {
            (matches!(
                identity_transport,
                Authenticator::OauthBearer | Authenticator::StaticBearer
            ) || (identity_transport == Authenticator::ProductCredential
                && caller.transport.product_credential_bound))
                && caller.transport.audience_bound
                && caller.transport.host_established_callback
                && scope_allowed
        }
        SkillLibrarySurface::Cli => identity_transport == Authenticator::UnixPeer,
    };
    valid
        .then_some(())
        .ok_or(SkillLibraryAuthorizationError::Denied)
}

fn validate_target_kind(
    action: SkillLibraryAction,
    target: SkillLibraryTarget<'_>,
) -> Result<(), SkillLibraryAuthorizationError> {
    let valid = if action.is_mutation() {
        matches!(
            target,
            SkillLibraryTarget::Mutation(_) | SkillLibraryTarget::CreateForCaller
        ) || (action == SkillLibraryAction::Refresh && target == SkillLibraryTarget::LibraryRoot)
    } else {
        matches!(
            target,
            SkillLibraryTarget::SharedActive | SkillLibraryTarget::Personal(_)
        )
    };
    valid
        .then_some(())
        .ok_or(SkillLibraryAuthorizationError::Denied)
}

fn resolve_grant(
    role: &ProjectRole,
    tenant_id: &LibraryTenantId,
    actor_id: &LibraryActorId,
    project_id: &LibraryActorId,
    team_ids: &BTreeSet<LibraryActorId>,
    team_management_ids: &BTreeSet<LibraryActorId>,
    is_platform_admin: bool,
    action: SkillLibraryAction,
    target: SkillLibraryTarget<'_>,
) -> Option<(LibraryGrant, LibraryOwnership)> {
    if target == SkillLibraryTarget::SharedActive {
        return Some((
            LibraryGrant::Owner,
            LibraryOwnership::canonical(tenant_id.clone(), actor_id.clone()),
        ));
    }
    let ownership = match target {
        SkillLibraryTarget::CreateForCaller => {
            LibraryOwnership::canonical(tenant_id.clone(), actor_id.clone())
        }
        SkillLibraryTarget::Personal(ownership) | SkillLibraryTarget::Mutation(ownership) => {
            ownership.clone()
        }
        SkillLibraryTarget::LibraryRoot => {
            if !matches!(role, ProjectRole::Owner | ProjectRole::Admin) {
                return None;
            }
            LibraryOwnership::canonical(tenant_id.clone(), actor_id.clone())
        }
        SkillLibraryTarget::SharedActive => unreachable!(),
    };
    if ownership.tenant_id != *tenant_id {
        return None;
    }
    if is_platform_admin {
        return Some((LibraryGrant::Admin, ownership));
    }
    if ownership.owner_kind() == LibraryOwnerKind::Personal && ownership.owner_id == *actor_id {
        return Some((LibraryGrant::Owner, ownership));
    }
    match ownership.owner_kind() {
        LibraryOwnerKind::Project if ownership.owner_id == *project_id => {
            Some((LibraryGrant::Owner, ownership))
        }
        LibraryOwnerKind::Team
            if team_ids.len() == 1
                && team_ids.contains(&ownership.owner_id)
                && (!action.is_mutation() || team_management_ids.contains(&ownership.owner_id)) =>
        {
            Some((LibraryGrant::Owner, ownership))
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use labby_auth::PrincipalLink;

    use super::*;
    use crate::access::{AccessStore, BootstrapOwnerInput};

    fn secure_tempdir() -> tempfile::TempDir {
        let directory = tempfile::tempdir().unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            std::fs::set_permissions(directory.path(), std::fs::Permissions::from_mode(0o700))
                .unwrap();
        }
        directory
    }

    fn browser(subject: &str) -> VerifiedIdentity {
        VerifiedIdentity::external(
            Authenticator::BrowserSession,
            "https://accounts.google.com",
            subject,
        )
        .unwrap()
    }

    async fn fixture() -> (tempfile::TempDir, AccessRuntime, VerifiedIdentity) {
        let directory = secure_tempdir();
        let path = directory.path().join("access.db");
        let store = AccessStore::open(path.clone()).await.unwrap();
        let owner = browser("owner-subject");
        store
            .bootstrap_owner(BootstrapOwnerInput::new(owner.clone(), "Local", "Default").unwrap())
            .await
            .unwrap();
        drop(store);
        (directory, AccessRuntime::initialize(path).await, owner)
    }

    fn local(credential: &str) -> VerifiedIdentity {
        VerifiedIdentity::local_credential(Authenticator::StaticBearer, credential).unwrap()
    }

    fn browser_caller(identity: VerifiedIdentity, csrf: bool) -> SkillLibraryCaller {
        SkillLibraryCaller::new(identity, [], SkillLibraryTransport::browser(true, csrf))
    }

    fn ownership(owner: &str) -> LibraryOwnership {
        LibraryOwnership::canonical(
            LibraryTenantId::from_canonical_projection("bootstrap-local").unwrap(),
            LibraryActorId::from_canonical_projection(owner).unwrap(),
        )
    }

    #[test]
    fn final_authorization_snapshot_is_bounded_to_the_explicit_team_context() {
        let mut snapshot = crate::access::LibraryAccessSnapshot {
            principal_id: "member".into(),
            organization_id: "bootstrap-local".into(),
            project_id: "project".into(),
            role: ProjectRole::Member,
            global_revision: 1,
            team_ids: vec!["team-a".into(), "team-b".into()],
            team_management_ids: vec!["team-a".into(), "team-b".into()],
            is_platform_admin: false,
        };

        narrow_to_selected_team(&mut snapshot, Some("team-a")).unwrap();

        assert_eq!(snapshot.team_ids, ["team-a"]);
        assert_eq!(snapshot.team_management_ids, ["team-a"]);
        assert_eq!(
            narrow_to_selected_team(&mut snapshot, Some("team-b")),
            Err(SkillLibraryAuthorizationError::Denied),
            "a final-boundary snapshot cannot regain a different Team after context selection",
        );
    }

    async fn decide(
        runtime: &AccessRuntime,
        caller: SkillLibraryCaller,
        project_id: &str,
        action: SkillLibraryAction,
        target_id: &str,
        target: SkillLibraryTarget<'_>,
        correlation_id: &str,
    ) -> Result<SkillLibraryAuthorizationDecision, SkillLibraryAuthorizationError> {
        authorize_at_boundary(
            runtime,
            caller,
            project_id,
            action,
            &CanonicalArtifactId::parse(target_id).unwrap(),
            target,
            &SkillLibraryCorrelationId::parse(correlation_id).unwrap(),
        )
        .await
    }

    #[tokio::test]
    async fn browser_owner_requires_csrf_and_uses_canonical_access_ids() {
        let (_directory, runtime, owner) = fixture().await;
        let denied = decide(
            &runtime,
            browser_caller(owner.clone(), false),
            "bootstrap-default",
            SkillLibraryAction::Activate,
            "artifact-1",
            SkillLibraryTarget::Mutation(&ownership("bootstrap-owner")),
            "request-1",
        )
        .await;
        assert!(matches!(
            denied,
            Err(SkillLibraryAuthorizationError::Denied)
        ));

        let snapshot = runtime
            .store()
            .await
            .unwrap()
            .authorize_skill_library(
                owner.clone(),
                "bootstrap-default".to_owned(),
                Permission::AssetUse,
            )
            .await
            .unwrap();
        assert_eq!(snapshot.principal_id, "bootstrap-owner");

        let allowed = decide(
            &runtime,
            browser_caller(owner, true),
            "bootstrap-default",
            SkillLibraryAction::Activate,
            "artifact-1",
            SkillLibraryTarget::Mutation(&ownership("bootstrap-owner")),
            "request-1",
        )
        .await
        .unwrap();
        assert_eq!(allowed.ownership.owner_id.as_str(), "bootstrap-owner");
        assert_eq!(allowed.ownership.tenant_id.as_str(), "bootstrap-local");
    }

    #[tokio::test]
    async fn scope_or_static_bearer_without_current_mapping_never_grants_admin() {
        let (_directory, runtime, _owner) = fixture().await;
        let bearer = VerifiedIdentity::local_credential(
            Authenticator::StaticBearer,
            "allowlisted-but-unmapped",
        )
        .unwrap();
        let denied = decide(
            &runtime,
            SkillLibraryCaller::new(
                bearer,
                ["lab:admin".to_string()],
                SkillLibraryTransport::bearer(SkillLibrarySurface::Mcp, true),
            ),
            "bootstrap-default",
            SkillLibraryAction::Archive,
            "private-artifact",
            SkillLibraryTarget::Mutation(&ownership("bootstrap-owner")),
            "request-2",
        )
        .await;
        assert!(matches!(
            denied,
            Err(SkillLibraryAuthorizationError::Denied)
        ));
    }

    #[tokio::test]
    async fn project_bootstrap_credential_can_use_the_bound_mcp_skill_library_surface() {
        let directory = secure_tempdir();
        let path = directory.path().join("access.db");
        let store = AccessStore::open(path.clone()).await.unwrap();
        let owner = VerifiedIdentity::local_credential(
            Authenticator::ProductCredential,
            "bootstrap-project-credential",
        )
        .unwrap();
        store
            .bootstrap_owner(BootstrapOwnerInput::new(owner.clone(), "Local", "Default").unwrap())
            .await
            .unwrap();
        drop(store);
        let runtime = AccessRuntime::initialize(path).await;

        let decision = decide(
            &runtime,
            SkillLibraryCaller::new(
                owner,
                ["lab:read".to_string(), "lab:admin".to_string()],
                SkillLibraryTransport::product_bearer(SkillLibrarySurface::Mcp),
            ),
            "bootstrap-default",
            SkillLibraryAction::List,
            "library",
            SkillLibraryTarget::SharedActive,
            "project-credential-list",
        )
        .await
        .unwrap();

        assert_eq!(decision.ownership.owner_id.as_str(), "bootstrap-owner");

        let generic_api = decide(
            &runtime,
            SkillLibraryCaller::new(
                VerifiedIdentity::local_credential(
                    Authenticator::ProductCredential,
                    "bootstrap-project-credential",
                )
                .unwrap(),
                ["lab:admin".to_string()],
                SkillLibraryTransport::product_bearer(SkillLibrarySurface::ApiBearer),
            ),
            "bootstrap-default",
            SkillLibraryAction::Import,
            "artifact",
            SkillLibraryTarget::CreateForCaller,
            "generic-api-product-credential",
        )
        .await;
        assert!(matches!(
            generic_api,
            Err(SkillLibraryAuthorizationError::Denied)
        ));

        let unbound = decide(
            &runtime,
            SkillLibraryCaller::new(
                VerifiedIdentity::local_credential(
                    Authenticator::ProductCredential,
                    "bootstrap-project-credential",
                )
                .unwrap(),
                ["lab:admin".to_string()],
                SkillLibraryTransport::bearer(SkillLibrarySurface::ApiBearer, true),
            ),
            "bootstrap-default",
            SkillLibraryAction::Import,
            "artifact",
            SkillLibraryTarget::CreateForCaller,
            "unbound-product-credential",
        )
        .await;
        assert!(matches!(
            unbound,
            Err(SkillLibraryAuthorizationError::Denied)
        ));
    }

    #[tokio::test]
    async fn project_roles_cannot_mutate_another_principals_personal_record() {
        let (_directory, runtime, _owner) = fixture().await;
        let store = runtime.store().await.unwrap();
        store.seed_loadout_roles_for_test().await.unwrap();
        let target = ownership("bootstrap-owner");
        let admin = decide(
            &runtime,
            SkillLibraryCaller::new(
                local("static-bearer:admin"),
                ["lab".to_string()],
                SkillLibraryTransport::bearer(SkillLibrarySurface::ApiBearer, true),
            ),
            "admin-project",
            SkillLibraryAction::Archive,
            "artifact-1",
            SkillLibraryTarget::Mutation(&target),
            "request-admin",
        )
        .await;
        assert!(matches!(admin, Err(SkillLibraryAuthorizationError::Denied)));

        let member = decide(
            &runtime,
            SkillLibraryCaller::new(
                local("static-bearer:member"),
                ["lab:admin".to_string()],
                SkillLibraryTransport::bearer(SkillLibrarySurface::ApiBearer, true),
            ),
            "member-project",
            SkillLibraryAction::Archive,
            "artifact-1",
            SkillLibraryTarget::Mutation(&target),
            "request-member",
        )
        .await;
        assert!(matches!(
            member,
            Err(SkillLibraryAuthorizationError::Denied)
        ));
    }

    #[tokio::test]
    async fn record_visibility_is_private_to_owner_admin_and_shares_only_active_tenant_records() {
        let (_directory, runtime, _owner) = fixture().await;
        runtime
            .store()
            .await
            .unwrap()
            .seed_loadout_roles_for_test()
            .await
            .unwrap();
        let owned_by_bootstrap = ownership("bootstrap-owner");
        let member = decide(
            &runtime,
            SkillLibraryCaller::new(
                local("static-bearer:member"),
                ["lab:read".to_string()],
                SkillLibraryTransport::bearer(SkillLibrarySurface::Resource, true),
            ),
            "member-project",
            SkillLibraryAction::Read,
            "shared-target",
            SkillLibraryTarget::SharedActive,
            "visibility-member",
        )
        .await
        .unwrap();
        assert!(!member.permits_record(&owned_by_bootstrap, SkillVisibility::Private, false));
        assert!(!member.permits_record(&owned_by_bootstrap, SkillVisibility::Tenant, false));
        assert!(member.permits_record(&owned_by_bootstrap, SkillVisibility::Tenant, true));

        let cross_tenant = LibraryOwnership::canonical(
            LibraryTenantId::from_canonical_projection("other-tenant").unwrap(),
            LibraryActorId::from_canonical_projection("bootstrap-owner").unwrap(),
        );
        assert!(!member.permits_record(&cross_tenant, SkillVisibility::Tenant, true));

        let admin = decide(
            &runtime,
            SkillLibraryCaller::new(
                local("static-bearer:admin"),
                ["lab:read".to_string()],
                SkillLibraryTransport::bearer(SkillLibrarySurface::Resource, true),
            ),
            "admin-project",
            SkillLibraryAction::Read,
            "private-target",
            SkillLibraryTarget::SharedActive,
            "visibility-admin",
        )
        .await
        .unwrap();
        assert!(!admin.permits_record(&owned_by_bootstrap, SkillVisibility::Private, false));

        let platform = decide(
            &runtime,
            browser_caller(_owner.clone(), true),
            "bootstrap-default",
            SkillLibraryAction::Read,
            "private-target",
            SkillLibraryTarget::SharedActive,
            "visibility-platform-admin",
        )
        .await
        .unwrap();
        let another_person = ownership("another-person");
        assert!(platform.permits_record(&another_person, SkillVisibility::Private, false));
        let another_team = LibraryOwnership::scoped(
            another_person.tenant_id.clone(),
            LibraryOwnerKind::Team,
            LibraryActorId::from_canonical_projection("another-team").unwrap(),
        );
        assert!(platform.permits_record(&another_team, SkillVisibility::Private, false));
    }

    #[test]
    fn team_role_ladder_separates_use_from_management() {
        let tenant = LibraryTenantId::from_canonical_projection("org-a").unwrap();
        let actor = LibraryActorId::from_canonical_projection("member").unwrap();
        let project = LibraryActorId::from_canonical_projection("project-a").unwrap();
        let team = LibraryActorId::from_canonical_projection("team-a").unwrap();
        let ownership =
            LibraryOwnership::scoped(tenant.clone(), LibraryOwnerKind::Team, team.clone());
        let teams = BTreeSet::from([team.clone()]);
        let no_management = BTreeSet::new();
        assert!(
            resolve_grant(
                &ProjectRole::Member,
                &tenant,
                &actor,
                &project,
                &teams,
                &no_management,
                false,
                SkillLibraryAction::Read,
                SkillLibraryTarget::Personal(&ownership),
            )
            .is_some()
        );
        assert!(
            resolve_grant(
                &ProjectRole::Member,
                &tenant,
                &actor,
                &project,
                &teams,
                &no_management,
                false,
                SkillLibraryAction::Archive,
                SkillLibraryTarget::Mutation(&ownership),
            )
            .is_none()
        );
        assert!(
            resolve_grant(
                &ProjectRole::Member,
                &tenant,
                &actor,
                &project,
                &teams,
                &BTreeSet::from([team]),
                false,
                SkillLibraryAction::Archive,
                SkillLibraryTarget::Mutation(&ownership),
            )
            .is_some()
        );
    }

    #[tokio::test]
    async fn one_snapshot_filters_256_personal_records_and_commit_reauth_observes_revocation() {
        let (_directory, runtime, owner) = fixture().await;
        let decision = decide(
            &runtime,
            browser_caller(owner.clone(), true),
            "bootstrap-default",
            SkillLibraryAction::List,
            "library",
            SkillLibraryTarget::SharedActive,
            "request-list",
        )
        .await
        .unwrap();
        let store = runtime.store().await.unwrap();
        let after_list_authorization = store.skill_library_authorization_count_for_test();
        let own = ownership("bootstrap-owner");
        let other = ownership("other-principal");
        let visible = (0..256)
            .filter(|index| decision.permits_personal(if index % 2 == 0 { &own } else { &other }))
            .count();
        assert_eq!(
            visible, 256,
            "platform administrators can inspect every same-tenant personal record"
        );
        assert_eq!(
            store.skill_library_authorization_count_for_test(),
            after_list_authorization
        );

        // Resource lookup performs one authorization read, then filters any number of
        // collision candidates from that immutable decision without further store access.
        let resource_decision = decide(
            &runtime,
            browser_caller(owner.clone(), true),
            "bootstrap-default",
            SkillLibraryAction::Read,
            "resource",
            SkillLibraryTarget::SharedActive,
            "request-resource",
        )
        .await
        .unwrap();
        let after_resource_authorization = store.skill_library_authorization_count_for_test();
        for _ in 0..256 {
            assert!(resource_decision.permits_personal(&own));
        }
        assert_eq!(
            store.skill_library_authorization_count_for_test(),
            after_resource_authorization
        );

        runtime
            .store()
            .await
            .unwrap()
            .execute_test_statement(
                "UPDATE platform_administrators SET status='revoked',revoked_at=2,updated_at=2 WHERE principal_id='bootstrap-owner';
                 UPDATE project_memberships SET status='suspended' WHERE membership_id='bootstrap-owner-membership'",
            )
            .await
            .unwrap();
        let executed = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let marker = std::sync::Arc::clone(&executed);
        let commit = authorize_and_commit(
            &runtime,
            browser_caller(owner, true),
            "bootstrap-default",
            SkillLibraryAction::Save,
            &CanonicalArtifactId::parse("artifact-1").unwrap(),
            SkillLibraryTarget::Mutation(&own),
            &SkillLibraryCorrelationId::parse("request-commit").unwrap(),
            move |_, _| {
                marker.store(true, std::sync::atomic::Ordering::SeqCst);
                Ok::<_, ()>(())
            },
        )
        .await;
        assert!(matches!(
            commit,
            Err(SkillLibraryCommitError::Authorization(
                SkillLibraryAuthorizationError::Denied
            ))
        ));
        assert!(!executed.load(std::sync::atomic::Ordering::SeqCst));
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn commit_authorization_linearizes_with_membership_revocation() {
        let (directory, runtime, owner) = fixture().await;
        let primary = runtime.store().await.unwrap();
        let secondary = AccessStore::open_existing_current(directory.path().join("access.db"))
            .await
            .unwrap();
        primary
            .execute_test_statement(
                "UPDATE platform_administrators SET status='revoked',revoked_at=2,updated_at=2 WHERE principal_id='bootstrap-owner'",
            )
            .await
            .unwrap();
        let (entered_tx, entered_rx) = std::sync::mpsc::sync_channel(1);
        let (release_tx, release_rx) = std::sync::mpsc::sync_channel(1);
        let guarded_store = primary.clone();
        let guarded_owner = owner.clone();
        let guarded = tokio::spawn(async move {
            guarded_store
                .authorize_skill_library_and_execute(
                    guarded_owner,
                    "bootstrap-default".to_string(),
                    Permission::AssetUse,
                    move |_| {
                        entered_tx.send(()).unwrap();
                        release_rx.recv().unwrap();
                        Ok::<_, ()>(())
                    },
                )
                .await
        });
        entered_rx.recv().unwrap();

        // Assert the ordering positively rather than by wall clock. "the
        // revocation did not finish within 500ms" passes on a loaded machine
        // even with the lease removed, because the spawned task may simply not
        // have been scheduled. Instead the revocation reports whether the lease
        // had already been released when its write landed, which is the actual
        // linearization property and has no timing budget in it.
        let lease_released = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let observed_release = std::sync::Arc::clone(&lease_released);
        let (revocation_started_tx, revocation_started_rx) = tokio::sync::oneshot::channel();
        let revocation = tokio::spawn(async move {
            let _ = revocation_started_tx.send(());
            let result = secondary
                .execute_test_statement(
                    "UPDATE project_memberships SET status='suspended' WHERE membership_id='bootstrap-owner-membership'",
                )
                .await;
            (
                result,
                observed_release.load(std::sync::atomic::Ordering::SeqCst),
            )
        });
        revocation_started_rx.await.unwrap();

        lease_released.store(true, std::sync::atomic::Ordering::SeqCst);
        release_tx.send(()).unwrap();
        assert!(guarded.await.unwrap().unwrap().is_ok());
        let (revocation_result, saw_release) =
            tokio::time::timeout(std::time::Duration::from_secs(30), revocation)
                .await
                .expect("revocation must not deadlock once the lease is released")
                .unwrap();
        revocation_result.unwrap();
        assert!(
            saw_release,
            "the revocation write must land after the commit-bound authorization lease is released"
        );

        let executed = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let marker = std::sync::Arc::clone(&executed);
        let denied = primary
            .authorize_skill_library_and_execute(
                owner,
                "bootstrap-default".to_string(),
                Permission::AssetUse,
                move |_| {
                    marker.store(true, std::sync::atomic::Ordering::SeqCst);
                    Ok::<_, ()>(())
                },
            )
            .await;
        assert!(matches!(denied, Err(AccessStoreError::NotAuthorized)));
        assert!(!executed.load(std::sync::atomic::Ordering::SeqCst));
    }

    #[test]
    fn transport_matrix_rejects_csrf_ambiguity_and_untrusted_app_metadata() {
        let browser_identity = browser("owner-subject");
        let browser_missing_origin = SkillLibraryCaller::new(
            browser_identity,
            [],
            SkillLibraryTransport::browser(false, true),
        );
        assert_eq!(
            validate_transport(&browser_missing_origin, SkillLibraryAction::Save),
            Err(SkillLibraryAuthorizationError::Denied)
        );

        let bearer = VerifiedIdentity::local_credential(
            Authenticator::StaticBearer,
            "static-bearer:callback",
        )
        .unwrap();
        let iframe_claim = SkillLibraryCaller::new(
            bearer,
            ["lab:admin".to_string()],
            SkillLibraryTransport::app_callback(true, false),
        );
        assert_eq!(
            validate_transport(&iframe_claim, SkillLibraryAction::Activate),
            Err(SkillLibraryAuthorizationError::Denied)
        );
        assert_eq!(
            validate_target_kind(
                SkillLibraryAction::Activate,
                SkillLibraryTarget::SharedActive,
            ),
            Err(SkillLibraryAuthorizationError::Denied)
        );
    }

    #[test]
    fn every_action_and_surface_has_an_explicit_transport_decision() {
        for action in SkillLibraryAction::ALL {
            let browser = SkillLibraryCaller::new(
                browser("matrix-browser"),
                [],
                SkillLibraryTransport::browser(true, true),
            );
            assert!(validate_transport(&browser, action).is_ok());

            for surface in [
                SkillLibrarySurface::ApiBearer,
                SkillLibrarySurface::Mcp,
                SkillLibrarySurface::CodeMode,
                SkillLibrarySurface::Resource,
            ] {
                let bearer = SkillLibraryCaller::new(
                    local("matrix-bearer"),
                    ["lab".to_string(), "lab:read".to_string()],
                    SkillLibraryTransport::bearer(surface, true),
                );
                assert!(
                    validate_transport(&bearer, action).is_ok(),
                    "{surface:?} {action:?}"
                );
            }

            let callback = SkillLibraryCaller::new(
                local("matrix-callback"),
                ["lab".to_string(), "lab:read".to_string()],
                SkillLibraryTransport::app_callback(true, true),
            );
            assert!(validate_transport(&callback, action).is_ok());

            let cli = SkillLibraryCaller::new(
                VerifiedIdentity::local_credential(Authenticator::UnixPeer, "uid:1000").unwrap(),
                [],
                SkillLibraryTransport {
                    surface: SkillLibrarySurface::Cli,
                    same_origin: false,
                    csrf_verified: false,
                    audience_bound: false,
                    host_established_callback: false,
                    product_credential_bound: false,
                },
            );
            assert!(validate_transport(&cli, action).is_ok());
        }
    }

    #[tokio::test]
    async fn anonymous_is_denied_before_access_resolution() {
        let (_directory, runtime, _owner) = fixture().await;
        let result = authorize_optional_at_boundary(
            &runtime,
            None,
            "bootstrap-default",
            SkillLibraryAction::Read,
            &CanonicalArtifactId::parse("artifact-anonymous").unwrap(),
            SkillLibraryTarget::SharedActive,
            &SkillLibraryCorrelationId::parse("request-anonymous").unwrap(),
        )
        .await;
        assert!(matches!(
            result,
            Err(SkillLibraryAuthorizationError::Denied)
        ));
    }

    #[test]
    fn role_action_target_matrix_is_complete_and_non_enumerating() {
        let tenant = LibraryTenantId::from_canonical_projection("company-a").unwrap();
        let actor = LibraryActorId::from_canonical_projection("actor-a").unwrap();
        let own = LibraryOwnership::canonical(tenant.clone(), actor.clone());
        let other = LibraryOwnership::canonical(
            tenant.clone(),
            LibraryActorId::from_canonical_projection("actor-b").unwrap(),
        );
        let cross_company = LibraryOwnership::canonical(
            LibraryTenantId::from_canonical_projection("company-b").unwrap(),
            actor.clone(),
        );
        for action in SkillLibraryAction::ALL {
            for role in [
                ProjectRole::Owner,
                ProjectRole::Admin,
                ProjectRole::Member,
                ProjectRole::Viewer,
            ] {
                let own_target = if action.is_mutation() {
                    SkillLibraryTarget::Mutation(&own)
                } else {
                    SkillLibraryTarget::Personal(&own)
                };
                assert!(
                    resolve_grant(
                        &role,
                        &tenant,
                        &actor,
                        &actor,
                        &BTreeSet::new(),
                        &BTreeSet::new(),
                        false,
                        action,
                        own_target,
                    )
                    .is_some()
                );

                let other_target = if action.is_mutation() {
                    SkillLibraryTarget::Mutation(&other)
                } else {
                    SkillLibraryTarget::Personal(&other)
                };
                assert!(
                    resolve_grant(
                        &role,
                        &tenant,
                        &actor,
                        &actor,
                        &BTreeSet::new(),
                        &BTreeSet::new(),
                        false,
                        action,
                        other_target,
                    )
                    .is_none(),
                    "{role:?} {action:?}"
                );

                let cross_target = if action.is_mutation() {
                    SkillLibraryTarget::Mutation(&cross_company)
                } else {
                    SkillLibraryTarget::Personal(&cross_company)
                };
                assert!(
                    resolve_grant(
                        &role,
                        &tenant,
                        &actor,
                        &actor,
                        &BTreeSet::new(),
                        &BTreeSet::new(),
                        false,
                        action,
                        cross_target,
                    )
                    .is_none()
                );
            }
        }
    }

    #[tokio::test]
    async fn protected_loadout_and_revoked_membership_fail_closed_for_all_actions() {
        let (_directory, runtime, owner) = fixture().await;
        for (index, action) in SkillLibraryAction::ALL.into_iter().enumerate() {
            let target = ownership("bootstrap-owner");
            let kind = if action.is_mutation() {
                SkillLibraryTarget::Mutation(&target)
            } else {
                SkillLibraryTarget::Personal(&target)
            };
            let missing = decide(
                &runtime,
                browser_caller(owner.clone(), true),
                "protected-or-missing-loadout",
                action,
                &format!("protected-{index}"),
                kind,
                &format!("request-protected-{index}"),
            )
            .await;
            assert!(matches!(
                missing,
                Err(SkillLibraryAuthorizationError::Denied)
            ));
        }
        runtime
            .store()
            .await
            .unwrap()
            .execute_test_statement(
                "UPDATE platform_administrators SET status='revoked',revoked_at=2,updated_at=2 WHERE principal_id='bootstrap-owner';
                 UPDATE project_memberships SET status='suspended' WHERE membership_id='bootstrap-owner-membership'",
            )
            .await
            .unwrap();
        let denied = decide(
            &runtime,
            browser_caller(owner, true),
            "bootstrap-default",
            SkillLibraryAction::Read,
            "revoked",
            SkillLibraryTarget::SharedActive,
            "request-revoked",
        )
        .await;
        assert!(matches!(
            denied,
            Err(SkillLibraryAuthorizationError::Denied)
        ));
    }

    #[test]
    fn forged_params_and_meta_have_no_policy_vocabulary() {
        let fields = std::any::type_name::<SkillLibraryCaller>();
        assert!(!fields.contains("owner"));
        let caller = SkillLibraryCaller::new(
            local("forged-client-metadata"),
            ["owner=true".to_string(), "_meta.role=admin".to_string()],
            SkillLibraryTransport::bearer(SkillLibrarySurface::Mcp, true),
        );
        assert_eq!(
            validate_transport(&caller, SkillLibraryAction::Archive),
            Err(SkillLibraryAuthorizationError::Denied)
        );
    }

    #[tokio::test]
    async fn equivalent_verified_transport_identity_resolves_same_principal() {
        let (_directory, runtime, _owner) = fixture().await;
        let oauth = VerifiedIdentity::external(
            Authenticator::OauthBearer,
            "https://accounts.google.com",
            "owner-subject",
        )
        .unwrap();
        let decision = decide(
            &runtime,
            SkillLibraryCaller::new(
                oauth,
                ["lab:read".to_string()],
                SkillLibraryTransport::bearer(SkillLibrarySurface::Resource, true),
            ),
            "bootstrap-default",
            SkillLibraryAction::Read,
            "artifact-identity",
            SkillLibraryTarget::Personal(&ownership("bootstrap-owner")),
            "request-identity",
        )
        .await
        .unwrap();
        assert_eq!(decision.audit.actor_id.unwrap().as_str(), "bootstrap-owner");
    }

    #[tokio::test]
    async fn personal_unknown_cross_tenant_and_unauthorized_share_one_denial() {
        let (_directory, runtime, owner) = fixture().await;
        let unknown = decide(
            &runtime,
            browser_caller(owner.clone(), true),
            "missing-project",
            SkillLibraryAction::Read,
            "unknown",
            SkillLibraryTarget::Personal(&ownership("someone-else")),
            "request-3",
        )
        .await;
        let unauthorized = decide(
            &runtime,
            browser_caller(owner, true),
            "bootstrap-default",
            SkillLibraryAction::Read,
            "known",
            SkillLibraryTarget::Personal(&LibraryOwnership::canonical(
                LibraryTenantId::from_canonical_projection("other-company").unwrap(),
                LibraryActorId::from_canonical_projection("someone-else").unwrap(),
            )),
            "request-4",
        )
        .await;
        assert!(matches!(
            unknown,
            Err(SkillLibraryAuthorizationError::Denied)
        ));
        assert!(matches!(
            unauthorized,
            Err(SkillLibraryAuthorizationError::Denied)
        ));
    }

    #[test]
    fn identity_provider_facts_are_not_policy_inputs() {
        let identity = browser("same-subject");
        assert!(matches!(
            identity.principal_link(),
            PrincipalLink::External { .. }
        ));
        let caller = browser_caller(identity, true);
        assert!(validate_transport(&caller, SkillLibraryAction::Create).is_ok());
    }
}
