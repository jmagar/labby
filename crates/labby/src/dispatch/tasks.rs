//! Authenticated, owner-scoped Agent Task surface shared by HTTP and MCP.

use crate::access::{
    ActionAuthoritySpec, AuthorityCeiling, AuthorityRequest, authorize_action,
    refresh_authority_epochs,
};
use crate::dispatch::error::ToolError;
use labby_auth::VerifiedIdentity;
use labby_primitives::{
    access::{
        ActionRef, Capability, InstallationId, OwnerScope, PrincipalId, ProjectId, ResourceFamily,
        ResourceId, ResourceRef, TeamId,
    },
    action::{ActionSpec, ParamSpec},
    agent::AgentSessionBinding,
    task::{TaskIntent, TaskSettlement, TaskState},
};
use labby_runtime::{
    agent_runtime::{
        AgentAuthority, AgentExecutionOutput, AgentExecutionRequest, AgentExecutor,
        AgentResourceBounds, AgentRuntimeError, Cancellation, ExecutionGuard,
    },
    authority::{AuthorityEpochVector, AuthoritySafeBoundary},
    task_runtime::{ScheduledTask, TaskLedger, TaskRuntimeError, TaskScheduler, execute_task},
};
use serde_json::{Value, json};
use std::sync::LazyLock;
use std::time::{SystemTime, UNIX_EPOCH};

static TASK_SCHEDULER: LazyLock<TaskScheduler> =
    LazyLock::new(|| TaskScheduler::new(4).expect("valid fixed task quota"));

const fn param(name: &'static str) -> ParamSpec {
    ParamSpec {
        name,
        ty: "string",
        required: true,
        description: "",
    }
}
const fn optional_param(name: &'static str) -> ParamSpec {
    ParamSpec {
        name,
        ty: "string",
        required: false,
        description: "",
    }
}
const fn action(
    name: &'static str,
    description: &'static str,
    params: &'static [ParamSpec],
) -> ActionSpec {
    ActionSpec {
        name,
        description,
        destructive: false,
        requires_admin: false,
        params,
        returns: "object",
    }
}
pub const ACTIONS: &[ActionSpec] = &[
    action(
        "tasks.create",
        "Create an immutable Agent Task intent",
        &[
            param("task_id"),
            param("idempotency_key"),
            param("owner_kind"),
            param("owner_id"),
            param("agent_id"),
            param("input_digest"),
        ],
    ),
    action(
        "tasks.list",
        "List caller-visible Agent Tasks",
        &[optional_param("cursor"), optional_param("limit")],
    ),
    action(
        "tasks.get",
        "Get a caller-visible Agent Task",
        &[param("task_id")],
    ),
    action("tasks.queue", "Queue an Agent Task", &[param("task_id")]),
    action("tasks.cancel", "Cancel an Agent Task", &[param("task_id")]),
    action(
        "tasks.result",
        "Read an Agent Task result",
        &[param("task_id")],
    ),
];

#[derive(Clone)]
pub(crate) struct TaskDispatchContext {
    pub store: crate::access::AccessStore,
    pub identity: VerifiedIdentity,
    pub ceiling: AuthorityCeiling,
}

