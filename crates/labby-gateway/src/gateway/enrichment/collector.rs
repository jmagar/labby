use std::collections::{BTreeMap, BTreeSet};

use labby_runtime::error::ToolError;
use labby_runtime::gateway_config::{
    CODE_MODE_HINT_SANITIZER_VERSION, GatewayConfig, UpstreamConfig, normalize_code_mode_hint,
};
use serde::{Deserialize, Serialize};
use sha2::Digest;

use crate::gateway::params::GatewayEnrichPreviewParams;
use crate::gateway::projection::sanitize_tool_text;
use crate::upstream::pool::UpstreamPool;
use crate::upstream::types::UpstreamEnrichmentCatalogEntry;

pub(crate) const COLLECTOR_VERSION: &str = "gateway_enrichment_collector_v1";
pub(crate) const MAX_MANUAL_UPSTREAMS: usize = 25;
pub(crate) const MAX_TOOLS_PER_UPSTREAM: usize = 100;
pub(crate) const MAX_TOTAL_TOOLS: usize = 300;
pub(crate) const MAX_RESOURCES_PER_UPSTREAM: usize = 50;
pub(crate) const MAX_PROMPTS_PER_UPSTREAM: usize = 50;
pub(crate) const MAX_PROVIDER_INPUT_BYTES: usize = 64 * 1024;
const MAX_DESCRIPTION_CHARS: usize = 180;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct UpstreamEnrichmentInput {
    pub(crate) name: String,
    pub(crate) existing_hint: Option<String>,
    pub(crate) transport: String,
    pub(crate) enabled: bool,
    pub(crate) tool_names: Vec<String>,
    pub(crate) tool_descriptions: Vec<String>,
    pub(crate) resource_count: usize,
    pub(crate) prompt_count: usize,
    pub(crate) metadata_hash: String,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct EnrichmentInputStats {
    pub(crate) bytes: usize,
    pub(crate) upstream_count: usize,
    pub(crate) tool_count: usize,
    pub(crate) truncated: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct CollectedEnrichmentInputs {
    pub(crate) inputs: Vec<UpstreamEnrichmentInput>,
    pub(crate) omitted_inputs: Vec<UpstreamEnrichmentInput>,
    pub(crate) stats: EnrichmentInputStats,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SelectedUpstream {
    pub(crate) name: String,
    pub(crate) explicit: bool,
}

pub(crate) fn select_upstreams_for_preview(
    cfg: &GatewayConfig,
    params: &GatewayEnrichPreviewParams,
) -> Result<Vec<SelectedUpstream>, ToolError> {
    if params.all && !params.upstreams.is_empty() {
        return Err(ToolError::InvalidParam {
            message: "gateway.enrich.preview requires either `all` or `upstreams`, not both"
                .to_string(),
            param: "upstreams".to_string(),
        });
    }
    if !params.all && params.upstreams.is_empty() {
        return Err(ToolError::InvalidParam {
            message: "gateway.enrich.preview requires `upstreams` or `all: true`".to_string(),
            param: "upstreams".to_string(),
        });
    }

    if params.all {
        let limit = params
            .max_upstreams
            .unwrap_or(MAX_MANUAL_UPSTREAMS)
            .min(MAX_MANUAL_UPSTREAMS);
        return Ok(cfg
            .upstream
            .iter()
            .filter(|upstream| upstream.enabled)
            .map(|upstream| upstream.name.clone())
            .take(limit)
            .map(|name| SelectedUpstream {
                name,
                explicit: false,
            })
            .collect());
    }

    let configured = cfg
        .upstream
        .iter()
        .map(|upstream| (upstream.name.as_str(), upstream))
        .collect::<BTreeMap<_, _>>();

    let mut selected = Vec::new();
    let mut seen = BTreeSet::new();
    for raw in &params.upstreams {
        let name = raw.trim();
        if name.is_empty() {
            return Err(ToolError::InvalidParam {
                message: "upstream names must not be empty".to_string(),
                param: "upstreams".to_string(),
            });
        }
        if !configured.contains_key(name) {
            return Err(ToolError::Sdk {
                sdk_kind: "unknown_upstream".to_string(),
                message: format!("unknown gateway upstream `{name}`"),
            });
        }
        if seen.insert(name.to_string()) {
            selected.push(SelectedUpstream {
                name: name.to_string(),
                explicit: true,
            });
        }
    }
    if selected.len() > MAX_MANUAL_UPSTREAMS {
        return Err(ToolError::InvalidParam {
            message: format!(
                "gateway.enrich.preview accepts at most {MAX_MANUAL_UPSTREAMS} explicit upstreams"
            ),
            param: "upstreams".to_string(),
        });
    }
    Ok(selected)
}

pub(crate) async fn collect_enrichment_inputs(
    pool: Option<&UpstreamPool>,
    cfg: &GatewayConfig,
    selected: &[SelectedUpstream],
) -> Result<CollectedEnrichmentInputs, ToolError> {
    let allowed = selected
        .iter()
        .map(|selected| selected.name.clone())
        .collect::<BTreeSet<_>>();
    let cached = match pool {
        Some(pool) => {
            pool.cached_enrichment_snapshot(Some(&allowed), MAX_TOOLS_PER_UPSTREAM + 1)
                .await
        }
        None => Vec::new(),
    }
    .into_iter()
    .map(|entry| (entry.upstream.clone(), entry))
    .collect::<BTreeMap<_, _>>();

    let config_by_name = cfg
        .upstream
        .iter()
        .map(|upstream| (upstream.name.as_str(), upstream))
        .collect::<BTreeMap<_, _>>();

    let mut inputs = Vec::new();
    let mut total_tools = 0usize;
    let mut truncated = false;
    for selected in selected {
        let Some(upstream) = config_by_name.get(selected.name.as_str()) else {
            return Err(ToolError::Sdk {
                sdk_kind: "unknown_upstream".to_string(),
                message: format!("unknown gateway upstream `{}`", selected.name),
            });
        };
        if !selected.explicit && !upstream.enabled {
            continue;
        }
        let cached_entry = cached.get(&selected.name);
        let remaining_tools = MAX_TOTAL_TOOLS.saturating_sub(total_tools);
        let tool_limit = MAX_TOOLS_PER_UPSTREAM.min(remaining_tools);
        if remaining_tools == 0 {
            truncated = true;
        }
        let mut input = input_from_upstream(upstream, cached_entry, tool_limit);
        total_tools += input.tool_names.len();
        if cached_entry.is_some_and(|entry| {
            entry.tool_rows.iter().filter(|row| row.exposed).count() > tool_limit
        }) {
            truncated = true;
        }
        input.metadata_hash = hash_enrichment_input(&input);
        inputs.push(input);
    }

    let mut omitted_inputs = Vec::new();
    let mut stats = input_stats(&inputs, truncated);
    while stats.bytes > MAX_PROVIDER_INPUT_BYTES && !inputs.is_empty() {
        truncated = true;
        if let Some(input) = inputs.pop() {
            omitted_inputs.push(input);
        }
        stats = input_stats(&inputs, truncated);
    }
    omitted_inputs.reverse();
    Ok(CollectedEnrichmentInputs {
        inputs,
        omitted_inputs,
        stats,
    })
}

fn input_stats(inputs: &[UpstreamEnrichmentInput], truncated: bool) -> EnrichmentInputStats {
    EnrichmentInputStats {
        bytes: serde_json::to_vec(inputs).map_or(0, |bytes| bytes.len()),
        upstream_count: inputs.len(),
        tool_count: inputs.iter().map(|input| input.tool_names.len()).sum(),
        truncated,
    }
}

fn input_from_upstream(
    upstream: &UpstreamConfig,
    cached: Option<&UpstreamEnrichmentCatalogEntry>,
    tool_limit: usize,
) -> UpstreamEnrichmentInput {
    let transport = if upstream.url.is_some() {
        "http"
    } else {
        "stdio"
    }
    .to_string();
    let mut tool_names = Vec::new();
    let mut tool_descriptions = Vec::new();
    if let Some(cached) = cached {
        for row in cached
            .tool_rows
            .iter()
            .filter(|row| row.exposed)
            .take(tool_limit)
        {
            let tool_name = sanitize_identifier(&row.name);
            tool_names.push(tool_name);
            if let Some(description) = row.description.as_deref() {
                let description = sanitize_metadata_text(description, MAX_DESCRIPTION_CHARS);
                tool_descriptions.push(description);
            } else {
                tool_descriptions.push(String::new());
            }
        }
    }
    UpstreamEnrichmentInput {
        name: sanitize_identifier(&upstream.name),
        existing_hint: upstream
            .code_mode_hint
            .as_deref()
            .and_then(normalize_code_mode_hint),
        transport,
        enabled: upstream.enabled,
        tool_names,
        tool_descriptions,
        resource_count: cached
            .map(|entry| entry.resource_count.min(MAX_RESOURCES_PER_UPSTREAM))
            .unwrap_or_default(),
        prompt_count: cached
            .map(|entry| entry.prompt_count.min(MAX_PROMPTS_PER_UPSTREAM))
            .unwrap_or_default(),
        metadata_hash: String::new(),
    }
}

pub(crate) fn sanitize_metadata_text(input: &str, max_chars: usize) -> String {
    let sanitized = sanitize_tool_text(input, max_chars);
    let mut out = String::with_capacity(sanitized.len());
    let mut previous_was_space = false;
    for ch in sanitized.chars() {
        if ch.is_control() {
            continue;
        }
        if ch.is_whitespace() {
            if !previous_was_space {
                out.push(' ');
                previous_was_space = true;
            }
            continue;
        }
        previous_was_space = false;
        out.push(ch);
    }
    redact_for_provider_input(out.trim())
}

fn sanitize_identifier(input: &str) -> String {
    let mut out = input
        .chars()
        .filter(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_' | '.'))
        .take(128)
        .collect::<String>();
    if out.is_empty() {
        out = "upstream".to_string();
    }
    out
}

fn redact_for_provider_input(input: &str) -> String {
    let lower = input.to_ascii_lowercase();
    for needle in [
        "authorization",
        "bearer ",
        "api_key",
        "apikey",
        "access_token",
        "refresh_token",
        "password",
        "passwd",
        "secret",
        "credential",
        ".env",
        "/proc/environ",
        "lab_",
    ] {
        if lower.contains(needle) {
            return "[redacted]".to_string();
        }
    }

    input
        .split_whitespace()
        .map(|part| {
            let lower = part.to_ascii_lowercase();
            if lower.starts_with("/home/")
                || lower.starts_with("/users/")
                || lower.starts_with("/etc/")
                || lower.starts_with("/var/run/")
                || lower.ends_with(".sock")
            {
                "[redacted]"
            } else {
                part
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

pub(crate) fn hash_enrichment_input(input: &UpstreamEnrichmentInput) -> String {
    #[derive(Serialize)]
    struct CanonicalTool<'a> {
        name: &'a str,
        description: &'a str,
    }

    #[derive(Serialize)]
    struct Canonical<'a> {
        sanitizer_version: &'static str,
        collector_version: &'static str,
        name: &'a str,
        transport: &'a str,
        enabled: bool,
        tools: Vec<CanonicalTool<'a>>,
        resource_count: usize,
        prompt_count: usize,
        caps: BTreeMap<&'static str, usize>,
    }

    let mut tools = input
        .tool_names
        .iter()
        .enumerate()
        .map(|(idx, name)| CanonicalTool {
            name,
            description: input
                .tool_descriptions
                .get(idx)
                .map(String::as_str)
                .unwrap_or_default(),
        })
        .collect::<Vec<_>>();
    tools.sort_by(|left, right| {
        left.name
            .cmp(right.name)
            .then_with(|| left.description.cmp(right.description))
    });
    let caps = BTreeMap::from([
        ("max_manual_upstreams", MAX_MANUAL_UPSTREAMS),
        ("max_tools_per_upstream", MAX_TOOLS_PER_UPSTREAM),
        ("max_total_tools", MAX_TOTAL_TOOLS),
        ("max_resources_per_upstream", MAX_RESOURCES_PER_UPSTREAM),
        ("max_prompts_per_upstream", MAX_PROMPTS_PER_UPSTREAM),
        ("max_provider_input_bytes", MAX_PROVIDER_INPUT_BYTES),
    ]);
    let canonical = Canonical {
        sanitizer_version: CODE_MODE_HINT_SANITIZER_VERSION,
        collector_version: COLLECTOR_VERSION,
        name: &input.name,
        transport: &input.transport,
        enabled: input.enabled,
        tools,
        resource_count: input.resource_count,
        prompt_count: input.prompt_count,
        caps,
    };
    let bytes = serde_json::to_vec(&canonical).expect("canonical enrichment input serializes");
    let digest = sha2::Sha256::digest(&bytes);
    format!("sha256:{}", hex::encode(digest))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_input(existing_hint: Option<&str>) -> UpstreamEnrichmentInput {
        UpstreamEnrichmentInput {
            name: "github".to_string(),
            existing_hint: existing_hint.map(str::to_string),
            transport: "http".to_string(),
            enabled: true,
            tool_names: vec!["search".to_string(), "issues.list".to_string()],
            tool_descriptions: vec![
                "Search repository metadata".to_string(),
                "List issue summaries".to_string(),
            ],
            resource_count: 2,
            prompt_count: 1,
            metadata_hash: String::new(),
        }
    }

    #[test]
    fn metadata_hash_excludes_existing_hint_text() {
        let without_hint = sample_input(None);
        let with_hint = sample_input(Some("Use for GitHub repo and issue metadata."));

        assert_eq!(
            hash_enrichment_input(&without_hint),
            hash_enrichment_input(&with_hint)
        );
    }

    #[test]
    fn sanitize_metadata_text_redacts_sensitive_provider_input() {
        assert_eq!(
            sanitize_metadata_text("Authorization: Bearer super-secret-token", 200),
            "[redacted]"
        );
        assert_eq!(
            sanitize_metadata_text("reads /home/jacob/.config/app/config.toml", 200),
            "reads [redacted]"
        );
        assert_eq!(
            sanitize_metadata_text("uses LAB_FOO_TOKEN from .env", 200),
            "[redacted]"
        );
    }

    #[test]
    fn metadata_hash_preserves_tool_name_description_pairing() {
        let mut first = sample_input(None);
        let mut second = sample_input(None);
        first.tool_descriptions = vec!["one".to_string(), "two".to_string()];
        second.tool_descriptions = vec!["two".to_string(), "one".to_string()];

        assert_ne!(
            hash_enrichment_input(&first),
            hash_enrichment_input(&second)
        );
    }
}
