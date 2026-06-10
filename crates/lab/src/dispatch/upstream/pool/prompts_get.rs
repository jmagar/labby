//! Subject-scoped prompt discovery and prompt fetching.
//!
//! `subject_scoped_prompts`/`subject_scoped_prompt_owner` discover prompts for
//! OAuth upstreams under a subject; `get_prompt`/`subject_scoped_get_prompt`
//! fetch a single prompt with a request timeout and structured logging.

use std::time::Instant;

use futures::StreamExt;
use futures::stream::FuturesUnordered;
use rmcp::model::{GetPromptRequestParams, GetPromptResult, Prompt};

use crate::config::UpstreamConfig;

use super::super::types::UpstreamCapability;
use super::UpstreamPool;
use super::capability_call::timed_capability_call;
use super::helpers::{
    bare_upstream_prompt_name, merge_upstream_prompts, prefixed_upstream_prompt_name,
    upstream_transport,
};
use super::logging::{UpstreamRequestLog, log_upstream_request_error, log_upstream_request_start};

impl UpstreamPool {
    /// Discover prompts from all OAuth upstreams visible to `subject`.
    ///
    /// P-C1 fix: uses `acquire_or_connect_subject` so connections are cached;
    /// the tools list from connect is not needed here but the cached peer is
    /// used directly for `list_prompts`.
    pub async fn subject_scoped_prompts(
        &self,
        configs: &[UpstreamConfig],
        subject: &str,
        builtin_names: &[&str],
    ) -> Vec<Prompt> {
        let mut futures = FuturesUnordered::new();
        for config in configs.iter().filter(|config| config.oauth.is_some()) {
            let config = config.clone();
            let subject = subject.to_string();
            let pool = self.clone();
            futures.push(async move {
                let result = pool
                    .acquire_or_connect_subject(&config, &subject)
                    .await
                    .map(|(peer, _tools)| peer);
                (config.name.clone(), result)
            });
        }

        let mut upstream_prompts = Vec::new();
        while let Some((name, result)) = futures.next().await {
            let Ok(peer) = result else {
                continue;
            };
            match peer.list_prompts(None).await {
                Ok(result) => upstream_prompts.push((name, result.prompts)),
                Err(error) => {
                    tracing::warn!(
                        upstream = %name,
                        error = %error,
                        "subject-scoped upstream prompt discovery failed"
                    );
                }
            }
        }

        let (prompts, _) = merge_upstream_prompts(builtin_names, upstream_prompts);
        prompts
    }

    /// Find which upstream owns `prompt_name` for `subject`.
    ///
    /// P-C1 fix: uses `acquire_or_connect_subject` so connections are cached.
    pub async fn subject_scoped_prompt_owner(
        &self,
        configs: &[UpstreamConfig],
        subject: &str,
        prompt_name: &str,
    ) -> Option<String> {
        let mut futures = FuturesUnordered::new();
        for config in configs.iter().filter(|config| config.oauth.is_some()) {
            let config = config.clone();
            let subject = subject.to_string();
            let pool = self.clone();
            let target_prompt = prompt_name.to_string();
            futures.push(async move {
                let result = pool
                    .acquire_or_connect_subject(&config, &subject)
                    .await
                    .map(|(peer, _tools)| peer);
                (config.name.clone(), target_prompt, result)
            });
        }

        while let Some((name, target_prompt, result)) = futures.next().await {
            let Ok(peer) = result else {
                continue;
            };
            if let Ok(result) = peer.list_prompts(None).await
                && result.prompts.iter().any(|prompt| {
                    // The requested name is namespaced as `{upstream}/{name}`;
                    // the upstream advertises the bare name, so compare against
                    // the prefixed form.
                    prefixed_upstream_prompt_name(&name, &prompt.name) == target_prompt
                })
            {
                return Some(name);
            }
        }
        None
    }

    /// Proxy a get-prompt request to a specific upstream.
    pub async fn get_prompt(
        &self,
        upstream_name: &str,
        mut params: GetPromptRequestParams,
    ) -> Option<Result<GetPromptResult, String>> {
        let start = Instant::now();
        // The gateway namespaces upstream prompt names as `{upstream}/{name}`,
        // but the upstream only knows the bare name — strip the prefix before
        // forwarding (mirrors `read_upstream_resource` stripping the URI prefix).
        params.name = bare_upstream_prompt_name(upstream_name, &params.name).to_string();
        let prompt_name = params.name.to_string();
        let event = UpstreamRequestLog::prompt(upstream_name, &prompt_name, false);
        let peer = self
            .acquire_peer(upstream_name, UpstreamCapability::Prompts, "prompt.get")
            .await?;

        log_upstream_request_start(event);

        let timeout_ms = self.request_timeout.as_millis();
        Some(
            timed_capability_call(
                self,
                upstream_name,
                UpstreamCapability::Prompts,
                event,
                start,
                peer.get_prompt(params),
                |_result: &GetPromptResult| 0, // prompts have no size cap
                None,
                |e| format!("upstream prompt get failed: {e}"),
                format!("upstream prompt get timed out after {timeout_ms}ms"),
            )
            .await,
        )
    }

    /// Get a prompt from an OAuth-subject-scoped upstream.
    ///
    /// P-C1 fix: uses `acquire_or_connect_subject` so the per-(upstream,subject)
    /// connection is reused from cache rather than opened fresh each call.
    pub async fn subject_scoped_get_prompt(
        &self,
        config: &UpstreamConfig,
        subject: &str,
        mut params: GetPromptRequestParams,
    ) -> Result<GetPromptResult, String> {
        let start = Instant::now();
        // Strip the `{upstream}/` namespace before forwarding the bare name.
        params.name = bare_upstream_prompt_name(&config.name, &params.name).to_string();
        let prompt_name = params.name.to_string();
        let event = UpstreamRequestLog::prompt(&config.name, &prompt_name, true)
            .with_transport(upstream_transport(config));
        log_upstream_request_start(event);
        // P-C1: reuse cached per-(upstream,subject) connection.
        let (peer, _tools) = match self.acquire_or_connect_subject(config, subject).await {
            Ok(pair) => pair,
            Err(error) => {
                self.record_failure_for(
                    &config.name,
                    UpstreamCapability::Prompts,
                    format!("upstream prompt connect failed: {error}"),
                )
                .await;
                log_upstream_request_error(
                    event,
                    start.elapsed().as_millis(),
                    "upstream_connect_error",
                    Some(&error),
                    None,
                    None,
                );
                return Err(error.to_string());
            }
        };
        let timeout_ms = self.request_timeout.as_millis();
        timed_capability_call(
            self,
            &config.name,
            UpstreamCapability::Prompts,
            event,
            start,
            peer.get_prompt(params),
            |_result: &GetPromptResult| 0, // prompts have no size cap
            Some(subject),
            |e| format!("upstream prompt get failed: {e}"),
            format!("upstream prompt get timed out after {timeout_ms}ms"),
        )
        .await
    }
}

#[cfg(test)]
mod tests {
    use rmcp::model::GetPromptRequestParams;

    use super::super::testsupport::*;

    #[tokio::test]
    async fn get_prompt_times_out_slow_upstream_response() {
        let pool = slow_response_pool("slow").await;

        let result = pool
            .get_prompt("slow", GetPromptRequestParams::new("slow.prompt"))
            .await
            .expect("upstream is connected")
            .expect_err("slow prompt get should time out");

        assert!(result.contains("timed out"));
    }
}
