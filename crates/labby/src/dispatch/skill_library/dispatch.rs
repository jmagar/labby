//! Skill Library mutation transaction and publication orchestration.
//!
//! Surface parsing lives in the sibling vocabulary modules. This module owns the ordering rule:
//! build an exact immutable candidate, commit durable library state, then publish that same
//! candidate without fallible work between commit and the `Arc` swap.

#![allow(
    dead_code,
    reason = "shared Skill Library core is invoked by the Wave 3 surface adapters"
)]

use std::sync::{Arc, Mutex};

use arc_swap::ArcSwap;
use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use labby_runtime::artifacts::{
    ArtifactError, ArtifactRevision, ArtifactStore, LibraryIdempotency, LibraryMutation,
    LibraryMutationOutcome, LibrarySnapshot, LibraryTimestamp, SkillLibraryFile,
    SkillLibraryRecord, SkillTransactionBoundary, SkillVisibility,
};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use serde_json::{Value, json};
use sha2::{Digest as _, Sha256};

use crate::access::AccessRuntime;

use super::audit::{
    CanonicalArtifactId, SkillLibraryAuditEvent, SkillLibraryCorrelationId,
    SkillLibraryTerminalAudit, SkillLibraryTerminalOutcome, SkillLibraryTerminalStage,
    durable_terminal_audit, record_terminal_mutation,
};
use super::auth::{
    SkillLibraryAction, SkillLibraryAuthorizationError, SkillLibraryCaller, SkillLibraryTarget,
    authorize_at_boundary,
};
use super::blocking::{
    BlockingError, BoundedBlockingExecutor, FaultInjector, FaultStage, InjectedFault,
    NoFaultInjector,
};
use super::params::{
    ArtifactParams, PageParams, ReadRevisionParams, SearchParams, ValidateParams, normalized_query,
    page_limit, validate_cursor,
};
use super::types::{
    CreateVisibility, CursorPage, MutationReceipt, OwnerSummary, ProvenanceSummary,
    RELIST_GUIDANCE, RevisionFileSummary, RevisionSummary, SkillLibrarySummary,
    ValidationRejection, ValidationResponse, VersionedRevisionFile, VersionedRevisionPage,
    VersionedSkillLibraryPage, VersionedSkillLibrarySummary,
};

/// Builds the exact post-mutation immutable generation without publishing it.
pub(crate) trait GenerationProjection<G>: Send + Sync {
    fn prepare(
        &self,
        store: &ArtifactStore,
        snapshot: &LibrarySnapshot,
        mutation: Option<&LibraryMutation>,
    ) -> Result<Arc<G>, ArtifactError>;
}

pub(crate) struct ArtifactFirstPartyProjection;

impl GenerationProjection<crate::skills::registry::FirstPartyGeneration>
    for ArtifactFirstPartyProjection
{
    fn prepare(
        &self,
        store: &ArtifactStore,
        snapshot: &LibrarySnapshot,
        mutation: Option<&LibraryMutation>,
    ) -> Result<Arc<crate::skills::registry::FirstPartyGeneration>, ArtifactError> {
        let base = crate::skills::registry::first_party_generation_manager().generation();
        crate::skills::registry::project_artifact_generation(store, snapshot, mutation, &base)
    }
}

/// Process-shared management core registered beneath the existing `skills` service.
pub(crate) struct SkillLibraryService<G> {
    pub(crate) store: Arc<ArtifactStore>,
    pub(crate) blocking: BoundedBlockingExecutor,
    pub(crate) publication: Arc<ActivationCoordinator<G>>,
    pub(crate) projection: Arc<dyn GenerationProjection<G>>,
    faults: Arc<dyn FaultInjector>,
}

