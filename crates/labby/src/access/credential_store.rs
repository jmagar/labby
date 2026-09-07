//! Transactional persistence primitives for project-bound credentials.
//!
//! Callers must perform transport authentication and published-Loadout admission
//! before entering these methods. Every mutating method rechecks persisted
//! ownership and generations in one `IMMEDIATE` transaction.

use rusqlite::{OptionalExtension, Transaction, TransactionBehavior, params};
use subtle::ConstantTimeEq as _;

use super::bootstrap::{
    AUDIT_ID, LINK_ID, MEMBERSHIP_ID, ORGANIZATION_ID, PRINCIPAL_ID, PROJECT_ID,
};
use super::error::{AccessStoreError, AccessStoreResult};
use super::store::{AccessStore, map_sqlite_error};

const MAX_ID: usize = 160;
const MAX_NAME: usize = 128;
const MAX_URI: usize = 2048;
const MAX_SCOPES_JSON: usize = 4096;

#[derive(Clone)]
pub(crate) struct ActivateProofInput {
    pub(crate) proof_id: String,
    pub(crate) prepare_id: String,
    pub(crate) installation_id: String,
    pub(crate) installation_generation: i64,
    pub(crate) proof_digest: [u8; 32],
    pub(crate) manifest_digest: [u8; 32],
    pub(crate) request_digest: [u8; 32],
    pub(crate) idempotency_digest: [u8; 32],
    pub(crate) credential_id: String,
    pub(crate) credential_digest: [u8; 32],
    pub(crate) proof_generation: i64,
    pub(crate) created_at: i64,
    pub(crate) expires_at: i64,
}

#[derive(Clone)]
pub(crate) struct ConsumeBootstrapInput {
    pub(crate) proof_id: String,
    pub(crate) proof_digest: [u8; 32],
    pub(crate) request_digest: [u8; 32],
    pub(crate) idempotency_digest: [u8; 32],
    pub(crate) organization_name: String,
    pub(crate) project_name: String,
    pub(crate) canonical_issuer: String,
    pub(crate) subject: String,
    pub(crate) identity_fingerprint: String,
    pub(crate) loadout_id: String,
    pub(crate) loadout_generation: i64,
    pub(crate) catalog_generation: i64,
    pub(crate) loadout_policy_fingerprint: [u8; 32],
    pub(crate) route_id: String,
    pub(crate) route_generation: i64,
    pub(crate) resource: String,
    pub(crate) audience: String,
    pub(crate) scopes_json: String,
    pub(crate) now: i64,
    pub(crate) credential_expires_at: i64,
}

