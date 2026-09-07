//! `LabMcpServer` — the MCP `ServerHandler` implementation.
//!
//! Extracted from `cli/serve.rs` so that both the stdio and HTTP transports
//! can share the same handler logic.

use std::borrow::Cow;
use std::sync::atomic::{AtomicU8, AtomicU64, Ordering};
use std::sync::{Arc, OnceLock};

use axum::http;
use dashmap::DashMap;
use labby_primitives::mcp::{
    MCP_RELAY_CANCELLATION_REQUEST_METHOD, MCP_RELAY_CANCELLATION_TOKEN_META_KEY,
};
// Not feature-gated: `mcp_extensions()` is unconditional so a build with one
// extension and not another still advertises the one it has.
use rmcp::model::ExtensionCapabilities;
use rmcp::model::{
    CacheScope, CallToolRequestParams, CallToolResponse, CancelTaskParams,
    CancelledNotificationParam, CompleteRequestParams, CompleteResult, CustomRequest, CustomResult,
    DiscoverResult, GetPromptRequestParams, GetPromptResponse, GetTaskParams, GetTaskResult,
    InitializeRequestParams, InitializeResult, ListPromptsResult, ListResourceTemplatesResult,
    ListResourcesResult, ListToolsResult, PaginatedRequestParams, ProtocolVersion,
    ReadResourceRequestParams, ReadResourceResponse, RequestMetaObject, ServerCapabilities,
    ServerInfo, SubscriptionFilter, UpdateTaskParams,
};
use rmcp::service::{NotificationContext, RequestContext, SubscriptionContext};
use rmcp::{ErrorData, RoleServer, ServerHandler};
use tokio::sync::RwLock;
use tokio_util::sync::CancellationToken;

use crate::access::AccessRuntime;
#[cfg(feature = "gateway")]
use crate::dispatch::gateway::manager::GatewayManager;
use crate::mcp::context::{actor_key_from_extensions, subject_from_extensions};
use crate::mcp::provenance;
use crate::mcp::route_scope::McpRouteScope;
use crate::mcp::runtime::McpRouteRuntime;
use crate::registry::ToolRegistry;

/// Process-global counter minting a unique `relay_session_id` per
/// `LabMcpServer` instance.
///
/// **The instance lifetime differs by transport, and the id inherits that.** On
/// stdio one `LabMcpServer` serves the whole process, so the id is genuinely
/// stable for the session. On streamable HTTP the service factory runs *per
/// POST* (rmcp `StreamableHttpService::handle_post` calls `get_service()` for
/// each request under the stateless mode Labby configures — see
/// `build_mcp_service`), so a fresh instance and a fresh id are minted per
/// request; all of them share one peer registry via `PeerNotifier`.
///
/// The id therefore binds a cached upstream relay connection to one downstream
/// agent without ever reusing it across agents — which holds on both
/// transports — but it must not be read as a stable per-connection session
/// identity on HTTP.
static RELAY_SESSION_COUNTER: AtomicU64 = AtomicU64::new(1);

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
enum ActiveRequestKey {
    Relay(String),
    Authenticated {
        scope: String,
        request_id: rmcp::model::RequestId,
    },
    Session {
        relay_session_id: u64,
        request_id: rmcp::model::RequestId,
    },
}

pub(crate) struct ActiveRequestCancellation {
    token: CancellationToken,
}

static ACTIVE_REQUEST_CANCELLATIONS: OnceLock<
    DashMap<ActiveRequestKey, Vec<Arc<ActiveRequestCancellation>>>,
> = OnceLock::new();

fn active_request_cancellations()
-> &'static DashMap<ActiveRequestKey, Vec<Arc<ActiveRequestCancellation>>> {
    ACTIVE_REQUEST_CANCELLATIONS.get_or_init(DashMap::new)
}

#[derive(serde::Deserialize)]
struct RelayCancellationNotification {
    token: String,
}

fn cancel_tracked_request_by_token(token: &str) -> bool {
    let key = ActiveRequestKey::Relay(token.to_string());
    let Some((_, cancellations)) = active_request_cancellations().remove(&key) else {
        return false;
    };
    for cancellation in cancellations {
        cancellation.token.cancel();
    }
    true
}

fn authenticated_cancellation_scope(extensions: &rmcp::model::Extensions) -> Option<String> {
    actor_key_from_extensions(extensions)
        .map(|actor| format!("actor:{actor}"))
        .or_else(|| subject_from_extensions(extensions).map(|subject| format!("subject:{subject}")))
}

fn cancellation_token_from_meta(meta: &rmcp::model::NotificationMetaObject) -> Option<&str> {
    meta.0
        .0
        .get(MCP_RELAY_CANCELLATION_TOKEN_META_KEY)
        .and_then(serde_json::Value::as_str)
}

/// Restore the canonical wire metadata that rmcp places on the request context,
/// while preserving metadata supplied by a direct typed-handler caller when the
/// context carries no metadata of its own.
fn restore_request_meta(typed: &mut Option<RequestMetaObject>, context: &RequestMetaObject) {
    if !context.0.0.is_empty() || typed.is_none() {
        *typed = Some(context.clone());
    }
}

fn request_cancellation_key(
    context: &RequestContext<RoleServer>,
    relay_session_id: u64,
) -> ActiveRequestKey {
    if let Some(token) = context
        .meta
        .0
        .0
        .get(MCP_RELAY_CANCELLATION_TOKEN_META_KEY)
        .and_then(serde_json::Value::as_str)
    {
        return ActiveRequestKey::Relay(token.to_string());
    }

    authenticated_cancellation_scope(&context.extensions).map_or_else(
        || ActiveRequestKey::Session {
            relay_session_id,
            request_id: context.id.clone(),
        },
        |scope| ActiveRequestKey::Authenticated {
            scope,
            request_id: context.id.clone(),
        },
    )
}

fn notification_cancellation_key(
    notification: &CancelledNotificationParam,
    context: &NotificationContext<RoleServer>,
    relay_session_id: u64,
) -> Option<ActiveRequestKey> {
    let token = notification
        .meta
        .as_ref()
        .and_then(cancellation_token_from_meta)
        .or_else(|| cancellation_token_from_meta(&context.meta))
        .or_else(|| {
            context
                .extensions
                .get::<rmcp::model::NotificationMetaObject>()
                .and_then(cancellation_token_from_meta)
        });
    if let Some(token) = token {
        return Some(ActiveRequestKey::Relay(token.to_string()));
    }

    let request_id = notification.request_id.as_ref()?;
    Some(
        authenticated_cancellation_scope(&context.extensions).map_or_else(
            || ActiveRequestKey::Session {
                relay_session_id,
                request_id: request_id.clone(),
            },
            |scope| ActiveRequestKey::Authenticated {
                scope,
                request_id: request_id.clone(),
            },
        ),
    )
}

#[derive(Clone)]
pub(crate) struct LabRequestCancellation(Arc<ActiveRequestCancellation>);

impl LabRequestCancellation {
    pub(crate) fn token(&self) -> CancellationToken {
        self.0.token.clone()
    }
}

struct ActiveRequestCancellationGuard {
    key: Option<ActiveRequestKey>,
    cancellation: Arc<ActiveRequestCancellation>,
}

impl ActiveRequestCancellationGuard {
    fn cancellation(&self) -> Arc<ActiveRequestCancellation> {
        Arc::clone(&self.cancellation)
    }
}

impl Drop for ActiveRequestCancellationGuard {
    fn drop(&mut self) {
        if let Some(key) = self.key.take() {
            active_request_cancellations().remove_if_mut(&key, |_, current| {
                current.retain(|candidate| !Arc::ptr_eq(candidate, &self.cancellation));
                current.is_empty()
            });
        }
    }
}

