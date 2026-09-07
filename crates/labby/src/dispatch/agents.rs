//! Authenticated, owner-scoped Agent surface shared by HTTP and MCP.

use crate::{
    access::{
        ActionAuthoritySpec, AuthorityCeiling, AuthorityRequest, authorize_action,
        refresh_authority_epochs,
    },
    dispatch::error::ToolError,
};
use labby_auth::VerifiedIdentity;
use labby_primitives::{
    access::{
        ActionRef, Capability, InstallationId, OwnerScope, PrincipalId, ProjectId, ResourceFamily,
        ResourceId, ResourceRef, TeamId,
    },
    action::{ActionSpec, ParamSpec},
    agent::{
        AgentDefinition, AgentRevision, AgentSessionBinding, AgentState, RunningRevocationPolicy,
    },
};
use labby_runtime::{
    agent_runtime::{
        AgentAuthority, AgentExecutionOutput, AgentExecutionRequest, AgentExecutor,
        AgentResourceBounds, AgentRuntimeError, Cancellation, ExecutionGuard, execute_agent,
    },
    authority::{AuthorityEpochVector, AuthoritySafeBoundary},
};
use serde_json::{Value, json};
use std::time::{SystemTime, UNIX_EPOCH};

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
        "agents.create",
        "Create an Agent definition",
        &[
            param("agent_id"),
            param("owner_kind"),
            param("owner_id"),
            param("content_digest"),
            param("repository_digest"),
            param("image_digest"),
            param("harness_digest"),
            param("loadout_digest"),
            param("catalog_generation"),
        ],
    ),
    action(
        "agents.list",
        "List caller-visible Agents",
        &[optional_param("cursor"), optional_param("limit")],
    ),
    action(
        "agents.get",
        "Get a caller-visible Agent",
        &[param("agent_id")],
    ),
    action(
        "agents.update",
        "Create the next immutable Agent revision",
        &[param("agent_id")],
    ),
    action("agents.suspend", "Suspend an Agent", &[param("agent_id")]),
    action("agents.delete", "Delete an Agent", &[param("agent_id")]),
    action(
        "agents.run",
        "Start a pinned Agent session",
        &[param("agent_id")],
    ),
    action(
        "agents.session.status",
        "Read Agent session status",
        &[param("agent_id"), param("session_id")],
    ),
];

#[derive(Clone)]
pub(crate) struct AgentDispatchContext {
    pub store: crate::access::AccessStore,
    pub identity: VerifiedIdentity,
    pub ceiling: AuthorityCeiling,
}

