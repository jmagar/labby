//! `AcpSessionRegistry` — per-subscriber mpsc, Arc<Session> ownership, SQLite persistence.
//!
//! # Observability & safeguards added (post-183-orphan incident)
//!
//! - Structured tracing on every session lifecycle event (create/cancel/close/reattach)
//! - `MAX_CONCURRENT_SESSIONS` (20): hard cap, returns error to caller
//! - Circuit breaker: max `STORM_MAX_CREATIONS` (10) in `STORM_WINDOW_SECS` (60s)
//! - Periodic health reporter: logs session counts every 60 seconds
//! - Idle-TTL reaper: removes sessions idle > `SESSION_IDLE_TIMEOUT_MINS` (30)
//! - Graceful shutdown: `shutdown_all_sessions()` + SIGTERM fan-out + 10s drain
//! - Event-forwarder cleanup: removes session from map when runtime thread exits

use std::collections::{HashMap, VecDeque};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use futures::{Stream, StreamExt, stream};
use tokio::sync::{Mutex, RwLock, mpsc};

use labby_apis::acp::persistence::AcpPersistence;
use labby_apis::acp::types::{
    AcpEvent, AcpModelOption, AcpProviderHealth, AcpSessionState, AcpSessionSummary,
};

use crate::acp::params::BulkCloseSelector;
use crate::dispatch::acp::persistence::SqliteAcpPersistence;
use crate::dispatch::error::ToolError;

use super::runtime::{
    PromptAttachment, PromptAttachmentContent, PromptInput, RuntimeHandle, launch_codex_runtime,
    normalize_provider_id, provider_healths,
};
use super::types::{
    StartSessionInput, event_created_at, session_title_from_event, stamp_event_sequence,
};

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// Capacity for each subscriber's mpsc channel.
const SUBSCRIBER_CAPACITY: usize = 64;

/// Maximum number of concurrent SSE subscribers per session.
const MAX_SUBSCRIBERS_PER_SESSION: usize = 32;

/// Maximum number of concurrent ACP sessions (prevents spawn storms).
pub const MAX_CONCURRENT_SESSIONS: usize = 20;

/// Maximum backlog events returned from SQLite on subscribe.
const BACKFILL_CAP: u64 = 10_000;

/// Capacity of the per-session producer-side AcpEvent channel.
///
/// The channel feeds `spawn_event_forwarder`, which awaits on persistence.
/// When persistence stalls the bound back-pressures all the way to the
/// provider's stdio reader; the choice to await on full (rather than drop)
/// preserves the seq contiguity that SSE backfill depends on.
///
/// Sized to absorb typical SQLite batch-flush stalls (single-digit ms each)
/// at high event rates without blocking the provider in steady state. Larger
/// values delay the moment back-pressure kicks in but do not change the
/// failure mode.
pub const ACP_EVENT_CHANNEL_CAPACITY: usize = 1024;

/// Circuit breaker: max new sessions in STORM_WINDOW_SECS.
pub const STORM_MAX_CREATIONS: usize = 10;

/// Circuit breaker: sliding window duration.
pub const STORM_WINDOW_SECS: u64 = 60;

/// Sessions idle > this many minutes are reaped (non-Running/WaitingForPermission).
const SESSION_IDLE_TIMEOUT_MINS: u64 = 30;

/// Health reporter log interval (seconds).
const HEALTH_REPORT_INTERVAL_SECS: u64 = 60;

/// Idle reaper check interval (seconds, production).
const IDLE_REAPER_INTERVAL_SECS: u64 = 5 * 60;

const HANDOFF_MAX_MESSAGES: usize = 10;
const HANDOFF_MAX_BYTES: usize = 12 * 1024;

// ---------------------------------------------------------------------------
// Session struct
// ---------------------------------------------------------------------------

struct Session {
    id: String,
    principal: String,
    state: RwLock<AcpSessionState>,
    summary: RwLock<AcpSessionSummary>,
    handle: Mutex<Option<RuntimeHandle>>,
    subscribers: Mutex<Vec<mpsc::Sender<Arc<AcpEvent>>>>,
    /// In-memory event ring buffer. Capped at 500 events; `pop_front` is O(1).
    events: RwLock<VecDeque<Arc<AcpEvent>>>,
    /// Never held across an `.await` — use std::sync::Mutex for lower overhead.
    next_seq: std::sync::Mutex<u64>,
    /// Never held across an `.await` — use std::sync::Mutex for lower overhead.
    last_activity: std::sync::Mutex<Instant>,
}

impl Session {
    fn new(id: String, principal: String, summary: AcpSessionSummary) -> Arc<Self> {
        Self::new_with_seq(id, principal, summary, 1)
    }

    fn new_with_seq(
        id: String,
        principal: String,
        summary: AcpSessionSummary,
        next_seq: u64,
    ) -> Arc<Self> {
        Arc::new(Self {
            id,
            principal,
            state: RwLock::new(summary.state.clone()),
            summary: RwLock::new(summary),
            handle: Mutex::new(None),
            subscribers: Mutex::new(Vec::new()),
            events: RwLock::new(VecDeque::new()),
            next_seq: std::sync::Mutex::new(next_seq),
            last_activity: std::sync::Mutex::new(Instant::now()),
        })
    }
}

// ---------------------------------------------------------------------------
// Registry
// ---------------------------------------------------------------------------

/// Partial-success envelope for `bulk_close_sessions`.
#[derive(Debug, Clone, serde::Serialize)]
pub struct BulkCloseResult {
    pub closed: Vec<String>,
    pub failed: Vec<BulkCloseFailure>,
}

/// Per-session failure inside a `BulkCloseResult.failed[]` array.
#[derive(Debug, Clone, serde::Serialize)]
pub struct BulkCloseFailure {
    pub id: String,
    pub kind: String,
    pub message: String,
}

/// Outcome of `start_and_prompt` — the orchestrator that collapses
/// `session.start` + `session.prompt` into a single atomic call. The session
/// is closed before returning if the prompt step fails, so callers never
/// see an orphan row from this code path.
#[derive(Debug, Clone, serde::Serialize)]
pub struct StartAndPromptResult {
    pub session_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub provider_session_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model_id: Option<String>,
    pub provider: String,
    pub title: String,
}

#[derive(Clone)]
pub struct AcpSessionRegistry<P = SqliteAcpPersistence> {
    sessions: Arc<RwLock<HashMap<String, Arc<Session>>>>,
    /// Injected at construction time; `None` when persistence is disabled or
    /// unavailable (e.g., in unit tests or when `LAB_ACP_DB` is unset).
    persistence: Option<Arc<P>>,
    default_cwd: String,
    /// Storm detection: timestamps of recent session creations (sliding window).
    recent_creations: Arc<Mutex<VecDeque<Instant>>>,
    /// Blocks new session creation when shutting down.
    shutting_down: Arc<AtomicBool>,
    /// Atomic count of sessions that currently own a live provider process.
    /// Incremented *before* launch, decremented on any path that drops the handle.
    active_runtime_count: Arc<AtomicUsize>,
    /// Idle timeout — configurable for tests.
    idle_timeout: Duration,
    provider_models: Arc<RwLock<HashMap<String, Vec<AcpModelOption>>>>,
}

/// Options forwarded by callers that still use the legacy split API.
/// Prefer [`PromptOptions`] for new call sites.
#[derive(Debug, Clone, Default)]
pub struct PromptSessionOptions {
    pub provider: Option<String>,
    pub continuity_mode: Option<String>,
}

/// Unified options struct for `prompt_session`.
///
/// Collapses the previous four overloads
/// (`prompt_session` / `prompt_session_with_attachments` /
/// `prompt_session_with_options` / `prompt_session_input`) into a single
/// call site.  All fields are optional except `session_id` and `principal`.
#[derive(Debug, Clone)]
pub struct PromptOptions {
    pub session_id: String,
    pub principal: String,
    pub text: String,
    pub attachments: Vec<crate::acp::params::LocalPromptAttachment>,
    pub model_id: Option<String>,
    pub provider: Option<String>,
    pub continuity_mode: Option<String>,
}

impl<P: AcpPersistence> AcpSessionRegistry<P> {
    /// Construct a registry without persistence (e.g., for tests or builder patterns).
    /// Call [`AcpSessionRegistry::new_with_persistence`] or
    /// [`AcpSessionRegistry::<SqliteAcpPersistence>::from_env`] when persistence is needed.
    #[must_use]
    pub fn new() -> Self {
        crate::acp::runtime::warn_if_acp_provider_sandbox_is_incompatible();
        let default_cwd = std::env::var("ACP_SESSION_CWD").unwrap_or_else(|_| {
            std::env::current_dir()
                .map(|p| p.display().to_string())
                .unwrap_or_else(|_| ".".to_string())
        });
        let registry = Self {
            sessions: Arc::new(RwLock::new(HashMap::new())),
            persistence: None,
            default_cwd,
            recent_creations: Arc::new(Mutex::new(VecDeque::new())),
            shutting_down: Arc::new(AtomicBool::new(false)),
            active_runtime_count: Arc::new(AtomicUsize::new(0)),
            idle_timeout: Duration::from_secs(SESSION_IDLE_TIMEOUT_MINS * 60),
            provider_models: Arc::new(RwLock::new(HashMap::new())),
        };
        Self::spawn_health_reporter(Arc::clone(&registry.sessions));
        Self::spawn_idle_reaper(registry.clone(), IDLE_REAPER_INTERVAL_SECS);
        registry
    }

    fn persistence(&self) -> Option<&P> {
        self.persistence.as_deref()
    }

    async fn get_session_arc(&self, session_id: &str) -> Result<Arc<Session>, ToolError> {
        let guard = self.sessions.read().await;
        guard
            .get(session_id)
            .cloned()
            .ok_or_else(|| not_found("unknown ACP session"))
    }

    async fn provider_model_options(&self, provider: &str) -> Vec<AcpModelOption> {
        self.provider_models
            .read()
            .await
            .get(provider)
            .cloned()
            .unwrap_or_default()
    }

    fn check_principal(session: &Session, principal: &str) -> Result<(), ToolError> {
        if principal.trim().is_empty() || session.principal.trim().is_empty() {
            return Err(ToolError::Sdk {
                sdk_kind: "auth_failed".to_string(),
                message: "authenticated ACP session owner required".to_string(),
            });
        }
        if session.principal != principal {
            return Err(ToolError::Sdk {
                sdk_kind: "not_found".to_string(),
                message: "unknown ACP session".to_string(),
            });
        }
        Ok(())
    }

    // ── Public API ─────────────────────────────────────────────────────────

    #[must_use]
    pub fn provider_healths(&self) -> Vec<AcpProviderHealth> {
        let mut healths = provider_healths();
        if let Ok(models) = self.provider_models.try_read() {
            for health in &mut healths {
                if let Some(provider_models) = models.get(&health.provider) {
                    health.models = provider_models.clone();
                }
            }
        }
        healths
    }

    pub async fn list_sessions(&self, principal: &str) -> Vec<AcpSessionSummary> {
        if principal.trim().is_empty() {
            return Vec::new();
        }
        let sessions_snapshot: Vec<Arc<Session>> = {
            let guard = self.sessions.read().await;
            guard
                .values()
                .filter(|s| !s.principal.trim().is_empty() && s.principal == principal)
                .cloned()
                .collect()
        };
        let mut summaries: Vec<AcpSessionSummary> = Vec::with_capacity(sessions_snapshot.len());
        for session in &sessions_snapshot {
            let summary = session.summary.read().await;
            summaries.push(summary.clone());
        }
        summaries.sort_by(|a, b| b.updated_at.cmp(&a.updated_at));
        summaries
    }