impl<G: Send + Sync + 'static> SkillLibraryService<G> {
    pub(crate) fn new(
        store: Arc<ArtifactStore>,
        blocking: BoundedBlockingExecutor,
        publication: Arc<ActivationCoordinator<G>>,
        projection: Arc<dyn GenerationProjection<G>>,
    ) -> Self {
        Self {
            store,
            blocking,
            publication,
            projection,
            faults: Arc::new(NoFaultInjector),
        }
    }

    #[cfg(test)]
    pub(crate) fn with_fault_injector(mut self, faults: Arc<dyn FaultInjector>) -> Self {
        self.faults = faults;
        self
    }

    /// Commit bytes acquired by the sealed server-side source coordinator.
    /// Public JSON dispatch never accepts an
    /// [`ArtifactAcquisition`](labby_runtime::artifacts::ArtifactAcquisition).
    pub(super) async fn import_acquired(
        &self,
        runtime: &AccessRuntime,
        caller: SkillLibraryCaller,
        project_id: &str,
        acquisition: labby_runtime::artifacts::ArtifactAcquisition,
        expected_library_version: u64,
        idempotency_key: String,
        correlation_id: &SkillLibraryCorrelationId,
    ) -> Result<Value, SkillLibraryDispatchError> {
        let acquisition = validate_owned_acquisition(acquisition)?;
        super::params::validate_idempotency_key(&idempotency_key).map_err(|reason| {
            ArtifactError::InvalidField {
                field: "idempotency_key",
                reason,
            }
        })?;
        let target_id = acquisition.interchange.descriptor.id.clone();
        let target = CanonicalArtifactId::parse(target_id.clone())?;
        let materialized = self
            .blocking
            .run("skill_artifact_import_prepare", move || {
                labby_runtime::artifacts::materialize_acquired_skill_owned(acquisition)
            })
            .await
            .map_err(map_blocking)?;
        let now = LibraryTimestamp::parse(jiff::Timestamp::now().to_string())?;
        let revision_id = materialized.interchange.revision.id.clone();
        let name = materialized.interchange.descriptor.name.clone();
        let search_metadata = descriptor_search_metadata(&materialized.interchange.descriptor);
        let request_digest = labby_runtime::artifacts::canonical_json::digest(&json!({
            "action":"artifacts.import", "artifact_id":target_id,
            "revision_id": revision_id,
            "expected_library_version":expected_library_version,
            "idempotency_key":idempotency_key
        }))?;
        let store = Arc::clone(&self.store);
        let projection = Arc::clone(&self.projection);
        let publication = Arc::clone(&self.publication);
        let faults = Arc::clone(&self.faults);
        let outcome = self
            .blocking
            .run_after_admission("skill_artifact_import_commit", || async move {
                let decision = authorize_at_boundary(
                    runtime,
                    caller,
                    project_id,
                    SkillLibraryAction::Import,
                    &target,
                    SkillLibraryTarget::CreateForCaller,
                    correlation_id,
                )
                .await
                .map_err(SkillLibraryDispatchError::Authorization)?;
                let authorization = decision.authorization;
                let ownership = decision.ownership;
                let audit = decision.audit;
                let mut materialized = materialized;
                labby_runtime::artifacts::qualify_materialized_skill_owner(
                    &mut materialized,
                    &ownership,
                )?;
                let target_id = materialized.interchange.descriptor.id.clone();
                let audit = audit.with_target(&CanonicalArtifactId::parse(target_id.clone())?);
                let request_digest =
                    bind_idempotency_to_owner(&request_digest, &ownership, project_id)?;
                let audited_revision_id = revision_id.clone();
                let mutation = LibraryMutation::Create {
                    record: SkillLibraryRecord {
                        artifact_id: target_id.clone(),
                        name,
                        ownership: ownership.clone(),
                        visibility: SkillVisibility::Private,
                        archived: false,
                        active_revision_id: None,
                        latest_revision_id: revision_id.clone(),
                        latest_revision_files: library_files(&materialized.interchange.revision),
                        search_metadata,
                        provenance_provider: materialized.interchange.provenance.provider.clone(),
                        materialized: true,
                        created_at: now.clone(),
                        updated_at: now.clone(),
                    },
                };
                let committed_version = expected_library_version
                    .checked_add(1)
                    .ok_or(ArtifactError::Conflict("library_version_exhausted"))?;
                let terminal = SkillLibraryTerminalAudit::new(
                    SkillLibraryTerminalOutcome::Committed,
                    SkillLibraryTerminalStage::Commit,
                )
                .with_revision_id(&audited_revision_id)
                .with_versions(Some(committed_version), Some(committed_version));
                let idempotency = LibraryIdempotency {
                    key: idempotency_key,
                    request_digest,
                    terminal_audit: Some(runtime_durable_audit(durable_terminal_audit(
                        &audit, terminal,
                    )?)),
                };
                Ok(move || {
                    let result = (|| {
                        let snapshot = store.library_snapshot()?;
                        let generation = projection.prepare(&store, &snapshot, Some(&mutation))?;
                        publication.commit_library_outcome(
                            generation,
                            expected_library_version,
                            faults.as_ref(),
                            || {
                                store
                                    .mutate_library_with_materialized_outcome(
                                        &authorization,
                                        &ownership,
                                        expected_library_version,
                                        idempotency,
                                        mutation,
                                        now,
                                        materialized,
                                        None,
                                        |boundary| transaction_fault(faults.as_ref(), boundary),
                                    )
                                    .map_err(SkillLibraryDispatchError::Artifact)
                            },
                        )
                    })();
                    record_terminal_result(&audit, Some(&audited_revision_id), &result);
                    result
                })
            })
            .await
            .map_err(map_dispatch_blocking)?;
        let response_target = outcome.receipt().artifact_id.clone();
        self.mutation_response(response_target, outcome, false)
            .await
    }

    async fn mutation_response(
        &self,
        artifact_id: String,
        outcome: LibraryMutationOutcome,
        relist_required: bool,
    ) -> Result<Value, SkillLibraryDispatchError> {
        let receipt = outcome.receipt().clone();
        let replayed = outcome.is_replay();
        let published_library_version = match self.publication.health() {
            PublicationHealth::Ready { library_version } => library_version,
            PublicationHealth::Degraded {
                published_library_version,
                ..
            } => published_library_version,
        };
        if !replayed && let Err(error) = self.faults.check(FaultStage::AfterSwapBeforeResponse) {
            let mut terminal = receipt
                .terminal_audit
                .clone()
                .ok_or(ArtifactError::LibraryCorrupt("missing_terminal_audit"))?;
            terminal.outcome = "failed".to_owned();
            terminal.stage = "response".to_owned();
            terminal.published_version = Some(published_library_version);
            let store = Arc::clone(&self.store);
            let outcome_capability = outcome.clone();
            self.blocking
                .run("skill_library_terminal_response", move || {
                    store.update_library_terminal_audit(&outcome_capability, terminal)
                })
                .await
                .map_err(map_blocking)?;
            return Err(error.into());
        }
        let response = self
            .blocking
            .run("skill_library_receipt", move || {
                let facts = receipt.response_facts.ok_or(ArtifactError::LibraryCorrupt(
                    "missing_mutation_receipt_facts",
                ))?;
                Ok::<_, ArtifactError>(MutationReceipt {
                    outcome: if replayed { "replayed" } else { "committed" }.to_owned(),
                    artifact_id,
                    active_revision_id: facts.active_revision_id,
                    canonical_uri: facts.canonical_uri,
                    old_generation: facts.old_generation,
                    new_generation: facts.new_generation,
                    committed_library_version: facts.committed_library_version,
                    published_library_version,
                    library_digest: facts.library_digest,
                    rejected_entries: CursorPage {
                        items: Vec::new(),
                        next_cursor: None,
                    },
                    relist_required: facts.relist_required || relist_required,
                    relist_guidance: RELIST_GUIDANCE,
                    list_changed_notification: false,
                })
            })
            .await
            .map_err(map_blocking)?;
        serde_json::to_value(response).map_err(|_| SkillLibraryDispatchError::Serialization)
    }

    #[allow(clippy::too_many_arguments)]
    async fn refresh_authorized(
        &self,
        runtime: &AccessRuntime,
        caller: SkillLibraryCaller,
        project_id: &str,
        expected_version: u64,
        idempotency_key: String,
        correlation_id: &SkillLibraryCorrelationId,
    ) -> Result<Value, SkillLibraryDispatchError> {
        let store = Arc::clone(&self.store);
        let projection = Arc::clone(&self.projection);
        let (snapshot, mutation, candidate, now) = self
            .blocking
            .run("skill_library_refresh", move || {
                let snapshot = store.library_snapshot()?;
                if snapshot.version != expected_version {
                    return Err(ArtifactError::Conflict("library_version_changed"));
                }
                let mutation = LibraryMutation::Refresh {
                    artifact_id: "library".to_owned(),
                };
                let candidate = projection.prepare(&store, &snapshot, Some(&mutation))?;
                let now = LibraryTimestamp::parse(jiff::Timestamp::now().to_string())?;
                Ok::<_, ArtifactError>((snapshot, mutation, candidate, now))
            })
            .await
            .map_err(map_blocking)?;
        let request_digest = labby_runtime::artifacts::canonical_json::digest(&json!({
            "action": SkillLibraryAction::Refresh.as_str(),
            "artifact_id": "library",
            "expected_library_version": expected_version,
            "idempotency_key": idempotency_key,
        }))?;
        let outcome = commit_authorized_mutation(
            &self.blocking,
            Arc::clone(&self.publication),
            Arc::clone(&self.faults),
            runtime,
            caller,
            project_id,
            SkillLibraryAction::Refresh,
            &CanonicalArtifactId::parse("library")?,
            SkillLibraryTarget::LibraryRoot,
            correlation_id,
            Arc::clone(&self.store),
            snapshot.version,
            LibraryIdempotency {
                key: idempotency_key,
                request_digest,
                terminal_audit: None,
            },
            mutation,
            now,
            candidate,
        )
        .await
        .map_err(map_dispatch_blocking)?;
        self.mutation_response("library".to_owned(), outcome, true)
            .await
    }

    #[allow(clippy::too_many_arguments)]
    async fn mutate_existing(
        &self,
        runtime: &AccessRuntime,
        caller: SkillLibraryCaller,
        project_id: &str,
        action: SkillLibraryAction,
        artifact_id: String,
        expected_library_version: u64,
        idempotency_key: String,
        revision_id: Option<String>,
        correlation_id: &SkillLibraryCorrelationId,
    ) -> Result<Value, SkillLibraryDispatchError> {
        super::params::validate_idempotency_key(&idempotency_key).map_err(|reason| {
            ArtifactError::InvalidField {
                field: "idempotency_key",
                reason,
            }
        })?;
        let now = LibraryTimestamp::parse(jiff::Timestamp::now().to_string())?;
        let requested_revision_id = revision_id.clone();
        let mutation = match action {
            SkillLibraryAction::Activate => LibraryMutation::Activate {
                artifact_id: artifact_id.clone(),
                revision_id: revision_id.ok_or(SkillLibraryDispatchError::InvalidParams)?,
                updated_at: now.clone(),
            },
            SkillLibraryAction::Rollback => LibraryMutation::Rollback {
                artifact_id: artifact_id.clone(),
                revision_id: revision_id.ok_or(SkillLibraryDispatchError::InvalidParams)?,
                updated_at: now.clone(),
            },
            SkillLibraryAction::Deactivate => LibraryMutation::Deactivate {
                artifact_id: artifact_id.clone(),
                updated_at: now.clone(),
            },
            SkillLibraryAction::Archive => LibraryMutation::Archive {
                artifact_id: artifact_id.clone(),
                updated_at: now.clone(),
            },
            _ => return Err(SkillLibraryDispatchError::InvalidParams),
        };
        let preparation_store = Arc::clone(&self.store);
        let projection = Arc::clone(&self.projection);
        let preparation_artifact_id = artifact_id.clone();
        let preparation_mutation = mutation.clone();
        let (ownership, candidate) = self
            .blocking
            .run("skill_library_mutation_prepare", move || {
                let snapshot = preparation_store.library_snapshot()?;
                let ownership = snapshot
                    .records
                    .get(&preparation_artifact_id)
                    .ok_or(ArtifactError::NotFound("library_record"))?
                    .ownership
                    .clone();
                let candidate = projection.prepare(
                    &preparation_store,
                    &snapshot,
                    Some(&preparation_mutation),
                )?;
                Ok::<_, ArtifactError>((ownership, candidate))
            })
            .await
            .map_err(map_target_lookup)?;
        let request_digest = labby_runtime::artifacts::canonical_json::digest(&json!({
            "action": action.as_str(), "artifact_id": artifact_id,
            "revision_id": requested_revision_id,
            "expected_library_version": expected_library_version, "idempotency_key": idempotency_key
        }))?;
        let outcome = commit_authorized_mutation(
            &self.blocking,
            Arc::clone(&self.publication),
            Arc::clone(&self.faults),
            runtime,
            caller,
            project_id,
            action,
            &CanonicalArtifactId::parse(artifact_id.clone())?,
            SkillLibraryTarget::Mutation(&ownership),
            correlation_id,
            Arc::clone(&self.store),
            expected_library_version,
            LibraryIdempotency {
                key: idempotency_key,
                request_digest,
                terminal_audit: None,
            },
            mutation,
            now,
            candidate,
        )
        .await
        .map_err(map_dispatch_blocking)?;
        self.mutation_response(artifact_id, outcome, true).await
    }

    #[allow(clippy::too_many_arguments)]
    async fn create_or_save(
        &self,
        runtime: &AccessRuntime,
        caller: SkillLibraryCaller,
        project_id: &str,
        name: String,
        artifact_id: Option<String>,
        expected_revision_id: Option<String>,
        files: Vec<super::types::LogicalFileInput>,
        expected_library_version: u64,
        idempotency_key: String,
        create_visibility: CreateVisibility,
        correlation_id: &SkillLibraryCorrelationId,
    ) -> Result<Value, SkillLibraryDispatchError> {
        super::params::validate_idempotency_key(&idempotency_key).map_err(|reason| {
            ArtifactError::InvalidField {
                field: "idempotency_key",
                reason,
            }
        })?;
        let action = if artifact_id.is_some() {
            SkillLibraryAction::Save
        } else {
            SkillLibraryAction::Create
        };
        let preparation_store = Arc::clone(&self.store);
        let requested_artifact_id = artifact_id.clone();
        let (mut candidate_artifact, target_ownership) = self
            .blocking
            .run("skill_artifact_prepare", move || {
                let logical = files
                    .into_iter()
                    .map(|file| {
                        labby_runtime::artifacts::LogicalSkillFile::new(file.path, file.content)
                    })
                    .collect();
                let candidate = labby_runtime::artifacts::materialize_logical_skill(
                    &name,
                    logical,
                    Default::default(),
                )?;
                let ownership = requested_artifact_id
                    .as_ref()
                    .map(|id| {
                        preparation_store
                            .library_snapshot()?
                            .records
                            .get(id)
                            .map(|record| record.ownership.clone())
                            .ok_or(ArtifactError::NotFound("library_record"))
                    })
                    .transpose()?;
                Ok::<_, ArtifactError>((candidate, ownership))
            })
            .await
            .map_err(map_target_lookup)?;
        if let (Some(expected_id), Some(ownership)) =
            (artifact_id.as_ref(), target_ownership.as_ref())
            && candidate_artifact.interchange.descriptor.id != *expected_id
        {
            labby_runtime::artifacts::qualify_materialized_skill_owner(
                &mut candidate_artifact,
                ownership,
            )?;
        }
        if let Some(expected_id) = artifact_id.as_ref()
            && candidate_artifact.interchange.descriptor.id != *expected_id
        {
            return Err(ArtifactError::Conflict("library_artifact_identity_changed").into());
        }
        let artifact_id = candidate_artifact.interchange.descriptor.id.clone();
        let revision_id = candidate_artifact.interchange.revision.id.clone();
        let now = LibraryTimestamp::parse(jiff::Timestamp::now().to_string())?;
        let request_digest = labby_runtime::artifacts::canonical_json::digest(&json!({
            "action":action.as_str(), "artifact_id":artifact_id, "revision_id":revision_id,
            "expected_library_version":expected_library_version, "idempotency_key":idempotency_key
        }))?;
        let target_id = CanonicalArtifactId::parse(artifact_id.clone())?;
        let target = target_ownership.as_ref().map_or(
            SkillLibraryTarget::CreateForCaller,
            SkillLibraryTarget::Mutation,
        );
        let store = Arc::clone(&self.store);
        let projection = Arc::clone(&self.projection);
        let publication = Arc::clone(&self.publication);
        let faults = Arc::clone(&self.faults);
        let expected_head = expected_revision_id;
        let outcome = self
            .blocking
            .run_after_admission("skill_artifact_commit", || async move {
                let decision = authorize_at_boundary(
                    runtime,
                    caller,
                    project_id,
                    action,
                    &target_id,
                    target,
                    correlation_id,
                )
                .await
                .map_err(SkillLibraryDispatchError::Authorization)?;
                let (authorization, ownership, audit) = if action == SkillLibraryAction::Create
                    && matches!(create_visibility, CreateVisibility::Shared)
                {
                    decision.into_shared_create()?
                } else {
                    (decision.authorization, decision.ownership, decision.audit)
                };
                let mut candidate_artifact = candidate_artifact;
                if action == SkillLibraryAction::Create {
                    labby_runtime::artifacts::qualify_materialized_skill_owner(
                        &mut candidate_artifact,
                        &ownership,
                    )?;
                }
                let artifact_id = candidate_artifact.interchange.descriptor.id.clone();
                let audit = if action == SkillLibraryAction::Create {
                    audit.with_target(&CanonicalArtifactId::parse(artifact_id.clone())?)
                } else {
                    audit
                };
                let request_digest =
                    bind_idempotency_to_owner(&request_digest, &ownership, project_id)?;
                let mutation = if action == SkillLibraryAction::Create {
                    LibraryMutation::Create {
                        record: SkillLibraryRecord {
                            artifact_id: artifact_id.clone(),
                            name: candidate_artifact.interchange.descriptor.name.clone(),
                            ownership: ownership.clone(),
                            visibility: match create_visibility {
                                CreateVisibility::Private => SkillVisibility::Private,
                                CreateVisibility::Shared => SkillVisibility::Tenant,
                            },
                            archived: false,
                            active_revision_id: None,
                            latest_revision_id: revision_id.clone(),
                            latest_revision_files: library_files(
                                &candidate_artifact.interchange.revision,
                            ),
                            search_metadata: descriptor_search_metadata(
                                &candidate_artifact.interchange.descriptor,
                            ),
                            provenance_provider: candidate_artifact
                                .interchange
                                .provenance
                                .provider
                                .clone(),
                            materialized: true,
                            created_at: now.clone(),
                            updated_at: now.clone(),
                        },
                    }
                } else {
                    LibraryMutation::Save {
                        artifact_id: artifact_id.clone(),
                        revision_id: revision_id.clone(),
                        updated_at: now.clone(),
                    }
                };
                let audited_revision_id = revision_id.clone();
                let committed_version = expected_library_version
                    .checked_add(1)
                    .ok_or(ArtifactError::Conflict("library_version_exhausted"))?;
                let terminal = SkillLibraryTerminalAudit::new(
                    SkillLibraryTerminalOutcome::Committed,
                    SkillLibraryTerminalStage::Commit,
                )
                .with_revision_id(&audited_revision_id)
                .with_versions(Some(committed_version), Some(committed_version));
                let idempotency = LibraryIdempotency {
                    key: idempotency_key,
                    request_digest,
                    terminal_audit: Some(runtime_durable_audit(durable_terminal_audit(
                        &audit, terminal,
                    )?)),
                };
                Ok(move || {
                    let result = (|| {
                        let snapshot = store.library_snapshot()?;
                        let generation = projection.prepare(&store, &snapshot, Some(&mutation))?;
                        publication.commit_library_outcome(
                            generation,
                            expected_library_version,
                            faults.as_ref(),
                            || {
                                store
                                    .mutate_library_with_materialized_outcome(
                                        &authorization,
                                        &ownership,
                                        expected_library_version,
                                        idempotency,
                                        mutation,
                                        now,
                                        candidate_artifact,
                                        expected_head.as_deref(),
                                        |boundary| transaction_fault(faults.as_ref(), boundary),
                                    )
                                    .map_err(SkillLibraryDispatchError::Artifact)
                            },
                        )
                    })();
                    record_terminal_result(&audit, Some(&audited_revision_id), &result);
                    result
                })
            })
            .await
            .map_err(map_dispatch_blocking)?;
        let response_artifact_id = outcome.receipt().artifact_id.clone();
        self.mutation_response(response_artifact_id, outcome, false)
            .await
    }

    pub(crate) async fn dispatch(
        &self,
        runtime: &AccessRuntime,
        caller: SkillLibraryCaller,
        project_id: &str,
        action: &str,
        params: Value,
        correlation_id: &SkillLibraryCorrelationId,
    ) -> Result<Value, SkillLibraryDispatchError> {
        match action {
            "artifacts.search" => {
                let params: SearchParams = parse(params)?;
                let query = normalized_query(params.query).map_err(|reason| {
                    ArtifactError::InvalidField {
                        field: "query",
                        reason,
                    }
                })?;
                let decision = authorize_at_boundary(
                    runtime,
                    caller,
                    project_id,
                    SkillLibraryAction::Search,
                    &CanonicalArtifactId::parse("library")?,
                    SkillLibraryTarget::SharedActive,
                    correlation_id,
                )
                .await?;
                let store = Arc::clone(&self.store);
                let published_library_version = published_version(&self.publication);
                let page = self
                    .blocking
                    .run("artifact_search", move || {
                        let snapshot = store.library_snapshot()?;
                        list_page_visible(
                            &store,
                            &snapshot,
                            &decision,
                            params.cursor,
                            params.limit,
                            published_library_version,
                            Some(&query),
                        )
                    })
                    .await
                    .map_err(map_blocking)?;
                serde_json::to_value(page).map_err(|_| SkillLibraryDispatchError::Serialization)
            }
            "artifacts.list" => {
                let params: PageParams = parse(params)?;
                let decision = authorize_at_boundary(
                    runtime,
                    caller,
                    project_id,
                    SkillLibraryAction::List,
                    &CanonicalArtifactId::parse("library")?,
                    SkillLibraryTarget::SharedActive,
                    correlation_id,
                )
                .await?;
                let store = Arc::clone(&self.store);
                let published_library_version = published_version(&self.publication);
                let page = self
                    .blocking
                    .run("skill_library_list", move || {
                        let snapshot = store.library_snapshot()?;
                        list_page_visible(
                            &store,
                            &snapshot,
                            &decision,
                            params.cursor,
                            params.limit,
                            published_library_version,
                            None,
                        )
                    })
                    .await
                    .map_err(map_blocking)?;
                serde_json::to_value(page).map_err(|_| SkillLibraryDispatchError::Serialization)
            }
            "artifacts.get" => {
                let params: ArtifactParams = parse(params)?;
                let target = CanonicalArtifactId::parse(params.artifact_id.clone())?;
                let store = Arc::clone(&self.store);
                let artifact_id = params.artifact_id.clone();
                let record = self
                    .blocking
                    .run("skill_library_get_target", move || {
                        store
                            .library_snapshot()?
                            .records
                            .get(&artifact_id)
                            .cloned()
                            .ok_or(ArtifactError::NotFound("library_record"))
                    })
                    .await
                    .map_err(map_target_lookup)?;
                let policy_target = read_target(&record)?;
                let decision = authorize_at_boundary(
                    runtime,
                    caller,
                    project_id,
                    SkillLibraryAction::Get,
                    &target,
                    policy_target,
                    correlation_id,
                )
                .await?;
                let store = Arc::clone(&self.store);
                let published_library_version = published_version(&self.publication);
                let item = self
                    .blocking
                    .run("skill_library_get", move || {
                        let snapshot = store.library_snapshot()?;
                        let visible = snapshot
                            .records
                            .get(&params.artifact_id)
                            .filter(|record| {
                                !record.archived || decision.permits_personal(&record.ownership)
                            })
                            .filter(|record| {
                                decision.permits_record(
                                    &record.ownership,
                                    record.visibility,
                                    record.active_revision_id.is_some(),
                                )
                            })
                            .ok_or(ArtifactError::NotFound("library_record"))?;
                        Ok(VersionedSkillLibrarySummary {
                            library_version: snapshot.version,
                            item: summary(
                                visible,
                                &decision,
                                snapshot.version,
                                published_library_version,
                            ),
                        })
                    })
                    .await
                    .map_err(map_blocking)?;
                serde_json::to_value(item).map_err(|_| SkillLibraryDispatchError::Serialization)
            }
            "artifacts.read" => {
                let params: ReadRevisionParams = parse(params)?;
                let target = CanonicalArtifactId::parse(params.artifact_id.clone())?;
                let target_store = Arc::clone(&self.store);
                let target_artifact = params.artifact_id.clone();
                let record = self
                    .blocking
                    .run("skill_library_read_target", move || {
                        target_store
                            .library_snapshot()?
                            .records
                            .get(&target_artifact)
                            .cloned()
                            .ok_or(ArtifactError::NotFound("library_record"))
                    })
                    .await
                    .map_err(map_target_lookup)?;
                let policy_target = read_target(&record)?;
                let decision = authorize_at_boundary(
                    runtime,
                    caller,
                    project_id,
                    SkillLibraryAction::Read,
                    &target,
                    policy_target,
                    correlation_id,
                )
                .await?;
                let store = Arc::clone(&self.store);
                let artifact_id = params.artifact_id.clone();
                let revision_id = params.revision_id.clone();
                let path = params.path.clone();
                let (library_version, bytes) = self
                    .blocking
                    .run("skill_library_read", move || {
                        let snapshot = store.library_snapshot()?;
                        let current = snapshot
                            .records
                            .get(&artifact_id)
                            .filter(|record| {
                                !record.archived || decision.permits_personal(&record.ownership)
                            })
                            .filter(|record| {
                                decision.permits_record(
                                    &record.ownership,
                                    record.visibility,
                                    record.active_revision_id.is_some(),
                                )
                            })
                            .ok_or(ArtifactError::NotFound("library_record"))?;
                        if current.visibility == SkillVisibility::Tenant
                            && !decision.permits_personal(&current.ownership)
                            && current.active_revision_id.as_deref() != Some(&revision_id)
                        {
                            return Err(ArtifactError::NotFound("library_record"));
                        }
                        let bytes =
                            store.read_skill_revision_file(&artifact_id, &revision_id, &path)?;
                        Ok((snapshot.version, bytes))
                    })
                    .await
                    .map_err(map_blocking)?;
                let text = String::from_utf8(bytes).map_err(|_| ArtifactError::InvalidField {
                    field: "content",
                    reason: "non_utf8",
                })?;
                serde_json::to_value(VersionedRevisionFile {
                    library_version,
                    artifact_id: params.artifact_id,
                    revision_id: params.revision_id,
                    path: params.path,
                    text,
                })
                .map_err(|_| SkillLibraryDispatchError::Serialization)
            }
            "artifacts.history" => {
                #[derive(serde::Deserialize)]
                #[serde(deny_unknown_fields)]
                struct History {
                    artifact_id: String,
                    cursor: Option<String>,
                    limit: Option<usize>,
                }
                let params: History = parse(params)?;
                let target = CanonicalArtifactId::parse(params.artifact_id.clone())?;
                let target_store = Arc::clone(&self.store);
                let target_artifact = params.artifact_id.clone();
                let record = self
                    .blocking
                    .run("skill_library_history_target", move || {
                        target_store
                            .library_snapshot()?
                            .records
                            .get(&target_artifact)
                            .cloned()
                            .ok_or(ArtifactError::NotFound("library_record"))
                    })
                    .await
                    .map_err(map_target_lookup)?;
                let policy_target = read_target(&record)?;
                let decision = authorize_at_boundary(
                    runtime,
                    caller,
                    project_id,
                    SkillLibraryAction::History,
                    &target,
                    policy_target,
                    correlation_id,
                )
                .await?;
                let store = Arc::clone(&self.store);
                let page = self
                    .blocking
                    .run("skill_library_history", move || {
                        let snapshot = store.library_snapshot()?;
                        let record = snapshot
                            .records
                            .get(&params.artifact_id)
                            .filter(|record| {
                                decision.permits_record(
                                    &record.ownership,
                                    record.visibility,
                                    record.active_revision_id.is_some(),
                                )
                            })
                            .ok_or(ArtifactError::NotFound("library_record"))?;
                        let page = history_page(
                            &store,
                            record,
                            &decision,
                            snapshot.version,
                            params.cursor,
                            params.limit,
                        )?;
                        Ok(VersionedRevisionPage {
                            library_version: snapshot.version,
                            items: page.items,
                            next_cursor: page.next_cursor,
                        })
                    })
                    .await
                    .map_err(map_blocking)?;
                serde_json::to_value(page).map_err(|_| SkillLibraryDispatchError::Serialization)
            }
            "artifacts.validate" => {
                let params: ValidateParams = parse(params)?;
                let decision = authorize_at_boundary(
                    runtime,
                    caller,
                    project_id,
                    SkillLibraryAction::Validate,
                    &CanonicalArtifactId::parse("validation")?,
                    SkillLibraryTarget::SharedActive,
                    correlation_id,
                )
                .await?;
                let candidate = self
                    .blocking
                    .run("skill_library_validate", move || {
                        let files = params
                            .files
                            .into_iter()
                            .map(|file| {
                                labby_runtime::artifacts::LogicalSkillFile::new(
                                    file.path,
                                    file.content,
                                )
                            })
                            .collect();
                        let mut materialized = labby_runtime::artifacts::materialize_logical_skill(
                            &params.name,
                            files,
                            Default::default(),
                        )?;
                        labby_runtime::artifacts::qualify_materialized_skill_owner(
                            &mut materialized,
                            &decision.ownership,
                        )?;
                        Ok(materialized)
                    })
                    .await;
                let response = match candidate {
                    Ok(candidate) => ValidationResponse {
                        valid: true,
                        artifact_id: Some(candidate.interchange.descriptor.id),
                        revision_id: Some(candidate.interchange.revision.id),
                        rejections: Vec::new(),
                    },
                    Err(BlockingError::Operation(error)) => ValidationResponse {
                        valid: false,
                        artifact_id: None,
                        revision_id: None,
                        rejections: vec![validation_rejection(error)?],
                    },
                    Err(error) => return Err(map_blocking(error)),
                };
                serde_json::to_value(response).map_err(|_| SkillLibraryDispatchError::Serialization)
            }
            "artifacts.refresh" => {
                #[derive(serde::Deserialize)]
                #[serde(deny_unknown_fields)]
                struct Refresh {
                    expected_library_version: u64,
                    idempotency_key: String,
                }
                let params: Refresh = parse(params)?;
                super::params::validate_idempotency_key(&params.idempotency_key).map_err(
                    |reason| ArtifactError::InvalidField {
                        field: "idempotency_key",
                        reason,
                    },
                )?;
                self.refresh_authorized(
                    runtime,
                    caller,
                    project_id,
                    params.expected_library_version,
                    params.idempotency_key,
                    correlation_id,
                )
                .await
            }
            "artifacts.activate" | "artifacts.rollback" => {
                let params: super::params::RevisionMutationParams = parse(params)?;
                let action = if action.ends_with("activate") {
                    SkillLibraryAction::Activate
                } else {
                    SkillLibraryAction::Rollback
                };
                self.mutate_existing(
                    runtime,
                    caller,
                    project_id,
                    action,
                    params.artifact_id,
                    params.expected_library_version,
                    params.idempotency_key,
                    Some(params.expected_revision_id),
                    correlation_id,
                )
                .await
            }
            "artifacts.create" => {
                let params: super::params::CreateParams = parse(params)?;
                self.create_or_save(
                    runtime,
                    caller,
                    project_id,
                    params.name,
                    None,
                    None,
                    params.files,
                    params.expected_library_version,
                    params.idempotency_key,
                    params.visibility,
                    correlation_id,
                )
                .await
            }
            "artifacts.save" => {
                let params: super::params::SaveParams = parse(params)?;
                let store = Arc::clone(&self.store);
                let artifact_id = params.artifact_id.clone();
                let name = self
                    .blocking
                    .run("skill_library_save_target", move || {
                        store
                            .library_snapshot()?
                            .records
                            .get(&artifact_id)
                            .map(|record| record.name.clone())
                            .ok_or(ArtifactError::NotFound("library_record"))
                    })
                    .await
                    .map_err(map_target_lookup)?;
                self.create_or_save(
                    runtime,
                    caller,
                    project_id,
                    name,
                    Some(params.artifact_id),
                    Some(params.expected_revision_id),
                    params.files,
                    params.expected_library_version,
                    params.idempotency_key,
                    CreateVisibility::Private,
                    correlation_id,
                )
                .await
            }
            // Import is intentionally unavailable through the generic JSON dispatcher. Public
            // adapters parse a server-side source selector and the sealed coordinator passes the
            // owned, verified acquisition to `import_acquired` without serializing its bytes.
            "artifacts.import" | "artifacts.import_batch" => {
                Err(SkillLibraryDispatchError::InvalidParams)
            }
            "artifacts.deactivate" | "artifacts.archive" => {
                let params: super::params::LibraryMutationParams = parse(params)?;
                let action = if action.ends_with("deactivate") {
                    SkillLibraryAction::Deactivate
                } else {
                    SkillLibraryAction::Archive
                };
                self.mutate_existing(
                    runtime,
                    caller,
                    project_id,
                    action,
                    params.artifact_id,
                    params.expected_library_version,
                    params.idempotency_key,
                    None,
                    correlation_id,
                )
                .await
            }
            _ => Err(SkillLibraryDispatchError::UnknownAction),
        }
    }
}

