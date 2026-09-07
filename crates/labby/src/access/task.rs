//! Durable, fenced Agent Task intent and settlement records.

use super::error::{AccessStoreError, AccessStoreResult};
use labby_primitives::access::OwnerScope;
use labby_primitives::task::{TaskIntent, TaskSettlement, TaskState, validate_intent};
use rusqlite::{Connection, OptionalExtension, params};
use std::path::Path;

pub(crate) struct TaskStore {
    connection: Connection,
}

#[derive(Clone, Debug)]
pub(crate) struct TaskRecord {
    pub intent: TaskIntent,
    pub state: TaskState,
    pub attempt: u32,
    pub output_digest: Option<String>,
    pub error_code: Option<String>,
}

impl TaskStore {
    pub(crate) fn open(path: &Path) -> AccessStoreResult<Self> {
        let connection = Connection::open(path).map_err(super::store::map_sqlite_error)?;
        connection.execute_batch("PRAGMA foreign_keys=ON; CREATE TABLE IF NOT EXISTS agent_tasks(task_id TEXT PRIMARY KEY,idempotency_key TEXT NOT NULL,owner_kind TEXT NOT NULL CHECK(owner_kind IN ('installation','team','project','personal')),owner_id TEXT NOT NULL,project_id TEXT,creator_principal_id TEXT NOT NULL,agent_id TEXT NOT NULL,agent_version INTEGER NOT NULL CHECK(agent_version>0),agent_revision_digest TEXT NOT NULL,input_digest TEXT NOT NULL,catalog_generation TEXT NOT NULL,authority_fingerprint TEXT NOT NULL,state TEXT NOT NULL CHECK(state IN ('created','queued','running','cancelling','succeeded','failed','cancelled','expired')),attempt INTEGER NOT NULL DEFAULT 0 CHECK(attempt>=0),fencing_token TEXT,lease_expires_at INTEGER,output_digest TEXT,error_code TEXT,created_at INTEGER NOT NULL,updated_at INTEGER NOT NULL,UNIQUE(owner_kind,owner_id,idempotency_key)); CREATE INDEX IF NOT EXISTS agent_tasks_owner_state ON agent_tasks(owner_kind,owner_id,state,task_id); CREATE TABLE IF NOT EXISTS agent_task_audit(event_id TEXT PRIMARY KEY,task_id TEXT NOT NULL,actor_principal_id TEXT NOT NULL,from_state TEXT,to_state TEXT NOT NULL,attempt INTEGER NOT NULL,occurred_at INTEGER NOT NULL,FOREIGN KEY(task_id) REFERENCES agent_tasks(task_id) ON DELETE CASCADE);").map_err(super::store::map_sqlite_error)?;
        Ok(Self { connection })
    }

    pub(crate) fn create(&mut self, intent: &TaskIntent, now: i64) -> AccessStoreResult<String> {
        let tx = self
            .connection
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
            .map_err(super::store::map_sqlite_error)?;
        let id = Self::create_in_transaction(&tx, intent, now)?;
        tx.commit().map_err(super::store::map_sqlite_error)?;
        Ok(id)
    }