pub(crate) async fn dispatch(
    context: TaskDispatchContext,
    name: &str,
    params: Value,
) -> Result<Value, ToolError> {
    if name == "help" {
        return Ok(crate::dispatch::helpers::help_payload("tasks", ACTIONS));
    }
    if name == "schema" {
        return crate::dispatch::helpers::action_schema(ACTIONS, &required(&params, "action")?);
    }
    if !ACTIONS.iter().any(|a| a.name == name) {
        return Err(unknown(name));
    }
    let now = now()?;
    match name {
        "tasks.create" => {
            let owner = owner(&params)?;
            let task_id = required(&params, "task_id")?;
            let request = authority_request(
                &context,
                name,
                &owner,
                task_id.clone(),
                Capability::ScopeCreate,
                now,
            )?;
            let agent = context
                .store
                .get_agent_definition(required(&params, "agent_id")?)
                .await
                .map_err(map)?
                .ok_or_else(denied)?;
            if agent.owner != owner || agent.state != labby_primitives::agent::AgentState::Active {
                return Err(denied());
            }
            let intent = TaskIntent {
                id: task_id,
                idempotency_key: required(&params, "idempotency_key")?,
                owner,
                project: params
                    .get("project_id")
                    .and_then(Value::as_str)
                    .map(|v| ProjectId::new(v.to_owned()).map_err(|_| invalid("project_id")))
                    .transpose()?,
                creator: PrincipalId::new("pending-authority-binding")
                    .map_err(|_| invalid("principal"))?,
                agent_id: agent.id,
                agent_version: agent.revision.version,
                agent_revision_digest: agent.revision.content_digest,
                input_digest: required(&params, "input_digest")?,
                catalog_generation: agent.revision.catalog_generation,
                authority_fingerprint: context.identity.safe_fingerprint(),
            };
            let id = context
                .store
                .authorize_and_create_agent_task(
                    request,
                    intent,
                    i64::try_from(now).map_err(|_| internal())?,
                )
                .await
                .map_err(map)?;
            Ok(json!({"task_id":id,"state":"created"}))
        }
        "tasks.list" => {
            let limit = page_limit(&params)?;
            let cursor = params
                .get("cursor")
                .and_then(Value::as_str)
                .unwrap_or_default();
            let probe_owner = OwnerScope::Installation(
                InstallationId::new("authorized-list-probe").map_err(|_| internal())?,
            );
            let request = authority_request(
                &context,
                name,
                &probe_owner,
                "authorized-list-probe".to_owned(),
                Capability::ScopeRead,
                now,
            )?;
            let page = context
                .store
                .list_authorized_agent_tasks(cursor.to_owned(), limit, request)
                .await
                .map_err(map)?;
            let next_cursor = page.last().map(|record| record.intent.id.clone());
            let tasks = page.iter().map(render).collect::<Vec<_>>();
            Ok(json!({"tasks":tasks,"next_cursor":next_cursor}))
        }
        "tasks.get" | "tasks.result" => {
            let record = load(&context, &params).await?;
            let authority_lease = authorize(
                &context,
                name,
                &record.intent.owner,
                record.intent.id.clone(),
                Capability::ScopeRead,
                now,
            )
            .await?;
            if name == "tasks.result" {
                // Task outputs default to creator-only. Team administration is
                // not a secret-output grant; a future broader policy must be
                // captured explicitly in the durable intent.
                if !record.state.terminal()
                    || record.intent.creator.as_str() != authority_lease.binding().principal_id()
                {
                    return Err(denied());
                }
            }
            Ok(render(&record))
        }
        "tasks.queue" | "tasks.cancel" => {
            let record = load(&context, &params).await?;
            let request = authority_request(
                &context,
                name,
                &record.intent.owner,
                record.intent.id.clone(),
                Capability::ScopeOperate,
                now,
            )?;
            let next = if name == "tasks.queue" {
                TaskState::Queued
            } else {
                TaskState::Cancelling
            };
            let authority_lease = context
                .store
                .authorize_and_transition_agent_task(
                    request,
                    record.intent.id.clone(),
                    record.state,
                    next,
                    context.identity.safe_fingerprint(),
                    record.attempt,
                    i64::try_from(now).map_err(|_| internal())?,
                )
                .await
                .map_err(map)?;
            if name == "tasks.queue" {
                execute_queued(&context, &record, authority_lease, now).await?;
            } else {
                context
                    .store
                    .transition_agent_task(
                        record.intent.id.clone(),
                        TaskState::Cancelling,
                        TaskState::Cancelled,
                        context.identity.safe_fingerprint(),
                        record.attempt,
                        i64::try_from(now).map_err(|_| internal())?,
                    )
                    .await
                    .map_err(map)?;
            }
            Ok(
                json!({"task_id":record.intent.id,"state":if name == "tasks.cancel" { "cancelled" } else { next.wire() }}),
            )
        }
        _ => Err(unknown(name)),
    }
}