/// Validate a coordinator-owned payload without changing its allocation. Keeping this move-only
/// seam separate makes it impossible to accidentally reintroduce a JSON/string conversion at the
/// product boundary.
pub(super) fn validate_owned_acquisition(
    acquisition: labby_runtime::artifacts::ArtifactAcquisition,
) -> Result<labby_runtime::artifacts::ArtifactAcquisition, ArtifactError> {
    acquisition.validate()?;
    Ok(acquisition)
}

fn parse<T: DeserializeOwned>(value: Value) -> Result<T, SkillLibraryDispatchError> {
    serde_json::from_value(value).map_err(|_| SkillLibraryDispatchError::InvalidParams)
}

fn library_files(revision: &ArtifactRevision) -> Vec<SkillLibraryFile> {
    revision
        .components
        .iter()
        .map(|component| SkillLibraryFile {
            path: component.path.clone(),
            digest: component.digest.clone(),
            size: component.size,
            media_type: component.media_type.clone(),
        })
        .collect()
}

fn map_blocking(error: BlockingError<ArtifactError>) -> SkillLibraryDispatchError {
    match error {
        BlockingError::Operation(error) => error.into(),
        BlockingError::Busy { .. } => ArtifactError::Busy.into(),
        BlockingError::Timeout { operation } => {
            SkillLibraryDispatchError::BlockingTimeout { operation }
        }
        BlockingError::WorkerFailed { operation } => {
            tracing::error!(operation, "Skill Library blocking worker failed");
            SkillLibraryDispatchError::BlockingWorkerFailed { operation }
        }
    }
}

