//! Durable Dev Container contract ledger. No container runtime work belongs here.

use labby_primitives::access::{OwnerKind, OwnerScope};
use labby_primitives::dev_container::{
    ApprovedTemplate, DesiredState, HostCapability, ObservedState, OwnedDevContainer,
    SecretReference,
};
use rusqlite::{Connection, OptionalExtension, params};
use thiserror::Error;

pub(super) const DEV_CONTAINER_SCHEMA: &str = "
CREATE TABLE dev_container_templates (
    template_id TEXT PRIMARY KEY CHECK(length(trim(template_id)) > 0),
    image_digest TEXT NOT NULL CHECK(length(image_digest) = 71 AND substr(image_digest,1,7) = 'sha256:' AND substr(image_digest,8) NOT GLOB '*[^0-9a-f]*'),
    max_active_instances INTEGER NOT NULL CHECK(max_active_instances > 0),
    cpu_millis INTEGER NOT NULL CHECK(cpu_millis > 0),
    memory_bytes INTEGER NOT NULL CHECK(memory_bytes > 0),
    disk_bytes INTEGER NOT NULL CHECK(disk_bytes > 0),
    max_lifetime_seconds INTEGER NOT NULL CHECK(max_lifetime_seconds > 0),
    host_capabilities_json TEXT NOT NULL CHECK(json_valid(host_capabilities_json)),
    status TEXT NOT NULL CHECK(status IN ('approved','revoked')),
    policy_epoch INTEGER NOT NULL CHECK(policy_epoch > 0),
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL
) STRICT;

CREATE TABLE dev_container_owner_quotas (
    owner_kind TEXT NOT NULL CHECK(owner_kind IN ('installation','team','project','personal')),
    owner_id TEXT NOT NULL CHECK(length(trim(owner_id)) > 0),
    max_active_instances INTEGER NOT NULL CHECK(max_active_instances > 0),
    policy_epoch INTEGER NOT NULL CHECK(policy_epoch > 0),
    updated_at INTEGER NOT NULL,
    PRIMARY KEY(owner_kind, owner_id)
) STRICT;

CREATE TABLE dev_container_instances (
    instance_id TEXT PRIMARY KEY CHECK(length(trim(instance_id)) > 0),
    owner_kind TEXT NOT NULL CHECK(owner_kind IN ('installation','team','project','personal')),
    owner_id TEXT NOT NULL CHECK(length(trim(owner_id)) > 0),
    template_id TEXT NOT NULL,
    image_digest TEXT NOT NULL CHECK(length(image_digest) = 71 AND substr(image_digest,1,7) = 'sha256:' AND substr(image_digest,8) NOT GLOB '*[^0-9a-f]*'),
    lifecycle_nonce TEXT NOT NULL UNIQUE CHECK(length(lifecycle_nonce) BETWEEN 32 AND 128),
    desired_state TEXT NOT NULL CHECK(desired_state IN ('running','stopped','deleted')),
    observed_state TEXT NOT NULL CHECK(observed_state IN ('pending','starting','running','stopping','stopped','failed','deleted')),
    cpu_millis INTEGER NOT NULL CHECK(cpu_millis > 0),
    memory_bytes INTEGER NOT NULL CHECK(memory_bytes > 0),
    disk_bytes INTEGER NOT NULL CHECK(disk_bytes > 0),
    lifetime_seconds INTEGER NOT NULL CHECK(lifetime_seconds > 0),
    secret_references_json TEXT NOT NULL CHECK(json_valid(secret_references_json)),
    authority_fingerprint TEXT NOT NULL CHECK(length(trim(authority_fingerprint)) > 0),
    revision INTEGER NOT NULL CHECK(revision > 0),
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL,
    deleted_at INTEGER,
    CHECK ((desired_state = 'deleted') = (deleted_at IS NOT NULL)),
    FOREIGN KEY(template_id) REFERENCES dev_container_templates(template_id) ON DELETE RESTRICT
) STRICT;
CREATE INDEX dev_container_instances_owner_state
    ON dev_container_instances(owner_kind, owner_id, desired_state, observed_state, instance_id);