async fn load(
    context: &TaskDispatchContext,
    params: &Value,
) -> Result<crate::access::TaskRecord, ToolError> {
    context
        .store
        .get_agent_task(required(params, "task_id")?)
        .await
        .map_err(map)?
        .ok_or_else(denied)
}
async fn authorize(
    context: &TaskDispatchContext,
    name: &str,
    owner: &OwnerScope,
    id: String,
    capability: Capability,
    now: u64,
) -> Result<labby_runtime::authority::AuthorityLease, ToolError> {
    authorize_action(
        &context.store,
        authority_request(context, name, owner, id, capability, now)?,
    )
    .await
    .map_err(map)
}
fn authority_request(
    context: &TaskDispatchContext,
    name: &str,
    owner: &OwnerScope,
    id: String,
    capability: Capability,
    now: u64,
) -> Result<AuthorityRequest, ToolError> {
    let action = ActionRef::new("tasks", name).map_err(|_| invalid("action"))?;
    Ok(AuthorityRequest::new(
        context.identity.clone(),
        ActionAuthoritySpec::SCHEMA_VERSION,
        action.clone(),
        ResourceRef::new(
            owner.clone(),
            ResourceFamily::Task,
            ResourceId::new(id).map_err(|_| invalid("task_id"))?,
        ),
        context.ceiling.clone(),
        None,
        now,
        vec![
            AuthoritySafeBoundary::BeforeDispatch,
            AuthoritySafeBoundary::BeforeCommit,
        ],
        vec![ActionAuthoritySpec::new(
            action,
            ResourceFamily::Task,
            capability,
        )],
    ))
}

async fn execute_queued(
    context: &TaskDispatchContext,
    record: &crate::access::TaskRecord,
    lease: labby_runtime::authority::AuthorityLease,
    now: u64,
) -> Result<(), ToolError> {
    use sha2::{Digest as _, Sha256};
    let definition = context
        .store
        .get_agent_definition(record.intent.agent_id.clone())
        .await
        .map_err(map)?
        .ok_or_else(denied)?;
    let epochs = refresh_authority_epochs(
        &context.store,
        context.identity.clone(),
        record.intent.owner.clone(),
        Capability::ScopeOperate,
    )
    .await
    .map_err(map)?;
    let attempt = record.attempt.saturating_add(1);
    let fence = hex::encode(Sha256::digest(format!(
        "{}:{attempt}:{now}",
        record.intent.id
    )));
    let expires = now.saturating_add(30_000);
    let request = AgentExecutionRequest {
        definition: definition.clone(),
        session: AgentSessionBinding {
            session_id: format!("task-{}-{attempt}", record.intent.id),
            agent_id: definition.id.clone(),
            agent_version: definition.revision.version,
            principal: PrincipalId::new(context.identity.safe_fingerprint())
                .map_err(|_| invalid("principal"))?,
            owner: record.intent.owner.clone(),
            catalog_generation: definition.revision.catalog_generation.clone(),
            authority_fingerprint: epochs.fingerprint().as_str().into(),
            lease_expires_at: i64::try_from(lease.expires_at_millis()).map_err(|_| internal())?,
        },
        lease,
        bounds: AgentResourceBounds {
            max_runtime_millis: 300_000,
            max_output_bytes: 16 * 1024 * 1024,
            max_external_effects: 1_000,
        },
    };
    let task = ScheduledTask {
        task_id: record.intent.id.clone(),
        owner: record.intent.owner.clone(),
        attempt,
        fencing_token: fence,
        lease_expires_at: expires,
        agent_request: request,
    };
    let ledger = StoreLedger {
        store: context.store.clone(),
        actor: context.identity.safe_fingerprint(),
        now,
    };
    let _ = ledger.recover_expired(now).await.map_err(|_| internal())?;
    match execute_task(
        &TASK_SCHEDULER,
        &ledger,
        &LiveExecutionAuthority {
            store: context.store.clone(),
            identity: context.identity.clone(),
            owner: record.intent.owner.clone(),
        },
        &DisabledExecutor,
        task,
        Cancellation::new(),
        now,
    )
    .await
    {
        Ok(_) => Ok(()),
        Err(_) => Err(ToolError::Sdk {
            sdk_kind: "service_unavailable".into(),
            message: "Agent Task execution backend is not configured".into(),
        }),
    }
}