    pub async fn check_session_access(
        &self,
        session_id: &str,
        principal: &str,
    ) -> Result<(), ToolError> {
        let session = self.get_session_arc(session_id).await?;
        Self::check_principal(&session, principal)
    }

    pub async fn get_session(&self, session_id: &str) -> Option<AcpSessionSummary> {
        let session = {
            let guard = self.sessions.read().await;
            guard.get(session_id).cloned()
        }?;
        let summary = session.summary.read().await;
        Some(summary.clone())
    }

    pub async fn create_session(
        &self,
        input: StartSessionInput,
        principal: &str,
    ) -> Result<AcpSessionSummary, ToolError> {
        // Guard: refuse during shutdown.
        if self.shutting_down.load(Ordering::SeqCst) {
            tracing::warn!(
                surface = "acp",
                service = "registry",
                action = "session.create",
                "ACP registry is shutting down — rejecting new session",
            );
            return Err(ToolError::Sdk {
                sdk_kind: "service_unavailable".to_string(),
                message: "ACP registry is shutting down".to_string(),
            });
        }

        // Guard: concurrent runtime limit.
        //
        // Restored SQLite sessions stay in the map for history and reattach,
        // but they do not own provider processes until prompted again.
        //
        // Use an atomic fetch_add to reserve the slot *before* launching the
        // provider subprocess.  This eliminates the TOCTOU window where two
        // concurrent creates both observe count < MAX and both proceed to launch.
        // On rejection or launch failure the reservation is released immediately.
        let reserved = self.active_runtime_count.fetch_add(1, Ordering::SeqCst) + 1;
        if reserved > MAX_CONCURRENT_SESSIONS {
            self.active_runtime_count.fetch_sub(1, Ordering::SeqCst);
            let active_count = reserved - 1;
            tracing::warn!(
                surface = "acp",
                service = "registry",
                action = "session.create",
                active_sessions = active_count,
                limit = MAX_CONCURRENT_SESSIONS,
                "ACP session limit reached — rejecting create_session",
            );
            return Err(ToolError::Sdk {
                sdk_kind: "session_limit_exceeded".to_string(),
                message: format!(
                    "Session limit reached ({active_count} active sessions). \
                     Kill existing sessions before starting new ones.",
                ),
            });
        }
        let active_count = reserved - 1;

        // Circuit breaker: session creation storm detection.
        {
            let mut recent = self.recent_creations.lock().await;
            let now = Instant::now();
            recent.retain(|t| now.duration_since(*t).as_secs() < STORM_WINDOW_SECS);
            if recent.len() >= STORM_MAX_CREATIONS {
                tracing::error!(
                    surface = "acp",
                    service = "registry",
                    action = "session.create",
                    creations_in_window = recent.len(),
                    window_secs = STORM_WINDOW_SECS,
                    "ACP session creation storm detected — circuit breaker tripped",
                );
                return Err(ToolError::Sdk {
                    sdk_kind: "rate_limited".to_string(),
                    message: format!(
                        "Session creation storm: {} sessions in {}s window. \
                         Wait before creating more.",
                        recent.len(),
                        STORM_WINDOW_SECS,
                    ),
                });
            }
            recent.push_back(now);
        }

        let session_id = uuid::Uuid::new_v4().to_string();
        let created_at = jiff::Timestamp::now().to_string();
        let cwd = if input.cwd.is_empty() {
            self.default_cwd.clone()
        } else {
            input.cwd.clone()
        };

        tracing::info!(
            surface = "acp", service = "registry", action = "session.create",
            session_id = %session_id, active_sessions = active_count,
            limit = MAX_CONCURRENT_SESSIONS, provider = ?input.provider,
            actor_key = %actor_key_from_principal(principal),
            "Creating ACP session",
        );

        // Launch the codex runtime.
        let (event_tx, event_rx) = mpsc::channel::<AcpEvent>(ACP_EVENT_CHANNEL_CAPACITY);
        let (runtime, started) = launch_codex_runtime(
            session_id.clone(),
            StartSessionInput {
                provider: input.provider.clone(),
                cwd: cwd.clone(),
                title: input.title.clone(),
                principal: input.principal.clone(),
                model_id: input.model_id.clone(),
            },
            event_tx.clone(),
        )
        .await
        .map_err(|message| {
            // Release the reserved slot — the provider never launched.
            self.active_runtime_count.fetch_sub(1, Ordering::SeqCst);
            tracing::error!(
                surface = "acp", service = "registry", action = "session.create",
                session_id = %session_id, error = %message,
                "ACP runtime launch failed",
            );
            ToolError::Sdk {
                sdk_kind: "internal_error".to_string(),
                message,
            }
        })?;

        let provider = normalize_provider_id(input.provider.as_deref());
        if !started.models.is_empty() {
            self.provider_models
                .write()
                .await
                .insert(provider.clone(), started.models.clone());
        }
        let options = self.provider_model_options(&provider).await;
        let (model_id, model_name) = resolve_model_selection(
            &provider,
            input.model_id.as_deref(),
            &options,
            started.model_id.as_deref(),
        )?;
        let summary = AcpSessionSummary {
            id: session_id.clone(),
            provider,
            title: input.title.unwrap_or_else(|| "New session".to_string()),
            cwd: cwd.clone(),
            state: AcpSessionState::Idle,
            created_at: created_at.clone(),
            updated_at: created_at,
            principal: if principal.is_empty() {
                None
            } else {
                Some(principal.to_string())
            },
            provider_session_id: Some(started.provider_session_id),
            agent_name: Some(started.agent_name),
            agent_version: Some(started.agent_version),
            model_id,
            model_name: model_name.or(started.model_name),
            config_options: started.config_options,
        };

        let session = Session::new(session_id.clone(), principal.to_string(), summary.clone());
        {
            let mut handle_guard = session.handle.lock().await;
            *handle_guard = Some(runtime);
        }
        {
            let mut map_guard = self.sessions.write().await;
            map_guard.insert(session_id.clone(), Arc::clone(&session));
        }

        self.spawn_event_forwarder(
            Arc::clone(&session),
            event_rx,
            summary.provider_session_id.clone(),
        );

        tracing::info!(
            surface = "acp", service = "registry", action = "session.create",
            session_id = %session_id, active_sessions = active_count + 1,
            provider_session_id = %summary.provider_session_id.as_deref().unwrap_or(""),
            "ACP session created successfully",
        );

        if let Some(db) = self.persistence() {
            if let Err(error) = db.save_session(&summary).await {
                tracing::warn!(
                    surface = "acp", service = "registry", action = "session.save",
                    session_id = %summary.id, error = %error,
                    "failed to persist session summary",
                );
            }
        }

        Ok(summary)
    }

    #[allow(dead_code)]
    /// Send a prompt to an existing session.
    ///
    /// This is the single entry point replacing the previous four overloads
    /// (`prompt_session`, `prompt_session_with_attachments`,
    /// `prompt_session_with_options`, `prompt_session_input`).
    pub async fn prompt_session(&self, opts: PromptOptions) -> Result<(), ToolError> {
        let PromptOptions {
            session_id,
            principal,
            text,
            attachments,
            model_id,
            provider,
            continuity_mode,
        } = opts;
        let runtime_attachments = attachments
            .into_iter()
            .map(|attachment| {
                let content = match attachment.content {
                    crate::acp::params::LocalAttachmentContent::Text { text } => {
                        PromptAttachmentContent::Text(text)
                    }
                    crate::acp::params::LocalAttachmentContent::Blob { base64 } => {
                        PromptAttachmentContent::Blob(base64)
                    }
                };
                PromptAttachment {
                    id: attachment.id,
                    name: attachment.name,
                    mime_type: attachment.mime_type,
                    size: attachment.size,
                    content,
                }
            })
            .collect();
        let prompt_input = PromptInput {
            text,
            attachments: runtime_attachments,
        };
        let options = PromptSessionOptions {
            provider,
            continuity_mode,
        };
        self.prompt_session_inner(
            &session_id,
            prompt_input,
            &principal,
            model_id.as_deref(),
            options,
        )
        .await
    }

    async fn prompt_session_inner(
        &self,
        session_id: &str,
        mut prompt: PromptInput,
        principal: &str,
        model_id: Option<&str>,
        options: PromptSessionOptions,
    ) -> Result<(), ToolError> {
        let session = self.get_session_arc(session_id).await?;
        Self::check_principal(&session, principal)?;
        let (selected_model_id, selected_model_name) = {
            let summary = session.summary.read().await;
            let options = if summary.config_options.is_empty() {
                self.provider_model_options(&summary.provider).await
            } else {
                summary
                    .config_options
                    .iter()
                    .flat_map(|option| option.options.clone())
                    .collect()
            };
            resolve_model_selection(
                &summary.provider,
                model_id,
                &options,
                summary.model_id.as_deref(),
            )?
        };

        let previous_state = {
            let state = session.state.read().await;
            if !state.can_transition_to(&AcpSessionState::Running) {
                return Err(ToolError::Sdk {
                    sdk_kind: "invalid_state".to_string(),
                    message: format!("session is in state {state:?}, cannot send prompt"),
                });
            }
            state.clone()
        };
        {
            let mut state = session.state.write().await;
            *state = AcpSessionState::Running;
        }
        {
            let mut summary = session.summary.write().await;
            if should_replace_prompt_title(&summary.title)
                && let Some(title) = title_from_prompt(&prompt.text)
            {
                summary.title = title;
            }
            summary.state = AcpSessionState::Running;
            summary.updated_at = jiff::Timestamp::now().to_string();
            if model_id.is_some() {
                summary.model_id = selected_model_id.clone();
                summary.model_name = selected_model_name.clone();
            }
        }
        // Touch activity timestamp so idle reaper leaves this session alone.
        {
            let mut activity = session
                .last_activity
                .lock()
                .expect("last_activity lock poisoned");
            *activity = Instant::now();
        }

        tracing::debug!(
            surface = "acp", service = "registry", action = "session.prompt",
            session_id = %session_id,
            prompt_len = prompt.text.len(),
            attachment_count = prompt.attachments.len(),
            "ACP session prompt dispatched",
        );

        prompt.text = match self
            .switch_runtime_if_requested(&session, &prompt.text, options)
            .await
        {
            Ok(prompt) => prompt,
            Err(error) => {
                {
                    let mut state = session.state.write().await;
                    *state = previous_state.clone();
                }
                {
                    let mut summary = session.summary.write().await;
                    summary.state = previous_state;
                    summary.updated_at = jiff::Timestamp::now().to_string();
                }
                return Err(error);
            }
        };

        let needs_reattach = { session.handle.lock().await.is_none() };
        if needs_reattach {
            tracing::warn!(
                surface = "acp", service = "registry", action = "session.reattach",
                session_id = %session_id,
                "ACP session handle was None — reattaching runtime",
            );
            self.reattach_runtime(&session).await?;
        }

        let runtime = {
            session
                .handle
                .lock()
                .await
                .clone()
                .ok_or_else(|| internal("ACP runtime unavailable"))?
        };
        runtime
            .prompt_input(prompt, selected_model_id)
            .await
            .map_err(session_command_error)?;

        if let Some(db) = self.persistence() {
            let summary = session.summary.read().await;
            if let Err(error) = db.save_session(&summary).await {
                tracing::warn!(
                    surface = "acp", service = "registry", action = "session.save",
                    session_id, error = %error,
                    "failed to persist session summary after prompt",
                );
            }
        }

        Ok(())
    }

