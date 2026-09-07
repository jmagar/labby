//! Durable-ledger-backed Agent Task scheduling and fenced settlement.

use crate::agent_runtime::{
    AgentAuthority, AgentExecutionOutput, AgentExecutionRequest, AgentExecutor, AgentRuntimeError,
    Cancellation, execute_agent,
};
use crate::authority::AuthoritySafeBoundary;
use labby_primitives::{access::OwnerScope, task::TaskState};
use std::{
    collections::BTreeMap,
    future::Future,
    sync::{Arc, Mutex},
};
use thiserror::Error;
use tokio::sync::{OwnedSemaphorePermit, Semaphore};

#[derive(Clone, Debug)]
pub struct ScheduledTask {
    pub task_id: String,
    pub owner: OwnerScope,
    pub attempt: u32,
    pub fencing_token: String,
    pub lease_expires_at: u64,
    pub agent_request: AgentExecutionRequest,
}

pub trait TaskLedger: Send + Sync {
    fn acquire(
        &self,
        task: &ScheduledTask,
    ) -> impl Future<Output = Result<(), TaskRuntimeError>> + Send;
    fn settle(
        &self,
        task: &ScheduledTask,
        state: TaskState,
        output: Option<&AgentExecutionOutput>,
        reason: Option<&str>,
    ) -> impl Future<Output = Result<(), TaskRuntimeError>> + Send;
    fn recover_expired(
        &self,
        now: u64,
    ) -> impl Future<Output = Result<usize, TaskRuntimeError>> + Send;
}

pub struct TaskScheduler {
    per_owner_limit: usize,
    owners: Mutex<BTreeMap<(u8, String), Arc<Semaphore>>>,
}
impl TaskScheduler {
    pub fn new(per_owner_limit: usize) -> Result<Self, TaskRuntimeError> {
        if per_owner_limit == 0 || per_owner_limit > 64 {
            return Err(TaskRuntimeError::InvalidQuota);
        }
        Ok(Self {
            per_owner_limit,
            owners: Mutex::new(BTreeMap::new()),
        })
    }
    async fn admit(&self, owner: &OwnerScope) -> Result<OwnedSemaphorePermit, TaskRuntimeError> {
        let key = owner_key(owner);
        let semaphore = {
            let mut owners = self
                .owners
                .lock()
                .map_err(|_| TaskRuntimeError::Unavailable)?;
            // A completed owner's semaphore is referenced only by this map. Drop
            // those idle entries opportunistically on every admission so churn
            // cannot make scheduler memory grow with historical owners.
            owners.retain(|existing, semaphore| {
                existing == &key
                    || Arc::strong_count(semaphore) > 1
                    || semaphore.available_permits() != self.per_owner_limit
            });
            owners
                .entry(key)
                .or_insert_with(|| Arc::new(Semaphore::new(self.per_owner_limit)))
                .clone()
        };
        semaphore
            .acquire_owned()
            .await
            .map_err(|_| TaskRuntimeError::Unavailable)
    }

    #[cfg(test)]
    fn retained_owner_count(&self) -> usize {
        self.owners.lock().expect("scheduler mutex").len()
    }
}

pub async fn execute_task<L, A, E>(
    scheduler: &TaskScheduler,
    ledger: &L,
    authority: &A,
    executor: &E,
    task: ScheduledTask,
    cancellation: Cancellation,
    now: u64,
) -> Result<AgentExecutionOutput, TaskRuntimeError>
where
    L: TaskLedger,
    A: AgentAuthority,
    E: AgentExecutor,
{
    let _permit = scheduler.admit(&task.owner).await?;
    if task.fencing_token.len() < 32 || task.lease_expires_at <= now {
        return Err(TaskRuntimeError::InvalidLease);
    }
    ledger.acquire(&task).await?;
    match execute_agent(
        authority,
        executor,
        task.agent_request.clone(),
        cancellation,
        now,
    )
    .await
    {
        Ok(output) => {
            // Settlement is Task's durable success commit, distinct from the
            // executor's output commit. Revalidate immediately at this owning
            // boundary so a revoked result is never recorded as successful.
            let epochs = authority
                .current_epochs()
                .await
                .map_err(TaskRuntimeError::Agent)?;
            task.agent_request
                .lease
                .validate_at(AuthoritySafeBoundary::BeforeCommit, now, &epochs)
                .map_err(AgentRuntimeError::Lease)
                .map_err(TaskRuntimeError::Agent)?;
            ledger
                .settle(&task, TaskState::Succeeded, Some(&output), None)
                .await?;
            Ok(output)
        }
        Err(AgentRuntimeError::Cancelled) => {
            ledger
                .settle(&task, TaskState::Cancelled, None, Some("cancelled"))
                .await?;
            Err(TaskRuntimeError::Agent(AgentRuntimeError::Cancelled))
        }
        Err(error) => {
            ledger
                .settle(&task, TaskState::Failed, None, Some(reason(&error)))
                .await?;
            Err(TaskRuntimeError::Agent(error))
        }
    }
}

