use labby_runtime::error::ToolError;

use crate::gateway::enrichment::collector::{
    SelectedUpstream, collect_enrichment_inputs, select_upstreams_for_preview,
};
use crate::gateway::enrichment::provider::{ProviderRunner, run_provider_preview};
use crate::gateway::params::{GatewayEnrichApplyParams, GatewayEnrichPreviewParams};
use crate::gateway::types::{
    GatewayEnrichmentPreviewStatsView, GatewayEnrichmentPreviewView, GatewayEnrichmentProvider,
    GatewayHintApplyView, GatewayHintProposalView,
};

use super::GatewayManager;

impl GatewayManager {
    pub async fn preview_enrichment(
        &self,
        mut params: GatewayEnrichPreviewParams,
    ) -> Result<GatewayEnrichmentPreviewView, ToolError> {
        let cfg = self.current_config().await;
        let selected = select_upstreams_for_preview(&cfg, &params)?;
        let pool = self.current_pool().await;
        let collected = collect_enrichment_inputs(pool.as_deref(), &cfg, &selected).await?;
        let mut runner = ProviderRunner::default();
        if let Some(timeout_ms) = params.timeout_ms.take() {
            runner.timeout_ms = timeout_ms;
        }
        let proposals = run_provider_preview(params.provider, &collected.inputs, &runner).await?;
        Ok(GatewayEnrichmentPreviewView {
            provider: params.provider,
            stats: GatewayEnrichmentPreviewStatsView {
                bytes: collected.stats.bytes,
                upstream_count: collected.stats.upstream_count,
                tool_count: collected.stats.tool_count,
                truncated: collected.stats.truncated,
            },
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
    ) -> Option<GatewayHintProposalView> {
        let preview = tokio::time::timeout(
            std::time::Duration::from_secs(2),
            self.preview_enrichment(GatewayEnrichPreviewParams {
                upstreams: vec![upstream.to_string()],
                all: false,
                provider: GatewayEnrichmentProvider::Deterministic,
                max_upstreams: Some(1),
                timeout_ms: Some(2_000),
            }),
        )
        .await
        .ok()?
        .ok()?;
        preview.proposals.into_iter().next()
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