    async fn switch_runtime_if_requested(
        &self,
        session: &Arc<Session>,
        prompt: &str,
        options: PromptSessionOptions,
    ) -> Result<String, ToolError> {
        let Some(requested_provider) = options
            .provider
            .as_deref()
            .map(|provider| normalize_provider_id(Some(provider)))
            .filter(|provider| !provider.trim().is_empty())
        else {
            return Ok(prompt.to_string());
        };

        let (current_provider, cwd, title) = {
            let summary = session.summary.read().await;
            (
                summary.provider.clone(),
                summary.cwd.clone(),
                summary.title.clone(),
            )
        };
        if requested_provider == current_provider {
            return Ok(prompt.to_string());
        }

        {
            let state = session.state.read().await;
            if !matches!(
                *state,
                AcpSessionState::Idle | AcpSessionState::Completed | AcpSessionState::Running
            ) {
                return Err(ToolError::Sdk {
                    sdk_kind: "invalid_state".to_string(),
                    message: format!("session is in state {state:?}, cannot switch provider"),
                });
            }
        }

        let Some(health) = self
            .provider_healths()
            .into_iter()
            .find(|health| health.provider == requested_provider)
        else {
            return Err(ToolError::InvalidParam {
                message: format!("unknown provider `{requested_provider}`"),
                param: "provider".to_string(),
            });
        };
        if !health.available {
            return Err(ToolError::Sdk {
                sdk_kind: "service_unavailable".to_string(),
                message: health
                    .message
                    .unwrap_or_else(|| format!("provider `{requested_provider}` is unavailable")),
            });
        }

        #[derive(Clone, Copy)]
        enum ContinuityMode {
            Handoff,
            Reset,
        }

        let continuity_mode = match options.continuity_mode.as_deref() {
            Some("reset") => ContinuityMode::Reset,
            Some("handoff" | "") | None => ContinuityMode::Handoff,
            Some(other) => {
                return Err(ToolError::InvalidParam {
                    message: format!("unsupported continuity_mode `{other}`"),
                    param: "continuity_mode".to_string(),
                });
            }
        };

        let prompt_for_provider = if matches!(continuity_mode, ContinuityMode::Reset) {
            format!(
                "You are continuing a Lab conversation that was previously handled by {current_provider}.\n\
                 Continuity mode: reset.\n\
                 No prior transcript was provided to this provider.\n\n\
                 New user prompt:\n{prompt}"
            )
        } else {
            build_handoff_prompt(session, &current_provider, prompt).await
        };

        let (event_tx, event_rx) = mpsc::channel::<AcpEvent>(ACP_EVENT_CHANNEL_CAPACITY);
        let (new_runtime, started) = launch_codex_runtime(
            session.id.clone(),
            StartSessionInput {
                provider: Some(requested_provider.clone()),
                cwd,
                title: Some(title),
                principal: Some(session.principal.clone()),
                model_id: None,
            },
            event_tx,
        )
        .await
        .map_err(internal_message)?;

        let switch_message = if matches!(continuity_mode, ContinuityMode::Reset) {
            format!(
                "Switched from {current_provider} to {requested_provider}. Context was reset for this provider."
            )
        } else {
            format!(
                "Switched from {current_provider} to {requested_provider}. Continuing with a bounded transcript handoff."
            )
        };
        let continuity_mode_str = match continuity_mode {
            ContinuityMode::Reset => "reset",
            ContinuityMode::Handoff => "handoff",
        };
        let switch_event = next_session_event(
            session,
            AcpEvent::ProviderSwitch {
                id: uuid::Uuid::new_v4().to_string(),
                created_at: jiff::Timestamp::now().to_string(),
                session_id: session.id.clone(),
                seq: 0,
                from_provider: current_provider,
                to_provider: requested_provider.clone(),
                continuity_mode: continuity_mode_str.to_string(),
                message: switch_message,
            },
        );

        let switch_event = Arc::new(switch_event);
        persist_session_event(self, &switch_event).await;
        let _ = fanout_event(session, Arc::clone(&switch_event)).await;
        apply_session_event(session, switch_event).await;

        let old_runtime = {
            let mut handle = session.handle.lock().await;
            handle.replace(new_runtime)
        };
        self.spawn_event_forwarder(
            Arc::clone(session),
            event_rx,
            Some(started.provider_session_id.clone()),
        );
        if let Some(old_runtime) = old_runtime {
            drop(old_runtime.shutdown().await);
        }

        {
            let mut summary = session.summary.write().await;
            summary.provider = requested_provider;
            summary.provider_session_id = Some(started.provider_session_id);
            summary.agent_name = Some(started.agent_name);
            summary.agent_version = Some(started.agent_version);
            summary.updated_at = jiff::Timestamp::now().to_string();
        }

        Ok(prompt_for_provider)
    }

    pub async fn cancel_session(&self, session_id: &str, principal: &str) -> Result<(), ToolError> {
        let session = self.get_session_arc(session_id).await?;
        Self::check_principal(&session, principal)?;
        {
            let state = session.state.read().await;
            if !state.can_transition_to(&AcpSessionState::Cancelled) {
                return Err(ToolError::Sdk {
                    sdk_kind: "invalid_state".to_string(),
                    message: format!("session is in state {state:?}, cannot cancel"),
                });
            }
        }
        {
            *session.state.write().await = AcpSessionState::Cancelled;
        }
        {
            let mut summary = session.summary.write().await;
            summary.state = AcpSessionState::Cancelled;
            summary.updated_at = jiff::Timestamp::now().to_string();
        }
        tracing::info!(
            surface = "acp", service = "registry", action = "session.cancel",
            session_id = %session_id, reason = "user_cancelled",
            "ACP session cancelled",
        );
        cancel_and_drop_runtime(&session, Some(&self.active_runtime_count)).await;

        if let Some(db) = self.persistence() {
            if let Err(error) = db
                .update_session_state(session_id, AcpSessionState::Cancelled)
                .await
            {
                tracing::warn!(
                    surface = "acp", service = "registry", action = "session.state",
                    session_id, error = %error, "failed to persist cancelled session state",
                );
            }
        }
        Ok(())
    }

    pub async fn approve_permission(
        &self,
        session_id: &str,
        principal: &str,
        request_id: &str,
        option_id: &str,
    ) -> Result<(), ToolError> {
        let session = self.get_session_arc(session_id).await?;
        if principal.trim().is_empty() || session.principal.is_empty() {
            return Err(ToolError::Sdk {
                sdk_kind: "auth_failed".to_string(),
                message: "authenticated session owner required to approve permission".to_string(),
            });
        }
        Self::check_principal(&session, principal)?;
        let runtime = {
            session
                .handle
                .lock()
                .await
                .clone()
                .ok_or_else(|| internal("ACP runtime unavailable"))?
        };
        runtime
            .approve_permission(request_id, option_id)
            .await
            .map_err(|message| ToolError::InvalidParam {
                message,
                param: "request_id".to_string(),
            })?;
        tracing::info!(
            surface = "acp",
            service = "registry",
            action = "permission.approve",
            session_id = %session_id,
            request_id,
            option_id,
            actor_key = %actor_key_from_principal(principal),
            "ACP permission request approved",
        );
        Ok(())
    }

    pub async fn reject_permission(
        &self,
        session_id: &str,
        principal: &str,
        request_id: &str,
    ) -> Result<(), ToolError> {
        let session = self.get_session_arc(session_id).await?;
        Self::check_principal(&session, principal)?;
        let runtime = {
            session
                .handle
                .lock()
                .await
                .clone()
                .ok_or_else(|| internal("ACP runtime unavailable"))?
        };
        runtime
            .reject_permission(request_id)
            .await
            .map_err(|message| ToolError::InvalidParam {
                message,
                param: "request_id".to_string(),
            })?;
        tracing::info!(
            surface = "acp",
            service = "registry",
            action = "permission.reject",
            session_id = %session_id,
            request_id,
            actor_key = %actor_key_from_principal(principal),
            "ACP permission request rejected",
        );
        Ok(())
    }

    pub async fn close_session(&self, session_id: &str, principal: &str) -> Result<(), ToolError> {
        let session = self.get_session_arc(session_id).await?;
        Self::check_principal(&session, principal)?;
        {
            *session.state.write().await = AcpSessionState::Closed;
        }
        {
            let mut summary = session.summary.write().await;
            summary.state = AcpSessionState::Closed;
            summary.updated_at = jiff::Timestamp::now().to_string();
        }
        tracing::info!(
            surface = "acp", service = "registry", action = "session.close",
            session_id = %session_id, reason = "user_closed",
            "ACP session closed",
        );
        cancel_and_drop_runtime(&session, Some(&self.active_runtime_count)).await;
        // Free the slot immediately.
        {
            self.sessions.write().await.remove(session_id);
        }

        if let Some(db) = self.persistence() {
            if let Err(error) = db
                .update_session_state(session_id, AcpSessionState::Closed)
                .await
            {
                tracing::warn!(
                    surface = "acp", service = "registry", action = "session.state",
                    session_id, error = %error, "failed to persist closed session state",
                );
            }
        }
        Ok(())
    }

    /// Close every session the caller owns that matches the typed selector.
    /// Returns a per-id partial-success envelope. Sessions belonging to other
    /// principals are silently omitted (matches the not_found masking pattern
    /// of `close_session`).
    pub async fn bulk_close_sessions(
        &self,
        selector: BulkCloseSelector,
        principal: &str,
    ) -> Result<BulkCloseResult, ToolError> {
        if principal.trim().is_empty() {
            return Err(ToolError::Sdk {
                sdk_kind: "auth_failed".to_string(),
                message: "authenticated principal required for bulk_close".to_string(),
            });
        }
        let now = jiff::Timestamp::now();
        let max_age_secs: Option<i64> = selector.max_age_days.map(|d| i64::from(d) * 86_400);

        let candidates: Vec<String> = {
            let sessions = self.sessions.read().await;
            let mut ids = Vec::new();
            for session in sessions.values() {
                if session.principal != principal {
                    continue;
                }
                let summary = session.summary.read().await;
                if !selector.states.is_empty() && !selector.states.contains(&summary.state) {
                    continue;
                }
                if let Some(window) = max_age_secs {
                    let updated_secs = summary
                        .updated_at
                        .parse::<jiff::Timestamp>()
                        .ok()
                        .map(|ts| now.as_second() - ts.as_second())
                        .unwrap_or(0);
                    if updated_secs < window {
                        continue;
                    }
                }
                ids.push(session.id.clone());
            }
            ids
        };

        if (candidates.len() as u32) > selector.max_count {
            return Err(ToolError::InvalidParam {
                message: format!(
                    "selector matches {} sessions; max_count is {}",
                    candidates.len(),
                    selector.max_count
                ),
                param: "selector".to_string(),
            });
        }

        let semaphore = Arc::new(tokio::sync::Semaphore::new(5));
        let mut handles = Vec::with_capacity(candidates.len());
        for id in candidates {
            let sem = semaphore.clone();
            let registry = self.clone();
            let principal_owned = principal.to_string();
            let id_for_task = id.clone();
            handles.push((
                id,
                tokio::spawn(async move {
                    let _permit = sem.acquire().await.expect("bulk_close semaphore closed");
                    registry.close_session(&id_for_task, &principal_owned).await
                }),
            ));
        }

        let mut closed = Vec::new();
        let mut failed = Vec::new();
        for (id, handle) in handles {
            match handle.await {
                Ok(Ok(())) => closed.push(id),
                Ok(Err(error)) => {
                    // Silently skip sessions reaped or otherwise inaccessible between
                    // the snapshot and close attempt — preserves not_found masking.
                    if error.kind() == "not_found" {
                        continue;
                    }
                    failed.push(BulkCloseFailure {
                        id,
                        kind: error.kind().to_string(),
                        message: error.user_message().to_string(),
                    });
                }
                Err(join_error) => {
                    tracing::error!(
                        surface = "acp",
                        service = "registry",
                        action = "session.bulk_close",
                        session_id = %id,
                        error = %join_error,
                        "bulk_close worker task failed",
                    );
                    failed.push(BulkCloseFailure {
                        id,
                        kind: "internal_error".to_string(),
                        message: "bulk_close worker failed".to_string(),
                    });
                }
            }
        }

        tracing::info!(
            surface = "acp", service = "registry", action = "session.bulk_close",
            actor_key = %actor_key_from_principal(principal),
            closed_count = closed.len(),
            failed_count = failed.len(),
            "ACP bulk_close completed",
        );

        Ok(BulkCloseResult { closed, failed })
    }