fn track_request_cancellation(
    context: &RequestContext<RoleServer>,
    relay_session_id: u64,
) -> ActiveRequestCancellationGuard {
    let key = request_cancellation_key(context, relay_session_id);
    let cancellation = Arc::new(ActiveRequestCancellation {
        token: context.ct.child_token(),
    });
    // Independent stateless clients authenticated as the same actor may reuse
    // the same JSON-RPC id concurrently. The fallback actor/id key cannot
    // distinguish those requests, so retain every registration and fail safe
    // by cancelling all matches. Private relay UUID tokens remain exact.
    active_request_cancellations()
        .entry(key.clone())
        .or_default()
        .push(Arc::clone(&cancellation));
    ActiveRequestCancellationGuard {
        key: Some(key),
        cancellation,
    }
}

fn cancel_tracked_request(
    notification: &CancelledNotificationParam,
    context: &NotificationContext<RoleServer>,
    relay_session_id: u64,
) -> bool {
    let key = notification_cancellation_key(notification, context, relay_session_id);
    let Some(key) = key else {
        return false;
    };
    let Some((_, cancellations)) = active_request_cancellations().remove(&key) else {
        return false;
    };
    for cancellation in cancellations {
        cancellation.token.cancel();
    }
    true
}

/// Mint the next unique relay-session id. Called once per `LabMcpServer`.
pub(crate) fn next_relay_session_id() -> u64 {
    RELAY_SESSION_COUNTER.fetch_add(1, Ordering::Relaxed)
}

/// Transports on which a missing `AuthContext` means "trusted local
/// operator" rather than "this hop carries no auth layer".
///
/// Owned here, beside the `transport_label` field it interprets, so the
/// policy has one home and does not depend on any feature-gated module.
/// Adding a transport is an explicit trust decision: an unlisted label
/// fails closed, and `requires_admin` builtin actions are refused on it.
/// See [`LabMcpServer::absent_auth_trust`].
/// `"test"` is used only by `#[cfg(test)]` server fixtures, so the suite
/// exercises the stdio branch; verify with `rg 'transport_label: "test"'`
/// before any production site adopts it.
pub(crate) const TRANSPORTS_TRUSTING_ABSENT_AUTH: &[&str] = &["stdio", "http", "test"];

const MAX_TOOL_CONTRACT_SUBJECTS: usize = 256;
const MAX_TOOL_CONTRACTS_PER_SUBJECT: usize = 8;

#[derive(Default)]
pub(crate) struct ToolContractBaselineStore {
    subjects: std::collections::HashMap<
        Option<String>,
        std::collections::VecDeque<crate::mcp::catalog::ToolCatalogSnapshot>,
    >,
    subject_order: std::collections::VecDeque<Option<String>>,
}

impl ToolContractBaselineStore {
    pub(crate) fn publish(
        &mut self,
        subject: Option<String>,
        snapshot: crate::mcp::catalog::ToolCatalogSnapshot,
    ) {
        if !self.subjects.contains_key(&subject) {
            while self.subjects.len() >= MAX_TOOL_CONTRACT_SUBJECTS {
                if let Some(evicted) = self.subject_order.pop_front() {
                    self.subjects.remove(&evicted);
                }
            }
            self.subject_order.push_back(subject.clone());
        }
        let candidates = self.subjects.entry(subject).or_default();
        while candidates.len() >= MAX_TOOL_CONTRACTS_PER_SUBJECT {
            candidates.pop_front();
        }
        candidates.push_back(snapshot);
    }

    /// Claim a baseline only when one unambiguous completed list exists for
    /// this subject. Stateless requests carry no conversation identifier; if
    /// concurrent conversations produced multiple candidates, returning None
    /// deliberately causes a conservative catch-up notification rather than
    /// attributing another conversation's list and missing a real change.
    pub(crate) fn claim_unambiguous(
        &mut self,
        subject: &Option<String>,
    ) -> Option<crate::mcp::catalog::ToolCatalogSnapshot> {
        let candidates = self.subjects.get_mut(subject)?;
        let claimed = if candidates.len() == 1 {
            candidates.pop_front()
        } else {
            candidates.clear();
            None
        };
        if candidates.is_empty() {
            self.subjects.remove(subject);
            self.subject_order.retain(|candidate| candidate != subject);
        }
        claimed
    }

    #[cfg(test)]
    pub(crate) fn candidate_count(&self, subject: &Option<String>) -> usize {
        self.subjects
            .get(subject)
            .map_or(0, std::collections::VecDeque::len)
    }
}

pub(crate) type ToolContractBaselines = Arc<RwLock<ToolContractBaselineStore>>;

/// MCP server handler — one tool per registered service.
pub struct LabMcpServer {
    pub registry: Arc<ToolRegistry>,
    /// Process-scoped access-control lifecycle owner shared by every MCP
    /// handler on this route. Adapters must carry this exact allocation; they
    /// must not independently reopen access persistence or make policy
    /// decisions while constructing a request handler.
    #[allow(dead_code)] // Consumed by the next project-binding/enforcement wave.
    pub(crate) access_runtime: Arc<AccessRuntime>,
    /// Process-owned File Stash runtime shared by every surface adapter.
    pub(crate) file_stash_runtime: Arc<crate::file_stash::FileStashRuntime>,
    /// Shared gateway manager used to resolve the current live upstream pool.
    #[cfg(feature = "gateway")]
    pub gateway_manager: Option<Arc<GatewayManager>>,
    /// Active subscription sinks for list-changed notifications.
    pub peers: crate::mcp::peers::PeerRegistry,
    /// Gateway-wide switch for the explicit Code Mode MCP App surface.
    pub(crate) code_mode_app_state: crate::mcp::catalog::CodeModeAppState,
    /// Complete tool contracts most recently published to callers of this MCP
    /// route, keyed by authenticated subject. Stateless HTTP constructs a new
    /// handler per request, so the factory shares this registry across those
    /// handlers. Partial pagination never advances a baseline.
    pub(crate) last_listed_tool_contract: ToolContractBaselines,
    /// Long-lived state shared by every request handler on this MCP route.
    /// Stateless HTTP creates a fresh handler per POST, so paginated live
    /// catalogs resume through this state instead of repeating upstream I/O.
    pub(crate) route_runtime: Arc<McpRouteRuntime>,
    /// Observed inbound MCP client registry — shared with `GatewayManager`
    /// via `with_client_registry` so `gateway.clients.list` can read it.
    #[cfg(feature = "gateway")]
    pub client_registry: labby_runtime::client_registry::ClientRegistryHandle,
    /// This route's transport, recorded verbatim into
    /// `ConnectedClient::transport` during discovery. One of `"stdio"`,
    /// `"http"`, `"in-process"` (built-in service peers), or `"test"`.
    pub(crate) transport_label: &'static str,
    /// Negotiated RMCP logging threshold for this server route.
    pub logging_level: Arc<AtomicU8>,
    /// Visibility and dispatch constraints for this MCP route.
    pub(crate) route_scope: McpRouteScope,
    /// Unique id for this handler's downstream relay scope. On stdio that is
    /// the connection lifetime; on stateless HTTP it is deliberately one POST,
    /// because the downstream peer dies with that response and must never be
    /// reused for a later interactive relay. The historical field name is kept
    /// to avoid a noisy mechanical rename.
    pub(crate) relay_session_id: u64,
    #[cfg(test)]
    pub(crate) code_mode_widget_callbacks_enabled_for_test: bool,
}