CREATE TABLE dev_container_ledger (
    event_id TEXT PRIMARY KEY CHECK(length(trim(event_id)) > 0),
    instance_id TEXT NOT NULL,
    lifecycle_nonce TEXT NOT NULL,
    revision INTEGER NOT NULL CHECK(revision > 0),
    event_kind TEXT NOT NULL CHECK(event_kind IN ('created','desired_changed','observed_changed','reconciled')),
    occurred_at INTEGER NOT NULL,
    detail_json TEXT NOT NULL CHECK(json_valid(detail_json) AND length(detail_json) <= 4096),
    UNIQUE(instance_id, revision),
    FOREIGN KEY(instance_id) REFERENCES dev_container_instances(instance_id) ON DELETE RESTRICT
) STRICT;
";

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct ReservedResources {
    pub cpu_millis: u32,
    pub memory_bytes: u64,
    pub disk_bytes: u64,
    pub lifetime_seconds: u64,
}

#[derive(Clone, Debug)]
pub(super) struct CreateInstance<'a> {
    pub instance: &'a OwnedDevContainer,
    pub resources: ReservedResources,
    pub authority_fingerprint: &'a str,
    pub event_id: &'a str,
    pub occurred_at: i64,
}

#[derive(Clone, Debug, Eq, Error, PartialEq)]
pub(crate) enum DevContainerLedgerError {
    #[error("Dev Container ledger input is invalid")]
    InvalidInput,
    #[error("Dev Container template is unavailable")]
    TemplateUnavailable,
    #[error("Dev Container owner quota is exhausted")]
    QuotaExhausted,
    #[error("Dev Container ledger storage failed")]
    Storage,
}

pub(super) fn install_schema(connection: &Connection) -> Result<(), DevContainerLedgerError> {
    connection
        .execute_batch(DEV_CONTAINER_SCHEMA)
        .map_err(|_| DevContainerLedgerError::Storage)
}

pub(super) fn approve_template(
    connection: &Connection,
    template: &ApprovedTemplate,
    now: i64,
) -> Result<(), DevContainerLedgerError> {
    let quota = template.quota_ceiling();
    let memory_bytes = sqlite_u64(quota.memory_bytes)?;
    let disk_bytes = sqlite_u64(quota.disk_bytes)?;
    let max_lifetime_seconds = sqlite_u64(quota.max_lifetime_seconds)?;
    let host_capabilities = template
        .host_capabilities()
        .values()
        .iter()
        .map(|capability| host_capability_name(*capability))
        .collect::<Vec<_>>();
    let host_capabilities = serde_json::to_string(&host_capabilities)
        .map_err(|_| DevContainerLedgerError::InvalidInput)?;
    connection
        .execute(
            "INSERT INTO dev_container_templates(
                template_id,image_digest,max_active_instances,cpu_millis,memory_bytes,disk_bytes,
                max_lifetime_seconds,host_capabilities_json,status,policy_epoch,created_at,updated_at)
             VALUES(?1,?2,?3,?4,?5,?6,?7,?8,'approved',1,?9,?9)
             ON CONFLICT(template_id) DO UPDATE SET
                image_digest=excluded.image_digest,
                max_active_instances=excluded.max_active_instances,
                cpu_millis=excluded.cpu_millis,
                memory_bytes=excluded.memory_bytes,
                disk_bytes=excluded.disk_bytes,
                max_lifetime_seconds=excluded.max_lifetime_seconds,
                host_capabilities_json=excluded.host_capabilities_json,
                status='approved',policy_epoch=policy_epoch+1,updated_at=excluded.updated_at",
            params![
                template.id().as_str(),
                template.image().as_str(),
                quota.max_active_instances,
                quota.cpu_millis,
                memory_bytes,
                disk_bytes,
                max_lifetime_seconds,
                host_capabilities,
                now,
            ],
        )
        .map_err(|_| DevContainerLedgerError::Storage)?;
    Ok(())
}

pub(super) fn set_owner_quota(
    connection: &Connection,
    owner: &OwnerScope,
    max_active_instances: u32,
    now: i64,
) -> Result<(), DevContainerLedgerError> {
    if max_active_instances == 0 || now < 0 {
        return Err(DevContainerLedgerError::InvalidInput);
    }
    connection
        .execute(
            "INSERT INTO dev_container_owner_quotas(
                owner_kind,owner_id,max_active_instances,policy_epoch,updated_at)
             VALUES(?1,?2,?3,1,?4)
             ON CONFLICT(owner_kind,owner_id) DO UPDATE SET
                max_active_instances=excluded.max_active_instances,
                policy_epoch=policy_epoch+1,updated_at=excluded.updated_at",
            params![
                owner_kind_name(owner.kind()),
                owner.id(),
                max_active_instances,
                now
            ],
        )
        .map_err(|_| DevContainerLedgerError::Storage)?;
    Ok(())
}

