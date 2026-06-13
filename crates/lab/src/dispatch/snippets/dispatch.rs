use serde::Deserialize;
use serde_json::{Value, json};

use crate::dispatch::error::ToolError;
use crate::dispatch::gateway::code_mode::{
    CodeModeBroker, CodeModeCaller, CodeModeCapabilityFilter, CodeModeSurface,
};
use crate::dispatch::helpers::{action_schema, help_payload, lab_home, require_str, to_json};

use super::catalog::ACTIONS;
use super::store::{
    builtin_snippet_dir, code_for_snippet, create_user_snippet, list_snippets, merge_snippet_input,
    remove_user_snippet, resolve_snippet, validate_snippet_body, validate_snippet_name,
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
    max_tool_calls: Option<usize>,
    #[serde(default)]
    all: bool,
}

#[derive(Debug, Deserialize)]
struct ValidateParams {
    name: Option<String>,
    body: Option<String>,
}

pub async fn dispatch(action: &str, params: Value) -> Result<Value, ToolError> {
    let manager = crate::dispatch::gateway::current_gateway_manager();
    dispatch_inner(manager.as_deref(), action, params).await
}

pub async fn dispatch_with_manager(
    manager: &crate::dispatch::gateway::manager::GatewayManager,
    action: &str,
    params: Value,
) -> Result<Value, ToolError> {
    dispatch_inner(Some(manager), action, params).await
}

async fn dispatch_inner(
    manager: Option<&crate::dispatch::gateway::manager::GatewayManager>,
    action: &str,
    params: Value,
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
                return Err(ToolError::MissingParam {
                    message: "missing required parameter `name`".to_string(),
                    param: "name".to_string(),
                });
            };
            execute_snippet(manager, &name, params.params, params.max_tool_calls).await
        }
        "snippets.test" => {
            let params: ExecParams = parse_params(params)?;
            if params.all {
                return test_all_snippets(manager).await;
            }
            let Some(name) = params.name else {
                return Err(ToolError::MissingParam {
                    message: "missing required parameter `name` or set `all: true`".to_string(),
                    param: "name".to_string(),
                });
            };
            let response = execute_snippet(manager, &name, params.params, None).await?;
            let passed = response
                .get("result")
                .and_then(|result| result.get("ok"))
                .and_then(Value::as_bool)
                .unwrap_or(true);
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

fn validate_snippet(name: Option<&str>, body: Option<&str>) -> Result<Value, ToolError> {
    if let Some(body) = body {
        let name = name.ok_or_else(|| ToolError::MissingParam {
            message: "missing required parameter `name` when validating a body".to_string(),
            param: "name".to_string(),
        })?;
        validate_snippet_name(name)?;
        validate_snippet_body(name, body)?;
        return to_json(json!({
            "valid": true,
            "name": name,
            "mode": "body",
        }));
    }

    let name = name.ok_or_else(|| ToolError::MissingParam {
        message: "missing required parameter `name` or `body`".to_string(),
        param: "name".to_string(),
    })?;
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
        match execute_snippet(
            manager,
            &snippet.name,
            Value::Object(Default::default()),
            None,
        )
        .await
        {
            Ok(response) => {
                let passed = response
                    .get("result")
                    .and_then(|result| result.get("ok"))
                    .and_then(Value::as_bool)
                    .unwrap_or(true);
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

async fn execute_snippet(
    manager: Option<&crate::dispatch::gateway::manager::GatewayManager>,
    name: &str,
    input: Value,
    max_tool_calls: Option<usize>,
) -> Result<Value, ToolError> {
    let owned_manager;
    let manager = if let Some(manager) = manager {
        manager
    } else {
        owned_manager = crate::dispatch::gateway::require_gateway_manager()?;
        owned_manager.as_ref()
    };
    let registry = manager.builtin_service_registry();
    let broker = CodeModeBroker::new(&registry, Some(manager));
    let config = manager.code_mode_config().await;
    let snippet = resolve_snippet(&lab_home(), &builtin_snippet_dir(), name)?;
    let code = code_for_snippet(&snippet)?;
    let input = merge_snippet_input(&snippet, input)?;
    let code = wrap_snippet_with_input(&code, &input)?;
    let response = broker
        .execute(
            &code,
            max_tool_calls.unwrap_or(config.max_tool_calls),
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
