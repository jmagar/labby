//! Shared Dev Container dispatch orchestration.
//!
//! Transport adapters are intentionally absent. Runtime effects are delegated
//! to the surface-neutral, pluggable engine contract.

use labby_auth::VerifiedIdentity;
use labby_primitives::{
    access::{
        ActionRef, Capability, InstallationId, OwnerKind, OwnerScope, PrincipalId, ProjectId,
        ResourceFamily, ResourceId, ResourceRef, TeamId,
    },
    action::{ActionSpec, ParamSpec},
    dev_container::{DesiredState, DevContainerId, LifecycleNonce},
};
use serde_json::Value;
use std::{
    collections::BTreeSet,
    sync::Arc,
    time::{SystemTime, UNIX_EPOCH},
};

#[allow(unused_imports)]
pub(crate) use labby_runtime::dev_container_runtime::{
    ContainerRuntime, DurableIntent, EngineCreateRequest, EngineHandle, EngineState,
    RecoveryAction, RuntimeError, create, reconcile,
};

const INSTANCE_ID: ParamSpec = ParamSpec {
    name: "instance_id",
    ty: "string",
    required: true,
    description: "Opaque Dev Container identifier",
};
const TEMPLATE_ID: ParamSpec = ParamSpec {
    name: "template_id",
    ty: "string",
    required: true,
    description: "Administrator-approved template identifier",
};
const OWNER_KIND: ParamSpec = ParamSpec {
    name: "owner_kind",
    ty: "installation|team|project|personal",
    required: true,
    description: "Single durable owner scope",
};
const OWNER_ID: ParamSpec = ParamSpec {
    name: "owner_id",
    ty: "string",
    required: true,
    description: "Owner identifier resolved against caller authority",
};
const SECRET_REFS: ParamSpec = ParamSpec {
    name: "secret_references",
    ty: "string[]",
    required: false,
    description: "Opaque secret references; secret material is never accepted",
};
const CURSOR: ParamSpec = ParamSpec {
    name: "cursor",
    ty: "string",
    required: false,
    description: "Exclusive instance identifier cursor",
};
const LIMIT: ParamSpec = ParamSpec {
    name: "limit",
    ty: "string",
    required: false,
    description: "Page size from 1 through 100",
};

const fn action(
    name: &'static str,
    description: &'static str,
    params: &'static [ParamSpec],
    destructive: bool,
) -> ActionSpec {
    ActionSpec {
        name,
        description,
        destructive,
        requires_admin: false,
        params,
        returns: "object",
    }
}

pub(crate) const ACTIONS: &[ActionSpec] = &[
    action(
        "dev_containers.create",
        "Create from an approved template",
        &[INSTANCE_ID, TEMPLATE_ID, OWNER_KIND, OWNER_ID, SECRET_REFS],
        false,
    ),
    action(
        "dev_containers.list",
        "List Dev Containers visible to the caller",
        &[CURSOR, LIMIT],
        false,
    ),
    action(
        "dev_containers.start",
        "Request a stopped Dev Container start",
        &[INSTANCE_ID],
        false,
    ),
    action(
        "dev_containers.stop",
        "Request a Dev Container stop",
        &[INSTANCE_ID],
        false,
    ),
    action(
        "dev_containers.destroy",
        "Permanently destroy a Dev Container",
        &[INSTANCE_ID],
        true,
    ),
    action(
        "dev_containers.reconcile",
        "Reconcile durable intent with the runtime",
        &[INSTANCE_ID],
        false,
    ),
];

/// Exact evaluator input used before resolving a lease. Unknown actions deny.
pub(crate) fn required_capability(action: &str, _owner: OwnerKind) -> Option<Capability> {
    match action {
        "dev_containers.list" => Some(Capability::ScopeRead),
        "dev_containers.create" => Some(Capability::ScopeCreate),
        "dev_containers.start" | "dev_containers.stop" | "dev_containers.reconcile" => {
            Some(Capability::ScopeOperate)
        }
        "dev_containers.destroy" => Some(Capability::ScopeDelete),
        _ => None,
    }
}