#[derive(Clone)]
pub(crate) struct IssueCredentialInput {
    pub(crate) actor_credential_id: String,
    pub(crate) actor_credential_generation: i64,
    pub(crate) credential_id: String,
    pub(crate) credential_digest: [u8; 32],
    pub(crate) credential_generation: i64,
    pub(crate) scopes_json: String,
    pub(crate) issued_at: i64,
    pub(crate) expires_at: i64,
    pub(crate) idempotency_digest: [u8; 32],
    pub(crate) request_digest: [u8; 32],
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct CredentialSnapshot {
    pub(crate) credential_id: String,
    pub(crate) installation_id: String,
    pub(crate) canonical_issuer: String,
    pub(crate) subject: String,
    pub(crate) organization_id: String,
    pub(crate) principal_id: String,
    pub(crate) project_id: String,
    pub(crate) loadout_id: String,
    pub(crate) route_id: String,
    pub(crate) resource: String,
    pub(crate) audience: String,
    pub(crate) scopes_json: String,
    pub(crate) credential_generation: i64,
    pub(crate) membership_generation: i64,
    pub(crate) organization_policy_epoch: i64,
    pub(crate) project_policy_epoch: i64,
    pub(crate) loadout_assignment_generation: i64,
    pub(crate) expires_at: i64,
    pub(crate) revocation_generation: i64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum MutationOutcome {
    Created,
    AlreadyApplied,
}

impl AccessStore {
    pub(crate) async fn admit_security_operation(
        &self,
        admission_class: String,
        bucket_fingerprint: [u8; 32],
        now: i64,
        window_seconds: i64,
        limit: i64,
    ) -> AccessStoreResult<bool> {
        if !matches!(
            admission_class.as_str(),
            "proof_global" | "proof_peer" | "credential_global" | "credential_peer"
        ) || window_seconds <= 0
            || limit <= 0
            || limit > 64
        {
            return Err(AccessStoreError::InvalidBootstrapInput);
        }
        self.with_connection(move |connection| {
            let transaction=connection.transaction_with_behavior(TransactionBehavior::Immediate).map_err(map_sqlite_error)?;
            transaction.execute("DELETE FROM access_admission_buckets WHERE updated_at < ?1",[now.saturating_sub(window_seconds.saturating_mul(2))]).map_err(map_sqlite_error)?;
            let current=transaction.query_row("SELECT window_started_at,attempts FROM access_admission_buckets WHERE admission_class=?1 AND bucket_fingerprint=?2",params![admission_class,bucket_fingerprint.as_slice()],|row|Ok((row.get::<_,i64>(0)?,row.get::<_,i64>(1)?))).optional().map_err(map_sqlite_error)?;
            let admitted=match current {
                None => { transaction.execute("INSERT INTO access_admission_buckets VALUES(?1,?2,?3,1,?3)",params![admission_class,bucket_fingerprint.as_slice(),now]).map_err(map_sqlite_error)?; true }
                Some((started,_)) if now.saturating_sub(started)>=window_seconds => { transaction.execute("UPDATE access_admission_buckets SET window_started_at=?1,attempts=1,updated_at=?1 WHERE admission_class=?2 AND bucket_fingerprint=?3",params![now,admission_class,bucket_fingerprint.as_slice()]).map_err(map_sqlite_error)?; true }
                Some((_started,attempts)) if attempts < limit => { transaction.execute("UPDATE access_admission_buckets SET attempts=attempts+1,updated_at=?1 WHERE admission_class=?2 AND bucket_fingerprint=?3",params![now,admission_class,bucket_fingerprint.as_slice()]).map_err(map_sqlite_error)?; true }
                Some(_) => false,
            };
            transaction.commit().map_err(map_sqlite_error)?;
            Ok(admitted)
        }).await
    }

    pub(crate) async fn record_security_event(
        &self,
        event_kind: String,
        decision: String,
        reason_code: String,
        target_fingerprint: [u8; 32],
        peer_fingerprint: Option<[u8; 32]>,
        now: i64,
    ) -> AccessStoreResult<()> {
        self.with_connection(move |connection| {
            let transaction=connection.transaction_with_behavior(TransactionBehavior::Immediate).map_err(map_sqlite_error)?;
            transaction.execute("DELETE FROM access_security_events WHERE event_id IN (SELECT event_id FROM access_security_events ORDER BY occurred_at DESC,event_id DESC LIMIT -1 OFFSET 4095)",[]).map_err(map_sqlite_error)?;
            transaction.execute("INSERT INTO access_security_events(event_id,occurred_at,event_kind,decision,reason_code,target_fingerprint,peer_fingerprint,metadata_json) VALUES(?1,?2,?3,?4,?5,?6,?7,'{}')",params![ulid::Ulid::new().to_string(),now,event_kind,decision,reason_code,target_fingerprint.as_slice(),peer_fingerprint.as_ref().map(<[u8;32]>::as_slice)]).map_err(map_sqlite_error)?;
            transaction.commit().map_err(map_sqlite_error)?;
            Ok(())
        }).await
    }

    pub(crate) async fn reconcile_project_policy(
        &self,
        project_id: String,
        fingerprint: [u8; 32],
        now: i64,
    ) -> AccessStoreResult<i64> {
        validate_id(&project_id)?;
        self.with_connection(move |connection| {
            let transaction = connection
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .map_err(map_sqlite_error)?;
            let epoch = reconcile_policy(&transaction, &project_id, &fingerprint, now)?;
            transaction.commit().map_err(map_sqlite_error)?;
            Ok(epoch)
        })
        .await
    }

    pub(crate) async fn activate_bootstrap_proof(
        &self,
        input: ActivateProofInput,
    ) -> AccessStoreResult<MutationOutcome> {
        validate_activate(&input)?;
        self.with_connection(move |connection| {
            let transaction = connection
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .map_err(map_sqlite_error)?;
            if let Some(exact) = transaction
                .query_row(
                    "SELECT proof_digest=?2 AND prepare_id=?3 AND installation_id=?4
                            AND installation_generation=?5 AND manifest_digest=?6
                            AND request_digest=?7 AND idempotency_digest=?8
                            AND credential_id=?9 AND credential_digest=?10
                            AND proof_generation=?11 AND expires_at=?12
                     FROM bootstrap_proofs WHERE proof_id=?1",
                    params![
                        input.proof_id,
                        input.proof_digest.as_slice(),
                        input.prepare_id,
                        input.installation_id,
                        input.installation_generation,
                        input.manifest_digest.as_slice(),
                        input.request_digest.as_slice(),
                        input.idempotency_digest.as_slice(),
                        input.credential_id,
                        input.credential_digest.as_slice(),
                        input.proof_generation,
                        input.expires_at
                    ],
                    |row| row.get::<_, bool>(0),
                )
                .optional()
                .map_err(map_sqlite_error)?
            {
                return if exact {
                    transaction.commit().map_err(map_sqlite_error)?;
                    Ok(MutationOutcome::AlreadyApplied)
                } else {
                    Err(AccessStoreError::BootstrapConflict)
                };
            }
            ensure_pristine(&transaction)?;
            if artifact_tombstoned(&transaction, "proof", &input.proof_id, &input.proof_digest)?
                || artifact_tombstoned(
                    &transaction,
                    "credential",
                    &input.credential_id,
                    &input.credential_digest,
                )?
            {
                return Err(AccessStoreError::BootstrapConflict);
            }
            bind_installation(
                &transaction,
                &input.installation_id,
                input.installation_generation,
                input.created_at,
            )?;
            transaction
                .execute(
                    "INSERT INTO bootstrap_proofs(
                    proof_id,prepare_id,installation_id,installation_generation,proof_digest,
                    manifest_digest,request_digest,idempotency_digest,credential_id,
                    credential_digest,proof_generation,semantic_attempts,status,created_at,
                    expires_at,updated_at)
                 VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,0,'active',?12,?13,?12)",
                    params![
                        input.proof_id,
                        input.prepare_id,
                        input.installation_id,
                        input.installation_generation,
                        input.proof_digest.as_slice(),
                        input.manifest_digest.as_slice(),
                        input.request_digest.as_slice(),
                        input.idempotency_digest.as_slice(),
                        input.credential_id,
                        input.credential_digest.as_slice(),
                        input.proof_generation,
                        input.created_at,
                        input.expires_at
                    ],
                )
                .map_err(map_sqlite_error)?;
            transaction.commit().map_err(map_sqlite_error)?;
            Ok(MutationOutcome::Created)
        })
        .await
    }