    pub(super) fn create_in_transaction(
        tx: &rusqlite::Transaction<'_>,
        intent: &TaskIntent,
        now: i64,
    ) -> AccessStoreResult<String> {
        if !validate_intent(intent) {
            return Err(AccessStoreError::MalformedVocabulary);
        }
        let (kind, owner) = owner(&intent.owner);
        if let Some((id,input,agent))=tx.query_row("SELECT task_id,input_digest,agent_revision_digest FROM agent_tasks WHERE owner_kind=?1 AND owner_id=?2 AND idempotency_key=?3",params![kind,owner,intent.idempotency_key],|r|Ok((r.get::<_,String>(0)?,r.get::<_,String>(1)?,r.get::<_,String>(2)?))).optional().map_err(super::store::map_sqlite_error)? { if input==intent.input_digest && agent==intent.agent_revision_digest{return Ok(id)} return Err(AccessStoreError::IntegrityViolation{check:"task_idempotency"}) }
        tx.execute("INSERT INTO agent_tasks VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,'created',0,NULL,NULL,NULL,NULL,?13,?13)",params![intent.id,intent.idempotency_key,kind,owner,intent.project.as_ref().map(|p|p.as_str()),intent.creator.as_str(),intent.agent_id,i64::try_from(intent.agent_version).map_err(|_|AccessStoreError::MalformedVocabulary)?,intent.agent_revision_digest,intent.input_digest,intent.catalog_generation,intent.authority_fingerprint,now]).map_err(super::store::map_sqlite_error)?;
        tx.execute(
            "INSERT INTO agent_task_audit VALUES(?1,?2,?3,NULL,'created',0,?4)",
            params![
                format!("task-{}-create", intent.id),
                intent.id,
                intent.creator.as_str(),
                now
            ],
        )
        .map_err(super::store::map_sqlite_error)?;
        Ok(intent.id.clone())
    }

    pub(crate) fn get(&self, id: &str) -> AccessStoreResult<Option<TaskRecord>> {
        self.connection
            .query_row(
                "SELECT task_id,idempotency_key,owner_kind,owner_id,project_id,creator_principal_id,agent_id,agent_version,agent_revision_digest,input_digest,catalog_generation,authority_fingerprint,state,attempt,output_digest,error_code FROM agent_tasks WHERE task_id=?1",
                [id],
                decode,
            )
            .optional()
            .map_err(super::store::map_sqlite_error)
    }

    pub(crate) fn list_page(
        &self,
        after: &str,
        limit: usize,
    ) -> AccessStoreResult<Vec<TaskRecord>> {
        if limit == 0 || limit > 100 {
            return Err(AccessStoreError::MalformedVocabulary);
        }
        let mut statement = self.connection.prepare("SELECT task_id,idempotency_key,owner_kind,owner_id,project_id,creator_principal_id,agent_id,agent_version,agent_revision_digest,input_digest,catalog_generation,authority_fingerprint,state,attempt,output_digest,error_code FROM agent_tasks WHERE task_id>?1 ORDER BY task_id LIMIT ?2").map_err(super::store::map_sqlite_error)?;
        statement
            .query_map(
                params![
                    after,
                    i64::try_from(limit).map_err(|_| AccessStoreError::MalformedVocabulary)?
                ],
                decode,
            )
            .map_err(super::store::map_sqlite_error)?
            .collect::<rusqlite::Result<Vec<_>>>()
            .map_err(super::store::map_sqlite_error)
    }

    pub(crate) fn transition(
        &mut self,
        id: &str,
        from: TaskState,
        to: TaskState,
        actor: &str,
        attempt: u32,
        fence: Option<&str>,
        settlement: Option<&TaskSettlement>,
        now: i64,
    ) -> AccessStoreResult<()> {
        if !from.permits(to) {
            return Err(AccessStoreError::MalformedVocabulary);
        }
        let tx = self
            .connection
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
            .map_err(super::store::map_sqlite_error)?;
        Self::transition_in_transaction(&tx, id, from, to, actor, attempt, fence, settlement, now)?;
        tx.commit().map_err(super::store::map_sqlite_error)
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) fn transition_in_transaction(
        tx: &rusqlite::Transaction<'_>,
        id: &str,
        from: TaskState,
        to: TaskState,
        actor: &str,
        attempt: u32,
        fence: Option<&str>,
        settlement: Option<&TaskSettlement>,
        now: i64,
    ) -> AccessStoreResult<()> {
        if !from.permits(to) {
            return Err(AccessStoreError::MalformedVocabulary);
        }
        let changed=tx.execute("UPDATE agent_tasks SET state=?1,attempt=?2,output_digest=?3,error_code=?4,updated_at=?5 WHERE task_id=?6 AND state=?7 AND attempt<=?2 AND (?8 IS NULL OR fencing_token=?8)",params![to.wire(),attempt,settlement.and_then(|s|s.output_digest.as_deref()),settlement.and_then(|s|s.error_code.as_deref()),now,id,from.wire(),fence]).map_err(super::store::map_sqlite_error)?;
        if changed != 1 {
            return Err(AccessStoreError::IntegrityViolation {
                check: "task_transition_fence",
            });
        }
        tx.execute(
            "INSERT INTO agent_task_audit VALUES(?1,?2,?3,?4,?5,?6,?7)",
            params![
                format!("task-{id}-{}-{attempt}", to.wire()),
                id,
                actor,
                from.wire(),
                to.wire(),
                attempt,
                now
            ],
        )
        .map_err(super::store::map_sqlite_error)?;
        Ok(())
    }