/// MCP cannot supply host-established identity/epochs. Refuse without revealing
/// whether a requested instance exists; authenticated HTTP uses the bound path.
pub(crate) async fn dispatch_unbound(
    action: &str,
    params: Value,
) -> Result<Value, crate::dispatch::error::ToolError> {
    if action == "help" {
        return Ok(crate::dispatch::helpers::help_payload(
            "dev_containers",
            ACTIONS,
        ));
    }
    if action == "schema" {
        let requested = params
            .get("action")
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty())
            .ok_or_else(|| crate::dispatch::error::ToolError::MissingParam {
                message: "missing required parameter `action`".into(),
                param: "action".into(),
            })?;
        return crate::dispatch::helpers::action_schema(ACTIONS, requested);
    }
    Err(crate::dispatch::error::ToolError::Forbidden {
        message: "Dev Container operation is not authorized".into(),
        required_scopes: Vec::new(),
    })
}

#[derive(Clone)]
pub(crate) struct DevContainerDispatchContext {
    pub access_runtime: Arc<crate::access::AccessRuntime>,
    pub identity: VerifiedIdentity,
    pub ceiling: crate::access::AuthorityCeiling,
}

pub(crate) async fn dispatch(
    context: DevContainerDispatchContext,
    action: &str,
    params: Value,
) -> Result<Value, crate::dispatch::error::ToolError> {
    if action == "help" {
        return Ok(crate::dispatch::helpers::help_payload(
            "dev_containers",
            ACTIONS,
        ));
    }
    if action == "schema" {
        return crate::dispatch::helpers::action_schema(
            ACTIONS,
            params
                .get("action")
                .and_then(Value::as_str)
                .unwrap_or_default(),
        );
    }
    let store = context
        .access_runtime
        .store()
        .await
        .map_err(|_| unavailable())?;
    if action == "dev_containers.list" {
        let limit = match params.get("limit") {
            None | Some(Value::Null) => 100,
            Some(Value::String(value)) => value
                .parse::<usize>()
                .ok()
                .filter(|value| (1..=100).contains(value))
                .ok_or_else(denied)?,
            _ => return Err(denied()),
        };
        let cursor = params
            .get("cursor")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let probe_owner = OwnerScope::Installation(
            InstallationId::new("authorized-list-probe").map_err(|_| denied())?,
        );
        let now = now_millis()?;
        let request =
            authority_request(&context, action, probe_owner, "authorized-list-probe", now)
                .map_err(|_| denied())?;
        let inventory = store
            .list_authorized_dev_containers(cursor.to_owned(), limit, request)
            .await
            .map_err(|_| denied())?;
        let next_cursor = inventory.last().map(|record| record.instance_id.clone());
        let visible = inventory.iter().map(record_json).collect::<Vec<_>>();
        return Ok(serde_json::json!({"instances":visible,"next_cursor":next_cursor}));
    }
    if action == "dev_containers.create" {
        let instance_id = params
            .get("instance_id")
            .and_then(Value::as_str)
            .ok_or_else(denied)?
            .to_owned();
        let template_id = params
            .get("template_id")
            .and_then(Value::as_str)
            .ok_or_else(denied)?
            .to_owned();
        let owner_id = params
            .get("owner_id")
            .and_then(Value::as_str)
            .ok_or_else(denied)?;
        let kind = match params.get("owner_kind").and_then(Value::as_str) {
            Some("installation") => OwnerKind::Installation,
            Some("team") => OwnerKind::Team,
            Some("project") => OwnerKind::Project,
            Some("personal") => OwnerKind::Personal,
            _ => return Err(denied()),
        };
        let owner = owner_scope(kind, owner_id)?;
        let (lease, _) = authorize(&context, &store, action, owner.clone(), &instance_id)
            .await
            .map_err(|_| denied())?;
        let secrets = params
            .get("secret_references")
            .and_then(Value::as_array)
            .map(|v| {
                v.iter()
                    .map(|x| x.as_str().map(str::to_owned).ok_or_else(denied))
                    .collect()
            })
            .transpose()?
            .unwrap_or_default();
        let now = now_millis()?;
        let created = crate::access::create_approved_for_store(
            &store,
            owner,
            instance_id.clone(),
            template_id,
            secrets,
            context.identity.safe_fingerprint(),
            format!("create-{now}"),
            i64::try_from(now / 1000).map_err(|_| unavailable())?,
        )
        .await
        .map_err(|_| unavailable())?;
        // The durable admission write above is an asynchronous boundary. Fetch
        // current epochs immediately before the engine call so a membership or
        // policy revocation cannot reuse the earlier authorization snapshot.
        let epochs = crate::access::refresh_authority_epochs(
            &store,
            context.identity.clone(),
            created.instance.owner().clone(),
            required_capability(action, created.instance.owner().kind()).ok_or_else(denied)?,
        )
        .await
        .map_err(|_| denied())?;
        create(
            context.access_runtime.dev_container_runtime().as_ref(),
            &lease,
            &epochs,
            now,
            &created.template,
            EngineCreateRequest {
                handle: EngineHandle {
                    instance_id: created.instance.id().clone(),
                    lifecycle_nonce: created.instance.lifecycle_nonce().clone(),
                },
                image_digest: created.instance.image().as_str().into(),
                cpu_millis: created.resources.cpu_millis,
                memory_bytes: created.resources.memory_bytes,
                disk_bytes: created.resources.disk_bytes,
                lifetime_seconds: created.resources.lifetime_seconds,
                host_capabilities: BTreeSet::new(),
            },
        )
        .await
        .map_err(|_| unavailable())?;
        return Ok(
            serde_json::json!({"instance_id":instance_id,"desired_state":"running","observed_state":"pending"}),
        );
    }
    let instance_id = params
        .get("instance_id")
        .and_then(Value::as_str)
        .ok_or_else(denied)?;
    let inventory = crate::access::recovery_inventory_for_store(&store)
        .await
        .map_err(|_| unavailable())?;
    let record = inventory
        .into_iter()
        .find(|item| item.instance_id == instance_id)
        .ok_or_else(denied)?;
    let owner = owner_scope(record.owner_kind, &record.owner_id)?;
    let capability = required_capability(action, owner.kind()).ok_or_else(denied)?;
    let (lease, _) = authorize(&context, &store, action, owner.clone(), instance_id)
        .await
        .map_err(|_| denied())?;
    let desired = match action {
        "dev_containers.start" => Some(DesiredState::Running),
        "dev_containers.stop" => Some(DesiredState::Stopped),
        "dev_containers.destroy" => Some(DesiredState::Deleted),
        "dev_containers.reconcile" => None,
        _ => return Err(denied()),
    };
    let now = now_millis()?;
    if let Some(desired) = desired {
        crate::access::set_desired_for_store(
            &store,
            record.instance_id.clone(),
            record.lifecycle_nonce.clone(),
            desired,
            format!("{}-{now}", action.replace('.', "-")),
            i64::try_from(now / 1000).map_err(|_| unavailable())?,
        )
        .await
        .map_err(|_| unavailable())?;
    }
    let intent = match desired.unwrap_or(record.desired_state) {
        DesiredState::Running => DurableIntent::Running,
        DesiredState::Stopped => DurableIntent::Stopped,
        DesiredState::Deleted => DurableIntent::Deleted,
    };
    let handle = EngineHandle {
        instance_id: DevContainerId::new(record.instance_id.clone()).map_err(|_| unavailable())?,
        lifecycle_nonce: LifecycleNonce::new(record.lifecycle_nonce).map_err(|_| unavailable())?,
    };
    // Desired-state persistence is not authority for a later host effect.
    // Re-read epochs at the final boundary so revocation invalidates the lease.
    let epochs = crate::access::refresh_authority_epochs(
        &store,
        context.identity.clone(),
        owner,
        capability,
    )
    .await
    .map_err(|_| denied())?;
    let result = reconcile(
        context.access_runtime.dev_container_runtime().as_ref(),
        &lease,
        &epochs,
        now,
        &handle,
        intent,
    )
    .await
    .map_err(|_| unavailable())?;
    Ok(
        serde_json::json!({"instance_id":instance_id,"recovery_action":format!("{result:?}").to_ascii_lowercase()}),
    )
}

