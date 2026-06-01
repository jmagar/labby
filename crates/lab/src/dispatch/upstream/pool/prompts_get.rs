//! Subject-scoped prompt discovery and prompt fetching.
//!
//! `subject_scoped_prompts`/`subject_scoped_prompt_owner` discover prompts for
//! OAuth upstreams under a subject; `get_prompt`/`subject_scoped_get_prompt`
//! fetch a single prompt with a request timeout and structured logging.
//!
//! NO-TOUCH (plan §6): the `subject_scoped_*` methods retain their `subject`
//! argument threading; bodies are moved byte-identical from `pool.rs`.

use std::time::Instant;

use futures::StreamExt;
use futures::stream::FuturesUnordered;
use rmcp::model::{GetPromptRequestParams, GetPromptResult, Prompt};

use crate::config::UpstreamConfig;

use super::super::types::UpstreamCapability;
use super::UpstreamPool;
use super::connect::connect_upstream;
use super::helpers::{
    bare_upstream_prompt_name, merge_upstream_prompts, prefixed_upstream_prompt_name,
    upstream_transport,
};
use super::logging::{
    UpstreamRequestLog, log_upstream_request_error, log_upstream_request_finish,
    log_upstream_request_start,
};

impl UpstreamPool {
    pub async fn subject_scoped_prompts(
        &self,
        configs: &[UpstreamConfig],
        subject: &str,
        builtin_names: &[&str],
    ) -> Vec<Prompt> {
        let mut futures = FuturesUnordered::new();
        let oauth_client_cache = self.oauth_client_cache.clone();
        for config in configs.iter().filter(|config| config.oauth.is_some()) {
            let config = config.clone();
            let subject = subject.to_string();
            let oauth_client_cache = oauth_client_cache.clone();
            futures.push(async move {
                let result = connect_upstream(
                    &config,
                    Some(subject.as_str()),
                    oauth_client_cache.as_ref(),
                    None,
                    None,
                )
                .await
                .map(|(conn, _)| conn);
                (config.name.clone(), result)
            });
        }

        let mut upstream_prompts = Vec::new();
        while let Some((name, result)) = futures.next().await {
            let Ok(conn) = result else {
                continue;
            };
            match conn.peer.list_prompts(None).await {
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

    pub async fn subject_scoped_prompt_owner(
        &self,
        configs: &[UpstreamConfig],
        subject: &str,
        prompt_name: &str,
    ) -> Option<String> {
        let mut futures = FuturesUnordered::new();
        let oauth_client_cache = self.oauth_client_cache.clone();
        for config in configs.iter().filter(|config| config.oauth.is_some()) {
            let config = config.clone();
            let subject = subject.to_string();
            let oauth_client_cache = oauth_client_cache.clone();
            let target_prompt = prompt_name.to_string();
            futures.push(async move {
                let result = connect_upstream(
                    &config,
                    Some(subject.as_str()),
                    oauth_client_cache.as_ref(),
                    None,
                    None,
                )
                .await
                .map(|(conn, _)| conn);
                (config.name.clone(), target_prompt, result)
            });
        }

        while let Some((name, target_prompt, result)) = futures.next().await {
            let Ok(conn) = result else {
                continue;
            };
            if let Ok(result) = conn.peer.list_prompts(None).await
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

        match tokio::time::timeout(self.request_timeout, peer.get_prompt(params)).await {
            Ok(Ok(result)) => {
                self.record_success_for(upstream_name, UpstreamCapability::Prompts)
                    .await;
                log_upstream_request_finish(event, start.elapsed().as_millis(), None);
                Some(Ok(result))
            }
            Ok(Err(e)) => {
                self.record_failure_for(
                    upstream_name,
                    UpstreamCapability::Prompts,
                    format!("upstream prompt get failed: {e}"),
                )
                .await;
                log_upstream_request_error(
                    event,
                    start.elapsed().as_millis(),
                    "upstream_error",
                    Some(&e),
                    None,
                    None,
                );
                Some(Err(format!("upstream prompt get failed: {e}")))
            }
            Err(_) => {
                let message = format!(
                    "upstream prompt get timed out after {}ms",
                    self.request_timeout.as_millis()
                );
                self.record_failure_for(
                    upstream_name,
                    UpstreamCapability::Prompts,
                    message.clone(),
                )
                .await;
                log_upstream_request_error(
                    event,
                    start.elapsed().as_millis(),
                    "timeout",
                    None,
                    None,
                    None,
                );
                Some(Err(message))
            }
        }
    }

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
        let (conn, _) = match connect_upstream(
            config,
            Some(subject),
            self.oauth_client_cache.as_ref(),
            None,
            None,
        )
        .await
        {
            Ok(conn) => conn,
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
        match tokio::time::timeout(self.request_timeout, conn.peer.get_prompt(params)).await {
            Ok(Ok(result)) => {
                self.record_success_for(&config.name, UpstreamCapability::Prompts)
                    .await;
                log_upstream_request_finish(event, start.elapsed().as_millis(), None);
                Ok(result)
            }
            Ok(Err(error)) => {
                self.record_failure_for(
                    &config.name,
                    UpstreamCapability::Prompts,
                    format!("upstream prompt get failed: {error}"),
                )
                .await;
                log_upstream_request_error(
                    event,
                    start.elapsed().as_millis(),
                    "upstream_error",
                    Some(&error),
                    None,
                    None,
                );
                Err(format!("upstream prompt get failed: {error}"))
            }
            Err(_) => {
                let message = format!(
                    "upstream prompt get timed out after {}ms",
                    self.request_timeout.as_millis()
                );
                self.record_failure_for(&config.name, UpstreamCapability::Prompts, message.clone())
                    .await;
                log_upstream_request_error(
                    event,
                    start.elapsed().as_millis(),
                    "timeout",
                    None,
                    None,
                    None,
                );
                Err(message)
            }
        }
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
