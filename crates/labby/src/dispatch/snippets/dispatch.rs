use serde::Deserialize;
use serde_json::{Value, json};

use crate::dispatch::error::ToolError;
use crate::dispatch::gateway::code_mode::{
    CodeModeBroker, CodeModeCaller, CodeModeCapabilityFilter, CodeModeSourceLookup, CodeModeSurface,
};
use crate::dispatch::helpers::{action_schema, help_payload, lab_home, require_str, to_json};

use super::catalog::ACTIONS;
use super::store::{
    builtin_snippet_dir, code_for_snippet, create_promoted_user_snippet, create_user_snippet,
    list_snippets, merge_snippet_input, remove_user_snippet, resolve_snippet,
    validate_snippet_body, validate_snippet_name,
};

#[derive(Debug, Deserialize)]
struct CreateParams {
    name: String,
    body: String,
    description: Option<String>,
    #[serde(default)]
    force: bool,
}

#[derive(Debug, Deserialize)]
struct ExecParams {
    name: Option<String>,
    #[serde(default)]
    params: Value,
    #[serde(default)]
    all: bool,
}

#[derive(Debug, Deserialize)]
struct ValidateParams {
    name: Option<String>,
    body: Option<String>,
}

#[derive(Debug, Deserialize)]
struct PromoteParams {
    execution_id: String,
    name: String,
    description: Option<String>,
    #[serde(default)]
    force: bool,
    #[serde(default)]
    shadow_builtin: bool,
}

#[derive(Debug, Clone)]
pub struct SnippetPromotionContext {
    pub actor_key: Option<String>,
    pub is_admin: bool,
    pub route_scope: String,
    pub capability_filter_fingerprint: String,
}

impl SnippetPromotionContext {
    #[must_use]
    pub fn trusted_local() -> Self {
        Self {
            actor_key: None,
            is_admin: true,
            route_scope: "root".to_string(),
            capability_filter_fingerprint: CodeModeCapabilityFilter::default().fingerprint(),
        }
    }
}

pub async fn dispatch(action: &str, params: Value) -> Result<Value, ToolError> {
    let manager = crate::dispatch::gateway::current_gateway_manager();
    dispatch_inner(manager.as_deref(), action, params, None).await
}

pub async fn dispatch_with_manager_and_context(
    manager: &crate::dispatch::gateway::manager::GatewayManager,
    action: &str,
    params: Value,
    promotion_context: Option<SnippetPromotionContext>,
) -> Result<Value, ToolError> {
    dispatch_inner(Some(manager), action, params, promotion_context).await
}

async fn dispatch_inner(
    manager: Option<&crate::dispatch::gateway::manager::GatewayManager>,
    action: &str,
    params: Value,
    promotion_context: Option<SnippetPromotionContext>,
) -> Result<Value, ToolError> {
    match action {
        "help" => Ok(help_payload("snippets", ACTIONS)),
        "schema" => {
            let a = require_str(&params, "action")?;
            action_schema(ACTIONS, a)
        }
        "snippets.list" => {
            let snippets = list_snippets(&lab_home(), &builtin_snippet_dir())?;
            to_json(json!({ "snippets": snippets }))
        }
        "snippets.get" => {
            let name = require_str(&params, "name")?;
            to_json(resolve_snippet(&lab_home(), &builtin_snippet_dir(), &name)?)
        }
        "snippets.create" => {
            let params: CreateParams = parse_params(params)?;
            to_json(create_user_snippet(
                &lab_home(),
                &params.name,
                &params.body,
                params.description.as_deref(),
                params.force,
            )?)
        }
        "snippets.promote" => {
            let params: PromoteParams = parse_params(params)?;
            promote_snippet(manager, params, promotion_context).await
        }
        "snippets.validate" => {
            let params: ValidateParams = parse_params(params)?;
            validate_snippet(params.name.as_deref(), params.body.as_deref())
        }
        "snippets.remove" => {
            let name = require_str(&params, "name")?;
            to_json(remove_user_snippet(
                &lab_home(),
                &builtin_snippet_dir(),
                &name,
            )?)
        }
        "snippets.exec" => {
            let params: ExecParams = parse_params(params)?;
            let Some(name) = params.name else {
                return Err(missing_param("missing required parameter `name`", "name"));
            };
            execute_snippet(manager, &name, params.params).await
        }
        "snippets.test" => {
            let params: ExecParams = parse_params(params)?;
            if params.all {
                return test_all_snippets(manager).await;
            }
            let Some(name) = params.name else {
                return Err(missing_param(
                    "missing required parameter `name` or set `all: true`",
                    "name",
                ));
            };
            let response = execute_snippet(manager, &name, params.params).await?;
            let passed = snippet_response_passed(&response);
            to_json(json!({
                "name": name,
                "passed": passed,
                "response": response,
            }))
        }
        unknown => Err(ToolError::UnknownAction {
            message: format!("unknown action `{unknown}` for service `snippets`"),
            valid: ACTIONS.iter().map(|a| a.name.to_string()).collect(),
            hint: None,
        }),
    }
}

