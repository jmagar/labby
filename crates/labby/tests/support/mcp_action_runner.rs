use std::collections::BTreeSet;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use labby_gateway::upstream::http_client::BodyCappedHttpClient;
use rmcp::model::{
    CallToolRequestParams, CallToolResult, ClientCapabilities, ClientInfo, ElicitRequestParams,
    ElicitResult, ElicitationAction, ElicitationCapability, ElicitationSchema,
    FormElicitationCapability, Implementation, PaginatedRequestParams, PrimitiveSchemaDefinition,
    ProtocolVersion,
};
use rmcp::service::{ClientLifecycleMode, ClientServiceExt, RequestContext, RunningService};
use rmcp::transport::TokioChildProcess;
use rmcp::transport::streamable_http_client::{
    StreamableHttpClientTransportConfig, StreamableHttpClientWorker,
};
use rmcp::{ClientHandler, ErrorData, RoleClient};

use crate::support::{CleanupResult, LiveLabbyBuilder, LiveLabbyGuard};

pub(crate) const MAX_PAGES: usize = 8;
pub(crate) const MAX_TOOLS: usize = 64;
pub(crate) const MAX_RESPONSE_BYTES: usize = 1024 * 1024;
pub(crate) const MAX_CONCURRENCY: usize = 4;
pub(crate) const MAX_OUTSTANDING: usize = 8;
pub(crate) const REQUEST_TIMEOUT: Duration = Duration::from_secs(10);
const TEST_TOKEN: &str = "live-mcp-action-matrix-token";
const _: () = assert!(MAX_CONCURRENCY > 0 && MAX_CONCURRENCY <= 4);
const _: () = assert!(MAX_OUTSTANDING >= MAX_CONCURRENCY && MAX_OUTSTANDING <= 8);

#[derive(Clone, Default)]
struct ExactDestructiveConfirmationClient {
    expected_messages: Arc<Mutex<BTreeSet<String>>>,
}

impl ExactDestructiveConfirmationClient {
    fn expect(&self, service: &str, action: &str) -> String {
        let message = format!(
            "Action `{service}.{action}` is destructive and cannot be undone. Set `confirm` to true to proceed."
        );
        self.expected_messages
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .insert(message.clone());
        message
    }

    fn clear(&self, message: &str) {
        self.expected_messages
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .remove(message);
    }
}

impl ClientHandler for ExactDestructiveConfirmationClient {
    fn get_info(&self) -> ClientInfo {
        ClientInfo::new(
            ClientCapabilities::builder()
                .enable_elicitation_with(
                    ElicitationCapability::new().with_form(FormElicitationCapability::new()),
                )
                .build(),
            Implementation::new("labby-live-e2e", "1.0.0"),
        )
        .with_protocol_version(ProtocolVersion::V_2026_07_28)
    }

