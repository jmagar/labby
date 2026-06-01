//! In-process service-peer registration.
//!
//! Built-in lab services are exposed to the gateway as in-process upstream peers
//! over an in-memory transport. These methods register each service concurrently
//! (isolating slow/failing peers), populate the catalog, and record failures as
//! degraded entries. The `InProcessConnector`/`InProcessRegistration` types stay
//! defined in `pool.rs`; this descendant module sees them without annotation.

use std::collections::HashMap;
use std::sync::Arc;

use futures::StreamExt;
use futures::stream::FuturesUnordered;

use crate::registry::{RegisteredService, ToolRegistry};

use super::connect_stdio::connect_in_process_service_peer;
use super::entries::{
    failed_in_process_entry, failed_in_process_entry_from_existing, healthy_in_process_entry,
};
use super::helpers::{
    IN_PROCESS_DISCOVERY_TIMEOUT, cached_upstream_tool, in_process_upstream_name,
};
use super::{InProcessConnector, InProcessRegistration, UpstreamPool};

impl UpstreamPool {
    pub async fn register_in_process_service_peers(&self, registry: &ToolRegistry) {
        let services: Vec<RegisteredService> = registry
            .services()
            .iter()
            .filter(|service| !service.actions.is_empty())
            .cloned()
            .collect();
        self.register_in_process_service_list(services).await;
    }

    async fn register_in_process_service_list(&self, services: Vec<RegisteredService>) {
        let connector: InProcessConnector = Arc::new(|service| {
            Box::pin(async move {
                let upstream_name = in_process_upstream_name(service.name);
                let entry_name: Arc<str> = Arc::from(upstream_name.as_str());
                let (conn, tools) = connect_in_process_service_peer(&service).await?;
                Ok(InProcessRegistration {
                    connection: Some(conn),
                    tools,
                    entry_name,
                    upstream_name,
                })
            })
        });
        self.register_in_process_service_list_with_connector(services, connector)
            .await;
    }