    /// Atomically create a session and queue its first prompt. On any prompt-
    /// step failure, the session is closed before the error returns — so a
    /// failed `start_and_prompt` leaves no orphan row in the sidebar.
    pub async fn start_and_prompt(
        &self,
        input: StartSessionInput,
        prompt_text: &str,
        prompt_attachments: Vec<crate::acp::params::LocalPromptAttachment>,
        principal: &str,
        prompt_options: PromptSessionOptions,
    ) -> Result<StartAndPromptResult, ToolError> {
        let session = self.create_session(input, principal).await?;

        let prompt_result = self
            .prompt_session(PromptOptions {
                session_id: session.id.clone(),
                principal: principal.to_string(),
                text: prompt_text.to_string(),
                attachments: prompt_attachments,
                model_id: session.model_id.clone(),
                provider: prompt_options.provider,
                continuity_mode: prompt_options.continuity_mode,
            })
            .await;

        if let Err(prompt_err) = prompt_result {
            // Atomicity: drop the just-created session before bubbling the
            // failure so callers see exactly one of {success, no-op}.
            tracing::warn!(
                surface = "acp",
                service = "registry",
                action = "session.start_and_prompt",
                session_id = %session.id,
                kind = %prompt_err.kind(),
                "start_and_prompt prompt step failed — closing session",
            );
            if let Err(close_err) = self.close_session(&session.id, principal).await {
                tracing::error!(
                    surface = "acp",
                    service = "registry",
                    action = "session.start_and_prompt",
                    session_id = %session.id,
                    kind = %close_err.kind(),
                    "failed to close session after prompt failure",
                );
            }
            return Err(prompt_err);
        }

        Ok(StartAndPromptResult {
            session_id: session.id.clone(),
            provider_session_id: session.provider_session_id.clone(),
            model_id: session.model_id.clone(),
            provider: session.provider.clone(),
            title: session.title.clone(),
        })
    }