async fn promote_snippet(
    manager: Option<&crate::dispatch::gateway::manager::GatewayManager>,
    params: PromoteParams,
    promotion_context: Option<SnippetPromotionContext>,
) -> Result<Value, ToolError> {
    validate_snippet_name(&params.name)?;
    let manager = manager.ok_or_else(|| ToolError::Sdk {
        sdk_kind: "gateway_unavailable".to_string(),
        message: "snippets.promote requires the live gateway manager source store".to_string(),
    })?;
    let context = promotion_context.unwrap_or_else(SnippetPromotionContext::trusted_local);
    let source = manager
        .resolve_code_mode_source(
            &params.execution_id,
            &CodeModeSourceLookup {
                actor_key: context.actor_key,
                is_admin: context.is_admin,
                route_scope: context.route_scope,
                capability_filter_fingerprint: context.capability_filter_fingerprint,
            },
        )
        .await?;
    let info = create_promoted_user_snippet(
        &lab_home(),
        &builtin_snippet_dir(),
        &params.name,
        &source.code,
        params.description.as_deref(),
        params.force,
        params.shadow_builtin,
    )?;
    to_json(json!({
        "execution_id": source.execution_id,
        "source": {
            "created_at_ms": source.created_at_ms,
            "is_admin": source.is_admin,
            "surface": match source.surface {
                CodeModeSurface::Mcp => "mcp",
                CodeModeSurface::Cli => "cli",
            },
            "route_scope": source.route_scope,
        },
        "snippet": info,
    }))
}

fn validate_snippet(name: Option<&str>, body: Option<&str>) -> Result<Value, ToolError> {
    if let Some(body) = body {
        let name = name.ok_or_else(|| {
            missing_param(
                "missing required parameter `name` when validating a body",
                "name",
            )
        })?;
        validate_snippet_name(name)?;
        validate_snippet_body(name, body)?;
        return to_json(json!({
            "valid": true,
            "name": name,
            "mode": "body",
        }));
    }

    let name =
        name.ok_or_else(|| missing_param("missing required parameter `name` or `body`", "name"))?;
    let snippet = resolve_snippet(&lab_home(), &builtin_snippet_dir(), name)?;
    let _code = code_for_snippet(&snippet)?;
    to_json(json!({
        "valid": true,
        "name": snippet.name,
        "mode": "existing",
        "source": snippet.source,
        "path": snippet.path,
    }))
}

async fn test_all_snippets(
    manager: Option<&crate::dispatch::gateway::manager::GatewayManager>,
) -> Result<Value, ToolError> {
    let snippets = list_snippets(&lab_home(), &builtin_snippet_dir())?;
    let mut results = Vec::with_capacity(snippets.len());
    for snippet in snippets {
        match execute_snippet(manager, &snippet.name, Value::Object(Default::default())).await {
            Ok(response) => {
                let passed = snippet_response_passed(&response);
                results.push(json!({
                    "name": snippet.name,
                    "passed": passed,
                    "response": response,
                }));
            }
            Err(error) => {
                results.push(json!({
                    "name": snippet.name,
                    "passed": false,
                    "error": error,
                }));
            }
        }
    }
    let passed = results.iter().all(|result| {
        result
            .get("passed")
            .and_then(Value::as_bool)
            .unwrap_or(false)
    });
    to_json(json!({
        "passed": passed,
        "results": results,
    }))
}

fn snippet_response_passed(response: &Value) -> bool {
    response
        .get("result")
        .and_then(|result| result.get("ok"))
        .and_then(Value::as_bool)
        .unwrap_or(true)
}

async fn execute_snippet(
    manager: Option<&crate::dispatch::gateway::manager::GatewayManager>,
    name: &str,
    input: Value,
) -> Result<Value, ToolError> {
    let owned_manager;
    let manager = if let Some(manager) = manager {
        manager
    } else {
        owned_manager = crate::dispatch::gateway::require_gateway_manager()?;
        owned_manager.as_ref()
    };
    let broker = CodeModeBroker::new(Some(manager));
    let config = manager.code_mode_config().await;
    let snippet = resolve_snippet(&lab_home(), &builtin_snippet_dir(), name)?;
    let code = code_for_snippet(&snippet)?;
    let input = merge_snippet_input(&snippet, input)?;
    let code = wrap_snippet_with_input(&code, &input)?;
    let response = broker
        .execute(
            &code,
            CodeModeCaller::TrustedLocal,
            CodeModeSurface::Cli,
            config,
            CodeModeCapabilityFilter::default(),
        )
        .await
        .map_err(|error| error.into_tool_error())?;
    to_json(response)
}

fn wrap_snippet_with_input(code: &str, input: &Value) -> Result<String, ToolError> {
    let input = serde_json::to_string(input).map_err(|e| ToolError::InvalidParam {
        message: format!("snippet params must be JSON-serializable: {e}"),
        param: "params".to_string(),
    })?;
    Ok(format!(
        "async () => {{\n  const __labSnippetInput = {input};\n  return await ({code})(__labSnippetInput);\n}}"
    ))
}

fn parse_params<T: serde::de::DeserializeOwned>(params: Value) -> Result<T, ToolError> {
    serde_json::from_value(params).map_err(|e| ToolError::InvalidParam {
        message: format!("invalid snippets params: {e}"),
        param: "params".to_string(),
    })
}

fn missing_param(message: &str, param: &str) -> ToolError {
    ToolError::MissingParam {
        message: message.to_string(),
        param: param.to_string(),
    }
}