    pub(crate) fn acquire_lease(
        &mut self,
        id: &str,
        attempt: u32,
        fencing_token: &str,
        expires_at: i64,
        now: i64,
    ) -> AccessStoreResult<()> {
        if fencing_token.len() < 32 || expires_at <= now {
            return Err(AccessStoreError::MalformedVocabulary);
        }
        let changed=self.connection.execute("UPDATE agent_tasks SET fencing_token=?1,lease_expires_at=?2,attempt=?3,updated_at=?4 WHERE task_id=?5 AND state='queued' AND attempt<?3",params![fencing_token,expires_at,attempt,now,id]).map_err(super::store::map_sqlite_error)?;
        if changed == 1 {
            Ok(())
        } else {
            Err(AccessStoreError::IntegrityViolation {
                check: "task_lease_fence",
            })
        }
    }

    pub(crate) fn recover_expired(&mut self, now: i64) -> AccessStoreResult<usize> {
        self.connection.execute("UPDATE agent_tasks SET state='expired',updated_at=?1 WHERE state IN ('running','cancelling') AND lease_expires_at IS NOT NULL AND lease_expires_at<=?1",[now]).map_err(super::store::map_sqlite_error)
    }

    pub(crate) fn purge_terminal_before(
        &mut self,
        cutoff: i64,
        limit: usize,
    ) -> AccessStoreResult<usize> {
        if limit == 0 || limit > 1000 {
            return Err(AccessStoreError::MalformedVocabulary);
        }
        self.connection.execute("DELETE FROM agent_tasks WHERE task_id IN (SELECT task_id FROM agent_tasks WHERE state IN ('succeeded','failed','cancelled','expired') AND updated_at<?1 ORDER BY updated_at,task_id LIMIT ?2)",params![cutoff,i64::try_from(limit).map_err(|_|AccessStoreError::MalformedVocabulary)?]).map_err(super::store::map_sqlite_error)
    }
}