pub(super) fn create_instance(
    connection: &mut Connection,
    input: &CreateInstance<'_>,
) -> Result<(), DevContainerLedgerError> {
    validate_create(input)?;
    let transaction = connection
        .transaction()
        .map_err(|_| DevContainerLedgerError::Storage)?;
    let owner = input.instance.owner();
    let owner_kind = owner_kind_name(owner.kind());
    let template = transaction
        .query_row(
            "SELECT image_digest,max_active_instances,cpu_millis,memory_bytes,disk_bytes,
                    max_lifetime_seconds,status
             FROM dev_container_templates WHERE template_id=?1",
            [input.instance.template_id().as_str()],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, u32>(1)?,
                    row.get::<_, u32>(2)?,
                    row.get::<_, i64>(3)?,
                    row.get::<_, i64>(4)?,
                    row.get::<_, i64>(5)?,
                    row.get::<_, String>(6)?,
                ))
            },
        )
        .optional()
        .map_err(|_| DevContainerLedgerError::Storage)?
        .filter(|template| template.6 == "approved")
        .ok_or(DevContainerLedgerError::TemplateUnavailable)?;
    if template.0 != input.instance.image().as_str()
        || input.resources.cpu_millis > template.2
        || sqlite_u64(input.resources.memory_bytes)? > template.3
        || sqlite_u64(input.resources.disk_bytes)? > template.4
        || sqlite_u64(input.resources.lifetime_seconds)? > template.5
    {
        return Err(DevContainerLedgerError::TemplateUnavailable);
    }
    let owner_max_active = transaction
        .query_row(
            "SELECT max_active_instances FROM dev_container_owner_quotas
             WHERE owner_kind=?1 AND owner_id=?2",
            params![owner_kind, owner.id()],
            |row| row.get::<_, u32>(0),
        )
        .optional()
        .map_err(|_| DevContainerLedgerError::Storage)?
        .ok_or(DevContainerLedgerError::QuotaExhausted)?;
    let active = transaction
        .query_row(
            "SELECT count(*) FROM dev_container_instances
             WHERE owner_kind=?1 AND owner_id=?2
               AND observed_state IN ('pending','starting','running','stopping')",
            params![owner_kind, owner.id()],
            |row| row.get::<_, u32>(0),
        )
        .map_err(|_| DevContainerLedgerError::Storage)?;
    if active >= template.1.min(owner_max_active) {
        return Err(DevContainerLedgerError::QuotaExhausted);
    }
    let secret_references = input
        .instance
        .secret_references()
        .iter()
        .map(SecretReference::as_str)
        .collect::<Vec<_>>();
    let secret_references = serde_json::to_string(&secret_references)
        .map_err(|_| DevContainerLedgerError::InvalidInput)?;
    let memory_bytes = sqlite_u64(input.resources.memory_bytes)?;
    let disk_bytes = sqlite_u64(input.resources.disk_bytes)?;
    let lifetime_seconds = sqlite_u64(input.resources.lifetime_seconds)?;
    transaction
        .execute(
            "INSERT INTO dev_container_instances(
                instance_id,owner_kind,owner_id,template_id,image_digest,lifecycle_nonce,
                desired_state,observed_state,cpu_millis,memory_bytes,disk_bytes,lifetime_seconds,
                secret_references_json,authority_fingerprint,revision,created_at,updated_at,deleted_at)
             VALUES(?1,?2,?3,?4,?5,?6,'running','pending',?7,?8,?9,?10,?11,?12,1,?13,?13,NULL)",
            params![
                input.instance.id().as_str(),
                owner_kind,
                owner.id(),
                input.instance.template_id().as_str(),
                input.instance.image().as_str(),
                input.instance.lifecycle_nonce().as_str(),
                input.resources.cpu_millis,
                memory_bytes,
                disk_bytes,
                lifetime_seconds,
                secret_references,
                input.authority_fingerprint,
                input.occurred_at,
            ],
        )
        .map_err(|_| DevContainerLedgerError::Storage)?;
    transaction
        .execute(
            "INSERT INTO dev_container_ledger(
                event_id,instance_id,lifecycle_nonce,revision,event_kind,occurred_at,detail_json)
             VALUES(?1,?2,?3,1,'created',?4,'{}')",
            params![
                input.event_id,
                input.instance.id().as_str(),
                input.instance.lifecycle_nonce().as_str(),
                input.occurred_at,
            ],
        )
        .map_err(|_| DevContainerLedgerError::Storage)?;
    transaction
        .commit()
        .map_err(|_| DevContainerLedgerError::Storage)
}

