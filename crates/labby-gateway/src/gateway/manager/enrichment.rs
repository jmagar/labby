use labby_runtime::error::ToolError;

use crate::gateway::enrichment::collector::{
    EnrichmentInputStats, MAX_MANUAL_UPSTREAMS, SelectedUpstream, UpstreamEnrichmentInput,
    collect_enrichment_inputs, select_upstreams_for_preview,
};
use crate::gateway::enrichment::provider::{ProviderRunner, run_provider_preview};
use crate::gateway::params::{GatewayEnrichApplyParams, GatewayEnrichPreviewParams};
use crate::gateway::types::{
    GatewayCatalogDiff, GatewayEnrichmentPreviewStatsView, GatewayEnrichmentPreviewView,
    GatewayEnrichmentProvider, GatewayHintApplyView, GatewayHintProposalStatus,
    GatewayHintProposalView,
};

use super::GatewayManager;

impl From<EnrichmentInputStats> for GatewayEnrichmentPreviewStatsView {
    fn from(stats: EnrichmentInputStats) -> Self {
        Self {
            bytes: stats.bytes,
            upstream_count: stats.upstream_count,
            tool_count: stats.tool_count,
            truncated: stats.truncated,
        }
    }
}

impl GatewayManager {
    pub async fn preview_enrichment(
        &self,
        mut params: GatewayEnrichPreviewParams,
    ) -> Result<GatewayEnrichmentPreviewView, ToolError> {
        let cfg = self.current_config().await;
        let selection_truncated = params.all
            && cfg
                .upstream
                .iter()
                .filter(|upstream| upstream.enabled)
                .count()
                > params
                    .max_upstreams
                    .unwrap_or(MAX_MANUAL_UPSTREAMS)
                    .min(MAX_MANUAL_UPSTREAMS);
        let selected = select_upstreams_for_preview(&cfg, &params)?;
        let pool = self.current_pool().await;
        let mut collected = collect_enrichment_inputs(pool.as_deref(), &cfg, &selected).await?;
        if selection_truncated {
            collected.stats.truncated = true;
        }
        let mut runner = ProviderRunner::default();
        if let Some(timeout_ms) = params.timeout_ms.take() {
            runner.timeout_ms = timeout_ms;
        }
        let mut proposals =
            run_provider_preview(params.provider, &collected.inputs, &runner).await?;
        proposals.extend(
            collected
                .omitted_inputs
                .iter()
                .map(|input| omitted_input_proposal(input, params.provider)),
        );
        Ok(GatewayEnrichmentPreviewView {
            provider: params.provider,
            stats: collected.stats.into(),
            proposals,
        })
    }

    pub async fn apply_enrichment(
        &self,
        params: GatewayEnrichApplyParams,
    ) -> Result<GatewayHintApplyView, ToolError> {
        let hint = validate_hint(&params.hint)?;
        let _mutation_guard = self.config_mutation.lock().await;
        let mut cfg = self.config.read().await.clone();
        let pool = self.current_pool().await;
        let selected = [SelectedUpstream {
            name: params.upstream.clone(),
            explicit: true,
        }];
        let collected = collect_enrichment_inputs(pool.as_deref(), &cfg, &selected).await?;
        let current_hash = collected
            .inputs
            .first()
            .map(|input| input.metadata_hash.as_str())
            .ok_or_else(|| ToolError::Sdk {
                sdk_kind: "unknown_upstream".to_string(),
                message: format!("unknown gateway upstream `{}`", params.upstream),
            })?;
        if current_hash != params.metadata_hash {
            return Err(ToolError::Sdk {
                sdk_kind: "stale_suggestion".to_string(),
                message:
                    "gateway enrichment suggestion no longer matches current upstream metadata"
                        .to_string(),
            });
        }

        let upstream = cfg
            .upstream
            .iter_mut()
            .find(|upstream| upstream.name == params.upstream)
            .ok_or_else(|| ToolError::Sdk {
                sdk_kind: "unknown_upstream".to_string(),
                message: format!("unknown gateway upstream `{}`", params.upstream),
            })?;
        let previous_hint = upstream
            .code_mode_hint
            .as_deref()
            .and_then(labby_runtime::gateway_config::normalize_code_mode_hint);
        upstream.code_mode_hint = Some(hint.clone());
        self.persist_config(cfg).await?;
        self.notify_catalog_changes(&GatewayCatalogDiff {
            tools_changed: true,
            resources_changed: false,
            prompts_changed: false,
        });

        Ok(GatewayHintApplyView {
            upstream: params.upstream,
            hint,
            applied: true,
            previous_hint,
        })
    }

    pub(crate) async fn preview_enrichment_for_new_upstream(
        &self,
        upstream: &str,
    ) -> (Option<GatewayHintProposalView>, Option<String>) {
        let preview_result = tokio::time::timeout(
            std::time::Duration::from_secs(2),
            self.preview_enrichment(GatewayEnrichPreviewParams {
                upstreams: vec![upstream.to_string()],
                all: false,
                provider: GatewayEnrichmentProvider::Deterministic,
                max_upstreams: Some(1),
                timeout_ms: Some(2_000),
            }),
        )
        .await;
        let preview = match preview_result {
            Ok(Ok(preview)) => preview,
            Ok(Err(err)) => {
                let message = err.to_string();
                tracing::warn!(
                    surface = "dispatch",
                    service = "gateway",
                    action = "gateway.enrich.preview",
                    upstream,
                    kind = %err.kind(),
                    error = %message,
                    "gateway enrichment suggestion skipped"
                );
                return (None, Some(message));
            }
            Err(_) => {
                let message = "gateway enrichment suggestion timed out".to_string();
                tracing::warn!(
                    surface = "dispatch",
                    service = "gateway",
                    action = "gateway.enrich.preview",
                    upstream,
                    kind = "timeout",
                    error = %message,
                    "gateway enrichment suggestion skipped"
                );
                return (None, Some(message));
            }
        };
        (preview.proposals.into_iter().next(), None)
    }
}

fn validate_hint(hint: &str) -> Result<String, ToolError> {
    labby_runtime::gateway_config::normalize_code_mode_hint(hint).ok_or_else(|| ToolError::Sdk {
        sdk_kind: "invalid_hint".to_string(),
        message:
            "code mode hint must be plain, non-instructional text from 1-240 characters on one line"
                .to_string(),
    })
}

fn omitted_input_proposal(
    input: &UpstreamEnrichmentInput,
    provider: GatewayEnrichmentProvider,
) -> GatewayHintProposalView {
    let existing_hint = input.existing_hint.clone();
    let status = if existing_hint.is_some() {
        GatewayHintProposalStatus::Existing
    } else {
        GatewayHintProposalStatus::MetadataInsufficient
    };
    GatewayHintProposalView {
        upstream: input.name.clone(),
        hint: existing_hint.clone(),
        status,
        metadata_hash: input.metadata_hash.clone(),
        provider,
        tool_count: input.tool_names.len(),
        resource_count: input.resource_count,
        prompt_count: input.prompt_count,
        existing_hint,
    }
}