    pub(crate) async fn consume_bootstrap_proof(
        &self,
        input: ConsumeBootstrapInput,
    ) -> AccessStoreResult<MutationOutcome> {
        validate_consume(&input)?;
        self.with_connection(move |connection| {
            let transaction = connection
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .map_err(map_sqlite_error)?;
            let proof = transaction.query_row(
                "SELECT installation_id,installation_generation,credential_id,credential_digest,
                        proof_generation,status,expires_at,semantic_attempts,
                        proof_digest,request_digest,idempotency_digest
                 FROM bootstrap_proofs WHERE proof_id=?1",
                [input.proof_id.as_str()],
                |row| Ok((row.get::<_,String>(0)?,row.get::<_,i64>(1)?,row.get::<_,String>(2)?,
                    row.get::<_,Vec<u8>>(3)?,row.get::<_,i64>(4)?,row.get::<_,String>(5)?,
                    row.get::<_,i64>(6)?,row.get::<_,i64>(7)?,row.get::<_,Vec<u8>>(8)?,
                    row.get::<_,Vec<u8>>(9)?,row.get::<_,Vec<u8>>(10)?)),
            ).optional().map_err(map_sqlite_error)?.ok_or(AccessStoreError::NotAuthorized)?;
            if !ct_digest(&proof.8,&input.proof_digest) || !ct_digest(&proof.9,&input.request_digest)
                || !ct_digest(&proof.10,&input.idempotency_digest) || proof.3.len()!=32 {
                return Err(AccessStoreError::NotAuthorized);
            }
            if proof.5=="consumed" && idempotency_committed(&transaction,&input.idempotency_digest,&input.request_digest)? {
                transaction.execute("INSERT INTO access_security_events VALUES(?1,?2,'proof','allow','idempotent_replay',?3,NULL,'{}')",params![ulid::Ulid::new().to_string(),input.now,input.proof_digest.as_slice()]).map_err(map_sqlite_error)?;
                transaction.commit().map_err(map_sqlite_error)?;
                return Ok(MutationOutcome::AlreadyApplied);
            }
            if proof.5 != "active" || proof.6 <= input.now || proof.7 >= 8 { return Err(AccessStoreError::NotAuthorized); }
            let credential_digest:[u8;32]=proof.3.as_slice().try_into().map_err(|_|AccessStoreError::MalformedVocabulary)?;
            if artifact_tombstoned(&transaction,"proof",&input.proof_id,&input.proof_digest)?
                || artifact_tombstoned(&transaction,"credential",&proof.2,&credential_digest)?
            { return Err(AccessStoreError::NotAuthorized); }
            ensure_pristine(&transaction)?;
            transaction.execute("INSERT INTO organizations(organization_id,name,status,policy_epoch,created_at,updated_at) VALUES(?1,?2,'active',0,?3,?3)", params![ORGANIZATION_ID,input.organization_name,input.now]).map_err(map_sqlite_error)?;
            let owner_label = format!("{} owner", input.organization_name.trim());
            transaction.execute("INSERT INTO principals(principal_id,organization_id,kind,status,display_name,created_at,updated_at) VALUES(?1,?2,'user','active',?3,?4,?4)", params![PRINCIPAL_ID,ORGANIZATION_ID,owner_label,input.now]).map_err(map_sqlite_error)?;
            transaction.execute("INSERT INTO principal_links(link_id,principal_id,link_kind,issuer,subject,credential_id,status,verification_generation,link_generation,created_at,updated_at) VALUES(?1,?2,'external',?3,?4,NULL,'active',1,1,?5,?5)", params![LINK_ID,PRINCIPAL_ID,input.canonical_issuer,input.subject,input.now]).map_err(map_sqlite_error)?;
            transaction.execute("INSERT INTO projects(project_id,organization_id,name,status,project_policy_epoch,created_at,updated_at) VALUES(?1,?2,?3,'active',0,?4,?4)", params![PROJECT_ID,ORGANIZATION_ID,input.project_name,input.now]).map_err(map_sqlite_error)?;
            transaction.execute("INSERT INTO project_memberships(membership_id,organization_id,project_id,principal_id,role,status,created_by,created_at,updated_at) VALUES(?1,?2,?3,?4,'owner','active',?4,?5,?5)", params![MEMBERSHIP_ID,ORGANIZATION_ID,PROJECT_ID,PRINCIPAL_ID,input.now]).map_err(map_sqlite_error)?;
            transaction.execute("INSERT INTO project_loadouts(organization_id,project_id,loadout_name,created_by,created_at,updated_at) VALUES(?1,?2,?3,?4,?5,?5)", params![ORGANIZATION_ID,PROJECT_ID,input.loadout_id,PRINCIPAL_ID,input.now]).map_err(map_sqlite_error)?;
            let policy_epoch = reconcile_policy(&transaction, PROJECT_ID, &input.loadout_policy_fingerprint, input.now)?;
            let mut input = input;
            input.loadout_generation = policy_epoch;
            input.catalog_generation = policy_epoch;
            input.route_generation = policy_epoch;
            insert_credential(&transaction, &input, &proof)?;
            transaction.execute("INSERT INTO principal_links(link_id,principal_id,link_kind,issuer,subject,credential_id,status,verification_generation,link_generation,created_at,updated_at) VALUES(?1,?2,'local_credential',NULL,NULL,?3,'active',1,1,?4,?4)", params![format!("credential-link:{}",proof.2),PRINCIPAL_ID,proof.2,input.now]).map_err(map_sqlite_error)?;
            transaction.execute("INSERT INTO access_audit(event_id,occurred_at,correlation_id,actor_principal_id,organization_id,project_id,action,target_kind,target_fingerprint,decision,reason_code,policy_epoch,metadata_json) VALUES(?1,?2,NULL,?3,?4,?5,'access.bootstrap_owner','project',?6,'allow','explicit_owner_bootstrap',0,'{}')", params![AUDIT_ID,input.now,PRINCIPAL_ID,ORGANIZATION_ID,PROJECT_ID,input.identity_fingerprint]).map_err(map_sqlite_error)?;
            transaction.execute("UPDATE access_metadata SET global_revision=global_revision+1,bootstrap_generation=1,bootstrap_identity_fingerprint=?1,updated_at=?2 WHERE singleton=1 AND bootstrap_generation=0 AND bootstrap_identity_fingerprint IS NULL", params![input.identity_fingerprint,input.now]).map_err(map_sqlite_error)?;
            transaction.execute("UPDATE bootstrap_proofs SET status='consumed',consumed_at=?1,updated_at=?1 WHERE proof_id=?2 AND status='active'", params![input.now,input.proof_id]).map_err(map_sqlite_error)?;
            transaction.execute("INSERT INTO credential_idempotency(idempotency_digest,installation_id,operation,request_digest,proof_id,credential_id,status,created_at,updated_at) VALUES(?1,?2,'bootstrap_consume',?3,?4,?5,'committed',?6,?6)", params![input.idempotency_digest.as_slice(),proof.0,input.request_digest.as_slice(),input.proof_id,proof.2,input.now]).map_err(map_sqlite_error)?;
            transaction.execute("INSERT INTO access_security_events VALUES(?1,?2,'proof','allow','consumed',?3,NULL,'{}')",params![ulid::Ulid::new().to_string(),input.now,input.proof_digest.as_slice()]).map_err(map_sqlite_error)?;
            transaction.commit().map_err(map_sqlite_error)?;
            Ok(MutationOutcome::Created)
        }).await
    }

    pub(crate) async fn record_bootstrap_semantic_failure(
        &self,
        proof_id: String,
        proof_digest: [u8; 32],
        now: i64,
    ) -> AccessStoreResult<i64> {
        validate_id(&proof_id)?;
        self.with_connection(move |connection| {
            let transaction=connection.transaction_with_behavior(TransactionBehavior::Immediate).map_err(map_sqlite_error)?;
            let row=transaction.query_row("SELECT proof_digest,status,expires_at,semantic_attempts FROM bootstrap_proofs WHERE proof_id=?1",[proof_id.as_str()],|row|Ok((row.get::<_,Vec<u8>>(0)?,row.get::<_,String>(1)?,row.get::<_,i64>(2)?,row.get::<_,i64>(3)?))).optional().map_err(map_sqlite_error)?.ok_or(AccessStoreError::NotAuthorized)?;
            // Authentication precedes accounting: a public proof ID paired
            // with the wrong secret must never consume semantic attempts.
            if !ct_digest(&row.0,&proof_digest) { return Err(AccessStoreError::NotAuthorized); }
            if row.1 != "active" || row.2 <= now || row.3 >= 8 { return Err(AccessStoreError::NotAuthorized); }
            let attempts=row.3+1;
            transaction.execute("UPDATE bootstrap_proofs SET semantic_attempts=?1,status=CASE WHEN ?1>=8 THEN 'expired' ELSE status END,updated_at=?2 WHERE proof_id=?3 AND status='active' AND semantic_attempts=?4",params![attempts,now,proof_id,row.3]).map_err(map_sqlite_error)?;
            transaction.commit().map_err(map_sqlite_error)?;
            Ok(attempts)
        }).await
    }