/// Collapse target-resolution misses into the same denial returned for inaccessible records.
///
/// These lookups happen before authorization can inspect a record's ownership. Keeping this
/// normalization at that seam prevents target IDs from becoming an existence oracle while
/// preserving ordinary `not_found` errors for authorized revision and file lookups.
fn map_target_lookup(error: BlockingError<ArtifactError>) -> SkillLibraryDispatchError {
    match error {
        BlockingError::Operation(ArtifactError::NotFound("library_record")) => {
            SkillLibraryAuthorizationError::Denied.into()
        }
        error => map_blocking(error),
    }
}

fn map_dispatch_blocking(
    error: BlockingError<SkillLibraryDispatchError>,
) -> SkillLibraryDispatchError {
    match error {
        BlockingError::Operation(error) => error,
        BlockingError::Busy { .. } => ArtifactError::Busy.into(),
        BlockingError::Timeout { operation } | BlockingError::WorkerFailed { operation } => {
            SkillLibraryDispatchError::BlockingIndeterminate { operation }
        }
    }
}

fn transaction_fault(
    faults: &dyn FaultInjector,
    boundary: SkillTransactionBoundary,
) -> Result<(), ArtifactError> {
    let stage = match boundary {
        SkillTransactionBoundary::IntentWrite
        | SkillTransactionBoundary::LibraryWrite
        | SkillTransactionBoundary::PromotionWrite
        | SkillTransactionBoundary::AppliedWrite => FaultStage::DiskWrite,
        SkillTransactionBoundary::IntentFileSync
        | SkillTransactionBoundary::LibraryFileSync
        | SkillTransactionBoundary::PromotionFileSync
        | SkillTransactionBoundary::AppliedFileSync => FaultStage::FileSync,
        SkillTransactionBoundary::IntentRename
        | SkillTransactionBoundary::LibraryRename
        | SkillTransactionBoundary::PromotionRename
        | SkillTransactionBoundary::AppliedRename => FaultStage::RenameCommit,
        SkillTransactionBoundary::IntentParentSync
        | SkillTransactionBoundary::LibraryParentSync
        | SkillTransactionBoundary::PromotionParentSync
        | SkillTransactionBoundary::AppliedParentSync => FaultStage::ParentSync,
    };
    faults
        .check(stage)
        .map_err(|_| ArtifactError::Conflict("injected_transaction_fault"))
}

fn summary(
    record: &SkillLibraryRecord,
    decision: &super::auth::SkillLibraryAuthorizationDecision,
    current_generation: u64,
    published_library_version: u64,
) -> SkillLibrarySummary {
    let personal = decision.permits_personal(&record.ownership);
    let materialized = record.materialized;
    let active = record.active_revision_id.is_some();
    SkillLibrarySummary {
        artifact_id: record.artifact_id.clone(),
        name: record.name.clone(),
        archived: record.archived,
        active_revision_id: record.active_revision_id.clone(),
        latest_revision_id: record.latest_revision_id.clone(),
        visibility: match record.visibility {
            SkillVisibility::Private => "private",
            SkillVisibility::Tenant => "shared",
        },
        access_label: if personal { "personal" } else { "shared" },
        can_mutate: personal,
        owner: OwnerSummary {
            relationship: if decision.owns(&record.ownership) {
                "self"
            } else {
                "other"
            },
        },
        provenance: ProvenanceSummary {
            source: provenance_source(record.provenance_provider.as_deref()),
        },
        materialized,
        canonical_uri: active.then(|| format!("skill://labby/{}/SKILL.md", record.name)),
        current_generation,
        published_library_version,
        allowed_actions: item_allowed_actions(personal, active),
        latest_revision_files: record
            .latest_revision_files
            .iter()
            .map(|component| RevisionFileSummary {
                path: component.path.clone(),
                digest: component.digest.clone(),
                size: component.size,
                media_type: component.media_type.clone(),
            })
            .collect(),
    }
}

fn provenance_source(provider: Option<&str>) -> &'static str {
    match provider {
        None => "local",
        Some("depot") => "depot",
        Some("repository" | "git" | "github") => "repository",
        Some(_) => "imported",
    }
}

fn item_allowed_actions(personal: bool, active: bool) -> Vec<&'static str> {
    let mut actions = vec!["artifacts.get", "artifacts.read", "artifacts.history"];
    if personal {
        actions.extend(["artifacts.save", "artifacts.archive", "artifacts.activate"]);
        if active {
            actions.push("artifacts.deactivate");
        }
        actions.push("artifacts.rollback");
    }
    actions
}

fn published_version<G>(publication: &ActivationCoordinator<G>) -> u64 {
    match publication.health() {
        PublicationHealth::Ready { library_version } => library_version,
        PublicationHealth::Degraded {
            published_library_version,
            ..
        } => published_library_version,
    }
}

fn bind_idempotency_to_owner(
    request_digest: &str,
    ownership: &labby_runtime::artifacts::LibraryOwnership,
    project_id: &str,
) -> Result<String, ArtifactError> {
    labby_runtime::artifacts::canonical_json::digest(&(
        request_digest,
        ownership.tenant_id.as_str(),
        ownership.owner_kind(),
        ownership.owner_id.as_str(),
        project_id,
    ))
}

fn read_target(record: &SkillLibraryRecord) -> Result<SkillLibraryTarget<'_>, ArtifactError> {
    match record.visibility {
        SkillVisibility::Private => Ok(SkillLibraryTarget::Personal(&record.ownership)),
        SkillVisibility::Tenant if record.active_revision_id.is_some() => {
            Ok(SkillLibraryTarget::SharedActive)
        }
        SkillVisibility::Tenant => Ok(SkillLibraryTarget::Personal(&record.ownership)),
    }
}

fn list_page_visible(
    store: &ArtifactStore,
    snapshot: &LibrarySnapshot,
    decision: &super::auth::SkillLibraryAuthorizationDecision,
    cursor: Option<String>,
    limit: Option<usize>,
    published_library_version: u64,
    query: Option<&str>,
) -> Result<VersionedSkillLibraryPage, ArtifactError> {
    let cursor = validate_cursor(cursor).map_err(|reason| ArtifactError::InvalidField {
        field: "cursor",
        reason,
    })?;
    let limit = page_limit(limit).map_err(|reason| ArtifactError::InvalidField {
        field: "limit",
        reason,
    })?;
    let cursor_binding = ListCursorBinding::new(decision, snapshot.version, query);
    let cursor = cursor
        .map(|encoded| decode_list_cursor(&encoded, &cursor_binding))
        .transpose()?;
    let lower_bound = cursor.map_or(std::ops::Bound::Unbounded, |cursor| {
        std::ops::Bound::Excluded(cursor.artifact_id)
    });
    let records = snapshot
        .records
        .range((lower_bound, std::ops::Bound::Unbounded));
    let page_records = records
        .map(|(_, record)| record)
        .filter(|record| !record.archived || decision.permits_personal(&record.ownership))
        .filter(|record| {
            decision.permits_record(
                &record.ownership,
                record.visibility,
                record.active_revision_id.is_some(),
            )
        })
        .filter(|record| artifact_matches(store, record, query))
        .take(limit + 1)
        .collect::<Vec<_>>();
    let has_more = page_records.len() > limit;
    let items = page_records
        .into_iter()
        .take(limit)
        .map(|record| {
            summary(
                record,
                decision,
                snapshot.version,
                published_library_version,
            )
        })
        .collect::<Vec<_>>();
    let next_cursor = has_more
        .then(|| {
            encode_list_cursor(
                &cursor_binding,
                items
                    .last()
                    .expect("non-empty paginated page")
                    .artifact_id
                    .clone(),
            )
        })
        .transpose()?;
    Ok(VersionedSkillLibraryPage {
        library_version: snapshot.version,
        published_library_version,
        can_create: true,
        create_visibilities: vec!["private", "shared"],
        allowed_actions: vec!["artifacts.validate", "artifacts.create", "artifacts.import"],
        items,
        next_cursor,
    })
}

#[derive(Serialize, Deserialize, PartialEq, Eq)]
struct ListCursorBinding {
    version: u8,
    context_digest: String,
    library_version: u64,
    query_digest: Option<String>,
}

impl ListCursorBinding {
    fn new(
        decision: &super::auth::SkillLibraryAuthorizationDecision,
        library_version: u64,
        query: Option<&str>,
    ) -> Self {
        let (tenant, principal, project, authority_generation) = decision.cursor_binding();
        let mut context = Sha256::new();
        for value in [tenant, principal, project] {
            context.update(value.len().to_be_bytes());
            context.update(value.as_bytes());
        }
        for team_id in decision.cursor_team_ids() {
            context.update(team_id.len().to_be_bytes());
            context.update(team_id.as_bytes());
        }
        context.update(authority_generation.to_be_bytes());
        Self {
            version: 1,
            context_digest: hex::encode(context.finalize()),
            library_version,
            query_digest: query.map(|value| hex::encode(Sha256::digest(value.as_bytes()))),
        }
    }
}

#[derive(Serialize, Deserialize)]
struct ListCursor {
    #[serde(flatten)]
    binding: ListCursorBinding,
    artifact_id: String,
}

fn encode_list_cursor(
    binding: &ListCursorBinding,
    artifact_id: String,
) -> Result<String, ArtifactError> {
    let bytes = serde_json::to_vec(&ListCursor {
        binding: ListCursorBinding {
            version: binding.version,
            context_digest: binding.context_digest.clone(),
            library_version: binding.library_version,
            query_digest: binding.query_digest.clone(),
        },
        artifact_id,
    })
    .map_err(|_| invalid_list_cursor())?;
    Ok(URL_SAFE_NO_PAD.encode(bytes))
}

fn decode_list_cursor(
    encoded: &str,
    expected: &ListCursorBinding,
) -> Result<ListCursor, ArtifactError> {
    let bytes = URL_SAFE_NO_PAD
        .decode(encoded)
        .map_err(|_| invalid_list_cursor())?;
    let cursor: ListCursor = serde_json::from_slice(&bytes).map_err(|_| invalid_list_cursor())?;
    if cursor.binding != *expected || cursor.artifact_id.is_empty() {
        return Err(invalid_list_cursor());
    }
    Ok(cursor)
}

fn invalid_list_cursor() -> ArtifactError {
    ArtifactError::InvalidField {
        field: "cursor",
        reason: "context_mismatch",
    }
}

fn artifact_matches(
    store: &ArtifactStore,
    record: &SkillLibraryRecord,
    query: Option<&str>,
) -> bool {
    let Some(query) = query else {
        return true;
    };
    if record.artifact_id.to_lowercase().contains(query)
        || record.name.to_lowercase().contains(query)
        || record
            .provenance_provider
            .as_deref()
            .is_some_and(|provider| provider.to_lowercase().contains(query))
        || record
            .search_metadata
            .iter()
            .any(|value| value.to_lowercase().contains(query))
    {
        return true;
    }
    // Records written before the snapshot search index was introduced deserialize with an empty
    // index. Consult durable descriptor data only for those records; newly indexed libraries keep
    // the scan filesystem-free.
    if !record.search_metadata.is_empty() {
        return false;
    }
    let Ok(artifact) = store.get(&record.artifact_id) else {
        return false;
    };
    descriptor_search_metadata(&artifact.descriptor)
        .iter()
        .any(|value| value.to_lowercase().contains(query))
}

fn descriptor_search_metadata(
    descriptor: &labby_runtime::artifacts::ArtifactDescriptor,
) -> Vec<String> {
    descriptor
        .title
        .iter()
        .chain(descriptor.description.iter())
        .chain(descriptor.tags.iter())
        .cloned()
        .collect()
}

fn validation_rejection(error: ArtifactError) -> Result<ValidationRejection, ArtifactError> {
    Ok(match error {
        ArtifactError::InvalidField { field, reason } => ValidationRejection {
            field,
            code: reason,
            path: None,
        },
        ArtifactError::LogicalSkillFile { path, reason } => ValidationRejection {
            field: "files",
            code: reason,
            path: Some(path),
        },
        ArtifactError::UnsafePath(code) => ValidationRejection {
            field: "files",
            code,
            path: None,
        },
        ArtifactError::LimitExceeded { what, .. } => ValidationRejection {
            field: what,
            code: "limit_exceeded",
            path: None,
        },
        ArtifactError::SkillVerification => ValidationRejection {
            field: "files",
            code: "skill_verification",
            path: None,
        },
        ArtifactError::UnsupportedSchema => ValidationRejection {
            field: "skill",
            code: "unsupported_schema",
            path: None,
        },
        ArtifactError::Conflict(code) => ValidationRejection {
            field: "skill",
            code,
            path: None,
        },
        other => return Err(other),
    })
}