async fn authorize(
    context: &DevContainerDispatchContext,
    store: &crate::access::AccessStore,
    action: &str,
    owner: OwnerScope,
    id: &str,
) -> Result<
    (
        labby_runtime::authority::AuthorityLease,
        labby_runtime::authority::AuthorityEpochVector,
    ),
    crate::access::AccessStoreError,
> {
    let capability = required_capability(action, owner.kind())
        .ok_or(crate::access::AccessStoreError::NotAuthorized)?;
    let now = now_millis().map_err(|_| crate::access::AccessStoreError::NotAuthorized)?;
    let lease = crate::access::authorize_action(
        store,
        authority_request(context, action, owner.clone(), id, now)?,
    )
    .await?;
    let epochs =
        crate::access::refresh_authority_epochs(store, context.identity.clone(), owner, capability)
            .await?;
    Ok((lease, epochs))
}

fn authority_request(
    context: &DevContainerDispatchContext,
    action: &str,
    owner: OwnerScope,
    id: &str,
    now: u64,
) -> Result<crate::access::AuthorityRequest, crate::access::AccessStoreError> {
    let capability = required_capability(action, owner.kind())
        .ok_or(crate::access::AccessStoreError::NotAuthorized)?;
    let action_ref = ActionRef::new("dev_containers", action)
        .map_err(|_| crate::access::AccessStoreError::MalformedVocabulary)?;
    let resource = ResourceRef::new(
        owner.clone(),
        ResourceFamily::DevContainer,
        ResourceId::new(id).map_err(|_| crate::access::AccessStoreError::MalformedVocabulary)?,
    );
    Ok(crate::access::AuthorityRequest::new(
        context.identity.clone(),
        crate::access::ActionAuthoritySpec::SCHEMA_VERSION,
        action_ref.clone(),
        resource,
        context.ceiling.clone(),
        None,
        now,
        vec![labby_runtime::authority::AuthoritySafeBoundary::BeforeExternalEffect],
        vec![crate::access::ActionAuthoritySpec::new(
            action_ref,
            ResourceFamily::DevContainer,
            capability,
        )],
    ))
}
fn owner_scope(kind: OwnerKind, id: &str) -> Result<OwnerScope, crate::dispatch::error::ToolError> {
    Ok(match kind {
        OwnerKind::Installation => {
            OwnerScope::Installation(InstallationId::new(id).map_err(|_| denied())?)
        }
        OwnerKind::Team => OwnerScope::Team(TeamId::new(id).map_err(|_| denied())?),
        OwnerKind::Project => OwnerScope::Project(ProjectId::new(id).map_err(|_| denied())?),
        OwnerKind::Personal => OwnerScope::Personal(PrincipalId::new(id).map_err(|_| denied())?),
    })
}
fn record_json(record: &crate::access::RecoveryRecord) -> Value {
    serde_json::json!({"instance_id":record.instance_id,"owner_kind":format!("{:?}",record.owner_kind).to_ascii_lowercase(),"owner_id":record.owner_id,"desired_state":format!("{:?}",record.desired_state).to_ascii_lowercase(),"observed_state":format!("{:?}",record.observed_state).to_ascii_lowercase()})
}
fn now_millis() -> Result<u64, crate::dispatch::error::ToolError> {
    u64::try_from(
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(|_| unavailable())?
            .as_millis(),
    )
    .map_err(|_| unavailable())
}
fn denied() -> crate::dispatch::error::ToolError {
    crate::dispatch::error::ToolError::Forbidden {
        message: "Dev Container operation is not authorized".into(),
        required_scopes: Vec::new(),
    }
}
fn unavailable() -> crate::dispatch::error::ToolError {
    crate::dispatch::error::ToolError::Sdk {
        sdk_kind: "service_unavailable".into(),
        message: "Dev Container runtime is unavailable".into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn actions_have_exact_fail_closed_capabilities_and_no_raw_host_authority() {
        assert_eq!(
            required_capability("dev_containers.destroy", OwnerKind::Team),
            Some(Capability::ScopeDelete)
        );
        assert_eq!(
            required_capability("dev_containers.unknown", OwnerKind::Team),
            None
        );
        assert_eq!(
            required_capability("dev_containers.create", OwnerKind::Installation),
            Some(Capability::ScopeCreate)
        );
        assert!(
            ACTIONS
                .iter()
                .flat_map(|action| action.params)
                .all(|param| !matches!(
                    param.name,
                    "image" | "privileged" | "host_network" | "devices" | "mounts"
                ))
        );
    }

    #[tokio::test]
    async fn unbound_denial_does_not_enumerate_resources() {
        let missing = dispatch_unbound(
            "dev_containers.start",
            serde_json::json!({"instance_id":"missing"}),
        )
        .await
        .unwrap_err()
        .to_string();
        let existing = dispatch_unbound(
            "dev_containers.start",
            serde_json::json!({"instance_id":"known"}),
        )
        .await
        .unwrap_err()
        .to_string();
        assert_eq!(missing, existing);
    }
}