    pub(crate) async fn issue_project_credential(
        &self,
        input: IssueCredentialInput,
    ) -> AccessStoreResult<MutationOutcome> {
        validate_issue(&input)?;
        self.with_connection(move |connection| {
            let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate).map_err(map_sqlite_error)?;
            let actor = credential_by_authenticated_actor(&transaction,&input.actor_credential_id,input.actor_credential_generation,input.issued_at)?.ok_or(AccessStoreError::NotAuthorized)?;
            if idempotency_committed(&transaction,&input.idempotency_digest,&input.request_digest)? {
                transaction.execute("INSERT INTO access_security_events VALUES(?1,?2,'credential_issue','allow','idempotent_replay',?3,NULL,'{}')",params![ulid::Ulid::new().to_string(),input.issued_at,input.credential_digest.as_slice()]).map_err(map_sqlite_error)?;
                transaction.commit().map_err(map_sqlite_error)?;
                return Ok(MutationOutcome::AlreadyApplied);
            }
            if artifact_tombstoned(&transaction,"credential",&input.credential_id,&input.credential_digest)? { return Err(AccessStoreError::NotAuthorized); }
            if !normalized_scopes(&input.scopes_json)?.is_subset(&normalized_scopes(&actor.scopes_json)?)
                || input.expires_at > actor.expires_at
            {
                return Err(AccessStoreError::NotAuthorized);
            }
            let authority:Option<(String,i64,i64,i64,String,i64)>=transaction.query_row("SELECT m.role,m.updated_at,o.policy_epoch,p.project_policy_epoch,l.loadout_name,l.updated_at FROM project_memberships m JOIN organizations o ON o.organization_id=m.organization_id JOIN projects p ON p.organization_id=m.organization_id AND p.project_id=m.project_id JOIN project_loadouts l ON l.organization_id=m.organization_id AND l.project_id=m.project_id WHERE m.organization_id=?1 AND m.project_id=?2 AND m.principal_id=?3 AND m.status='active' AND o.status='active' AND p.status='active'",params![actor.organization_id,actor.project_id,actor.principal_id],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?,r.get(4)?,r.get(5)?))).optional().map_err(map_sqlite_error)?;
            let Some((role,membership_generation,organization_epoch,project_epoch,loadout_id,assignment_generation))=authority else{return Err(AccessStoreError::NotAuthorized)};
            if !matches!(role.as_str(),"owner"|"admin") || membership_generation!=actor.membership_generation || organization_epoch!=actor.organization_policy_epoch || project_epoch!=actor.project_policy_epoch || loadout_id!=actor.loadout_id || assignment_generation!=actor.loadout_assignment_generation {return Err(AccessStoreError::NotAuthorized);}
            transaction.execute("INSERT INTO project_credentials SELECT ?1,installation_id,installation_generation,?2,?3,canonical_issuer,subject,organization_id,principal_id,project_id,membership_generation,organization_policy_epoch,project_policy_epoch,loadout_id,loadout_generation,loadout_assignment_generation,catalog_generation,loadout_policy_fingerprint,route_id,route_generation,resource,audience,?4,'active',?5,?6,NULL,0,?5 FROM project_credentials WHERE credential_id=?7", params![input.credential_id,input.credential_digest.as_slice(),input.credential_generation,input.scopes_json,input.issued_at,input.expires_at,input.actor_credential_id]).map_err(map_sqlite_error)?;
            transaction.execute("INSERT INTO principal_links(link_id,principal_id,link_kind,issuer,subject,credential_id,status,verification_generation,link_generation,created_at,updated_at) VALUES(?1,?2,'local_credential',NULL,NULL,?3,'active',1,1,?4,?4)", params![format!("credential-link:{}",input.credential_id),actor.principal_id,input.credential_id,input.issued_at]).map_err(map_sqlite_error)?;
            transaction.execute("INSERT INTO credential_idempotency(idempotency_digest,installation_id,operation,request_digest,proof_id,credential_id,status,created_at,updated_at) VALUES(?1,?2,'issue',?3,NULL,?4,'committed',?5,?5)", params![input.idempotency_digest.as_slice(),actor.installation_id,input.request_digest.as_slice(),input.credential_id,input.issued_at]).map_err(map_sqlite_error)?;
            transaction.execute("INSERT INTO access_security_events VALUES(?1,?2,'credential_issue','allow','issued',?3,NULL,'{}')",params![ulid::Ulid::new().to_string(),input.issued_at,input.credential_digest.as_slice()]).map_err(map_sqlite_error)?;
            transaction.commit().map_err(map_sqlite_error)?;
            Ok(MutationOutcome::Created)
        }).await
    }

    pub(crate) async fn introspect_project_credential(
        &self,
        credential_id: String,
        credential_generation: i64,
        now: i64,
    ) -> AccessStoreResult<Option<CredentialSnapshot>> {
        validate_id(&credential_id)?;
        self.with_connection(move |connection| {
            credential_by_authenticated_actor(
                connection,
                &credential_id,
                credential_generation,
                now,
            )
        })
        .await
    }

    pub(crate) async fn revoke_project_credential(
        &self,
        actor_id: String,
        actor_generation: i64,
        target_id: String,
        now: i64,
    ) -> AccessStoreResult<MutationOutcome> {
        validate_id(&actor_id)?;
        validate_id(&target_id)?;
        self.with_connection(move |connection| {
            let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate).map_err(map_sqlite_error)?;
            let actor = credential_by_authenticated_actor(&transaction,&actor_id,actor_generation,now)?.ok_or(AccessStoreError::NotAuthorized)?;
            let target: Option<(String,String,String,Vec<u8>)> = transaction.query_row("SELECT organization_id,project_id,principal_id,credential_digest FROM project_credentials WHERE credential_id=?1",[&target_id],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?))).optional().map_err(map_sqlite_error)?;
            let Some(target)=target else { transaction.commit().map_err(map_sqlite_error)?; return Ok(MutationOutcome::AlreadyApplied); };
            let self_revoke=actor_id==target_id;
            if !self_revoke {
                let role:Option<String>=transaction.query_row("SELECT role FROM project_memberships WHERE organization_id=?1 AND project_id=?2 AND principal_id=?3 AND status='active'",params![actor.organization_id,actor.project_id,actor.principal_id],|r|r.get(0)).optional().map_err(map_sqlite_error)?;
                if target.0!=actor.organization_id || target.1!=actor.project_id || !matches!(role.as_deref(),Some("owner"|"admin")){
                    transaction.commit().map_err(map_sqlite_error)?;
                    return Ok(MutationOutcome::AlreadyApplied);
                }
            }
            let changed=transaction.execute("UPDATE project_credentials SET status='revoked',revoked_at=?1,revocation_generation=revocation_generation+1,updated_at=?1 WHERE credential_id=?2 AND status='active'",params![now,target_id]).map_err(map_sqlite_error)?;
            transaction.execute("UPDATE principal_links SET status='revoked',link_generation=link_generation+1,updated_at=?1 WHERE credential_id=?2",params![now,target_id]).map_err(map_sqlite_error)?;
            transaction.execute("INSERT INTO access_security_events VALUES(?1,?2,'credential_revoke','allow',?3,?4,NULL,'{}')",params![ulid::Ulid::new().to_string(),now,if changed==0{"idempotent_replay"}else{"revoked"},target.3]).map_err(map_sqlite_error)?;
            transaction.commit().map_err(map_sqlite_error)?;
            Ok(if changed==0{MutationOutcome::AlreadyApplied}else{MutationOutcome::Created})
        }).await
    }

    pub(crate) async fn tombstone_access_artifact(
        &self,
        installation_id: String,
        kind: String,
        public_id: String,
        digest: [u8; 32],
        generation: i64,
        reason: String,
        now: i64,
    ) -> AccessStoreResult<MutationOutcome> {
        self.tombstone_access_artifacts(
            installation_id,
            vec![(kind, public_id, digest, generation)],
            reason,
            now,
        )
        .await
    }

    pub(crate) async fn tombstone_access_artifacts(
        &self,
        installation_id: String,
        artifacts: Vec<(String, String, [u8; 32], i64)>,
        reason: String,
        now: i64,
    ) -> AccessStoreResult<MutationOutcome> {
        validate_id(&installation_id)?;
        validate_id(&reason)?;
        if artifacts.is_empty()
            || artifacts.iter().any(|(kind, public_id, _, generation)| {
                validate_id(public_id).is_err()
                    || !matches!(
                        kind.as_str(),
                        "prepare" | "proof" | "credential" | "session_source"
                    )
                    || *generation <= 0
            })
        {
            return Err(AccessStoreError::InvalidBootstrapInput);
        }
        self.with_connection(move|connection|{
            let transaction=connection.transaction_with_behavior(TransactionBehavior::Immediate).map_err(map_sqlite_error)?;
            let mut any_changed = false;
            for (kind, public_id, digest, generation) in artifacts {
                let changed=transaction.execute("INSERT INTO access_tombstones(tombstone_id,installation_id,artifact_kind,public_id,canonical_digest,artifact_generation,reason_code,created_at) VALUES(?1,?2,?3,?4,?5,?6,?7,?8) ON CONFLICT(installation_id,artifact_kind,public_id) DO NOTHING",params![format!("tombstone:{kind}:{public_id}:{generation}"),installation_id,kind,public_id,digest.as_slice(),generation,reason,now]).map_err(map_sqlite_error)?;
                if kind=="proof"{transaction.execute("UPDATE bootstrap_proofs SET status='tombstoned',revoked_at=?1,updated_at=?1 WHERE proof_id=?2 AND status!='consumed'",params![now,public_id]).map_err(map_sqlite_error)?;}
                if kind=="credential"{transaction.execute("UPDATE project_credentials SET status='tombstoned',revoked_at=?1,revocation_generation=revocation_generation+1,updated_at=?1 WHERE credential_id=?2 AND status!='tombstoned'",params![now,public_id]).map_err(map_sqlite_error)?;}
                if changed==0 {
                    let exact:bool=transaction.query_row("SELECT artifact_generation=?4 AND reason_code=?5 AND canonical_digest=?6 FROM access_tombstones WHERE installation_id=?1 AND artifact_kind=?2 AND public_id=?3",params![installation_id,kind,public_id,generation,reason,digest.as_slice()],|r|r.get(0)).map_err(map_sqlite_error)?;
                    if !exact{return Err(AccessStoreError::BootstrapConflict)}
                } else {
                    any_changed = true;
                }
            }
            transaction.commit().map_err(map_sqlite_error)?;
            Ok(if any_changed{MutationOutcome::Created}else{MutationOutcome::AlreadyApplied})
        }).await
    }
}