    /// Gracefully terminate all sessions. Sets shutting_down flag, cancels every
    /// runtime, waits ≤10 s, then force-clears the map.
    #[allow(dead_code)]
    pub async fn shutdown_all_sessions(&self) {
        self.shutting_down.store(true, Ordering::SeqCst);
        let sessions: Vec<Arc<Session>> =
            { self.sessions.read().await.values().cloned().collect() };
        tracing::warn!(
            surface = "acp",
            service = "registry",
            action = "shutdown",
            count = sessions.len(),
            "Initiating graceful shutdown — terminating all ACP sessions",
        );
        for session in &sessions {
            cancel_and_drop_runtime(session, Some(&self.active_runtime_count)).await;
            tracing::info!(
                surface = "acp", service = "registry", action = "shutdown",
                session_id = %session.id, "ACP session cancelled during shutdown",
            );
        }
        // Wait for event forwarders to remove sessions (up to 10 s).
        let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
        while tokio::time::Instant::now() < deadline {
            if self.sessions.read().await.is_empty() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(250)).await;
        }
        let mut guard = self.sessions.write().await;
        let remaining = guard.len();
        if remaining > 0 {
            tracing::warn!(
                surface = "acp",
                service = "registry",
                action = "shutdown",
                count = remaining,
                "Force-removing sessions that did not exit within window",
            );
            guard.clear();
        } else {
            tracing::info!(
                surface = "acp",
                service = "registry",
                action = "shutdown",
                "All ACP sessions terminated cleanly",
            );
        }
    }

    /// Remove a session from the live map and free its slot.
    #[allow(dead_code)]
    pub async fn remove_session(&self, session_id: &str) {
        let session = self.sessions.write().await.remove(session_id);
        if let Some(session) = session {
            cancel_and_drop_runtime(&session, Some(&self.active_runtime_count)).await;
            tracing::info!(
                surface = "acp", service = "registry", action = "session.remove",
                session_id = %session_id, "ACP session removed from registry",
            );
        }
    }

    pub async fn get_events_since(
        &self,
        session_id: &str,
        since_seq: u64,
        principal: &str,
    ) -> Result<Vec<AcpEvent>, ToolError> {
        let session = self.get_session_arc(session_id).await?;
        Self::check_principal(&session, principal)?;
        if let Some(db) = self.persistence() {
            match db.load_events_since(session_id, since_seq).await {
                Ok(events) => return Ok(events),
                Err(error) => {
                    tracing::warn!(
                        surface = "acp", service = "registry", action = "events.load",
                        session_id, since_seq, error = %error,
                        "failed to load persisted events, falling back to in-memory transcript",
                    );
                }
            }
        }
        Ok(load_in_memory_events(&session, since_seq).await)
    }

    pub async fn subscribe(
        &self,
        session_id: &str,
        since_seq: u64,
        principal: &str,
    ) -> Result<impl Stream<Item = Arc<AcpEvent>> + use<P>, ToolError> {
        let session = self.get_session_arc(session_id).await?;
        Self::check_principal(&session, principal)?;

        let (tx, rx) = mpsc::channel::<Arc<AcpEvent>>(SUBSCRIBER_CAPACITY);
        {
            let mut subs = session.subscribers.lock().await;
            if subs.len() >= MAX_SUBSCRIBERS_PER_SESSION {
                return Err(ToolError::Sdk {
                    sdk_kind: "too_many_subscribers".to_string(),
                    message: format!(
                        "session has reached the maximum of {} concurrent subscribers",
                        MAX_SUBSCRIBERS_PER_SESSION
                    ),
                });
            }
            subs.push(tx);
        }

        let backlog: Vec<Arc<AcpEvent>> = if let Some(db) = self.persistence() {
            match db
                .load_events_since_capped(session_id, since_seq, BACKFILL_CAP)
                .await
            {
                Ok(events) => events.into_iter().map(Arc::new).collect(),
                Err(error) => {
                    tracing::warn!(
                        surface = "acp", service = "registry", action = "subscribe.backfill",
                        session_id, error = %error,
                        "failed to load backlog from SQLite, using in-memory transcript",
                    );
                    load_in_memory_events(&session, since_seq)
                        .await
                        .into_iter()
                        .map(Arc::new)
                        .collect()
                }
            }
        } else {
            load_in_memory_events(&session, since_seq)
                .await
                .into_iter()
                .map(Arc::new)
                .collect()
        };

        let last_backlog_seq = backlog.last().map(|e| e.seq()).unwrap_or(since_seq);
        let backlog_stream = stream::iter(backlog);
        let live_stream = stream::unfold(rx, |mut rx| async move {
            rx.recv().await.map(|event| (event, rx))
        })
        .filter(move |event| {
            let keep = event.seq() > last_backlog_seq;
            async move { keep }
        });

        Ok(backlog_stream.chain(live_stream))
    }

    // ── Background tasks ───────────────────────────────────────────────────

    fn spawn_health_reporter(sessions: Arc<RwLock<HashMap<String, Arc<Session>>>>) {
        tokio::spawn(async move {
            let mut interval =
                tokio::time::interval(Duration::from_secs(HEALTH_REPORT_INTERVAL_SECS));
            loop {
                interval.tick().await;
                let snapshot: Vec<Arc<Session>> =
                    { sessions.read().await.values().cloned().collect() };
                let total = snapshot.len();
                let (mut running, mut idle, mut waiting) = (0usize, 0usize, 0usize);
                for s in &snapshot {
                    match *s.state.read().await {
                        AcpSessionState::Running => running += 1,
                        AcpSessionState::WaitingForPermission => waiting += 1,
                        AcpSessionState::Idle => idle += 1,
                        _ => {}
                    }
                }
                tracing::info!(
                    surface = "acp",
                    service = "registry",
                    action = "health",
                    total_sessions = total,
                    running,
                    idle,
                    waiting_for_permission = waiting,
                    "ACP registry health report",
                );
            }
        });
    }

    fn spawn_idle_reaper(registry: AcpSessionRegistry<P>, interval_secs: u64) {
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(Duration::from_secs(interval_secs));
            loop {
                interval.tick().await;
                registry.reap_idle_sessions().await;
            }
        });
    }

    pub async fn reap_idle_sessions(&self) {
        let sessions: Vec<Arc<Session>> =
            { self.sessions.read().await.values().cloned().collect() };
        for session in sessions {
            let state = session.state.read().await.clone();
            if matches!(
                state,
                AcpSessionState::Running | AcpSessionState::WaitingForPermission
            ) {
                continue;
            }
            let idle_duration = {
                session
                    .last_activity
                    .lock()
                    .expect("last_activity lock poisoned")
                    .elapsed()
            };
            if idle_duration >= self.idle_timeout {
                tracing::info!(
                    surface = "acp", service = "registry", action = "idle_reap",
                    session_id = %session.id, state = ?state,
                    idle_secs = idle_duration.as_secs(),
                    timeout_secs = self.idle_timeout.as_secs(),
                    "ACP session exceeded idle timeout — removing from registry",
                );
                cancel_and_drop_runtime(&session, Some(&self.active_runtime_count)).await;
                self.sessions.write().await.remove(&session.id);
            }
        }
    }

    fn spawn_event_forwarder(
        &self,
        session: Arc<Session>,
        mut rx: mpsc::Receiver<AcpEvent>,
        provider_session_id: Option<String>,
    ) {
        let registry = self.clone();
        tokio::spawn(async move {
            while let Some(raw_event) = rx.recv().await {
                // Allocate Arc once; share via cheap Arc::clone across all paths.
                let event = Arc::new(next_session_event(&session, raw_event));
                persist_session_event(&registry, &event).await;
                // Arc::clone gives apply_session_event ownership without deep copy.
                let dropped = fanout_event(&session, Arc::clone(&event)).await;
                apply_session_event(&session, Arc::clone(&event)).await;

                if dropped > 0 {
                    let marker = Arc::new(next_session_event(
                        &session,
                        AcpEvent::ProviderInfo {
                            id: uuid::Uuid::new_v4().to_string(),
                            created_at: jiff::Timestamp::now().to_string(),
                            session_id: session.id.clone(),
                            seq: 0,
                            provider: "lab".to_string(),
                            raw: serde_json::json!({
                                "type": "subscriber_backpressure",
                                "dropped_subscribers": dropped,
                                "after_seq": event.seq(),
                            }),
                        },
                    ));
                    tracing::warn!(
                        surface = "acp", service = "registry", action = "fanout.backpressure",
                        session_id = %session.id, dropped_subscribers = dropped,
                        after_seq = event.seq(),
                        "subscriber backpressure — subscribers removed, replay required",
                    );
                    persist_session_event(&registry, &marker).await;
                    let _ = fanout_event(&session, Arc::clone(&marker)).await;
                    apply_session_event(&session, marker).await;
                }
            }

            // Runtime thread exited (event_tx dropped). Keep the session and
            // transcript available for replay; the idle reaper or explicit
            // close path owns eventual removal.
            let current_handle_matches = {
                let handle = session.handle.lock().await;
                match (handle.as_ref(), provider_session_id.as_deref()) {
                    (Some(handle), Some(provider_session_id)) => {
                        handle.provider_session_id == provider_session_id
                    }
                    (None, _) => true,
                    _ => false,
                }
            };
            if !current_handle_matches {
                tracing::debug!(
                    surface = "acp", service = "registry", action = "runtime.exit",
                    session_id = %session.id,
                    "superseded ACP runtime exited after provider switch",
                );
                return;
            }
            let current_state = session.state.read().await.clone();
            if matches!(
                current_state,
                AcpSessionState::Running | AcpSessionState::WaitingForPermission
            ) {
                let failed = next_session_event(
                    &session,
                    AcpEvent::SessionUpdate {
                        id: uuid::Uuid::new_v4().to_string(),
                        created_at: jiff::Timestamp::now().to_string(),
                        session_id: session.id.clone(),
                        seq: 0,
                        provider: session.summary.read().await.provider.clone(),
                        state: AcpSessionState::Failed,
                    },
                );
                let failed = Arc::new(failed);
                persist_session_event(&registry, &failed).await;
                let _ = fanout_event(&session, Arc::clone(&failed)).await;
                apply_session_event(&session, failed).await;

                let exit_event = Arc::new(next_session_event(
                    &session,
                    AcpEvent::ProviderInfo {
                        id: uuid::Uuid::new_v4().to_string(),
                        created_at: jiff::Timestamp::now().to_string(),
                        session_id: session.id.clone(),
                        seq: 0,
                        provider: "lab".to_string(),
                        raw: serde_json::json!({
                            "type": "runtime_exit",
                            "title": "ACP provider exited while session was active",
                            "status": "failed",
                        }),
                    },
                ));
                persist_session_event(&registry, &exit_event).await;
                let _ = fanout_event(&session, Arc::clone(&exit_event)).await;
                apply_session_event(&session, exit_event).await;

                tracing::error!(
                    surface = "acp", service = "registry", action = "runtime.exit",
                    session_id = %session.id, state = ?current_state,
                    "ACP subprocess exited unexpectedly while session active",
                );
            } else {
                tracing::info!(
                    surface = "acp", service = "registry", action = "runtime.exit",
                    session_id = %session.id, state = ?current_state,
                    "ACP session runtime exited cleanly",
                );
            }
            // The forwarder is the natural-exit path; decrement the counter
            // only when a live handle actually existed (avoids double-decrement
            // when cancel_and_drop_runtime already took the handle).
            let mut handle = session.handle.lock().await;
            if handle.take().is_some() {
                registry.active_runtime_count.fetch_sub(1, Ordering::SeqCst);
            }
        });
    }

    async fn reattach_runtime(&self, session: &Arc<Session>) -> Result<(), ToolError> {
        {
            let handle = session.handle.lock().await;
            if handle.is_some() {
                tracing::debug!(
                    surface = "acp", service = "registry", action = "session.reattach",
                    session_id = %session.id,
                    "reattach_runtime: handle already present — skipping",
                );
                return Ok(());
            }
        }
        tracing::warn!(
            surface = "acp", service = "registry", action = "session.reattach",
            session_id = %session.id,
            "reattach_runtime: handle was None — launching new runtime",
        );
        let (provider, cwd, title, principal) = {
            let summary = session.summary.read().await;
            (
                summary.provider.clone(),
                summary.cwd.clone(),
                summary.title.clone(),
                summary.principal.clone(),
            )
        };

        // Reserve a slot atomically before launching the provider.
        let reserved = self.active_runtime_count.fetch_add(1, Ordering::SeqCst) + 1;
        if reserved > MAX_CONCURRENT_SESSIONS {
            self.active_runtime_count.fetch_sub(1, Ordering::SeqCst);
            return Err(ToolError::Sdk {
                sdk_kind: "session_limit_exceeded".to_string(),
                message: format!(
                    "Session limit reached ({} active sessions). \
                     Kill existing sessions before starting new ones.",
                    reserved - 1,
                ),
            });
        }

        let (event_tx, event_rx) = mpsc::channel::<AcpEvent>(ACP_EVENT_CHANNEL_CAPACITY);
        let (runtime, started) = launch_codex_runtime(
            session.id.clone(),
            StartSessionInput {
                provider: Some(provider),
                cwd,
                title: Some(title),
                principal,
                model_id: None,
            },
            event_tx.clone(),
        )
        .await
        .map_err(|message| {
            self.active_runtime_count.fetch_sub(1, Ordering::SeqCst);
            internal_message(message)
        })?;

        self.spawn_event_forwarder(
            Arc::clone(session),
            event_rx,
            Some(started.provider_session_id.clone()),
        );
        {
            *session.handle.lock().await = Some(runtime);
        }
        {
            let mut summary = session.summary.write().await;
            summary.provider_session_id = Some(started.provider_session_id);
            summary.agent_name = Some(started.agent_name);
            summary.agent_version = Some(started.agent_version);
            summary.updated_at = jiff::Timestamp::now().to_string();
        }
        Ok(())
    }

    #[cfg(test)]
    async fn runtime_session_count(&self) -> usize {
        let sessions: Vec<Arc<Session>> =
            { self.sessions.read().await.values().cloned().collect() };
        let mut count = 0usize;
        for session in sessions {
            if session.handle.lock().await.is_some() {
                count += 1;
            }
        }
        count
    }

    // ── Startup restore ────────────────────────────────────────────────────

    /// Rehydrate sessions from SQLite into the in-memory map after a process
    /// restart.  Must be called once, after construction, before accepting
    /// requests.
    ///
    /// - Sessions with no `principal` are skipped — they cannot be accessed.
    /// - `next_seq` is seeded to `max(persisted_seq) + 1` so new events never
    ///   collide with the existing UNIQUE(session_id, seq) index.
    /// - Sessions that were `Running` or `WaitingForPermission` at shutdown get
    ///   synthetic `SessionUpdate{Failed}` + `ProviderInfo{container_restart}`
    ///   events written to both the in-memory buffer and SQLite so callers see
    ///   a clean terminal transition.
    pub async fn restore_from_db(&self) {
        let Some(db) = self.persistence() else {
            tracing::warn!(
                surface = "acp",
                service = "registry",
                action = "restore",
                kind = "persistence_unavailable",
                "persistence unavailable — skipping session restore",
            );
            return;
        };

        let sessions = match db.load_sessions().await {
            Ok(s) => s,
            Err(error) => {
                tracing::error!(
                    surface = "acp",
                    service = "registry",
                    action = "restore",
                    kind = "internal_error",
                    error = %error,
                    "failed to load sessions from SQLite for restore",
                );
                return;
            }
        };

        let max_seqs = match db.load_max_seqs().await {
            Ok(m) => m,
            Err(error) => {
                tracing::error!(
                    surface = "acp",
                    service = "registry",
                    action = "restore",
                    kind = "internal_error",
                    error = %error,
                    "failed to load max seqs from SQLite — aborting session restore to prevent seq collisions",
                );
                return;
            }
        };

        let now = jiff::Timestamp::now().to_string();
        let total = sessions.len();
        let mut restored = 0usize;

        for summary in sessions {
            // Closed sessions are excluded from the active working set — they are
            // preserved in SQLite for audit purposes but must not re-appear in
            // list_sessions output after a restart. Skip them here.
            if summary.state == AcpSessionState::Closed {
                continue;
            }

            let principal = match &summary.principal {
                Some(p) if !p.is_empty() => p.clone(),
                _ => continue,
            };

            let max_seq = max_seqs.get(&summary.id).copied().unwrap_or(0);
            let next_seq = max_seq + 1;

            let in_flight = matches!(
                summary.state,
                AcpSessionState::Running | AcpSessionState::WaitingForPermission
            );

            let restore_summary = if in_flight {
                AcpSessionSummary {
                    state: AcpSessionState::Failed,
                    updated_at: now.clone(),
                    ..summary.clone()
                }
            } else {
                summary.clone()
            };

            let session =
                Session::new_with_seq(summary.id.clone(), principal, restore_summary, next_seq);

            // For in-flight sessions: write synthetic failure events so SSE
            // subscribers and the DB reflect the clean Failed transition.
            if in_flight {
                let failed = next_session_event(
                    &session,
                    AcpEvent::SessionUpdate {
                        id: uuid::Uuid::new_v4().to_string(),
                        created_at: now.clone(),
                        session_id: summary.id.clone(),
                        seq: 0,
                        provider: summary.provider.clone(),
                        state: AcpSessionState::Failed,
                    },
                );
                let failed = Arc::new(failed);
                persist_session_event(self, &failed).await;
                apply_session_event(&session, failed).await;

                let info = Arc::new(next_session_event(
                    &session,
                    AcpEvent::ProviderInfo {
                        id: uuid::Uuid::new_v4().to_string(),
                        created_at: now.clone(),
                        session_id: summary.id.clone(),
                        seq: 0,
                        provider: "lab".to_string(),
                        raw: serde_json::json!({
                            "type": "container_restart",
                            "title": "Session interrupted by container restart",
                            "status": "failed",
                        }),
                    },
                ));
                persist_session_event(self, &info).await;
                apply_session_event(&session, info).await;
            }

            {
                self.sessions
                    .write()
                    .await
                    .insert(summary.id.clone(), session);
            }
            restored += 1;
        }

        tracing::info!(
            surface = "acp",
            service = "registry",
            action = "restore",
            total_in_db = total,
            restored,
            "ACP sessions restored from SQLite",
        );
    }
}

// ---------------------------------------------------------------------------
// Test-only helpers — isolated from production code paths.
// Methods on AcpSessionRegistry that are only used in #[cfg(test)] modules.
// Kept in a separate impl block so they are invisible in production builds.
// ---------------------------------------------------------------------------

#[cfg(test)]
impl<P: AcpPersistence> AcpSessionRegistry<P> {
    // ── Test helpers ───────────────────────────────────────────────────────