pub(super) fn decode(row: &rusqlite::Row<'_>) -> rusqlite::Result<TaskRecord> {
    use labby_primitives::access::{InstallationId, PrincipalId, ProjectId, TeamId};
    let owner_id: String = row.get(3)?;
    let owner = match row.get::<_, String>(2)?.as_str() {
        "installation" => OwnerScope::Installation(
            InstallationId::new(owner_id).map_err(|_| rusqlite::Error::InvalidQuery)?,
        ),
        "team" => {
            OwnerScope::Team(TeamId::new(owner_id).map_err(|_| rusqlite::Error::InvalidQuery)?)
        }
        "project" => OwnerScope::Project(
            ProjectId::new(owner_id).map_err(|_| rusqlite::Error::InvalidQuery)?,
        ),
        "personal" => OwnerScope::Personal(
            PrincipalId::new(owner_id).map_err(|_| rusqlite::Error::InvalidQuery)?,
        ),
        _ => return Err(rusqlite::Error::InvalidQuery),
    };
    let state = match row.get::<_, String>(12)?.as_str() {
        "created" => TaskState::Created,
        "queued" => TaskState::Queued,
        "running" => TaskState::Running,
        "cancelling" => TaskState::Cancelling,
        "succeeded" => TaskState::Succeeded,
        "failed" => TaskState::Failed,
        "cancelled" => TaskState::Cancelled,
        "expired" => TaskState::Expired,
        _ => return Err(rusqlite::Error::InvalidQuery),
    };
    Ok(TaskRecord {
        intent: TaskIntent {
            id: row.get(0)?,
            idempotency_key: row.get(1)?,
            owner,
            project: row
                .get::<_, Option<String>>(4)?
                .map(|v| ProjectId::new(v).map_err(|_| rusqlite::Error::InvalidQuery))
                .transpose()?,
            creator: PrincipalId::new(row.get::<_, String>(5)?)
                .map_err(|_| rusqlite::Error::InvalidQuery)?,
            agent_id: row.get(6)?,
            agent_version: u64::try_from(row.get::<_, i64>(7)?)
                .map_err(|_| rusqlite::Error::InvalidQuery)?,
            agent_revision_digest: row.get(8)?,
            input_digest: row.get(9)?,
            catalog_generation: row.get(10)?,
            authority_fingerprint: row.get(11)?,
        },
        state,
        attempt: u32::try_from(row.get::<_, i64>(13)?)
            .map_err(|_| rusqlite::Error::InvalidQuery)?,
        output_digest: row.get(14)?,
        error_code: row.get(15)?,
    })
}
fn owner(v: &OwnerScope) -> (&'static str, &str) {
    match v {
        OwnerScope::Installation(x) => ("installation", x.as_str()),
        OwnerScope::Team(x) => ("team", x.as_str()),
        OwnerScope::Project(x) => ("project", x.as_str()),
        OwnerScope::Personal(x) => ("personal", x.as_str()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use labby_primitives::access::{PrincipalId, TeamId};
    fn d() -> String {
        format!("sha256:{}", "b".repeat(64))
    }
    fn intent() -> TaskIntent {
        TaskIntent {
            id: "task-1".into(),
            idempotency_key: "key-1".into(),
            owner: OwnerScope::Team(TeamId::new("team-1").unwrap()),
            project: None,
            creator: PrincipalId::new("p-1").unwrap(),
            agent_id: "agent-1".into(),
            agent_version: 1,
            agent_revision_digest: d(),
            input_digest: d(),
            catalog_generation: "catalog-1".into(),
            authority_fingerprint: "authority-1".into(),
        }
    }
    #[test]
    fn duplicate_create_and_terminal_race_are_fenced_and_audited() {
        let dir = tempfile::tempdir().unwrap();
        let mut s = TaskStore::open(&dir.path().join("tasks.db")).unwrap();
        assert_eq!(s.create(&intent(), 1).unwrap(), "task-1");
        assert_eq!(s.create(&intent(), 2).unwrap(), "task-1");
        assert_eq!(s.get("task-1").unwrap().unwrap().state, TaskState::Created);
        assert_eq!(s.list_page("", 100).unwrap().len(), 1);
        s.transition(
            "task-1",
            TaskState::Created,
            TaskState::Queued,
            "p-1",
            0,
            None,
            None,
            3,
        )
        .unwrap();
        s.transition(
            "task-1",
            TaskState::Queued,
            TaskState::Running,
            "p-1",
            1,
            None,
            None,
            4,
        )
        .unwrap();
        let settled = TaskSettlement {
            state: TaskState::Succeeded,
            output_digest: Some(d()),
            error_code: None,
            settled_at: 5,
        };
        s.transition(
            "task-1",
            TaskState::Running,
            TaskState::Succeeded,
            "p-1",
            1,
            None,
            Some(&settled),
            5,
        )
        .unwrap();
        assert!(
            s.transition(
                "task-1",
                TaskState::Running,
                TaskState::Failed,
                "p-1",
                1,
                None,
                None,
                6
            )
            .is_err()
        );
        let n: i64 = s
            .connection
            .query_row("SELECT count(*) FROM agent_task_audit", [], |r| r.get(0))
            .unwrap();
        assert_eq!(n, 4);
    }
}
