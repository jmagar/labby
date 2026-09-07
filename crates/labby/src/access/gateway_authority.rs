use std::time::{SystemTime, UNIX_EPOCH};

use labby_auth::{VerifiedIdentity, auth_context::AuthContext};
use labby_primitives::access::{
    ActionRef, Capability, InstallationId, OwnerScope, ResourceFamily, ResourceId, ResourceRef,
    TeamId,
};
use labby_runtime::authority::AuthoritySafeBoundary;

use super::{
    AccessRuntime, ActionAuthoritySpec, AuthorityCeiling, AuthorityRequest, authorize_action,
};
use crate::dispatch::error::ToolError;
use serde_json::Value;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum GatewayAuthorityClass {
    Public,
    ScopedRead,
    ScopedManage,
    PlatformManage,
}

/// Gateway policy is team-manageable; host configuration and process/credential
/// lifecycle are installation authority. Unknown actions fail closed.
pub(crate) fn gateway_authority_class(action: &str) -> Option<GatewayAuthorityClass> {
    Some(match action {
        "help" | "schema" => GatewayAuthorityClass::Public,
        "gateway.loadout.list"
        | "gateway.loadout.list_state"
        | "gateway.loadout.get"
        | "gateway.protected_route.list"
        | "gateway.protected_route.list_state"
        | "gateway.protected_route.get" => GatewayAuthorityClass::ScopedRead,
        action
            if action.starts_with("gateway.loadout.")
                || action.starts_with("gateway.protected_route.") =>
        {
            GatewayAuthorityClass::ScopedManage
        }
        action if action.starts_with("gateway.") => GatewayAuthorityClass::PlatformManage,
        _ => return None,
    })
}

/// OAuth transport scopes are only the outer admission ceiling. Team-scoped
/// policy actions are authorized by the domain evaluator below and therefore
/// must not be pre-emptively classified as platform-admin operations.
pub(crate) fn gateway_transport_requires_admin(action: &str) -> bool {
    matches!(
        gateway_authority_class(action),
        Some(GatewayAuthorityClass::PlatformManage)
    )
}

pub(crate) fn qualify_team_gateway_params(
    action: &str,
    team_id: Option<&str>,
    mut params: Value,
) -> Result<Value, ToolError> {
    if !matches!(
        gateway_authority_class(action),
        Some(GatewayAuthorityClass::ScopedRead | GatewayAuthorityClass::ScopedManage)
    ) {
        return Ok(params);
    }
    let team_id = team_id.ok_or_else(denied)?;
    TeamId::new(team_id).map_err(|_| denied())?;
    let prefix = format!("team:{team_id}:");
    qualify_named_fields(&mut params, &prefix);
    Ok(params)
}

pub(crate) fn gateway_runtime_subject(
    action: &str,
    team_id: Option<&str>,
    subject: Option<&str>,
) -> Option<String> {
    let subject = subject?;
    if matches!(
        gateway_authority_class(action),
        Some(GatewayAuthorityClass::ScopedRead | GatewayAuthorityClass::ScopedManage)
    ) {
        // Membership is checked before this point. All members of one Team
        // deliberately select its shared custodied credential, while sibling
        // teams retain distinct pool/cache identities.
        return team_id.map(|team| format!("team:{team}"));
    }
    Some(subject.to_owned())
}

fn qualify_named_fields(value: &mut Value, prefix: &str) {
    match value {
        Value::Object(object) => {
            // `team_id` is an authorization selector, not part of any Gateway
            // action schema. Consume it at this boundary so strict dispatch
            // deserializers do not reject otherwise-authorized MCP requests.
            object.remove("team_id");
            for (key, child) in object {
                if matches!(key.as_str(), "name" | "loadout") {
                    if let Some(name) = child.as_str() {
                        if !name.starts_with(prefix) {
                            *child = Value::String(format!("{prefix}{name}"));
                        }
                    } else {
                        // `loadout` is a string selector on read/delete actions,
                        // but an object containing its own `name` on create and
                        // update actions. Walk the object form instead of
                        // treating the key itself as a terminal field.
                        qualify_named_fields(child, prefix);
                    }
                } else {
                    qualify_named_fields(child, prefix);
                }
            }
        }
        Value::Array(values) => values
            .iter_mut()
            .for_each(|value| qualify_named_fields(value, prefix)),
        _ => {}
    }
}

pub(crate) fn filter_team_gateway_projection(team_id: Option<&str>, value: &mut Value) {
    let Some(team_id) = team_id else { return };
    let prefix = format!("team:{team_id}:");
    filter_projection(value, &prefix);
}

fn filter_projection(value: &mut Value, prefix: &str) -> bool {
    match value {
        Value::Array(values) => {
            values.retain_mut(|value| filter_projection(value, prefix));
            true
        }
        Value::Object(object) => {
            if let Some(name) = object
                .get_mut("name")
                .and_then(|value| value.as_str())
                .map(str::to_owned)
            {
                if !name.starts_with(prefix) {
                    return false;
                }
                object.insert(
                    "name".into(),
                    Value::String(name[prefix.len()..].to_owned()),
                );
            }
            for child in object.values_mut() {
                filter_projection(child, prefix);
            }
            true
        }
        _ => true,
    }
}