fn validate_create(input: &CreateInstance<'_>) -> Result<(), DevContainerLedgerError> {
    if input.authority_fingerprint.trim().is_empty()
        || input.event_id.trim().is_empty()
        || input.occurred_at < 0
        || input.resources.cpu_millis == 0
        || input.resources.memory_bytes == 0
        || input.resources.disk_bytes == 0
        || input.resources.lifetime_seconds == 0
        || input.instance.desired_state() != DesiredState::Running
        || input.instance.observed_state() != ObservedState::Pending
    {
        return Err(DevContainerLedgerError::InvalidInput);
    }
    Ok(())
}

fn sqlite_u64(value: u64) -> Result<i64, DevContainerLedgerError> {
    i64::try_from(value).map_err(|_| DevContainerLedgerError::InvalidInput)
}

fn owner_kind_name(kind: OwnerKind) -> &'static str {
    match kind {
        OwnerKind::Installation => "installation",
        OwnerKind::Team => "team",
        OwnerKind::Project => "project",
        OwnerKind::Personal => "personal",
    }
}

fn host_capability_name(capability: HostCapability) -> &'static str {
    match capability {
        HostCapability::Privileged => "privileged",
        HostCapability::HostFilesystem => "host_filesystem",
        HostCapability::ContainerRuntimeSocket => "container_runtime_socket",
        HostCapability::HostNetwork => "host_network",
        HostCapability::HostDevice => "host_device",
        HostCapability::KernelAdministration => "kernel_administration",
    }
}

