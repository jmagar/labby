use std::cell::RefCell;
use std::cmp::Ordering as CmpOrdering;
use std::collections::HashMap;
use std::io::{self, BufRead, BufReader, BufWriter, Write};
use std::process::ExitCode;
use std::process::Stdio;
use std::time::Duration;

use boa_engine::builtins::promise::{PromiseState, ResolvingFunctions};
use boa_engine::object::builtins::JsPromise;
use boa_engine::{
    Context, JsArgs, JsError, JsNativeError, JsResult, JsValue, NativeFunction, Source, js_string,
};
use futures::{FutureExt, StreamExt, stream::FuturesUnordered};
use lab_apis::core::action::{ActionSpec, ParamSpec};
use rmcp::model::CallToolRequestParams;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};
use tempfile::TempDir;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader as TokioBufReader};
use tokio::process::{Child, ChildStdin, Command};

use crate::dispatch::error::ToolError;
use crate::dispatch::gateway::manager::GatewayManager;
use crate::registry::{RegisteredService, ToolRegistry};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CodeModeToolId {
    pub raw: String,
    pub reference: CodeModeToolRef,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CodeModeToolRef {
    LabAction { service: String, action: String },
    UpstreamTool { upstream: String, tool: String },
}

impl CodeModeToolId {
    pub fn parse(raw: &str) -> Result<Self, ToolError> {
        let raw = raw.trim();
        if raw.is_empty() {
            return Err(invalid_code_mode_id("Code Mode tool id must not be empty"));
        }

        if let Some(rest) = raw.strip_prefix("lab::") {
            let (service, action) = rest.split_once('.').ok_or_else(|| {
                invalid_code_mode_id("lab Code Mode ids must use lab::<service>.<action>")
            })?;
            if service.trim().is_empty() || action.trim().is_empty() {
                return Err(invalid_code_mode_id(
                    "lab Code Mode ids must include service and action",
                ));
            }
            return Ok(Self {
                raw: raw.to_string(),
                reference: CodeModeToolRef::LabAction {
                    service: service.trim().to_string(),
                    action: action.trim().to_string(),
                },
            });
        }

        if let Some(rest) = raw.strip_prefix("upstream::") {
            let (upstream, tool) = rest.split_once("::").ok_or_else(|| {
                invalid_code_mode_id("upstream Code Mode ids must use upstream::<upstream>::<tool>")
            })?;
            if upstream.trim().is_empty() || tool.trim().is_empty() {
                return Err(invalid_code_mode_id(
                    "upstream Code Mode ids must include upstream and tool",
                ));
            }
            return Ok(Self {
                raw: raw.to_string(),
                reference: CodeModeToolRef::UpstreamTool {
                    upstream: upstream.trim().to_string(),
                    tool: tool.trim().to_string(),
                },
            });
        }

        Err(invalid_code_mode_id(
            "Code Mode ids must start with lab:: or upstream::",
        ))
    }
}

#[must_use]
pub fn lab_action_id(service: &str, action: &str) -> String {
    format!("lab::{service}.{action}")
}

#[must_use]
pub fn upstream_tool_id(upstream: &str, tool: &str) -> String {
    format!("upstream::{upstream}::{tool}")
}

#[must_use]
pub fn sanitize_code_mode_schema(schema: Option<Value>) -> Option<Value> {
    super::projection::sanitize_schema(schema)
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct CodeModeSearchCandidate {
    pub id: String,
    pub name: String,
    pub upstream: String,
    pub description: String,
    pub score: f32,
    pub schema_available: bool,
}

impl CodeModeSearchCandidate {
    #[must_use]
    pub fn lab_action(service: &str, action: &str, description: &str, score: f32) -> Self {
        Self {
            id: lab_action_id(service, action),
            name: action.to_string(),
            upstream: "lab".to_string(),
            description: description.to_string(),
            score,
            schema_available: true,
        }
    }

    #[must_use]
    pub fn upstream_tool(
        upstream: &str,
        tool: &str,
        description: &str,
        score: f32,
        schema: Option<Value>,
    ) -> Self {
        Self {
            id: upstream_tool_id(upstream, tool),
            name: tool.to_string(),
            upstream: upstream.to_string(),
            description: description.to_string(),
            score,
            schema_available: schema.is_some(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct CodeModeSchemaResponse {
    pub id: String,
    pub kind: &'static str,
    pub name: String,
    pub upstream: String,
    pub schema: Value,
    pub schema_format: &'static str,
    pub input_schema: Value,
    pub bindings: CodeModeBindings,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CodeModeBindings {
    pub typescript: String,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct CodeModeExecutionResponse {
    pub calls: Vec<CodeModeExecutedCall>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct CodeModeExecutedCall {
    pub id: String,
    pub result: Value,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CodeModeCaller {
    TrustedLocal,
    Scoped {
        scopes: Vec<String>,
        subject: Option<String>,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CodeModeSurface {
    Mcp {
        expose_builtin_services: bool,
        allow_destructive_actions: bool,
    },
    Cli,
}

impl CodeModeCaller {
    #[must_use]
    pub fn can_read(&self) -> bool {
        match self {
            Self::TrustedLocal => true,
            Self::Scoped { scopes, .. } => scopes
                .iter()
                .any(|scope| matches!(scope.as_str(), "lab:read" | "lab" | "lab:admin")),
        }
    }

    #[must_use]
    pub fn can_execute(&self) -> bool {
        match self {
            Self::TrustedLocal => true,
            Self::Scoped { scopes, .. } => scopes
                .iter()
                .any(|scope| matches!(scope.as_str(), "lab" | "lab:admin")),
        }
    }

    #[must_use]
    pub fn can_execute_action(&self, entry: &RegisteredService, action: &str) -> bool {
        if !builtin_action_requires_admin(entry, action) {
            return self.can_execute();
        }
        match self {
            Self::TrustedLocal => true,
            Self::Scoped { scopes, .. } => scopes.iter().any(|scope| scope == "lab:admin"),
        }
    }

    #[must_use]
    pub fn subject(&self) -> Option<&str> {
        match self {
            Self::TrustedLocal => None,
            Self::Scoped { subject, .. } => subject.as_deref(),
        }
    }
}

pub struct CodeModeBroker<'a> {
    registry: &'a ToolRegistry,
    gateway_manager: Option<&'a GatewayManager>,
}

impl<'a> CodeModeBroker<'a> {
    #[must_use]
    pub fn new(registry: &'a ToolRegistry, gateway_manager: Option<&'a GatewayManager>) -> Self {
        Self {
            registry,
            gateway_manager,
        }
    }

    pub async fn search(
        &self,
        query: &str,
        top_k: usize,
        caller: CodeModeCaller,
        surface: CodeModeSurface,
    ) -> Result<Vec<CodeModeSearchCandidate>, ToolError> {
        if !caller.can_read() {
            return Err(ToolError::Sdk {
                sdk_kind: "forbidden".to_string(),
                message: "code_search requires one of scopes: lab:read, lab, lab:admin".to_string(),
            });
        }

        let score_floor_fraction = match self.gateway_manager {
            Some(manager) => manager.tool_search_config().await.score_floor_fraction,
            None => 0.0,
        };
        let mut candidates = self
            .search_builtin_candidates(query, top_k, score_floor_fraction, surface)
            .await;

        if let Some(manager) = self.gateway_manager {
            match manager.search_tools(query, top_k, true).await {
                Ok(upstream_results) => {
                    candidates.extend(upstream_results.into_iter().map(|result| {
                        CodeModeSearchCandidate::upstream_tool(
                            &result.upstream,
                            &result.name,
                            &result.description,
                            result.score,
                            result.input_schema,
                        )
                    }));
                }
                Err(err) if err.kind() == "index_warming" && !candidates.is_empty() => {}
                Err(err) => return Err(err),
            }
        }

        candidates.sort_by(compare_code_mode_search_candidates);
        candidates.truncate(top_k.max(1).min(50));
        Ok(candidates)
    }

    pub async fn schema(
        &self,
        id: &str,
        caller: CodeModeCaller,
        surface: CodeModeSurface,
    ) -> Result<CodeModeSchemaResponse, ToolError> {
        if !caller.can_execute() {
            return Err(ToolError::Sdk {
                sdk_kind: "forbidden".to_string(),
                message: "code_schema requires one of scopes: lab, lab:admin".to_string(),
            });
        }
        let parsed = CodeModeToolId::parse(id)?;
        match parsed.reference {
            CodeModeToolRef::LabAction { service, action } => {
                self.schema_for_lab_action(&parsed.raw, &service, &action, surface)
                    .await
            }
            CodeModeToolRef::UpstreamTool { upstream, tool } => {
                self.schema_for_upstream_tool(&parsed.raw, &upstream, &tool)
                    .await
            }
        }
    }

    pub async fn execute(
        &self,
        code: &str,
        max_tool_calls: usize,
        caller: CodeModeCaller,
        surface: CodeModeSurface,
        config: crate::config::CodeModeConfig,
    ) -> Result<CodeModeExecutionResponse, ToolError> {
        if !config.enabled {
            return Err(ToolError::Sdk {
                sdk_kind: "code_mode_disabled".to_string(),
                message:
                    "Code Mode execution is disabled; set [code_mode].enabled = true to enable it"
                        .to_string(),
            });
        }
        if !caller.can_execute() {
            return Err(ToolError::Sdk {
                sdk_kind: "forbidden".to_string(),
                message: "code_execute requires one of scopes: lab, lab:admin".to_string(),
            });
        }
        self.execute_sandboxed(
            code,
            max_tool_calls.max(1).min(config.max_tool_calls.max(1)),
            Duration::from_millis(config.timeout_ms.max(1)),
            caller,
            surface,
        )
        .await
    }

    async fn search_builtin_candidates(
        &self,
        query: &str,
        top_k: usize,
        score_floor_fraction: f32,
        surface: CodeModeSurface,
    ) -> Vec<CodeModeSearchCandidate> {
        let needle = query.trim().to_ascii_lowercase();
        if needle.is_empty() || needle.len() > 500 {
            return Vec::new();
        }

        let mut candidates = Vec::new();
        for service in self.registry.services() {
            if !self.service_visible(service.name, surface).await {
                continue;
            }
            for action in self.searchable_builtin_actions(service, surface).await {
                let haystack = format!(
                    "{}\n{}\n{}\n{}",
                    service.name, service.description, action.name, action.description
                )
                .to_ascii_lowercase();
                let score = crate::dispatch::gateway::score_name_haystack(
                    &needle,
                    &action.name.to_ascii_lowercase(),
                    &haystack,
                );
                if score > 0.0 {
                    candidates.push(CodeModeSearchCandidate::lab_action(
                        service.name,
                        action.name,
                        action.description,
                        score,
                    ));
                }
            }
        }

        candidates.sort_by(compare_code_mode_search_candidates);
        if score_floor_fraction > 0.0
            && let Some(top) = candidates.first()
        {
            let floor = top.score * score_floor_fraction;
            candidates.retain(|candidate| candidate.score >= floor);
        }
        candidates.truncate(top_k.max(1).min(50));
        candidates
    }

    async fn schema_for_lab_action(
        &self,
        id: &str,
        service_name: &str,
        action_name: &str,
        surface: CodeModeSurface,
    ) -> Result<CodeModeSchemaResponse, ToolError> {
        let Some(entry) = self
            .registry
            .services()
            .iter()
            .find(|entry| entry.name == service_name)
        else {
            return Err(ToolError::Sdk {
                sdk_kind: "not_found".to_string(),
                message: format!("Lab service `{service_name}` was not found"),
            });
        };
        if !self.service_visible(entry.name, surface).await
            || !self.action_allowed(entry.name, action_name, surface).await
        {
            return Err(ToolError::Sdk {
                sdk_kind: "not_found".to_string(),
                message: format!(
                    "Lab action `{service_name}.{action_name}` is not exposed on this surface"
                ),
            });
        }
        let action = entry
            .actions
            .iter()
            .find(|action| action.name == action_name)
            .ok_or_else(|| ToolError::Sdk {
                sdk_kind: "not_found".to_string(),
                message: format!("Lab action `{service_name}.{action_name}` was not found"),
            })?;
        let input_schema = action_input_schema(action);
        crate::dispatch::helpers::action_schema(entry.actions, action_name).map(|schema| {
            CodeModeSchemaResponse::lab_action_with_input_schema(
                id,
                action_name,
                schema,
                input_schema,
            )
        })
    }

    async fn schema_for_upstream_tool(
        &self,
        id: &str,
        upstream: &str,
        tool: &str,
    ) -> Result<CodeModeSchemaResponse, ToolError> {
        let Some(manager) = self.gateway_manager else {
            return Err(ToolError::Sdk {
                sdk_kind: "upstream_error".to_string(),
                message: "gateway manager is unavailable".to_string(),
            });
        };
        let candidate = manager
            .resolve_code_mode_upstream_tool(upstream, tool)
            .await?;
        let Some(schema) = sanitize_code_mode_schema(candidate.input_schema) else {
            return Err(ToolError::Sdk {
                sdk_kind: "schema_unavailable".to_string(),
                message: format!(
                    "upstream tool `{upstream}::{tool}` schema is unavailable or exceeds the safe return size"
                ),
            });
        };
        Ok(CodeModeSchemaResponse::upstream_tool(
            id, upstream, tool, schema,
        ))
    }

    async fn execute_sandboxed(
        &self,
        code: &str,
        max_tool_calls: usize,
        timeout: Duration,
        caller: CodeModeCaller,
        surface: CodeModeSurface,
    ) -> Result<CodeModeExecutionResponse, ToolError> {
        let exe = std::env::current_exe().map_err(|err| ToolError::Sdk {
            sdk_kind: "internal_error".to_string(),
            message: format!("failed to locate current executable for Code Mode runner: {err}"),
        })?;
        let temp_dir = TempDir::new().map_err(|err| ToolError::Sdk {
            sdk_kind: "internal_error".to_string(),
            message: format!("failed to create Code Mode sandbox directory: {err}"),
        })?;
        let mut child = Command::new(exe)
            .args(["internal", "code-mode-runner"])
            .current_dir(temp_dir.path())
            .env_clear()
            .kill_on_drop(true)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|err| ToolError::Sdk {
                sdk_kind: "internal_error".to_string(),
                message: format!("failed to spawn Code Mode runner: {err}"),
            })?;

        let mut stdin = child.stdin.take().ok_or_else(|| ToolError::Sdk {
            sdk_kind: "internal_error".to_string(),
            message: "Code Mode runner stdin was not available".to_string(),
        })?;
        let stdout = child.stdout.take().ok_or_else(|| ToolError::Sdk {
            sdk_kind: "internal_error".to_string(),
            message: "Code Mode runner stdout was not available".to_string(),
        })?;
        write_runner_input(
            &mut stdin,
            &CodeModeRunnerInput::Start {
                code: code.to_string(),
            },
        )
        .await?;

        let mut lines = TokioBufReader::new(stdout).lines();
        let mut calls = Vec::new();
        let mut pending_tool_calls = FuturesUnordered::new();
        let mut started_tool_calls = 0usize;
        let deadline = tokio::time::Instant::now() + timeout;

        loop {
            tokio::select! {
                line = tokio::time::timeout_at(deadline, lines.next_line()) => {
                    let line = match line {
                        Ok(line) => line,
                        Err(_) => {
                            terminate_code_mode_runner(&mut child).await;
                            return Err(ToolError::Sdk {
                                sdk_kind: "timeout".to_string(),
                                message: "Code Mode execution timed out".to_string(),
                            });
                        }
                    };
                    let Some(line) = line.map_err(|err| ToolError::Sdk {
                        sdk_kind: "internal_error".to_string(),
                        message: format!("failed to read Code Mode runner output: {err}"),
                    })?
                    else {
                        let status = child.wait().await.map_err(|err| ToolError::Sdk {
                            sdk_kind: "internal_error".to_string(),
                            message: format!("failed to wait for Code Mode runner: {err}"),
                        })?;
                        return Err(ToolError::Sdk {
                            sdk_kind: "code_execution_failed".to_string(),
                            message: format!(
                                "Code Mode runner exited before completion with status {status}"
                            ),
                        });
                    };
                    match serde_json::from_str::<CodeModeRunnerOutput>(&line).map_err(|err| {
                        ToolError::Sdk {
                            sdk_kind: "internal_error".to_string(),
                            message: format!("Code Mode runner emitted invalid protocol JSON: {err}"),
                        }
                    })? {
                        CodeModeRunnerOutput::ToolCall { seq, id, params } => {
                            if started_tool_calls >= max_tool_calls {
                                terminate_code_mode_runner(&mut child).await;
                                return Err(ToolError::Sdk {
                                    sdk_kind: "tool_call_limit_exceeded".to_string(),
                                    message: format!(
                                        "Code Mode execution exceeded max_tool_calls={max_tool_calls}"
                                    ),
                                });
                            }
                            started_tool_calls += 1;
                            let call_id = id.clone();
                            let caller = caller.clone();
                            pending_tool_calls.push(
                                async move {
                                    let result = self
                                        .call_tool_id_before_deadline(
                                            &id, params, deadline, caller, surface,
                                        )
                                        .await;
                                    (seq, call_id, result)
                                }
                                .boxed(),
                            );
                        }
                        CodeModeRunnerOutput::Done => {
                            if !pending_tool_calls.is_empty() {
                                terminate_code_mode_runner(&mut child).await;
                                return Err(ToolError::Sdk {
                                    sdk_kind: "code_execution_failed".to_string(),
                                    message: "Code Mode runner completed with pending tool calls".to_string(),
                                });
                            }
                            if calls.is_empty() {
                                terminate_code_mode_runner(&mut child).await;
                                return Err(ToolError::Sdk {
                                    sdk_kind: "invalid_param".to_string(),
                                    message:
                                        "Code Mode snippet must call callTool(id, params) at least once"
                                            .to_string(),
                                });
                            }
                            let status = child.wait().await.map_err(|err| ToolError::Sdk {
                                sdk_kind: "internal_error".to_string(),
                                message: format!("failed to wait for Code Mode runner: {err}"),
                            })?;
                            if !status.success() {
                                return Err(ToolError::Sdk {
                                    sdk_kind: "code_execution_failed".to_string(),
                                    message: format!("Code Mode runner exited with status {status}"),
                                });
                            }
                            calls.sort_by_key(|(seq, _)| *seq);
                            return Ok(CodeModeExecutionResponse {
                                calls: calls.into_iter().map(|(_, call)| call).collect(),
                            });
                        }
                        CodeModeRunnerOutput::Error { kind, message } => {
                            drop(child.wait().await);
                            return Err(ToolError::Sdk {
                                sdk_kind: kind,
                                message,
                            });
                        }
                    }
                }
                completed = pending_tool_calls.next(), if !pending_tool_calls.is_empty() => {
                    let Some((seq, id, result)) = completed else {
                        continue;
                    };
                    let result = match result {
                        Ok(result) => result,
                        Err(err) => {
                            drop(
                                write_runner_input(
                                    &mut stdin,
                                    &CodeModeRunnerInput::ToolError {
                                        seq,
                                        kind: match &err {
                                            ToolError::Sdk { sdk_kind, .. } => {
                                                sdk_kind.as_str()
                                            }
                                            other => other.kind(),
                                        }
                                        .to_string(),
                                        message: err.to_string(),
                                    },
                                )
                                .await,
                            );
                            terminate_code_mode_runner(&mut child).await;
                            return Err(err);
                        }
                    };
                    calls.push((seq, CodeModeExecutedCall {
                        id: id.clone(),
                        result: result.clone(),
                    }));
                    write_runner_input(
                        &mut stdin,
                        &CodeModeRunnerInput::ToolResult { seq, result },
                    )
                    .await?;
                }
            }
        }
    }

    pub(crate) async fn call_tool_id_before_deadline(
        &self,
        id: &str,
        params: Value,
        deadline: tokio::time::Instant,
        caller: CodeModeCaller,
        surface: CodeModeSurface,
    ) -> Result<Value, ToolError> {
        match tokio::time::timeout_at(deadline, self.call_tool_id(id, params, caller, surface))
            .await
        {
            Ok(result) => result,
            Err(_) => Err(ToolError::Sdk {
                sdk_kind: "timeout".to_string(),
                message: "Code Mode execution timed out".to_string(),
            }),
        }
    }

    pub(crate) async fn call_tool_id(
        &self,
        id: &str,
        params: Value,
        caller: CodeModeCaller,
        surface: CodeModeSurface,
    ) -> Result<Value, ToolError> {
        let parsed = CodeModeToolId::parse(id)?;
        match parsed.reference {
            CodeModeToolRef::LabAction { service, action } => {
                self.call_lab_action(&service, &action, params, caller, surface)
                    .await
            }
            CodeModeToolRef::UpstreamTool { upstream, tool } => {
                self.call_upstream_tool(&upstream, &tool, params).await
            }
        }
    }

    async fn call_lab_action(
        &self,
        service_name: &str,
        action_name: &str,
        params: Value,
        caller: CodeModeCaller,
        surface: CodeModeSurface,
    ) -> Result<Value, ToolError> {
        let Some(entry) = self
            .registry
            .services()
            .iter()
            .find(|entry| entry.name == service_name)
        else {
            return Err(ToolError::Sdk {
                sdk_kind: "not_found".to_string(),
                message: format!("Lab service `{service_name}` was not found"),
            });
        };
        if !self.service_visible(entry.name, surface).await
            || !self.action_allowed(entry.name, action_name, surface).await
        {
            return Err(ToolError::Sdk {
                sdk_kind: "not_found".to_string(),
                message: format!(
                    "Lab action `{service_name}.{action_name}` is not exposed on this surface"
                ),
            });
        }
        if !caller.can_execute_action(entry, action_name) {
            return Err(ToolError::Sdk {
                sdk_kind: "forbidden".to_string(),
                message: format!(
                    "action `{action_name}` for service `{}` requires `lab:admin` scope",
                    entry.name
                ),
            });
        }
        let is_destructive = entry
            .actions
            .iter()
            .any(|action| action.name == action_name && action.destructive);
        let confirmed = params.get("confirm").and_then(Value::as_bool) == Some(true);
        if is_destructive && !confirmed {
            return Err(ToolError::Sdk {
                sdk_kind: "confirmation_required".to_string(),
                message: format!(
                    "action `{action_name}` is destructive - pass {{\"confirm\":true}} in params"
                ),
            });
        }
        if is_destructive && !surface.allows_destructive_actions() {
            return Err(ToolError::Sdk {
                sdk_kind: "confirmation_required".to_string(),
                message: format!(
                    "action `{action_name}` is destructive - pass {{\"confirm\":true}} to code_execute and to the tool params"
                ),
            });
        }
        let params = strip_code_mode_control_params(params);
        let params = if entry.name == "gateway" {
            inject_gateway_origin_param(params, caller.subject(), surface)
        } else {
            params
        };
        (entry.dispatch)(action_name.to_string(), params).await
    }

    async fn call_upstream_tool(
        &self,
        upstream: &str,
        tool: &str,
        params: Value,
    ) -> Result<Value, ToolError> {
        let Some(manager) = self.gateway_manager else {
            return Err(ToolError::Sdk {
                sdk_kind: "upstream_error".to_string(),
                message: "gateway manager is unavailable".to_string(),
            });
        };
        manager
            .resolve_code_mode_upstream_tool(upstream, tool)
            .await?;
        let Some(pool) = manager.current_pool().await else {
            return Err(ToolError::Sdk {
                sdk_kind: "upstream_error".to_string(),
                message: "gateway upstream pool is unavailable".to_string(),
            });
        };
        let mut upstream_params = CallToolRequestParams::new(tool.to_string());
        upstream_params.arguments = Some(match params {
            Value::Object(map) => map,
            _ => Map::new(),
        });
        match pool.call_tool(upstream, upstream_params).await {
            Some(Ok(result)) => {
                if result.is_error == Some(true) {
                    let error_text = result
                        .content
                        .first()
                        .and_then(|content| content.as_text())
                        .map(|content| content.text.as_str());
                    let (kind, message, counts_as_failure) =
                        code_mode_upstream_error_info(error_text);
                    if counts_as_failure {
                        pool.record_failure(upstream, message.clone()).await;
                    } else {
                        pool.record_success(upstream).await;
                    }
                    return Err(ToolError::Sdk {
                        sdk_kind: kind.to_string(),
                        message,
                    });
                }
                pool.record_success(upstream).await;
                serde_json::to_value(result).map_err(|err| ToolError::Sdk {
                    sdk_kind: "internal_error".to_string(),
                    message: format!("failed to serialize upstream tool result: {err}"),
                })
            }
            Some(Err(err)) => {
                pool.record_failure(upstream, err.clone()).await;
                Err(ToolError::Sdk {
                    sdk_kind: "upstream_error".to_string(),
                    message: err,
                })
            }
            None => {
                pool.record_failure(upstream, format!("upstream `{upstream}` is not connected"))
                    .await;
                Err(ToolError::Sdk {
                    sdk_kind: "not_found".to_string(),
                    message: format!("upstream tool `{upstream}::{tool}` was not found"),
                })
            }
        }
    }

    async fn searchable_builtin_actions<'b>(
        &self,
        service: &'b RegisteredService,
        surface: CodeModeSurface,
    ) -> Vec<&'b ActionSpec> {
        let mut actions = service.actions.iter().collect::<Vec<_>>();
        if let Some(allowed_actions) = self.allowed_actions(service.name, surface).await
            && !allowed_actions.is_empty()
        {
            actions.retain(|action| allowed_actions.iter().any(|allowed| allowed == action.name));
        }
        actions
    }

    async fn service_visible(&self, service: &str, surface: CodeModeSurface) -> bool {
        match (surface, self.gateway_manager) {
            (
                CodeModeSurface::Mcp {
                    expose_builtin_services: false,
                    ..
                },
                _,
            ) => false,
            (CodeModeSurface::Mcp { .. }, Some(manager)) => {
                manager.surface_enabled_for_service(service, "mcp").await
            }
            (CodeModeSurface::Cli, Some(manager)) => {
                manager.surface_enabled_for_service(service, "cli").await
            }
            _ => true,
        }
    }

    async fn action_allowed(&self, service: &str, action: &str, surface: CodeModeSurface) -> bool {
        match (surface, self.gateway_manager) {
            (CodeModeSurface::Mcp { .. }, Some(manager)) => {
                manager
                    .mcp_action_allowed_for_service(service, action)
                    .await
            }
            _ => true,
        }
    }

    async fn allowed_actions(
        &self,
        service: &str,
        surface: CodeModeSurface,
    ) -> Option<Vec<String>> {
        match (surface, self.gateway_manager) {
            (CodeModeSurface::Mcp { .. }, Some(manager)) => {
                manager.allowed_mcp_actions_for_service(service).await
            }
            _ => None,
        }
    }
}

impl CodeModeSurface {
    fn allows_destructive_actions(self) -> bool {
        match self {
            Self::Cli => true,
            Self::Mcp {
                allow_destructive_actions,
                ..
            } => allow_destructive_actions,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum CodeModeRunnerInput {
    Start {
        code: String,
    },
    ToolResult {
        seq: u64,
        result: Value,
    },
    ToolError {
        seq: u64,
        kind: String,
        message: String,
    },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum CodeModeRunnerOutput {
    ToolCall { seq: u64, id: String, params: Value },
    Done,
    Error { kind: String, message: String },
}

struct CodeModeRunnerState {
    reader: BufReader<io::Stdin>,
    writer: BufWriter<io::Stdout>,
    next_seq: u64,
    pending_calls: HashMap<u64, ResolvingFunctions>,
}

const CODE_MODE_LOOP_ITERATION_LIMIT: u64 = 1_000_000;
const CODE_MODE_STACK_SIZE_LIMIT: usize = 16 * 1024;
const CODE_MODE_RECURSION_LIMIT: usize = 256;

thread_local! {
    static RUNNER_STATE: RefCell<Option<CodeModeRunnerState>> = const { RefCell::new(None) };
}

impl CodeModeSchemaResponse {
    #[cfg(test)]
    #[must_use]
    pub fn lab_action(id: &str, action: &str, schema: Value) -> Self {
        Self::lab_action_with_input_schema(id, action, schema.clone(), schema)
    }

    #[must_use]
    pub fn lab_action_with_input_schema(
        id: &str,
        action: &str,
        schema: Value,
        input_schema: Value,
    ) -> Self {
        Self {
            id: id.to_string(),
            kind: "lab_action",
            name: action.to_string(),
            upstream: "lab".to_string(),
            schema,
            schema_format: "lab_action_spec",
            bindings: CodeModeBindings {
                typescript: typescript_binding(id, "ToolArgs", &input_schema),
            },
            input_schema,
        }
    }

    #[must_use]
    pub fn upstream_tool(id: &str, upstream: &str, tool: &str, schema: Value) -> Self {
        Self {
            id: id.to_string(),
            kind: "upstream_tool",
            name: tool.to_string(),
            upstream: upstream.to_string(),
            bindings: CodeModeBindings {
                typescript: typescript_binding(id, "ToolArgs", &schema),
            },
            input_schema: schema.clone(),
            schema,
            schema_format: "json_schema",
        }
    }
}

pub fn invalid_code_mode_id(message: impl Into<String>) -> ToolError {
    ToolError::Sdk {
        sdk_kind: "invalid_code_mode_id".to_string(),
        message: message.into(),
    }
}

fn compare_code_mode_search_candidates(
    a: &CodeModeSearchCandidate,
    b: &CodeModeSearchCandidate,
) -> CmpOrdering {
    b.score
        .partial_cmp(&a.score)
        .unwrap_or(CmpOrdering::Equal)
        .then_with(|| a.id.cmp(&b.id))
}

async fn write_runner_input(
    stdin: &mut ChildStdin,
    input: &CodeModeRunnerInput,
) -> Result<(), ToolError> {
    let mut line = serde_json::to_vec(input).map_err(|err| ToolError::Sdk {
        sdk_kind: "internal_error".to_string(),
        message: format!("failed to encode Code Mode runner input: {err}"),
    })?;
    line.push(b'\n');
    stdin.write_all(&line).await.map_err(|err| ToolError::Sdk {
        sdk_kind: "internal_error".to_string(),
        message: format!("failed to write Code Mode runner input: {err}"),
    })
}

async fn terminate_code_mode_runner(child: &mut Child) {
    drop(child.kill().await);
    drop(child.wait().await);
}

fn strip_code_mode_control_params(mut params: Value) -> Value {
    if let Value::Object(map) = &mut params {
        map.remove("confirm");
    }
    params
}

fn inject_gateway_origin_param(
    params: Value,
    subject: Option<&str>,
    surface: CodeModeSurface,
) -> Value {
    let surface_label = match surface {
        CodeModeSurface::Mcp { .. } => "mcp",
        CodeModeSurface::Cli => "cli",
    };
    let raw = subject
        .map(|value| format!("{surface_label}:{value}"))
        .unwrap_or_else(|| format!("{surface_label}:anonymous"));
    let Some(mut object) = params.as_object().cloned() else {
        return params;
    };
    object.insert(
        "owner".to_string(),
        json!({
            "surface": surface_label,
            "subject": subject,
            "raw": raw,
        }),
    );
    object.insert("origin".to_string(), Value::String(raw));
    Value::Object(object)
}

fn builtin_action_requires_admin(entry: &RegisteredService, action: &str) -> bool {
    if entry.name == "gateway" {
        return !matches!(
            action,
            "help" | "schema" | "gateway.help" | "gateway.schema"
        );
    }
    entry.name == "setup"
        && entry
            .actions
            .iter()
            .any(|spec| spec.name == action && spec.destructive)
}

fn code_mode_canonical_error_kind(s: &str) -> &'static str {
    match s {
        "unknown_action" => "unknown_action",
        "unknown_subaction" => "unknown_subaction",
        "missing_param" => "missing_param",
        "invalid_param" => "invalid_param",
        "unknown_instance" => "unknown_instance",
        "confirmation_required" => "confirmation_required",
        "conflict" => "conflict",
        "auth_failed" => "auth_failed",
        "not_found" => "not_found",
        "rate_limited" => "rate_limited",
        "validation_failed" => "validation_failed",
        "network_error" => "network_error",
        "server_error" => "server_error",
        "decode_error" => "decode_error",
        "internal_error" => "internal_error",
        "upstream_error" => "upstream_error",
        _ => "internal_error",
    }
}

fn code_mode_upstream_error_info(text: Option<&str>) -> (&'static str, String, bool) {
    let Some(text) = text else {
        return (
            "upstream_error",
            "upstream returned a non-text error payload".to_string(),
            true,
        );
    };

    let Ok(parsed) = serde_json::from_str::<Value>(text) else {
        return ("upstream_error", text.to_string(), true);
    };

    let error_obj = parsed
        .get("error")
        .and_then(Value::as_object)
        .or_else(|| parsed.as_object());
    let Some(error_obj) = error_obj else {
        return ("upstream_error", text.to_string(), true);
    };

    let kind = error_obj
        .get("kind")
        .and_then(Value::as_str)
        .map(code_mode_canonical_error_kind)
        .unwrap_or("upstream_error");
    let message = error_obj
        .get("message")
        .and_then(Value::as_str)
        .unwrap_or(text)
        .to_string();
    let counts_as_failure = matches!(
        kind,
        "upstream_error" | "network_error" | "server_error" | "decode_error" | "internal_error"
    );

    (kind, message, counts_as_failure)
}

pub fn run_code_mode_runner_stdio() -> ExitCode {
    RUNNER_STATE.with(|state| {
        *state.borrow_mut() = Some(CodeModeRunnerState {
            reader: BufReader::new(io::stdin()),
            writer: BufWriter::new(io::stdout()),
            next_seq: 0,
            pending_calls: HashMap::new(),
        });
    });

    let result = run_code_mode_runner();
    if let Err(err) = result {
        drop(runner_emit(CodeModeRunnerOutput::Error {
            kind: "code_execution_failed".to_string(),
            message: err,
        }));
        return ExitCode::from(1);
    }
    ExitCode::SUCCESS
}

fn run_code_mode_runner() -> Result<(), String> {
    let CodeModeRunnerInput::Start { code } = runner_read_input()? else {
        return Err("runner expected start message".to_string());
    };

    let mut context = Context::default();
    configure_code_mode_runtime_limits(&mut context);
    context
        .register_global_builtin_callable(
            js_string!("callTool"),
            2,
            NativeFunction::from_copy_closure(code_mode_call_tool_native),
        )
        .map_err(js_error_message)?;

    let wrapped = format!("(async () => {{\n{code}\n}})()");
    let value = context
        .eval(Source::from_bytes(wrapped.as_bytes()))
        .map_err(js_error_message)?;
    let object = value
        .as_object()
        .ok_or_else(|| "Code Mode script did not return a promise".to_string())?;
    let promise = JsPromise::from_object(object.clone()).map_err(js_error_message)?;

    loop {
        context.run_jobs().map_err(js_error_message)?;

        match promise.state() {
            PromiseState::Fulfilled(_) => break,
            PromiseState::Rejected(reason) => return Err(js_value_message(&reason, &mut context)),
            PromiseState::Pending => {
                let input = runner_read_input()?;
                settle_code_mode_tool_promise(input, &mut context)?;
            }
        }
    }

    runner_emit(CodeModeRunnerOutput::Done)
}

fn configure_code_mode_runtime_limits(context: &mut Context) {
    let limits = context.runtime_limits_mut();
    limits.set_loop_iteration_limit(CODE_MODE_LOOP_ITERATION_LIMIT);
    limits.set_stack_size_limit(CODE_MODE_STACK_SIZE_LIMIT);
    limits.set_recursion_limit(CODE_MODE_RECURSION_LIMIT);
}

fn code_mode_call_tool_native(
    _this: &JsValue,
    args: &[JsValue],
    context: &mut Context,
) -> JsResult<JsValue> {
    let id = args
        .get_or_undefined(0)
        .to_string(context)?
        .to_std_string_escaped();
    if id.trim().is_empty() {
        return Err(js_type_error("callTool id must be a non-empty string"));
    }

    let params = args
        .get(1)
        .map(|value| value.to_json(context))
        .transpose()?
        .flatten()
        .unwrap_or_else(|| json!({}));
    if !params.is_object() {
        return Err(js_type_error("callTool params must be a JSON object"));
    }

    let (promise, resolvers) = JsPromise::new_pending(context);
    let seq = RUNNER_STATE
        .with(|state| {
            let mut state = state.borrow_mut();
            let state = state
                .as_mut()
                .ok_or_else(|| "runner state is not initialized".to_string())?;
            let seq = state.next_seq;
            state.next_seq += 1;
            state.pending_calls.insert(seq, resolvers);
            Ok::<_, String>(seq)
        })
        .map_err(js_type_error)?;

    runner_emit(CodeModeRunnerOutput::ToolCall { seq, id, params }).map_err(js_type_error)?;
    Ok(promise.into())
}

fn settle_code_mode_tool_promise(
    input: CodeModeRunnerInput,
    context: &mut Context,
) -> Result<(), String> {
    let (seq, result) = match input {
        CodeModeRunnerInput::ToolResult { seq, result } => (seq, Ok(result)),
        CodeModeRunnerInput::ToolError { seq, kind, message } => {
            (seq, Err(format!("{kind}: {message}")))
        }
        CodeModeRunnerInput::Start { .. } => {
            return Err("runner received unexpected start message".to_string());
        }
    };

    let resolvers = RUNNER_STATE.with(|state| {
        let mut state = state.borrow_mut();
        let state = state
            .as_mut()
            .ok_or_else(|| "runner state is not initialized".to_string())?;
        state
            .pending_calls
            .remove(&seq)
            .ok_or_else(|| "runner received a response for an unknown tool call".to_string())
    })?;

    match result {
        Ok(result) => {
            let value = JsValue::from_json(&result, context).map_err(js_error_message)?;
            resolvers
                .resolve
                .call(&JsValue::undefined(), &[value], context)
                .map_err(js_error_message)?;
        }
        Err(message) => {
            let reason = JsValue::from(js_string!(message.as_str()));
            resolvers
                .reject
                .call(&JsValue::undefined(), &[reason], context)
                .map_err(js_error_message)?;
        }
    }
    Ok(())
}

fn runner_emit(output: CodeModeRunnerOutput) -> Result<(), String> {
    RUNNER_STATE.with(|state| {
        let mut state = state.borrow_mut();
        let state = state
            .as_mut()
            .ok_or_else(|| "runner state is not initialized".to_string())?;
        serde_json::to_writer(&mut state.writer, &output).map_err(|err| err.to_string())?;
        state
            .writer
            .write_all(b"\n")
            .map_err(|err| err.to_string())?;
        state.writer.flush().map_err(|err| err.to_string())
    })
}

fn runner_read_input() -> Result<CodeModeRunnerInput, String> {
    RUNNER_STATE.with(|state| {
        let mut state = state.borrow_mut();
        let state = state
            .as_mut()
            .ok_or_else(|| "runner state is not initialized".to_string())?;
        let mut line = String::new();
        let read = state
            .reader
            .read_line(&mut line)
            .map_err(|err| err.to_string())?;
        if read == 0 {
            return Err("runner input closed".to_string());
        }
        serde_json::from_str(&line).map_err(|err| err.to_string())
    })
}

fn js_type_error(message: impl Into<String>) -> JsError {
    JsNativeError::typ().with_message(message.into()).into()
}

fn js_error_message(error: JsError) -> String {
    error.to_string()
}

fn js_value_message(value: &JsValue, context: &mut Context) -> String {
    value
        .to_string(context)
        .map(|value| value.to_std_string_escaped())
        .unwrap_or_else(|_| "promise rejected".to_string())
}

#[must_use]
pub fn action_input_schema(action: &ActionSpec) -> Value {
    let mut properties = Map::new();
    let mut required = Vec::new();

    for param in action.params {
        let mut schema = param_json_schema(param);
        if let Value::Object(map) = &mut schema
            && !param.description.is_empty()
        {
            map.insert(
                "description".to_string(),
                Value::String(param.description.to_string()),
            );
        }
        properties.insert(param.name.to_string(), schema);
        if param.required {
            required.push(Value::String(param.name.to_string()));
        }
    }

    let mut schema = Map::from_iter([
        ("type".to_string(), Value::String("object".to_string())),
        ("properties".to_string(), Value::Object(properties)),
        ("additionalProperties".to_string(), Value::Bool(false)),
    ]);
    if !required.is_empty() {
        schema.insert("required".to_string(), Value::Array(required));
    }
    Value::Object(schema)
}

fn param_json_schema(param: &ParamSpec) -> Value {
    let ty = param.ty.trim();
    if let Some(item) = ty.strip_suffix("[]") {
        return json!({
            "type": "array",
            "items": type_label_json_schema(item)
        });
    }
    if ty.contains('|')
        && ty.split('|').all(|part| {
            !matches!(
                part.trim(),
                "string" | "number" | "integer" | "boolean" | "object" | "array" | "null"
            )
        })
    {
        return json!({
            "type": "string",
            "enum": ty.split('|').map(str::trim).collect::<Vec<_>>()
        });
    }
    if ty.contains('|') {
        return json!({
            "anyOf": ty.split('|').map(|part| type_label_json_schema(part.trim())).collect::<Vec<_>>()
        });
    }
    type_label_json_schema(ty)
}

fn type_label_json_schema(ty: &str) -> Value {
    match ty {
        "string" => json!({ "type": "string" }),
        "integer" | "int" | "i64" | "u64" | "usize" => json!({ "type": "integer" }),
        "number" | "float" | "f64" => json!({ "type": "number" }),
        "boolean" | "bool" => json!({ "type": "boolean" }),
        "object" | "json" | "value" => json!({ "type": "object" }),
        "array" | "list" => json!({ "type": "array" }),
        "null" => json!({ "type": "null" }),
        _ => json!({ "description": format!("Lab type hint: {ty}") }),
    }
}

#[must_use]
pub fn typescript_binding(id: &str, type_name: &str, schema: &Value) -> String {
    let args_type = typescript_type(schema, 0);
    format!(
        "export type {type_name} = {args_type};\n\n\
         export interface CodeModeToolCaller {{\n  callTool<T = unknown>(id: string, args: unknown): Promise<T>;\n}}\n\n\
         export async function call(caller: CodeModeToolCaller, args: {type_name}): Promise<unknown> {{\n  return caller.callTool({id_literal}, args);\n}}\n",
        id_literal = json!(id)
    )
}

fn typescript_type(schema: &Value, indent: usize) -> String {
    if let Some(values) = schema.get("enum").and_then(Value::as_array) {
        let literals = values
            .iter()
            .filter_map(Value::as_str)
            .map(|value| json!(value).to_string())
            .collect::<Vec<_>>();
        if !literals.is_empty() {
            return literals.join(" | ");
        }
    }
    if let Some(any_of) = schema.get("anyOf").and_then(Value::as_array) {
        return any_of
            .iter()
            .map(|schema| typescript_type(schema, indent))
            .collect::<Vec<_>>()
            .join(" | ");
    }
    match schema.get("type").and_then(Value::as_str) {
        Some("string") => "string".to_string(),
        Some("integer" | "number") => "number".to_string(),
        Some("boolean") => "boolean".to_string(),
        Some("null") => "null".to_string(),
        Some("array") => {
            let item = schema
                .get("items")
                .map(|items| typescript_type(items, indent))
                .unwrap_or_else(|| "unknown".to_string());
            format!("{item}[]")
        }
        Some("object") => object_typescript_type(schema, indent),
        _ => "unknown".to_string(),
    }
}

fn object_typescript_type(schema: &Value, indent: usize) -> String {
    let Some(properties) = schema.get("properties").and_then(Value::as_object) else {
        return "Record<string, unknown>".to_string();
    };
    if properties.is_empty() {
        return "Record<string, never>".to_string();
    }
    let required = schema
        .get("required")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .collect::<std::collections::BTreeSet<_>>();
    let pad = " ".repeat(indent);
    let child_pad = " ".repeat(indent + 2);
    let mut lines = vec!["{".to_string()];
    for (name, property_schema) in properties {
        let optional = if required.contains(name.as_str()) {
            ""
        } else {
            "?"
        };
        lines.push(format!(
            "{child_pad}{}{optional}: {};",
            typescript_property_name(name),
            typescript_type(property_schema, indent + 2)
        ));
    }
    lines.push(format!("{pad}}}"));
    lines.join("\n")
}

fn typescript_property_name(name: &str) -> String {
    let mut chars = name.chars();
    let valid_first = chars
        .next()
        .is_some_and(|ch| ch == '_' || ch == '$' || ch.is_ascii_alphabetic());
    let valid_rest = chars.all(|ch| ch == '_' || ch == '$' || ch.is_ascii_alphanumeric());
    if valid_first && valid_rest {
        name.to_string()
    } else {
        json!(name).to_string()
    }
}

#[cfg(test)]
mod tests {
    use boa_engine::{Context, Source};
    use serde_json::json;
    use std::future::Future;
    use std::pin::Pin;

    use super::{
        CodeModeSchemaResponse, CodeModeSearchCandidate, CodeModeToolId, CodeModeToolRef,
        action_input_schema, code_mode_upstream_error_info, configure_code_mode_runtime_limits,
        sanitize_code_mode_schema,
    };
    use crate::dispatch::error::ToolError;
    use crate::registry::{RegisteredService, RegisteredServiceKind, ToolRegistry};
    use lab_apis::core::action::{ActionSpec, ParamSpec};

    #[test]
    fn parses_lab_action_id() {
        let parsed = CodeModeToolId::parse("lab::gateway.gateway.schema").unwrap();
        assert_eq!(
            parsed,
            CodeModeToolId {
                raw: "lab::gateway.gateway.schema".to_string(),
                reference: CodeModeToolRef::LabAction {
                    service: "gateway".to_string(),
                    action: "gateway.schema".to_string(),
                },
            }
        );
    }

    #[test]
    fn parses_upstream_tool_id() {
        let parsed = CodeModeToolId::parse("upstream::github::search_issues").unwrap();
        assert_eq!(
            parsed,
            CodeModeToolId {
                raw: "upstream::github::search_issues".to_string(),
                reference: CodeModeToolRef::UpstreamTool {
                    upstream: "github".to_string(),
                    tool: "search_issues".to_string(),
                },
            }
        );
    }

    #[test]
    fn rejects_invalid_ids() {
        for id in [
            "",
            "gateway.gateway.schema",
            "lab::gateway",
            "upstream::github",
            "upstream::::tool",
        ] {
            assert!(CodeModeToolId::parse(id).is_err(), "{id} should be invalid");
        }
    }

    #[test]
    fn upstream_error_info_preserves_user_error_kinds() {
        let text = json!({
            "error": {
                "kind": "missing_param",
                "message": "query is required",
                "param": "query"
            }
        })
        .to_string();

        let (kind, message, counts_as_failure) = code_mode_upstream_error_info(Some(&text));

        assert_eq!(kind, "missing_param");
        assert_eq!(message, "query is required");
        assert!(!counts_as_failure);
    }

    const DESTRUCTIVE_ACTIONS: &[ActionSpec] = &[ActionSpec {
        name: "danger",
        description: "Dangerous test action",
        destructive: true,
        params: &[],
        returns: "object",
    }];

    fn echo_dispatch(
        _action: String,
        params: serde_json::Value,
    ) -> Pin<Box<dyn Future<Output = Result<serde_json::Value, ToolError>> + Send>> {
        Box::pin(async move { Ok(params) })
    }

    fn destructive_test_registry() -> ToolRegistry {
        let mut registry = ToolRegistry::new();
        registry.register(RegisteredService {
            name: "gateway",
            description: "Gateway",
            category: "bootstrap",
            kind: RegisteredServiceKind::BootstrapOperator,
            status: "available",
            actions: DESTRUCTIVE_ACTIONS,
            dispatch: echo_dispatch,
        });
        registry
    }

    #[tokio::test]
    async fn mcp_code_mode_requires_top_level_confirmation_for_destructive_actions() {
        let registry = destructive_test_registry();
        let broker = super::CodeModeBroker::new(&registry, None);

        let err = broker
            .call_tool_id(
                "lab::gateway.danger",
                json!({"confirm": true}),
                super::CodeModeCaller::TrustedLocal,
                super::CodeModeSurface::Mcp {
                    expose_builtin_services: true,
                    allow_destructive_actions: false,
                },
            )
            .await
            .expect_err("mcp destructive action should require top-level code_execute confirm");

        assert_eq!(err.kind(), "confirmation_required");
    }

    #[tokio::test]
    async fn code_mode_overwrites_gateway_provenance_fields() {
        let registry = destructive_test_registry();
        let broker = super::CodeModeBroker::new(&registry, None);

        let result = broker
            .call_tool_id(
                "lab::gateway.danger",
                json!({
                    "confirm": true,
                    "origin": "spoofed",
                    "owner": {"raw": "spoofed"}
                }),
                super::CodeModeCaller::Scoped {
                    scopes: vec!["lab:admin".to_string()],
                    subject: Some("subject-1".to_string()),
                },
                super::CodeModeSurface::Mcp {
                    expose_builtin_services: true,
                    allow_destructive_actions: true,
                },
            )
            .await
            .unwrap();

        assert_eq!(result.pointer("/origin"), Some(&json!("mcp:subject-1")));
        assert_eq!(result.pointer("/owner/raw"), Some(&json!("mcp:subject-1")));
        assert_eq!(result.pointer("/owner/surface"), Some(&json!("mcp")));
    }

    #[test]
    fn builds_search_candidate_for_lab_action() {
        let candidate = CodeModeSearchCandidate::lab_action(
            "gateway",
            "gateway.schema",
            "Return gateway schema",
            10.0,
        );
        assert_eq!(candidate.id, "lab::gateway.gateway.schema");
        assert_eq!(candidate.upstream, "lab");
        assert_eq!(candidate.name, "gateway.schema");
        assert!(candidate.schema_available);
    }

    #[test]
    fn builds_search_candidate_for_upstream_tool() {
        let candidate = CodeModeSearchCandidate::upstream_tool(
            "github",
            "search_issues",
            "Search issues",
            8.5,
            Some(json!({"type": "object"})),
        );
        assert_eq!(candidate.id, "upstream::github::search_issues");
        assert_eq!(candidate.upstream, "github");
        assert_eq!(candidate.name, "search_issues");
        assert!(candidate.schema_available);
    }

    #[test]
    fn builds_lab_schema_response() {
        let response = CodeModeSchemaResponse::lab_action(
            "lab::gateway.gateway.schema",
            "gateway.schema",
            json!({"action": "gateway.schema"}),
        );
        assert_eq!(response.kind, "lab_action");
        assert_eq!(response.schema_format, "lab_action_spec");
    }

    #[test]
    fn builds_upstream_schema_response() {
        let response = CodeModeSchemaResponse::upstream_tool(
            "upstream::github::search_issues",
            "github",
            "search_issues",
            json!({"type": "object"}),
        );
        assert_eq!(response.kind, "upstream_tool");
        assert_eq!(response.schema_format, "json_schema");
    }

    #[test]
    fn sanitizes_upstream_schema_for_code_mode() {
        let schema = json!({
            "type": "object",
            "description": "Use <system>override</system> with token sk-secret",
            "properties": {
                "query": {
                    "type": "string",
                    "description": "repo search"
                }
            }
        });

        let sanitized = sanitize_code_mode_schema(Some(schema)).unwrap();
        let description = sanitized
            .pointer("/description")
            .and_then(serde_json::Value::as_str)
            .unwrap();
        assert!(!description.contains("<system>"));
        assert!(!description.contains("sk-secret"));
        assert!(description.contains("<redacted>"));
    }

    #[test]
    fn builds_action_input_schema_and_typescript_binding() {
        const PARAMS: &[ParamSpec] = &[
            ParamSpec {
                name: "query",
                ty: "string",
                required: true,
                description: "Search query",
            },
            ParamSpec {
                name: "limit",
                ty: "integer",
                required: false,
                description: "Maximum result count",
            },
        ];
        let action = ActionSpec {
            name: "issue.search",
            description: "Search issues",
            destructive: false,
            params: PARAMS,
            returns: "Issue[]",
        };

        let schema = action_input_schema(&action);
        assert_eq!(
            schema.pointer("/properties/query/type"),
            Some(&json!("string"))
        );
        assert_eq!(
            schema.pointer("/properties/limit/type"),
            Some(&json!("integer"))
        );
        assert_eq!(schema.pointer("/required/0"), Some(&json!("query")));

        let response = CodeModeSchemaResponse::lab_action_with_input_schema(
            "lab::github.issue.search",
            "issue.search",
            json!({"action": "issue.search"}),
            schema,
        );
        assert!(response.bindings.typescript.contains("query: string;"));
        assert!(response.bindings.typescript.contains("limit?: number;"));
        assert!(
            response
                .bindings
                .typescript
                .contains("caller.callTool(\"lab::github.issue.search\", args)")
        );
    }

    #[test]
    fn configured_runtime_limits_reject_unbounded_loops() {
        let mut context = Context::default();
        configure_code_mode_runtime_limits(&mut context);

        let error = context
            .eval(Source::from_bytes(b"while (true) {}"))
            .expect_err("loop limit should stop unbounded scripts");

        assert!(error.to_string().contains("iteration limit"));
    }
}