struct LiveExecutionAuthority {
    store: crate::access::AccessStore,
    identity: VerifiedIdentity,
    owner: OwnerScope,
}
impl AgentAuthority for LiveExecutionAuthority {
    async fn current_epochs(&self) -> Result<AuthorityEpochVector, AgentRuntimeError> {
        refresh_authority_epochs(
            &self.store,
            self.identity.clone(),
            self.owner.clone(),
            Capability::ScopeOperate,
        )
        .await
        .map_err(|_| AgentRuntimeError::AuthorityUnavailable)
    }
}
struct DisabledExecutor;
impl AgentExecutor for DisabledExecutor {
    async fn execute(
        &self,
        _: AgentExecutionRequest,
        _: ExecutionGuard<'_>,
    ) -> Result<AgentExecutionOutput, AgentRuntimeError> {
        if cfg!(debug_assertions) && std::env::var_os("LABBY_E2E_DETERMINISTIC_EXECUTORS").is_some()
        {
            Ok(AgentExecutionOutput {
                digest: format!("sha256:{}", "0".repeat(64)),
                bytes: 0,
                external_effects: 0,
            })
        } else {
            Err(AgentRuntimeError::ExecutorFailed)
        }
    }
}
struct StoreLedger {
    store: crate::access::AccessStore,
    actor: String,
    now: u64,
}
impl TaskLedger for StoreLedger {
    async fn acquire(&self, task: &ScheduledTask) -> Result<(), TaskRuntimeError> {
        let now = i64::try_from(self.now).map_err(|_| TaskRuntimeError::Unavailable)?;
        self.store
            .acquire_agent_task_lease(
                task.task_id.clone(),
                task.attempt,
                task.fencing_token.clone(),
                i64::try_from(task.lease_expires_at).map_err(|_| TaskRuntimeError::Unavailable)?,
                now,
            )
            .await
            .map_err(|_| TaskRuntimeError::FencedConflict)?;
        self.store
            .settle_agent_task(
                task.task_id.clone(),
                TaskState::Queued,
                TaskState::Running,
                self.actor.clone(),
                task.attempt,
                task.fencing_token.clone(),
                TaskSettlement {
                    state: TaskState::Running,
                    output_digest: None,
                    error_code: None,
                    settled_at: now,
                },
                now,
            )
            .await
            .map_err(|_| TaskRuntimeError::FencedConflict)
    }
    async fn settle(
        &self,
        task: &ScheduledTask,
        state: TaskState,
        output: Option<&AgentExecutionOutput>,
        reason: Option<&str>,
    ) -> Result<(), TaskRuntimeError> {
        let now = i64::try_from(self.now).map_err(|_| TaskRuntimeError::Unavailable)?;
        self.store
            .settle_agent_task(
                task.task_id.clone(),
                TaskState::Running,
                state,
                self.actor.clone(),
                task.attempt,
                task.fencing_token.clone(),
                TaskSettlement {
                    state,
                    output_digest: output.map(|v| v.digest.clone()),
                    error_code: reason.map(str::to_owned),
                    settled_at: now,
                },
                now,
            )
            .await
            .map_err(|_| TaskRuntimeError::FencedConflict)
    }
    async fn recover_expired(&self, now: u64) -> Result<usize, TaskRuntimeError> {
        self.store
            .recover_expired_agent_tasks(
                i64::try_from(now).map_err(|_| TaskRuntimeError::Unavailable)?,
            )
            .await
            .map_err(|_| TaskRuntimeError::Unavailable)
    }
}
fn owner(params: &Value) -> Result<OwnerScope, ToolError> {
    let id = required(params, "owner_id")?;
    match required(params, "owner_kind")?.as_str() {
        "installation" => Ok(OwnerScope::Installation(
            InstallationId::new(id).map_err(|_| invalid("owner_id"))?,
        )),
        "team" => Ok(OwnerScope::Team(
            TeamId::new(id).map_err(|_| invalid("owner_id"))?,
        )),
        "project" => Ok(OwnerScope::Project(
            ProjectId::new(id).map_err(|_| invalid("owner_id"))?,
        )),
        "personal" => Ok(OwnerScope::Personal(
            PrincipalId::new(id).map_err(|_| invalid("owner_id"))?,
        )),
        _ => Err(invalid("owner_kind")),
    }
}
fn render(v: &crate::access::TaskRecord) -> Value {
    let (kind, id) = match &v.intent.owner {
        OwnerScope::Installation(x) => ("installation", x.as_str()),
        OwnerScope::Team(x) => ("team", x.as_str()),
        OwnerScope::Project(x) => ("project", x.as_str()),
        OwnerScope::Personal(x) => ("personal", x.as_str()),
    };
    json!({"task_id":v.intent.id,"owner_kind":kind,"owner_id":id,"agent_id":v.intent.agent_id,"agent_version":v.intent.agent_version,"state":v.state.wire(),"attempt":v.attempt,"output_digest":v.output_digest,"error_code":v.error_code})
}
fn required(v: &Value, k: &str) -> Result<String, ToolError> {
    v.get(k)
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .map(ToOwned::to_owned)
        .ok_or_else(|| invalid(k))
}
fn page_limit(params: &Value) -> Result<usize, ToolError> {
    match params.get("limit") {
        None | Some(Value::Null) => Ok(100),
        Some(Value::String(value)) => value
            .parse::<usize>()
            .ok()
            .filter(|value| (1..=100).contains(value))
            .ok_or_else(|| invalid("limit")),
        _ => Err(invalid("limit")),
    }
}
fn now() -> Result<u64, ToolError> {
    u64::try_from(
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(|_| internal())?
            .as_millis(),
    )
    .map_err(|_| internal())
}
fn invalid(p: &str) -> ToolError {
    ToolError::InvalidParam {
        message: format!("invalid parameter `{p}`"),
        param: p.into(),
    }
}
fn denied() -> ToolError {
    ToolError::Forbidden {
        message: "access denied".into(),
        required_scopes: vec![],
    }
}
fn internal() -> ToolError {
    ToolError::internal_message("Task service unavailable")
}
fn map(e: crate::access::AccessStoreError) -> ToolError {
    match e {
        crate::access::AccessStoreError::NotAuthorized
        | crate::access::AccessStoreError::IdentityUnavailable
        | crate::access::AccessStoreError::ProjectAccessUnavailable
        | crate::access::AccessStoreError::TeamUnavailable => denied(),
        _ => internal(),
    }
}
fn unknown(name: &str) -> ToolError {
    ToolError::UnknownAction {
        message: "unknown Task action".into(),
        valid: ACTIONS.iter().map(|a| a.name.into()).collect(),
        hint: ACTIONS
            .iter()
            .find(|a| a.name.starts_with(name))
            .map(|a| a.name.into()),
    }
}
pub async fn dispatch_unbound(name: &str, params: Value) -> Result<Value, ToolError> {
    if name == "help" {
        return Ok(crate::dispatch::helpers::help_payload("tasks", ACTIONS));
    }
    if name == "schema" {
        return crate::dispatch::helpers::action_schema(ACTIONS, &required(&params, "action")?);
    }
    Err(denied())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn catalog_is_complete() {
        assert_eq!(ACTIONS.len(), 6);
        assert!(ACTIONS.iter().all(|a| a.name.starts_with("tasks.")));
        let create = ACTIONS
            .iter()
            .find(|action| action.name == "tasks.create")
            .unwrap();
        assert!(
            create
                .params
                .iter()
                .any(|param| param.name == "input_digest" && param.required)
        );
    }
    #[tokio::test]
    async fn unbound_is_non_enumerating() {
        assert_eq!(
            dispatch_unbound("tasks.get", json!({"task_id":"guessed"}))
                .await
                .unwrap_err()
                .kind(),
            "forbidden"
        );
    }
}