pub(crate) async fn dispatch(
    context: AgentDispatchContext,
    name: &str,
    params: Value,
) -> Result<Value, ToolError> {
    if name == "help" {
        return Ok(crate::dispatch::helpers::help_payload("agents", ACTIONS));
    }
    if name == "schema" {
        return crate::dispatch::helpers::action_schema(ACTIONS, &required(&params, "action")?);
    }
    if !ACTIONS.iter().any(|a| a.name == name) {
        return Err(unknown(name));
    }
    let now = now()?;
    match name {
        "agents.create" => {
            let definition = definition(&params, None)?;
            let request = authority_request(
                &context,
                name,
                &definition.owner,
                &definition.id,
                Capability::ScopeCreate,
                now,
            )?;
            context
                .store
                .authorize_and_put_agent_definition(
                    request,
                    definition.clone(),
                    context.identity.safe_fingerprint(),
                    i64::try_from(now).map_err(|_| internal())?,
                )
                .await
                .map_err(map)?;
            Ok(render(&definition))
        }
        "agents.list" => {
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
                "authorized-list-probe",
                Capability::ScopeRead,
                now,
            )?;
            let page = context
                .store
                .list_authorized_agent_definitions(cursor.to_owned(), limit, request)
                .await
                .map_err(map)?;
            let next_cursor = page.last().map(|definition| definition.id.clone());
            let visible = page.iter().map(render).collect::<Vec<_>>();
            Ok(json!({"agents":visible,"next_cursor":next_cursor}))
        }
        "agents.get" => {
            let definition = load(&context, &params).await?;
            authorize(
                &context,
                name,
                &definition.owner,
                &definition.id,
                Capability::ScopeRead,
                now,
            )
            .await?;
            Ok(render(&definition))
        }
        "agents.update" => {
            let prior = load(&context, &params).await?;
            let definition = definition(&params, Some(&prior))?;
            let request = authority_request(
                &context,
                name,
                &prior.owner,
                &prior.id,
                Capability::ScopeManage,
                now,
            )?;
            context
                .store
                .authorize_and_put_agent_definition(
                    request,
                    definition.clone(),
                    context.identity.safe_fingerprint(),
                    i64::try_from(now).map_err(|_| internal())?,
                )
                .await
                .map_err(map)?;
            Ok(render(&definition))
        }
        "agents.suspend" | "agents.delete" => {
            let definition = load(&context, &params).await?;
            let capability = if name.ends_with("delete") {
                Capability::ScopeDelete
            } else {
                Capability::ScopeManage
            };
            let request = authority_request(
                &context,
                name,
                &definition.owner,
                &definition.id,
                capability,
                now,
            )?;
            let state = if name.ends_with("delete") {
                AgentState::Deleted
            } else {
                AgentState::Suspended
            };
            context
                .store
                .authorize_and_set_agent_definition_state(
                    request,
                    definition.id.clone(),
                    state,
                    context.identity.safe_fingerprint(),
                    i64::try_from(now).map_err(|_| internal())?,
                )
                .await
                .map_err(map)?;
            Ok(json!({"agent_id":definition.id,"state":state_name(state)}))
        }
        "agents.run" => {
            let definition = load(&context, &params).await?;
            if definition.state != AgentState::Active {
                return Err(denied());
            }
            let lease = authorize(
                &context,
                name,
                &definition.owner,
                &definition.id,
                Capability::ScopeOperate,
                now,
            )
            .await?;
            let epochs = refresh_authority_epochs(
                &context.store,
                context.identity.clone(),
                definition.owner.clone(),
                Capability::ScopeOperate,
            )
            .await
            .map_err(map)?;
            lease
                .validate_at(AuthoritySafeBoundary::BeforeCommit, now, &epochs)
                .map_err(|_| denied())?;
            let session_id = format!("{}-{now}", definition.id);
            let lease_expires_at = lease.expires_at_millis();
            context
                .store
                .create_agent_session(
                    session_id.clone(),
                    definition.clone(),
                    context.identity.safe_fingerprint(),
                    epochs.fingerprint().as_str().to_owned(),
                    i64::try_from(lease.expires_at_millis()).map_err(|_| internal())?,
                    i64::try_from(now).map_err(|_| internal())?,
                )
                .await
                .map_err(map)?;
            context
                .store
                .set_agent_session_status(
                    definition.id.clone(),
                    session_id.clone(),
                    "admitted".into(),
                    "running".into(),
                )
                .await
                .map_err(map)?;
            let request = AgentExecutionRequest {
                definition: definition.clone(),
                session: AgentSessionBinding {
                    session_id: session_id.clone(),
                    agent_id: definition.id.clone(),
                    agent_version: definition.revision.version,
                    principal: PrincipalId::new(context.identity.safe_fingerprint())
                        .map_err(|_| invalid("principal"))?,
                    owner: definition.owner.clone(),
                    catalog_generation: definition.revision.catalog_generation.clone(),
                    authority_fingerprint: epochs.fingerprint().as_str().into(),
                    lease_expires_at: i64::try_from(lease_expires_at).map_err(|_| internal())?,
                },
                lease,
                bounds: AgentResourceBounds {
                    max_runtime_millis: 300_000,
                    max_output_bytes: 16 * 1024 * 1024,
                    max_external_effects: 1_000,
                },
            };
            let result = execute_agent(
                &LiveExecutionAuthority {
                    store: context.store.clone(),
                    identity: context.identity.clone(),
                    owner: definition.owner.clone(),
                },
                &DisabledExecutor,
                request,
                Cancellation::new(),
                now,
            )
            .await;
            let next = match result {
                Ok(_) => "completed",
                Err(AgentRuntimeError::Revoked | AgentRuntimeError::Lease(_)) => "revoked",
                Err(AgentRuntimeError::Cancelled) => "cancelled",
                Err(_) => "failed",
            };
            context
                .store
                .set_agent_session_status(
                    definition.id.clone(),
                    session_id.clone(),
                    "running".into(),
                    next.into(),
                )
                .await
                .map_err(map)?;
            match result {
                Ok(output) => Ok(
                    json!({"agent_id":definition.id,"agent_version":definition.revision.version,"session_id":session_id,"status":next,"output_digest":output.digest,"authority_expires_at":lease_expires_at}),
                ),
                Err(_) => Err(ToolError::Sdk {
                    sdk_kind: "service_unavailable".into(),
                    message: "Agent execution backend is not configured".into(),
                }),
            }
        }
        "agents.session.status" => {
            let definition = load(&context, &params).await?;
            authorize(
                &context,
                name,
                &definition.owner,
                &definition.id,
                Capability::ScopeRead,
                now,
            )
            .await?;
            let session_id = required(&params, "session_id")?;
            let status = context
                .store
                .get_agent_session_status(definition.id.clone(), session_id.clone())
                .await
                .map_err(map)?
                .ok_or_else(denied)?;
            Ok(json!({"agent_id":definition.id,"session_id":session_id,"status":status}))
        }
        _ => Err(unknown(name)),
    }
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