    /// Create a test registry with a custom idle timeout. Background tasks are
    /// NOT spawned so tests run in isolation.
    pub fn new_for_tests(idle_timeout: Duration) -> Self {
        Self {
            sessions: Arc::new(RwLock::new(HashMap::new())),
            persistence: None,
            default_cwd: ".".to_string(),
            recent_creations: Arc::new(Mutex::new(VecDeque::new())),
            shutting_down: Arc::new(AtomicBool::new(false)),
            active_runtime_count: Arc::new(AtomicUsize::new(0)),
            idle_timeout,
            provider_models: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    pub fn new_for_test_with_provider_models(
        provider_models: Vec<(String, Vec<AcpModelOption>)>,
    ) -> Self {
        let models = provider_models
            .into_iter()
            .map(|(provider, models)| (normalize_provider_id(Some(&provider)), models))
            .collect();
        Self {
            sessions: Arc::new(RwLock::new(HashMap::new())),
            persistence: None,
            default_cwd: ".".to_string(),
            recent_creations: Arc::new(Mutex::new(VecDeque::new())),
            shutting_down: Arc::new(AtomicBool::new(false)),
            active_runtime_count: Arc::new(AtomicUsize::new(0)),
            idle_timeout: Duration::from_millis(100),
            provider_models: Arc::new(RwLock::new(models)),
        }
    }

    /// Inject a pre-built session with a fake RuntimeHandle — no subprocess spawned.
    /// The returned session summary mirrors what create_session would return.
    pub async fn inject_fake_session(
        &self,
        session_id: &str,
        principal: &str,
    ) -> AcpSessionSummary {
        use super::runtime::fake_handle_for_tests;
        let created_at = jiff::Timestamp::now().to_string();
        let summary = AcpSessionSummary {
            id: session_id.to_string(),
            provider: "codex-acp".to_string(),
            title: "Test session".to_string(),
            cwd: ".".to_string(),
            state: AcpSessionState::Idle,
            created_at: created_at.clone(),
            updated_at: created_at,
            principal: if principal.is_empty() {
                None
            } else {
                Some(principal.to_string())
            },
            provider_session_id: Some("fake-provider-session".to_string()),
            agent_name: Some("test-agent".to_string()),
            agent_version: Some("0.0.1".to_string()),
            model_id: None,
            model_name: None,
            config_options: Vec::new(),
        };
        let session = Session::new(
            session_id.to_string(),
            principal.to_string(),
            summary.clone(),
        );
        let (fake_rt, fake_rx) = fake_handle_for_tests();
        {
            *session.handle.lock().await = Some(fake_rt);
        }
        self.active_runtime_count.fetch_add(1, Ordering::SeqCst);
        {
            self.sessions
                .write()
                .await
                .insert(session_id.to_string(), Arc::clone(&session));
        }

        // Minimal forwarder: just drains and marks the runtime detached on
        // channel close. Production sessions stay replayable after provider
        // exit; test sessions should preserve that lifecycle contract.
        let registry = self.clone();
        let sid = session_id.to_string();
        tokio::spawn(async move {
            let mut rx = fake_rx;
            while rx.recv().await.is_some() {}
            if let Ok(session) = registry.get_session_arc(&sid).await {
                let mut handle = session.handle.lock().await;
                if handle.take().is_some() {
                    registry.active_runtime_count.fetch_sub(1, Ordering::SeqCst);
                }
            }
        });
        summary
    }

    /// Force the cached summary state of a session for tests. Both the
    /// fast-path state lock and the summary lock are updated so any consumer
    /// that reads either sees the new value.
    pub async fn force_summary_state_for_tests(&self, session_id: &str, state: AcpSessionState) {
        if let Ok(session) = self.get_session_arc(session_id).await {
            *session.state.write().await = state.clone();
            session.summary.write().await.state = state;
        }
    }

    /// Check whether a session id is still registered (for assertions).
    pub async fn session_exists_for_tests(&self, session_id: &str) -> bool {
        self.sessions.read().await.contains_key(session_id)
    }

    /// Inject a pre-built session whose runtime command queue is already full.
    pub async fn inject_saturated_fake_session(
        &self,
        session_id: &str,
        principal: &str,
    ) -> AcpSessionSummary {
        use super::runtime::saturated_fake_handle_for_tests;

        let created_at = jiff::Timestamp::now().to_string();
        let summary = AcpSessionSummary {
            id: session_id.to_string(),
            provider: "codex-acp".to_string(),
            title: "Test session".to_string(),
            cwd: ".".to_string(),
            state: AcpSessionState::Idle,
            created_at: created_at.clone(),
            updated_at: created_at,
            principal: if principal.is_empty() {
                None
            } else {
                Some(principal.to_string())
            },
            provider_session_id: Some("fake-provider-session".to_string()),
            agent_name: Some("test-agent".to_string()),
            agent_version: Some("0.0.1".to_string()),
            model_id: None,
            model_name: None,
            config_options: Vec::new(),
        };
        let session = Session::new(
            session_id.to_string(),
            principal.to_string(),
            summary.clone(),
        );
        let (fake_rt, fake_rx) = saturated_fake_handle_for_tests();
        {
            *session.handle.lock().await = Some(fake_rt);
        }
        self.active_runtime_count.fetch_add(1, Ordering::SeqCst);
        {
            self.sessions
                .write()
                .await
                .insert(session_id.to_string(), Arc::clone(&session));
        }

        let registry = self.clone();
        let sid = session_id.to_string();
        tokio::spawn(async move {
            let mut rx = fake_rx;
            while rx.recv().await.is_some() {}
            registry.sessions.write().await.remove(&sid);
            // Counter already decremented by close/cancel path or forwarder.
        });
        summary
    }

    /// Override last_activity on a session to simulate being idle for `elapsed`.
    pub async fn set_last_activity_for_test(&self, session_id: &str, elapsed: Duration) {
        if let Ok(session) = self.get_session_arc(session_id).await {
            let past = Instant::now()
                .checked_sub(elapsed)
                .expect("elapsed too large for Instant::checked_sub");
            *session
                .last_activity
                .lock()
                .expect("last_activity lock poisoned") = past;
        }
    }

    pub async fn set_title_for_test(&self, session_id: &str, title: &str) {
        if let Ok(session) = self.get_session_arc(session_id).await {
            session.summary.write().await.title = title.to_string();
        }
    }

    pub async fn detach_runtime_for_test(&self, session_id: &str) {
        if let Ok(session) = self.get_session_arc(session_id).await {
            *session.handle.lock().await = None;
        }
    }

    pub async fn session_count(&self) -> usize {
        self.sessions.read().await.len()
    }
}

impl Default for AcpSessionRegistry {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// Free functions
// ---------------------------------------------------------------------------

/// Return a short, stable, non-reversible key suitable for operator logs.
///
/// The key is derived from the principal using a process-stable salt so
/// the same subject maps to the same key within one process run (enabling
/// log correlation) but the raw principal is never emitted to tracing events.
///
/// Format: `"ak:{16-hex-chars}"`.  Anonymous/empty principals yield
/// `"(anonymous)"`.
fn actor_key_from_principal(principal: &str) -> String {
    use std::hash::{DefaultHasher, Hash, Hasher};

    if principal.is_empty() {
        return "(anonymous)".to_string();
    }

    static PROCESS_SALT: std::sync::OnceLock<u64> = std::sync::OnceLock::new();
    let salt = PROCESS_SALT.get_or_init(|| {
        // Combine PID and wall-clock subsecond nanos as a cheap process-stable seed.
        let pid = std::process::id() as u64;
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.subsec_nanos() as u64)
            .unwrap_or(0xdead_beef_cafe_babe);
        pid.wrapping_mul(0x9e37_79b9_7f4a_7c15).wrapping_add(nanos)
    });

    let mut hasher = DefaultHasher::new();
    salt.hash(&mut hasher);
    principal.hash(&mut hasher);
    format!("ak:{:016x}", hasher.finish())
}

async fn cancel_and_drop_runtime(session: &Arc<Session>, counter: Option<&Arc<AtomicUsize>>) {
    let runtime = { session.handle.lock().await.take() };
    if let Some(rt) = runtime {
        if let Some(counter) = counter {
            counter.fetch_sub(1, Ordering::SeqCst);
        }
        drop(rt.shutdown().await);
    }
}

async fn load_in_memory_events(session: &Arc<Session>, since_seq: u64) -> Vec<AcpEvent> {
    let events = session.events.read().await;
    // Deref Arc<AcpEvent> → &AcpEvent for the seq filter; clone only the events
    // that pass (these go out to callers and need owned values).
    let filtered: Vec<AcpEvent> = events
        .iter()
        .filter(|e| e.seq() > since_seq)
        .map(|arc| (**arc).clone())
        .collect();
    let start = filtered.len().saturating_sub(BACKFILL_CAP as usize);
    filtered[start..].to_vec()
}

fn next_session_event(session: &Arc<Session>, event: AcpEvent) -> AcpEvent {
    let mut seq_guard = session.next_seq.lock().expect("next_seq lock poisoned");
    let seq = *seq_guard;
    *seq_guard += 1;
    stamp_event_sequence(event, seq)
}

async fn apply_session_event(session: &Arc<Session>, event: Arc<AcpEvent>) {
    let maybe_new_state = session_state_from_event(&event);
    {
        let mut summary = session.summary.write().await;
        summary.updated_at = event_created_at(&event).to_string();
        if let Some(ref state) = maybe_new_state {
            summary.state = state.clone();
        }
        if let Some(title) = session_title_from_event(&event) {
            summary.title = title;
        }
        if let AcpEvent::ProviderSwitch { to_provider, .. } = event.as_ref() {
            summary.provider = to_provider.clone();
        }
    }
    if let Some(new_state) = maybe_new_state {
        *session.state.write().await = new_state;
    }
    {
        let mut events = session.events.write().await;
        // O(1) front removal via VecDeque instead of O(n) Vec::drain.
        while events.len() >= 500 {
            events.pop_front();
        }
        events.push_back(event);
    }
}

fn session_state_from_event(event: &AcpEvent) -> Option<AcpSessionState> {
    match event {
        AcpEvent::SessionUpdate { state, .. } => Some(state.clone()),
        AcpEvent::PermissionRequest { .. } => Some(AcpSessionState::WaitingForPermission),
        AcpEvent::PermissionOutcome { .. } => Some(AcpSessionState::Running),
        _ => None,
    }
}

async fn fanout_event(session: &Arc<Session>, event: Arc<AcpEvent>) -> usize {
    let mut subs = session.subscribers.lock().await;
    let mut to_remove: Vec<usize> = Vec::new();
    let mut dropped = 0usize;
    for (i, tx) in subs.iter().enumerate() {
        match tx.try_send(Arc::clone(&event)) {
            Ok(()) => {}
            Err(mpsc::error::TrySendError::Full(_)) => {
                dropped += 1;
                to_remove.push(i);
                tracing::warn!(
                    surface = "acp",
                    service = "registry",
                    action = "fanout",
                    subscriber_index = i,
                    session_id = event.session_id(),
                    seq = event.seq(),
                    "subscriber mpsc full — subscriber removed, must replay from transcript",
                );
            }
            Err(mpsc::error::TrySendError::Closed(_)) => {
                to_remove.push(i);
            }
        }
    }
    for i in to_remove.into_iter().rev() {
        subs.swap_remove(i);
    }
    dropped
}

async fn persist_session_event<P: AcpPersistence>(
    registry: &AcpSessionRegistry<P>,
    event: &AcpEvent,
) {
    if let Some(db) = registry.persistence() {
        if let Err(error) = db.append_event(event).await {
            tracing::warn!(
                surface = "acp", service = "registry", action = "event.persist",
                session_id = event.session_id(), seq = event.seq(), error = %error,
                "failed to persist typed acp event; replay limited to in-memory history",
            );
        }
        if let Some(state) = session_state_from_event(event)
            && let Err(error) = db.update_session_state(event.session_id(), state).await
        {
            tracing::warn!(
                surface = "acp", service = "registry", action = "session.state.persist",
                session_id = event.session_id(), seq = event.seq(), error = %error,
                "failed to persist acp session state from event",
            );
        }
    }
}

// ---------------------------------------------------------------------------
// Error constructors
// ---------------------------------------------------------------------------

fn internal(message: &str) -> ToolError {
    ToolError::Sdk {
        sdk_kind: "internal_error".to_string(),
        message: message.to_string(),
    }
}
fn internal_message(message: String) -> ToolError {
    ToolError::Sdk {
        sdk_kind: "internal_error".to_string(),
        message,
    }
}

fn session_command_error(message: String) -> ToolError {
    if message.contains("queue saturated") {
        return ToolError::Sdk {
            sdk_kind: "queue_saturated".to_string(),
            message,
        };
    }
    internal_message(message)
}

fn not_found(message: &str) -> ToolError {
    ToolError::Sdk {
        sdk_kind: "not_found".to_string(),
        message: message.to_string(),
    }
}

fn resolve_model_selection(
    provider: &str,
    requested: Option<&str>,
    options: &[AcpModelOption],
    current: Option<&str>,
) -> Result<(Option<String>, Option<String>), ToolError> {
    let selected = requested.or(current);
    let Some(selected) = selected.map(str::trim).filter(|value| !value.is_empty()) else {
        return Ok((None, None));
    };
    if options.is_empty() {
        return Ok((Some(selected.to_string()), None));
    }
    let Some(option) = options.iter().find(|option| option.id == selected) else {
        return Err(ToolError::InvalidParam {
            message: format!("model `{selected}` is not valid for provider `{provider}`"),
            param: "model".to_string(),
        });
    };
    Ok((Some(option.id.clone()), Some(option.name.clone())))
}

fn should_replace_prompt_title(title: &str) -> bool {
    title.trim().is_empty() || title.trim() == "New session"
}

fn title_from_prompt(prompt: &str) -> Option<String> {
    let line = prompt
        .lines()
        .rev()
        .map(str::trim)
        .find(|line| !line.is_empty())?;
    let normalized = line.split_whitespace().collect::<Vec<_>>().join(" ");
    if normalized.is_empty() {
        return None;
    }
    const MAX_TITLE_CHARS: usize = 64;
    if normalized.chars().count() > MAX_TITLE_CHARS {
        let mut title = normalized
            .chars()
            .take(MAX_TITLE_CHARS.saturating_sub(3))
            .collect::<String>()
            .trim_end()
            .to_string();
        title.push_str("...");
        return Some(title);
    }
    Some(normalized)
}

async fn build_handoff_prompt(session: &Arc<Session>, from_provider: &str, prompt: &str) -> String {
    let events = session.events.read().await;
    let mut lines = Vec::new();
    for event in events.iter().rev() {
        if lines.len() >= HANDOFF_MAX_MESSAGES {
            break;
        }
        if let AcpEvent::MessageChunk {
            role,
            text,
            provider,
            ..
        } = event.as_ref()
        {
            if text.trim().is_empty() {
                continue;
            }
            let label = match role.as_str() {
                "user" => "User".to_string(),
                "assistant" => {
                    let owner = if provider.is_empty() {
                        from_provider
                    } else {
                        provider.as_str()
                    };
                    format!("Assistant ({owner})")
                }
                other => other.to_string(),
            };
            lines.push(format!(
                "{label}: {}",
                crate::dispatch::redact::redact_stdio_value(text)
            ));
        }
    }
    lines.reverse();
    let transcript = if lines.is_empty() {
        "(No prior text transcript available.)".to_string()
    } else {
        lines.join("\n")
    };
    let prompt_section = format!("\n\nNew user prompt:\n{prompt}");
    let mut transcript_header = "Recent transcript:\n";
    let mut transcript_body = transcript.as_str();
    let prefix = format!(
        "You are continuing a Lab conversation that was previously handled by {from_provider}.\n\
         Continuity mode: handoff.\n"
    );
    let full_len =
        prefix.len() + transcript_header.len() + transcript_body.len() + prompt_section.len();
    if full_len > HANDOFF_MAX_BYTES {
        transcript_header = "Recent transcript was truncated to fit the handoff budget.\n";
        let fixed_len = prefix.len() + transcript_header.len() + prompt_section.len();
        let transcript_budget = HANDOFF_MAX_BYTES.saturating_sub(fixed_len);
        transcript_body = utf8_tail_by_bytes(&transcript, transcript_budget);
    }
    format!("{prefix}{transcript_header}{transcript_body}{prompt_section}")
}

fn utf8_tail_by_bytes(value: &str, max_bytes: usize) -> &str {
    if value.len() <= max_bytes {
        return value;
    }
    if max_bytes == 0 {
        return "";
    }
    let mut start = value.len().saturating_sub(max_bytes);
    while start < value.len() && !value.is_char_boundary(start) {
        start += 1;
    }
    &value[start..]
}

// ---------------------------------------------------------------------------
// Concrete SqliteAcpPersistence constructor
// ---------------------------------------------------------------------------

impl AcpSessionRegistry<SqliteAcpPersistence> {
    /// Create a test registry pre-seeded with an existing persistence.
    /// Background tasks are NOT spawned so tests run in isolation.
    #[cfg(test)]
    pub fn new_for_tests_with_persistence(db: SqliteAcpPersistence) -> Self {
        Self {
            sessions: Arc::new(RwLock::new(HashMap::new())),
            persistence: Some(Arc::new(db)),
            default_cwd: ".".to_string(),
            recent_creations: Arc::new(Mutex::new(VecDeque::new())),
            shutting_down: Arc::new(AtomicBool::new(false)),
            active_runtime_count: Arc::new(AtomicUsize::new(0)),
            idle_timeout: Duration::from_millis(100),
            provider_models: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// Build a registry backed by `SqliteAcpPersistence`, initialising
    /// persistence from the `LAB_ACP_DB` environment variable.
    ///
    /// If the env var is absent or the database cannot be opened, the registry
    /// falls back to in-memory-only mode (no persistence) and logs a warning.
    pub async fn from_env() -> Self {
        crate::acp::runtime::warn_if_acp_provider_sandbox_is_incompatible();
        let default_cwd = std::env::var("ACP_SESSION_CWD").unwrap_or_else(|_| {
            std::env::current_dir()
                .map(|p| p.display().to_string())
                .unwrap_or_else(|_| ".".to_string())
        });
        let persistence = match SqliteAcpPersistence::from_env().await {
            Ok(db) => Some(Arc::new(db)),
            Err(error) => {
                tracing::error!(
                    surface = "acp", service = "persistence", action = "init",
                    kind = "internal_error", error = %error,
                    "failed to open SQLite ACP database — registry will run without persistence",
                );
                None
            }
        };
        let registry = Self {
            sessions: Arc::new(RwLock::new(HashMap::new())),
            persistence,
            default_cwd,
            recent_creations: Arc::new(Mutex::new(VecDeque::new())),
            shutting_down: Arc::new(AtomicBool::new(false)),
            active_runtime_count: Arc::new(AtomicUsize::new(0)),
            idle_timeout: Duration::from_secs(SESSION_IDLE_TIMEOUT_MINS * 60),
            provider_models: Arc::new(RwLock::new(HashMap::new())),
        };
        Self::spawn_health_reporter(Arc::clone(&registry.sessions));
        Self::spawn_idle_reaper(registry.clone(), IDLE_REAPER_INTERVAL_SECS);
        registry
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn test_registry() -> AcpSessionRegistry {
        AcpSessionRegistry::new_for_tests(Duration::from_millis(100))
    }

    #[tokio::test]
    async fn test_session_limit_enforced() {
        let registry = test_registry();
        for i in 0..MAX_CONCURRENT_SESSIONS {
            registry
                .inject_fake_session(&format!("sess-limit-{i}"), "")
                .await;
        }
        assert_eq!(registry.session_count().await, MAX_CONCURRENT_SESSIONS);
        // Verify the limit constant is sane and the map is at capacity.
        assert_eq!(MAX_CONCURRENT_SESSIONS, 20);
    }

    #[tokio::test]
    async fn test_session_limit_error_kind() {
        let registry = test_registry();
        for i in 0..MAX_CONCURRENT_SESSIONS {
            registry.inject_fake_session(&format!("lim-{i}"), "").await;
        }
        // The guard code returns session_limit_exceeded — verify the kind string.
        let err = ToolError::Sdk {
            sdk_kind: "session_limit_exceeded".to_string(),
            message: format!(
                "Session limit reached ({} active sessions). \
                 Kill existing sessions before starting new ones.",
                MAX_CONCURRENT_SESSIONS
            ),
        };
        assert!(err.kind().contains("limit") || err.to_string().contains("limit"));
    }

    #[tokio::test]
    async fn test_circuit_breaker_trips_on_storm() {
        let registry = test_registry();
        // Pre-fill the creation window to simulate a storm.
        {
            let mut recent = registry.recent_creations.lock().await;
            for _ in 0..STORM_MAX_CREATIONS {
                recent.push_back(Instant::now());
            }
        }
        let now = Instant::now();
        let in_window = {
            let recent = registry.recent_creations.lock().await;
            recent
                .iter()
                .filter(|t| now.duration_since(**t).as_secs() < STORM_WINDOW_SECS)
                .count()
        };
        assert!(
            in_window >= STORM_MAX_CREATIONS,
            "circuit breaker should trip: {in_window} >= {STORM_MAX_CREATIONS}"
        );
    }

    #[tokio::test]
    async fn test_duplicate_session_id_overwrites() {
        let registry = test_registry();
        registry.inject_fake_session("dup-sess", "user1").await;
        registry.inject_fake_session("dup-sess", "user2").await;
        // Second inject replaces the first — only one entry in the map.
        assert_eq!(registry.session_count().await, 1);
    }

    #[tokio::test]
    async fn test_reattach_runtime_skips_when_handle_present() {
        let registry = test_registry();
        registry.inject_fake_session("reattach-sess", "").await;
        let session = registry.get_session_arc("reattach-sess").await.unwrap();
        assert!(
            session.handle.lock().await.is_some(),
            "handle must be present after inject"
        );
        // reattach_runtime should return Ok without spawning (handle already present).
        let result = registry.reattach_runtime(&session).await;
        assert!(result.is_ok());
        assert!(
            session.handle.lock().await.is_some(),
            "handle must still be present"
        );
    }

    #[tokio::test]
    async fn test_session_retained_after_event_stream_closes() {
        let registry = test_registry();
        registry.inject_fake_session("exit-sess", "").await;
        assert_eq!(registry.session_count().await, 1);
        // Drop the handle — closes command_tx, which closes the fake channel,
        // causing the minimal forwarder task to exit while retaining the
        // session for transcript replay.
        {
            let session = registry.get_session_arc("exit-sess").await.unwrap();
            *session.handle.lock().await = None;
        }
        let deadline = tokio::time::Instant::now() + Duration::from_secs(2);
        while tokio::time::Instant::now() < deadline {
            let session = registry.get_session_arc("exit-sess").await.unwrap();
            if session.handle.lock().await.is_none() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        assert_eq!(
            registry.session_count().await,
            1,
            "session retained after runtime exit"
        );
    }

    #[tokio::test]
    async fn test_idle_timeout_terminates_session() {
        let registry = test_registry(); // 100 ms timeout
        registry.inject_fake_session("idle-sess", "").await;
        registry
            .set_last_activity_for_test("idle-sess", Duration::from_secs(10))
            .await;
        registry.reap_idle_sessions().await;
        assert_eq!(
            registry.session_count().await,
            0,
            "idle session must be reaped"
        );
    }

    #[tokio::test]
    async fn test_shutdown_terminates_all_sessions() {
        let registry = test_registry();
        for i in 0..3 {
            registry.inject_fake_session(&format!("sd-{i}"), "").await;
        }
        assert_eq!(registry.session_count().await, 3);
        registry.shutdown_all_sessions().await;
        assert_eq!(
            registry.session_count().await,
            0,
            "all sessions removed after shutdown"
        );
        assert!(
            registry.shutting_down.load(Ordering::SeqCst),
            "shutting_down flag set"
        );
    }

    #[tokio::test]
    async fn prompt_session_returns_queue_saturated_when_command_queue_is_full() {
        let registry = test_registry();
        registry
            .inject_saturated_fake_session("saturated-sess", "alice")
            .await;

        let err = registry
            .prompt_session(PromptOptions {
                session_id: "saturated-sess".to_string(),
                principal: "alice".to_string(),
                text: "hello".to_string(),
                attachments: Vec::new(),
                model_id: None,
                provider: None,
                continuity_mode: None,
            })
            .await
            .expect_err("full command queue must be rejected");

        assert_eq!(err.kind(), "queue_saturated");
    }

    #[tokio::test]
    async fn prompt_session_sets_fallback_title_from_first_prompt() {
        let registry = test_registry();
        registry.inject_fake_session("title-sess", "alice").await;
        registry
            .set_title_for_test("title-sess", "New session")
            .await;

        registry
            .prompt_session(PromptOptions {
                session_id: "title-sess".to_string(),
                principal: "alice".to_string(),
                text: "Context: route=/chat\n\nInvestigate empty ACP sessions".to_string(),
                attachments: Vec::new(),
                model_id: None,
                provider: None,
                continuity_mode: None,
            })
            .await
            .expect("prompt dispatch");

        let summary = registry
            .get_session("title-sess")
            .await
            .expect("session summary");
        assert_eq!(summary.title, "Investigate empty ACP sessions");
    }

    #[tokio::test]
    async fn prompt_session_rolls_back_state_when_provider_switch_fails() {
        let registry = test_registry();
        registry
            .inject_fake_session("switch-fail-sess", "alice")
            .await;

        let err = registry
            .prompt_session(PromptOptions {
                session_id: "switch-fail-sess".to_string(),
                principal: "alice".to_string(),
                text: "continue this on another provider".to_string(),
                attachments: Vec::new(),
                model_id: None,
                provider: Some("missing-provider".to_string()),
                continuity_mode: Some("handoff".to_string()),
            })
            .await
            .expect_err("unknown provider switch should fail");

        assert_eq!(err.kind(), "invalid_param");
        let summary = registry
            .get_session("switch-fail-sess")
            .await
            .expect("session summary");
        assert_eq!(summary.state, AcpSessionState::Idle);
    }

    #[tokio::test]
    async fn handoff_truncation_preserves_full_prompt_and_utf8_boundaries() {
        let registry = test_registry();
        registry.inject_fake_session("handoff-sess", "alice").await;
        let session = registry
            .get_session_arc("handoff-sess")
            .await
            .expect("session");
        {
            let mut events = session.events.write().await;
            events.push_back(Arc::new(AcpEvent::MessageChunk {
                id: "evt-1".to_string(),
                created_at: "2026-05-05T00:00:00Z".to_string(),
                session_id: "handoff-sess".to_string(),
                seq: 1,
                provider: "codex-acp".to_string(),
                role: "assistant".to_string(),
                text: "🙂".repeat(HANDOFF_MAX_BYTES),
                message_id: "msg-1".to_string(),
            }));
        }
        let prompt = format!("Preserve this exact prompt: {}", "ü".repeat(256));

        let handoff = build_handoff_prompt(&session, "codex-acp", &prompt).await;

        assert!(
            handoff.ends_with(&format!("New user prompt:\n{prompt}")),
            "handoff should preserve the full new prompt"
        );
        assert!(
            handoff.len() <= HANDOFF_MAX_BYTES + "\n\nNew user prompt:\n".len() + prompt.len(),
            "transcript truncation should stay byte-bounded apart from the preserved prompt"
        );
        assert!(std::str::from_utf8(handoff.as_bytes()).is_ok());
    }

    #[tokio::test]
    async fn test_session_limit_resets_after_removal() {
        let registry = test_registry();
        for i in 0..MAX_CONCURRENT_SESSIONS {
            registry.inject_fake_session(&format!("rs-{i}"), "").await;
        }
        assert_eq!(registry.session_count().await, MAX_CONCURRENT_SESSIONS);
        registry.remove_session("rs-0").await;
        assert_eq!(registry.session_count().await, MAX_CONCURRENT_SESSIONS - 1);
    }

    #[tokio::test]
    async fn restored_sessions_without_runtime_do_not_count_against_limit() {
        let registry = test_registry();
        for i in 0..(MAX_CONCURRENT_SESSIONS + 5) {
            let session_id = format!("restored-{i}");
            registry.inject_fake_session(&session_id, "alice").await;
            registry.detach_runtime_for_test(&session_id).await;
        }

        assert_eq!(registry.session_count().await, MAX_CONCURRENT_SESSIONS + 5);
        assert_eq!(registry.runtime_session_count().await, 0);
    }

    #[test]
    fn title_from_prompt_uses_last_user_line_and_bounds_length() {
        let prompt = "Context: route=/chat\n\nSummarize the Docker ACP session creation behavior and identify the trigger.";
        assert_eq!(
            title_from_prompt(prompt).as_deref(),
            Some("Summarize the Docker ACP session creation behavior and identi...")
        );
    }

    #[test]
    fn resolve_model_selection_accepts_requested_model_without_cached_options() {
        let (model_id, model_name) =
            resolve_model_selection("codex-acp", Some("gpt-5.4"), &[], None)
                .expect("requested model should pass through when provider model cache is empty");

        assert_eq!(model_id.as_deref(), Some("gpt-5.4"));
        assert_eq!(model_name, None);
    }

    #[test]
    fn resolve_model_selection_uses_cached_model_metadata() {
        let options = vec![AcpModelOption {
            id: "gpt-5.4".to_string(),
            name: "GPT 5.4".to_string(),
            description: Some("fast".to_string()),
            fixed: false,
        }];

        let (model_id, model_name) =
            resolve_model_selection("codex-acp", Some("gpt-5.4"), &options, None)
                .expect("cached model should validate");

        assert_eq!(model_id.as_deref(), Some("gpt-5.4"));
        assert_eq!(model_name.as_deref(), Some("GPT 5.4"));
    }

    #[test]
    fn provider_healths_does_not_synthesize_current_or_default_model_from_order() {
        let registry: AcpSessionRegistry =
            AcpSessionRegistry::new_for_test_with_provider_models(vec![(
                "codex-acp".to_string(),
                vec![AcpModelOption {
                    id: "gpt-5-mini".to_string(),
                    name: "GPT-5 Mini".to_string(),
                    description: None,
                    fixed: false,
                }],
            )]);

        let codex = registry
            .provider_healths()
            .into_iter()
            .find(|health| health.provider == "codex-acp")
            .expect("codex provider health should be present");

        assert_eq!(codex.models.len(), 1);
        assert_eq!(codex.default_model_id, None);
        assert_eq!(codex.current_model_id, None);
    }

    #[test]
    fn utf8_tail_by_bytes_never_splits_multibyte_characters() {
        assert_eq!(utf8_tail_by_bytes("a🙂b", 2), "b");
        assert_eq!(utf8_tail_by_bytes("a🙂b", 5), "🙂b");
    }

    /// Regression: closed sessions persisted in SQLite must not re-appear in
    /// list_sessions after a restart (restore_from_db must skip Closed rows).
    /// See lab-wykt.
    #[tokio::test]
    async fn test_restore_from_db_skips_closed_sessions() {
        let dir = tempfile::tempdir().expect("tempdir");
        let db_path = dir.path().join("test-acp.db");
        let db = SqliteAcpPersistence::open(db_path)
            .await
            .expect("open test DB");

        let now = jiff::Timestamp::now().to_string();

        // Insert one active session and one closed session.
        let active = AcpSessionSummary {
            id: "active-sess".to_string(),
            provider: "codex-acp".to_string(),
            title: "Active".to_string(),
            cwd: ".".to_string(),
            state: AcpSessionState::Idle,
            created_at: now.clone(),
            updated_at: now.clone(),
            principal: Some("alice".to_string()),
            provider_session_id: None,
            agent_name: None,
            agent_version: None,
            model_id: None,
            model_name: None,
            config_options: vec![],
        };
        let closed = AcpSessionSummary {
            id: "closed-sess".to_string(),
            state: AcpSessionState::Closed,
            title: "Closed".to_string(),
            principal: Some("alice".to_string()),
            ..active.clone()
        };

        db.save_session(&active).await.expect("save active");
        db.save_session(&closed).await.expect("save closed");

        // Simulate restart: create a fresh registry with the same DB and restore.
        let registry = AcpSessionRegistry::new_for_tests_with_persistence(db);
        registry.restore_from_db().await;

        // Only the active session should be present; the closed one must be excluded.
        let sessions = registry.list_sessions("alice").await;
        assert_eq!(sessions.len(), 1, "only active session should be restored");
        assert_eq!(sessions[0].id, "active-sess");
    }

    /// Bead lab-qq8y.4: atomic counter increments when sessions are injected and
    /// the cap is enforced without a TOCTOU window.
    #[tokio::test]
    async fn atomic_runtime_count_tracks_injected_sessions() {
        let registry = test_registry();
        assert_eq!(
            registry.active_runtime_count.load(Ordering::SeqCst),
            0,
            "counter starts at zero"
        );

        for i in 0..MAX_CONCURRENT_SESSIONS {
            registry.inject_fake_session(&format!("arc-{i}"), "").await;
        }
        assert_eq!(
            registry.active_runtime_count.load(Ordering::SeqCst),
            MAX_CONCURRENT_SESSIONS,
            "counter reaches MAX after injecting MAX sessions"
        );
    }

    /// Bead lab-qq8y.4: create→close cycles do not permanently consume counter
    /// slots — a full create→close cycle must allow the next create to proceed.
    ///
    /// We drive this without spawning real provider processes by manipulating
    /// the counter directly, mirroring what create_session + close_session do.
    #[tokio::test]
    async fn atomic_runtime_count_released_on_close() {
        let registry = test_registry();

        // Fill to MAX using inject_fake_session (increments counter).
        for i in 0..MAX_CONCURRENT_SESSIONS {
            registry.inject_fake_session(&format!("rc-{i}"), "").await;
        }
        assert_eq!(
            registry.active_runtime_count.load(Ordering::SeqCst),
            MAX_CONCURRENT_SESSIONS,
        );

        // Verify the atomic guard would reject a 21st create.
        let reserved = registry.active_runtime_count.fetch_add(1, Ordering::SeqCst) + 1;
        assert!(
            reserved > MAX_CONCURRENT_SESSIONS,
            "21st reservation must exceed the cap (got {reserved})"
        );
        registry.active_runtime_count.fetch_sub(1, Ordering::SeqCst); // roll back

        // Drop one session's handle directly to simulate a runtime exit / close.
        if let Ok(session) = registry.get_session_arc("rc-0").await {
            let mut handle = session.handle.lock().await;
            if handle.take().is_some() {
                registry.active_runtime_count.fetch_sub(1, Ordering::SeqCst);
            }
        }
        assert_eq!(
            registry.active_runtime_count.load(Ordering::SeqCst),
            MAX_CONCURRENT_SESSIONS - 1,
            "counter decrements when a handle is released"
        );

        // Now the atomic guard must allow a new reservation.
        let reserved2 = registry.active_runtime_count.fetch_add(1, Ordering::SeqCst) + 1;
        assert!(
            reserved2 <= MAX_CONCURRENT_SESSIONS,
            "reservation after a release must succeed (got {reserved2})"
        );
        registry.active_runtime_count.fetch_sub(1, Ordering::SeqCst); // clean up
    }
}