fn parse_host_capability(value: &str) -> Option<HostCapability> {
    match value {
        "privileged" => Some(HostCapability::Privileged),
        "host_filesystem" => Some(HostCapability::HostFilesystem),
        "container_runtime_socket" => Some(HostCapability::ContainerRuntimeSocket),
        "host_network" => Some(HostCapability::HostNetwork),
        "host_device" => Some(HostCapability::HostDevice),
        "kernel_administration" => Some(HostCapability::KernelAdministration),
        _ => None,
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct RecoveryRecord {
    pub instance_id: String,
    pub owner_kind: OwnerKind,
    pub owner_id: String,
    pub lifecycle_nonce: String,
    pub desired_state: DesiredState,
    pub observed_state: ObservedState,
}

pub(crate) struct CreatedRuntimeSpec {
    pub template: ApprovedTemplate,
    pub instance: OwnedDevContainer,
    pub resources: ReservedResources,
}

#[allow(clippy::too_many_arguments)]
pub(crate) async fn create_approved_for_store(
    store: &super::AccessStore,
    owner: OwnerScope,
    instance_id: String,
    template_id: String,
    secret_references: Vec<String>,
    authority_fingerprint: String,
    event_id: String,
    now: i64,
) -> Result<CreatedRuntimeSpec, DevContainerLedgerError> {
    store.with_connection(move |connection| {
        let row=connection.query_row("SELECT image_digest,max_active_instances,cpu_millis,memory_bytes,disk_bytes,max_lifetime_seconds,host_capabilities_json,status FROM dev_container_templates WHERE template_id=?1",[&template_id],|r|Ok((r.get::<_,String>(0)?,r.get::<_,u32>(1)?,r.get::<_,u32>(2)?,r.get::<_,i64>(3)?,r.get::<_,i64>(4)?,r.get::<_,i64>(5)?,r.get::<_,String>(6)?,r.get::<_,String>(7)?))).optional().map_err(super::store::map_sqlite_error)?.filter(|r|r.7=="approved").ok_or(super::AccessStoreError::NotAuthorized)?;
        let caps=serde_json::from_str::<Vec<String>>(&row.6).map_err(|_|super::AccessStoreError::MalformedVocabulary)?.into_iter().map(|v|parse_host_capability(&v).ok_or(super::AccessStoreError::MalformedVocabulary)).collect::<Result<Vec<_>,_>>()?;
        let template=ApprovedTemplate::new(labby_primitives::dev_container::DevContainerTemplateId::new(template_id).map_err(|_|super::AccessStoreError::MalformedVocabulary)?,labby_primitives::dev_container::ImageDigest::new(row.0).map_err(|_|super::AccessStoreError::MalformedVocabulary)?,labby_primitives::dev_container::DevContainerQuota{max_active_instances:row.1,cpu_millis:row.2,memory_bytes:u64::try_from(row.3).map_err(|_|super::AccessStoreError::MalformedVocabulary)?,disk_bytes:u64::try_from(row.4).map_err(|_|super::AccessStoreError::MalformedVocabulary)?,max_lifetime_seconds:u64::try_from(row.5).map_err(|_|super::AccessStoreError::MalformedVocabulary)?},labby_primitives::dev_container::HostCapabilityPolicy::approved(caps)).map_err(|_|super::AccessStoreError::MalformedVocabulary)?;
        let instance=OwnedDevContainer::new(labby_primitives::dev_container::DevContainerId::new(instance_id).map_err(|_|super::AccessStoreError::MalformedVocabulary)?,owner,&template,labby_primitives::dev_container::LifecycleNonce::new(uuid::Uuid::new_v4().simple().to_string()).map_err(|_|super::AccessStoreError::MalformedVocabulary)?,secret_references.into_iter().map(SecretReference::new).collect::<Result<Vec<_>,_>>().map_err(|_|super::AccessStoreError::MalformedVocabulary)?).map_err(|_|super::AccessStoreError::MalformedVocabulary)?;
        let q=template.quota_ceiling(); let resources=ReservedResources{cpu_millis:q.cpu_millis,memory_bytes:q.memory_bytes,disk_bytes:q.disk_bytes,lifetime_seconds:q.max_lifetime_seconds};
        create_instance(connection,&CreateInstance{instance:&instance,resources,authority_fingerprint:&authority_fingerprint,event_id:&event_id,occurred_at:now}).map_err(|_|super::AccessStoreError::Unavailable("Dev Container creation failed".into()))?;
        Ok(CreatedRuntimeSpec{template,instance,resources})
    }).await.map_err(|_|DevContainerLedgerError::Storage)
}

pub(crate) fn recovery_inventory(
    connection: &Connection,
) -> Result<Vec<RecoveryRecord>, DevContainerLedgerError> {
    let mut statement = connection
        .prepare(
            "SELECT instance_id,owner_kind,owner_id,lifecycle_nonce,desired_state,observed_state
             FROM dev_container_instances
             WHERE observed_state != 'deleted' ORDER BY instance_id",
        )
        .map_err(|_| DevContainerLedgerError::Storage)?;
    statement
        .query_map([], |row| {
            let owner_kind = owner_kind(&row.get::<_, String>(1)?)?;
            let desired = desired_state(&row.get::<_, String>(4)?)?;
            let observed = observed_state(&row.get::<_, String>(5)?)?;
            Ok(RecoveryRecord {
                instance_id: row.get(0)?,
                owner_kind,
                owner_id: row.get(2)?,
                lifecycle_nonce: row.get(3)?,
                desired_state: desired,
                observed_state: observed,
            })
        })
        .map_err(|_| DevContainerLedgerError::Storage)?
        .collect::<rusqlite::Result<Vec<_>>>()
        .map_err(|_| DevContainerLedgerError::Storage)
}

pub(crate) fn recovery_inventory_page(
    connection: &Connection,
    after: &str,
    limit: usize,
) -> Result<Vec<RecoveryRecord>, DevContainerLedgerError> {
    if limit == 0 || limit > 100 {
        return Err(DevContainerLedgerError::InvalidInput);
    }
    let mut statement = connection
        .prepare(
            "SELECT instance_id,owner_kind,owner_id,lifecycle_nonce,desired_state,observed_state
         FROM dev_container_instances WHERE observed_state != 'deleted' AND instance_id>?1
         ORDER BY instance_id LIMIT ?2",
        )
        .map_err(|_| DevContainerLedgerError::Storage)?;
    statement
        .query_map(
            params![
                after,
                i64::try_from(limit).map_err(|_| DevContainerLedgerError::InvalidInput)?
            ],
            |row| {
                Ok(RecoveryRecord {
                    instance_id: row.get(0)?,
                    owner_kind: owner_kind(&row.get::<_, String>(1)?)?,
                    owner_id: row.get(2)?,
                    lifecycle_nonce: row.get(3)?,
                    desired_state: desired_state(&row.get::<_, String>(4)?)?,
                    observed_state: observed_state(&row.get::<_, String>(5)?)?,
                })
            },
        )
        .map_err(|_| DevContainerLedgerError::Storage)?
        .collect::<rusqlite::Result<Vec<_>>>()
        .map_err(|_| DevContainerLedgerError::Storage)
}

pub(super) fn authorized_recovery_inventory_page(
    connection: &Connection,
    after: &str,
    limit: usize,
    principal_id: &str,
    platform_admin: bool,
) -> Result<Vec<RecoveryRecord>, DevContainerLedgerError> {
    if limit == 0 || limit > 100 {
        return Err(DevContainerLedgerError::InvalidInput);
    }
    let mut statement = connection.prepare("WITH authorized_owners(owner_kind,owner_id) AS (SELECT 'personal',?3 UNION SELECT 'team',g.group_id FROM groups g JOIN team_memberships tm ON tm.organization_id=g.organization_id AND tm.team_id=g.group_id WHERE tm.principal_id=?3 AND tm.status='active' AND g.kind='team' AND g.status='active' UNION SELECT 'project',p.project_id FROM projects p JOIN project_memberships pm ON pm.organization_id=p.organization_id AND pm.project_id=p.project_id WHERE pm.principal_id=?3 AND pm.status='active' AND p.status='active' UNION SELECT 'personal',p.principal_id FROM principals p WHERE ?4 AND p.status='active' UNION SELECT 'team',g.group_id FROM groups g WHERE ?4 AND g.kind='team' AND g.status='active' UNION SELECT 'project',p.project_id FROM projects p WHERE ?4 AND p.status='active'), visible AS (SELECT d.* FROM dev_container_instances d JOIN authorized_owners a USING(owner_kind,owner_id) WHERE d.observed_state!='deleted' UNION ALL SELECT d.* FROM dev_container_instances d WHERE ?4 AND d.owner_kind='installation' AND d.observed_state!='deleted') SELECT instance_id,owner_kind,owner_id,lifecycle_nonce,desired_state,observed_state FROM visible WHERE instance_id>?1 ORDER BY instance_id LIMIT ?2").map_err(|_| DevContainerLedgerError::Storage)?;
    statement
        .query_map(
            params![
                after,
                i64::try_from(limit).map_err(|_| DevContainerLedgerError::InvalidInput)?,
                principal_id,
                platform_admin
            ],
            |row| {
                Ok(RecoveryRecord {
                    instance_id: row.get(0)?,
                    owner_kind: owner_kind(&row.get::<_, String>(1)?)?,
                    owner_id: row.get(2)?,
                    lifecycle_nonce: row.get(3)?,
                    desired_state: desired_state(&row.get::<_, String>(4)?)?,
                    observed_state: observed_state(&row.get::<_, String>(5)?)?,
                })
            },
        )
        .map_err(|_| DevContainerLedgerError::Storage)?
        .collect::<rusqlite::Result<Vec<_>>>()
        .map_err(|_| DevContainerLedgerError::Storage)
}

pub(crate) async fn recovery_inventory_for_store(
    store: &super::AccessStore,
) -> Result<Vec<RecoveryRecord>, DevContainerLedgerError> {
    store
        .with_connection(|connection| {
            recovery_inventory(connection).map_err(|_| {
                super::AccessStoreError::Unavailable("Dev Container persistence unavailable".into())
            })
        })
        .await
        .map_err(|_| DevContainerLedgerError::Storage)
}

pub(crate) async fn recovery_inventory_page_for_store(
    store: &super::AccessStore,
    after: String,
    limit: usize,
) -> Result<Vec<RecoveryRecord>, DevContainerLedgerError> {
    store
        .with_connection(move |connection| {
            recovery_inventory_page(connection, &after, limit).map_err(|_| {
                super::AccessStoreError::Unavailable("Dev Container persistence unavailable".into())
            })
        })
        .await
        .map_err(|_| DevContainerLedgerError::Storage)
}

pub(crate) async fn set_desired_for_store(
    store: &super::AccessStore,
    instance_id: String,
    nonce: String,
    desired: DesiredState,
    event_id: String,
    now: i64,
) -> Result<(), DevContainerLedgerError> {
    store
        .with_connection(move |connection| {
            set_desired_state(connection, &instance_id, &nonce, desired, &event_id, now).map_err(
                |_| {
                    super::AccessStoreError::Unavailable(
                        "Dev Container persistence unavailable".into(),
                    )
                },
            )
        })
        .await
        .map_err(|_| DevContainerLedgerError::Storage)
}

pub(crate) fn set_desired_state(
    connection: &mut Connection,
    instance_id: &str,
    lifecycle_nonce: &str,
    desired: DesiredState,
    event_id: &str,
    now: i64,
) -> Result<(), DevContainerLedgerError> {
    if event_id.trim().is_empty() || now < 0 {
        return Err(DevContainerLedgerError::InvalidInput);
    }
    let transaction = connection
        .transaction()
        .map_err(|_| DevContainerLedgerError::Storage)?;
    let revision = transaction
        .query_row(
            "SELECT revision FROM dev_container_instances WHERE instance_id=?1 AND lifecycle_nonce=?2",
            params![instance_id, lifecycle_nonce],
            |row| row.get::<_, i64>(0),
        )
        .optional()
        .map_err(|_| DevContainerLedgerError::Storage)?
        .ok_or(DevContainerLedgerError::InvalidInput)?
        .checked_add(1)
        .ok_or(DevContainerLedgerError::InvalidInput)?;
    let desired = desired_state_name(desired);
    transaction
        .execute(
            "UPDATE dev_container_instances SET desired_state=?1,revision=?2,updated_at=?3,
             deleted_at=CASE WHEN ?1='deleted' THEN ?3 ELSE NULL END
             WHERE instance_id=?4 AND lifecycle_nonce=?5",
            params![desired, revision, now, instance_id, lifecycle_nonce],
        )
        .map_err(|_| DevContainerLedgerError::Storage)?;
    transaction
        .execute(
            "INSERT INTO dev_container_ledger VALUES(?1,?2,?3,?4,'desired_changed',?5,?6)",
            params![
                event_id,
                instance_id,
                lifecycle_nonce,
                revision,
                now,
                format!("{{\"desired_state\":\"{desired}\"}}")
            ],
        )
        .map_err(|_| DevContainerLedgerError::Storage)?;
    transaction
        .commit()
        .map_err(|_| DevContainerLedgerError::Storage)
}

pub(crate) fn record_observation(
    connection: &mut Connection,
    instance_id: &str,
    nonce: &labby_primitives::dev_container::LifecycleNonce,
    next: ObservedState,
    event_id: &str,
    now: i64,
) -> Result<(), DevContainerLedgerError> {
    let transaction = connection
        .transaction()
        .map_err(|_| DevContainerLedgerError::Storage)?;
    let (desired, prior, revision, durable_nonce) = transaction
        .query_row(
            "SELECT desired_state,observed_state,revision,lifecycle_nonce FROM dev_container_instances WHERE instance_id=?1",
            [instance_id],
            |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?, row.get::<_, i64>(2)?, row.get::<_, String>(3)?)),
        )
        .optional()
        .map_err(|_| DevContainerLedgerError::Storage)?
        .ok_or(DevContainerLedgerError::InvalidInput)?;
    let durable_nonce = labby_primitives::dev_container::LifecycleNonce::new(durable_nonce)
        .map_err(|_| DevContainerLedgerError::Storage)?;
    labby_runtime::dev_container::validate_observation(
        &durable_nonce,
        nonce,
        desired_state(&desired).map_err(|_| DevContainerLedgerError::Storage)?,
        observed_state(&prior).map_err(|_| DevContainerLedgerError::Storage)?,
        next,
    )
    .map_err(|_| DevContainerLedgerError::InvalidInput)?;
    let revision = revision
        .checked_add(1)
        .ok_or(DevContainerLedgerError::InvalidInput)?;
    let next = observed_state_name(next);
    transaction.execute("UPDATE dev_container_instances SET observed_state=?1,revision=?2,updated_at=?3 WHERE instance_id=?4", params![next,revision,now,instance_id]).map_err(|_| DevContainerLedgerError::Storage)?;
    transaction
        .execute(
            "INSERT INTO dev_container_ledger VALUES(?1,?2,?3,?4,'observed_changed',?5,?6)",
            params![
                event_id,
                instance_id,
                nonce.as_str(),
                revision,
                now,
                format!("{{\"observed_state\":\"{next}\"}}")
            ],
        )
        .map_err(|_| DevContainerLedgerError::Storage)?;
    transaction
        .commit()
        .map_err(|_| DevContainerLedgerError::Storage)
}