fn reconcile_policy(
    transaction: &Transaction<'_>,
    project_id: &str,
    fingerprint: &[u8; 32],
    now: i64,
) -> AccessStoreResult<i64> {
    let current = transaction
        .query_row(
            "SELECT policy_fingerprint,policy_epoch FROM project_policy_publications WHERE project_id=?1",
            [project_id],
            |row| Ok((row.get::<_, Vec<u8>>(0)?, row.get::<_, i64>(1)?)),
        )
        .optional()
        .map_err(map_sqlite_error)?;
    match current {
        None => {
            transaction.execute("INSERT INTO project_policy_publications(project_id,policy_fingerprint,policy_epoch,updated_at) VALUES(?1,?2,1,?3)", params![project_id, fingerprint.as_slice(), now]).map_err(map_sqlite_error)?;
            Ok(1)
        }
        Some((stored, epoch)) if stored.as_slice() == fingerprint => Ok(epoch),
        Some((_stored, epoch)) => {
            let next = epoch
                .checked_add(1)
                .ok_or(AccessStoreError::ProjectAccessUnavailable)?;
            transaction.execute("UPDATE project_policy_publications SET policy_fingerprint=?1,policy_epoch=?2,updated_at=?3 WHERE project_id=?4 AND policy_epoch=?5", params![fingerprint.as_slice(),next,now,project_id,epoch]).map_err(map_sqlite_error)?;
            Ok(next)
        }
    }
}

