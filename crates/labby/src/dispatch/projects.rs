//! Team-scoped Project lifecycle semantics shared by authenticated surfaces.
use crate::{
    access::{AccessStore, ManageTeamProjectInput},
    dispatch::error::ToolError,
};
use labby_auth::VerifiedIdentity;
use serde_json::Value;

pub(crate) async fn dispatch(
    store: AccessStore,
    identity: VerifiedIdentity,
    action: &str,
    params: Value,
) -> Result<Value, ToolError> {
    let team = || required(&params, "team_id");
    let project = || required(&params, "project_id");
    let result = match action {
        "projects.list" => {
            return serde_json::to_value(store.list_managed_projects(identity).await.map_err(map)?)
                .map_err(|_| unavailable());
        }
        "projects.create" => {
            store
                .create_managed_project(
                    ManageTeamProjectInput::new(
                        identity,
                        team()?,
                        project()?,
                        Some(required(&params, "name")?),
                    )
                    .map_err(map)?,
                )
                .await
        }
        "projects.get" => {
            store
                .get_managed_project(
                    ManageTeamProjectInput::new(identity, team()?, project()?, None)
                        .map_err(map)?,
                )
                .await
        }
        "projects.update" => {
            store
                .update_managed_project(
                    ManageTeamProjectInput::new(
                        identity,
                        team()?,
                        project()?,
                        Some(required(&params, "name")?),
                    )
                    .map_err(map)?,
                    false,
                )
                .await
        }
        "projects.archive" => {
            store
                .update_managed_project(
                    ManageTeamProjectInput::new(identity, team()?, project()?, None)
                        .map_err(map)?,
                    true,
                )
                .await
        }
        _ => {
            return Err(ToolError::UnknownAction {
                message: "unknown Project action".into(),
                valid: vec![
                    "projects.list".into(),
                    "projects.create".into(),
                    "projects.get".into(),
                    "projects.update".into(),
                    "projects.archive".into(),
                ],
                hint: None,
            });
        }
    }
    .map_err(map)?;
    serde_json::to_value(result).map_err(|_| unavailable())
}
fn required(v: &Value, key: &str) -> Result<String, ToolError> {
    v.get(key)
        .and_then(Value::as_str)
        .filter(|v| !v.is_empty())
        .map(str::to_owned)
        .ok_or_else(|| ToolError::InvalidParam {
            message: format!("invalid parameter `{key}`"),
            param: key.into(),
        })
}
fn map(error: crate::access::AccessStoreError) -> ToolError {
    match error {
        crate::access::AccessStoreError::NotAuthorized
        | crate::access::AccessStoreError::TeamUnavailable
        | crate::access::AccessStoreError::ProjectAccessUnavailable => denied(),
        _ => unavailable(),
    }
}
fn denied() -> ToolError {
    ToolError::Forbidden {
        message: "access denied".into(),
        required_scopes: vec![],
    }
}
fn unavailable() -> ToolError {
    ToolError::Sdk {
        sdk_kind: "service_unavailable".into(),
        message: "Project service unavailable".into(),
    }
}