pub async fn recover_tasks<L: TaskLedger>(ledger: &L, now: u64) -> Result<usize, TaskRuntimeError> {
    ledger.recover_expired(now).await
}
fn reason(error: &AgentRuntimeError) -> &'static str {
    match error {
        AgentRuntimeError::Revoked | AgentRuntimeError::Lease(_) => "authority_revoked",
        AgentRuntimeError::ResourceLimit => "resource_limit",
        AgentRuntimeError::Cancelled => "cancelled",
        _ => "execution_failed",
    }
}
fn owner_key(owner: &OwnerScope) -> (u8, String) {
    match owner {
        OwnerScope::Installation(v) => (0, v.as_str().into()),
        OwnerScope::Team(v) => (1, v.as_str().into()),
        OwnerScope::Project(v) => (2, v.as_str().into()),
        OwnerScope::Personal(v) => (3, v.as_str().into()),
    }
}

#[derive(Debug, Error, Eq, PartialEq)]
pub enum TaskRuntimeError {
    #[error("invalid owner quota")]
    InvalidQuota,
    #[error("invalid task lease")]
    InvalidLease,
    #[error("task runtime unavailable")]
    Unavailable,
    #[error("fenced task conflict")]
    FencedConflict,
    #[error("agent runtime: {0}")]
    Agent(AgentRuntimeError),
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{agent_runtime::*, authority::*};
    use labby_primitives::access::PrincipalId;
    use labby_primitives::agent::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    struct Ledger {
        settles: AtomicUsize,
    }
    impl TaskLedger for Ledger {
        async fn acquire(&self, _: &ScheduledTask) -> Result<(), TaskRuntimeError> {
            Ok(())
        }
        async fn settle(
            &self,
            _: &ScheduledTask,
            _: TaskState,
            _: Option<&AgentExecutionOutput>,
            _: Option<&str>,
        ) -> Result<(), TaskRuntimeError> {
            self.settles.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }
        async fn recover_expired(&self, _: u64) -> Result<usize, TaskRuntimeError> {
            Ok(2)
        }
    }
    struct Auth(AuthorityEpochVector);
    impl AgentAuthority for Auth {
        async fn current_epochs(&self) -> Result<AuthorityEpochVector, AgentRuntimeError> {
            Ok(self.0.clone())
        }
    }
    struct Exec;