/// Revision history is metadata-only; bodies remain exclusive to `artifacts.read`.
pub(crate) fn history_page(
    store: &ArtifactStore,
    record: &SkillLibraryRecord,
    decision: &super::auth::SkillLibraryAuthorizationDecision,
    library_version: u64,
    cursor: Option<String>,
    limit: Option<usize>,
) -> Result<CursorPage<RevisionSummary>, ArtifactError> {
    let cursor = validate_cursor(cursor).map_err(|reason| ArtifactError::InvalidField {
        field: "cursor",
        reason,
    })?;
    let limit = page_limit(limit).map_err(|reason| ArtifactError::InvalidField {
        field: "limit",
        reason,
    })?;
    let artifact = store.get(&record.artifact_id)?;
    let binding = HistoryCursorBinding::new(
        decision,
        library_version,
        &record.artifact_id,
        &record.ownership,
    );
    let (page_start, end) =
        history_page_window(&artifact.revision_ids, cursor.as_deref(), limit, &binding)?;
    let ids = artifact.revision_ids[page_start..end]
        .iter()
        .rev()
        .map(String::as_str)
        .collect::<Vec<_>>();
    let items = store
        .revision_batch(&record.artifact_id, &ids)?
        .into_iter()
        .map(|revision| RevisionSummary {
            revision_id: revision.id,
            created_at: revision.authored_at,
        })
        .collect();
    let next_cursor = (page_start > 0)
        .then(|| {
            encode_history_cursor(
                &binding,
                page_start,
                ids.last().expect("non-empty paginated page"),
            )
        })
        .transpose()?;
    Ok(CursorPage { items, next_cursor })
}

#[derive(Serialize, Deserialize, PartialEq, Eq)]
struct HistoryCursorBinding {
    version: u8,
    context_digest: String,
    library_version: u64,
    artifact_id: String,
    owner_kind: labby_runtime::artifacts::LibraryOwnerKind,
    owner_id: String,
}

impl HistoryCursorBinding {
    fn new(
        decision: &super::auth::SkillLibraryAuthorizationDecision,
        library_version: u64,
        artifact_id: &str,
        ownership: &labby_runtime::artifacts::LibraryOwnership,
    ) -> Self {
        let list = ListCursorBinding::new(decision, library_version, None);
        Self {
            version: 1,
            context_digest: list.context_digest,
            library_version,
            artifact_id: artifact_id.to_owned(),
            owner_kind: ownership.owner_kind(),
            owner_id: ownership.owner_id.as_str().to_owned(),
        }
    }
}

#[derive(Serialize, Deserialize)]
struct HistoryCursor {
    #[serde(flatten)]
    binding: HistoryCursorBinding,
    position: usize,
    revision_id: String,
}

fn history_page_window(
    revision_ids: &[String],
    cursor: Option<&str>,
    limit: usize,
    binding: &HistoryCursorBinding,
) -> Result<(usize, usize), ArtifactError> {
    let end = cursor.map_or(Ok(revision_ids.len()), |cursor| {
        let cursor = decode_history_cursor(cursor, binding)?;
        if revision_ids.get(cursor.position).map(String::as_str) != Some(&cursor.revision_id) {
            return Err(invalid_history_cursor());
        }
        Ok(cursor.position)
    })?;
    Ok((end.saturating_sub(limit), end))
}

fn encode_history_cursor(
    binding: &HistoryCursorBinding,
    position: usize,
    revision_id: &str,
) -> Result<String, ArtifactError> {
    serde_json::to_vec(&HistoryCursor {
        binding: HistoryCursorBinding {
            version: binding.version,
            context_digest: binding.context_digest.clone(),
            library_version: binding.library_version,
            artifact_id: binding.artifact_id.clone(),
            owner_kind: binding.owner_kind,
            owner_id: binding.owner_id.clone(),
        },
        position,
        revision_id: revision_id.to_owned(),
    })
    .map(|bytes| URL_SAFE_NO_PAD.encode(bytes))
    .map_err(|_| invalid_history_cursor())
}

fn decode_history_cursor(
    cursor: &str,
    expected: &HistoryCursorBinding,
) -> Result<HistoryCursor, ArtifactError> {
    let bytes = URL_SAFE_NO_PAD
        .decode(cursor)
        .map_err(|_| invalid_history_cursor())?;
    let cursor: HistoryCursor =
        serde_json::from_slice(&bytes).map_err(|_| invalid_history_cursor())?;
    if cursor.binding != *expected || cursor.revision_id.is_empty() {
        return Err(invalid_history_cursor());
    }
    Ok(cursor)
}

fn invalid_history_cursor() -> ArtifactError {
    ArtifactError::InvalidField {
        field: "cursor",
        reason: "context_mismatch",
    }
}

/// Publication health exposed to dispatch/read adapters.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum PublicationHealth {
    Ready {
        library_version: u64,
    },
    Degraded {
        committed_library_version: u64,
        published_library_version: u64,
    },
}

#[derive(Debug)]
struct PublicationVersions {
    library_version: u64,
    committed_library_version: u64,
}

/// Library-wide activation serializer and infallible generation publication cell.
pub(crate) struct ActivationCoordinator<G> {
    generation: Arc<ArcSwap<G>>,
    activation: Mutex<()>,
    versions: Mutex<PublicationVersions>,
}

impl<G> ActivationCoordinator<G> {
    pub(crate) fn new(generation: Arc<G>, library_version: u64) -> Self {
        Self::from_cell(Arc::new(ArcSwap::from(generation)), library_version)
    }

    pub(crate) fn from_cell(generation: Arc<ArcSwap<G>>, library_version: u64) -> Self {
        Self {
            generation,
            activation: Mutex::new(()),
            versions: Mutex::new(PublicationVersions {
                library_version,
                committed_library_version: library_version,
            }),
        }
    }

    pub(crate) fn generation(&self) -> Arc<G> {
        self.generation.load_full()
    }

    pub(crate) fn health(&self) -> PublicationHealth {
        let state = self
            .versions
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if state.library_version == state.committed_library_version {
            PublicationHealth::Ready {
                library_version: state.library_version,
            }
        } else {
            PublicationHealth::Degraded {
                committed_library_version: state.committed_library_version,
                published_library_version: state.library_version,
            }
        }
    }

    /// Serialize one activation-class transaction.
    ///
    /// `candidate` must be completely built and validated before this call. `commit` performs the
    /// durable CAS and returns its committed library version. Once it succeeds, publication is an
    /// allocation-free `Arc` move under this mutex and cannot fail.
    pub(crate) fn commit_and_publish<E>(
        &self,
        candidate: Arc<G>,
        commit: impl FnOnce() -> Result<u64, E>,
    ) -> Result<u64, E> {
        let _activation = self
            .activation
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let committed = commit()?;
        self.generation.store(candidate);
        let mut versions = self
            .versions
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        versions.committed_library_version = committed;
        versions.library_version = committed;
        Ok(committed)
    }

    fn commit_and_publish_outcome<E>(
        &self,
        candidate: Arc<G>,
        commit: impl FnOnce() -> Result<LibraryMutationOutcome, E>,
    ) -> Result<LibraryMutationOutcome, E> {
        let _activation = self
            .activation
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let outcome = commit()?;
        if let LibraryMutationOutcome::Committed(receipt, _) = &outcome {
            self.generation.store(candidate);
            let mut versions = self
                .versions
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            versions.committed_library_version = receipt.committed_version;
            versions.library_version = receipt.committed_version;
        }
        Ok(outcome)
    }

    fn commit_library_outcome(
        &self,
        candidate: Arc<G>,
        expected_library_version: u64,
        faults: &dyn FaultInjector,
        commit: impl FnOnce() -> Result<LibraryMutationOutcome, SkillLibraryDispatchError>,
    ) -> Result<LibraryMutationOutcome, SkillLibraryDispatchError> {
        let _activation = self
            .activation
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        {
            let versions = self
                .versions
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if versions.committed_library_version != versions.library_version
                && expected_library_version
                    .checked_add(1)
                    .is_none_or(|version| version != versions.committed_library_version)
            {
                return Err(ArtifactError::Conflict("publication_reconciliation_required").into());
            }
        }
        faults.check(FaultStage::BeforeCommit)?;
        let outcome = match commit() {
            Ok(outcome) => outcome,
            Err(SkillLibraryDispatchError::Artifact(ArtifactError::CommittedPending {
                committed_version,
            })) => {
                let mut versions = self
                    .versions
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                versions.committed_library_version = committed_version;
                return Err(ArtifactError::CommittedPending { committed_version }.into());
            }
            Err(error) => return Err(error),
        };
        if let LibraryMutationOutcome::Committed(receipt, _) = &outcome {
            if let Err(error) = faults.check(FaultStage::AfterCommitBeforeSwap) {
                let mut versions = self
                    .versions
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                versions.committed_library_version = receipt.committed_version;
                return Err(error.into());
            }
            self.generation.store(candidate);
            let mut versions = self
                .versions
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            versions.committed_library_version = receipt.committed_version;
            versions.library_version = receipt.committed_version;
        } else if let LibraryMutationOutcome::Replayed(receipt, _) = &outcome {
            let mut versions = self
                .versions
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            // A retry is also the reconciliation path for a response lost after durable commit but
            // before publication. Never let an older replay replace a newer published generation.
            if versions.committed_library_version == receipt.committed_version
                && versions.library_version < receipt.committed_version
            {
                self.generation.store(candidate);
                versions.library_version = receipt.committed_version;
            }
        }
        Ok(outcome)
    }

    /// Record a durable commit discovered after restart or an interrupted response boundary.
    /// Readers retain the last-good generation until reconciliation supplies the exact candidate.
    pub(crate) fn mark_committed(&self, committed_library_version: u64) {
        let mut state = self
            .versions
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        state.committed_library_version = committed_library_version;
    }

    /// Publish a candidate rebuilt from the durable library snapshot.
    pub(crate) fn reconcile(&self, candidate: Arc<G>, library_version: u64) -> bool {
        let _activation = self
            .activation
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let mut versions = self
            .versions
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if library_version != versions.committed_library_version {
            return false;
        }
        self.generation.store(candidate);
        versions.library_version = library_version;
        true
    }
}