fn desired_state(value: &str) -> rusqlite::Result<DesiredState> {
    match value {
        "running" => Ok(DesiredState::Running),
        "stopped" => Ok(DesiredState::Stopped),
        "deleted" => Ok(DesiredState::Deleted),
        _ => Err(rusqlite::Error::InvalidQuery),
    }
}
fn owner_kind(value: &str) -> rusqlite::Result<OwnerKind> {
    match value {
        "installation" => Ok(OwnerKind::Installation),
        "team" => Ok(OwnerKind::Team),
        "project" => Ok(OwnerKind::Project),
        "personal" => Ok(OwnerKind::Personal),
        _ => Err(rusqlite::Error::InvalidQuery),
    }
}
fn observed_state(value: &str) -> rusqlite::Result<ObservedState> {
    match value {
        "pending" => Ok(ObservedState::Pending),
        "starting" => Ok(ObservedState::Starting),
        "running" => Ok(ObservedState::Running),
        "stopping" => Ok(ObservedState::Stopping),
        "stopped" => Ok(ObservedState::Stopped),
        "failed" => Ok(ObservedState::Failed),
        "deleted" => Ok(ObservedState::Deleted),
        _ => Err(rusqlite::Error::InvalidQuery),
    }
}
fn desired_state_name(value: DesiredState) -> &'static str {
    match value {
        DesiredState::Running => "running",
        DesiredState::Stopped => "stopped",
        DesiredState::Deleted => "deleted",
    }
}
fn observed_state_name(value: ObservedState) -> &'static str {
    match value {
        ObservedState::Pending => "pending",
        ObservedState::Starting => "starting",
        ObservedState::Running => "running",
        ObservedState::Stopping => "stopping",
        ObservedState::Stopped => "stopped",
        ObservedState::Failed => "failed",
        ObservedState::Deleted => "deleted",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use labby_primitives::access::PrincipalId;
    use labby_primitives::dev_container::{
        DevContainerId, DevContainerQuota, DevContainerTemplateId, HostCapabilityPolicy,
        ImageDigest, LifecycleNonce,
    };

    fn fixture() -> (ApprovedTemplate, OwnedDevContainer) {
        let template = ApprovedTemplate::new(
            DevContainerTemplateId::new("rust").unwrap(),
            ImageDigest::new(format!("sha256:{}", "a".repeat(64))).unwrap(),
            DevContainerQuota {
                max_active_instances: 1,
                cpu_millis: 1_000,
                memory_bytes: 2_000,
                disk_bytes: 3_000,
                max_lifetime_seconds: 60,
            },
            HostCapabilityPolicy::deny_all(),
        )
        .unwrap();
        let instance = OwnedDevContainer::new(
            DevContainerId::new("dc-1").unwrap(),
            OwnerScope::Personal(PrincipalId::new("principal-1").unwrap()),
            &template,
            LifecycleNonce::new("11111111111111111111111111111111").unwrap(),
            vec![SecretReference::new("secret-ref").unwrap()],
        )
        .unwrap();
        (template, instance)
    }

    #[test]
    fn create_is_atomic_and_owner_quota_is_durable() {
        let mut connection = Connection::open_in_memory().unwrap();
        connection.execute_batch("PRAGMA foreign_keys=ON;").unwrap();
        install_schema(&connection).unwrap();
        let (template, first) = fixture();
        approve_template(&connection, &template, 1).unwrap();
        set_owner_quota(&connection, first.owner(), 1, 1).unwrap();
        let resources = ReservedResources {
            cpu_millis: 500,
            memory_bytes: 1_000,
            disk_bytes: 2_000,
            lifetime_seconds: 30,
        };
        create_instance(
            &mut connection,
            &CreateInstance {
                instance: &first,
                resources,
                authority_fingerprint: "sha256:authority",
                event_id: "event-1",
                occurred_at: 2,
            },
        )
        .unwrap();
        assert_eq!(
            connection
                .query_row("SELECT count(*) FROM dev_container_ledger", [], |row| row
                    .get::<_, u32>(
                    0
                ))
                .unwrap(),
            1
        );
        assert_eq!(
            recovery_inventory(&connection).unwrap(),
            vec![RecoveryRecord {
                instance_id: "dc-1".into(),
                owner_kind: OwnerKind::Personal,
                owner_id: "principal-1".into(),
                lifecycle_nonce: "11111111111111111111111111111111".into(),
                desired_state: DesiredState::Running,
                observed_state: ObservedState::Pending,
            }]
        );
        assert_eq!(
            record_observation(
                &mut connection,
                "dc-1",
                &LifecycleNonce::new("99999999999999999999999999999999").unwrap(),
                ObservedState::Starting,
                "stale-event",
                3,
            ),
            Err(DevContainerLedgerError::InvalidInput)
        );
        set_desired_state(
            &mut connection,
            "dc-1",
            first.lifecycle_nonce().as_str(),
            DesiredState::Deleted,
            "delete-event",
            3,
        )
        .unwrap();

        let second = OwnedDevContainer::new(
            DevContainerId::new("dc-2").unwrap(),
            first.owner().clone(),
            &template,
            LifecycleNonce::new("22222222222222222222222222222222").unwrap(),
            vec![],
        )
        .unwrap();
        assert_eq!(
            create_instance(
                &mut connection,
                &CreateInstance {
                    instance: &second,
                    resources,
                    authority_fingerprint: "sha256:authority",
                    event_id: "event-2",
                    occurred_at: 3,
                }
            ),
            Err(DevContainerLedgerError::QuotaExhausted)
        );
        assert_eq!(
            connection
                .query_row("SELECT count(*) FROM dev_container_instances", [], |row| {
                    row.get::<_, u32>(0)
                })
                .unwrap(),
            1
        );
    }
}