async fn load(
    context: &AgentDispatchContext,
    params: &Value,
) -> Result<AgentDefinition, ToolError> {
    context
        .store
        .get_agent_definition(required(params, "agent_id")?)
        .await
        .map_err(map)?
        .ok_or_else(denied)
}
async fn authorize(
    context: &AgentDispatchContext,
    name: &str,
    owner: &OwnerScope,
    id: &str,
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
    context: &AgentDispatchContext,
    name: &str,
    owner: &OwnerScope,
    id: &str,
    capability: Capability,
    now: u64,
) -> Result<AuthorityRequest, ToolError> {
    let action = ActionRef::new("agents", name).map_err(|_| invalid("action"))?;
    Ok(AuthorityRequest::new(
        context.identity.clone(),
        ActionAuthoritySpec::SCHEMA_VERSION,
        action.clone(),
        ResourceRef::new(
            owner.clone(),
            ResourceFamily::Agent,
            ResourceId::new(id).map_err(|_| invalid("agent_id"))?,
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
            ResourceFamily::Agent,
            capability,
        )],
    ))
}
fn definition(
    params: &Value,
    prior: Option<&AgentDefinition>,
) -> Result<AgentDefinition, ToolError> {
    let id = prior.map_or_else(|| required(params, "agent_id"), |v| Ok(v.id.clone()))?;
    let owner = prior.map_or_else(|| owner(params), |v| Ok(v.owner.clone()))?;
    let version = prior.map_or(1, |v| v.revision.version + 1);
    let digest = |key: &str| {
        params
            .get(key)
            .and_then(Value::as_str)
            .map(ToOwned::to_owned)
            .or_else(|| {
                prior.map(|v| match key {
                    "content_digest" => v.revision.content_digest.clone(),
                    "repository_digest" => v.revision.repository_digest.clone(),
                    "image_digest" => v.revision.image_digest.clone(),
                    "harness_digest" => v.revision.harness_digest.clone(),
                    "loadout_digest" => v.revision.loadout_digest.clone(),
                    _ => v.revision.catalog_generation.clone(),
                })
            })
            .ok_or_else(|| invalid(key))
    };
    let value = AgentDefinition {
        id,
        owner,
        revision: AgentRevision {
            version,
            content_digest: digest("content_digest")?,
            repository_digest: digest("repository_digest")?,
            image_digest: digest("image_digest")?,
            harness_digest: digest("harness_digest")?,
            loadout_digest: digest("loadout_digest")?,
            catalog_generation: digest("catalog_generation")?,
            credential_references: params
                .get("credential_references")
                .and_then(Value::as_array)
                .map(|a| {
                    a.iter()
                        .filter_map(|v| v.as_str().map(ToOwned::to_owned))
                        .collect()
                })
                .or_else(|| prior.map(|v| v.revision.credential_references.clone()))
                .unwrap_or_default(),
        },
        state: AgentState::Active,
        required_capabilities: vec![Capability::ScopeOperate],
        authority_epoch: params
            .get("authority_epoch")
            .and_then(Value::as_u64)
            .or_else(|| prior.map(|v| v.authority_epoch))
            .unwrap_or(1),
        publication_epoch: params
            .get("publication_epoch")
            .and_then(Value::as_u64)
            .or_else(|| prior.map(|v| v.publication_epoch))
            .unwrap_or(1),
        revocation_policy: RunningRevocationPolicy::StopAtSafeBoundary,
    };
    value.validate().map_err(|_| invalid("definition"))?;
    Ok(value)
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
fn render(v: &AgentDefinition) -> Value {
    let (kind, id) = match &v.owner {
        OwnerScope::Installation(x) => ("installation", x.as_str()),
        OwnerScope::Team(x) => ("team", x.as_str()),
        OwnerScope::Project(x) => ("project", x.as_str()),
        OwnerScope::Personal(x) => ("personal", x.as_str()),
    };
    json!({"agent_id":v.id,"owner_kind":kind,"owner_id":id,"version":v.revision.version,"state":state_name(v.state),"catalog_generation":v.revision.catalog_generation,"authority_epoch":v.authority_epoch,"publication_epoch":v.publication_epoch})
}
fn state_name(v: AgentState) -> &'static str {
    match v {
        AgentState::Active => "active",
        AgentState::Suspended => "suspended",
        AgentState::Deleted => "deleted",
    }
}
fn required(v: &Value, k: &str) -> Result<String, ToolError> {
    v.get(k)
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .map(ToOwned::to_owned)
        .ok_or_else(|| invalid(k))
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
    ToolError::internal_message("Agent service unavailable")
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
        message: "unknown Agent action".into(),
        valid: ACTIONS.iter().map(|a| a.name.into()).collect(),
        hint: ACTIONS
            .iter()
            .find(|a| a.name.starts_with(name))
            .map(|a| a.name.into()),
    }
}
pub async fn dispatch_unbound(name: &str, params: Value) -> Result<Value, ToolError> {
    if name == "help" {
        return Ok(crate::dispatch::helpers::help_payload("agents", ACTIONS));
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
    fn catalog_is_complete_and_unbound_denies() {
        assert_eq!(ACTIONS.len(), 8);
        let create = ACTIONS
            .iter()
            .find(|action| action.name == "agents.create")
            .unwrap();
        for required in [
            "content_digest",
            "repository_digest",
            "image_digest",
            "harness_digest",
            "loadout_digest",
            "catalog_generation",
        ] {
            assert!(
                create
                    .params
                    .iter()
                    .any(|param| param.name == required && param.required)
            );
        }
        assert!(ACTIONS.iter().all(|a| a.name.starts_with("agents.")));
    }
    #[tokio::test]
    async fn context_free_is_fail_closed() {
        assert_eq!(
            dispatch_unbound("agents.list", json!({}))
                .await
                .unwrap_err()
                .kind(),
            "forbidden"
        );
    }
}