#[derive(Debug, thiserror::Error)]
pub(crate) enum SkillLibraryDispatchError {
    #[error(transparent)]
    Authorization(#[from] SkillLibraryAuthorizationError),
    #[error(transparent)]
    Artifact(#[from] ArtifactError),
    #[error("invalid Skill Library parameters")]
    InvalidParams,
    #[error("unknown Skill Library action")]
    UnknownAction,
    #[error("Skill Library response serialization failed")]
    Serialization,
    #[error(transparent)]
    InjectedFault(#[from] InjectedFault),
    /// The caller stopped waiting for blocking work whose durable outcome is not yet known.
    /// Retrying the identical mutation with the same idempotency key reconciles a late commit.
    #[error("Skill Library blocking outcome is indeterminate for {operation}")]
    BlockingIndeterminate { operation: &'static str },
    #[error("Skill Library blocking work timed out for {operation}")]
    BlockingTimeout { operation: &'static str },
    #[error("Skill Library blocking worker failed for {operation}")]
    BlockingWorkerFailed { operation: &'static str },
}

/// Final production mutation boundary shared by every transport adapter.
///
/// Blocking capacity is acquired first. Authorization is then re-resolved from AccessRuntime and
/// the resulting sealed grant is moved directly into the synchronous Artifact commit closure.
/// This prevents queue wait from opening an authorize/revoke/mutate race.
#[allow(clippy::too_many_arguments)]
pub(crate) async fn commit_authorized_mutation<G: Send + Sync + 'static>(
    executor: &BoundedBlockingExecutor,
    coordinator: Arc<ActivationCoordinator<G>>,
    faults: Arc<dyn FaultInjector>,
    runtime: &AccessRuntime,
    caller: SkillLibraryCaller,
    project_id: &str,
    action: SkillLibraryAction,
    target_id: &CanonicalArtifactId,
    target: SkillLibraryTarget<'_>,
    correlation_id: &SkillLibraryCorrelationId,
    store: Arc<ArtifactStore>,
    expected_library_version: u64,
    mut idempotency: LibraryIdempotency,
    mutation: LibraryMutation,
    committed_at: LibraryTimestamp,
    prebuilt_candidate: Arc<G>,
) -> Result<LibraryMutationOutcome, BlockingError<SkillLibraryDispatchError>> {
    executor
        .run_after_admission("skill_library_commit", || async move {
            let decision = authorize_at_boundary(
                runtime,
                caller,
                project_id,
                action,
                target_id,
                target,
                correlation_id,
            )
            .await
            .map_err(SkillLibraryDispatchError::Authorization)?;
            idempotency.request_digest = bind_idempotency_to_owner(
                &idempotency.request_digest,
                &decision.ownership,
                project_id,
            )?;
            let authorization = decision.authorization;
            let ownership = decision.ownership;
            let audit = decision.audit;
            let revision_id = mutation_revision_id(&mutation).map(str::to_owned);
            let committed_version = expected_library_version
                .checked_add(1)
                .ok_or(ArtifactError::Conflict("library_version_exhausted"))?;
            let terminal = SkillLibraryTerminalAudit::new(
                SkillLibraryTerminalOutcome::Committed,
                SkillLibraryTerminalStage::Commit,
            )
            .with_versions(Some(committed_version), Some(committed_version));
            let terminal = revision_id
                .as_deref()
                .map_or(terminal, |revision| terminal.with_revision_id(revision));
            let mut idempotency = idempotency;
            idempotency.terminal_audit = Some(runtime_durable_audit(durable_terminal_audit(
                &audit, terminal,
            )?));
            Ok(move || {
                let result = coordinator.commit_library_outcome(
                    prebuilt_candidate,
                    expected_library_version,
                    faults.as_ref(),
                    || {
                        store
                            .mutate_library_outcome(
                                &authorization,
                                &ownership,
                                expected_library_version,
                                idempotency,
                                mutation,
                                committed_at,
                            )
                            .map_err(SkillLibraryDispatchError::Artifact)
                    },
                );
                record_terminal_result(&audit, revision_id.as_deref(), &result);
                result
            })
        })
        .await
}

fn mutation_revision_id(mutation: &LibraryMutation) -> Option<&str> {
    match mutation {
        LibraryMutation::Save { revision_id, .. }
        | LibraryMutation::Activate { revision_id, .. }
        | LibraryMutation::Rollback { revision_id, .. } => Some(revision_id),
        LibraryMutation::Create { .. }
        | LibraryMutation::SetVisibility { .. }
        | LibraryMutation::Deactivate { .. }
        | LibraryMutation::Archive { .. }
        | LibraryMutation::Refresh { .. } => None,
    }
}

fn record_terminal_result(
    audit: &SkillLibraryAuditEvent,
    revision_id: Option<&str>,
    result: &Result<LibraryMutationOutcome, SkillLibraryDispatchError>,
) {
    let terminal = match result {
        Ok(outcome) => {
            let receipt = outcome.receipt();
            SkillLibraryTerminalAudit::new(
                SkillLibraryTerminalOutcome::Committed,
                SkillLibraryTerminalStage::Commit,
            )
            .with_versions(
                Some(receipt.committed_version),
                Some(receipt.committed_version),
            )
            .replayed(outcome.is_replay())
        }
        Err(SkillLibraryDispatchError::InjectedFault(InjectedFault {
            stage: FaultStage::AfterCommitBeforeSwap,
        })) => SkillLibraryTerminalAudit::new(
            SkillLibraryTerminalOutcome::Failed,
            SkillLibraryTerminalStage::Publication,
        ),
        Err(_) => SkillLibraryTerminalAudit::new(
            SkillLibraryTerminalOutcome::Failed,
            SkillLibraryTerminalStage::Commit,
        ),
    };
    let terminal = revision_id.map_or(terminal, |revision| terminal.with_revision_id(revision));
    let _recorded = record_terminal_mutation(audit, terminal);
}

fn runtime_durable_audit(
    audit: super::audit::SkillLibraryDurableAudit,
) -> labby_runtime::artifacts::LibraryDurableAudit {
    labby_runtime::artifacts::LibraryDurableAudit {
        schema_version: audit.schema_version,
        correlation_id: audit.correlation_id,
        action: audit.action,
        target_digest: audit.target_digest,
        revision_digest: audit.revision_digest,
        tenant_id: audit.tenant_id,
        actor_id: audit.actor_id,
        surface: audit.surface,
        policy_revision: audit.policy_revision,
        committed_version: audit.committed_version,
        published_version: audit.published_version,
        outcome: audit.outcome,
        stage: audit.stage,
        replayed: audit.replayed,
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

    use labby_auth::{Authenticator, VerifiedIdentity};
    use serde_json::json;

    use super::*;
    use crate::access::{AccessStore, BootstrapOwnerInput};
    use crate::dispatch::skill_library::auth::SkillLibraryTransport;

    #[test]
    fn active_owned_artifacts_still_advertise_revision_activation() {
        let actions = item_allowed_actions(true, true);
        assert!(actions.contains(&"artifacts.activate"));
        assert!(actions.contains(&"artifacts.deactivate"));
    }

    #[test]
    fn list_cursor_is_opaque_and_bound_to_authority_query_and_generation() {
        let binding = ListCursorBinding {
            version: 1,
            context_digest: "a".repeat(64),
            library_version: 7,
            query_digest: Some("b".repeat(64)),
        };
        let encoded = encode_list_cursor(&binding, "artifact-1".to_owned()).unwrap();
        assert!(!encoded.contains("artifact-1"));
        assert_eq!(
            decode_list_cursor(&encoded, &binding).unwrap().artifact_id,
            "artifact-1"
        );

        let stale = ListCursorBinding {
            library_version: 8,
            ..binding
        };
        assert!(matches!(
            decode_list_cursor(&encoded, &stale),
            Err(ArtifactError::InvalidField {
                field: "cursor",
                reason: "context_mismatch"
            })
        ));
    }

    #[test]
    fn artifact_search_matches_only_snapshot_indexed_metadata() {
        let root = tempfile::tempdir().unwrap();
        let store = ArtifactStore::new(root.path().join("artifacts")).unwrap();
        let timestamp = LibraryTimestamp::parse("2026-01-01T00:00:00Z").unwrap();
        let record = SkillLibraryRecord {
            artifact_id: "artifact-fleet-health".to_owned(),
            name: "fleet-health".to_owned(),
            ownership: labby_runtime::artifacts::LibraryOwnership::canonical(
                labby_runtime::artifacts::LibraryTenantId::from_canonical_projection("tenant-1")
                    .unwrap(),
                labby_runtime::artifacts::LibraryActorId::from_canonical_projection("owner-1")
                    .unwrap(),
            ),
            visibility: SkillVisibility::Private,
            archived: false,
            active_revision_id: None,
            latest_revision_id: "revision-1".to_owned(),
            latest_revision_files: Vec::new(),
            search_metadata: vec![
                "Fleet Health".to_owned(),
                "Monitor storage fleet health".to_owned(),
                "monitoring".to_owned(),
            ],
            provenance_provider: Some("repository".to_owned()),
            materialized: true,
            created_at: timestamp.clone(),
            updated_at: timestamp,
        };

        assert!(artifact_matches(&store, &record, Some("fleet")));
        assert!(artifact_matches(&store, &record, Some("repository")));
        assert!(artifact_matches(&store, &record, Some("storage")));
        assert!(artifact_matches(&store, &record, Some("monitoring")));
        assert!(!artifact_matches(
            &store,
            &record,
            Some("descriptor-only-value")
        ));
    }

    #[test]
    fn history_window_is_page_bounded_and_preserves_newest_first_cursor_order() {
        let revisions = (0..10_000)
            .map(|index| format!("rev-{index:05}"))
            .collect::<Vec<_>>();
        let binding = HistoryCursorBinding {
            version: 1,
            context_digest: "authority-context".to_owned(),
            library_version: 7,
            artifact_id: "artifact-1".to_owned(),
            owner_kind: labby_runtime::artifacts::LibraryOwnerKind::Team,
            owner_id: "team-1".to_owned(),
        };

        let (start, end) = history_page_window(&revisions, None, 100, &binding).unwrap();
        assert_eq!((start, end), (9_900, 10_000));
        assert_eq!(&revisions[end - 1], "rev-09999");
        assert_eq!(&revisions[start], "rev-09900");

        let cursor = encode_history_cursor(&binding, start, &revisions[start]).unwrap();
        let (next_start, next_end) =
            history_page_window(&revisions, Some(&cursor), 100, &binding).unwrap();
        assert_eq!((next_start, next_end), (9_800, 9_900));
        assert_eq!(&revisions[next_end - 1], "rev-09899");

        assert!(matches!(
            history_page_window(&revisions, Some("h1:9900:rev-09899"), 100, &binding),
            Err(ArtifactError::InvalidField {
                field: "cursor",
                reason: "context_mismatch"
            })
        ));
        let mut other = binding;
        other.owner_id = "team-2".to_owned();
        assert!(history_page_window(&revisions, Some(&cursor), 100, &other).is_err());
        assert!(history_page_window(&revisions, Some("rev-09900"), 100, &other).is_err());
    }

    struct OneStageFault(FaultStage);

    impl FaultInjector for OneStageFault {
        fn check(&self, stage: FaultStage) -> Result<(), InjectedFault> {
            if stage == self.0 {
                Err(InjectedFault { stage })
            } else {
                Ok(())
            }
        }
    }

    struct OneShotStageFault {
        stage: FaultStage,
        armed: AtomicBool,
    }

    struct OneShotStageDelay {
        stage: FaultStage,
        armed: AtomicBool,
        delay: std::time::Duration,
    }

    async fn acceptance_dispatch(
        service: &SkillLibraryService<crate::skills::registry::FirstPartyGeneration>,
        runtime: &AccessRuntime,
        identity: &VerifiedIdentity,
        action: &'static str,
        params: Value,
        correlation: &'static str,
    ) -> Result<Value, SkillLibraryDispatchError> {
        let correlation = SkillLibraryCorrelationId::parse(correlation).unwrap();
        service
            .dispatch(
                runtime,
                SkillLibraryCaller::new(
                    identity.clone(),
                    [],
                    SkillLibraryTransport::browser(true, true),
                ),
                "bootstrap-default",
                action,
                params,
                &correlation,
            )
            .await
    }

    impl FaultInjector for OneShotStageFault {
        fn check(&self, stage: FaultStage) -> Result<(), InjectedFault> {
            if stage == self.stage && self.armed.swap(false, Ordering::SeqCst) {
                Err(InjectedFault { stage })
            } else {
                Ok(())
            }
        }
    }

    impl FaultInjector for OneShotStageDelay {
        fn check(&self, stage: FaultStage) -> Result<(), InjectedFault> {
            if stage == self.stage && self.armed.swap(false, Ordering::SeqCst) {
                std::thread::sleep(self.delay);
            }
            Ok(())
        }
    }

    #[tokio::test]
    async fn commit_timeout_is_indeterminate_and_same_key_reconciles_late_commit() {
        let root = tempfile::tempdir().unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            std::fs::set_permissions(root.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
        }
        let access_path = root.path().join("access.db");
        let access_store = AccessStore::open(access_path.clone()).await.unwrap();
        let identity = VerifiedIdentity::external(
            Authenticator::BrowserSession,
            "https://accounts.google.com",
            "timeout-owner",
        )
        .unwrap();
        access_store
            .bootstrap_owner(
                BootstrapOwnerInput::new(identity.clone(), "Local", "Default").unwrap(),
            )
            .await
            .unwrap();
        drop(access_store);
        let runtime = AccessRuntime::initialize(access_path).await;
        let store = Arc::new(ArtifactStore::new(root.path().join("artifacts")).unwrap());
        let projection: Arc<
            dyn GenerationProjection<crate::skills::registry::FirstPartyGeneration>,
        > = Arc::new(ArtifactFirstPartyProjection);
        let initial = projection
            .prepare(&store, &store.library_snapshot().unwrap(), None)
            .unwrap();
        let publication = Arc::new(ActivationCoordinator::new(initial, 0));
        let blocking = BoundedBlockingExecutor::new(
            1,
            std::time::Duration::from_secs(1),
            std::time::Duration::from_millis(40),
        )
        .unwrap();
        let service = SkillLibraryService::new(
            Arc::clone(&store),
            blocking,
            Arc::clone(&publication),
            Arc::clone(&projection),
        )
        .with_fault_injector(Arc::new(OneShotStageDelay {
            stage: FaultStage::AfterCommitBeforeSwap,
            armed: AtomicBool::new(true),
            delay: std::time::Duration::from_millis(150),
        }));
        let params = json!({
            "name": "timeout-recovery",
            "files": [{"path":"SKILL.md", "content":"---\nname: timeout-recovery\ndescription: timeout recovery\n---\nbody\n"}],
            "expected_library_version": 0,
            "idempotency_key": "create-timeout-recovery"
        });

        let first = acceptance_dispatch(
            &service,
            &runtime,
            &identity,
            "artifacts.create",
            params.clone(),
            "timeout-recovery-first",
        )
        .await;
        assert!(matches!(
            first,
            Err(SkillLibraryDispatchError::BlockingIndeterminate {
                operation: "skill_artifact_commit"
            })
        ));

        // Loaded Windows CI runners can take longer to schedule the blocking
        // reconciliation task after the deliberately injected timeout.
        let reconciliation_timeout = if cfg!(windows) {
            std::time::Duration::from_secs(5)
        } else {
            std::time::Duration::from_secs(1)
        };
        tokio::time::timeout(reconciliation_timeout, async {
            loop {
                if store.library_snapshot().unwrap().version == 1
                    && publication.health() == (PublicationHealth::Ready { library_version: 1 })
                {
                    break;
                }
                tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap();
        assert_eq!(
            publication.health(),
            PublicationHealth::Ready { library_version: 1 }
        );

        let replay = acceptance_dispatch(
            &service,
            &runtime,
            &identity,
            "artifacts.create",
            params,
            "timeout-recovery-replay",
        )
        .await
        .unwrap();
        assert_eq!(replay["outcome"], "replayed");
        assert_eq!(replay["committed_library_version"], 1);
    }

    #[tokio::test]
    async fn lost_response_replay_keeps_original_receipt_and_newer_generation() {
        let root = tempfile::tempdir().unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            std::fs::set_permissions(root.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
        }
        let access_path = root.path().join("access.db");
        let access_store = AccessStore::open(access_path.clone()).await.unwrap();
        let identity = VerifiedIdentity::external(
            Authenticator::BrowserSession,
            "https://accounts.google.com",
            "owner-subject",
        )
        .unwrap();
        access_store
            .bootstrap_owner(
                BootstrapOwnerInput::new(identity.clone(), "Local", "Default").unwrap(),
            )
            .await
            .unwrap();
        drop(access_store);
        let runtime = AccessRuntime::initialize(access_path).await;
        let caller = || {
            SkillLibraryCaller::new(
                identity.clone(),
                [],
                SkillLibraryTransport::browser(true, true),
            )
        };

        let store = Arc::new(ArtifactStore::new(root.path().join("artifacts")).unwrap());
        let projection: Arc<
            dyn GenerationProjection<crate::skills::registry::FirstPartyGeneration>,
        > = Arc::new(ArtifactFirstPartyProjection);
        let initial = projection
            .prepare(&store, &store.library_snapshot().unwrap(), None)
            .unwrap();
        let publication = Arc::new(ActivationCoordinator::new(initial, 0));
        let blocking = BoundedBlockingExecutor::new(
            2,
            std::time::Duration::from_secs(1),
            std::time::Duration::from_secs(10),
        )
        .unwrap();
        let service = SkillLibraryService::new(
            Arc::clone(&store),
            blocking,
            Arc::clone(&publication),
            Arc::clone(&projection),
        )
        .with_fault_injector(Arc::new(OneShotStageFault {
            stage: FaultStage::AfterSwapBeforeResponse,
            armed: AtomicBool::new(true),
        }));
        let first_params = json!({
            "name": "lost-response",
            "files": [{"path":"SKILL.md", "content":"---\nname: lost-response\ndescription: first\n---\nfirst\n"}],
            "expected_library_version": 0,
            "idempotency_key": "create-lost-response"
        });
        let first = service
            .dispatch(
                &runtime,
                caller(),
                "bootstrap-default",
                "artifacts.create",
                first_params.clone(),
                &SkillLibraryCorrelationId::parse("lost-response-1").unwrap(),
            )
            .await;
        assert!(matches!(
            first,
            Err(SkillLibraryDispatchError::InjectedFault(InjectedFault {
                stage: FaultStage::AfterSwapBeforeResponse
            }))
        ));
        assert_eq!(
            publication.health(),
            PublicationHealth::Ready { library_version: 1 }
        );

        let reopened = ArtifactStore::new(root.path().join("artifacts")).unwrap();
        let committed = reopened.library_snapshot().unwrap();
        let first_receipt = committed.receipts.values().next().unwrap().clone();
        let original_facts = first_receipt.response_facts.clone().unwrap();
        let terminal = first_receipt.terminal_audit.as_ref().unwrap();
        assert_eq!(
            (terminal.outcome.as_str(), terminal.stage.as_str()),
            ("failed", "response")
        );

        service
            .dispatch(
                &runtime,
                caller(),
                "bootstrap-default",
                "artifacts.create",
                json!({
                    "name": "newer-generation",
                    "files": [{"path":"SKILL.md", "content":"---\nname: newer-generation\ndescription: second\n---\nsecond\n"}],
                    "expected_library_version": 1,
                    "idempotency_key": "create-newer-generation"
                }),
                &SkillLibraryCorrelationId::parse("newer-generation-2").unwrap(),
            )
            .await
            .unwrap();
        let newer_generation = publication.generation();
        assert_eq!(
            publication.health(),
            PublicationHealth::Ready { library_version: 2 }
        );

        let replay = service
            .dispatch(
                &runtime,
                caller(),
                "bootstrap-default",
                "artifacts.create",
                first_params,
                &SkillLibraryCorrelationId::parse("lost-response-1").unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(replay["outcome"], "replayed");
        assert_eq!(
            replay["committed_library_version"],
            original_facts.committed_library_version
        );
        assert_eq!(replay["old_generation"], original_facts.old_generation);
        assert_eq!(replay["new_generation"], original_facts.new_generation);
        assert_eq!(replay["library_digest"], original_facts.library_digest);
        assert_eq!(
            replay["active_revision_id"],
            serde_json::to_value(&original_facts.active_revision_id).unwrap()
        );
        assert_eq!(
            replay["canonical_uri"],
            serde_json::to_value(&original_facts.canonical_uri).unwrap()
        );
        assert_eq!(replay["published_library_version"], 2);
        assert!(Arc::ptr_eq(&publication.generation(), &newer_generation));

        let after_replay = ArtifactStore::new(root.path().join("artifacts"))
            .unwrap()
            .library_snapshot()
            .unwrap();
        assert_eq!(after_replay.version, 2);
        let record = after_replay
            .records
            .values()
            .find(|record| record.name == "lost-response")
            .unwrap();
        let artifact_id = record.artifact_id.clone();
        let revision_id = store
            .get(&artifact_id)
            .unwrap()
            .revision_ids
            .last()
            .unwrap()
            .clone();
        let list = service
            .dispatch(
                &runtime,
                caller(),
                "bootstrap-default",
                "artifacts.list",
                json!({}),
                &SkillLibraryCorrelationId::parse("versioned-list").unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(list["library_version"], 2);
        let summary = list["items"]
            .as_array()
            .unwrap()
            .iter()
            .find(|item| item["artifact_id"] == artifact_id)
            .unwrap();
        assert_eq!(summary["latest_revision_id"], revision_id);
        assert_eq!(summary["visibility"], "private");
        assert_eq!(summary["access_label"], "personal");
        assert_eq!(summary["can_mutate"], true);
        assert_eq!(summary["owner"]["relationship"], "self");
        assert_eq!(summary["provenance"]["source"], "local");
        assert_eq!(summary["materialized"], true);
        assert_eq!(summary["canonical_uri"], Value::Null);
        assert_eq!(summary["current_generation"], 2);
        assert_eq!(summary["published_library_version"], 2);
        assert_eq!(summary["latest_revision_files"][0]["path"], "SKILL.md");
        assert!(summary["latest_revision_files"][0].get("content").is_none());
        assert_eq!(list["can_create"], true);
        assert_eq!(list["create_visibilities"], json!(["private", "shared"]));
        assert_eq!(list["published_library_version"], 2);
        assert!(
            list["allowed_actions"]
                .as_array()
                .unwrap()
                .iter()
                .any(|action| action == "artifacts.create")
        );
        assert!(
            summary["allowed_actions"]
                .as_array()
                .unwrap()
                .iter()
                .any(|action| action == "artifacts.activate")
        );
        assert!(summary.get("ownership").is_none());

        let invalid = service
            .dispatch(
                &runtime,
                caller(),
                "bootstrap-default",
                "artifacts.validate",
                json!({
                    "name": "bad",
                    "files": [{"path":"../secret", "content":"nope"}]
                }),
                &SkillLibraryCorrelationId::parse("structured-validation").unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(invalid["valid"], false);
        assert_eq!(invalid["revision_id"], Value::Null);
        assert_eq!(invalid["rejections"][0]["field"], "files");
        assert!(invalid["rejections"][0].get("code").is_some());

        for (action, params) in [
            ("artifacts.get", json!({"artifact_id": artifact_id})),
            ("artifacts.history", json!({"artifact_id": artifact_id})),
            (
                "artifacts.read",
                json!({
                    "artifact_id": artifact_id,
                    "revision_id": revision_id,
                    "path": "SKILL.md"
                }),
            ),
        ] {
            let response = service
                .dispatch(
                    &runtime,
                    caller(),
                    "bootstrap-default",
                    action,
                    params,
                    &SkillLibraryCorrelationId::parse(format!("versioned-{action}")).unwrap(),
                )
                .await
                .unwrap();
            assert_eq!(response["library_version"], 2, "{action}");
        }
        service
            .dispatch(
                &runtime,
                caller(),
                "bootstrap-default",
                "artifacts.archive",
                json!({
                    "artifact_id": artifact_id,
                    "expected_library_version": 2,
                    "idempotency_key": "archive-owner-read-regression"
                }),
                &SkillLibraryCorrelationId::parse("archive-owner-read").unwrap(),
            )
            .await
            .unwrap();
        for (action, params) in [
            ("artifacts.get", json!({"artifact_id": artifact_id})),
            ("artifacts.history", json!({"artifact_id": artifact_id})),
            (
                "artifacts.read",
                json!({
                    "artifact_id": artifact_id,
                    "revision_id": revision_id,
                    "path": "SKILL.md"
                }),
            ),
        ] {
            let response = service
                .dispatch(
                    &runtime,
                    caller(),
                    "bootstrap-default",
                    action,
                    params,
                    &SkillLibraryCorrelationId::parse(format!("archived-{action}")).unwrap(),
                )
                .await
                .unwrap();
            assert_eq!(response["library_version"], 3, "{action}");
        }
        let durable = after_replay
            .receipts
            .get(&first_receipt.scope_digest)
            .unwrap()
            .terminal_audit
            .as_ref()
            .unwrap();
        assert_eq!(
            (durable.outcome.as_str(), durable.stage.as_str()),
            ("failed", "response")
        );
        assert_eq!(
            after_replay
                .audit_intents
                .iter()
                .filter(|audit| audit.sequence == first_receipt.sequence)
                .count(),
            1
        );
    }

    #[tokio::test]
    async fn three_principals_share_activate_restart_replay_and_revoke_exact_skills() {
        let root = tempfile::tempdir().unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            std::fs::set_permissions(root.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
        }
        let access_path = root.path().join("access.db");
        let access_store = AccessStore::open(access_path.clone()).await.unwrap();
        let identity = |subject| {
            VerifiedIdentity::external(
                Authenticator::BrowserSession,
                "https://accounts.google.com",
                subject,
            )
            .unwrap()
        };
        let eli = identity("eli");
        let pujit = identity("pujit");
        let jake = identity("jake");
        access_store
            .bootstrap_owner(BootstrapOwnerInput::new(eli.clone(), "Local", "Default").unwrap())
            .await
            .unwrap();
        access_store
            .execute_test_statement(
                "INSERT INTO principals VALUES
                   ('pujit-principal','bootstrap-local','user','active','Pujit',2,2),
                   ('jake-principal','bootstrap-local','user','active','Jake',2,2);
                 INSERT INTO principal_links VALUES
                   ('pujit-link','pujit-principal','external','https://accounts.google.com','pujit',NULL,'active',1,1,2,2),
                   ('jake-link','jake-principal','external','https://accounts.google.com','jake',NULL,'active',1,1,2,2);
                 INSERT INTO project_memberships VALUES
                   ('pujit-membership','bootstrap-local','bootstrap-default','pujit-principal','member','active','bootstrap-owner',2,2),
                   ('jake-membership','bootstrap-local','bootstrap-default','jake-principal','admin','active','bootstrap-owner',2,2);",
            )
            .await
            .unwrap();
        drop(access_store);
        let runtime = AccessRuntime::initialize(access_path.clone()).await;
        let artifacts_path = root.path().join("artifacts");
        let store = Arc::new(ArtifactStore::new(artifacts_path.clone()).unwrap());
        let projection: Arc<
            dyn GenerationProjection<crate::skills::registry::FirstPartyGeneration>,
        > = Arc::new(ArtifactFirstPartyProjection);
        let initial = projection
            .prepare(&store, &store.library_snapshot().unwrap(), None)
            .unwrap();
        let publication = Arc::new(ActivationCoordinator::new(initial, 0));
        let service = SkillLibraryService::new(
            Arc::clone(&store),
            BoundedBlockingExecutor::new(
                2,
                std::time::Duration::from_secs(1),
                std::time::Duration::from_secs(10),
            )
            .unwrap(),
            Arc::clone(&publication),
            Arc::clone(&projection),
        );
        let shared_content =
            "---\nname: shared-brief\ndescription: Shared acceptance skill\n---\nExact bytes.\n";
        let created = acceptance_dispatch(
            &service,
            &runtime,
            &eli,
            "artifacts.create",
            json!({
                "name": "shared-brief",
                "visibility": "shared",
                "files": [
                    {"path": "SKILL.md", "content": shared_content},
                    {"path": "references/check.md", "content": "support bytes\n"}
                ],
                "expected_library_version": 0,
                "idempotency_key": "eli-create-shared"
            }),
            "eli-create-shared",
        )
        .await
        .unwrap();
        let artifact_id = created["artifact_id"].as_str().unwrap().to_owned();
        let revision_id = store
            .get(&artifact_id)
            .unwrap()
            .revision_ids
            .last()
            .unwrap()
            .clone();
        assert_eq!(created["active_revision_id"], Value::Null);
        assert_eq!(created["canonical_uri"], Value::Null);
        assert!(
            publication
                .generation()
                .providers
                .find("skill://labby/shared-brief/SKILL.md")
                .is_none(),
            "saving must not activate"
        );

        let activated = acceptance_dispatch(
            &service,
            &runtime,
            &eli,
            "artifacts.activate",
            json!({
                "artifact_id": artifact_id,
                "expected_revision_id": revision_id,
                "expected_library_version": 1,
                "idempotency_key": "eli-activate-shared"
            }),
            "eli-activate-shared",
        )
        .await
        .unwrap();
        assert_eq!(activated["new_generation"], 2);
        assert_eq!(activated["committed_library_version"], 2);
        assert_eq!(activated["published_library_version"], 2);
        assert_eq!(
            activated["canonical_uri"],
            "skill://labby/shared-brief/SKILL.md"
        );
        let generation = publication.generation();
        assert_eq!(generation.id, 2);
        assert!(
            activated["library_digest"]
                .as_str()
                .unwrap()
                .starts_with("sha256:")
        );
        assert!(generation.digest.starts_with("sha256:"));
        let entry = generation
            .providers
            .find("skill://labby/shared-brief/SKILL.md")
            .unwrap();
        assert_eq!(
            generation
                .providers
                .read(entry, "skill://labby/shared-brief/SKILL.md", 64 * 1024)
                .await
                .unwrap()
                .bytes,
            shared_content.as_bytes()
        );
        assert_eq!(
            generation
                .providers
                .read(
                    entry,
                    "skill://labby/shared-brief/references/check.md",
                    64 * 1024,
                )
                .await
                .unwrap()
                .bytes,
            b"support bytes\n"
        );

        let member_list = acceptance_dispatch(
            &service,
            &runtime,
            &pujit,
            "artifacts.list",
            json!({}),
            "pujit-list-shared",
        )
        .await
        .unwrap();
        let shared = member_list["items"]
            .as_array()
            .unwrap()
            .iter()
            .find(|item| item["artifact_id"] == artifact_id)
            .unwrap();
        assert_eq!(shared["active_revision_id"], revision_id);
        assert_eq!(shared["canonical_uri"], activated["canonical_uri"]);
        assert_eq!(shared["published_library_version"], 2);
        let read = acceptance_dispatch(
            &service,
            &runtime,
            &pujit,
            "artifacts.read",
            json!({"artifact_id": artifact_id, "revision_id": revision_id, "path": "SKILL.md"}),
            "pujit-read-exact",
        )
        .await
        .unwrap();
        assert_eq!(read["text"], shared_content);
        assert_eq!(read["library_version"], 2);

        let personal = acceptance_dispatch(&service, &runtime,
            &pujit,
            "artifacts.create",
            json!({
                "name": "pujit-private",
                "files": [{"path": "SKILL.md", "content": "---\nname: pujit-private\ndescription: private\n---\nprivate\n"}],
                "expected_library_version": 2,
                "idempotency_key": "pujit-create-private"
            }),
            "pujit-create-private",
        )
        .await
        .unwrap();
        let personal_id = personal["artifact_id"].as_str().unwrap().to_owned();
        let owner_private = acceptance_dispatch(
            &service,
            &runtime,
            &eli,
            "artifacts.create",
            json!({
                "name": "eli-private",
                "files": [{"path": "SKILL.md", "content": "---\nname: eli-private\ndescription: private\n---\nprivate\n"}],
                "expected_library_version": 3,
                "idempotency_key": "eli-create-private"
            }),
            "eli-create-private",
        )
        .await
        .unwrap();
        let owner_private_id = owner_private["artifact_id"].as_str().unwrap().to_owned();
        let owner_private_revision = store
            .get(&owner_private_id)
            .unwrap()
            .revision_ids
            .last()
            .unwrap()
            .clone();
        let random_id = "missing-artifact";
        let before_denials = store.library_snapshot().unwrap();
        let denial_cases = [
            (
                "artifacts.get",
                json!({"artifact_id": owner_private_id}),
                json!({"artifact_id": random_id}),
            ),
            (
                "artifacts.read",
                json!({"artifact_id": owner_private_id, "revision_id": owner_private_revision, "path": "SKILL.md"}),
                json!({"artifact_id": random_id, "revision_id": owner_private_revision, "path": "SKILL.md"}),
            ),
            (
                "artifacts.history",
                json!({"artifact_id": owner_private_id}),
                json!({"artifact_id": random_id}),
            ),
            (
                "artifacts.save",
                json!({
                    "artifact_id": owner_private_id,
                    "expected_revision_id": owner_private_revision,
                    "files": [{"path": "SKILL.md", "content": "---\nname: eli-private\ndescription: private\n---\nprivate\n"}],
                    "expected_library_version": before_denials.version,
                    "idempotency_key": "inaccessible-save"
                }),
                json!({
                    "artifact_id": random_id,
                    "expected_revision_id": owner_private_revision,
                    "files": [{"path": "SKILL.md", "content": "---\nname: eli-private\ndescription: private\n---\nprivate\n"}],
                    "expected_library_version": before_denials.version,
                    "idempotency_key": "random-save"
                }),
            ),
            (
                "artifacts.activate",
                json!({"artifact_id": owner_private_id, "expected_revision_id": owner_private_revision, "expected_library_version": before_denials.version, "idempotency_key": "inaccessible-activate"}),
                json!({"artifact_id": random_id, "expected_revision_id": owner_private_revision, "expected_library_version": before_denials.version, "idempotency_key": "random-activate"}),
            ),
            (
                "artifacts.rollback",
                json!({"artifact_id": owner_private_id, "expected_revision_id": owner_private_revision, "expected_library_version": before_denials.version, "idempotency_key": "inaccessible-rollback"}),
                json!({"artifact_id": random_id, "expected_revision_id": owner_private_revision, "expected_library_version": before_denials.version, "idempotency_key": "random-rollback"}),
            ),
            (
                "artifacts.deactivate",
                json!({"artifact_id": owner_private_id, "expected_library_version": before_denials.version, "idempotency_key": "inaccessible-deactivate"}),
                json!({"artifact_id": random_id, "expected_library_version": before_denials.version, "idempotency_key": "random-deactivate"}),
            ),
            (
                "artifacts.archive",
                json!({"artifact_id": owner_private_id, "expected_library_version": before_denials.version, "idempotency_key": "inaccessible-archive"}),
                json!({"artifact_id": random_id, "expected_library_version": before_denials.version, "idempotency_key": "random-archive"}),
            ),
        ];
        for (action, inaccessible_params, random_params) in denial_cases {
            let inaccessible = acceptance_dispatch(
                &service,
                &runtime,
                &pujit,
                action,
                inaccessible_params,
                "existing-inaccessible-denial",
            )
            .await
            .unwrap_err();
            let random = acceptance_dispatch(
                &service,
                &runtime,
                &pujit,
                action,
                random_params,
                "random-target-denial",
            )
            .await
            .unwrap_err();
            assert!(
                matches!(
                    &inaccessible,
                    SkillLibraryDispatchError::Authorization(
                        SkillLibraryAuthorizationError::Denied
                    )
                ),
                "{action}: {inaccessible:?}"
            );
            assert!(matches!(
                random,
                SkillLibraryDispatchError::Authorization(SkillLibraryAuthorizationError::Denied)
            ));
            let inaccessible_public = serde_json::to_value(
                crate::dispatch::skill_library::map_dispatch_error(inaccessible),
            )
            .unwrap();
            let random_public =
                serde_json::to_value(crate::dispatch::skill_library::map_dispatch_error(random))
                    .unwrap();
            assert_eq!(inaccessible_public, random_public, "{action}");
        }
        let after_denials = store.library_snapshot().unwrap();
        assert_eq!(after_denials.version, before_denials.version);
        assert_eq!(after_denials.records.len(), before_denials.records.len());
        assert_eq!(after_denials.receipts.len(), before_denials.receipts.len());
        assert!(
            acceptance_dispatch(
                &service,
                &runtime,
                &pujit,
                "artifacts.get",
                json!({"artifact_id": owner_private_id}),
                "pujit-private-denied"
            )
            .await
            .is_err()
        );
        let admin_view = acceptance_dispatch(
            &service,
            &runtime,
            &jake,
            "artifacts.get",
            json!({"artifact_id": personal_id}),
            "jake-private-admin",
        )
        .await;
        assert!(admin_view.is_err());

        let replay = acceptance_dispatch(
            &service,
            &runtime,
            &eli,
            "artifacts.create",
            json!({
                "name": "shared-brief",
                "visibility": "shared",
                "files": [
                    {"path": "SKILL.md", "content": shared_content},
                    {"path": "references/check.md", "content": "support bytes\n"}
                ],
                "expected_library_version": 0,
                "idempotency_key": "eli-create-shared"
            }),
            "eli-create-shared-replay",
        )
        .await
        .unwrap();
        assert_eq!(replay["outcome"], "replayed");
        assert_eq!(replay["committed_library_version"], 1);
        assert!(
            acceptance_dispatch(
                &service,
                &runtime,
                &eli,
                "artifacts.create",
                json!({
                    "name": "shared-brief",
                    "visibility": "shared",
                    "files": [{"path": "SKILL.md", "content": "changed"}],
                    "expected_library_version": 0,
                    "idempotency_key": "eli-create-shared"
                }),
                "eli-idempotency-collision"
            )
            .await
            .is_err()
        );

        let restarted_store = Arc::new(ArtifactStore::new(artifacts_path).unwrap());
        let restarted_snapshot = restarted_store.library_snapshot().unwrap();
        let restarted_generation = projection
            .prepare(&restarted_store, &restarted_snapshot, None)
            .unwrap();
        assert_eq!(restarted_snapshot.version, 4);
        assert_eq!(restarted_generation.id, 4);
        let restarted_entry = restarted_generation
            .providers
            .find("skill://labby/shared-brief/SKILL.md")
            .unwrap();
        assert_eq!(
            restarted_generation
                .providers
                .read(
                    restarted_entry,
                    "skill://labby/shared-brief/SKILL.md",
                    64 * 1024,
                )
                .await
                .unwrap()
                .bytes,
            shared_content.as_bytes()
        );

        let access_store = runtime.store().await.unwrap();
        access_store
            .execute_test_statement(
                "UPDATE project_memberships SET status='disabled', updated_at=3
                 WHERE membership_id='pujit-membership';
                 UPDATE projects SET project_policy_epoch=project_policy_epoch+1, updated_at=3
                 WHERE project_id='bootstrap-default';",
            )
            .await
            .unwrap();
        assert!(
            service
                .dispatch(
                    &runtime,
                    SkillLibraryCaller::new(
                        pujit.clone(),
                        [],
                        SkillLibraryTransport::browser(true, true),
                    ),
                    "bootstrap-default",
                    "artifacts.list",
                    json!({}),
                    &SkillLibraryCorrelationId::parse("pujit-revoked").unwrap(),
                )
                .await
                .is_err()
        );
    }

    #[test]
    fn failed_commit_keeps_last_good_generation() {
        let coordinator = ActivationCoordinator::new(Arc::new("old"), 1);
        let result = coordinator.commit_and_publish(Arc::new("new"), || Err::<u64, _>("cas"));
        assert_eq!(result, Err("cas"));
        assert_eq!(*coordinator.generation(), "old");
        assert_eq!(
            coordinator.health(),
            PublicationHealth::Ready { library_version: 1 }
        );
    }

    #[test]
    fn durable_commit_precedes_one_exact_infallible_arc_publication() {
        let coordinator = ActivationCoordinator::new(Arc::new("old"), 1);
        let commits = AtomicUsize::new(0);
        let candidate = Arc::new("exact-candidate");
        let retained = Arc::clone(&candidate);
        let committed = coordinator
            .commit_and_publish(candidate, || {
                commits.fetch_add(1, Ordering::SeqCst);
                Ok::<_, ()>(2)
            })
            .unwrap();
        assert_eq!(committed, 2);
        assert_eq!(commits.load(Ordering::SeqCst), 1);
        assert!(Arc::ptr_eq(&coordinator.generation(), &retained));
        assert_eq!(
            coordinator.health(),
            PublicationHealth::Ready { library_version: 2 }
        );
    }

    #[test]
    fn restart_gap_is_degraded_until_exact_version_reconciles() {
        let coordinator = ActivationCoordinator::new(Arc::new("generation-one"), 1);
        coordinator.mark_committed(2);
        assert_eq!(
            coordinator.health(),
            PublicationHealth::Degraded {
                committed_library_version: 2,
                published_library_version: 1,
            }
        );
        assert!(!coordinator.reconcile(Arc::new("stale"), 1));
        assert_eq!(*coordinator.generation(), "generation-one");
        assert!(coordinator.reconcile(Arc::new("generation-two"), 2));
        assert_eq!(*coordinator.generation(), "generation-two");
        assert_eq!(
            coordinator.health(),
            PublicationHealth::Ready { library_version: 2 }
        );
    }
}