#[cfg(feature = "gateway")]
pub fn verify_upstream_subject_resolution_support() -> anyhow::Result<()> {
    let (parts, _) = http::Request::new(()).into_parts();
    let auth = labby_auth::auth_context::AuthContext {
        sub: "startup-self-test".to_string(),
        actor_key: None,
        scopes: Vec::new(),
        issuer: "https://lab.example.com".to_string(),
        via_session: false,
        csrf_token: None,
        email: None,
    };

    let mut extensions = rmcp::model::Extensions::new();
    let mut parts = parts;
    parts.extensions.insert(auth);
    extensions.insert(parts);

    if subject_from_extensions(&extensions) == Some("startup-self-test") {
        return Ok(());
    }

    anyhow::bail!(
        "rmcp subject extraction self-test failed: RequestContext.extensions did not yield \
         http::request::Parts/AuthContext. The current runtime expects rmcp 3 request \
         extension propagation. Wire the tokio::task_local fallback or pin \
         a compatible rmcp version before starting."
    );
}

/// Advertise the MCP Apps UI extension (`io.modelcontextprotocol/ui`, SEP-1724)
/// so hosts like Claude.ai know to render the Code Mode inspector widgets served
/// at `ui://lab/code-mode/{search,execute,history}`. The `mimeTypes` value mirrors
/// the MIME the widget resources are published with (`text/html;profile=mcp-app`).
#[cfg(feature = "gateway")]
fn mcp_apps_ui_extension() -> ExtensionCapabilities {
    let mut extensions = ExtensionCapabilities::new();
    let mut ui_ext = serde_json::Map::new();
    ui_ext.insert(
        "mimeTypes".to_string(),
        serde_json::json!([crate::mcp::handlers_resources::CODE_MODE_APP_MIME]),
    );
    extensions.insert("io.modelcontextprotocol/ui".to_string(), ui_ext);
    extensions
}

/// Extension capabilities advertised in `initialize`.
///
/// Deliberately not gated as a whole: each extension carries its own `cfg` so a
/// build that enables one and not another still advertises the one it has. The
/// previous whole-function gate meant a `--features skills` build without
/// `gateway` advertised no extensions block at all, and so could never announce
/// skills support.
fn mcp_extensions() -> ExtensionCapabilities {
    let mut extensions = ExtensionCapabilities::new();
    #[cfg(feature = "gateway")]
    extensions.extend(mcp_apps_ui_extension());
    #[cfg(feature = "skills")]
    extensions.insert(
        labby_runtime::skills::wire::SKILLS_EXTENSION_KEY.to_string(),
        // Empty object: supported, with no optional features. Labby does not
        // implement `resources/directory/read`, and a client must not call it
        // against a server that has not declared `directoryRead`.
        serde_json::Map::new(),
    );
    extensions
}

/// Build the `ConnectedClient` record for `server/discover` — pulled out of
/// the `ServerHandler` impl so redaction can be unit tested directly against
/// a fabricated `Extensions`/`AuthContext` without standing up a full
/// `NotificationContext<RoleServer>`.
///
/// The redaction step is the whole point of this function existing
/// separately: `subject_from_extensions` returns the raw authenticated
/// subject, and it must never reach `labby_runtime::client_registry`
/// unredacted. `connected_at` is threaded in rather than read here so this
/// stays pure and testable (`jiff::Timestamp::now()` at the one real call
/// site in `discover`).
#[cfg(feature = "gateway")]
fn connected_client_from_discovery(
    client_info: Option<rmcp::model::Implementation>,
    extensions: &rmcp::model::Extensions,
    transport_label: &str,
    connected_at: String,
) -> labby_runtime::client_registry::ConnectedClient {
    let subject_tag =
        subject_from_extensions(extensions).map(crate::mcp::context::redact_subject_for_logging);
    labby_runtime::client_registry::ConnectedClient {
        subject_tag,
        client_name: client_info.as_ref().map(|info| info.name.clone()),
        client_version: client_info.as_ref().map(|info| info.version.clone()),
        transport: transport_label.to_string(),
        connected_at,
    }
}

/// Strip capabilities a legacy (pre-2026-07-28) session cannot actually use.
///
/// MCP advertises a single `resources.subscribe` flag for two different
/// mechanisms: the deprecated `resources/subscribe` RPC pair, and the
/// `subscriptions/listen` stream that replaced it in 2026-07-28. `get_info`
/// must keep the flag set, because rmcp intersects a client's requested
/// `SubscriptionFilter` against it (`SubscriptionFilter::supported_by`) —
/// clearing it there would silence modern subscriptions, including the
/// gateway's own upstream negotiation.
///
/// A legacy session can reach neither mechanism: Labby implements no
/// `resources/subscribe` handler, so rmcp answers with `method_not_found`, and
/// rmcp gates `subscriptions/listen` to modern sessions. Advertising the flag
/// on this path therefore promises something that cannot work, so withhold it
/// for legacy sessions only.
///
/// Extracted from `initialize` so it can be unit tested against a fabricated
/// `ServerInfo` without standing up a gateway manager (the flag is
/// gateway-conditional in `get_info`).
fn withhold_legacy_unusable_capabilities(info: &mut ServerInfo) {
    if let Some(resources) = info.capabilities.resources.as_mut() {
        resources.subscribe = None;
    }
}

