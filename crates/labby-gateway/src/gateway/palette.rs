//! Palette-facing launcher catalog and execution helpers.
//!
//! This module owns the gateway portion of the desktop launcher contract. It
//! projects already-discovered upstream MCP tools without cold-connecting, then
//! re-resolves the live tool at execution time before validating parameters and
//! dispatching through the same upstream call helper used by Code Mode.

use std::collections::{BTreeMap, BTreeSet};
use std::io::{self, Write};
use std::time::{Duration, Instant};

use labby_codemode::{CodeModeCaller, CodeModeCallerCapabilities, ToolCallOutcome, ToolScope};
use labby_runtime::caller_auth::{PropagatedCallerAuth, PropagatedCallerUpstreamScope};
use labby_runtime::error::ToolError;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};

use crate::gateway::SHARED_GATEWAY_OAUTH_SUBJECT;
use crate::gateway::manager::GatewayManager;
use crate::gateway::projection::sanitize_tool_text;
use crate::upstream::types::{UpstreamRuntimeOwner, UpstreamTool};

const MAX_SCHEMA_BYTES: usize = 64 * 1024;
const MAX_SCHEMA_DEPTH: usize = 64;
const MAX_CONTRACT_BYTES: usize = 160 * 1024;
const MAX_CATALOG_ENTRIES: usize = 1_000;
const CAPABILITY_CONTRACT_VERSION: u8 = 1;
const MAX_DESCRIPTION_CHARS: usize = 2_048;
const MAX_PALETTE_QUERY_CHARS: usize = 256;
const PALETTE_OAUTH_DISCOVERY_DEADLINE: Duration = Duration::from_secs(2);
const MAX_PALETTE_SEARCH_INSPECTIONS: usize = 10_000;

#[derive(Debug, Clone)]
pub struct PaletteSearchQuery {
    normalized: String,
}

impl PaletteSearchQuery {
    pub fn new(query: &str) -> Result<Self, ToolError> {
        let query = query.trim();
        if query.chars().count() > MAX_PALETTE_QUERY_CHARS {
            return Err(ToolError::Sdk {
                sdk_kind: "invalid_params".to_string(),
                message: format!(
                    "palette query exceeds the {MAX_PALETTE_QUERY_CHARS}-character limit"
                ),
            });
        }
        let normalized = query.to_ascii_lowercase();
        Ok(Self { normalized })
    }

    pub fn as_str(&self) -> &str {
        &self.normalized
    }

    pub fn is_empty(&self) -> bool {
        self.normalized.is_empty()
    }

    fn matches_tool(&self, upstream: &str, tool: &rmcp::model::Tool) -> bool {
        self.score_tool(upstream, tool) > 0
    }

    fn score_tool(&self, upstream: &str, tool: &rmcp::model::Tool) -> u16 {
        let name = tool.name.as_ref();
        let id = format!("mcp:{upstream}::{name}");
        let description = sanitize_tool_text(tool.description.as_deref().unwrap_or(""), 512);
        self.score_fields([id.as_str(), name, description.as_str(), upstream])
    }

    pub fn score_entry(&self, entry: &LauncherEntryView) -> u16 {
        match entry {
            LauncherEntryView::LabbyAction(entry) => self.score_fields([
                entry.id.as_str(),
                entry.label.as_str(),
                entry.description.as_str(),
                entry.source.as_str(),
                entry.service.as_str(),
                entry.action.as_str(),
            ]),
            LauncherEntryView::McpTool(entry) => self.score_fields([
                entry.id.as_str(),
                entry.label.as_str(),
                entry.description.as_str(),
                entry.source.as_str(),
                entry.upstream.as_str(),
                entry.tool.as_str(),
            ]),
        }
    }

    fn score_fields<'a>(&self, fields: impl IntoIterator<Item = &'a str>) -> u16 {
        if self.is_empty() {
            return 1;
        }
        fields
            .into_iter()
            .map(|field| palette_field_score(field, &self.normalized))
            .max()
            .unwrap_or(0)
    }
}

