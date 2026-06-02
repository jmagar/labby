//! Shared `#[cfg(test)]` fixtures and mock servers for the upstream-pool tests.
//!
//! These helpers are consumed by the co-located test modules across `pool/`
//! (discovery, tools, resources, prompts, health, …) and by the pool.rs test
//! module. They are `pub(super)` so every descendant test module can pull them
//! in with `use super::super::testsupport::*;` (or `use super::testsupport::*;`
//! from the pool.rs test module).

#![cfg(test)]

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::Duration;

use rmcp::model::{
    AnnotateAble, CallToolRequestParams, CallToolResult, ErrorData, GetPromptRequestParams,
    GetPromptResult, ListPromptsResult, ListResourcesResult, ListToolsResult,
    PaginatedRequestParams, Prompt, PromptMessage, PromptMessageRole, RawResource,
    ReadResourceRequestParams, ReadResourceResult, ServerCapabilities, ServerInfo,
};
use rmcp::service::RequestContext;
use rmcp::{RoleClient, RoleServer, ServerHandler, ServiceExt};

use crate::config::UpstreamConfig;

use super::super::types::{UpstreamRuntimeMetadata, UpstreamTool};
use super::entries::healthy_in_process_entry;
use super::helpers::IN_PROCESS_PEER_BUFFER_BYTES;
use super::{UpstreamConnection, UpstreamPool};

pub(super) fn test_upstream_config() -> UpstreamConfig {
    UpstreamConfig {
        enabled: true,
        name: "test".into(),
        url: None,
        bearer_token_env: None,
        command: None,
        args: vec![],
        env: std::collections::BTreeMap::new(),
        proxy_resources: false,
        proxy_prompts: false,
        expose_tools: None,
        expose_resources: None,
        expose_prompts: None,
        oauth: None,
        imported_from: None,
        priority: 1.0,
        tool_search: crate::config::ToolSearchConfig::default(),
    }
}

pub(super) fn named_test_upstream_config(name: &str) -> UpstreamConfig {
    UpstreamConfig {
        name: name.to_string(),
        command: Some("true".to_string()),
        ..test_upstream_config()
    }
}

pub(super) fn named_disabled_test_upstream_config(name: &str) -> UpstreamConfig {
    UpstreamConfig {
        enabled: false,
        ..named_test_upstream_config(name)
    }
}

pub(super) fn test_tool(name: &str) -> rmcp::model::Tool {
    rmcp::model::Tool::new(name.to_string(), "", Arc::new(serde_json::Map::new()))
}

pub(super) fn test_upstream_tool(upstream_name: &Arc<str>, name: &str) -> UpstreamTool {
    let schema = Arc::new(serde_json::Map::new());
    let tool = rmcp::model::Tool::new(name.to_string(), format!("{name} description"), schema);
    UpstreamTool {
        tool,
        input_schema: None,
        output_schema: None,
        upstream_name: Arc::clone(upstream_name),
        destructive: false,
    }
}

pub(super) fn test_upstream_tools(
    upstream_name: &Arc<str>,
    names: &[&str],
) -> HashMap<String, UpstreamTool> {
    names
        .iter()
        .map(|name| (name.to_string(), test_upstream_tool(upstream_name, name)))
        .collect()
}

#[derive(Clone, Default)]
pub(super) struct StaticCatalogServer {
    pub(super) list_prompts_count: Arc<AtomicUsize>,
    pub(super) get_prompt_count: Arc<AtomicUsize>,
    pub(super) fail_list_prompts: Arc<AtomicBool>,
}

impl ServerHandler for StaticCatalogServer {
    fn get_info(&self) -> ServerInfo {
        ServerInfo::new(
            ServerCapabilities::builder()
                .enable_resources()
                .enable_prompts()
                .build(),
        )
    }