    async fn register_in_process_service_list_with_connector(
        &self,
        services: Vec<RegisteredService>,
        connector: InProcessConnector,
    ) {
        let mut in_process_resource_names = Vec::new();
        let mut futures = FuturesUnordered::new();
        let mut failed_count = 0usize;
        let mut timeout_count = 0usize;

        for service in services {
            let upstream_name = in_process_upstream_name(service.name);
            tracing::info!(
                upstream = %upstream_name,
                service = service.name,
                timeout_secs = IN_PROCESS_DISCOVERY_TIMEOUT.as_secs(),
                "starting in-process peer registration"
            );
            let connector = Arc::clone(&connector);
            futures.push(async move {
                let service_name = service.name;
                let result =
                    tokio::time::timeout(IN_PROCESS_DISCOVERY_TIMEOUT, connector(service)).await;
                (service_name, upstream_name, result)
            });
        }

        while let Some((service_name, upstream_name, result)) = futures.next().await {
            match result {
                Ok(Ok(registration)) => {
                    let mut tool_map = HashMap::new();
                    let tool_count = registration.tools.len();
                    for tool in registration.tools {
                        tool_map.insert(
                            tool.name.to_string(),
                            cached_upstream_tool(tool, &registration.entry_name).1,
                        );
                    }

                    self.catalog.write().await.insert(
                        registration.upstream_name.clone(),
                        healthy_in_process_entry(Arc::clone(&registration.entry_name), tool_map),
                    );
                    if let Some(conn) = registration.connection {
                        self.connections
                            .write()
                            .await
                            .insert(registration.upstream_name.clone(), conn);
                    }
                    in_process_resource_names.push(registration.upstream_name.clone());
                    tracing::info!(
                        upstream = %registration.entry_name,
                        service = service_name,
                        tool_count,
                        resource_count = 0,
                        prompt_count = 0,
                        "in-process peer registration succeeded"
                    );
                }
                Ok(Err(error)) => {
                    failed_count += 1;
                    let error_message =
                        format!("failed to register in-process service peer: {error}");
                    tracing::warn!(
                        upstream = %upstream_name,
                        service = service_name,
                        error = %error_message,
                        "in-process peer registration failed"
                    );
                    let mut catalog = self.catalog.write().await;
                    let name: Arc<str> = Arc::from(upstream_name.as_str());
                    let entry = catalog
                        .remove(&upstream_name)
                        .map(|existing| {
                            failed_in_process_entry_from_existing(existing, error_message.clone())
                        })
                        .unwrap_or_else(|| failed_in_process_entry(name, error_message));
                    catalog.insert(upstream_name, entry);
                }
                Err(_) => {
                    failed_count += 1;
                    timeout_count += 1;
                    let error_message = format!(
                        "in-process peer registration timed out after {}s",
                        IN_PROCESS_DISCOVERY_TIMEOUT.as_secs()
                    );
                    tracing::warn!(
                        upstream = %upstream_name,
                        service = service_name,
                        timeout_secs = IN_PROCESS_DISCOVERY_TIMEOUT.as_secs(),
                        error = %error_message,
                        "in-process peer registration timed out"
                    );
                    let mut catalog = self.catalog.write().await;
                    let name: Arc<str> = Arc::from(upstream_name.as_str());
                    let entry = catalog
                        .remove(&upstream_name)
                        .map(|existing| {
                            failed_in_process_entry_from_existing(existing, error_message.clone())
                        })
                        .unwrap_or_else(|| failed_in_process_entry(name, error_message));
                    catalog.insert(upstream_name, entry);
                }
            }
        }

        if !in_process_resource_names.is_empty() {
            let mut resource_upstreams = self.resource_upstreams.write().await;
            resource_upstreams.extend(in_process_resource_names);
            resource_upstreams.sort_unstable();
            resource_upstreams.dedup();
        }

        if failed_count > 0 {
            tracing::warn!(
                failed_count,
                timeout_count,
                "in-process peer registration completed with degraded services"
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::time::Duration;

    use serde_json::Value;

    use crate::registry::RegisteredService;

    use super::super::helpers::in_process_upstream_name;
    use super::super::{InProcessConnector, InProcessRegistration};
    use super::*;

    #[tokio::test]
    async fn in_process_registration_isolates_slow_services_from_fast_services() {
        use futures::future::BoxFuture;
        use lab_apis::core::action::ActionSpec;
        use std::sync::atomic::{AtomicUsize, Ordering};

        static ACTIONS: &[ActionSpec] = &[ActionSpec {
            name: "status.read",
            description: "Read status",
            destructive: false,
            returns: "Value",
            params: &[],
        }];

        fn dispatch(
            _action: String,
            _params: Value,
        ) -> std::pin::Pin<
            Box<dyn Future<Output = Result<Value, crate::dispatch::error::ToolError>> + Send>,
        > {
            Box::pin(async { Ok(Value::Null) })
        }

        fn service(name: &'static str) -> RegisteredService {
            RegisteredService {
                name,
                description: "test service",
                category: "test",
                kind: crate::registry::RegisteredServiceKind::BuiltInUpstreamApi,
                status: "available",
                actions: ACTIONS,
                dispatch,
            }
        }

        let pool = UpstreamPool::new();
        let fast_seen = Arc::new(AtomicUsize::new(0));
        let fast_seen_for_connector = Arc::clone(&fast_seen);
        let connector: InProcessConnector = Arc::new(move |service| {
            let fast_seen = Arc::clone(&fast_seen_for_connector);
            let future: BoxFuture<'static, anyhow::Result<InProcessRegistration>> =
                Box::pin(async move {
                    if service.name == "slow" {
                        tokio::time::sleep(Duration::from_millis(100)).await;
                        anyhow::bail!("slow service failed to start");
                    }

                    fast_seen.fetch_add(1, Ordering::SeqCst);
                    let upstream_name: Arc<str> = Arc::from(in_process_upstream_name(service.name));
                    Ok(InProcessRegistration {
                        connection: None,
                        tools: Vec::new(),
                        entry_name: Arc::clone(&upstream_name),
                        upstream_name: upstream_name.to_string(),
                    })
                });
            future
        });

        let registration = tokio::spawn({
            let pool = pool.clone();
            async move {
                pool.register_in_process_service_list_with_connector(
                    vec![service("slow"), service("fast")],
                    connector,
                )
                .await;
            }
        });

        tokio::time::sleep(Duration::from_millis(20)).await;
        assert_eq!(
            fast_seen.load(Ordering::SeqCst),
            1,
            "fast service should register before slow service finishes"
        );

        registration.await.expect("registration task");
        assert_eq!(pool.upstream_count().await, 2);
    }

    #[tokio::test]
    async fn failed_in_process_registration_does_not_hide_healthy_peer_tools() {
        use futures::future::BoxFuture;
        use lab_apis::core::action::ActionSpec;

        static ACTIONS: &[ActionSpec] = &[ActionSpec {
            name: "status.read",
            description: "Read status",
            destructive: false,
            returns: "Value",
            params: &[],
        }];

        fn dispatch(
            _action: String,
            _params: Value,
        ) -> std::pin::Pin<
            Box<dyn Future<Output = Result<Value, crate::dispatch::error::ToolError>> + Send>,
        > {
            Box::pin(async { Ok(Value::Null) })
        }

        fn service(name: &'static str) -> RegisteredService {
            RegisteredService {
                name,
                description: "test service",
                category: "test",
                kind: crate::registry::RegisteredServiceKind::BuiltInUpstreamApi,
                status: "available",
                actions: ACTIONS,
                dispatch,
            }
        }

        let pool = UpstreamPool::new();
        let connector: InProcessConnector = Arc::new(|service| {
            let future: BoxFuture<'static, anyhow::Result<InProcessRegistration>> =
                Box::pin(async move {
                    if service.name == "bad" {
                        anyhow::bail!("bad service failed to start");
                    }

                    let upstream_name: Arc<str> = Arc::from(in_process_upstream_name(service.name));
                    let tool = rmcp::model::Tool::new(
                        "status.read",
                        "Read status",
                        Arc::new(serde_json::Map::new()),
                    );
                    Ok(InProcessRegistration {
                        connection: None,
                        tools: vec![tool],
                        entry_name: Arc::clone(&upstream_name),
                        upstream_name: upstream_name.to_string(),
                    })
                });
            future
        });

        pool.register_in_process_service_list_with_connector(
            vec![service("bad"), service("good")],
            connector,
        )
        .await;

        let good_tools = pool
            .healthy_tools_for_upstream(&in_process_upstream_name("good"))
            .await;
        let bad_tools = pool
            .healthy_tools_for_upstream(&in_process_upstream_name("bad"))
            .await;

        assert_eq!(good_tools.len(), 1);
        assert_eq!(good_tools[0].tool.name.as_ref(), "status.read");
        assert!(bad_tools.is_empty());
        assert_eq!(pool.upstream_count().await, 2);
    }
}
