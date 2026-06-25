use labby_runtime::gateway_config::normalize_code_mode_hint;

use crate::gateway::enrichment::collector::UpstreamEnrichmentInput;
use crate::gateway::types::{
    GatewayEnrichmentProvider, GatewayHintProposalStatus, GatewayHintProposalView,
};

pub(crate) fn summarize_batch(inputs: &[UpstreamEnrichmentInput]) -> Vec<GatewayHintProposalView> {
    inputs.iter().map(summarize_one).collect()
}

fn summarize_one(input: &UpstreamEnrichmentInput) -> GatewayHintProposalView {
    let existing_hint = input
        .existing_hint
        .as_deref()
        .and_then(normalize_code_mode_hint);
    let hint = if let Some(existing) = existing_hint.clone() {
        Some(existing)
    } else if input.tool_names.is_empty() {
        None
    } else {
        Some(format!(
            "capabilities: {}",
            input
                .tool_names
                .iter()
                .take(4)
                .cloned()
                .collect::<Vec<_>>()
                .join(", ")
        ))
    }
    .and_then(|raw| normalize_code_mode_hint(&raw));

    GatewayHintProposalView {
        upstream: input.name.clone(),
        hint,
        status: if existing_hint.is_some() {
            GatewayHintProposalStatus::Existing
        } else if input.tool_names.is_empty() {
            GatewayHintProposalStatus::MetadataInsufficient
        } else {
            GatewayHintProposalStatus::Suggested
        },
        metadata_hash: input.metadata_hash.clone(),
        provider: GatewayEnrichmentProvider::Deterministic,
        tool_count: input.tool_names.len(),
        resource_count: input.resource_count,
        prompt_count: input.prompt_count,
        existing_hint,
    }
}