pub(crate) async fn authorize_gateway_action(
    runtime: &AccessRuntime,
    identity: VerifiedIdentity,
    auth: &AuthContext,
    installation_id: &str,
    team_id: Option<&str>,
    action: &str,
) -> Result<(), ToolError> {
    let class = gateway_authority_class(action).ok_or_else(denied)?;
    if class == GatewayAuthorityClass::Public {
        return Ok(());
    }
    let store = runtime.store().await.map_err(|_| denied())?;
    let (owner, capability, resource_id) = match class {
        GatewayAuthorityClass::ScopedRead | GatewayAuthorityClass::ScopedManage => {
            let team_id = team_id.ok_or_else(denied)?;
            let owner = OwnerScope::Team(TeamId::new(team_id).map_err(|_| denied())?);
            let capability = if class == GatewayAuthorityClass::ScopedRead {
                Capability::ScopeRead
            } else {
                Capability::ScopeManage
            };
            (owner, capability, team_id.to_owned())
        }
        GatewayAuthorityClass::PlatformManage => (
            OwnerScope::Installation(InstallationId::new(installation_id).map_err(|_| denied())?),
            Capability::PlatformManage,
            installation_id.to_owned(),
        ),
        GatewayAuthorityClass::Public => unreachable!(),
    };
    let action_ref = ActionRef::new("gateway", action).map_err(|_| denied())?;
    let resource = ResourceRef::new(
        owner,
        ResourceFamily::Gateway,
        ResourceId::new(resource_id).map_err(|_| denied())?,
    );
    let now = u64::try_from(
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(|_| denied())?
            .as_millis(),
    )
    .map_err(|_| denied())?;
    authorize_action(
        &store,
        AuthorityRequest::new(
            identity,
            ActionAuthoritySpec::SCHEMA_VERSION,
            action_ref.clone(),
            resource,
            AuthorityCeiling::from_auth_context(auth),
            None,
            now,
            vec![AuthoritySafeBoundary::BeforeDispatch],
            vec![ActionAuthoritySpec::new(
                action_ref,
                ResourceFamily::Gateway,
                capability,
            )],
        ),
    )
    .await
    .map(|_| ())
    .map_err(|_| denied())
}

fn denied() -> ToolError {
    ToolError::Forbidden {
        message: "Gateway operation is not authorized".into(),
        required_scopes: Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn team_policy_is_distinct_from_host_authority() {
        assert_eq!(
            gateway_authority_class("gateway.loadout.add"),
            Some(GatewayAuthorityClass::ScopedManage)
        );
        assert_eq!(
            gateway_authority_class("gateway.protected_route.get"),
            Some(GatewayAuthorityClass::ScopedRead)
        );
        for action in [
            "gateway.add",
            "gateway.update",
            "gateway.remove",
            "gateway.reload",
            "gateway.oauth.clear",
            "gateway.mcp.restart",
            "gateway.service_config.set",
        ] {
            assert_eq!(
                gateway_authority_class(action),
                Some(GatewayAuthorityClass::PlatformManage),
                "{action}"
            );
        }
        assert_eq!(
            gateway_authority_class("gateway.unknown"),
            Some(GatewayAuthorityClass::PlatformManage)
        );
        assert_eq!(gateway_authority_class("other.action"), None);
        assert!(!gateway_transport_requires_admin("gateway.loadout.add"));
        assert!(gateway_transport_requires_admin("gateway.add"));
    }

    #[test]
    fn team_names_are_qualified_and_other_team_rows_are_removed() {
        let params = qualify_team_gateway_params(
            "gateway.loadout.add",
            Some("alpha"),
            serde_json::json!({"team_id":"alpha","loadout":{"name":"prod","upstreams":["shared"]}}),
        )
        .unwrap();
        assert_eq!(params["loadout"]["name"], "team:alpha:prod");
        assert!(params.get("team_id").is_none());

        let selector = qualify_team_gateway_params(
            "gateway.loadout.get",
            Some("alpha"),
            serde_json::json!({"loadout":"prod"}),
        )
        .unwrap();
        assert_eq!(selector["loadout"], "team:alpha:prod");
        let mut rows = serde_json::json!([{"name":"team:alpha:prod"},{"name":"team:beta:prod"}]);
        filter_team_gateway_projection(Some("alpha"), &mut rows);
        assert_eq!(rows, serde_json::json!([{"name":"prod"}]));
        assert_ne!(
            gateway_runtime_subject("gateway.loadout.get", Some("alpha"), Some("user")),
            gateway_runtime_subject("gateway.loadout.get", Some("beta"), Some("user")),
        );
        assert_eq!(
            gateway_runtime_subject("gateway.loadout.get", Some("alpha"), Some("first")),
            gateway_runtime_subject("gateway.loadout.get", Some("alpha"), Some("second")),
        );
    }
}