    async fn list_resources(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> Result<ListResourcesResult, ErrorData> {
        Ok(ListResourcesResult::with_all_items(vec![
            RawResource::new("file:///tmp/upstream-one", "upstream-one").no_annotation(),
            RawResource::new(
                "lab://upstream/old-name/file:///tmp/upstream-two",
                "upstream-two",
            )
            .no_annotation(),
        ]))
    }

    async fn list_prompts(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> Result<ListPromptsResult, ErrorData> {
        self.list_prompts_count.fetch_add(1, Ordering::SeqCst);
        if self.fail_list_prompts.load(Ordering::SeqCst) {
            return Err(ErrorData::internal_error(
                "prompt listing failed for test",
                None,
            ));
        }

        Ok(ListPromptsResult::with_all_items(vec![
            Prompt::new("upstream.prompt.one", Some("first prompt"), None),
            Prompt::new("upstream.prompt.two", Some("second prompt"), None),
        ]))
    }

    async fn get_prompt(
        &self,
        request: GetPromptRequestParams,
        _context: RequestContext<RoleServer>,
    ) -> Result<GetPromptResult, ErrorData> {
        self.get_prompt_count.fetch_add(1, Ordering::SeqCst);
        Ok(GetPromptResult::new(vec![PromptMessage::new_text(
            PromptMessageRole::User,
            format!("proxied {}", request.name),
        )]))
    }
}

pub(super) async fn static_catalog_pool(upstream_name: &str) -> Arc<UpstreamPool> {
    static_catalog_pool_with_server(upstream_name, StaticCatalogServer::default()).await
}

pub(super) async fn static_catalog_pool_with_server(
    upstream_name: &str,
    server: StaticCatalogServer,
) -> Arc<UpstreamPool> {
    let (server_transport, client_transport) = tokio::io::duplex(IN_PROCESS_PEER_BUFFER_BYTES);
    let server_task = tokio::spawn(async move {
        let running = server
            .serve(server_transport)
            .await
            .expect("static catalog server starts");
        running.waiting().await.expect("static catalog server runs");
    });
    let client_service: rmcp::service::RunningService<RoleClient, ()> = ()
        .serve(client_transport)
        .await
        .expect("static catalog client starts");
    let peer = client_service.peer().clone();

    let pool = Arc::new(UpstreamPool::new());
    let upstream_name_arc: Arc<str> = Arc::from(upstream_name);
    pool.catalog.write().await.insert(
        upstream_name.to_string(),
        healthy_in_process_entry(Arc::clone(&upstream_name_arc), HashMap::new()),
    );
    pool.connections.write().await.insert(
        upstream_name.to_string(),
        UpstreamConnection {
            _client_service: client_service,
            _server_task: Some(server_task),
            peer,
            runtime: UpstreamRuntimeMetadata::default(),
        },
    );
    pool.resource_upstreams
        .write()
        .await
        .push(upstream_name.to_string());

    pool
}

pub(super) struct SlowResponseServer;

impl ServerHandler for SlowResponseServer {
    fn get_info(&self) -> ServerInfo {
        ServerInfo::new(
            ServerCapabilities::builder()
                .enable_tools()
                .enable_resources()
                .enable_prompts()
                .build(),
        )
    }

    async fn list_tools(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> Result<ListToolsResult, ErrorData> {
        Ok(ListToolsResult::with_all_items(Vec::new()))
    }

    async fn call_tool(
        &self,
        _request: CallToolRequestParams,
        _context: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, ErrorData> {
        tokio::time::sleep(Duration::from_millis(200)).await;
        Ok(CallToolResult::success(Vec::new()))
    }

    async fn read_resource(
        &self,
        _request: ReadResourceRequestParams,
        _context: RequestContext<RoleServer>,
    ) -> Result<ReadResourceResult, ErrorData> {
        tokio::time::sleep(Duration::from_millis(200)).await;
        Ok(ReadResourceResult::new(Vec::new()))
    }

    async fn get_prompt(
        &self,
        _request: GetPromptRequestParams,
        _context: RequestContext<RoleServer>,
    ) -> Result<GetPromptResult, ErrorData> {
        tokio::time::sleep(Duration::from_millis(200)).await;
        Ok(GetPromptResult::new(Vec::new()))
    }
}

pub(super) async fn slow_response_pool(upstream_name: &str) -> Arc<UpstreamPool> {
    let (server_transport, client_transport) = tokio::io::duplex(IN_PROCESS_PEER_BUFFER_BYTES);
    let server_task = tokio::spawn(async move {
        let running = SlowResponseServer
            .serve(server_transport)
            .await
            .expect("slow response server starts");
        running.waiting().await.expect("slow response server runs");
    });
    let client_service: rmcp::service::RunningService<RoleClient, ()> = ()
        .serve(client_transport)
        .await
        .expect("slow response client starts");
    let peer = client_service.peer().clone();

    let pool = Arc::new(UpstreamPool::new().with_request_timeout(Duration::from_millis(25)));
    let upstream_name_arc: Arc<str> = Arc::from(upstream_name);
    let mut entry = healthy_in_process_entry(Arc::clone(&upstream_name_arc), HashMap::new());
    entry.prompt_count = 1;
    entry.resource_count = 1;
    entry.prompt_names = vec!["slow.prompt".to_string()];
    entry.resource_uris = vec!["file:///tmp/slow".to_string()];
    pool.catalog
        .write()
        .await
        .insert(upstream_name.to_string(), entry);
    pool.connections.write().await.insert(
        upstream_name.to_string(),
        UpstreamConnection {
            _client_service: client_service,
            _server_task: Some(server_task),
            peer,
            runtime: UpstreamRuntimeMetadata::default(),
        },
    );
    pool.resource_upstreams
        .write()
        .await
        .push(upstream_name.to_string());

    pool
}

impl UpstreamPool {
    /// Register an in-process upstream whose advertised tool list is backed by a
    /// shared `Arc<RwLock<Vec<String>>>`, so a test can mutate the live tool set
    /// after connection and exercise live-catalog refresh.
    pub async fn insert_live_tool_server_for_tests(
        &self,
        upstream_name: &str,
        tools: Arc<tokio::sync::RwLock<Vec<String>>>,
    ) {
        struct MutableToolCatalogServer {
            tools: Arc<tokio::sync::RwLock<Vec<String>>>,
        }

        impl ServerHandler for MutableToolCatalogServer {
            fn get_info(&self) -> ServerInfo {
                ServerInfo::new(ServerCapabilities::builder().enable_tools().build())
            }

            async fn list_tools(
                &self,
                _request: Option<PaginatedRequestParams>,
                _context: RequestContext<RoleServer>,
            ) -> Result<ListToolsResult, ErrorData> {
                let tools = self
                    .tools
                    .read()
                    .await
                    .iter()
                    .map(|name| {
                        rmcp::model::Tool::new(
                            name.to_string(),
                            format!("{name} description"),
                            Arc::new(serde_json::Map::new()),
                        )
                    })
                    .collect::<Vec<_>>();
                Ok(ListToolsResult::with_all_items(tools))
            }
        }

        let (server_transport, client_transport) = tokio::io::duplex(IN_PROCESS_PEER_BUFFER_BYTES);
        let server = MutableToolCatalogServer { tools };
        let server_task = tokio::spawn(async move {
            let running = server
                .serve(server_transport)
                .await
                .expect("mutable tool catalog server starts");
            running
                .waiting()
                .await
                .expect("mutable tool catalog server runs");
        });
        let client_service: rmcp::service::RunningService<RoleClient, ()> = ()
            .serve(client_transport)
            .await
            .expect("mutable tool catalog client starts");
        let peer = client_service.peer().clone();

        self.catalog
            .write()
            .await
            .entry(upstream_name.to_string())
            .or_insert_with(|| healthy_in_process_entry(Arc::from(upstream_name), HashMap::new()));
        self.connections.write().await.insert(
            upstream_name.to_string(),
            UpstreamConnection {
                _client_service: client_service,
                _server_task: Some(server_task),
                peer,
                runtime: UpstreamRuntimeMetadata::default(),
            },
        );
    }
}
