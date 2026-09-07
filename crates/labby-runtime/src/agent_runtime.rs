//! Pluggable, authority-fenced Agent execution orchestration.

use std::future::Future;
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

use labby_primitives::agent::{
    AgentDefinition, AgentSessionBinding, AgentState, RunningRevocationPolicy,
};
use thiserror::Error;

use crate::authority::{
    AuthorityEpochVector, AuthorityLease, AuthorityLeaseError, AuthoritySafeBoundary,
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct AgentResourceBounds {
    pub max_runtime_millis: u64,
    pub max_output_bytes: usize,
    pub max_external_effects: u32,
}
impl AgentResourceBounds {
    pub fn validate(self) -> Result<Self, AgentRuntimeError> {
        if self.max_runtime_millis == 0
            || self.max_runtime_millis > 86_400_000
            || self.max_output_bytes == 0
            || self.max_output_bytes > 64 * 1024 * 1024
            || self.max_external_effects > 10_000
        {
            Err(AgentRuntimeError::InvalidBounds)
        } else {
            Ok(self)
        }
    }
}

#[derive(Clone, Debug)]
pub struct AgentExecutionRequest {
    pub definition: AgentDefinition,
    pub session: AgentSessionBinding,
    pub lease: AuthorityLease,
    pub bounds: AgentResourceBounds,
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AgentExecutionOutput {
    pub digest: String,
    pub bytes: usize,
    pub external_effects: u32,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AgentRecoveryAction {
    ResumeAfterReauthorization,
    MarkInterrupted,
    MarkRevoked,
}
pub fn recovery_action(
    was_running: bool,
    state: AgentState,
    lease_valid: bool,
) -> AgentRecoveryAction {
    if state != AgentState::Active {
        AgentRecoveryAction::MarkRevoked
    } else if was_running && lease_valid {
        AgentRecoveryAction::ResumeAfterReauthorization
    } else {
        AgentRecoveryAction::MarkInterrupted
    }
}

pub trait AgentAuthority: Send + Sync {
    fn current_epochs(
        &self,
    ) -> impl Future<Output = Result<AuthorityEpochVector, AgentRuntimeError>> + Send;
}
pub trait AgentExecutor: Send + Sync {
    fn execute(
        &self,
        request: AgentExecutionRequest,
        guard: ExecutionGuard<'_>,
    ) -> impl Future<Output = Result<AgentExecutionOutput, AgentRuntimeError>> + Send;
}

#[derive(Clone)]
pub struct Cancellation {
    cancelled: Arc<AtomicBool>,
}
impl Cancellation {
    pub fn new() -> Self {
        Self {
            cancelled: Arc::new(AtomicBool::new(false)),
        }
    }
    pub fn cancel(&self) {
        self.cancelled.store(true, Ordering::Release)
    }
    fn is_cancelled(&self) -> bool {
        self.cancelled.load(Ordering::Acquire)
    }
}
impl Default for Cancellation {
    fn default() -> Self {
        Self::new()
    }
}

pub struct ExecutionGuard<'a> {
    authority: &'a dyn AuthorityDyn,
    lease: AuthorityLease,
    cancellation: Cancellation,
    revocation: RunningRevocationPolicy,
}
trait AuthorityDyn: Send + Sync {
    fn epochs(
        &self,
    ) -> std::pin::Pin<
        Box<dyn Future<Output = Result<AuthorityEpochVector, AgentRuntimeError>> + Send + '_>,
    >;
}
impl<T: AgentAuthority> AuthorityDyn for T {
    fn epochs(
        &self,
    ) -> std::pin::Pin<
        Box<dyn Future<Output = Result<AuthorityEpochVector, AgentRuntimeError>> + Send + '_>,
    > {
        Box::pin(self.current_epochs())
    }
}
impl ExecutionGuard<'_> {
    pub async fn check(
        &self,
        boundary: AuthoritySafeBoundary,
        now: u64,
    ) -> Result<(), AgentRuntimeError> {
        if self.cancellation.is_cancelled() {
            return Err(AgentRuntimeError::Cancelled);
        }
        let epochs = self.authority.epochs().await?;
        match self.lease.validate_at(boundary, now, &epochs) {
            Ok(()) => Ok(()),
            Err(AuthorityLeaseError::AuthorityChanged)
                if self.revocation == RunningRevocationPolicy::StopAtSafeBoundary =>
            {
                Err(AgentRuntimeError::Revoked)
            }
            Err(error) => Err(AgentRuntimeError::Lease(error)),
        }
    }
}

pub async fn execute_agent<A: AgentAuthority, E: AgentExecutor>(
    authority: &A,
    executor: &E,
    request: AgentExecutionRequest,
    cancellation: Cancellation,
    now: u64,
) -> Result<AgentExecutionOutput, AgentRuntimeError> {
    request
        .definition
        .validate()
        .map_err(|_| AgentRuntimeError::InvalidDefinition)?;
    request.bounds.validate()?;
    if request.definition.state != AgentState::Active
        || request.session.agent_id != request.definition.id
        || request.session.agent_version != request.definition.revision.version
        || request.session.catalog_generation != request.definition.revision.catalog_generation
    {
        return Err(AgentRuntimeError::PinnedDefinitionMismatch);
    }
    let epochs = authority.current_epochs().await?;
    request
        .lease
        .validate_at(AuthoritySafeBoundary::BeforeDispatch, now, &epochs)
        .map_err(AgentRuntimeError::Lease)?;
    let final_lease = request.lease.clone();
    let final_cancellation = cancellation.clone();
    let revocation = request.definition.revocation_policy;
    let guard = ExecutionGuard {
        authority,
        lease: request.lease.clone(),
        cancellation,
        revocation,
    };
    let bounds = request.bounds;
    let output = executor.execute(request, guard).await?;
    // Executors may check around their own external effects, but the runtime
    // owns the final commit boundary and never trusts an implementation to do
    // so. This closes the gap for executors that omit or misplace guard.check.
    ExecutionGuard {
        authority,
        lease: final_lease,
        cancellation: final_cancellation,
        revocation,
    }
    .check(AuthoritySafeBoundary::BeforeCommit, now)
    .await?;
    if output.bytes > bounds.max_output_bytes
        || output.external_effects > bounds.max_external_effects
    {
        return Err(AgentRuntimeError::ResourceLimit);
    }
    Ok(output)
}

#[derive(Debug, Error, Eq, PartialEq)]
pub enum AgentRuntimeError {
    #[error("invalid agent definition")]
    InvalidDefinition,
    #[error("pinned definition mismatch")]
    PinnedDefinitionMismatch,
    #[error("invalid resource bounds")]
    InvalidBounds,
    #[error("resource limit exceeded")]
    ResourceLimit,
    #[error("execution cancelled")]
    Cancelled,
    #[error("authority revoked")]
    Revoked,
    #[error("authority unavailable")]
    AuthorityUnavailable,
    #[error("executor failed")]
    ExecutorFailed,
    #[error("authority lease: {0}")]
    Lease(AuthorityLeaseError),
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::authority::*;
    use labby_primitives::{access::*, agent::*};
    struct Auth(AuthorityEpochVector);
    impl AgentAuthority for Auth {
        async fn current_epochs(&self) -> Result<AuthorityEpochVector, AgentRuntimeError> {
            Ok(self.0.clone())
        }
    }
    struct Exec;
    impl AgentExecutor for Exec {
        async fn execute(
            &self,
            _: AgentExecutionRequest,
            guard: ExecutionGuard<'_>,
        ) -> Result<AgentExecutionOutput, AgentRuntimeError> {
            guard.check(AuthoritySafeBoundary::BeforeCommit, 2).await?;
            Ok(AgentExecutionOutput {
                digest: dig(),
                bytes: 4,
                external_effects: 0,
            })
        }
    }
    struct ExecWithoutFinalCheck;
    impl AgentExecutor for ExecWithoutFinalCheck {
        async fn execute(
            &self,
            _: AgentExecutionRequest,
            _: ExecutionGuard<'_>,
        ) -> Result<AgentExecutionOutput, AgentRuntimeError> {
            Ok(AgentExecutionOutput {
                digest: dig(),
                bytes: 4,
                external_effects: 0,
            })
        }
    }
    struct ChangingAuth {
        initial: AuthorityEpochVector,
        changed: AuthorityEpochVector,
        reads: std::sync::atomic::AtomicUsize,
    }
    impl AgentAuthority for ChangingAuth {
        async fn current_epochs(&self) -> Result<AuthorityEpochVector, AgentRuntimeError> {
            if self.reads.fetch_add(1, Ordering::SeqCst) == 0 {
                Ok(self.initial.clone())
            } else {
                Ok(self.changed.clone())
            }
        }
    }
    fn dig() -> String {
        format!("sha256:{}", "c".repeat(64))
    }
    fn epochs(n: u64) -> AuthorityEpochVector {
        AuthorityEpochVector::new(AuthorityEpochVectorInput {
            version: 1,
            authority_schema_generation: 1,
            installation_epoch: 1,
            organization_epoch: 1,
            principal_epoch: n,
            team_membership_epochs: vec![],
            team_policy_epoch: None,
            project_membership_epoch: None,
            project_policy_epoch: None,
            resource_policy_epoch: None,
            gateway_catalog_generation: Some(1),
            depot_projection_watermark: None,
            credential_generation: Some(1),
            session_generation: 1,
        })
        .unwrap()
    }
    fn request(e: &AuthorityEpochVector) -> AgentExecutionRequest {
        let owner = OwnerScope::Personal(PrincipalId::new("p-1").unwrap());
        let binding = AuthorityBinding::new(
            PrincipalId::new("p-1").unwrap(),
            owner.clone(),
            Capability::ScopeOperate,
            "EXECUTE",
            ResourceId::new("agent-1").unwrap(),
            None,
        )
        .unwrap();
        AgentExecutionRequest {
            definition: AgentDefinition {
                id: "agent-1".into(),
                owner: owner.clone(),
                revision: AgentRevision {
                    version: 1,
                    content_digest: dig(),
                    repository_digest: dig(),
                    image_digest: dig(),
                    harness_digest: dig(),
                    loadout_digest: dig(),
                    catalog_generation: "catalog-1".into(),
                    credential_references: vec!["credential-ref".into()],
                },
                state: AgentState::Active,
                required_capabilities: vec![Capability::ScopeOperate],
                authority_epoch: 1,
                publication_epoch: 1,
                revocation_policy: RunningRevocationPolicy::StopAtSafeBoundary,
            },
            session: AgentSessionBinding {
                session_id: "session-1".into(),
                agent_id: "agent-1".into(),
                agent_version: 1,
                principal: PrincipalId::new("p-1").unwrap(),
                owner,
                catalog_generation: "catalog-1".into(),
                authority_fingerprint: e.fingerprint().as_str().into(),
                lease_expires_at: 100,
            },
            lease: AuthorityLease::new(
                binding,
                e,
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
                max_output_bytes: 100,
                max_external_effects: 1,
            },
        }
    }
    #[tokio::test]
    async fn admission_and_final_boundary_observe_revocation() {
        let initial = epochs(1);
        assert!(
            execute_agent(
                &Auth(initial.clone()),
                &Exec,
                request(&initial),
                Cancellation::new(),
                1
            )
            .await
            .is_ok()
        );
        assert_eq!(
            execute_agent(
                &Auth(epochs(2)),
                &Exec,
                request(&initial),
                Cancellation::new(),
                1
            )
            .await
            .unwrap_err(),
            AgentRuntimeError::Lease(AuthorityLeaseError::AuthorityChanged)
        );
    }

    #[tokio::test]
    async fn runtime_owns_final_boundary_even_when_executor_omits_it() {
        let initial = epochs(1);
        let authority = ChangingAuth {
            initial: initial.clone(),
            changed: epochs(2),
            reads: std::sync::atomic::AtomicUsize::new(0),
        };
        assert_eq!(
            execute_agent(
                &authority,
                &ExecWithoutFinalCheck,
                request(&initial),
                Cancellation::new(),
                1,
            )
            .await
            .unwrap_err(),
            AgentRuntimeError::Revoked
        );
    }
}