fn ensure_pristine(transaction: &Transaction<'_>) -> AccessStoreResult<()> {
    let state:bool=transaction.query_row("SELECT bootstrap_generation=0 AND bootstrap_identity_fingerprint IS NULL AND NOT EXISTS(SELECT 1 FROM organizations) AND NOT EXISTS(SELECT 1 FROM principals) AND NOT EXISTS(SELECT 1 FROM projects) FROM access_metadata WHERE singleton=1",[],|r|r.get(0)).map_err(map_sqlite_error)?;
    if state {
        Ok(())
    } else {
        Err(AccessStoreError::BootstrapConflict)
    }
}
fn bind_installation(
    tx: &Transaction<'_>,
    id: &str,
    generation: i64,
    now: i64,
) -> AccessStoreResult<()> {
    let changed=tx.execute("INSERT INTO access_installations(singleton,installation_id,installation_generation,created_at,updated_at) VALUES(1,?1,?2,?3,?3) ON CONFLICT(singleton) DO NOTHING",params![id,generation,now]).map_err(map_sqlite_error)?;
    if changed == 1 {
        return Ok(());
    }
    let exact:bool=tx.query_row("SELECT installation_id=?1 AND installation_generation=?2 FROM access_installations WHERE singleton=1",params![id,generation],|r|r.get(0)).map_err(map_sqlite_error)?;
    if exact {
        Ok(())
    } else {
        Err(AccessStoreError::BootstrapConflict)
    }
}
fn idempotency_committed(
    tx: &Transaction<'_>,
    id: &[u8; 32],
    request: &[u8; 32],
) -> AccessStoreResult<bool> {
    let row = tx
        .query_row(
            "SELECT request_digest,status FROM credential_idempotency WHERE idempotency_digest=?1",
            [id.as_slice()],
            |r| Ok((r.get::<_, Vec<u8>>(0)?, r.get::<_, String>(1)?)),
        )
        .optional()
        .map_err(map_sqlite_error)?;
    match row {
        None => Ok(false),
        Some((stored, status)) if ct_digest(&stored, request) && status == "committed" => Ok(true),
        Some(_) => Err(AccessStoreError::BootstrapConflict),
    }
}
fn insert_credential(
    tx: &Transaction<'_>,
    input: &ConsumeBootstrapInput,
    proof: &(
        String,
        i64,
        String,
        Vec<u8>,
        i64,
        String,
        i64,
        i64,
        Vec<u8>,
        Vec<u8>,
        Vec<u8>,
    ),
) -> AccessStoreResult<()> {
    tx.execute("INSERT INTO project_credentials(credential_id,installation_id,installation_generation,credential_digest,credential_generation,canonical_issuer,subject,organization_id,principal_id,project_id,membership_generation,organization_policy_epoch,project_policy_epoch,loadout_id,loadout_generation,loadout_assignment_generation,catalog_generation,loadout_policy_fingerprint,route_id,route_generation,resource,audience,scopes_json,status,issued_at,expires_at,revoked_at,revocation_generation,updated_at) VALUES(?1,?2,?3,?4,1,?5,?6,?7,?8,?9,?19,0,0,?10,?11,?19,?12,?13,?14,?15,?16,?17,?18,'active',?19,?20,NULL,0,?19)",params![proof.2,proof.0,proof.1,proof.3,input.canonical_issuer,input.subject,ORGANIZATION_ID,PRINCIPAL_ID,PROJECT_ID,input.loadout_id,input.loadout_generation,input.catalog_generation,input.loadout_policy_fingerprint.as_slice(),input.route_id,input.route_generation,input.resource,input.audience,input.scopes_json,input.now,input.credential_expires_at]).map_err(map_sqlite_error)?;
    Ok(())
}
fn credential_by_secret(
    connection: &rusqlite::Connection,
    id: &str,
    digest: &[u8; 32],
    now: i64,
) -> AccessStoreResult<Option<CredentialSnapshot>> {
    let row=connection.query_row("SELECT credential_digest,credential_id,installation_id,canonical_issuer,subject,organization_id,principal_id,project_id,loadout_id,route_id,resource,audience,scopes_json,credential_generation,membership_generation,organization_policy_epoch,project_policy_epoch,loadout_assignment_generation,expires_at,revocation_generation FROM project_credentials WHERE credential_id=?1 AND status='active' AND expires_at>?2",params![id,now],|r|Ok((r.get::<_,Vec<u8>>(0)?,CredentialSnapshot{credential_id:r.get(1)?,installation_id:r.get(2)?,canonical_issuer:r.get(3)?,subject:r.get(4)?,organization_id:r.get(5)?,principal_id:r.get(6)?,project_id:r.get(7)?,loadout_id:r.get(8)?,route_id:r.get(9)?,resource:r.get(10)?,audience:r.get(11)?,scopes_json:r.get(12)?,credential_generation:r.get(13)?,membership_generation:r.get(14)?,organization_policy_epoch:r.get(15)?,project_policy_epoch:r.get(16)?,loadout_assignment_generation:r.get(17)?,expires_at:r.get(18)?,revocation_generation:r.get(19)?}))).optional().map_err(map_sqlite_error)?;
    let Some((stored, snapshot)) = row else {
        return Ok(None);
    };
    if !ct_digest(&stored, digest) {
        return Ok(None);
    }
    let tombstoned=connection.query_row("SELECT EXISTS(SELECT 1 FROM access_tombstones WHERE artifact_kind='credential' AND (public_id=?1 OR canonical_digest=?2))",params![id,digest.as_slice()],|r|r.get::<_,bool>(0)).map_err(map_sqlite_error)?;
    Ok((!tombstoned).then_some(snapshot))
}
fn credential_by_authenticated_actor(
    connection: &rusqlite::Connection,
    id: &str,
    generation: i64,
    now: i64,
) -> AccessStoreResult<Option<CredentialSnapshot>> {
    if generation <= 0 {
        return Ok(None);
    }
    let digest = connection
        .query_row(
            "SELECT credential_digest FROM project_credentials
             WHERE credential_id=?1 AND credential_generation=?2
               AND status='active' AND expires_at>?3",
            params![id, generation, now],
            |row| row.get::<_, Vec<u8>>(0),
        )
        .optional()
        .map_err(map_sqlite_error)?;
    let Some(digest) = digest.and_then(|digest| digest.try_into().ok()) else {
        return Ok(None);
    };
    credential_by_secret(connection, id, &digest, now)
}
fn ct_digest(stored: &[u8], expected: &[u8; 32]) -> bool {
    stored.len() == 32 && bool::from(stored.ct_eq(expected.as_slice()))
}
fn artifact_tombstoned(
    connection: &rusqlite::Connection,
    kind: &str,
    id: &str,
    digest: &[u8; 32],
) -> AccessStoreResult<bool> {
    connection.query_row("SELECT EXISTS(SELECT 1 FROM access_tombstones WHERE artifact_kind=?1 AND (public_id=?2 OR canonical_digest=?3))",params![kind,id,digest.as_slice()],|r|r.get(0)).map_err(map_sqlite_error)
}
fn normalized_scopes(value: &str) -> AccessStoreResult<std::collections::BTreeSet<String>> {
    let scopes: Vec<String> =
        serde_json::from_str(value).map_err(|_| AccessStoreError::InvalidBootstrapInput)?;
    if scopes.is_empty()
        || scopes
            .iter()
            .any(|s| s.is_empty() || s.len() > MAX_ID || s.chars().any(char::is_control))
    {
        return Err(AccessStoreError::InvalidBootstrapInput);
    }
    let unique = scopes
        .iter()
        .cloned()
        .collect::<std::collections::BTreeSet<_>>();
    if unique.len() != scopes.len() || unique.iter().ne(scopes.iter()) {
        return Err(AccessStoreError::InvalidBootstrapInput);
    }
    Ok(unique)
}
fn validate_activate(v: &ActivateProofInput) -> AccessStoreResult<()> {
    validate_id(&v.proof_id)?;
    validate_id(&v.prepare_id)?;
    validate_id(&v.installation_id)?;
    validate_id(&v.credential_id)?;
    if v.installation_generation <= 0
        || v.proof_generation <= 0
        || v.created_at < 0
        || v.expires_at <= v.created_at
    {
        return Err(AccessStoreError::InvalidBootstrapInput);
    }
    Ok(())
}
fn validate_consume(v: &ConsumeBootstrapInput) -> AccessStoreResult<()> {
    for s in [
        &v.proof_id,
        &v.organization_name,
        &v.project_name,
        &v.canonical_issuer,
        &v.subject,
        &v.identity_fingerprint,
        &v.loadout_id,
        &v.route_id,
    ] {
        validate_id(s)?;
    }
    if v.organization_name.len() > MAX_NAME
        || v.project_name.len() > MAX_NAME
        || v.resource.len() > MAX_URI
        || v.audience.len() > MAX_URI
        || v.scopes_json.len() > MAX_SCOPES_JSON
        || v.loadout_generation <= 0
        || v.catalog_generation <= 0
        || v.route_generation <= 0
        || v.credential_expires_at <= v.now
        || normalized_scopes(&v.scopes_json).is_err()
    {
        return Err(AccessStoreError::InvalidBootstrapInput);
    }
    Ok(())
}
fn validate_issue(v: &IssueCredentialInput) -> AccessStoreResult<()> {
    validate_id(&v.actor_credential_id)?;
    validate_id(&v.credential_id)?;
    if v.credential_generation <= 0
        || v.issued_at < 0
        || v.expires_at <= v.issued_at
        || v.scopes_json.len() > MAX_SCOPES_JSON
        || normalized_scopes(&v.scopes_json).is_err()
    {
        return Err(AccessStoreError::InvalidBootstrapInput);
    }
    Ok(())
}
fn validate_id(value: &str) -> AccessStoreResult<()> {
    if value.is_empty() || value.len() > MAX_ID || value.chars().any(char::is_control) {
        Err(AccessStoreError::InvalidBootstrapInput)
    } else {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use labby_auth::{Authenticator, VerifiedIdentity};

    use super::*;

    async fn store() -> (tempfile::TempDir, AccessStore) {
        let directory = tempfile::tempdir().unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            std::fs::set_permissions(directory.path(), std::fs::Permissions::from_mode(0o700))
                .unwrap();
        }
        let store = AccessStore::open(directory.path().canonicalize().unwrap().join("access.db"))
            .await
            .unwrap();
        (directory, store)
    }

    fn activation() -> ActivateProofInput {
        ActivateProofInput {
            proof_id: "proof-1".into(),
            prepare_id: "prepare-1".into(),
            installation_id: "installation-1".into(),
            installation_generation: 1,
            proof_digest: [1; 32],
            manifest_digest: [2; 32],
            request_digest: [3; 32],
            idempotency_digest: [4; 32],
            credential_id: "credential-1".into(),
            credential_digest: [5; 32],
            proof_generation: 1,
            created_at: 10,
            expires_at: 100,
        }
    }

    fn consumption() -> ConsumeBootstrapInput {
        let identity = VerifiedIdentity::external(
            Authenticator::BrowserSession,
            "https://accounts.google.com",
            "operator-1",
        )
        .unwrap();
        ConsumeBootstrapInput {
            proof_id: "proof-1".into(),
            proof_digest: [1; 32],
            request_digest: [3; 32],
            idempotency_digest: [4; 32],
            organization_name: "Local".into(),
            project_name: "Default".into(),
            canonical_issuer: "https://accounts.google.com".into(),
            subject: "operator-1".into(),
            identity_fingerprint: identity.safe_fingerprint(),
            loadout_id: "production".into(),
            loadout_generation: 1,
            catalog_generation: 1,
            loadout_policy_fingerprint: [6; 32],
            route_id: "root".into(),
            route_generation: 1,
            resource: "http://127.0.0.1/mcp".into(),
            audience: "http://127.0.0.1/mcp".into(),
            scopes_json: r#"["lab:read"]"#.into(),
            now: 20,
            credential_expires_at: 90,
        }
    }

    #[tokio::test]
    async fn activation_and_consume_are_exactly_idempotent() {
        let (_directory, store) = store().await;
        assert_eq!(
            store.activate_bootstrap_proof(activation()).await.unwrap(),
            MutationOutcome::Created
        );
        assert_eq!(
            store.activate_bootstrap_proof(activation()).await.unwrap(),
            MutationOutcome::AlreadyApplied
        );
        assert_eq!(
            store.consume_bootstrap_proof(consumption()).await.unwrap(),
            MutationOutcome::Created
        );
        assert_eq!(
            store.consume_bootstrap_proof(consumption()).await.unwrap(),
            MutationOutcome::AlreadyApplied
        );
        assert!(
            store
                .introspect_project_credential("credential-1".into(), 1, 21)
                .await
                .unwrap()
                .is_some()
        );
    }

    #[tokio::test]
    async fn concurrent_activation_has_one_commit_and_one_exact_replay() {
        let (_directory, store) = store().await;
        let (left, right) = tokio::join!(
            store.activate_bootstrap_proof(activation()),
            store.activate_bootstrap_proof(activation())
        );
        let outcomes = [left.unwrap(), right.unwrap()];
        assert_eq!(
            outcomes
                .iter()
                .filter(|outcome| **outcome == MutationOutcome::Created)
                .count(),
            1
        );
        assert_eq!(
            outcomes
                .iter()
                .filter(|outcome| **outcome == MutationOutcome::AlreadyApplied)
                .count(),
            1
        );
    }

    #[tokio::test]
    async fn policy_epoch_is_restart_stable_and_never_reuses_an_a_b_a_epoch() {
        let (directory, store) = store().await;
        assert_eq!(
            store
                .reconcile_project_policy(PROJECT_ID.into(), [1; 32], 10)
                .await
                .unwrap(),
            1
        );
        assert_eq!(
            store
                .reconcile_project_policy(PROJECT_ID.into(), [1; 32], 11)
                .await
                .unwrap(),
            1
        );
        let path = directory.path().canonicalize().unwrap().join("access.db");
        drop(store);
        let reopened = AccessStore::open(path).await.unwrap();
        assert_eq!(
            reopened
                .reconcile_project_policy(PROJECT_ID.into(), [2; 32], 12)
                .await
                .unwrap(),
            2
        );
        assert_eq!(
            reopened
                .reconcile_project_policy(PROJECT_ID.into(), [1; 32], 13)
                .await
                .unwrap(),
            3
        );
    }

    #[tokio::test]
    async fn wrong_proof_secret_never_burns_semantic_attempts_but_correct_secret_exhausts_at_eight()
    {
        let (_directory, store) = store().await;
        store.activate_bootstrap_proof(activation()).await.unwrap();
        assert!(matches!(
            store
                .record_bootstrap_semantic_failure("proof-1".into(), [9; 32], 20)
                .await,
            Err(AccessStoreError::NotAuthorized)
        ));
        for expected in 1..=8 {
            assert_eq!(
                store
                    .record_bootstrap_semantic_failure("proof-1".into(), [1; 32], 20 + expected)
                    .await
                    .unwrap(),
                expected
            );
        }
        assert!(matches!(
            store
                .record_bootstrap_semantic_failure("proof-1".into(), [1; 32], 40)
                .await,
            Err(AccessStoreError::NotAuthorized)
        ));
    }

    #[tokio::test]
    async fn admission_windows_and_redacted_events_are_durable_and_bounded() {
        let (directory, store) = store().await;
        assert!(
            store
                .admit_security_operation("proof_peer".into(), [7; 32], 10, 60, 2)
                .await
                .unwrap()
        );
        assert!(
            store
                .admit_security_operation("proof_peer".into(), [7; 32], 11, 60, 2)
                .await
                .unwrap()
        );
        assert!(
            !store
                .admit_security_operation("proof_peer".into(), [7; 32], 12, 60, 2)
                .await
                .unwrap()
        );
        store
            .record_security_event(
                "proof".into(),
                "deny".into(),
                "rate_limited".into(),
                [8; 32],
                Some([7; 32]),
                12,
            )
            .await
            .unwrap();
        let path = directory.path().canonicalize().unwrap().join("access.db");
        drop(store);
        let reopened = AccessStore::open(path).await.unwrap();
        assert!(
            !reopened
                .admit_security_operation("proof_peer".into(), [7; 32], 13, 60, 2)
                .await
                .unwrap()
        );
        let event:(String,String,i64)=reopened.with_connection(|connection| connection.query_row("SELECT decision,reason_code,length(target_fingerprint) FROM access_security_events",[],|row|Ok((row.get(0)?,row.get(1)?,row.get(2)?))).map_err(map_sqlite_error)).await.unwrap();
        assert_eq!(event, ("deny".into(), "rate_limited".into(), 32));
    }

    #[tokio::test]
    async fn issue_revoke_and_tombstone_are_generation_bound_and_terminal() {
        let (_directory, store) = store().await;
        store.activate_bootstrap_proof(activation()).await.unwrap();
        store.consume_bootstrap_proof(consumption()).await.unwrap();
        let issue = IssueCredentialInput {
            actor_credential_id: "credential-1".into(),
            actor_credential_generation: 1,
            credential_id: "credential-2".into(),
            credential_digest: [7; 32],
            credential_generation: 1,
            scopes_json: r#"["lab:read"]"#.into(),
            issued_at: 30,
            expires_at: 80,
            idempotency_digest: [8; 32],
            request_digest: [9; 32],
        };
        assert_eq!(
            store.issue_project_credential(issue.clone()).await.unwrap(),
            MutationOutcome::Created
        );
        assert_eq!(
            store.issue_project_credential(issue).await.unwrap(),
            MutationOutcome::AlreadyApplied
        );
        assert_eq!(
            store
                .revoke_project_credential("credential-1".into(), 1, "credential-2".into(), 40,)
                .await
                .unwrap(),
            MutationOutcome::Created
        );
        assert!(
            store
                .introspect_project_credential("credential-2".into(), 1, 41)
                .await
                .unwrap()
                .is_none()
        );
        store
            .tombstone_access_artifact(
                "installation-1".into(),
                "credential".into(),
                "credential-1".into(),
                [5; 32],
                1,
                "cleanup".into(),
                50,
            )
            .await
            .unwrap();
        assert!(
            store
                .introspect_project_credential("credential-1".into(), 1, 51)
                .await
                .unwrap()
                .is_none()
        );
    }

    #[tokio::test]
    async fn batch_tombstones_roll_back_together_and_retry_exactly() {
        let (_directory, store) = store().await;
        store.activate_bootstrap_proof(activation()).await.unwrap();
        store.consume_bootstrap_proof(consumption()).await.unwrap();
        store
            .with_connection(|connection| {
                connection
                    .execute_batch(
                        "CREATE TEMP TRIGGER fail_proof_tombstone BEFORE INSERT ON access_tombstones WHEN NEW.artifact_kind='proof' BEGIN SELECT RAISE(ABORT, 'injected proof failure'); END;",
                    )
                    .map_err(map_sqlite_error)
            })
            .await
            .unwrap();
        let artifacts = vec![
            ("credential".into(), "credential-1".into(), [5; 32], 1),
            ("proof".into(), "proof-1".into(), [3; 32], 1),
            ("prepare".into(), "prepare-1".into(), [8; 32], 1),
        ];
        assert!(
            store
                .tombstone_access_artifacts(
                    "installation-1".into(),
                    artifacts.clone(),
                    "cleanup".into(),
                    50,
                )
                .await
                .is_err()
        );
        let count: i64 = store
            .with_connection(|connection| {
                connection
                    .query_row("SELECT count(*) FROM access_tombstones", [], |row| {
                        row.get(0)
                    })
                    .map_err(map_sqlite_error)
            })
            .await
            .unwrap();
        assert_eq!(
            count, 0,
            "a failed batch must not retain an early tombstone"
        );
        store
            .with_connection(|connection| {
                connection
                    .execute_batch("DROP TRIGGER fail_proof_tombstone")
                    .map_err(map_sqlite_error)
            })
            .await
            .unwrap();
        assert_eq!(
            store
                .tombstone_access_artifacts(
                    "installation-1".into(),
                    artifacts.clone(),
                    "cleanup".into(),
                    50,
                )
                .await
                .unwrap(),
            MutationOutcome::Created
        );
        assert_eq!(
            store
                .tombstone_access_artifacts(
                    "installation-1".into(),
                    artifacts,
                    "cleanup".into(),
                    50,
                )
                .await
                .unwrap(),
            MutationOutcome::AlreadyApplied
        );
    }
}