    #[tokio::test]
    async fn idle_owner_semaphores_are_evicted_during_owner_churn() {
        let scheduler = TaskScheduler::new(2).unwrap();
        for index in 0..1_000 {
            let owner = OwnerScope::Personal(PrincipalId::new(format!("p-{index}")).unwrap());
            drop(scheduler.admit(&owner).await.unwrap());
        }
        assert_eq!(scheduler.retained_owner_count(), 1);
    }
    impl AgentExecutor for Exec {
        async fn execute(
            &self,
            _: AgentExecutionRequest,
            g: ExecutionGuard<'_>,
        ) -> Result<AgentExecutionOutput, AgentRuntimeError> {
            g.check(AuthoritySafeBoundary::BeforeCommit, 2).await?;
            Ok(AgentExecutionOutput {
                digest: d(),
                bytes: 1,
                external_effects: 0,
            })
        }
    }
    struct ExecWithoutCheck;
    impl AgentExecutor for ExecWithoutCheck {
        async fn execute(
            &self,
            _: AgentExecutionRequest,
            _: ExecutionGuard<'_>,
        ) -> Result<AgentExecutionOutput, AgentRuntimeError> {
            Ok(AgentExecutionOutput {
                digest: d(),
                bytes: 1,
                external_effects: 0,
            })
        }
    }
    struct RevokedBeforeSettlement {
        reads: AtomicUsize,
        initial: AuthorityEpochVector,
        revoked: AuthorityEpochVector,
    }
    impl AgentAuthority for RevokedBeforeSettlement {
        async fn current_epochs(&self) -> Result<AuthorityEpochVector, AgentRuntimeError> {
            if self.reads.fetch_add(1, Ordering::SeqCst) < 2 {
                Ok(self.initial.clone())
            } else {
                Ok(self.revoked.clone())
            }
        }
    }
    fn d() -> String {
        format!("sha256:{}", "d".repeat(64))
    }
    fn epochs() -> AuthorityEpochVector {
        AuthorityEpochVector::new(AuthorityEpochVectorInput {
            version: 1,
            authority_schema_generation: 1,
            installation_epoch: 1,
            organization_epoch: 1,
            principal_epoch: 1,
            team_membership_epochs: vec![],
            team_policy_epoch: None,
            project_membership_epoch: None,
            project_policy_epoch: None,
            resource_policy_epoch: None,
            gateway_catalog_generation: Some(1),
            depot_projection_watermark: None,
            credential_generation: None,
            session_generation: 1,
        })
        .unwrap()
    }
    fn task() -> ScheduledTask {
        let e = epochs();
        let owner = OwnerScope::Personal(PrincipalId::new("p-1").unwrap());
        let binding = AuthorityBinding::new(
            PrincipalId::new("p-1").unwrap(),
            owner.clone(),
            Capability::ScopeOperate,
            "EXECUTE",
            ResourceId::new("agent-1").unwrap(),
            Some(ResourceId::new("task-1").unwrap()),
        )
        .unwrap();
        ScheduledTask {
            task_id: "task-1".into(),
            owner: owner.clone(),
            attempt: 1,
            fencing_token: "f".repeat(32),
            lease_expires_at: 100,
            agent_request: AgentExecutionRequest {
                definition: AgentDefinition {
                    id: "agent-1".into(),
                    owner: owner.clone(),
                    revision: AgentRevision {
                        version: 1,
                        content_digest: d(),
                        repository_digest: d(),
                        image_digest: d(),
                        harness_digest: d(),
                        loadout_digest: d(),
                        catalog_generation: "cat-1".into(),
                        credential_references: vec![],
                    },
                    state: AgentState::Active,
                    required_capabilities: vec![Capability::ScopeOperate],
                    authority_epoch: 1,
                    publication_epoch: 1,
                    revocation_policy: RunningRevocationPolicy::StopAtSafeBoundary,
                },
                session: AgentSessionBinding {
                    session_id: "s-1".into(),
                    agent_id: "agent-1".into(),
                    agent_version: 1,
                    principal: PrincipalId::new("p-1").unwrap(),
                    owner,
                    catalog_generation: "cat-1".into(),
                    authority_fingerprint: e.fingerprint().as_str().into(),
                    lease_expires_at: 100,
                },
                lease: AuthorityLease::new(
                    binding,
                    &e,
                    1,
                    100,
                    [
                        AuthoritySafeBoundary::BeforeDispatch,
                        AuthoritySafeBoundary::BeforeCommit,
                    ],
                )
                .unwrap(),
                bounds: AgentResourceBounds {
                    max_runtime_millis: 100,
                    max_output_bytes: 10,
                    max_external_effects: 1,
                },
            },
        }
    }
    #[tokio::test]
    async fn successful_attempt_settles_exactly_once_and_recovery_is_explicit() {
        let ledger = Ledger {
            settles: AtomicUsize::new(0),
        };
        let out = execute_task(
            &TaskScheduler::new(1).unwrap(),
            &ledger,
            &Auth(epochs()),
            &Exec,
            task(),
            Cancellation::new(),
            1,
        )
        .await
        .unwrap();
        assert_eq!(out.bytes, 1);
        assert_eq!(ledger.settles.load(Ordering::SeqCst), 1);
        assert_eq!(recover_tasks(&ledger, 101).await.unwrap(), 2);
    }

    #[tokio::test]
    async fn success_is_not_settled_after_authority_changes() {
        let ledger = Ledger {
            settles: AtomicUsize::new(0),
        };
        let initial = epochs();
        let revoked = AuthorityEpochVector::new(AuthorityEpochVectorInput {
            version: 1,
            authority_schema_generation: 1,
            installation_epoch: 1,
            organization_epoch: 1,
            principal_epoch: 2,
            team_membership_epochs: vec![],
            team_policy_epoch: None,
            project_membership_epoch: None,
            project_policy_epoch: None,
            resource_policy_epoch: None,
            gateway_catalog_generation: Some(1),
            depot_projection_watermark: None,
            credential_generation: None,
            session_generation: 1,
        })
        .unwrap();
        let authority = RevokedBeforeSettlement {
            reads: AtomicUsize::new(0),
            initial,
            revoked,
        };
        assert!(matches!(
            execute_task(
                &TaskScheduler::new(1).unwrap(),
                &ledger,
                &authority,
                &ExecWithoutCheck,
                task(),
                Cancellation::new(),
                1,
            )
            .await,
            Err(TaskRuntimeError::Agent(AgentRuntimeError::Lease(
                AuthorityLeaseError::AuthorityChanged
            )))
        ));
        assert_eq!(ledger.settles.load(Ordering::SeqCst), 0);
    }
}