impl ServerHandler for LabMcpServer {
    fn supported_protocol_versions(&self) -> Cow<'static, [ProtocolVersion]> {
        Cow::Borrowed(ProtocolVersion::KNOWN_VERSIONS)
    }

    async fn initialize(
        &self,
        request: InitializeRequestParams,
        context: RequestContext<RoleServer>,
    ) -> Result<InitializeResult, ErrorData> {
        tracing::warn!(
            surface = "mcp",
            service = "labby",
            action = "lifecycle.compat_legacy_initialize",
            subsystem = "mcp_server",
            requested_protocol_version = %request.protocol_version,
            client_name = %request.client_info.name,
            client_version = %request.client_info.version,
            "adapting legacy MCP initialize lifecycle to the stateless server"
        );
        context.peer.set_peer_info(request.clone());
        let mut info = self.get_info();
        // RMCP adapts subsequent request validation and wire behavior from the
        // negotiated peer version. Echo the requested version because every
        // SDK-known historical version is explicitly declared above.
        info.protocol_version = request.protocol_version;
        // This is the only legacy entry point — modern clients negotiate via
        // `discover`, which returns `get_info()` untouched — so it is also the
        // only place a capability can be withheld from legacy sessions alone.
        withhold_legacy_unusable_capabilities(&mut info);
        Ok(info)
    }

    #[allow(deprecated)]
    fn get_info(&self) -> ServerInfo {
        #[cfg(feature = "gateway")]
        let gateway_manager_configured = self.gateway_manager.is_some();
        #[cfg(not(feature = "gateway"))]
        let gateway_manager_configured = false;
        tracing::info!(
            surface = "mcp",
            service = "labby",
            action = "server.info",
            subsystem = "mcp_server",
            phase = "server.info",
            builtin_service_count = self.registry.services().len(),
            gateway_manager_configured,
            "advertising MCP server capabilities"
        );
        let builder = ServerCapabilities::builder()
            .enable_tools()
            .enable_tool_list_changed()
            .enable_resources()
            .enable_resources_list_changed()
            .enable_prompts()
            .enable_prompts_list_changed()
            .enable_completions();
        let builder = builder.enable_extensions_with(mcp_extensions());
        #[cfg(feature = "gateway")]
        let builder = if gateway_manager_configured {
            builder.enable_resources_subscribe().enable_tasks()
        } else {
            builder
        };
        #[cfg(feature = "gateway")]
        let capabilities = builder.build();
        #[cfg(not(feature = "gateway"))]
        let capabilities = builder.build();
        let mut info = ServerInfo::new(capabilities);
        info.server_info = rmcp::model::Implementation::new("labby", env!("CARGO_PKG_VERSION"));
        // A pointer, not skill content. It reaches clients that never parse the
        // capability map, which is the population most likely to miss an
        // extension entirely.
        #[cfg(feature = "skills")]
        {
            info.instructions = Some(
                "This server implements the MCP Skills extension                  (io.modelcontextprotocol/skills). Call `skills/list` to enumerate its Agent                  Skills, or `skills/get` with a `skill://` URI to fetch one entry. The contract                  this server implements is readable at `lab://contracts/skills-extension`."
                    .to_string(),
            );
        }
        info
    }

    async fn on_cancelled(
        &self,
        notification: CancelledNotificationParam,
        context: NotificationContext<RoleServer>,
    ) {
        let request_id = notification.request_id.clone();
        let correlated = cancel_tracked_request(&notification, &context, self.relay_session_id);
        tracing::debug!(
            surface = "mcp",
            service = "labby",
            action = "request.cancel",
            ?request_id,
            correlated,
            "processed MCP cancellation notification"
        );
    }

    async fn on_custom_request(
        &self,
        request: CustomRequest,
        context: RequestContext<RoleServer>,
    ) -> Result<CustomResult, ErrorData> {
        #[cfg(not(feature = "skills"))]
        let _ = &context;

        // `context` is threaded rather than ignored: the skills methods below
        // are scope-gated, and once proxied skills land they route per OAuth
        // subject. A context-blind custom-request handler is how the in-process
        // peer ended up trusting every caller (lab-m01gl) — do not reintroduce
        // that shape here.
        #[cfg(feature = "skills")]
        if matches!(
            request.method.as_str(),
            labby_runtime::skills::wire::SKILLS_LIST_METHOD
                | labby_runtime::skills::wire::SKILLS_GET_METHOD
        ) {
            return self.handle_skills_request(&request, &context).await;
        }

        if request.method != MCP_RELAY_CANCELLATION_REQUEST_METHOD {
            return Err(ErrorData::new(
                rmcp::model::ErrorCode::METHOD_NOT_FOUND,
                request.method,
                None,
            ));
        }

        let params = request
            .params_as::<RelayCancellationNotification>()
            .map_err(|error| ErrorData::invalid_params(error.to_string(), None))?
            .ok_or_else(|| {
                ErrorData::invalid_params("Labby relay cancellation request omitted params", None)
            })?;
        let correlated = cancel_tracked_request_by_token(&params.token);
        Ok(CustomResult::new(serde_json::json!({
            "cancelled": correlated,
        })))
    }

    async fn discover(
        &self,
        context: RequestContext<RoleServer>,
    ) -> Result<DiscoverResult, ErrorData> {
        #[cfg(feature = "gateway")]
        {
            let client_info = context.client_info();
            let connected_client = connected_client_from_discovery(
                client_info,
                &context.extensions,
                self.transport_label,
                jiff::Timestamp::now().to_string(),
            );
            self.client_registry.push(connected_client).await;
        }

        Ok(DiscoverResult::from_server_info(
            self.supported_protocol_versions().into_owned(),
            self.get_info(),
        ))
    }

    fn accepted_subscription_filter(
        &self,
        requested: &SubscriptionFilter,
    ) -> Option<SubscriptionFilter> {
        let mut accepted = requested.clone();
        #[cfg(feature = "gateway")]
        {
            let deliverable = self
                .gateway_manager
                .as_ref()
                .and_then(|manager| manager.current_pool_sync())
                .map(|pool| pool.subscribable_resource_uris_snapshot())
                .unwrap_or_default();
            accepted.resource_subscriptions = requested
                .resource_subscriptions
                .as_ref()
                .map(|uris| {
                    uris.iter()
                        .filter(|uri| {
                            deliverable.contains(*uri)
                                && uri
                                    .strip_prefix("lab://upstream/")
                                    .and_then(|rest| rest.split('/').next())
                                    .is_none_or(|upstream| {
                                        self.route_scope.allows_upstream(upstream)
                                    })
                        })
                        .cloned()
                        .collect::<Vec<_>>()
                })
                .filter(|uris| !uris.is_empty());
        }
        #[cfg(not(feature = "gateway"))]
        {
            accepted.resource_subscriptions = None;
        }
        Some(accepted)
    }

    async fn listen(&self, context: SubscriptionContext) -> Result<(), ErrorData> {
        // Seed from the contract this session actually completed through
        // `tools/list`, never from a fresh live-state sample. Re-sampling here
        // loses the list(A) -> mutate(B) -> subscribe race by treating B as if
        // the client had already received it. `None` is intentionally stale:
        // a client that subscribed before finishing pagination is owed the next
        // relevant list-changed signal.
        let contract = self.peer_contract_for_request(context.request_context());
        let last_contract = if self.transport_label == "http" {
            // Each stateless HTTP request gets a fresh handler and MCP peer.
            // Neither the authenticated subject nor anonymous `None` proves
            // that this listen belongs to an earlier tools/list request. An
            // abandoned request could otherwise suppress a real catch-up for
            // a later conversation. Without transport-provided correlation,
            // fail conservative and make the subscription catch up.
            None
        } else {
            let subject_key = self
                .request_subject(context.request_context())
                .map(str::to_owned);
            self.last_listed_tool_contract
                .write()
                .await
                .claim_unambiguous(&subject_key)
        };
        let route_scope_label = self.route_scope.label();
        let pruned_peer_count = crate::mcp::peers::prune_closed_peers(&self.peers).await;
        let mut peers = self.peers.write().await;
        let registered = crate::mcp::peers::RegisteredPeer::from_subscription(
            context.sink().clone(),
            contract,
            last_contract,
        );
        let registration_id = registered.registration_id;
        peers.push(registered.clone());
        tracing::info!(
            surface = "mcp",
            service = "peers",
            action = "peer.connect",
            subsystem = "mcp_server",
            phase = "subscription.listen",
            peer_count = peers.len(),
            pruned_peer_count,
            route_scope = route_scope_label,
            "mcp notification subscription connected"
        );
        drop(peers);

        // rmcp acknowledges subscriptions before invoking this handler. Replay
        // any matching resource-update edge events journaled during that gap,
        // now that the peer is visible to normal live fanout.
        crate::mcp::catalog_notifications::catch_up_resource_updates(&self.peers, &registered)
            .await;
        crate::mcp::catalog_notifications::catch_up_tool_contract(&self.peers, registration_id)
            .await;

        context.cancelled().await;

        let mut peers = self.peers.write().await;
        peers.retain(|registered| registered.registration_id != registration_id);
        tracing::info!(
            surface = "mcp",
            service = "peers",
            action = "peer.disconnect",
            subsystem = "mcp_server",
            phase = "subscription.closed",
            peer_count = peers.len(),
            route_scope = route_scope_label,
            "mcp notification subscription disconnected"
        );
        Ok(())
    }

    fn complete(
        &self,
        mut request: CompleteRequestParams,
        context: RequestContext<RoleServer>,
    ) -> impl Future<Output = Result<CompleteResult, ErrorData>> + Send {
        Box::pin(async move {
            restore_request_meta(&mut request.meta, &context.meta);
            Ok(provenance::stamp_complete_result(
                self.complete_impl(request, context).await?,
            ))
        })
    }

    async fn list_prompts(
        &self,
        request: Option<PaginatedRequestParams>,
        context: RequestContext<RoleServer>,
    ) -> Result<ListPromptsResult, ErrorData> {
        Ok(provenance::stamp_list_prompts_result(
            self.list_prompts_impl(request, context).await?,
        ))
    }

    fn get_prompt(
        &self,
        mut request: GetPromptRequestParams,
        context: RequestContext<RoleServer>,
    ) -> impl Future<Output = Result<GetPromptResponse, ErrorData>> + Send {
        // Bound the SDK's shared request-dispatch frame, including discovery
        // requests which never execute this branch.
        Box::pin(async move {
            restore_request_meta(&mut request.meta, &context.meta);
            Ok(provenance::stamp_get_prompt_response(
                self.get_prompt_impl(request, context).await?,
            ))
        })
    }

    async fn list_resources(
        &self,
        request: Option<PaginatedRequestParams>,
        context: RequestContext<RoleServer>,
    ) -> Result<ListResourcesResult, ErrorData> {
        Ok(provenance::stamp_list_resources_result(
            self.list_resources_impl(request, context).await?,
        ))
    }

    async fn list_resource_templates(
        &self,
        request: Option<PaginatedRequestParams>,
        context: RequestContext<RoleServer>,
    ) -> Result<ListResourceTemplatesResult, ErrorData> {
        Ok(provenance::stamp_list_resource_templates_result(
            self.list_resource_templates_impl(request, context).await?,
        ))
    }

    fn read_resource(
        &self,
        mut request: ReadResourceRequestParams,
        context: RequestContext<RoleServer>,
    ) -> impl Future<Output = Result<ReadResourceResponse, ErrorData>> + Send {
        Box::pin(async move {
            restore_request_meta(&mut request.meta, &context.meta);
            let response = match self.read_resource_impl(request, context).await? {
                ReadResourceResponse::Complete(result) => result
                    .with_ttl_ms(0)
                    .with_cache_scope(CacheScope::Private)
                    .into(),
                incomplete => incomplete,
            };
            Ok(provenance::stamp_read_resource_response(response))
        })
    }

    async fn list_tools(
        &self,
        request: Option<PaginatedRequestParams>,
        context: RequestContext<RoleServer>,
    ) -> Result<ListToolsResult, ErrorData> {
        Ok(provenance::stamp_list_tools_result(
            self.list_tools_impl(request, context).await?,
        ))
    }

    async fn call_tool(
        &self,
        mut request: CallToolRequestParams,
        mut context: RequestContext<RoleServer>,
    ) -> Result<CallToolResponse, ErrorData> {
        let cancellation_guard = track_request_cancellation(&context, self.relay_session_id);
        context
            .extensions
            .insert(LabRequestCancellation(cancellation_guard.cancellation()));
        // rmcp keeps deserialized wire metadata in RequestContext.extensions/meta
        // and intentionally leaves the typed params field empty. Restore the
        // canonical metadata before handing the envelope to proxy routing.
        restore_request_meta(&mut request.meta, &context.meta);
        // Keep the full product-dispatch future off rmcp's transport worker
        // stack. In a multi-hop relay, nested Labby servers otherwise poll the
        // all-features dispatch state on Tokio's bounded worker stack and can
        // overflow it as new in-process services enlarge that state machine.
        Ok(provenance::stamp_call_tool_response(
            self.boxed_call_tool_response_impl(request, context).await?,
        ))
    }

    async fn get_task(
        &self,
        request: GetTaskParams,
        context: RequestContext<RoleServer>,
    ) -> Result<GetTaskResult, ErrorData> {
        #[cfg(feature = "gateway")]
        if let Some(pool) = self.current_upstream_pool().await {
            let result = pool
                .get_task_routed(
                    request,
                    self.request_subject(&context),
                    &self.route_scope.task_authorization(),
                    context.peer.clone(),
                )
                .await
                .map_err(|message| {
                    if message == "task not found" {
                        ErrorData::invalid_params(message, None)
                    } else {
                        ErrorData::internal_error(message, None)
                    }
                })?;
            return Ok(provenance::stamp_get_task_result(result));
        }
        Err(ErrorData::invalid_params("task not found", None))
    }

    async fn update_task(
        &self,
        request: UpdateTaskParams,
        context: RequestContext<RoleServer>,
    ) -> Result<(), ErrorData> {
        let gateway_task_id = request.task_id.clone();
        #[cfg(feature = "gateway")]
        if let Some(pool) = self.current_upstream_pool().await {
            return pool
                .update_task_routed(
                    request,
                    self.request_subject(&context),
                    &self.route_scope.task_authorization(),
                    &gateway_task_id,
                    context.peer.clone(),
                )
                .await
                .map_err(|message| {
                    if message == "task not found" {
                        ErrorData::invalid_params(message, None)
                    } else {
                        ErrorData::internal_error(message, None)
                    }
                });
        }
        Err(ErrorData::invalid_params("task not found", None))
    }

    async fn cancel_task(
        &self,
        request: CancelTaskParams,
        context: RequestContext<RoleServer>,
    ) -> Result<(), ErrorData> {
        let gateway_task_id = request.task_id.clone();
        #[cfg(feature = "gateway")]
        if let Some(pool) = self.current_upstream_pool().await {
            return pool
                .cancel_task_routed(
                    request,
                    self.request_subject(&context),
                    &self.route_scope.task_authorization(),
                    &gateway_task_id,
                    context.peer.clone(),
                )
                .await
                .map_err(|message| {
                    if message == "task not found" {
                        ErrorData::invalid_params(message, None)
                    } else {
                        ErrorData::internal_error(message, None)
                    }
                });
        }
        Err(ErrorData::invalid_params("task not found", None))
    }
}