fn palette_field_score(field: &str, needle: &str) -> u16 {
    let field = field.to_ascii_lowercase();
    if field == needle {
        100
    } else if field.starts_with(needle) {
        80
    } else if field
        .split([' ', ':', '.', '_', '-'])
        .any(|part| part.starts_with(needle))
    {
        60
    } else if field.contains(needle) {
        30
    } else if is_subsequence(needle, &field) {
        10
    } else {
        0
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct CapabilityAnnotations {
    pub read_only_hint: Option<bool>,
    pub destructive_hint: Option<bool>,
    pub idempotent_hint: Option<bool>,
    pub open_world_hint: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct CapabilityContract {
    pub contract_version: u8,
    pub id: String,
    pub input_schema: Option<Value>,
    pub output_schema: Option<Value>,
    pub annotations: CapabilityAnnotations,
    pub destructive: bool,
    pub contract_hash: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct CapabilityDescriptor {
    pub contract_version: u8,
    pub catalog_revision: String,
    pub id: String,
    pub upstream: String,
    pub tool: String,
    pub description: String,
    pub input_schema: Option<Value>,
    pub output_schema: Option<Value>,
    pub annotations: CapabilityAnnotations,
    pub destructive: bool,
    pub contract_hash: String,
}

impl CapabilityContract {
    pub fn from_upstream_tool(tool: &UpstreamTool) -> Result<Self, ToolError> {
        validate_contract_schema(tool.input_schema.as_ref())?;
        validate_contract_schema(tool.output_schema.as_ref())?;
        let annotations = tool.tool.annotations.as_ref();
        let mut contract = Self {
            contract_version: CAPABILITY_CONTRACT_VERSION,
            id: format!("mcp:{}::{}", tool.upstream_name, tool.tool.name),
            input_schema: project_contract_schema(tool.input_schema.as_ref())?,
            output_schema: project_contract_schema(tool.output_schema.as_ref())?,
            annotations: CapabilityAnnotations {
                read_only_hint: annotations.and_then(|value| value.read_only_hint),
                destructive_hint: annotations.and_then(|value| value.destructive_hint),
                idempotent_hint: annotations.and_then(|value| value.idempotent_hint),
                open_world_hint: annotations.and_then(|value| value.open_world_hint),
            },
            destructive: tool.destructive,
            contract_hash: String::new(),
        };
        contract.contract_hash =
            contract.compute_hash(tool.input_schema.as_ref(), tool.output_schema.as_ref())?;
        Ok(contract)
    }

    /// Hash the exact executable contract without applying palette display-size caps.
    pub(crate) fn execution_hash_from_upstream_tool(
        tool: &UpstreamTool,
    ) -> Result<String, ToolError> {
        if tool
            .input_schema
            .as_ref()
            .is_some_and(schema_depth_exceeds_limit)
            || tool
                .output_schema
                .as_ref()
                .is_some_and(schema_depth_exceeds_limit)
        {
            return Err(descriptor_unsupported());
        }

        let annotations = tool.tool.annotations.as_ref();
        let contract = Self {
            contract_version: CAPABILITY_CONTRACT_VERSION,
            id: format!("mcp:{}::{}", tool.upstream_name, tool.tool.name),
            input_schema: None,
            output_schema: None,
            annotations: CapabilityAnnotations {
                read_only_hint: annotations.and_then(|value| value.read_only_hint),
                destructive_hint: annotations.and_then(|value| value.destructive_hint),
                idempotent_hint: annotations.and_then(|value| value.idempotent_hint),
                open_world_hint: annotations.and_then(|value| value.open_world_hint),
            },
            destructive: tool.destructive,
            contract_hash: String::new(),
        };
        let mut writer = CappedHashWriter::new(usize::MAX);
        write_contract_canonical(
            &mut writer,
            &contract,
            tool.input_schema.as_ref(),
            tool.output_schema.as_ref(),
        )
        .map_err(|error| ToolError::Sdk {
            sdk_kind: "invalid_tool_schema".to_string(),
            message: format!("failed to hash capability contract: {error}"),
        })?;
        Ok(hex_digest(&writer.finish()))
    }

    fn compute_hash(
        &self,
        exact_input_schema: Option<&Value>,
        exact_output_schema: Option<&Value>,
    ) -> Result<String, ToolError> {
        let mut writer = CappedHashWriter::new(MAX_CONTRACT_BYTES);
        write_contract_canonical(&mut writer, self, exact_input_schema, exact_output_schema)
            .map_err(|_| descriptor_unsupported())?;
        Ok(hex_digest(&writer.finish()))
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct LauncherCatalogView {
    pub fingerprint: String,
    pub entries: Vec<LauncherEntryView>,
    #[serde(default)]
    pub truncated: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", tag = "kind")]
pub enum LauncherEntryView {
    LabbyAction(LabbyActionLauncherEntry),
    McpTool(McpToolLauncherEntry),
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct LabbyActionLauncherEntry {
    pub id: String,
    pub label: String,
    pub description: String,
    pub source: String,
    pub destructive: bool,
    pub input_schema: Option<Value>,
    pub schema_fingerprint: Option<String>,
    pub contract_hash: String,
    pub service: String,
    pub action: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct McpToolLauncherEntry {
    pub id: String,
    pub label: String,
    pub description: String,
    pub source: String,
    pub destructive: bool,
    pub input_schema: Option<Value>,
    pub schema_fingerprint: Option<String>,
    pub contract_hash: String,
    pub upstream: String,
    pub tool: String,
}

#[derive(Debug, Clone)]
pub struct PaletteCaller {
    pub caller: CodeModeCaller,
    pub caller_auth: PropagatedCallerAuth,
    pub scope: ToolScope,
    pub owner: UpstreamRuntimeOwner,
    pub oauth_subject: String,
}

impl PaletteCaller {
    #[must_use]
    pub fn admin(subject: Option<&str>, request_id: Option<&str>) -> Self {
        let owner = crate::gateway::shared::make_api_runtime_owner(subject, request_id);
        let subject = subject.map(ToOwned::to_owned);
        let oauth_subject = subject
            .clone()
            .unwrap_or_else(|| SHARED_GATEWAY_OAUTH_SUBJECT.to_string());
        Self {
            caller: CodeModeCaller::Scoped {
                capabilities: CodeModeCallerCapabilities {
                    can_read: true,
                    can_execute: true,
                    can_use_snippets: false,
                    is_admin: true,
                },
                sub: subject.clone(),
            },
            caller_auth: PropagatedCallerAuth {
                sub: subject.clone(),
                scopes: vec!["lab:admin".to_string()],
                trusted_local: false,
                access_principal_id: None,
                private_context_token: None,
            },
            scope: ToolScope::default(),
            owner,
            oauth_subject,
        }
    }

    #[must_use]
    pub fn scoped_read_only(
        subject: Option<&str>,
        request_id: Option<&str>,
        allowed_upstreams: Vec<String>,
    ) -> Self {
        let owner = crate::gateway::shared::make_api_runtime_owner(subject, request_id);
        let subject = subject.map(ToOwned::to_owned);
        let scopes = std::iter::once("mcp:read".to_string())
            .chain(
                allowed_upstreams
                    .iter()
                    .map(|name| format!("gateway:{name}")),
            )
            .collect();
        Self {
            caller: CodeModeCaller::Scoped {
                capabilities: CodeModeCallerCapabilities {
                    can_read: true,
                    can_execute: false,
                    can_use_snippets: false,
                    is_admin: false,
                },
                sub: subject.clone(),
            },
            caller_auth: PropagatedCallerAuth {
                sub: subject.clone(),
                scopes,
                trusted_local: false,
                access_principal_id: None,
                private_context_token: None,
            },
            scope: ToolScope::scoped_namespaces(allowed_upstreams, Vec::new()).read_only(),
            owner,
            oauth_subject: subject.unwrap_or_else(|| SHARED_GATEWAY_OAUTH_SUBJECT.to_string()),
        }
    }

    #[must_use]
    pub fn scoped(
        subject: &str,
        request_id: Option<&str>,
        scopes: Vec<String>,
        allowed_upstreams: Vec<String>,
    ) -> Self {
        let can_read = scopes.iter().any(|scope| scope == "mcp:read");
        let can_execute =
            scopes.iter().any(|scope| scope == "mcp:write") && !allowed_upstreams.is_empty();
        Self {
            caller: CodeModeCaller::Scoped {
                capabilities: CodeModeCallerCapabilities {
                    can_read,
                    can_execute,
                    can_use_snippets: false,
                    is_admin: false,
                },
                sub: Some(subject.to_string()),
            },
            caller_auth: PropagatedCallerAuth {
                sub: Some(subject.to_string()),
                scopes,
                trusted_local: false,
                access_principal_id: None,
                private_context_token: None,
            },
            scope: ToolScope::scoped_namespaces(allowed_upstreams, Vec::new()),
            owner: crate::gateway::shared::make_api_runtime_owner(Some(subject), request_id),
            oauth_subject: subject.to_string(),
        }
    }

    fn allowed_upstreams(&self) -> Option<&BTreeSet<String>> {
        self.scope.allowed_namespaces()
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PaletteExecuteRequest {
    pub id: String,
    #[serde(default)]
    pub params: Value,
    pub expected_contract_hash: String,
    #[serde(default)]
    pub confirm_destructive: bool,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct PaletteExecuteResponse {
    pub id: String,
    pub result: Value,
    pub receipt: PaletteExecutionReceipt,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ui: Option<Value>,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct PaletteExecutionReceipt {
    pub request_id: String,
    pub tool_id: String,
    pub contract_hash: String,
    pub catalog_revision: String,
    pub execution_mode: PaletteExecutionMode,
    pub truncated: bool,
}

/// The launcher dispatch path, not a claim about work inside the selected tool.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum PaletteExecutionMode {
    Exact,
    LabbyAction,
}

impl GatewayManager {
    pub async fn palette_descriptor(
        &self,
        caller: &PaletteCaller,
        id: &str,
    ) -> Result<CapabilityDescriptor, ToolError> {
        if !caller.caller.can_read() {
            return Err(ToolError::Sdk {
                sdk_kind: "forbidden".to_string(),
                message: "palette descriptor requires mcp:read permission".to_string(),
            });
        }
        let (upstream, tool_name) = parse_mcp_launcher_id(id)?;
        if caller
            .allowed_upstreams()
            .is_some_and(|allowed| !allowed.contains(upstream))
        {
            return Err(ToolError::Sdk {
                sdk_kind: "not_found".to_string(),
                message: format!("launcher entry `{id}` was not found"),
            });
        }
        let tool = self
            .resolve_code_mode_upstream_tool(
                upstream,
                tool_name,
                Some(&caller.owner),
                Some(&caller.oauth_subject),
            )
            .await
            .map_err(map_unknown_tool_to_not_found)?;
        let contract = CapabilityContract::from_upstream_tool(&tool)?;
        let (_, pool) = self.published_config_and_pool().await;
        let descriptor = CapabilityDescriptor {
            contract_version: contract.contract_version,
            catalog_revision: pool
                .map(|pool| pool.revision_label())
                .unwrap_or_else(|| "unavailable".to_string()),
            id: contract.id,
            upstream: upstream.to_string(),
            tool: tool_name.to_string(),
            description: sanitize_tool_text(
                tool.tool.description.as_deref().unwrap_or(""),
                MAX_DESCRIPTION_CHARS,
            ),
            input_schema: contract.input_schema,
            output_schema: contract.output_schema,
            annotations: contract.annotations,
            destructive: contract.destructive,
            contract_hash: contract.contract_hash,
        };
        let mut writer = CountingWriter::new(MAX_CONTRACT_BYTES);
        serde_json::to_writer(&mut writer, &descriptor).map_err(|_| descriptor_unsupported())?;
        Ok(descriptor)
    }

    pub async fn palette_catalog(
        &self,
        caller: &PaletteCaller,
    ) -> Result<LauncherCatalogView, ToolError> {
        self.palette_catalog_inner(caller, true, None, None).await
    }

    pub async fn palette_catalog_snapshot(
        &self,
        caller: &PaletteCaller,
    ) -> Result<LauncherCatalogView, ToolError> {
        self.palette_catalog_inner(caller, false, None, None).await
    }

    /// Search every bounded upstream catalog before applying the global
    /// launcher cap, so a match is not hidden merely by upstream ordering.
    pub async fn palette_catalog_snapshot_matching(
        &self,
        caller: &PaletteCaller,
        query: &PaletteSearchQuery,
    ) -> Result<LauncherCatalogView, ToolError> {
        self.palette_catalog_inner(caller, false, None, Some(query))
            .await
    }

    /// Read one caller-visible tool from the published catalog without connecting
    /// upstreams or requiring Code Mode execution to be enabled.
    pub async fn palette_catalog_snapshot_for_tool(
        &self,
        caller: &PaletteCaller,
        id: &str,
    ) -> Result<LauncherCatalogView, ToolError> {
        let selected = parse_mcp_launcher_id(id)?;
        self.palette_catalog_inner(caller, false, Some(selected), None)
            .await
    }

    async fn palette_catalog_inner(
        &self,
        caller: &PaletteCaller,
        refresh: bool,
        selected: Option<(&str, &str)>,
        query: Option<&PaletteSearchQuery>,
    ) -> Result<LauncherCatalogView, ToolError> {
        if !caller.caller.can_read() {
            return Err(ToolError::Sdk {
                sdk_kind: "forbidden".to_string(),
                message: "palette catalog requires mcp:read permission".to_string(),
            });
        }
        let start = Instant::now();
        let mut entries = Vec::new();
        let mut ranked_entries = Vec::<(LauncherEntryView, u16)>::new();
        let mut truncated = false;

        if refresh {
            self.refresh_code_mode_catalog_allowed(
                Some(&caller.owner),
                Some(&caller.oauth_subject),
                caller.allowed_upstreams(),
            )
            .await?;
        }
        let (cfg, pool) = self.published_config_and_pool().await;
        if let Some(pool) = pool {
            let eligible = cfg.upstream.iter().filter(|upstream| {
                upstream.enabled
                    && upstream.priority > 0.0
                    && selected.is_none_or(|(name, _)| upstream.name == name)
                    && caller
                        .allowed_upstreams()
                        .is_none_or(|allowed| allowed.contains(&upstream.name))
            });
            let oauth_configs = eligible
                .clone()
                .filter(|upstream| upstream.oauth.is_some() && selected.is_none())
                .cloned()
                .collect::<Vec<_>>();
            let (oauth_tools, oauth_inspected, oauth_incomplete) = if oauth_configs.is_empty() {
                (Vec::new(), 0, false)
            } else if let Some(query) = query {
                let result = pool
                    .subject_scoped_upstream_tools_allowed_matching_bounded(
                        &oauth_configs,
                        &caller.oauth_subject,
                        None,
                        MAX_PALETTE_SEARCH_INSPECTIONS,
                        &|upstream, tool| query.matches_tool(upstream, tool),
                        PALETTE_OAUTH_DISCOVERY_DEADLINE,
                    )
                    .await;
                (result.tools, result.inspected, result.incomplete)
            } else {
                (
                    pool.subject_scoped_upstream_tools_allowed_bounded(
                        &oauth_configs,
                        &caller.oauth_subject,
                        None,
                        MAX_CATALOG_ENTRIES.saturating_add(1),
                    )
                    .await,
                    0,
                    false,
                )
            };
            truncated |= oauth_incomplete;
            let mut oauth_tools = oauth_tools.into_iter().fold(
                BTreeMap::<String, Vec<UpstreamTool>>::new(),
                |mut grouped, tool| {
                    grouped
                        .entry(tool.upstream_name.to_string())
                        .or_default()
                        .push(tool);
                    grouped
                },
            );
            let mut remaining_inspections =
                MAX_PALETTE_SEARCH_INSPECTIONS.saturating_sub(oauth_inspected);
            for upstream in eligible {
                let remaining = if query.is_some() {
                    MAX_CATALOG_ENTRIES
                } else {
                    MAX_CATALOG_ENTRIES.saturating_sub(entries.len())
                };
                let discovery_limit = remaining.saturating_add(1);
                let tools = if upstream.oauth.is_some()
                    && let Some((_, tool_name)) = selected
                {
                    pool.subject_scoped_upstream_tool_allowed(
                        upstream,
                        &caller.oauth_subject,
                        tool_name,
                    )
                    .await
                    .into_iter()
                    .collect()
                } else if upstream.oauth.is_some() {
                    oauth_tools.remove(&upstream.name).unwrap_or_default()
                } else if let Some((_, tool_name)) = selected {
                    pool.healthy_tool_for_upstream(&upstream.name, tool_name)
                        .await
                        .into_iter()
                        .collect()
                } else if let Some(query) = query {
                    if remaining_inspections == 0 {
                        truncated = true;
                        continue;
                    }
                    let (tools, inspected, exhausted) = pool
                        .healthy_tools_for_upstream_ranked_bounded(
                            &upstream.name,
                            discovery_limit,
                            remaining_inspections,
                            |tool| query.score_tool(&upstream.name, &tool.tool),
                        )
                        .await;
                    remaining_inspections = remaining_inspections.saturating_sub(inspected);
                    truncated |= exhausted;
                    tools.into_iter().map(|(tool, _)| tool).collect()
                } else {
                    pool.healthy_tools_for_upstream_bounded(&upstream.name, discovery_limit)
                        .await
                };
                if query.is_none() && selected.is_none() && tools.len() > remaining {
                    let mut tools = tools;
                    tools.truncate(remaining);
                    truncated = true;
                    for tool in tools {
                        let entry = mcp_entry(&upstream.name, tool)?;
                        entries.push(LauncherEntryView::McpTool(entry));
                    }
                    break;
                }
                for tool in tools {
                    if selected.is_some_and(|(_, name)| tool.tool.name.as_ref() != name) {
                        continue;
                    }
                    let entry = mcp_entry(&upstream.name, tool)?;
                    let entry = LauncherEntryView::McpTool(entry);
                    if let Some(query) = query {
                        insert_ranked_palette_entry(&mut ranked_entries, entry, query);
                    } else {
                        entries.push(entry);
                    }
                }
            }
        }

        if query.is_some() {
            truncated |= ranked_entries.len() > MAX_CATALOG_ENTRIES;
            ranked_entries.truncate(MAX_CATALOG_ENTRIES);
            entries = ranked_entries.into_iter().map(|(entry, _)| entry).collect();
        }

        entries.sort_by(|a, b| entry_id(a).cmp(entry_id(b)));
        let fingerprint = catalog_fingerprint(&entries);
        tracing::info!(
            surface = "api",
            service = "palette",
            action = "palette.catalog",
            entry_count = entries.len(),
            fingerprint,
            cache_hit = false,
            truncated,
            elapsed_ms = start.elapsed().as_millis(),
            "palette launcher catalog built"
        );
        Ok(LauncherCatalogView {
            fingerprint,
            entries,
            truncated,
        })
    }

    pub async fn palette_execute(
        &self,
        caller: &PaletteCaller,
        request: PaletteExecuteRequest,
    ) -> Result<PaletteExecuteResponse, ToolError> {
        self.palette_execute_with_consumed_approval(caller, request, false)
            .await
    }

    pub(crate) async fn palette_execute_with_consumed_approval(
        &self,
        caller: &PaletteCaller,
        request: PaletteExecuteRequest,
        consumed_server_approval: bool,
    ) -> Result<PaletteExecuteResponse, ToolError> {
        let start = Instant::now();
        let tool_id = request.id.clone();
        let (upstream, tool) = parse_mcp_launcher_id(&tool_id)?;
        validate_contract_hash(&request.expected_contract_hash)?;
        let contract_hash = request.expected_contract_hash.clone();
        let result = async {
            if caller
                .allowed_upstreams()
                .is_some_and(|allowed| !allowed.contains(upstream))
            {
                return Err(ToolError::Sdk {
                    sdk_kind: "forbidden".to_string(),
                    message: format!("upstream `{upstream}` is outside the caller scope"),
                });
            }
            if !caller.caller.can_execute() {
                return Err(ToolError::Sdk {
                    sdk_kind: "forbidden".to_string(),
                    message: "palette execution requires execute permission".to_string(),
                });
            }

            let destructive_allowed = consumed_server_approval
                || (caller.caller.is_admin() && request.confirm_destructive);
            let destructive_denial_kind = if caller.caller.is_admin() || consumed_server_approval {
                "confirmation_required"
            } else {
                "forbidden"
            };
            let checked = self
                .execute_upstream_tool_checked(
                    upstream,
                    tool,
                    request.params,
                    &caller.owner,
                    Some(&caller.oauth_subject),
                    Some(caller.caller_auth.clone()),
                    Some(PropagatedCallerUpstreamScope::new(
                        caller.scope.allowed_namespaces().cloned(),
                    )),
                    &contract_hash,
                    destructive_allowed,
                    destructive_denial_kind,
                )
                .await
                .map_err(map_unknown_tool_to_not_found)?;
            let receipt = PaletteExecutionReceipt {
                request_id: caller
                    .owner
                    .request_id
                    .clone()
                    .unwrap_or_else(|| "unavailable".to_string()),
                tool_id: tool_id.clone(),
                contract_hash: checked.contract_hash,
                catalog_revision: checked.catalog_revision,
                execution_mode: PaletteExecutionMode::Exact,
                truncated: false,
            };
            Ok(execution_response(
                tool_id.clone(),
                checked.outcome,
                receipt,
            ))
        };
        let result = result.await;
        let catalog_revision = result
            .as_ref()
            .ok()
            .map(|response| response.receipt.catalog_revision.as_str())
            .unwrap_or("unavailable");
        log_palette_execution(
            caller,
            upstream,
            tool,
            &contract_hash,
            catalog_revision,
            result
                .as_ref()
                .map_or_else(|error| error.kind(), |_| "success"),
            start.elapsed(),
        );
        result
    }

    pub async fn palette_schema(
        &self,
        caller: &PaletteCaller,
        id: &str,
    ) -> Result<Option<Value>, ToolError> {
        if !caller.caller.can_read() {
            return Err(ToolError::Sdk {
                sdk_kind: "forbidden".to_string(),
                message: "palette schema requires mcp:read permission".to_string(),
            });
        }
        let start = Instant::now();
        let (upstream, tool) = parse_mcp_launcher_id(id)?;
        if caller
            .allowed_upstreams()
            .is_some_and(|allowed| !allowed.contains(upstream))
        {
            return Err(ToolError::Sdk {
                sdk_kind: "not_found".to_string(),
                message: format!("launcher entry `{id}` was not found"),
            });
        }
        let upstream_tool = self
            .resolve_code_mode_upstream_tool(
                upstream,
                tool,
                Some(&caller.owner),
                Some(&caller.oauth_subject),
            )
            .await
            .map_err(map_unknown_tool_to_not_found)?;
        let schema = project_palette_schema(upstream_tool.input_schema);
        tracing::info!(
            surface = "api",
            service = "palette",
            action = "palette.schema",
            upstream,
            tool,
            has_schema = schema.is_some(),
            schema_bytes = schema
                .as_ref()
                .map(|schema| schema.to_string().len())
                .unwrap_or(0),
            elapsed_ms = start.elapsed().as_millis(),
            "palette launcher schema resolved"
        );
        Ok(schema)
    }
}

fn insert_ranked_palette_entry(
    entries: &mut Vec<(LauncherEntryView, u16)>,
    entry: LauncherEntryView,
    query: &PaletteSearchQuery,
) {
    let score = query.score_entry(&entry);
    let insert_at = entries
        .binary_search_by(|(existing, existing_score)| {
            existing_score
                .cmp(&score)
                .reverse()
                .then_with(|| entry_id(existing).cmp(entry_id(&entry)))
        })
        .unwrap_or_else(std::convert::identity);
    if insert_at <= MAX_CATALOG_ENTRIES {
        entries.insert(insert_at, (entry, score));
        if entries.len() > MAX_CATALOG_ENTRIES.saturating_add(1) {
            entries.pop();
        }
    }
}

fn is_subsequence(needle: &str, haystack: &str) -> bool {
    let mut chars = needle.chars();
    let mut next = chars.next();
    for character in haystack.chars() {
        if next == Some(character) {
            next = chars.next();
        }
    }
    next.is_none()
}

fn mcp_entry(upstream: &str, tool: UpstreamTool) -> Result<McpToolLauncherEntry, ToolError> {
    let contract = CapabilityContract::from_upstream_tool(&tool)?;
    let name = tool.tool.name.to_string();
    let input_schema = project_palette_schema(tool.input_schema);
    let schema_fingerprint = input_schema.as_ref().map(stable_json_fingerprint);
    Ok(McpToolLauncherEntry {
        id: format!("mcp:{upstream}::{name}"),
        label: name.clone(),
        description: sanitize_tool_text(
            tool.tool
                .description
                .as_ref()
                .map(|value| value.as_ref())
                .unwrap_or(""),
            512,
        ),
        source: upstream.to_string(),
        destructive: tool.destructive,
        // Catalog rows are bounded display hints. The exact schema is fetched
        // through `palette_schema` only after the operator selects an entry.
        input_schema: None,
        schema_fingerprint,
        contract_hash: contract.contract_hash,
        upstream: upstream.to_string(),
        tool: name,
    })
}

fn execution_response(
    id: String,
    outcome: ToolCallOutcome,
    receipt: PaletteExecutionReceipt,
) -> PaletteExecuteResponse {
    PaletteExecuteResponse {
        id,
        result: outcome.value,
        receipt,
        ui: outcome.ui.map(|ui| ui.ui_meta),
    }
}

fn parse_mcp_launcher_id(id: &str) -> Result<(&str, &str), ToolError> {
    let rest = id.strip_prefix("mcp:").ok_or_else(|| ToolError::Sdk {
        sdk_kind: "not_found".to_string(),
        message: format!("launcher entry `{id}` was not found"),
    })?;
    let Some((upstream, tool)) = rest.split_once("::") else {
        return Err(ToolError::Sdk {
            sdk_kind: "not_found".to_string(),
            message: format!("launcher entry `{id}` was not found"),
        });
    };
    if upstream.is_empty() || tool.is_empty() || tool.contains("::") {
        return Err(ToolError::Sdk {
            sdk_kind: "not_found".to_string(),
            message: format!("launcher entry `{id}` was not found"),
        });
    }
    Ok((upstream, tool))
}

fn map_unknown_tool_to_not_found(error: ToolError) -> ToolError {
    match error {
        ToolError::Sdk { sdk_kind, message }
            if sdk_kind == "unknown_tool"
                || sdk_kind == "unknown_upstream"
                || sdk_kind == "invalid_code_mode_id" =>
        {
            ToolError::Sdk {
                sdk_kind: "not_found".to_string(),
                message,
            }
        }
        other => other,
    }
}

fn project_palette_schema(schema: Option<Value>) -> Option<Value> {
    let mut schema = schema?;
    if schema_depth_exceeds_limit(&schema) {
        return None;
    }
    redact_schema_value(&mut schema);
    if schema.to_string().len() > MAX_SCHEMA_BYTES {
        return None;
    }
    Some(schema)
}

fn project_contract_schema(schema: Option<&Value>) -> Result<Option<Value>, ToolError> {
    let Some(schema) = schema else {
        return Ok(None);
    };
    let mut projected = schema.clone();
    redact_schema_value(&mut projected);
    let mut writer = CountingWriter::new(MAX_SCHEMA_BYTES);
    serde_json::to_writer(&mut writer, &projected).map_err(|_| descriptor_unsupported())?;
    Ok(Some(projected))
}

fn validate_contract_schema(schema: Option<&Value>) -> Result<(), ToolError> {
    let Some(schema) = schema else {
        return Ok(());
    };
    if schema_depth_exceeds_limit(schema) {
        return Err(descriptor_unsupported());
    }
    let mut writer = CountingWriter::new(MAX_SCHEMA_BYTES);
    serde_json::to_writer(&mut writer, schema).map_err(|_| descriptor_unsupported())
}

fn schema_depth_exceeds_limit(schema: &Value) -> bool {
    let mut pending = vec![(schema, 1usize)];
    while let Some((value, depth)) = pending.pop() {
        if depth > MAX_SCHEMA_DEPTH {
            return true;
        }
        match value {
            Value::Object(map) => pending.extend(map.values().filter_map(|child| {
                matches!(child, Value::Object(_) | Value::Array(_)).then_some((child, depth + 1))
            })),
            Value::Array(values) => pending.extend(values.iter().filter_map(|child| {
                matches!(child, Value::Object(_) | Value::Array(_)).then_some((child, depth + 1))
            })),
            _ => {}
        }
    }
    false
}

fn descriptor_unsupported() -> ToolError {
    ToolError::Sdk {
        sdk_kind: "descriptor_unsupported".to_string(),
        message: "capability descriptor exceeds the v1 contract limits".to_string(),
    }
}

fn validate_contract_hash(hash: &str) -> Result<(), ToolError> {
    if hash.len() == 64
        && hash
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return Ok(());
    }
    Err(ToolError::Sdk {
        sdk_kind: "invalid_param".to_string(),
        message: "expectedContractHash must be 64 lowercase hexadecimal characters".to_string(),
    })
}

fn log_palette_execution(
    caller: &PaletteCaller,
    upstream: &str,
    tool: &str,
    contract_hash: &str,
    catalog_revision: &str,
    kind: &str,
    elapsed: Duration,
) {
    let request_id = caller.owner.request_id.as_deref().unwrap_or("unavailable");
    let subject_fingerprint = labby_auth::util::fingerprint(&caller.oauth_subject);
    tracing::info!(
        surface = "api",
        service = "palette",
        action = "palette.execute",
        request_id,
        upstream,
        tool,
        subject_fingerprint,
        contract_hash,
        catalog_revision,
        elapsed_ms = elapsed.as_millis(),
        kind,
        "palette launcher execution completed"
    );
}

fn redact_schema_value(value: &mut Value) {
    match value {
        Value::Object(map) => {
            map.remove("default");
            map.remove("examples");
            map.remove("example");
            for (key, child) in map.iter_mut() {
                if secret_key(key) {
                    *child = Value::String("[REDACTED]".to_string());
                } else {
                    redact_schema_value(child);
                }
            }
        }
        Value::Array(values) => {
            values.retain(|value| !secret_enum_value(value));
            for child in values {
                redact_schema_value(child);
            }
        }
        Value::String(text) => {
            *text = sanitize_tool_text(text, 512);
        }
        _ => {}
    }
}

fn secret_key(key: &str) -> bool {
    let key = key.to_ascii_lowercase();
    key.contains("token")
        || key.contains("secret")
        || key.contains("password")
        || key.contains("apikey")
        || key.contains("api_key")
        || key.contains("authorization")
}

fn secret_enum_value(value: &Value) -> bool {
    value
        .as_str()
        .is_some_and(|text| secret_key(text) || text.starts_with("sk-") || text.starts_with("ghp_"))
}

fn entry_id(entry: &LauncherEntryView) -> &str {
    match entry {
        LauncherEntryView::LabbyAction(entry) => &entry.id,
        LauncherEntryView::McpTool(entry) => &entry.id,
    }
}

fn catalog_fingerprint(entries: &[LauncherEntryView]) -> String {
    let mut hasher = Sha256::new();
    for entry in entries {
        hasher.update(entry_id(entry).as_bytes());
        hasher.update([0]);
        match entry {
            LauncherEntryView::LabbyAction(entry) => {
                hasher.update(entry.contract_hash.as_bytes());
            }
            LauncherEntryView::McpTool(entry) => {
                hasher.update(entry.contract_hash.as_bytes());
            }
        }
        hasher.update([0xff]);
    }
    hex_digest(hasher.finalize().as_slice())
}

fn stable_json_fingerprint(value: &Value) -> String {
    let mut hasher = Sha256::new();
    hasher.update(value.to_string().as_bytes());
    hex_digest(hasher.finalize().as_slice())
}

fn hex_digest(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        out.push(HEX[(byte >> 4) as usize] as char);
        out.push(HEX[(byte & 0x0f) as usize] as char);
    }
    out
}

struct CountingWriter {
    written: usize,
    cap: usize,
}

impl CountingWriter {
    fn new(cap: usize) -> Self {
        Self { written: 0, cap }
    }
}

impl Write for CountingWriter {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        if self.written.saturating_add(bytes.len()) > self.cap {
            return Err(io::Error::other("serialized value exceeds cap"));
        }
        self.written += bytes.len();
        Ok(bytes.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

struct CappedHashWriter {
    hasher: Sha256,
    written: usize,
    cap: usize,
}

impl CappedHashWriter {
    fn new(cap: usize) -> Self {
        Self {
            hasher: Sha256::new(),
            written: 0,
            cap,
        }
    }

    fn finish(self) -> [u8; 32] {
        self.hasher.finalize().into()
    }
}

impl Write for CappedHashWriter {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        if self.written.saturating_add(bytes.len()) > self.cap {
            return Err(io::Error::other("canonical contract exceeds cap"));
        }
        self.hasher.update(bytes);
        self.written += bytes.len();
        Ok(bytes.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

fn write_contract_canonical(
    writer: &mut impl Write,
    contract: &CapabilityContract,
    exact_input_schema: Option<&Value>,
    exact_output_schema: Option<&Value>,
) -> io::Result<()> {
    writer.write_all(b"{\"annotations\":{")?;
    write_optional_bool(
        writer,
        "destructiveHint",
        contract.annotations.destructive_hint,
    )?;
    writer.write_all(b",")?;
    write_optional_bool(
        writer,
        "idempotentHint",
        contract.annotations.idempotent_hint,
    )?;
    writer.write_all(b",")?;
    write_optional_bool(
        writer,
        "openWorldHint",
        contract.annotations.open_world_hint,
    )?;
    writer.write_all(b",")?;
    write_optional_bool(writer, "readOnlyHint", contract.annotations.read_only_hint)?;
    writer.write_all(b"},\"contractVersion\":")?;
    serde_json::to_writer(&mut *writer, &contract.contract_version)?;
    writer.write_all(b",\"destructive\":")?;
    serde_json::to_writer(&mut *writer, &contract.destructive)?;
    writer.write_all(b",\"id\":")?;
    serde_json::to_writer(&mut *writer, &contract.id)?;
    writer.write_all(b",\"inputSchema\":")?;
    write_optional_json_canonical(writer, exact_input_schema)?;
    writer.write_all(b",\"outputSchema\":")?;
    write_optional_json_canonical(writer, exact_output_schema)?;
    writer.write_all(b"}")
}

fn write_optional_bool(writer: &mut impl Write, key: &str, value: Option<bool>) -> io::Result<()> {
    serde_json::to_writer(&mut *writer, key)?;
    writer.write_all(b":")?;
    serde_json::to_writer(writer, &value).map_err(io::Error::other)
}

fn write_optional_json_canonical(writer: &mut impl Write, value: Option<&Value>) -> io::Result<()> {
    match value {
        Some(value) => write_json_canonical(writer, value),
        None => writer.write_all(b"null"),
    }
}

fn write_json_canonical(writer: &mut impl Write, value: &Value) -> io::Result<()> {
    match value {
        Value::Null | Value::Bool(_) | Value::Number(_) | Value::String(_) => {
            serde_json::to_writer(writer, value).map_err(io::Error::other)
        }
        Value::Array(values) => {
            writer.write_all(b"[")?;
            for (index, value) in values.iter().enumerate() {
                if index > 0 {
                    writer.write_all(b",")?;
                }
                write_json_canonical(writer, value)?;
            }
            writer.write_all(b"]")
        }
        Value::Object(map) => {
            writer.write_all(b"{")?;
            let mut keys = map.keys().collect::<Vec<_>>();
            keys.sort_unstable();
            for (index, key) in keys.into_iter().enumerate() {
                if index > 0 {
                    writer.write_all(b",")?;
                }
                serde_json::to_writer(&mut *writer, key).map_err(io::Error::other)?;
                writer.write_all(b":")?;
                write_json_canonical(writer, &map[key])?;
            }
            writer.write_all(b"}")
        }
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::disallowed_methods)] // test fixtures construct upstream Tool values directly

    use super::*;
    use serde_json::json;
    use std::sync::Arc;
    use tracing_subscriber::layer::SubscriberExt;

    #[test]
    fn palette_search_query_normalizes_once_and_rejects_oversize_input() {
        let query = PaletteSearchQuery::new("  DePloY Safe  ").expect("valid query");
        assert_eq!(query.as_str(), "deploy safe");
        assert!(query.score_fields(["Deploy Production Safely"]) > 0);
        assert!(PaletteSearchQuery::new(&"x".repeat(257)).is_err());
    }

    #[test]
    fn palette_schema_projection_redacts_defaults_examples_and_secret_enums() {
        let projected = project_palette_schema(Some(json!({
            "type": "object",
            "default": { "token": "sk-secret" },
            "examples": [{ "token": "sk-secret" }],
            "properties": {
                "apiKey": {
                    "type": "string",
                    "enum": ["public", "sk-secretsecretsecretsecret"]
                },
                "name": { "type": "string" }
            }
        })))
        .expect("schema remains");

        assert!(projected.get("default").is_none());
        assert!(projected.get("examples").is_none());
        assert_eq!(
            projected.pointer("/properties/apiKey"),
            Some(&Value::String("[REDACTED]".to_string()))
        );
    }

    #[test]
    fn capability_contract_hash_matches_the_v1_canonical_vector() {
        let upstream_name = Arc::<str>::from("alpha");
        let tool = UpstreamTool {
            tool: rmcp::model::Tool::new(
                "ping".to_string(),
                "description is display-only",
                Arc::new(serde_json::Map::new()),
            ),
            input_schema: Some(json!({
                "type": "object",
                "properties": {"q": {"type": "string"}}
            })),
            output_schema: None,
            upstream_name,
            destructive: false,
        };

        let contract = CapabilityContract::from_upstream_tool(&tool).expect("bounded contract");

        assert_eq!(contract.contract_version, 1);
        assert_eq!(contract.id, "mcp:alpha::ping");
        assert_eq!(
            contract.contract_hash,
            "f54cdd4d74a33e09b603d2856fdfeb1f706d22c08cb7c386cfb7aa8354528ddf"
        );
    }

    #[test]
    fn capability_contract_hash_excludes_description_but_covers_safety_hints() {
        let make = |description: &str, destructive: bool| {
            let mut tool = rmcp::model::Tool::new(
                "ping".to_string(),
                description.to_string(),
                Arc::new(serde_json::Map::new()),
            );
            tool.annotations = Some(
                rmcp::model::ToolAnnotations::new()
                    .read_only(!destructive)
                    .destructive(destructive),
            );
            UpstreamTool {
                tool,
                input_schema: Some(json!({"type": "object"})),
                output_schema: None,
                upstream_name: Arc::from("alpha"),
                destructive,
            }
        };

        let first =
            CapabilityContract::from_upstream_tool(&make("first", false)).expect("first contract");
        let renamed = CapabilityContract::from_upstream_tool(&make("renamed", false))
            .expect("renamed contract");
        let destructive = CapabilityContract::from_upstream_tool(&make("first", true))
            .expect("destructive contract");

        assert_eq!(first.contract_hash, renamed.contract_hash);
        assert_ne!(first.contract_hash, destructive.contract_hash);
    }

    #[test]
    fn capability_contract_hash_covers_sensitive_property_validation_semantics() {
        let make = |property: &str, schema: Value| UpstreamTool {
            tool: rmcp::model::Tool::new(
                "ping".to_string(),
                "ping",
                Arc::new(serde_json::Map::new()),
            ),
            input_schema: Some(json!({
                "type": "object",
                "properties": {property: schema}
            })),
            output_schema: None,
            upstream_name: Arc::from("alpha"),
            destructive: false,
        };

        for property in ["apiKey", "token", "password"] {
            let string_contract = CapabilityContract::from_upstream_tool(&make(
                property,
                json!({"type": "string", "minLength": 8}),
            ))
            .expect("string contract");
            let integer_contract = CapabilityContract::from_upstream_tool(&make(
                property,
                json!({"type": "integer", "minimum": 1}),
            ))
            .expect("integer contract");
            assert_ne!(
                string_contract.contract_hash, integer_contract.contract_hash,
                "{property} validation changes must alter the contract hash"
            );
        }
    }

    #[test]
    fn capability_contract_accepts_depth_64_and_rejects_depth_65() {
        fn nested_schema(depth: usize) -> Value {
            let mut schema = json!({"type": "string"});
            for _ in 1..depth {
                schema = json!({"nested": schema});
            }
            schema
        }

        let make = |depth| UpstreamTool {
            tool: rmcp::model::Tool::new(
                "ping".to_string(),
                "ping",
                Arc::new(serde_json::Map::new()),
            ),
            input_schema: Some(nested_schema(depth)),
            output_schema: None,
            upstream_name: Arc::from("alpha"),
            destructive: false,
        };

        CapabilityContract::from_upstream_tool(&make(64)).expect("depth 64 is supported");
        let error = CapabilityContract::from_upstream_tool(&make(65))
            .expect_err("depth 65 must fail before recursive projection");
        assert_eq!(error.kind(), "descriptor_unsupported");
    }

    #[test]
    fn capability_contract_rejects_an_oversized_schema_instead_of_hashing_null() {
        let enum_values = (0..200)
            .map(|index| format!("{index:03}-{}", "x".repeat(509)))
            .collect::<Vec<_>>();
        let tool = UpstreamTool {
            tool: rmcp::model::Tool::new(
                "ping".to_string(),
                "ping",
                Arc::new(serde_json::Map::new()),
            ),
            input_schema: Some(json!({"type": "string", "enum": enum_values})),
            output_schema: None,
            upstream_name: Arc::from("alpha"),
            destructive: false,
        };

        let error = CapabilityContract::from_upstream_tool(&tool)
            .expect_err("oversized schemas fail explicitly");
        assert_eq!(error.kind(), "descriptor_unsupported");
    }

    #[test]
    fn capability_contract_v1_golden_vectors_match_canonical_json_and_sha256() {
        let fixture: Value = serde_json::from_str(include_str!(
            "../../tests/fixtures/capability-contract-v1.json"
        ))
        .expect("golden fixture parses");
        let cases = fixture["cases"].as_array().expect("cases array");
        assert!(!cases.is_empty());
        for case in cases {
            let name = case["name"].as_str().expect("case name");
            let mut canonical = Vec::new();
            let result = write_json_canonical(&mut canonical, &case["normalizedInput"]);
            assert!(
                result.is_ok(),
                "{name}: canonicalization failed: {result:?}"
            );
            let canonical = String::from_utf8(canonical).expect("canonical JSON is UTF-8");
            assert_eq!(canonical, case["canonicalJson"], "{name}: canonical JSON");
            let digest = Sha256::digest(canonical.as_bytes());
            assert_eq!(
                hex_digest(digest.as_slice()),
                case["expectedSha256"],
                "{name}: SHA-256"
            );
        }
    }

    #[test]
    fn palette_execution_telemetry_is_complete_and_redacted_for_every_outcome() {
        let _tracing_lock = crate::test_support::TRACING_TEST_LOCK
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let buffer = crate::test_support::SharedBuf::default();
        let subscriber = tracing_subscriber::registry().with(
            tracing_subscriber::fmt::layer()
                .json()
                .with_writer(buffer.clone())
                .with_ansi(false)
                .without_time(),
        );
        let _guard = tracing::subscriber::set_default(subscriber);
        let caller = PaletteCaller::admin(Some("raw-subject-CANARY"), Some("req-telemetry-123"));
        let contract_hash = "a".repeat(64);
        for kind in [
            "success",
            "not_found",
            "contract_changed",
            "invalid_param",
            "auth_failed",
            "timeout",
            "upstream_error",
        ] {
            log_palette_execution(
                &caller,
                "github",
                "search_issues",
                &contract_hash,
                "catalog-revision-1",
                kind,
                Duration::from_millis(7),
            );
        }
        drop(_guard);

        let logs = crate::test_support::captured_logs(&buffer);
        let subject_fingerprint = labby_auth::util::fingerprint("raw-subject-CANARY");
        for expected in [
            "req-telemetry-123",
            "github",
            "search_issues",
            &subject_fingerprint,
            &contract_hash,
            "elapsed_ms",
            "success",
            "not_found",
            "contract_changed",
            "invalid_param",
            "auth_failed",
            "timeout",
            "upstream_error",
        ] {
            assert!(
                logs.contains(expected),
                "missing `{expected}` from logs: {logs}"
            );
        }
        for forbidden in [
            "raw-subject-CANARY",
            "TOKEN-CANARY",
            "params",
            "inputSchema",
            "result-CANARY",
            "oauth-CANARY",
        ] {
            assert!(
                !logs.contains(forbidden),
                "logs leaked `{forbidden}`: {logs}"
            );
        }
    }
}