    async fn create_elicitation(
        &self,
        request: ElicitRequestParams,
        _context: RequestContext<RoleClient>,
    ) -> Result<ElicitResult, ErrorData> {
        let ElicitRequestParams::FormElicitationParams {
            message,
            requested_schema,
            ..
        } = request
        else {
            return Err(ErrorData::invalid_params(
                "only destructive form confirmation is accepted",
                None,
            ));
        };
        let expected_schema = ElicitationSchema::builder()
            .required_property(
                "confirm",
                PrimitiveSchemaDefinition::Boolean(rmcp::model::BooleanSchema::default()),
            )
            .build()
            .map_err(|error| ErrorData::internal_error(error.to_string(), None))?;
        if requested_schema != expected_schema
            || !self
                .expected_messages
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .remove(&message)
        {
            return Err(ErrorData::invalid_params(
                "unexpected destructive confirmation request",
                None,
            ));
        }
        Ok(ElicitResult::new(ElicitationAction::Accept)
            .with_content(serde_json::json!({"confirm": true})))
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct IdentityTuple {
    pub(crate) issuer: String,
    pub(crate) subject: String,
    pub(crate) project: String,
    pub(crate) loadout: String,
    pub(crate) route: String,
    pub(crate) scopes: Vec<String>,
}
impl IdentityTuple {
    pub(crate) fn local_admin() -> Self {
        Self {
            issuer: "labby-static-bearer".into(),
            subject: "local-action-matrix".into(),
            project: "disposable".into(),
            loadout: "root".into(),
            route: "/mcp".into(),
            scopes: vec!["lab:read".into(), "lab:admin".into()],
        }
    }

    pub(crate) fn from_public(identity: &crate::live_identity::PublicIdentity) -> Self {
        Self {
            issuer: identity.issuer.clone(),
            subject: identity.subject.clone(),
            project: identity.project_id.clone(),
            loadout: identity.loadout_id.clone(),
            route: identity.route_id.clone(),
            scopes: identity.scopes.clone(),
        }
    }

    pub(crate) fn fingerprint(&self) -> String {
        use sha2::{Digest as _, Sha256};
        let material = format!(
            "{}\0{}\0{}\0{}\0{}\0{}",
            self.issuer,
            self.subject,
            self.project,
            self.loadout,
            self.route,
            self.scopes.join(",")
        );
        hex::encode(Sha256::digest(material.as_bytes()))
    }
}

pub(crate) struct BuiltinMcpRunner {
    guard: Option<LiveLabbyGuard>,
    service: Option<RunningService<RoleClient, ExactDestructiveConfirmationClient>>,
    confirmation_client: ExactDestructiveConfirmationClient,
    identity: IdentityTuple,
    concurrency: tokio::sync::Semaphore,
    outstanding: tokio::sync::Semaphore,
    stdio_process: Option<(u32, String)>,
}

fn capped_http_client() -> BodyCappedHttpClient {
    // Each integration-test binary is a separate process and this constructor
    // is also exercised directly by unit tests before a runner is started.
    // Install the same provider used by the product before reqwest constructs
    // its rustls client so parallel nextest execution cannot observe an
    // uninitialized process-global provider.
    drop(rustls::crypto::ring::default_provider().install_default());
    BodyCappedHttpClient::new(reqwest::Client::new(), MAX_RESPONSE_BYTES)
}

impl BuiltinMcpRunner {
    pub(crate) async fn start() -> Result<Self, String> {
        Self::start_with_config(None).await
    }

    pub(crate) async fn start_code_mode() -> Result<Self, String> {
        Self::start_with_config(Some("[code_mode]\nenabled = true\n")).await
    }

    pub(crate) async fn start_stdio(command: std::process::Command) -> Result<Self, String> {
        let transport = TokioChildProcess::new(tokio::process::Command::from(command))
            .map_err(|error| error.to_string())?;
        let stdio_process = transport.id().map(|pid| (pid, process_start_identity(pid)));
        let confirmation_client = ExactDestructiveConfirmationClient::default();
        let service = tokio::time::timeout(
            REQUEST_TIMEOUT,
            confirmation_client.clone().serve_with_lifecycle(
                transport,
                ClientLifecycleMode::Discover {
                    preferred_versions: vec![ProtocolVersion::V_2026_07_28],
                },
            ),
        )
        .await
        .map_err(|_| "stdio MCP initialize timed out".to_string())?
        .map_err(|error| error.to_string())?;
        Ok(Self {
            guard: None,
            service: Some(service),
            confirmation_client,
            identity: IdentityTuple::local_admin(),
            concurrency: tokio::sync::Semaphore::new(MAX_CONCURRENCY),
            outstanding: tokio::sync::Semaphore::new(MAX_OUTSTANDING),
            stdio_process,
        })
    }

    async fn start_with_config(config_text: Option<&str>) -> Result<Self, String> {
        drop(rustls::crypto::ring::default_provider().install_default());
        let mut builder = LiveLabbyBuilder::new()
            .env("LABBY_MCP_HTTP_TOKEN", TEST_TOKEN)
            .env("LABBY_E2E_BOOTSTRAP_STATIC_OWNER", "1")
            .env("LABBY_E2E_TEAM_ID", "bootstrap-initial-team")
            // The host's optional Claude installation must not change the
            // action-matrix result. Exercise the declared unavailable-inventory
            // contract deterministically on every CI runner.
            .env(
                "LABBY_CLAUDE_BIN",
                "/definitely/not/a/labby-e2e-claude-binary",
            );
        if let Some(config_text) = config_text {
            builder = builder.config(config_text);
        }
        let guard = builder.start().await?;
        let endpoint = format!("{}/mcp", guard.connection().base_url);
        let mut config = StreamableHttpClientTransportConfig::with_uri(endpoint);
        config.auth_header = Some(TEST_TOKEN.to_string());
        config.custom_headers.insert(
            "x-labby-project-id".parse().expect("project header name"),
            "disposable".parse().expect("project header value"),
        );
        let worker = StreamableHttpClientWorker::new(capped_http_client(), config);
        let confirmation_client = ExactDestructiveConfirmationClient::default();
        let service = tokio::time::timeout(
            REQUEST_TIMEOUT,
            confirmation_client.clone().serve_with_lifecycle(
                worker,
                ClientLifecycleMode::Discover {
                    preferred_versions: vec![ProtocolVersion::V_2026_07_28],
                },
            ),
        )
        .await
        .map_err(|_| "MCP initialize timed out".to_string())?
        .map_err(|error| error.to_string())?;
        Ok(Self {
            guard: Some(guard),
            service: Some(service),
            confirmation_client,
            identity: IdentityTuple::local_admin(),
            concurrency: tokio::sync::Semaphore::new(MAX_CONCURRENCY),
            outstanding: tokio::sync::Semaphore::new(MAX_OUTSTANDING),
            stdio_process: None,
        })
    }

    pub(crate) async fn connect_project(
        base: &str,
        credential: &str,
        identity: IdentityTuple,
    ) -> Result<Self, String> {
        let local = reqwest::Url::parse(base).map_err(|error| error.to_string())?;
        let local_host = local.host_str().ok_or("project MCP base has no host")?;
        let local_port = local
            .port_or_known_default()
            .ok_or("project MCP base has no port")?;
        let local_address = format!("{local_host}:{local_port}")
            .parse()
            .map_err(|error: std::net::AddrParseError| error.to_string())?;
        // Keep the public virtual-host authority while connecting the disposable
        // listener port through reqwest's resolver.
        let endpoint = format!("http://mcp.example.test:{local_port}/operator");
        let http_client = reqwest::Client::builder()
            .no_proxy()
            .http1_only()
            .resolve("mcp.example.test", local_address)
            .build()
            .map_err(|error| error.to_string())?;
        let mut config = StreamableHttpClientTransportConfig::with_uri(endpoint);
        config.auth_header = Some(credential.to_string());
        let worker = StreamableHttpClientWorker::new(
            BodyCappedHttpClient::new(http_client, MAX_RESPONSE_BYTES),
            config,
        );
        let confirmation_client = ExactDestructiveConfirmationClient::default();
        let service = tokio::time::timeout(
            REQUEST_TIMEOUT,
            confirmation_client.clone().serve_with_lifecycle(
                worker,
                ClientLifecycleMode::Discover {
                    preferred_versions: vec![ProtocolVersion::V_2026_07_28],
                },
            ),
        )
        .await
        .map_err(|_| "project MCP initialize timed out".to_string())?
        .map_err(|error| error.to_string())?;
        Ok(Self {
            guard: None,
            service: Some(service),
            confirmation_client,
            identity,
            concurrency: tokio::sync::Semaphore::new(MAX_CONCURRENCY),
            outstanding: tokio::sync::Semaphore::new(MAX_OUTSTANDING),
            stdio_process: None,
        })
    }

    pub(crate) fn identity_fingerprint(&self) -> String {
        self.identity.fingerprint()
    }

    pub(crate) async fn list_tool_names(&self) -> Result<BTreeSet<String>, String> {
        let peer = self.service.as_ref().expect("runner active").peer();
        let deadline = tokio::time::Instant::now() + REQUEST_TIMEOUT;
        let mut cursor = None;
        let mut tools = BTreeSet::new();
        for _ in 0..MAX_PAGES {
            let params = cursor
                .take()
                .map(|cursor| PaginatedRequestParams::default().with_cursor(Some(cursor)));
            let page = tokio::time::timeout_at(deadline, peer.list_tools(params))
                .await
                .map_err(|_| "tools/list timed out".to_string())?
                .map_err(|error| error.to_string())?;
            for tool in page.tools {
                if !tools.insert(tool.name.into_owned()) {
                    return Err("tools/list returned a duplicate tool".into());
                }
                if tools.len() > MAX_TOOLS {
                    return Err("tools/list exceeded the item bound".into());
                }
            }
            cursor = page.next_cursor;
            if cursor.is_none() {
                return Ok(tools);
            }
        }
        Err("tools/list exceeded the page bound".into())
    }

    pub(crate) async fn tool_contract(&self, expected: &str) -> Result<Option<String>, String> {
        let peer = self.service.as_ref().expect("runner active").peer();
        let deadline = tokio::time::Instant::now() + REQUEST_TIMEOUT;
        let mut cursor = None;
        let mut count = 0usize;
        for _ in 0..MAX_PAGES {
            let params = cursor
                .take()
                .map(|cursor| PaginatedRequestParams::default().with_cursor(Some(cursor)));
            let page = tokio::time::timeout_at(deadline, peer.list_tools(params))
                .await
                .map_err(|_| "tools/list timed out".to_string())?
                .map_err(|error| error.to_string())?;
            for tool in page.tools {
                count += 1;
                if count > MAX_TOOLS {
                    return Err("tools/list exceeded the item bound".into());
                }
                if tool.name.as_ref() == expected {
                    return serde_json::to_string(&tool)
                        .map(Some)
                        .map_err(|error| error.to_string());
                }
            }
            cursor = page.next_cursor;
            if cursor.is_none() {
                return Ok(None);
            }
        }
        Err("tools/list exceeded the page bound".into())
    }

    pub(crate) async fn call(
        &self,
        service: &str,
        action: &str,
        mut params: serde_json::Map<String, serde_json::Value>,
    ) -> Result<CallToolResult, String> {
        if service == "gateway"
            && (action.starts_with("gateway.loadout.")
                || action.starts_with("gateway.protected_route."))
        {
            params
                .entry("team_id")
                .or_insert_with(|| serde_json::Value::String("bootstrap-initial-team".to_owned()));
        }
        let deadline = tokio::time::Instant::now() + REQUEST_TIMEOUT;
        let _outstanding = self
            .outstanding
            .try_acquire()
            .map_err(|_| "MCP action runner outstanding request bound exceeded".to_string())?;
        let _permit = tokio::time::timeout_at(deadline, self.concurrency.acquire())
            .await
            .map_err(|_| "tools/call timed out while queued".to_string())?
            .map_err(|_| "MCP action runner is shutting down".to_string())?;
        let arguments = serde_json::json!({"action": action, "params": params})
            .as_object()
            .expect("object")
            .clone();
        let request = CallToolRequestParams::new(service.to_string()).with_arguments(arguments);
        let expected_confirmation = self.confirmation_client.expect(service, action);
        let result = tokio::time::timeout_at(
            deadline,
            self.service
                .as_ref()
                .expect("runner active")
                .call_tool(request),
        )
        .await
        .map_err(|_| "tools/call timed out".to_string())
        .and_then(|result| result.map_err(|error| error.to_string()));
        self.confirmation_client.clear(&expected_confirmation);
        let result = result?;
        Ok(result)
    }

    pub(crate) async fn finish(mut self) -> CleanupResult {
        let cancellation_failure = if let Some(service) = self.service.take() {
            match tokio::time::timeout(REQUEST_TIMEOUT, service.cancel()).await {
                Ok(Ok(_)) => None,
                Ok(Err(error)) => Some(format!("MCP cancellation failed: {error}")),
                Err(_) => Some(format!("MCP cancellation exceeded {REQUEST_TIMEOUT:?}")),
            }
        } else {
            None
        };
        let mut cleanup = match self.guard.take() {
            Some(guard) => guard.finish().await,
            None => CleanupResult::default(),
        };
        if let Some(error) = cancellation_failure {
            cleanup.failures.push(error);
        }
        if let Some((pid, identity)) = self.stdio_process.take()
            && !wait_for_process_exit(pid, &identity).await
        {
            cleanup.failures.push(format!(
                "stdio MCP child {pid} retained its original process identity after cancellation"
            ));
        }
        cleanup
    }

    pub(crate) async fn disconnect(mut self) {
        if let Some(service) = self.service.take() {
            drop(tokio::time::timeout(REQUEST_TIMEOUT, service.cancel()).await);
        }
    }
}

#[cfg(unix)]
fn process_start_identity(pid: u32) -> String {
    std::process::Command::new("ps")
        .args(["-o", "lstart=", "-p", &pid.to_string()])
        .output()
        .ok()
        .filter(|output| output.status.success())
        .map(|output| String::from_utf8_lossy(&output.stdout).trim().to_owned())
        .filter(|identity| !identity.is_empty())
        .unwrap_or_else(|| "absent".to_string())
}

#[cfg(windows)]
fn process_start_identity(pid: u32) -> String {
    if labby_winjob::pid_is_alive(pid) {
        format!("pid:{pid}:alive")
    } else {
        format!("pid:{pid}:absent")
    }
}

async fn wait_for_process_exit(pid: u32, identity: &str) -> bool {
    let deadline = tokio::time::Instant::now() + REQUEST_TIMEOUT;
    loop {
        if process_start_identity(pid) != identity {
            return true;
        }
        if tokio::time::Instant::now() >= deadline {
            return false;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn transport_body_cap_matches_runner_contract() {
        assert_eq!(capped_http_client().max_bytes(), MAX_RESPONSE_BYTES);
    }

    #[tokio::test]
    async fn outstanding_admission_is_bounded_without_an_unbounded_queue() {
        let outstanding = tokio::sync::Semaphore::new(MAX_OUTSTANDING);
        let held = (0..MAX_OUTSTANDING)
            .map(|_| outstanding.try_acquire())
            .collect::<Result<Vec<_>, _>>()
            .expect("configured outstanding slots");
        assert!(outstanding.try_acquire().is_err());
        drop(held);
        assert!(outstanding.try_acquire().is_ok());
    }

    #[tokio::test]
    async fn queue_wait_consumes_the_absolute_request_deadline() {
        let concurrency = tokio::sync::Semaphore::new(1);
        let held = concurrency.acquire().await.expect("initial permit");
        let deadline = tokio::time::Instant::now() + Duration::from_millis(20);
        let queued = tokio::time::timeout_at(deadline, concurrency.acquire()).await;
        assert!(queued.is_err());
        drop(held);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn stdio_process_identity_observes_actual_termination() {
        let mut child = tokio::process::Command::new("sleep")
            .arg("30")
            .spawn()
            .expect("owned child");
        let pid = child.id().expect("child pid");
        let identity = process_start_identity(pid);
        assert_ne!(identity, "absent");
        child.kill().await.expect("kill owned child");
        child.wait().await.expect("reap owned child");
        assert!(wait_for_process_exit(pid, &identity).await);
    }
}