use crate::mcp::catalog::CatalogChangeSet;

/// Extension capability map, for tests that assert what `initialize` declares.
#[cfg(test)]
pub(crate) fn mcp_extensions_for_test() -> ExtensionCapabilities {
    mcp_extensions()
}

impl LabMcpServer {
    /// Whether a missing per-request `AuthContext` may be read as trusted local
    /// stdio on *this* server's transport.
    ///
    /// The in-process peer is served over a duplex pipe with no HTTP layer, so
    /// it produces `None` for every caller — including a remote non-admin one
    /// arriving through Code Mode. This is an allow-list: any future transport
    /// fails closed until it explicitly proves that absent auth means local.
    pub(crate) fn absent_auth_trust(&self) -> crate::mcp::context::AbsentAuth {
        if TRANSPORTS_TRUSTING_ABSENT_AUTH.contains(&self.transport_label) {
            crate::mcp::context::AbsentAuth::TrustedLocal
        } else {
            crate::mcp::context::AbsentAuth::Untrusted
        }
    }
    /// `source` attributes the emission — see `labby_runtime::catalog_notify`.
    /// Per-call sites pass their own label so a notification triggered by a
    /// tool call is never confused with a gateway reconcile.
    pub(crate) async fn notify_catalog_changes(
        &self,
        changes: CatalogChangeSet,
        source: &'static str,
    ) {
        // Scheduled, not sent: this runs at the tail of a tool call, and the
        // caller's turn is still open. Delivering here would invalidate the
        // binding that call is using. See `catalog_coalesce`.
        crate::mcp::catalog_coalesce::schedule_catalog_notification(
            &self.peers,
            changes.into(),
            source,
        );
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    #[cfg(feature = "gateway")]
    use super::verify_upstream_subject_resolution_support;
    use super::{
        LabMcpServer, MCP_RELAY_CANCELLATION_REQUEST_METHOD, MCP_RELAY_CANCELLATION_TOKEN_META_KEY,
        ServerCapabilities, ServerInfo, cancel_tracked_request_by_token, request_cancellation_key,
        restore_request_meta, track_request_cancellation, withhold_legacy_unusable_capabilities,
    };
    use crate::mcp::catalog_notifications::{CatalogNotificationChanges, notify_catalog_peers};
    use crate::mcp::logging::logging_level_rank;
    use crate::registry::ToolRegistry;
    use rmcp::ServerHandler;
    use rmcp::ServiceExt;
    use rmcp::model::{
        CustomRequest, CustomResult, NumberOrString, ProtocolVersion, ServerNotification,
        SubscriptionFilter,
    };
    use rmcp::service::{ClientLifecycleMode, ClientServiceExt};

    fn stateless_test_server(peers: crate::mcp::peers::PeerRegistry) -> LabMcpServer {
        LabMcpServer {
            registry: std::sync::Arc::new(ToolRegistry::new()),
            access_runtime: std::sync::Arc::new(crate::access::AccessRuntime::blocked_unavailable()),
            file_stash_runtime: std::sync::Arc::new(crate::file_stash::FileStashRuntime::blocked()),
            #[cfg(feature = "gateway")]
            gateway_manager: None,
            peers,
            code_mode_app_state: Default::default(),
            last_listed_tool_contract: Default::default(),
            route_runtime: Default::default(),
            #[cfg(feature = "gateway")]
            client_registry: Default::default(),
            transport_label: "test",
            logging_level: std::sync::Arc::new(std::sync::atomic::AtomicU8::new(
                logging_level_rank(crate::mcp::logging::LoggingLevel::Info),
            )),
            route_scope: crate::mcp::route_scope::McpRouteScope::Root,
            relay_session_id: 0,
            code_mode_widget_callbacks_enabled_for_test: false,
        }
    }

    #[test]
    fn initialize_support_declares_every_adapted_protocol() {
        let server = stateless_test_server(Default::default());

        assert_eq!(
            server.supported_protocol_versions().as_ref(),
            ProtocolVersion::KNOWN_VERSIONS
        );
    }

    #[test]
    fn typed_request_meta_survives_empty_context_at_handler_boundary() {
        let mut typed = Some(rmcp::model::RequestMetaObject::new());
        typed
            .as_mut()
            .expect("typed metadata")
            .0
            .0
            .insert("typed.trace".to_string(), serde_json::json!("preserve-me"));
        let empty_context = rmcp::model::RequestMetaObject::new();

        restore_request_meta(&mut typed, &empty_context);

        assert_eq!(
            typed.as_ref().and_then(|meta| meta.0.0.get("typed.trace")),
            Some(&serde_json::json!("preserve-me"))
        );

        let mut context = rmcp::model::RequestMetaObject::new();
        context.0.0.insert(
            "context.trace".to_string(),
            serde_json::json!("replace-typed"),
        );
        restore_request_meta(&mut typed, &context);
        assert_eq!(typed.as_ref(), Some(&context));
    }

    #[tokio::test]
    async fn relay_cancellation_custom_request_cancels_tracked_token() {
        let server = stateless_test_server(Default::default());
        let (transport, _client_transport) = tokio::io::duplex(64);
        let running = rmcp::service::serve_directly::<rmcp::RoleServer, _, _, std::io::Error, _>(
            server, transport, None,
        );
        let mut tracked_context = rmcp::service::RequestContext::new(
            NumberOrString::String(std::sync::Arc::from("tracked-request")),
            running.peer().clone(),
        );
        tracked_context.meta.0.0.insert(
            MCP_RELAY_CANCELLATION_TOKEN_META_KEY.to_string(),
            serde_json::Value::String("handler-correlation-test".to_string()),
        );
        let tracked = track_request_cancellation(&tracked_context, 0);
        let cancellation = tracked.cancellation();

        let result = running
            .service()
            .on_custom_request(
                CustomRequest::new(
                    MCP_RELAY_CANCELLATION_REQUEST_METHOD,
                    Some(serde_json::json!({
                        "reason": "downstream request cancelled",
                        "token": "handler-correlation-test",
                    })),
                ),
                rmcp::service::RequestContext::new(
                    NumberOrString::String(std::sync::Arc::from("cancellation-request")),
                    running.peer().clone(),
                ),
            )
            .await
            .expect("relay cancellation request succeeds");
        let CustomResult(value) = result;

        assert_eq!(value["cancelled"], true);
        assert!(
            cancellation.token.is_cancelled(),
            "custom request must cancel the correlated request token"
        );
    }

    #[tokio::test]
    async fn tracked_cancellation_inherits_native_request_cancellation() {
        let server = stateless_test_server(Default::default());
        let (transport, _client_transport) = tokio::io::duplex(64);
        let running = rmcp::service::serve_directly::<rmcp::RoleServer, _, _, std::io::Error, _>(
            server, transport, None,
        );
        let context = rmcp::service::RequestContext::new(
            NumberOrString::String(std::sync::Arc::from("native-cancelled-request")),
            running.peer().clone(),
        );
        let native_cancellation = context.ct.clone();
        let tracked = track_request_cancellation(&context, 0);
        let cancellation = tracked.cancellation();

        native_cancellation.cancel();

        assert!(
            cancellation.token.is_cancelled(),
            "rmcp request cancellation and transport teardown must propagate to the tracked token"
        );
    }

    #[tokio::test]
    async fn numeric_and_string_request_ids_have_distinct_cancellation_keys() {
        let server = stateless_test_server(Default::default());
        let (transport, _client_transport) = tokio::io::duplex(64);
        let running = rmcp::service::serve_directly::<rmcp::RoleServer, _, _, std::io::Error, _>(
            server, transport, None,
        );
        let numeric =
            rmcp::service::RequestContext::new(NumberOrString::Number(1), running.peer().clone());
        let string = rmcp::service::RequestContext::new(
            NumberOrString::String(std::sync::Arc::from("1")),
            running.peer().clone(),
        );

        assert_ne!(
            request_cancellation_key(&numeric, 7),
            request_cancellation_key(&string, 7),
            "JSON-RPC numeric 1 and string \"1\" must not share a cancellation key"
        );
    }

    #[tokio::test]
    async fn relay_cancellation_custom_request_reports_untracked_token() {
        let server = stateless_test_server(Default::default());
        let (transport, _client_transport) = tokio::io::duplex(64);
        let running = rmcp::service::serve_directly::<rmcp::RoleServer, _, _, std::io::Error, _>(
            server, transport, None,
        );

        let result = running
            .service()
            .on_custom_request(
                CustomRequest::new(
                    MCP_RELAY_CANCELLATION_REQUEST_METHOD,
                    Some(serde_json::json!({
                        "reason": "downstream request cancelled",
                        "token": "untracked-handler-correlation-test",
                    })),
                ),
                rmcp::service::RequestContext::new(
                    NumberOrString::String(std::sync::Arc::from("untracked-cancellation-request")),
                    running.peer().clone(),
                ),
            )
            .await
            .expect("untracked relay cancellation request still receives an acknowledgement");
        let CustomResult(value) = result;

        assert_eq!(
            value["cancelled"], false,
            "side channel must not claim cancellation for an untracked handler path"
        );
    }

    #[tokio::test]
    async fn stale_cancellation_guard_cannot_remove_other_same_key_request() {
        let server = stateless_test_server(Default::default());
        let (transport, _client_transport) = tokio::io::duplex(64);
        let running = rmcp::service::serve_directly::<rmcp::RoleServer, _, _, std::io::Error, _>(
            server, transport, None,
        );
        let mut context = rmcp::service::RequestContext::new(
            NumberOrString::String(std::sync::Arc::from("duplicate-request")),
            running.peer().clone(),
        );
        context.meta.0.0.insert(
            MCP_RELAY_CANCELLATION_TOKEN_META_KEY.to_string(),
            serde_json::Value::String("duplicate-correlation-token".to_string()),
        );

        let stale_guard = track_request_cancellation(&context, 0);
        let stale_cancellation = stale_guard.cancellation();
        let current_guard = track_request_cancellation(&context, 0);
        let current_cancellation = current_guard.cancellation();
        drop(stale_guard);

        assert!(
            cancel_tracked_request_by_token("duplicate-correlation-token"),
            "dropping the stale guard must leave the newer request registered"
        );
        assert!(current_cancellation.token.is_cancelled());
        assert!(
            !stale_cancellation.token.is_cancelled(),
            "a completed request must be removed from the shared cancellation key"
        );
    }

    #[tokio::test]
    async fn same_key_live_requests_are_all_cancelled() {
        let server = stateless_test_server(Default::default());
        let (transport, _client_transport) = tokio::io::duplex(64);
        let running = rmcp::service::serve_directly::<rmcp::RoleServer, _, _, std::io::Error, _>(
            server, transport, None,
        );
        let mut context = rmcp::service::RequestContext::new(
            NumberOrString::String(std::sync::Arc::from("shared-request-id")),
            running.peer().clone(),
        );
        context.meta.0.0.insert(
            MCP_RELAY_CANCELLATION_TOKEN_META_KEY.to_string(),
            serde_json::Value::String("shared-live-correlation-token".to_string()),
        );

        let first_guard = track_request_cancellation(&context, 0);
        let first = first_guard.cancellation();
        let second_guard = track_request_cancellation(&context, 0);
        let second = second_guard.cancellation();

        assert!(cancel_tracked_request_by_token(
            "shared-live-correlation-token"
        ));
        assert!(
            first.token.is_cancelled() && second.token.is_cancelled(),
            "an ambiguous stateless fallback key must not silently orphan either live request"
        );
    }

    /// A legacy session can use neither subscription mechanism, so it must not
    /// be told `resources.subscribe` is available. Regression guard for the
    /// capability-honesty defect in issue #211.
    #[test]
    fn legacy_initialize_withholds_resource_subscribe_capability() {
        let capabilities = ServerCapabilities::builder()
            .enable_resources()
            .enable_resources_list_changed()
            .enable_resources_subscribe()
            .build();
        let mut info = ServerInfo::new(capabilities);
        assert_eq!(
            info.capabilities
                .resources
                .as_ref()
                .and_then(|c| c.subscribe),
            Some(true),
            "fixture must start with the capability advertised, or this test proves nothing"
        );

        withhold_legacy_unusable_capabilities(&mut info);

        assert_eq!(
            info.capabilities
                .resources
                .as_ref()
                .and_then(|c| c.subscribe),
            None,
            "a legacy session reaches neither resources/subscribe (no handler) nor \
             subscriptions/listen (rmcp gates it to modern sessions), so advertising \
             the capability to it promises something that cannot work"
        );
        assert_eq!(
            info.capabilities
                .resources
                .as_ref()
                .and_then(|c| c.list_changed),
            Some(true),
            "withholding subscribe must not disturb the other resource capabilities"
        );
    }

    /// The withholding must not depend on `resources` being present — under
    /// `--no-default-features` the gateway-conditional branch never runs.
    #[test]
    fn withholding_is_safe_when_no_resource_capability_is_advertised() {
        let mut info = ServerInfo::new(ServerCapabilities::builder().enable_tools().build());
        withhold_legacy_unusable_capabilities(&mut info);
        assert!(info.capabilities.resources.is_none());
        assert!(
            info.capabilities.tools.is_some(),
            "unrelated capabilities must survive"
        );
    }

    /// `get_info` — the path `discover` returns to modern clients — must keep
    /// the flag. rmcp intersects a requested `SubscriptionFilter` against it
    /// (`SubscriptionFilter::supported_by`), so clearing it globally instead of
    /// per-session would silence modern subscriptions and the gateway's own
    /// upstream negotiation.
    #[test]
    fn get_info_does_not_withhold_capabilities_from_modern_sessions() {
        let server = stateless_test_server(Default::default());
        let info = server.get_info();

        // This fixture has no gateway manager, so `subscribe` is not advertised
        // in the first place. What this locks in is that `get_info` performs no
        // withholding of its own: the resource capability it does advertise is
        // unmodified, and withholding happens only on the legacy path.
        assert_eq!(
            info.capabilities
                .resources
                .as_ref()
                .and_then(|c| c.list_changed),
            Some(true),
            "get_info must return capabilities untouched by legacy withholding"
        );
    }

    #[test]
    fn server_capabilities_advertise_list_changed_support() {
        let server = stateless_test_server(Default::default());

        let info = server.get_info();
        assert_eq!(info.server_info.name, "labby");
        assert_eq!(info.server_info.version, env!("CARGO_PKG_VERSION"));
        assert_eq!(
            info.capabilities.tools.and_then(|c| c.list_changed),
            Some(true)
        );
        assert_eq!(
            info.capabilities.resources.and_then(|c| c.list_changed),
            Some(true)
        );
        assert_eq!(
            info.capabilities.prompts.and_then(|c| c.list_changed),
            Some(true)
        );
        assert!(
            info.capabilities.logging.is_none(),
            "2026-07-28 removes logging/setLevel and must not advertise legacy logging"
        );
        assert!(
            info.capabilities.completions.is_some(),
            "RMCP completion capability must be advertised"
        );
        if let Some(extensions) = info.capabilities.extensions.as_ref() {
            for invented_auth_extension in [
                "io.modelcontextprotocol/enterprise-managed-authorization",
                "io.modelcontextprotocol/oauth-client-credentials",
                "io.modelcontextprotocol/client-id-metadata-document",
            ] {
                assert!(
                    !extensions.contains_key(invented_auth_extension),
                    "OAuth extensions are discovered through authorization metadata, not MCP initialize capabilities"
                );
            }
        }

        #[cfg(feature = "gateway")]
        {
            // MCP Apps UI extension (SEP-1724) must be advertised so hosts render
            // the Code Mode inspector widgets.
            let extensions = info
                .capabilities
                .extensions
                .expect("MCP Apps UI extension capability must be advertised");
            let ui_ext = extensions
                .get("io.modelcontextprotocol/ui")
                .expect("io.modelcontextprotocol/ui extension must be present");
            assert_eq!(
                ui_ext.get("mimeTypes"),
                Some(&serde_json::json!(["text/html;profile=mcp-app"])),
                "UI extension must advertise the mcp-app widget MIME type"
            );
        }
        #[cfg(not(feature = "gateway"))]
        assert!(
            info.capabilities.extensions.is_none(),
            "no-gateway builds must not advertise MCP Apps UI"
        );
    }

    #[tokio::test]
    async fn resource_templates_include_required_rc_cache_metadata() {
        let server = stateless_test_server(Default::default());
        let (transport, _client_transport) = tokio::io::duplex(64);
        let running = rmcp::service::serve_directly::<rmcp::RoleServer, _, _, std::io::Error, _>(
            server, transport, None,
        );
        let context =
            rmcp::service::RequestContext::new(NumberOrString::Number(1), running.peer().clone());

        let result = running
            .service()
            .list_resource_templates(None, context)
            .await
            .expect("resource templates");

        assert_eq!(result.ttl_ms, Some(0));
        assert_eq!(result.cache_scope, Some(rmcp::model::CacheScope::Private));
        let wire = serde_json::to_value(result).expect("serialize resource templates");
        assert_eq!(wire["resultType"], "complete");
        assert_eq!(wire["ttlMs"], 0);
        assert_eq!(wire["cacheScope"], "private");
    }

    #[cfg(feature = "gateway")]
    #[test]
    fn upstream_subject_resolution_self_test_passes_for_plan_a() {
        verify_upstream_subject_resolution_support().expect("self-test");
    }

    #[cfg(feature = "gateway")]
    mod connected_client_from_discovery_tests {
        use axum::http;
        use rmcp::model::Implementation;

        use super::super::connected_client_from_discovery;

        // Same `Extensions` fabrication as `verify_upstream_subject_resolution_support`
        // above — an `http::request::Parts` carrying an `AuthContext`, wrapped in
        // `rmcp::model::Extensions`.
        fn extensions_with_subject(subject: &str) -> rmcp::model::Extensions {
            let (mut parts, _) = http::Request::new(()).into_parts();
            parts
                .extensions
                .insert(labby_auth::auth_context::AuthContext {
                    sub: subject.to_string(),
                    actor_key: None,
                    scopes: Vec::new(),
                    issuer: "https://lab.example.com".to_string(),
                    via_session: false,
                    csrf_token: None,
                    email: None,
                });
            let mut extensions = rmcp::model::Extensions::new();
            extensions.insert(parts);
            extensions
        }

        #[test]
        fn never_stores_the_raw_authenticated_subject() {
            let extensions = extensions_with_subject("jacob@example.com");
            let client = connected_client_from_discovery(
                Some(Implementation::new("claude-code", "2.4.1")),
                &extensions,
                "stdio",
                "2026-01-01T00:00:00Z".to_string(),
            );

            let tag = client.subject_tag.expect("subject_tag must be set");
            assert_ne!(tag, "jacob@example.com", "raw subject must never be stored");
            assert!(
                tag.starts_with("sub:"),
                "expected a redacted `sub:` tag, got {tag:?}"
            );
        }

        #[test]
        fn redaction_is_deterministic_for_the_same_subject() {
            let a = connected_client_from_discovery(
                None,
                &extensions_with_subject("same-subject"),
                "http",
                "2026-01-01T00:00:00Z".to_string(),
            );
            let b = connected_client_from_discovery(
                None,
                &extensions_with_subject("same-subject"),
                "http",
                "2026-01-01T00:00:00Z".to_string(),
            );

            assert_eq!(a.subject_tag, b.subject_tag);
        }

        #[test]
        fn distinct_subjects_redact_to_distinct_tags() {
            let a = connected_client_from_discovery(
                None,
                &extensions_with_subject("alice"),
                "http",
                "2026-01-01T00:00:00Z".to_string(),
            );
            let b = connected_client_from_discovery(
                None,
                &extensions_with_subject("bob"),
                "http",
                "2026-01-01T00:00:00Z".to_string(),
            );

            assert_ne!(a.subject_tag, b.subject_tag);
        }

        #[test]
        fn no_auth_context_yields_no_subject_tag() {
            let extensions = rmcp::model::Extensions::new();
            let client = connected_client_from_discovery(
                None,
                &extensions,
                "in-process",
                "2026-01-01T00:00:00Z".to_string(),
            );

            assert_eq!(client.subject_tag, None);
        }

        #[test]
        fn client_info_and_transport_pass_through_unmodified() {
            let extensions = rmcp::model::Extensions::new();
            let client = connected_client_from_discovery(
                Some(Implementation::new("codex-cli", "0.9.2")),
                &extensions,
                "stdio",
                "2026-01-01T00:00:00Z".to_string(),
            );

            assert_eq!(client.client_name.as_deref(), Some("codex-cli"));
            assert_eq!(client.client_version.as_deref(), Some("0.9.2"));
            assert_eq!(client.transport, "stdio");
            assert_eq!(client.connected_at, "2026-01-01T00:00:00Z");
        }
    }

    #[tokio::test]
    async fn stateless_subscription_receives_catalog_notifications() {
        let peers = Default::default();
        let server = stateless_test_server(std::sync::Arc::clone(&peers));
        let (server_transport, client_transport) = tokio::io::duplex(64 * 1024);
        let server_handle = tokio::spawn(async move {
            let running = server.serve(server_transport).await.expect("server starts");
            running.waiting().await
        });
        let client_service = ()
            .serve_with_lifecycle(
                client_transport,
                ClientLifecycleMode::Discover {
                    preferred_versions: vec![ProtocolVersion::V_2026_07_28],
                },
            )
            .await
            .expect("stateless client discovers server");

        let mut subscription = client_service
            .peer()
            .listen(
                SubscriptionFilter::builder()
                    .resources_list_changed()
                    .build(),
            )
            .await
            .expect("subscription is acknowledged");
        assert_eq!(peers.read().await.len(), 1);

        notify_catalog_peers(
            &peers,
            CatalogNotificationChanges::new(false, true, false),
            labby_runtime::catalog_notify::SOURCE_MCP_CALL_UPSTREAM,
        )
        .await;
        let notification = tokio::time::timeout(Duration::from_secs(5), subscription.next())
            .await
            .expect("catalog notification timed out")
            .expect("subscription remains healthy")
            .expect("catalog notification exists");
        assert!(matches!(
            notification,
            ServerNotification::ResourceListChangedNotification(_)
        ));

        subscription.cancel().await.expect("subscription cancels");
        tokio::time::timeout(Duration::from_secs(5), async {
            while !peers.read().await.is_empty() {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("cancelled subscription is removed");
        client_service.cancel().await.expect("client cancels");
        server_handle.abort();
    }

    #[test]
    fn concurrent_same_subject_conversations_cannot_claim_each_others_baseline() {
        let subject = Some("same-subject".to_string());
        let mut store = super::ToolContractBaselineStore::default();
        for conversation in ["conversation-a", "conversation-b"] {
            store.publish(
                subject.clone(),
                crate::mcp::catalog::ToolCatalogSnapshot::from_names(
                    std::iter::once(conversation.to_string()).collect(),
                ),
            );
        }

        assert_eq!(store.candidate_count(&subject), 2);
        assert!(
            store.claim_unambiguous(&subject).is_none(),
            "same-subject concurrent conversations must relist conservatively"
        );
        assert_eq!(store.candidate_count(&subject), 0);
    }

    #[test]
    fn baseline_store_evicts_oldest_subject_and_claims_single_candidate() {
        let mut store = super::ToolContractBaselineStore::default();
        let crowded = Some("crowded-subject".to_string());
        for index in 0..(super::MAX_TOOL_CONTRACTS_PER_SUBJECT + 3) {
            store.publish(
                crowded.clone(),
                crate::mcp::catalog::ToolCatalogSnapshot::from_names(
                    std::iter::once(format!("crowded-tool-{index}")).collect(),
                ),
            );
        }
        assert_eq!(
            store.candidate_count(&crowded),
            super::MAX_TOOL_CONTRACTS_PER_SUBJECT
        );
        for index in 0..=super::MAX_TOOL_CONTRACT_SUBJECTS {
            store.publish(
                Some(format!("subject-{index}")),
                crate::mcp::catalog::ToolCatalogSnapshot::from_names(
                    std::iter::once(format!("tool-{index}")).collect(),
                ),
            );
        }

        assert_eq!(store.subjects.len(), super::MAX_TOOL_CONTRACT_SUBJECTS);
        assert_eq!(store.candidate_count(&Some("subject-0".to_string())), 0);
        let newest = Some(format!("subject-{}", super::MAX_TOOL_CONTRACT_SUBJECTS));
        assert!(store.claim_unambiguous(&newest).is_some());
        assert_eq!(store.candidate_count(&newest), 0);
    }

    #[tokio::test]
    async fn subscription_catches_up_when_change_flushed_before_registration() {
        let peers = Default::default();
        let server = stateless_test_server(std::sync::Arc::clone(&peers));
        server.last_listed_tool_contract.write().await.publish(
            None,
            crate::mcp::catalog::ToolCatalogSnapshot::from_names(
                std::iter::once("tool-from-listed-contract-a".to_string()).collect(),
            ),
        );
        let (server_transport, client_transport) = tokio::io::duplex(64 * 1024);
        let server_handle = tokio::spawn(async move {
            let running = server.serve(server_transport).await.expect("server starts");
            running.waiting().await
        });
        let client_service = ()
            .serve_with_lifecycle(
                client_transport,
                ClientLifecycleMode::Discover {
                    preferred_versions: vec![ProtocolVersion::V_2026_07_28],
                },
            )
            .await
            .expect("stateless client discovers server");
        let mut subscription = client_service
            .peer()
            .listen(SubscriptionFilter::builder().tools_list_changed().build())
            .await
            .expect("subscription is acknowledged");

        let notification = tokio::time::timeout(Duration::from_secs(5), subscription.next())
            .await
            .expect("catch-up notification timed out")
            .expect("subscription remains healthy")
            .expect("catch-up notification exists");
        assert!(matches!(
            notification,
            ServerNotification::ToolListChangedNotification(_)
        ));

        subscription.cancel().await.expect("subscription cancels");
        client_service.cancel().await.expect("client cancels");
        server_handle.abort();
    }
}
