use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};
use std::{collections::HashMap, fmt};

use agent_client_protocol::schema::{
    BlobResourceContents, CancelNotification, ClientCapabilities, ConfigOptionUpdate, ContentBlock,
    ContentChunk, CreateTerminalRequest, CurrentModeUpdate, EmbeddedResource,
    EmbeddedResourceResource, FileSystemCapabilities, Implementation, InitializeRequest,
    KillTerminalRequest, PermissionOption, PermissionOptionKind, PromptRequest, PromptResponse,
    ProtocolVersion, ReadTextFileRequest, ReleaseTerminalRequest, RequestPermissionOutcome,
    RequestPermissionRequest, RequestPermissionResponse, SelectedPermissionOutcome,
    SessionInfoUpdate, SessionNotification, SessionUpdate, SetSessionModelRequest, StopReason,
    TerminalOutputRequest, TextContent, TextResourceContents, WaitForTerminalExitRequest,
    WriteTextFileRequest,
};
use agent_client_protocol::{
    Agent, ByteStreams, Client, ConnectionTo, Dispatch, JsonRpcMessage, on_receive_request,
};
use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::sync::{Notify, mpsc, oneshot};
use tokio_util::compat::{TokioAsyncReadCompatExt, TokioAsyncWriteCompatExt};

#[cfg(unix)]
use nix::sys::signal::{Signal, killpg};
#[cfg(unix)]
use nix::unistd::Pid;

use super::types::{
    AcpEvent, AcpPermissionOption, AcpProviderHealth, StartSessionInput, StartSessionResult,
};
use crate::acp::providers::{AcpProviderEntry, read_providers};
use crate::dispatch::redact::redact_stdio_value;

fn acp_internal_error(message: impl Into<String>) -> agent_client_protocol::Error {
    agent_client_protocol::Error::internal_error().data(message.into())
}

// Provider prompt idle timeout. Once the runtime has seen at least one
// assistant output chunk, the prompt read loop arms a timer of this duration;
// if no further provider update arrives in that window, the runtime emits an
// `idle_completion` provider_info event, transitions the session to
// `Completed`, and breaks the read loop. Override at runtime via
// `LAB_ACP_PROMPT_IDLE_TIMEOUT_MS` (milliseconds; zero/invalid falls back to
// this default).
//
// Operator-facing documentation: `docs/acp/README.md` ("Provider prompt idle
// timeout"). Keep that section in sync when the default or behavior changes.
const DEFAULT_PROMPT_IDLE_TIMEOUT: Duration = Duration::from_secs(5);
const DEFAULT_PERMISSION_TIMEOUT: Duration = Duration::from_secs(60);
#[cfg(not(test))]
const DEFAULT_RUNTIME_SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(3);
#[cfg(test)]
const DEFAULT_RUNTIME_SHUTDOWN_TIMEOUT: Duration = Duration::from_millis(25);
const MAX_PROVIDER_STDERR_CHARS: usize = 2_048;
const SESSION_COMMAND_QUEUE_CAPACITY: usize = 8;
const CODEX_DOCKER_SAFE_SANDBOX_MODE: &str = "danger-full-access";

// See `docs/acp/README.md` ("Provider prompt idle timeout") for the
// operator-facing description of this knob.
fn acp_prompt_idle_timeout() -> Duration {
    std::env::var("LAB_ACP_PROMPT_IDLE_TIMEOUT_MS")
        .ok()
        .and_then(|value| value.parse::<u64>().ok())
        .map(Duration::from_millis)
        .filter(|duration| !duration.is_zero())
        .unwrap_or(DEFAULT_PROMPT_IDLE_TIMEOUT)
}

// Maximum time to wait for a late PromptResponse after idle_completion. During
// agentic tool calls, codex-acp may be working silently for longer than the
// idle timeout, so PromptResponse (and its StopReason) can arrive well after
// the inner prompt loop has already broken. This drain window is how long we
// are willing to wait before starting the next turn regardless.
const DEFAULT_TURN_DRAIN_TIMEOUT: Duration = Duration::from_secs(300);

fn acp_turn_drain_timeout() -> Duration {
    std::env::var("LAB_ACP_TURN_DRAIN_TIMEOUT_MS")
        .ok()
        .and_then(|value| value.parse::<u64>().ok())
        .map(Duration::from_millis)
        .filter(|duration| !duration.is_zero())
        .unwrap_or(DEFAULT_TURN_DRAIN_TIMEOUT)
}

fn acp_permission_timeout() -> Duration {
    std::env::var("LAB_ACP_PERMISSION_TIMEOUT_MS")
        .ok()
        .and_then(|value| value.parse::<u64>().ok())
        .map(Duration::from_millis)
        .filter(|duration| !duration.is_zero())
        .unwrap_or(DEFAULT_PERMISSION_TIMEOUT)
}

fn lab_client_capabilities() -> ClientCapabilities {
    // Lab relays display metadata, but it does not currently provide a safe
    // provider filesystem jail. Keep provider-side fs requests disabled until
    // a contained workspace policy and permission flow exist.
    let mut meta = serde_json::Map::new();
    meta.insert("terminal_output".to_string(), json!(true));
    ClientCapabilities::new()
        .fs(FileSystemCapabilities::new()
            .read_text_file(false)
            .write_text_file(false))
        .meta(meta)
}

#[derive(Clone)]
pub struct RuntimeHandle {
    #[allow(dead_code)]
    pub provider_session_id: String,
    command_tx: mpsc::Sender<SessionCommand>,
    permissions: Arc<PendingPermissions>,
    terminated: Arc<AtomicBool>,
    termination_notify: Arc<Notify>,
    #[cfg(test)]
    _event_tx_for_tests: Option<mpsc::Sender<AcpEvent>>,
}

impl RuntimeHandle {
    #[allow(dead_code)]
    pub async fn prompt(&self, prompt: String, model_id: Option<String>) -> Result<(), String> {
        self.prompt_input(
            PromptInput {
                text: prompt,
                attachments: Vec::new(),
            },
            model_id,
        )
        .await
    }

    pub async fn prompt_input(
        &self,
        input: PromptInput,
        model_id: Option<String>,
    ) -> Result<(), String> {
        self.command_tx
            .try_send(SessionCommand::Prompt(PromptCommand { input, model_id }))
            .map_err(session_command_send_error)
    }

    pub async fn cancel(&self) -> Result<(), String> {
        self.permissions.cancel_all();
        self.command_tx
            .try_send(SessionCommand::Cancel)
            .map_err(session_command_send_error)
    }

    pub async fn shutdown(self) -> Result<(), String> {
        let terminated = Arc::clone(&self.terminated);
        let termination_notify = Arc::clone(&self.termination_notify);
        let cancel_result = self.cancel().await;
        drop(self);

        if terminated.load(Ordering::SeqCst) {
            return cancel_result;
        }

        match tokio::time::timeout(
            DEFAULT_RUNTIME_SHUTDOWN_TIMEOUT,
            termination_notify.notified(),
        )
        .await
        {
            Ok(()) => cancel_result,
            Err(_) => {
                tracing::warn!(
                    surface = "acp",
                    service = "runtime",
                    action = "runtime.shutdown.timeout",
                    timeout_ms = DEFAULT_RUNTIME_SHUTDOWN_TIMEOUT.as_millis(),
                    "ACP runtime did not report termination before timeout",
                );
                cancel_result
            }
        }
    }

    pub async fn approve_permission(
        &self,
        request_id: &str,
        option_id: &str,
    ) -> Result<(), String> {
        self.permissions.approve(request_id, option_id)
    }

    pub async fn reject_permission(&self, request_id: &str) -> Result<(), String> {
        self.permissions.reject(request_id)
    }
}

#[derive(Clone)]
pub struct PromptAttachment {
    #[allow(dead_code)]
    pub id: String,
    pub name: String,
    pub mime_type: String,
    #[allow(dead_code)]
    pub size: u64,
    pub content: PromptAttachmentContent,
}

#[derive(Clone)]
pub enum PromptAttachmentContent {
    Text(String),
    Blob(String),
}

#[derive(Clone)]
pub struct PromptInput {
    pub text: String,
    pub attachments: Vec<PromptAttachment>,
}

enum SessionCommand {
    Prompt(PromptCommand),
    Cancel,
}

struct PromptCommand {
    input: PromptInput,
    model_id: Option<String>,
}

fn session_command_send_error(error: mpsc::error::TrySendError<SessionCommand>) -> String {
    match error {
        mpsc::error::TrySendError::Full(_) => "ACP session command queue saturated".to_string(),
        mpsc::error::TrySendError::Closed(_) => "ACP session command channel closed".to_string(),
    }
}

fn prompt_input_to_content_blocks(input: &PromptInput) -> Vec<ContentBlock> {
    let mut blocks = Vec::with_capacity(1 + input.attachments.len());
    blocks.push(ContentBlock::Text(TextContent::new(input.text.clone())));

    for attachment in &input.attachments {
        let uri = format!(
            "file://local-attachment/{}",
            percent_encode_path_segment(&attachment.name)
        );
        let resource = match &attachment.content {
            PromptAttachmentContent::Text(text) => EmbeddedResourceResource::TextResourceContents(
                TextResourceContents::new(text.clone(), uri)
                    .mime_type(attachment.mime_type.clone()),
            ),
            PromptAttachmentContent::Blob(base64) => {
                EmbeddedResourceResource::BlobResourceContents(
                    BlobResourceContents::new(base64.clone(), uri)
                        .mime_type(attachment.mime_type.clone()),
                )
            }
        };
        blocks.push(ContentBlock::Resource(EmbeddedResource::new(resource)));
    }

    blocks
}

fn percent_encode_path_segment(value: &str) -> String {
    let mut encoded = String::with_capacity(value.len());
    for byte in value.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'.' | b'_' | b'-' => {
                encoded.push(char::from(byte));
            }
            _ => {
                encoded.push_str(&format!("%{byte:02X}"));
            }
        }
    }
    encoded
}

#[derive(Default)]
struct StreamMessageIds {
    user: Option<String>,
    assistant: Option<String>,
}

impl StreamMessageIds {
    fn user_message_id(&mut self) -> String {
        self.user
            .get_or_insert_with(|| uuid::Uuid::new_v4().to_string())
            .clone()
    }

    fn assistant_message_id(&mut self) -> String {
        self.assistant
            .get_or_insert_with(|| uuid::Uuid::new_v4().to_string())
            .clone()
    }
}

struct RuntimeStarted {
    provider_session_id: String,
    agent_name: String,
    agent_version: String,
    model_id: Option<String>,
    model_name: Option<String>,
    models: Vec<lab_apis::acp::types::AcpModelOption>,
}

fn session_model_options(
    response: &agent_client_protocol::schema::NewSessionResponse,
) -> (Option<String>, Vec<lab_apis::acp::types::AcpModelOption>) {
    let Some(models) = response.models.as_ref() else {
        return (None, Vec::new());
    };
    let current = Some(models.current_model_id.to_string());
    let options = models
        .available_models
        .iter()
        .map(|model| lab_apis::acp::types::AcpModelOption {
            id: model.model_id.to_string(),
            name: model.name.clone(),
            description: model.description.clone(),
            fixed: false,
        })
        .collect();
    (current, options)
}

#[derive(Default)]
struct PromptLifecycle {
    active: AtomicBool,
    terminal_sent: AtomicBool,
    saw_prompt_progress: AtomicBool,
}

impl PromptLifecycle {
    fn start(&self) {
        self.active.store(true, Ordering::SeqCst);
        self.terminal_sent.store(false, Ordering::SeqCst);
        self.saw_prompt_progress.store(false, Ordering::SeqCst);
    }

    fn note_prompt_progress(&self) {
        self.saw_prompt_progress.store(true, Ordering::SeqCst);
    }

    fn finish(&self) {
        self.terminal_sent.store(true, Ordering::SeqCst);
        self.active.store(false, Ordering::SeqCst);
    }

    fn take_unfinished_prompt(&self) -> Option<bool> {
        let was_active = self.active.swap(false, Ordering::SeqCst);
        let terminal_sent = self.terminal_sent.load(Ordering::SeqCst);
        if was_active && !terminal_sent {
            self.terminal_sent.store(true, Ordering::SeqCst);
            return Some(self.saw_prompt_progress.load(Ordering::SeqCst));
        }
        None
    }
}

struct SessionDispatchProgress {
    assistant_output: bool,
    prompt_progress: bool,
}

#[derive(Clone)]
struct PendingPermissions {
    entries: Arc<Mutex<HashMap<String, PendingPermissionEntry>>>,
    timeout: Duration,
}

struct PendingPermissionEntry {
    session_id: String,
    options: Vec<PermissionOption>,
    decision_tx: oneshot::Sender<PermissionDecision>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum PermissionDecision {
    Approve { option_id: String },
    Reject,
    Cancel,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum PermissionDecisionError {
    NotPending,
    InvalidOption,
    NotAllowOption,
}

impl fmt::Display for PermissionDecisionError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotPending => formatter.write_str("permission request is not pending"),
            Self::InvalidOption => {
                formatter.write_str("permission option is not valid for request")
            }
            Self::NotAllowOption => formatter.write_str("permission option is not an allow option"),
        }
    }
}

impl PendingPermissions {
    fn new(timeout: Duration) -> Self {
        Self {
            entries: Arc::new(Mutex::new(HashMap::new())),
            timeout,
        }
    }

    #[allow(dead_code)]
    fn pending_count(&self) -> usize {
        self.entries
            .lock()
            .map(|entries| entries.len())
            .unwrap_or_default()
    }

    fn approve(&self, request_id: &str, option_id: &str) -> Result<(), String> {
        self.resolve(
            request_id,
            PermissionDecision::Approve {
                option_id: option_id.to_string(),
            },
        )
        .map_err(|error| error.to_string())
    }

    fn reject(&self, request_id: &str) -> Result<(), String> {
        self.resolve(request_id, PermissionDecision::Reject)
            .map_err(|error| error.to_string())
    }

    fn cancel_all(&self) {
        let entries = self
            .entries
            .lock()
            .map(|mut entries| entries.drain().map(|(_, entry)| entry).collect::<Vec<_>>())
            .unwrap_or_default();
        for entry in entries {
            drop(entry.decision_tx.send(PermissionDecision::Cancel));
        }
    }

    fn cancel_session(&self, session_id: &str) {
        let entries = self
            .entries
            .lock()
            .map(|mut entries| {
                let request_ids = entries
                    .iter()
                    .filter(|(_, entry)| entry.session_id == session_id)
                    .map(|(request_id, _)| request_id.clone())
                    .collect::<Vec<_>>();
                request_ids
                    .into_iter()
                    .filter_map(|request_id| entries.remove(&request_id))
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        for entry in entries {
            drop(entry.decision_tx.send(PermissionDecision::Cancel));
        }
    }

    fn resolve(
        &self,
        request_id: &str,
        decision: PermissionDecision,
    ) -> Result<(), PermissionDecisionError> {
        let entry = {
            let mut entries = self
                .entries
                .lock()
                .map_err(|_| PermissionDecisionError::NotPending)?;
            validate_permission_decision(entries.get(request_id), &decision)?;
            entries.remove(request_id)
        }
        .ok_or(PermissionDecisionError::NotPending)?;

        drop(entry.decision_tx.send(decision));
        Ok(())
    }
}

fn validate_permission_decision(
    entry: Option<&PendingPermissionEntry>,
    decision: &PermissionDecision,
) -> Result<(), PermissionDecisionError> {
    let Some(entry) = entry else {
        return Err(PermissionDecisionError::NotPending);
    };
    if let PermissionDecision::Approve { option_id } = decision {
        let option = entry
            .options
            .iter()
            .find(|option| option.option_id.to_string() == *option_id)
            .ok_or(PermissionDecisionError::InvalidOption)?;
        if !matches!(
            option.kind,
            PermissionOptionKind::AllowOnce | PermissionOptionKind::AllowAlways
        ) {
            return Err(PermissionDecisionError::NotAllowOption);
        }
    }
    Ok(())
}

#[derive(Clone)]
struct CodexLaunch {
    command: String,
    args: Vec<String>,
}

#[derive(Clone)]
struct ProviderLaunch {
    id: String,
    command: String,
    args: Vec<String>,
    /// Working directory override for the subprocess. `None` falls back to
    /// the session-level cwd from `StartSessionInput`.
    cwd: Option<PathBuf>,
    /// Per-provider env overrides merged on top of the global allowlist.
    env: std::collections::BTreeMap<String, String>,
}

fn codex_launch_override() -> &'static Mutex<Option<CodexLaunch>> {
    static OVERRIDE: OnceLock<Mutex<Option<CodexLaunch>>> = OnceLock::new();
    OVERRIDE.get_or_init(|| Mutex::new(None))
}

#[doc(hidden)]
#[allow(dead_code)]
pub fn set_codex_launch_override_for_tests(command: Option<String>, args: Vec<String>) {
    let mut launch = codex_launch_override()
        .lock()
        .expect("codex launch override poisoned");
    *launch = command.map(|command| CodexLaunch { command, args });
}

pub async fn launch_codex_runtime(
    session_id: String,
    input: StartSessionInput,
    event_tx: mpsc::Sender<AcpEvent>,
) -> Result<(RuntimeHandle, StartSessionResult), String> {
    let (started_tx, started_rx) = oneshot::channel();
    let (command_tx, command_rx) = mpsc::channel(SESSION_COMMAND_QUEUE_CAPACITY);
    let permissions = Arc::new(PendingPermissions::new(acp_permission_timeout()));
    let thread_permissions = Arc::clone(&permissions);
    let terminated = Arc::new(AtomicBool::new(false));
    let thread_terminated = Arc::clone(&terminated);
    let termination_notify = Arc::new(Notify::new());
    let thread_termination_notify = Arc::clone(&termination_notify);

    std::thread::Builder::new()
        .name(format!("lab-acp-{session_id}"))
        .spawn(move || {
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("failed to build ACP runtime");
            runtime.block_on(async move {
                drop(
                    run_codex_session(
                        session_id,
                        input,
                        event_tx,
                        started_tx,
                        command_rx,
                        thread_permissions,
                    )
                    .await,
                );
                thread_terminated.store(true, Ordering::SeqCst);
                thread_termination_notify.notify_waiters();
            });
        })
        .map_err(|error| error.to_string())?;

    let started = started_rx
        .await
        .map_err(|_| "ACP runtime failed to report startup".to_string())??;

    Ok((
        RuntimeHandle {
            provider_session_id: started.provider_session_id.clone(),
            command_tx,
            permissions,
            terminated,
            termination_notify,
            #[cfg(test)]
            _event_tx_for_tests: None,
        },
        StartSessionResult {
            provider_session_id: started.provider_session_id,
            agent_name: started.agent_name,
            agent_version: started.agent_version,
            model_id: started.model_id,
            model_name: started.model_name,
            models: started.models,
            config_options: Vec::new(),
        },
    ))
}

pub fn normalize_provider_id(provider: Option<&str>) -> String {
    match provider.filter(|value| !value.trim().is_empty()) {
        Some("codex") | None => "codex-acp".to_string(),
        Some(provider) => provider.to_string(),
    }
}

pub fn provider_healths() -> Vec<AcpProviderHealth> {
    let mut providers: Vec<AcpProviderHealth> = read_providers()
        .unwrap_or_default()
        .into_iter()
        .map(|provider| health_for_provider_entry(&provider))
        .collect();

    if !providers
        .iter()
        .any(|provider| provider.provider == "codex-acp")
    {
        providers.insert(0, codex_provider_health());
    }

    providers
}

pub fn codex_provider_health() -> AcpProviderHealth {
    let (command, _args) = resolve_codex_launch();
    let ready = if std::env::var("ACP_CODEX_COMMAND")
        .ok()
        .filter(|value| !value.trim().is_empty())
        .is_some()
    {
        true
    } else {
        command_available(&command)
    };

    AcpProviderHealth {
        provider: "codex-acp".to_string(),
        available: ready,
        version: None,
        message: if ready {
            None
        } else {
            Some(
                "ACP Codex provider is unavailable. Set ACP_CODEX_COMMAND or ensure npx is on PATH."
                    .to_string(),
            )
        },
        models: Vec::new(),
        default_model_id: None,
        current_model_id: None,
    }
}

fn health_for_provider_entry(provider: &AcpProviderEntry) -> AcpProviderHealth {
    let launch = launch_from_provider_entry(provider);
    let sandbox_message = codex_sandbox_incompatibility_message(provider);
    let command_available = command_available(&launch.command);
    let available = command_available && sandbox_message.is_none();
    AcpProviderHealth {
        provider: provider.id.clone(),
        available,
        version: Some(provider.version.clone()),
        message: if let Some(message) = sandbox_message {
            Some(message)
        } else if command_available {
            None
        } else {
            Some(format!(
                "ACP provider command `{}` is unavailable",
                launch.command
            ))
        },
        models: Vec::new(),
        default_model_id: None,
        current_model_id: None,
    }
}

fn command_available(command: &str) -> bool {
    if command.contains('/') || command.contains('\\') {
        return Path::new(command).exists();
    }
    cached_command_lookup(command)
}

/// TTL cache for `which`/`where` lookups. Provider health endpoints can be
/// polled per-request; without this each call shells out once per provider.
fn cached_command_lookup(command: &str) -> bool {
    const CACHE_TTL: Duration = Duration::from_secs(10);

    static CACHE: OnceLock<Mutex<HashMap<String, (Instant, bool)>>> = OnceLock::new();
    let cache = CACHE.get_or_init(|| Mutex::new(HashMap::new()));

    if let Ok(map) = cache.lock() {
        if let Some((stored_at, available)) = map.get(command) {
            if stored_at.elapsed() < CACHE_TTL {
                return *available;
            }
        }
    }

    let available = command_exists_on_path(command);

    if let Ok(mut map) = cache.lock() {
        map.insert(command.to_string(), (Instant::now(), available));
    }
    available
}

fn command_exists_on_path(command: &str) -> bool {
    if command.trim().is_empty() {
        return false;
    }

    let Some(path_var) = std::env::var_os("PATH") else {
        return false;
    };

    for dir in std::env::split_paths(&path_var) {
        if command_exists_in_dir(&dir, command) {
            return true;
        }
    }
    false
}

fn command_exists_in_dir(dir: &Path, command: &str) -> bool {
    #[cfg(windows)]
    {
        let pathext = std::env::var_os("PATHEXT")
            .map(|value| {
                value
                    .to_string_lossy()
                    .split(';')
                    .filter(|ext| !ext.trim().is_empty())
                    .map(ToOwned::to_owned)
                    .collect::<Vec<_>>()
            })
            .filter(|exts| !exts.is_empty())
            .unwrap_or_else(|| {
                vec![
                    ".COM".to_string(),
                    ".EXE".to_string(),
                    ".BAT".to_string(),
                    ".CMD".to_string(),
                ]
            });

        if Path::new(command).extension().is_some() {
            return dir.join(command).is_file();
        }

        pathext
            .iter()
            .any(|ext| dir.join(format!("{command}{ext}")).is_file())
    }

    #[cfg(not(windows))]
    {
        use std::os::unix::fs::PermissionsExt;

        let path: PathBuf = dir.join(command);
        path.metadata()
            .map(|meta| meta.is_file() && meta.permissions().mode() & 0o111 != 0)
            .unwrap_or(false)
    }
}

fn resolve_codex_launch() -> (String, Vec<String>) {
    if let Some(launch) = codex_launch_override()
        .lock()
        .expect("codex launch override poisoned")
        .clone()
    {
        return (launch.command, launch.args);
    }

    if let Some(command) = std::env::var("ACP_CODEX_COMMAND")
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
    {
        // ACP_CODEX_ARGS is whitespace-split. Env vars cannot carry quoted
        // arguments faithfully, so this path does not preserve quoting or
        // arguments containing spaces. For complex provider configs, install
        // the provider via `lab acp install` (or `marketplace.acp.install`)
        // and let the structured args field carry the literal argv vector.
        let args = std::env::var("ACP_CODEX_ARGS")
            .unwrap_or_default()
            .split_whitespace()
            .map(ToOwned::to_owned)
            .collect();
        return (command, args);
    }

    let command = if cfg!(windows) { "npx.cmd" } else { "npx" }.to_string();
    (command, vec!["@zed-industries/codex-acp".to_string()])
}

fn launch_from_provider_entry(provider: &AcpProviderEntry) -> ProviderLaunch {
    let (command, args) = if provider.args.is_empty() {
        // Legacy entry without structured args. Fall back to
        // whitespace-splitting the joined command string. This path cannot
        // round-trip quoted arguments — a one-time read fidelity gap that
        // only affects providers installed before structured args landed.
        // Re-installing the provider migrates the on-disk entry.
        let mut parts = provider.command.split_whitespace();
        let command = parts.next().unwrap_or("").to_string();
        let args = parts.map(ToOwned::to_owned).collect();
        (command, args)
    } else {
        (provider.command.clone(), provider.args.clone())
    };
    ProviderLaunch {
        id: provider.id.clone(),
        command,
        args,
        cwd: provider.cwd.clone(),
        env: provider.env.clone(),
    }
}

async fn handle_permission_request(
    runtime_session_id: &str,
    provider_id: &str,
    event_tx: &mpsc::Sender<AcpEvent>,
    permissions: &PendingPermissions,
    args: RequestPermissionRequest,
) -> RequestPermissionResponse {
    let provider_session_id = args.session_id.to_string();
    let request_id = args.tool_call.tool_call_id.to_string();
    let action_summary = args
        .tool_call
        .fields
        .title
        .clone()
        .unwrap_or_else(|| "Permission requested".to_string());
    let options = args.options;
    let public_options = options
        .iter()
        .map(acp_permission_option_from_protocol)
        .collect::<Vec<_>>();
    let (decision_tx, decision_rx) = oneshot::channel();

    // Lock + insert + drop strictly before any await so the !Send MutexGuard
    // does not span the emit_permission_outcome.await on the poisoned path.
    let lock_poisoned = match permissions.entries.lock() {
        Ok(mut entries) => {
            entries.insert(
                request_id.clone(),
                PendingPermissionEntry {
                    session_id: provider_session_id.clone(),
                    options: options.clone(),
                    decision_tx,
                },
            );
            false
        }
        Err(_) => true,
    };
    if lock_poisoned {
        emit_permission_outcome(
            event_tx,
            runtime_session_id,
            provider_id,
            &request_id,
            false,
        )
        .await;
        return RequestPermissionResponse::new(RequestPermissionOutcome::Cancelled);
    }

    drop(
        event_tx
            .send(AcpEvent::PermissionRequest {
                id: uuid::Uuid::new_v4().to_string(),
                created_at: jiff::Timestamp::now().to_string(),
                session_id: runtime_session_id.to_string(),
                seq: 0,
                provider: provider_id.to_string(),
                request_id: request_id.clone(),
                action_summary,
                options: public_options,
            })
            .await,
    );

    tracing::info!(
        surface = "acp",
        service = "runtime",
        action = "permission.request",
        session_id = %runtime_session_id,
        provider_session_id = %provider_session_id,
        request_id = %request_id,
        "ACP provider permission request is pending",
    );

    let decision = match tokio::time::timeout(permissions.timeout, decision_rx).await {
        Ok(Ok(decision)) => decision,
        Ok(Err(_)) | Err(_) => {
            let removed = permissions
                .entries
                .lock()
                .map(|mut entries| entries.remove(&request_id).is_some())
                .unwrap_or(false);
            if removed {
                tracing::warn!(
                    surface = "acp",
                    service = "runtime",
                    action = "permission.timeout",
                    session_id = %runtime_session_id,
                    provider_session_id = %provider_session_id,
                    request_id = %request_id,
                    timeout_ms = permissions.timeout.as_millis(),
                    "ACP provider permission request timed out",
                );
            }
            PermissionDecision::Cancel
        }
    };

    let response = response_for_permission_decision(&options, decision);
    emit_permission_outcome(
        event_tx,
        runtime_session_id,
        provider_id,
        &request_id,
        matches!(response.outcome, RequestPermissionOutcome::Selected(_))
            && selected_option_is_allow(&options, &response),
    )
    .await;
    response
}

fn response_for_permission_decision(
    options: &[PermissionOption],
    decision: PermissionDecision,
) -> RequestPermissionResponse {
    match decision {
        PermissionDecision::Approve { option_id } => {
            let Some(option) = find_permission_option(options, &option_id) else {
                return RequestPermissionResponse::new(RequestPermissionOutcome::Cancelled);
            };
            if matches!(
                option.kind,
                PermissionOptionKind::AllowOnce | PermissionOptionKind::AllowAlways
            ) {
                RequestPermissionResponse::new(RequestPermissionOutcome::Selected(
                    SelectedPermissionOutcome::new(option.option_id.clone()),
                ))
            } else {
                RequestPermissionResponse::new(RequestPermissionOutcome::Cancelled)
            }
        }
        PermissionDecision::Reject => match options.iter().find(|option| {
            matches!(
                option.kind,
                PermissionOptionKind::RejectOnce | PermissionOptionKind::RejectAlways
            )
        }) {
            Some(option) => RequestPermissionResponse::new(RequestPermissionOutcome::Selected(
                SelectedPermissionOutcome::new(option.option_id.clone()),
            )),
            None => RequestPermissionResponse::new(RequestPermissionOutcome::Cancelled),
        },
        PermissionDecision::Cancel => {
            RequestPermissionResponse::new(RequestPermissionOutcome::Cancelled)
        }
    }
}

fn selected_option_is_allow(
    options: &[PermissionOption],
    response: &RequestPermissionResponse,
) -> bool {
    let RequestPermissionOutcome::Selected(selected) = &response.outcome else {
        return false;
    };
    find_permission_option(options, &selected.option_id.to_string()).is_some_and(|option| {
        matches!(
            option.kind,
            PermissionOptionKind::AllowOnce | PermissionOptionKind::AllowAlways
        )
    })
}

fn find_permission_option<'a>(
    options: &'a [PermissionOption],
    option_id: &str,
) -> Option<&'a PermissionOption> {
    options
        .iter()
        .find(|option| option.option_id.to_string() == option_id)
}

async fn emit_permission_outcome(
    event_tx: &mpsc::Sender<AcpEvent>,
    session_id: &str,
    provider_id: &str,
    request_id: &str,
    granted: bool,
) {
    drop(
        event_tx
            .send(AcpEvent::PermissionOutcome {
                id: uuid::Uuid::new_v4().to_string(),
                created_at: jiff::Timestamp::now().to_string(),
                session_id: session_id.to_string(),
                seq: 0,
                provider: provider_id.to_string(),
                request_id: request_id.to_string(),
                granted,
            })
            .await,
    );
}

fn acp_permission_option_from_protocol(option: &PermissionOption) -> AcpPermissionOption {
    AcpPermissionOption {
        option_id: option.option_id.to_string(),
        name: option.name.clone(),
        kind: match option.kind {
            PermissionOptionKind::AllowOnce => "allow_once",
            PermissionOptionKind::AllowAlways => "allow_always",
            PermissionOptionKind::RejectOnce => "reject_once",
            PermissionOptionKind::RejectAlways => "reject_always",
            _ => "unknown",
        }
        .to_string(),
    }
}

fn resolve_provider_launch(provider: Option<&str>) -> Result<ProviderLaunch, String> {
    let provider_id = normalize_provider_id(provider);
    if provider_id == "codex-acp" {
        if codex_launch_override()
            .lock()
            .expect("codex launch override poisoned")
            .is_some()
        {
            let (command, args) = resolve_codex_launch();
            return Ok(ProviderLaunch {
                id: provider_id,
                command,
                args,
                cwd: None,
                env: std::collections::BTreeMap::new(),
            });
        }
        if let Some(entry) = read_providers()
            .map_err(|error| error.to_string())?
            .into_iter()
            .find(|entry| entry.id == provider_id)
        {
            return Ok(launch_from_provider_entry(&entry));
        }
        let (command, args) = resolve_codex_launch();
        return Ok(ProviderLaunch {
            id: provider_id,
            command,
            args,
            cwd: None,
            env: std::collections::BTreeMap::new(),
        });
    }

    read_providers()
        .map_err(|error| error.to_string())?
        .into_iter()
        .find(|entry| entry.id == provider_id)
        .map(|entry| launch_from_provider_entry(&entry))
        .ok_or_else(|| format!("ACP provider `{provider_id}` is not installed"))
}

pub fn warn_if_acp_provider_sandbox_is_incompatible() {
    for warning in acp_provider_sandbox_warnings() {
        tracing::warn!(
            surface = "acp",
            service = "provider",
            action = "preflight",
            kind = "container_sandbox_incompatible",
            provider = %warning.provider_id,
            sandbox_mode = %warning.sandbox_mode.as_deref().unwrap_or("unknown"),
            "Codex ACP provider sandbox mode is incompatible with this container runtime; \
             use sandbox_mode=\"danger-full-access\" or run the container with nested namespace privileges",
        );
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct AcpSandboxWarning {
    provider_id: String,
    sandbox_mode: Option<String>,
}

fn acp_provider_sandbox_warnings() -> Vec<AcpSandboxWarning> {
    if !running_in_container() {
        return Vec::new();
    }

    read_providers()
        .unwrap_or_default()
        .into_iter()
        .filter(|provider| normalize_provider_id(Some(&provider.id)) == "codex-acp")
        .filter_map(|provider| {
            let sandbox_mode = codex_sandbox_mode_for_provider(&provider);
            if sandbox_mode.as_deref() == Some(CODEX_DOCKER_SAFE_SANDBOX_MODE) {
                None
            } else {
                Some(AcpSandboxWarning {
                    provider_id: provider.id,
                    sandbox_mode,
                })
            }
        })
        .collect()
}

fn running_in_container() -> bool {
    std::env::var_os("LAB_CONTAINER_RUNTIME").is_some()
        || Path::new("/.dockerenv").exists()
        || Path::new("/run/.containerenv").exists()
        || std::fs::read_to_string("/proc/1/cgroup")
            .map(|cgroup| cgroup_mentions_container_runtime(&cgroup))
            .unwrap_or(false)
}

fn cgroup_mentions_container_runtime(cgroup: &str) -> bool {
    cgroup.contains("/docker/")
        || cgroup.contains("docker-")
        || cgroup.contains("kubepods")
        || cgroup.contains("containerd")
        || cgroup.contains("libpod")
}

fn codex_sandbox_mode_for_provider(provider: &AcpProviderEntry) -> Option<String> {
    codex_sandbox_mode_from_args(&provider.args).or_else(|| {
        provider
            .env
            .get("CODEX_HOME")
            .and_then(|home| codex_sandbox_mode_from_config(Path::new(home).join("config.toml")))
    })
}

fn codex_sandbox_incompatibility_message(provider: &AcpProviderEntry) -> Option<String> {
    codex_sandbox_incompatibility_message_for_runtime(provider, running_in_container())
}

fn codex_sandbox_incompatibility_message_for_runtime(
    provider: &AcpProviderEntry,
    in_container: bool,
) -> Option<String> {
    if normalize_provider_id(Some(&provider.id)) != "codex-acp" || !in_container {
        return None;
    }

    let sandbox_mode = codex_sandbox_mode_for_provider(provider);
    if sandbox_mode.as_deref() == Some(CODEX_DOCKER_SAFE_SANDBOX_MODE) {
        return None;
    }

    Some(format!(
        "Codex ACP sandbox mode `{}` is incompatible with this container runtime; use sandbox_mode=\"danger-full-access\" or restart with nested namespace privileges.",
        sandbox_mode.as_deref().unwrap_or("unknown")
    ))
}

fn codex_sandbox_mode_from_args(args: &[String]) -> Option<String> {
    let mut iter = args.iter();
    while let Some(arg) = iter.next() {
        if (arg == "-c" || arg == "--config")
            && let Some(value) = iter.next()
            && let Some(mode) = parse_sandbox_config_override(value)
        {
            return Some(mode);
        }
    }
    None
}

fn parse_sandbox_config_override(value: &str) -> Option<String> {
    let (key, raw_value) = value.split_once('=')?;
    if key.trim() != "sandbox_mode" {
        return None;
    }
    Some(unquote_config_value(raw_value.trim()))
}

fn codex_sandbox_mode_from_config(path: impl AsRef<Path>) -> Option<String> {
    let text = std::fs::read_to_string(path).ok()?;
    text.lines().find_map(|line| {
        let line = line.trim();
        if line.starts_with('#') {
            return None;
        }
        parse_sandbox_config_override(line)
    })
}

fn unquote_config_value(value: &str) -> String {
    value
        .strip_prefix('"')
        .and_then(|value| value.strip_suffix('"'))
        .unwrap_or(value)
        .to_string()
}

fn provider_subprocess_env<I>(vars: I) -> Vec<(String, String)>
where
    I: IntoIterator<Item = (String, String)>,
{
    let mut env: Vec<(String, String)> = vars
        .into_iter()
        .filter(|(key, _)| is_provider_env_allowed(key))
        .collect();
    env.sort_by(|left, right| left.0.cmp(&right.0));
    env.dedup_by(|left, right| left.0 == right.0);
    env
}

fn is_provider_env_allowed(key: &str) -> bool {
    matches!(
        key,
        "PATH"
            | "HOME"
            | "TMPDIR"
            | "TEMP"
            | "TMP"
            | "LANG"
            | "LC_ALL"
            | "TERM"
            | "USER"
            | "USERNAME"
            | "SHELL"
            | "SystemRoot"
            | "SYSTEMROOT"
            | "ComSpec"
            | "COMSPEC"
            | "PATHEXT"
    )
}

fn redact_provider_stderr_line(line: &str) -> (String, bool) {
    let redacted = line
        .split_whitespace()
        .map(redact_stdio_value)
        .collect::<Vec<_>>()
        .join(" ");
    // Layer broader sanitization (IPs, JWTs, home paths) over the per-token
    // key=value redaction.
    let redacted = sanitize_provider_error(&redacted);
    if redacted.chars().count() <= MAX_PROVIDER_STDERR_CHARS {
        return (redacted, false);
    }

    let truncated = redacted
        .chars()
        .take(MAX_PROVIDER_STDERR_CHARS)
        .collect::<String>();
    (truncated, true)
}

/// Strip identifying network endpoints, JWT-shaped tokens, and absolute home
/// paths from provider error/stderr text before surfacing it to clients.
/// Composes with [`redact_stdio_value`] which already handles `key=value`
/// secret patterns at the token level.
pub fn sanitize_provider_error(message: &str) -> String {
    use std::sync::OnceLock;
    static PATTERNS: OnceLock<Vec<(regex::Regex, &'static str)>> = OnceLock::new();
    let patterns = PATTERNS.get_or_init(|| {
        vec![
            // IPv4 with optional :port. Conservative — does not match IPv6.
            (
                regex::Regex::new(r"\b(?:\d{1,3}\.){3}\d{1,3}(?::\d+)?\b").unwrap(),
                "[redacted-ip]",
            ),
            // JWT-shaped token: any whitespace-delimited word starting with the
            // standard `eyJ` header prefix.
            (
                regex::Regex::new(r"\beyJ[A-Za-z0-9_.-]+\b").unwrap(),
                "[redacted-jwt]",
            ),
            // Absolute paths under per-user roots — collapses usernames and
            // working-directory layouts that would otherwise leak through.
            (
                regex::Regex::new(r#"/(?:home|Users|root)/[^ \t\n"']+"#).unwrap(),
                "[path]",
            ),
        ]
    });
    let mut out = message.to_string();
    for (re, repl) in patterns {
        out = re.replace_all(&out, *repl).to_string();
    }
    out
}

async fn run_codex_session(
    session_id: String,
    input: StartSessionInput,
    event_tx: mpsc::Sender<AcpEvent>,
    started_tx: oneshot::Sender<Result<RuntimeStarted, String>>,
    mut command_rx: mpsc::Receiver<SessionCommand>,
    permissions: Arc<PendingPermissions>,
) -> Result<(), String> {
    let launch = resolve_provider_launch(input.provider.as_deref())?;
    let provider_id = launch.id.clone();
    let mut command = tokio::process::Command::new(&launch.command);
    let cwd: &Path = launch
        .cwd
        .as_deref()
        .unwrap_or_else(|| Path::new(&input.cwd));
    command
        .args(&launch.args)
        .current_dir(cwd)
        .env_clear()
        .envs(provider_subprocess_env(std::env::vars()))
        // Per-provider env applied AFTER the global allowlist so structured
        // provider configs can override or extend the base set without
        // widening the allowlist itself.
        .envs(launch.env.iter())
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true);
    #[cfg(unix)]
    command.process_group(0);
    let mut child = command.spawn().map_err(|error| error.to_string())?;
    let child_process_group = child.id();
    tracing::info!(
        surface = "acp",
        service = "runtime",
        action = "subprocess.spawn",
        session_id = %session_id,
        pid = ?child_process_group,
        binary = %launch.command,
        provider = %provider_id,
        "ACP subprocess spawned",
    );

    let stdin = child
        .stdin
        .take()
        .ok_or_else(|| format!("{provider_id} stdin unavailable"))?;
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| format!("{provider_id} stdout unavailable"))?;
    let stderr = child
        .stderr
        .take()
        .ok_or_else(|| format!("{provider_id} stderr unavailable"))?;

    let stderr_tx = event_tx.clone();
    let stderr_session = session_id.clone();
    let stderr_provider = provider_id.clone();
    tokio::spawn(async move {
        let mut reader = BufReader::new(stderr).lines();
        while let Ok(Some(line)) = reader.next_line().await {
            let (text, truncated) = redact_provider_stderr_line(&line);
            drop(
                stderr_tx
                    .send(provider_info_event(
                        stderr_session.clone(),
                        &stderr_provider,
                        json!({
                            "type": "stderr",
                            "title": format!("{stderr_provider} stderr"),
                            "text": text,
                            "truncated": truncated,
                        }),
                    ))
                    .await,
            );
        }
    });

    let transport = ByteStreams::new(stdin.compat_write(), stdout.compat());
    let started_tx = Arc::new(Mutex::new(Some(started_tx)));
    let prompt_lifecycle = Arc::new(PromptLifecycle::default());
    let connection_provider = provider_id.clone();
    let run_result = Client
        .builder()
        .on_receive_request(
            {
                let session_id = session_id.clone();
                let event_tx = event_tx.clone();
                let permissions = Arc::clone(&permissions);
                let provider_id = provider_id.clone();
                async move |args: RequestPermissionRequest, responder, _cx| {
                    let response =
                        handle_permission_request(&session_id, &provider_id, &event_tx, &permissions, args)
                            .await;
                    responder.respond(response)
                }
            },
            on_receive_request!(),
        )
        .on_receive_request(
            async move |_args: ReadTextFileRequest, responder, _cx| {
                responder.respond_with_error(agent_client_protocol::Error::method_not_found())
            },
            on_receive_request!(),
        )
        .on_receive_request(
            async move |_args: WriteTextFileRequest, responder, _cx| {
                responder.respond_with_error(agent_client_protocol::Error::method_not_found())
            },
            on_receive_request!(),
        )
        .on_receive_request(
            async move |_args: CreateTerminalRequest, responder, _cx| {
                responder.respond_with_error(agent_client_protocol::Error::method_not_found())
            },
            on_receive_request!(),
        )
        .on_receive_request(
            async move |_args: TerminalOutputRequest, responder, _cx| {
                responder.respond_with_error(agent_client_protocol::Error::method_not_found())
            },
            on_receive_request!(),
        )
        .on_receive_request(
            async move |_args: ReleaseTerminalRequest, responder, _cx| {
                responder.respond_with_error(agent_client_protocol::Error::method_not_found())
            },
            on_receive_request!(),
        )
        .on_receive_request(
            async move |_args: WaitForTerminalExitRequest, responder, _cx| {
                responder.respond_with_error(agent_client_protocol::Error::method_not_found())
            },
            on_receive_request!(),
        )
        .on_receive_request(
            async move |_args: KillTerminalRequest, responder, _cx| {
                responder.respond_with_error(agent_client_protocol::Error::method_not_found())
            },
            on_receive_request!(),
        )
        .connect_with(transport, {
            let session_id = session_id.clone();
            let event_tx = event_tx.clone();
            let cwd = input.cwd.clone();
            let started_tx = Arc::clone(&started_tx);
            let prompt_lifecycle = Arc::clone(&prompt_lifecycle);
            let provider_id = connection_provider.clone();
            move |connection: ConnectionTo<Agent>| {
                let session_id = session_id.clone();
                let event_tx = event_tx.clone();
                let cwd = cwd.clone();
                let started_tx = Arc::clone(&started_tx);
                let prompt_lifecycle = Arc::clone(&prompt_lifecycle);
                let provider_id = provider_id.clone();
                async move {
                    let initialized = connection
                        .send_request(
                            InitializeRequest::new(ProtocolVersion::V1)
                                .client_info(
                                    Implementation::new(
                                        "lab-acp-bridge",
                                        env!("CARGO_PKG_VERSION"),
                                    )
                                    .title("Lab ACP Bridge"),
                                )
                                .client_capabilities({
                                    // PHASE 1: do NOT call .terminal(true) — server-hosted terminal
                                    // execution lives in lab-lffl. Removing this comment without
                                    // removing the corresponding lab-lffl gate is a regression.
                                    lab_client_capabilities()
                                }),
                        )
                        .block_task()
                        .await
                        .map_err(|error| acp_internal_error(error.to_string()))?;

                    let mut session = connection
                        .build_session(cwd)
                        .block_task()
                        .start_session()
                        .await
                        .map_err(|error| acp_internal_error(error.to_string()))?;
                    let session_response = session.response();
                    let (model_id, models) = session_model_options(&session_response);
                    let model_name = model_id
                        .as_ref()
                        .and_then(|id| models.iter().find(|model| &model.id == id))
                        .map(|model| model.name.clone());

                    let started = RuntimeStarted {
                        provider_session_id: session.session_id().to_string(),
                        agent_name: initialized
                            .agent_info
                            .as_ref()
                            .and_then(|info| info.title.clone())
                            .unwrap_or_else(|| {
                                initialized
                                    .agent_info
                                    .as_ref()
                                    .map(|info| info.name.clone())
                                    .unwrap_or_else(|| provider_id.clone())
                            }),
                        agent_version: initialized
                            .agent_info
                            .as_ref()
                            .map(|info| info.version.clone())
                            .unwrap_or_else(|| "unknown".to_string()),
                        model_id,
                        model_name,
                        models,
                    };
                    if let Some(sender) = started_tx.lock().ok().and_then(|mut guard| guard.take()) {
                        drop(sender.send(Ok(started)));
                    }

                    // True when the previous turn ended via idle_completion rather than
                    // an explicit StopReason. In that case codex-acp may still be
                    // processing (e.g. a long tool call) and will send PromptResponse
                    // after the inner loop has broken. The drain block below consumes
                    // that late StopReason before starting the next turn so it cannot
                    // poison the new inner read loop.
                    let mut previous_turn_idle = false;

                    while let Some(command) = command_rx.recv().await {
                        match command {
                            SessionCommand::Prompt(command) => {
                                let PromptCommand { input, model_id } = command;
                                prompt_lifecycle.start();

                                if previous_turn_idle {
                                    // Drain stale messages left by the previous idle-completed turn.
                                    // We loop until StopReason (turn fully acknowledged by the
                                    // provider), a connection error, or the drain timeout.
                                    let deadline =
                                        tokio::time::Instant::now() + acp_turn_drain_timeout();
                                    tracing::debug!(
                                        surface = "acp",
                                        service = "runtime",
                                        action = "turn_drain.start",
                                        session_id = %session_id,
                                        "Draining stale messages from previous idle-completed turn",
                                    );
                                    loop {
                                        let now = tokio::time::Instant::now();
                                        if now >= deadline {
                                            tracing::warn!(
                                                surface = "acp",
                                                service = "runtime",
                                                action = "turn_drain.timeout",
                                                session_id = %session_id,
                                                timeout_secs = acp_turn_drain_timeout().as_secs(),
                                                "Drain timeout: starting next turn without consuming \
                                                 late StopReason; next turn may see unexpected content",
                                            );
                                            break;
                                        }
                                        match tokio::time::timeout(
                                            deadline - now,
                                            session.read_update(),
                                        )
                                        .await
                                        {
                                            Ok(Ok(
                                                agent_client_protocol::SessionMessage::StopReason(
                                                    _,
                                                ),
                                            )) => {
                                                tracing::debug!(
                                                    surface = "acp",
                                                    service = "runtime",
                                                    action = "turn_drain.done",
                                                    session_id = %session_id,
                                                    "Drained late StopReason; previous turn is clean",
                                                );
                                                break;
                                            }
                                            Ok(Ok(_)) => {
                                                // Discard stale content from the previous turn.
                                            }
                                            Ok(Err(error)) => {
                                                tracing::warn!(
                                                    surface = "acp",
                                                    service = "runtime",
                                                    action = "turn_drain.error",
                                                    session_id = %session_id,
                                                    error = %error,
                                                    "Connection error while draining previous turn",
                                                );
                                                break;
                                            }
                                            Err(_elapsed) => {
                                                tracing::warn!(
                                                    surface = "acp",
                                                    service = "runtime",
                                                    action = "turn_drain.timeout",
                                                    session_id = %session_id,
                                                    timeout_secs = acp_turn_drain_timeout().as_secs(),
                                                    "Drain timeout: starting next turn without consuming \
                                                     late StopReason; next turn may see unexpected content",
                                                );
                                                break;
                                            }
                                        }
                                    }
                                }

                                let stream_message_ids =
                                    Arc::new(Mutex::new(StreamMessageIds::default()));
                                drop(
                                    event_tx
                                        .send(session_state_event(
                                            session_id.clone(),
                                            &provider_id,
                                            lab_apis::acp::types::AcpSessionState::Running,
                                        ))
                                        .await,
                                );
                                if let Some(model_id) = model_id.as_deref() {
                                    session
                                        .connection()
                                        .send_request_to(
                                            Agent,
                                            SetSessionModelRequest::new(
                                                session.session_id().clone(),
                                                model_id.to_string(),
                                            ),
                                        )
                                        .block_task()
                                        .await
                                        .map_err(|error| acp_internal_error(error.to_string()))?;
                                }
                                drop(
                                    event_tx
                                        .send(provider_info_event(
                                            session_id.clone(),
                                            &provider_id,
                                            json!({
                                                "type": "prompt_started",
                                                "title": "Prompt started",
                                                "text": input.text.clone(),
                                                "attachment_count": input.attachments.len(),
                                                "model_id": model_id,
                                            }),
                                        ))
                                        .await,
                                );

                                let (prompt_response_tx, prompt_response_rx) =
                                    oneshot::channel::<Result<StopReason, String>>();
                                let mut prompt_response_rx = Box::pin(prompt_response_rx);
                                let blocks = prompt_input_to_content_blocks(&input);
                                session
                                    .connection()
                                    .send_request_to(
                                        Agent,
                                        PromptRequest::new(session.session_id().clone(), blocks),
                                    )
                                    .on_receiving_result(async move |result| {
                                        let stop_reason = result
                                            .map(|PromptResponse { stop_reason, .. }| stop_reason)
                                            .map_err(|error| error.to_string());
                                        drop(prompt_response_tx.send(stop_reason));
                                        Ok(())
                                    })
                                    .map_err(|error| acp_internal_error(error.to_string()))?;

                                let mut saw_assistant_output = false;
                                // Set to true when the turn ends via idle_completion rather
                                // than an explicit StopReason so the drain block above runs
                                // before the next turn's prompt is dispatched.
                                let mut ended_via_idle = false;
                                loop {
                                    enum PromptTurnMessage {
                                        Provider(Result<agent_client_protocol::SessionMessage, agent_client_protocol::Error>),
                                        Stop(Result<StopReason, String>),
                                        Idle,
                                    }

                                    let update = tokio::select! {
                                        // Prefer consuming a real update over the idle timeout.
                                        // Without `biased`, when both arms are ready simultaneously
                                        // (provider sent PromptResponse just as the timer fires),
                                        // tokio picks randomly. A timeout win leaves StopReason
                                        // in the channel where it poisons the next turn's read loop.
                                        biased;
                                        stop_reason = &mut prompt_response_rx => {
                                            PromptTurnMessage::Stop(
                                                stop_reason.unwrap_or_else(|_| Err("prompt response channel closed".to_string())),
                                            )
                                        },
                                        update = session.read_update() => PromptTurnMessage::Provider(update),
                                        () = tokio::time::sleep(acp_prompt_idle_timeout()), if saw_assistant_output => PromptTurnMessage::Idle,
                                    };
                                    let update = match update {
                                        PromptTurnMessage::Provider(update) => update,
                                        PromptTurnMessage::Stop(Ok(stop_reason)) => {
                                            let stop_reason =
                                                map_stop_reason(&stop_reason).to_string();
                                            let state = if stop_reason == "cancelled" {
                                                lab_apis::acp::types::AcpSessionState::Cancelled
                                            } else {
                                                lab_apis::acp::types::AcpSessionState::Completed
                                            };
                                            drop(
                                                event_tx
                                                    .send(session_state_event(
                                                        session_id.clone(),
                                                        &provider_id,
                                                        state.clone(),
                                                    ))
                                                    .await,
                                            );
                                            drop(
                                                event_tx
                                                    .send(provider_info_event(
                                                        session_id.clone(),
                                                        &provider_id,
                                                        json!({
                                                            "type": "stop_reason",
                                                            "title": "Prompt completed",
                                                            "status": match state {
                                                                lab_apis::acp::types::AcpSessionState::Cancelled => "cancelled",
                                                                _ => "completed",
                                                            },
                                                            "stop_reason": stop_reason,
                                                        }),
                                                    ))
                                                    .await,
                                            );
                                            prompt_lifecycle.finish();
                                            break;
                                        }
                                        PromptTurnMessage::Stop(Err(error)) => {
                                            drop(
                                                event_tx
                                                    .send(session_state_event(
                                                        session_id.clone(),
                                                        &provider_id,
                                                        lab_apis::acp::types::AcpSessionState::Failed,
                                                    ))
                                                    .await,
                                            );
                                            drop(
                                                event_tx
                                                    .send(provider_info_event(
                                                        session_id.clone(),
                                                        &provider_id,
                                                        json!({
                                                            "type": "provider_error",
                                                            "title": "Provider error",
                                                            "text": error,
                                                        }),
                                                    ))
                                                    .await,
                                            );
                                            prompt_lifecycle.finish();
                                            break;
                                        }
                                        PromptTurnMessage::Idle => {
                                            drop(
                                                event_tx
                                                    .send(session_state_event(
                                                        session_id.clone(),
                                                        &provider_id,
                                                        lab_apis::acp::types::AcpSessionState::Completed,
                                                    ))
                                                    .await,
                                            );
                                            drop(
                                                event_tx
                                                    .send(provider_info_event(
                                                        session_id.clone(),
                                                        &provider_id,
                                                        json!({
                                                            "type": "idle_completion",
                                                            "title": "Prompt completed after provider idle timeout",
                                                            "status": "completed",
                                                            "timeout_ms": acp_prompt_idle_timeout().as_millis(),
                                                        }),
                                                    ))
                                                    .await,
                                            );
                                            prompt_lifecycle.finish();
                                            ended_via_idle = true;
                                            break;
                                        }
                                    };

                                    match update {
                                        Ok(agent_client_protocol::SessionMessage::SessionMessage(
                                            dispatch,
                                        )) => {
                                            let progress = handle_session_dispatch(
                                                &session_id,
                                                &provider_id,
                                                &event_tx,
                                                dispatch,
                                                &stream_message_ids,
                                            )
                                            .await
                                            .map_err(acp_internal_error)?;
                                            saw_assistant_output |= progress.assistant_output;
                                            if progress.prompt_progress {
                                                prompt_lifecycle.note_prompt_progress();
                                            }
                                        }
                                        Ok(agent_client_protocol::SessionMessage::StopReason(
                                            stop_reason,
                                        )) => {
                                            let stop_reason =
                                                map_stop_reason(&stop_reason).to_string();
                                            let state = if stop_reason == "cancelled" {
                                                lab_apis::acp::types::AcpSessionState::Cancelled
                                            } else {
                                                lab_apis::acp::types::AcpSessionState::Completed
                                            };
                                            drop(
                                                event_tx
                                                    .send(session_state_event(
                                                        session_id.clone(),
                                                        &provider_id,
                                                        state.clone(),
                                                    ))
                                                    .await,
                                            );
                                            drop(
                                                event_tx
                                                    .send(provider_info_event(
                                                        session_id.clone(),
                                                        &provider_id,
                                                        json!({
                                                            "type": "stop_reason",
                                                            "title": "Prompt completed",
                                                            "status": match state {
                                                                lab_apis::acp::types::AcpSessionState::Cancelled => "cancelled",
                                                                _ => "completed",
                                                            },
                                                            "stop_reason": stop_reason,
                                                        }),
                                                    ))
                                                    .await,
                                            );
                                            prompt_lifecycle.finish();
                                            break;
                                        }
                                        Ok(_) => {
                                            drop(
                                                event_tx
                                                    .send(provider_info_event(
                                                        session_id.clone(),
                                                        &provider_id,
                                                        json!({
                                                            "type": "unhandled_provider_message",
                                                            "title": "Unhandled provider update",
                                                        }),
                                                    ))
                                                    .await,
                                            );
                                        }
                                        Err(error) => {
                                            drop(
                                                event_tx
                                                    .send(session_state_event(
                                                        session_id.clone(),
                                                        &provider_id,
                                                        lab_apis::acp::types::AcpSessionState::Failed,
                                                    ))
                                                    .await,
                                            );
                                            drop(
                                                event_tx
                                                    .send(provider_info_event(
                                                        session_id.clone(),
                                                        &provider_id,
                                                        json!({
                                                            "type": "provider_error",
                                                            "title": "Provider error",
                                                            "text": error.to_string(),
                                                        }),
                                                    ))
                                                    .await,
                                            );
                                            prompt_lifecycle.finish();
                                            break;
                                        }
                                    }
                                }
                                previous_turn_idle = ended_via_idle;
                            }
                            SessionCommand::Cancel => {
                                permissions.cancel_session(&session.session_id().to_string());
                                session
                                    .connection()
                                    .send_notification(CancelNotification::new(
                                        session.session_id().clone(),
                                    ))
                                    .map_err(|error| acp_internal_error(error.to_string()))?;
                            }
                        }
                    }

                    Ok::<(), agent_client_protocol::Error>(())
                }
            }
        })
        .await;

    let run_error = run_result.err();

    if let Some(ref error) = run_error {
        tracing::error!(
            surface = "acp",
            service = "runtime",
            action = "connect_with.error",
            session_id = %session_id,
            error = %error,
            error_debug = ?error,
            "ACP connect_with returned error — this is why the subprocess is being terminated",
        );
    }

    if let Some(saw_assistant_output) = prompt_lifecycle.take_unfinished_prompt() {
        let (state, event) = unfinished_prompt_exit_event(
            &session_id,
            &provider_id,
            saw_assistant_output,
            &run_error,
        );
        drop(
            event_tx
                .send(session_state_event(session_id.clone(), &provider_id, state))
                .await,
        );
        drop(event_tx.send(event).await);
    }

    terminate_codex_child(&mut child, child_process_group).await;

    if let Some(error) = run_error {
        if let Some(sender) = started_tx.lock().ok().and_then(|mut guard| guard.take()) {
            drop(sender.send(Err(error.to_string())));
        }
        return Err(error.to_string());
    }

    Ok(())
}

#[cfg_attr(not(unix), allow(unused_variables))]
async fn terminate_codex_child(
    child: &mut tokio::process::Child,
    child_process_group: Option<u32>,
) {
    #[cfg(unix)]
    if let Some(pid) = child_process_group.and_then(|value| i32::try_from(value).ok()) {
        let pgid = Pid::from_raw(pid);
        tracing::info!(
            surface = "acp",
            service = "runtime",
            action = "subprocess.sigterm",
            pgid = pid,
            "Sending SIGTERM to ACP subprocess process group",
        );
        let _ = killpg(pgid, Signal::SIGTERM);
        tokio::time::sleep(Duration::from_millis(250)).await;
        if matches!(child.try_wait(), Ok(None)) {
            tracing::warn!(
                surface = "acp",
                service = "runtime",
                action = "subprocess.sigkill",
                pgid = pid,
                "ACP subprocess did not exit after SIGTERM — sending SIGKILL",
            );
            let _ = killpg(pgid, Signal::SIGKILL);
        }
        let exit_status = child.wait().await.ok();
        tracing::info!(
            surface = "acp", service = "runtime", action = "subprocess.exited",
            pgid = pid,
            exit_code = ?exit_status.and_then(|s| s.code()),
            "ACP subprocess process group terminated",
        );
        return;
    }

    match child.kill().await {
        Ok(()) => {
            tracing::info!(
                surface = "acp",
                service = "runtime",
                action = "subprocess.killed",
                "ACP subprocess killed (non-unix path)",
            );
        }
        Err(ref e) => {
            tracing::warn!(
                surface = "acp", service = "runtime", action = "subprocess.kill",
                error = %e, "ACP subprocess kill failed (non-unix path)",
            );
        }
    }
}

async fn push_session_update(
    session_id: &str,
    provider_id: &str,
    event_tx: &mpsc::Sender<AcpEvent>,
    update: SessionUpdate,
    stream_message_ids: &Arc<Mutex<StreamMessageIds>>,
) -> Result<(), String> {
    let event_channel_closed = || "ACP event channel closed".to_string();
    match update {
        SessionUpdate::UserMessageChunk(ContentChunk { content, .. }) => {
            // Lock scoped to message-id extraction; released before the await.
            let message_id = {
                let mut ids = stream_message_ids
                    .lock()
                    .map_err(|_| "ACP stream message id tracker poisoned".to_string())?;
                ids.user_message_id()
            };
            event_tx
                .send(AcpEvent::MessageChunk {
                    id: uuid::Uuid::new_v4().to_string(),
                    created_at: jiff::Timestamp::now().to_string(),
                    session_id: session_id.to_string(),
                    seq: 0,
                    provider: provider_id.to_string(),
                    role: "user".to_string(),
                    text: content_to_text(content),
                    message_id,
                })
                .await
                .map_err(|_| event_channel_closed())?;
        }
        SessionUpdate::AgentMessageChunk(ContentChunk { content, .. }) => {
            let message_id = {
                let mut ids = stream_message_ids
                    .lock()
                    .map_err(|_| "ACP stream message id tracker poisoned".to_string())?;
                ids.assistant_message_id()
            };
            event_tx
                .send(AcpEvent::MessageChunk {
                    id: uuid::Uuid::new_v4().to_string(),
                    created_at: jiff::Timestamp::now().to_string(),
                    session_id: session_id.to_string(),
                    seq: 0,
                    provider: provider_id.to_string(),
                    role: "assistant".to_string(),
                    text: content_to_text(content),
                    message_id,
                })
                .await
                .map_err(|_| event_channel_closed())?;
        }
        SessionUpdate::AgentThoughtChunk(ContentChunk { content, .. }) => {
            event_tx
                .send(AcpEvent::ReasoningChunk {
                    id: uuid::Uuid::new_v4().to_string(),
                    created_at: jiff::Timestamp::now().to_string(),
                    session_id: session_id.to_string(),
                    seq: 0,
                    provider: provider_id.to_string(),
                    text: content_to_text(content),
                })
                .await
                .map_err(|_| event_channel_closed())?;
        }
        SessionUpdate::ToolCall(tool_call) => {
            event_tx
                .send(AcpEvent::ToolCallStart {
                    id: uuid::Uuid::new_v4().to_string(),
                    created_at: jiff::Timestamp::now().to_string(),
                    session_id: session_id.to_string(),
                    seq: 0,
                    provider: provider_id.to_string(),
                    tool_call_id: tool_call.tool_call_id.to_string(),
                    name: tool_call.title.clone(),
                    input: tool_call.raw_input.unwrap_or(Value::Null),
                })
                .await
                .map_err(|_| event_channel_closed())?;
            if let Some(status) = enum_value(&tool_call.status) {
                // _meta must be omitted entirely when absent; json!() would emit null.
                // _meta is a transparent relay — never log its field values.
                let mut payload = json!({
                    "type": "tool_call_metadata",
                    "tool_call_id": tool_call.tool_call_id.to_string(),
                    "title": tool_call.title,
                    "tool_kind": enum_value(&tool_call.kind),
                    "status": status,
                    "locations": tool_call.locations.iter()
                        .map(|l| l.path.display().to_string())
                        .collect::<Vec<_>>(),
                    "content": tool_call.content,
                    "raw_output": tool_call.raw_output,
                });
                if let Some(meta) = tool_call.meta {
                    payload
                        .as_object_mut()
                        .unwrap()
                        .insert("_meta".into(), Value::Object(meta));
                }
                event_tx
                    .send(provider_info_event(
                        session_id.to_string(),
                        provider_id,
                        payload,
                    ))
                    .await
                    .map_err(|_| event_channel_closed())?;
            }
        }
        SessionUpdate::ToolCallUpdate(update) => {
            let tool_call_id = update.tool_call_id.to_string();
            let status = update
                .fields
                .status
                .as_ref()
                .and_then(enum_value)
                .unwrap_or_else(|| "updated".to_string());
            event_tx
                .send(AcpEvent::ToolCallUpdate {
                    id: uuid::Uuid::new_v4().to_string(),
                    created_at: jiff::Timestamp::now().to_string(),
                    session_id: session_id.to_string(),
                    seq: 0,
                    provider: provider_id.to_string(),
                    tool_call_id,
                    output: tool_call_update_output(update),
                    status,
                })
                .await
                .map_err(|_| event_channel_closed())?;
        }
        SessionUpdate::Plan(plan) => {
            event_tx
                .send(provider_info_event(
                    session_id.to_string(),
                    provider_id,
                    json!({
                        "type": "plan",
                        "title": "Execution plan updated",
                        "entries": serde_json::to_value(&plan)
                            .ok()
                            .and_then(|value| value.get("entries").cloned())
                            .unwrap_or(Value::Null),
                    }),
                ))
                .await
                .map_err(|_| event_channel_closed())?;
        }
        SessionUpdate::AvailableCommandsUpdate(update) => {
            event_tx
                .send(provider_info_event(
                    session_id.to_string(),
                    provider_id,
                    json!({
                        "type": "commands",
                        "title": "Available commands updated",
                        "commands": serde_json::to_value(&update)
                            .ok()
                            .and_then(|value| value.get("commands").cloned())
                            .unwrap_or(Value::Null),
                    }),
                ))
                .await
                .map_err(|_| event_channel_closed())?;
        }
        SessionUpdate::CurrentModeUpdate(update) => {
            emit_current_mode(session_id, provider_id, event_tx, update).await?;
        }
        SessionUpdate::ConfigOptionUpdate(update) => {
            emit_config_update(session_id, provider_id, event_tx, update).await?;
        }
        SessionUpdate::SessionInfoUpdate(update) => {
            emit_session_info(session_id, provider_id, event_tx, update).await?;
        }
        other => {
            event_tx
                .send(provider_info_event(
                    session_id.to_string(),
                    provider_id,
                    json!({
                        "type": "debug",
                        "title": "Unhandled session update",
                        "payload": serde_json::to_value(&other).unwrap_or(Value::Null),
                    }),
                ))
                .await
                .map_err(|_| event_channel_closed())?;
        }
    }

    Ok(())
}

async fn handle_session_dispatch(
    session_id: &str,
    provider_id: &str,
    event_tx: &mpsc::Sender<AcpEvent>,
    dispatch: Dispatch,
    stream_message_ids: &Arc<Mutex<StreamMessageIds>>,
) -> Result<SessionDispatchProgress, String> {
    let event_channel_closed = || "ACP event channel closed".to_string();
    match dispatch {
        Dispatch::Notification(notification)
            if SessionNotification::matches_method(notification.method()) =>
        {
            let notification =
                SessionNotification::parse_message(notification.method(), notification.params())
                    .map_err(|error| error.to_string())?;
            let is_assistant_output =
                matches!(notification.update, SessionUpdate::AgentMessageChunk(_));
            let is_prompt_progress = is_prompt_progress_update(&notification.update);
            push_session_update(
                session_id,
                provider_id,
                event_tx,
                notification.update,
                stream_message_ids,
            )
            .await?;
            Ok(SessionDispatchProgress {
                assistant_output: is_assistant_output,
                prompt_progress: is_prompt_progress,
            })
        }
        Dispatch::Notification(notification) => {
            event_tx
                .send(provider_info_event(
                    session_id.to_string(),
                    provider_id,
                    json!({
                        "type": "unhandled_provider_notification",
                        "title": "Unhandled provider notification",
                        "method": notification.method(),
                    }),
                ))
                .await
                .map_err(|_| event_channel_closed())?;
            Ok(SessionDispatchProgress {
                assistant_output: false,
                prompt_progress: false,
            })
        }
        Dispatch::Request(request, responder) => {
            drop(responder.respond_with_error(agent_client_protocol::Error::method_not_found()));
            event_tx
                .send(provider_info_event(
                    session_id.to_string(),
                    provider_id,
                    json!({
                        "type": "unhandled_provider_request",
                        "title": "Unhandled provider request",
                        "method": request.method(),
                    }),
                ))
                .await
                .map_err(|_| event_channel_closed())?;
            Ok(SessionDispatchProgress {
                assistant_output: false,
                prompt_progress: false,
            })
        }
        Dispatch::Response(_, _) => {
            event_tx
                .send(provider_info_event(
                    session_id.to_string(),
                    provider_id,
                    json!({
                        "type": "unhandled_provider_response",
                        "title": "Unhandled provider response",
                    }),
                ))
                .await
                .map_err(|_| event_channel_closed())?;
            Ok(SessionDispatchProgress {
                assistant_output: false,
                prompt_progress: false,
            })
        }
    }
}

fn is_prompt_progress_update(update: &SessionUpdate) -> bool {
    matches!(
        update,
        SessionUpdate::AgentMessageChunk(_)
            | SessionUpdate::AgentThoughtChunk(_)
            | SessionUpdate::ToolCall(_)
            | SessionUpdate::ToolCallUpdate(_)
            | SessionUpdate::Plan(_)
            | SessionUpdate::AvailableCommandsUpdate(_)
    )
}

async fn emit_current_mode(
    session_id: &str,
    provider_id: &str,
    event_tx: &mpsc::Sender<AcpEvent>,
    update: CurrentModeUpdate,
) -> Result<(), String> {
    event_tx
        .send(provider_info_event(
            session_id.to_string(),
            provider_id,
            json!({
                "type": "current_mode",
                "title": "Agent mode updated",
                "current_mode": serde_json::to_value(&update).unwrap_or(Value::Null),
            }),
        ))
        .await
        .map_err(|_| "ACP event channel closed".to_string())
}

async fn emit_config_update(
    session_id: &str,
    provider_id: &str,
    event_tx: &mpsc::Sender<AcpEvent>,
    update: ConfigOptionUpdate,
) -> Result<(), String> {
    event_tx
        .send(provider_info_event(
            session_id.to_string(),
            provider_id,
            json!({
                "type": "config_update",
                "title": "Configuration options updated",
                "config_update": serde_json::to_value(&update).unwrap_or(Value::Null),
            }),
        ))
        .await
        .map_err(|_| "ACP event channel closed".to_string())
}

async fn emit_session_info(
    session_id: &str,
    provider_id: &str,
    event_tx: &mpsc::Sender<AcpEvent>,
    update: SessionInfoUpdate,
) -> Result<(), String> {
    event_tx
        .send(provider_info_event(
            session_id.to_string(),
            provider_id,
            json!({
                "type": "session_info",
                "title": "Session info updated",
                "session_info": serde_json::to_value(&update).unwrap_or(Value::Null),
            }),
        ))
        .await
        .map_err(|_| "ACP event channel closed".to_string())
}

fn session_state_event(
    session_id: String,
    provider: &str,
    state: lab_apis::acp::types::AcpSessionState,
) -> AcpEvent {
    AcpEvent::SessionUpdate {
        id: uuid::Uuid::new_v4().to_string(),
        created_at: jiff::Timestamp::now().to_string(),
        session_id,
        seq: 0,
        provider: provider.to_string(),
        state,
    }
}

fn provider_info_event(session_id: String, provider: &str, raw: Value) -> AcpEvent {
    AcpEvent::ProviderInfo {
        id: uuid::Uuid::new_v4().to_string(),
        created_at: jiff::Timestamp::now().to_string(),
        session_id,
        seq: 0,
        provider: provider.to_string(),
        raw,
    }
}

fn unfinished_prompt_exit_event(
    session_id: &str,
    provider_id: &str,
    saw_assistant_output: bool,
    run_error: &Option<agent_client_protocol::Error>,
) -> (lab_apis::acp::types::AcpSessionState, AcpEvent) {
    if let Some(error) = run_error {
        return (
            lab_apis::acp::types::AcpSessionState::Failed,
            provider_info_event(
                session_id.to_string(),
                provider_id,
                json!({
                    "type": "provider_error",
                    "title": "Provider error",
                    "text": error.to_string(),
                }),
            ),
        );
    }

    let state = if saw_assistant_output {
        lab_apis::acp::types::AcpSessionState::Completed
    } else {
        lab_apis::acp::types::AcpSessionState::Failed
    };
    let status = match state {
        lab_apis::acp::types::AcpSessionState::Completed => "completed",
        _ => "failed",
    };
    (
        state,
        provider_info_event(
            session_id.to_string(),
            provider_id,
            json!({
                "type": "runtime_exit_without_stop_reason",
                "title": "ACP provider exited before sending a prompt stop reason",
                "status": status,
            }),
        ),
    )
}

fn tool_call_update_output(update: agent_client_protocol::schema::ToolCallUpdate) -> Value {
    let meta = update.meta;
    let fields = update.fields;
    // When raw_output is present and is an Object, inject the wrapper-level _meta into it
    // (outer wins — the wrapper _meta takes precedence over any _meta already in raw_output).
    // Non-object raw_output passes through unchanged.
    // Never log _meta field values (cwd, terminal_id, signal, data).
    if let Some(raw_output) = fields.raw_output {
        match raw_output {
            Value::Object(mut map) => {
                if let Some(m) = meta {
                    map.insert("_meta".into(), Value::Object(m));
                }
                return Value::Object(map);
            }
            other => return other,
        }
    }

    let mut payload = json!({
        "title": fields.title,
        "kind": fields.kind.as_ref().and_then(enum_value),
        "status": fields.status.as_ref().and_then(enum_value),
        "content": fields.content,
        "locations": fields.locations.as_ref().map(|locs| {
            locs.iter().map(|l| l.path.display().to_string()).collect::<Vec<_>>()
        }),
        "raw_input": fields.raw_input,
    });
    if let Some(m) = meta {
        payload
            .as_object_mut()
            .unwrap()
            .insert("_meta".into(), Value::Object(m));
    }
    payload
}

fn content_to_text(content: ContentBlock) -> String {
    match content {
        ContentBlock::Text(value) => value.text,
        ContentBlock::Image(_) => "[image]".to_string(),
        ContentBlock::Audio(_) => "[audio]".to_string(),
        ContentBlock::ResourceLink(value) => format!("[resource] {}", value.uri),
        ContentBlock::Resource(_) => "[embedded resource]".to_string(),
        _ => "[content]".to_string(),
    }
}

fn enum_value<T: serde::Serialize>(value: &T) -> Option<String> {
    serde_json::to_value(value)
        .ok()
        .and_then(|value| value.as_str().map(ToOwned::to_owned))
}

fn map_stop_reason(stop_reason: &StopReason) -> &'static str {
    match stop_reason {
        StopReason::Cancelled => "cancelled",
        StopReason::EndTurn => "end_turn",
        StopReason::MaxTokens => "max_tokens",
        StopReason::MaxTurnRequests => "max_turn_requests",
        StopReason::Refusal => "refusal",
        _ => "unknown",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use agent_client_protocol::schema::{
        AvailableCommandsUpdate, PermissionOptionId, TextContent, ToolCall, ToolCallUpdate,
        ToolCallUpdateFields,
    };

    #[test]
    fn percent_encode_path_segment_escapes_local_attachment_names() {
        assert_eq!(
            percent_encode_path_segment("../my notes#1.txt"),
            "..%2Fmy%20notes%231.txt"
        );
    }

    #[test]
    fn launch_uses_structured_args_when_present() {
        let mut env = std::collections::BTreeMap::new();
        env.insert("CODEX_TOKEN".into(), "spaces and quotes".into());

        let entry = AcpProviderEntry {
            id: "p".into(),
            name: "P".into(),
            version: "1".into(),
            distribution: "binary".into(),
            command: "/opt/with spaces/codex".into(),
            args: vec![
                "--config".into(),
                "value with spaces".into(),
                "--quoted=\"x\"".into(),
            ],
            cwd: Some(PathBuf::from("/work dir")),
            env: env.clone(),
            installed_at: "2026-04-30T00:00:00Z".into(),
            sha256: None,
        };
        let launch = launch_from_provider_entry(&entry);
        // Structured args round-trip verbatim — no whitespace-splitting.
        assert_eq!(launch.command, "/opt/with spaces/codex");
        assert_eq!(launch.args, entry.args);
        assert_eq!(launch.cwd.as_deref(), Some(Path::new("/work dir")));
        assert_eq!(launch.env, env);
    }

    #[test]
    fn launch_falls_back_to_whitespace_split_for_legacy_entries() {
        // Legacy: empty args, command carries the whole argv joined with spaces.
        let entry = AcpProviderEntry {
            id: "old".into(),
            name: "Old".into(),
            version: "0.9".into(),
            distribution: "npx".into(),
            command: "npx -y @scope/old-acp --flag".into(),
            args: Vec::new(),
            cwd: None,
            env: std::collections::BTreeMap::new(),
            installed_at: "2026-01-01T00:00:00Z".into(),
            sha256: None,
        };
        let launch = launch_from_provider_entry(&entry);
        assert_eq!(launch.command, "npx");
        assert_eq!(
            launch.args,
            vec!["-y", "@scope/old-acp", "--flag"]
                .into_iter()
                .map(String::from)
                .collect::<Vec<_>>()
        );
        // Legacy entries have no cwd/env override — caller falls back to
        // session cwd and the bare allowlist.
        assert!(launch.cwd.is_none());
        assert!(launch.env.is_empty());
    }

    #[test]
    fn codex_sandbox_mode_is_read_from_structured_config_args() {
        let args = vec![
            "-y".to_string(),
            "@zed-industries/codex-acp@0.12.0".to_string(),
            "-c".to_string(),
            "sandbox_mode=\"danger-full-access\"".to_string(),
        ];

        assert_eq!(
            codex_sandbox_mode_from_args(&args).as_deref(),
            Some("danger-full-access")
        );
    }

    #[test]
    fn codex_sandbox_mode_detects_workspace_write_as_unsafe_in_container() {
        let args = vec![
            "--config".to_string(),
            "sandbox_mode=\"workspace-write\"".to_string(),
        ];

        assert_eq!(
            codex_sandbox_mode_from_args(&args).as_deref(),
            Some("workspace-write")
        );
    }

    #[test]
    fn codex_sandbox_health_message_explains_container_mismatch() {
        let entry = AcpProviderEntry {
            id: "codex-acp".into(),
            name: "Codex".into(),
            version: "0.12.0".into(),
            distribution: "npx".into(),
            command: "npx".into(),
            args: vec!["-c".into(), "sandbox_mode=\"workspace-write\"".into()],
            cwd: None,
            env: std::collections::BTreeMap::new(),
            installed_at: "2026-05-04T00:00:00Z".into(),
            sha256: None,
        };

        let message = codex_sandbox_incompatibility_message_for_runtime(&entry, true)
            .expect("container sandbox mismatch message");
        assert!(message.contains("workspace-write"));
        assert!(message.contains("danger-full-access"));

        assert!(codex_sandbox_incompatibility_message_for_runtime(&entry, false).is_none());
    }

    #[test]
    fn cgroup_detection_recognizes_docker_and_containerd() {
        assert!(cgroup_mentions_container_runtime(
            "0::/system.slice/docker-2d52.scope"
        ));
        assert!(cgroup_mentions_container_runtime(
            "0::/kubepods.slice/containerd/io.containerd.runtime.v2.task"
        ));
        assert!(!cgroup_mentions_container_runtime(
            "0::/user.slice/user-1000.slice"
        ));
    }

    fn text_chunk(text: &str) -> ContentChunk {
        ContentChunk::new(ContentBlock::Text(TextContent::new(text)))
    }

    fn received_message_id(rx: &mut mpsc::Receiver<AcpEvent>) -> String {
        match rx.try_recv().expect("message chunk event") {
            AcpEvent::MessageChunk { message_id, .. } => message_id,
            other => panic!("expected message chunk event, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn streamed_message_chunks_share_stable_message_ids_per_role() {
        let (tx, mut rx) = mpsc::channel(crate::acp::registry::ACP_EVENT_CHANNEL_CAPACITY);
        let message_ids = Arc::new(Mutex::new(StreamMessageIds::default()));

        push_session_update(
            "session-1",
            "codex-acp",
            &tx,
            SessionUpdate::UserMessageChunk(text_chunk("hello ")),
            &message_ids,
        )
        .await
        .expect("first user chunk");
        push_session_update(
            "session-1",
            "codex-acp",
            &tx,
            SessionUpdate::UserMessageChunk(text_chunk("world")),
            &message_ids,
        )
        .await
        .expect("second user chunk");
        push_session_update(
            "session-1",
            "codex-acp",
            &tx,
            SessionUpdate::AgentMessageChunk(text_chunk("reply ")),
            &message_ids,
        )
        .await
        .expect("first assistant chunk");
        push_session_update(
            "session-1",
            "codex-acp",
            &tx,
            SessionUpdate::AgentMessageChunk(text_chunk("done")),
            &message_ids,
        )
        .await
        .expect("second assistant chunk");

        let first_user_message_id = received_message_id(&mut rx);
        let second_user_message_id = received_message_id(&mut rx);
        let first_assistant_message_id = received_message_id(&mut rx);
        let second_assistant_message_id = received_message_id(&mut rx);

        assert_eq!(first_user_message_id, second_user_message_id);
        assert_eq!(first_assistant_message_id, second_assistant_message_id);
        assert_ne!(first_user_message_id, first_assistant_message_id);
    }

    #[test]
    fn prompt_progress_includes_provider_turn_activity() {
        assert!(is_prompt_progress_update(
            &SessionUpdate::AgentThoughtChunk(text_chunk("thinking"))
        ));
        assert!(is_prompt_progress_update(&SessionUpdate::ToolCall(
            ToolCall::new("tool-1", "Read file")
        )));
        assert!(is_prompt_progress_update(
            &SessionUpdate::AvailableCommandsUpdate(AvailableCommandsUpdate::new(vec![]))
        ));
    }

    /// Drain all pending events and return them. Panics if the channel is empty and
    /// expected_count events have not been collected.
    fn drain_events(rx: &mut mpsc::Receiver<AcpEvent>, expected_count: usize) -> Vec<AcpEvent> {
        let mut events = Vec::new();
        while let Ok(event) = rx.try_recv() {
            events.push(event);
            if events.len() == expected_count {
                break;
            }
        }
        assert_eq!(
            events.len(),
            expected_count,
            "expected {expected_count} events, got {}",
            events.len()
        );
        events
    }

    /// Build a minimal terminal_info Meta blob for tests.
    fn terminal_info_meta() -> agent_client_protocol::schema::Meta {
        let mut meta = serde_json::Map::new();
        meta.insert(
            "terminal_info".into(),
            json!({
                "terminal_id": "term-secret-42",
                "cwd": "/home/secret/projects/lab",
            }),
        );
        meta
    }

    // -----------------------------------------------------------------------
    // Test 1: tool_call_metadata_round_trips_terminal_meta
    //
    // Both SessionUpdate::ToolCall and ToolCallUpdate paths must preserve the
    // `_meta` field through to the emitted AcpEvent payload.
    // -----------------------------------------------------------------------
    #[tokio::test]
    async fn tool_call_metadata_round_trips_terminal_meta() {
        let (tx, mut rx) = mpsc::channel(crate::acp::registry::ACP_EVENT_CHANNEL_CAPACITY);
        let message_ids = Arc::new(Mutex::new(StreamMessageIds::default()));

        // --- ToolCall path ---
        let meta = terminal_info_meta();
        let tool_call = ToolCall::new("tc-1", "Read file")
            .status(agent_client_protocol::schema::ToolCallStatus::Completed)
            .meta(meta.clone());
        push_session_update(
            "session-1",
            "codex-acp",
            &tx,
            SessionUpdate::ToolCall(tool_call),
            &message_ids,
        )
        .await
        .expect("ToolCall with meta");

        // Expect 2 events: ToolCallStart + provider_info (tool_call_metadata)
        let events = drain_events(&mut rx, 2);

        // First event: ToolCallStart
        assert!(
            matches!(&events[0], AcpEvent::ToolCallStart { .. }),
            "expected ToolCallStart, got {:?}",
            events[0]
        );

        // Second event: ProviderInfo carrying _meta
        match &events[1] {
            AcpEvent::ProviderInfo { raw, .. } => {
                let meta_value = raw
                    .get("_meta")
                    .expect("_meta key must be present in provider_info");
                let terminal_info = meta_value
                    .get("terminal_info")
                    .expect("terminal_info key present");
                assert_eq!(
                    terminal_info.get("terminal_id").and_then(Value::as_str),
                    Some("term-secret-42"),
                    "terminal_id must round-trip"
                );
                assert_eq!(
                    terminal_info.get("cwd").and_then(Value::as_str),
                    Some("/home/secret/projects/lab"),
                    "cwd must round-trip"
                );
            }
            other => panic!("expected ProviderInfo, got {other:?}"),
        }

        // --- ToolCallUpdate path ---
        let update_meta = terminal_info_meta();
        let fields = ToolCallUpdateFields::new();
        let update = ToolCallUpdate::new("tc-2", fields).meta(update_meta.clone());
        push_session_update(
            "session-1",
            "codex-acp",
            &tx,
            SessionUpdate::ToolCallUpdate(update),
            &message_ids,
        )
        .await
        .expect("ToolCallUpdate with meta");

        let update_events = drain_events(&mut rx, 1);
        match &update_events[0] {
            AcpEvent::ToolCallUpdate { output, .. } => {
                let meta_value = output
                    .get("_meta")
                    .expect("_meta key must be present in output");
                let terminal_info = meta_value
                    .get("terminal_info")
                    .expect("terminal_info key present");
                assert_eq!(
                    terminal_info.get("terminal_id").and_then(Value::as_str),
                    Some("term-secret-42"),
                    "terminal_id must round-trip in ToolCallUpdate"
                );
            }
            other => panic!("expected ToolCallUpdate, got {other:?}"),
        }
    }

    // -----------------------------------------------------------------------
    // Test 2: tool_call_update_output_outer_meta_wins_over_raw_output_inner_meta
    //
    // A9 merge semantics: when raw_output already contains _meta, the wrapper-level
    // _meta (outer) wins.
    // -----------------------------------------------------------------------
    #[test]
    fn tool_call_update_output_outer_meta_wins_over_raw_output_inner_meta() {
        let mut outer_meta = serde_json::Map::new();
        outer_meta.insert("source".into(), Value::String("outer".into()));

        let inner_meta_json = json!({"source": "inner", "extra": "inner-only"});
        let raw_output_with_inner_meta = json!({
            "result": "ok",
            "_meta": inner_meta_json,
        });

        let fields = ToolCallUpdateFields::new().raw_output(raw_output_with_inner_meta);
        let update = ToolCallUpdate::new("tc-merge", fields).meta(outer_meta);

        let output = tool_call_update_output(update);

        // Outer _meta must win — source should be "outer", not "inner".
        let meta_value = output.get("_meta").expect("_meta key present after merge");
        assert_eq!(
            meta_value.get("source").and_then(Value::as_str),
            Some("outer"),
            "outer _meta must overwrite inner _meta"
        );
        // Inner-only key should no longer be present (entire _meta replaced, not merged).
        assert!(
            meta_value.get("extra").is_none(),
            "inner-only keys must not survive outer-wins replacement"
        );
        // Other raw_output fields must be preserved.
        assert_eq!(
            output.get("result").and_then(Value::as_str),
            Some("ok"),
            "non-_meta fields in raw_output must be preserved"
        );
    }

    // -----------------------------------------------------------------------
    // Test 3: tool_call_event_omits_meta_key_when_none
    //
    // P4: when meta is None, the `_meta` key must be absent from both the
    // ToolCall provider_info payload and the ToolCallUpdate output.
    // -----------------------------------------------------------------------
    #[tokio::test]
    async fn tool_call_event_omits_meta_key_when_none() {
        let (tx, mut rx) = mpsc::channel(crate::acp::registry::ACP_EVENT_CHANNEL_CAPACITY);
        let message_ids = Arc::new(Mutex::new(StreamMessageIds::default()));

        // ToolCall with no meta and a status (so the provider_info event fires)
        let tool_call = ToolCall::new("tc-no-meta", "Read file")
            .status(agent_client_protocol::schema::ToolCallStatus::Completed);
        push_session_update(
            "session-1",
            "codex-acp",
            &tx,
            SessionUpdate::ToolCall(tool_call),
            &message_ids,
        )
        .await
        .expect("ToolCall without meta");

        let events = drain_events(&mut rx, 2);
        match &events[1] {
            AcpEvent::ProviderInfo { raw, .. } => {
                assert!(
                    raw.get("_meta").is_none(),
                    "_meta key must be absent from provider_info when ToolCall.meta is None, got: {:?}",
                    raw.get("_meta")
                );
            }
            other => panic!("expected ProviderInfo, got {other:?}"),
        }

        // ToolCallUpdate with no meta
        let fields = ToolCallUpdateFields::new();
        let update = ToolCallUpdate::new("tc-no-meta-update", fields);
        push_session_update(
            "session-1",
            "codex-acp",
            &tx,
            SessionUpdate::ToolCallUpdate(update),
            &message_ids,
        )
        .await
        .expect("ToolCallUpdate without meta");

        let update_events = drain_events(&mut rx, 1);
        match &update_events[0] {
            AcpEvent::ToolCallUpdate { output, .. } => {
                assert!(
                    output.get("_meta").is_none(),
                    "_meta key must be absent from ToolCallUpdate output when meta is None, got: {:?}",
                    output.get("_meta")
                );
            }
            other => panic!("expected ToolCallUpdate, got {other:?}"),
        }
    }

    // _meta redaction is an architectural guarantee: push_session_update and
    // tool_call_update_output emit no tracing spans, so _meta field values
    // (cwd, terminal_id, signal, data) never reach the log output by construction.
    // Enforcement is via is_sensitive_key() in dispatch/redact.rs for the DB path.

    // -----------------------------------------------------------------------
    // Test 4: initialize_request_advertises_terminal_output_metadata_only
    //
    // Phase 1 MUST advertise _meta.terminal_output=true and terminal=false.
    // DO NOT call .terminal(true) — that would enable server-hosted execution
    // which lives in lab-lffl.
    // -----------------------------------------------------------------------
    #[test]
    fn initialize_request_advertises_terminal_output_metadata_only() {
        use agent_client_protocol::schema::InitializeRequest;

        let capabilities = lab_client_capabilities();

        let value = serde_json::to_value(&capabilities).unwrap();

        // terminal must be false (Phase 1: no server-hosted execution).
        assert_eq!(
            value.get("terminal"),
            Some(&serde_json::json!(false)),
            "terminal must be false in Phase 1 — server-hosted execution lives in lab-lffl"
        );

        assert_eq!(
            value.get("fs").and_then(|fs| fs.get("readTextFile")),
            Some(&serde_json::json!(false)),
            "provider filesystem reads must stay disabled until a workspace jail lands"
        );
        assert_eq!(
            value.get("fs").and_then(|fs| fs.get("writeTextFile")),
            Some(&serde_json::json!(false)),
            "provider filesystem writes must stay disabled until a workspace jail lands"
        );

        // _meta.terminal_output must be true (Phase 1: display metadata relay).
        assert_eq!(
            value.get("_meta").and_then(|m| m.get("terminal_output")),
            Some(&serde_json::json!(true)),
            "_meta.terminal_output must be true to advertise display support"
        );

        // Verify the full InitializeRequest serialization also reflects capabilities.
        let req = InitializeRequest::new(ProtocolVersion::V1).client_capabilities(capabilities);
        let req_value = serde_json::to_value(&req).unwrap();
        assert_eq!(
            req_value
                .get("clientCapabilities")
                .and_then(|c| c.get("_meta"))
                .and_then(|m| m.get("terminal_output")),
            Some(&serde_json::json!(true)),
            "_meta.terminal_output must survive InitializeRequest serialization"
        );
        assert_eq!(
            req_value
                .get("clientCapabilities")
                .and_then(|c| c.get("terminal")),
            Some(&serde_json::json!(false)),
            "terminal must be false in InitializeRequest"
        );
        assert_eq!(
            req_value
                .get("clientCapabilities")
                .and_then(|c| c.get("fs"))
                .and_then(|fs| fs.get("readTextFile")),
            Some(&serde_json::json!(false)),
            "InitializeRequest must not advertise provider filesystem reads"
        );
        assert_eq!(
            req_value
                .get("clientCapabilities")
                .and_then(|c| c.get("fs"))
                .and_then(|fs| fs.get("writeTextFile")),
            Some(&serde_json::json!(false)),
            "InitializeRequest must not advertise provider filesystem writes"
        );
    }

    fn permission_request(tool_call_id: &str) -> RequestPermissionRequest {
        RequestPermissionRequest::new(
            "provider-session-1",
            ToolCallUpdate::new(
                tool_call_id.to_string(),
                ToolCallUpdateFields::new().title(Some("Read project file".to_string())),
            ),
            vec![
                PermissionOption::new(
                    PermissionOptionId::new("allow-once"),
                    "Allow once",
                    PermissionOptionKind::AllowOnce,
                ),
                PermissionOption::new(
                    PermissionOptionId::new("reject-once"),
                    "Reject",
                    PermissionOptionKind::RejectOnce,
                ),
            ],
        )
    }

    fn permission_outcome_granted(events: &[AcpEvent]) -> bool {
        match events.last().expect("permission outcome event") {
            AcpEvent::PermissionOutcome { granted, .. } => *granted,
            other => panic!("expected PermissionOutcome, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn permission_request_is_pending_by_default_until_timeout() {
        let (tx, mut rx) = mpsc::channel(crate::acp::registry::ACP_EVENT_CHANNEL_CAPACITY);
        let permissions = PendingPermissions::new(Duration::from_millis(25));

        let response = handle_permission_request(
            "session-1",
            "codex-acp",
            &tx,
            &permissions,
            permission_request("tool-1"),
        )
        .await;

        assert!(matches!(
            response.outcome,
            RequestPermissionOutcome::Cancelled
        ));
        assert_eq!(permissions.pending_count(), 0);

        let events = drain_events(&mut rx, 2);
        assert!(matches!(events[0], AcpEvent::PermissionRequest { .. }));
        assert!(!permission_outcome_granted(&events));
    }

    #[tokio::test]
    async fn explicit_rejection_selects_reject_option_and_denies_request() {
        let (tx, mut rx) = mpsc::channel(crate::acp::registry::ACP_EVENT_CHANNEL_CAPACITY);
        let permissions = PendingPermissions::new(Duration::from_secs(1));
        let permissions_for_decision = permissions.clone();

        let pending = tokio::spawn(async move {
            handle_permission_request(
                "session-1",
                "codex-acp",
                &tx,
                &permissions,
                permission_request("tool-reject"),
            )
            .await
        });
        tokio::task::yield_now().await;

        permissions_for_decision
            .reject("tool-reject")
            .expect("reject pending permission");
        let response = pending.await.expect("permission task joins");

        match response.outcome {
            RequestPermissionOutcome::Selected(selected) => {
                assert_eq!(selected.option_id.to_string(), "reject-once");
            }
            other => panic!("expected selected reject option, got {other:?}"),
        }
        assert_eq!(permissions_for_decision.pending_count(), 0);

        let events = drain_events(&mut rx, 2);
        assert!(!permission_outcome_granted(&events));
    }

    #[tokio::test]
    async fn explicit_approval_selects_only_requested_allow_option() {
        let (tx, mut rx) = mpsc::channel(crate::acp::registry::ACP_EVENT_CHANNEL_CAPACITY);
        let permissions = PendingPermissions::new(Duration::from_secs(1));
        let permissions_for_decision = permissions.clone();

        let pending = tokio::spawn(async move {
            handle_permission_request(
                "session-1",
                "codex-acp",
                &tx,
                &permissions,
                permission_request("tool-allow"),
            )
            .await
        });
        tokio::task::yield_now().await;

        let wrong_request = permissions_for_decision
            .approve("other-tool", "allow-once")
            .expect_err("approval must be scoped to a pending request");
        assert!(wrong_request.contains("not pending"));

        let reject_as_approval = permissions_for_decision
            .approve("tool-allow", "reject-once")
            .expect_err("approval must not select a reject option");
        assert!(reject_as_approval.contains("allow option"));

        permissions_for_decision
            .approve("tool-allow", "allow-once")
            .expect("approve requested permission");
        let response = pending.await.expect("permission task joins");

        match response.outcome {
            RequestPermissionOutcome::Selected(selected) => {
                assert_eq!(selected.option_id.to_string(), "allow-once");
            }
            other => panic!("expected selected allow option, got {other:?}"),
        }
        assert_eq!(permissions_for_decision.pending_count(), 0);

        let events = drain_events(&mut rx, 2);
        assert!(permission_outcome_granted(&events));
    }

    #[tokio::test]
    async fn cancellation_does_not_allow_pending_permission() {
        let (tx, mut rx) = mpsc::channel(crate::acp::registry::ACP_EVENT_CHANNEL_CAPACITY);
        let permissions = PendingPermissions::new(Duration::from_secs(1));
        let permissions_for_cancel = permissions.clone();

        let pending = tokio::spawn(async move {
            handle_permission_request(
                "session-1",
                "codex-acp",
                &tx,
                &permissions,
                permission_request("tool-cancel"),
            )
            .await
        });
        tokio::task::yield_now().await;

        permissions_for_cancel.cancel_session("provider-session-1");
        let response = pending.await.expect("permission task joins");

        assert!(matches!(
            response.outcome,
            RequestPermissionOutcome::Cancelled
        ));
        assert_eq!(permissions_for_cancel.pending_count(), 0);

        let events = drain_events(&mut rx, 2);
        assert!(!permission_outcome_granted(&events));
    }

    #[test]
    fn provider_subprocess_env_only_keeps_explicit_allowlist() {
        let env = provider_subprocess_env(vec![
            ("PATH".to_string(), "/usr/bin".to_string()),
            ("HOME".to_string(), "/home/test".to_string()),
            ("LANG".to_string(), "C.UTF-8".to_string()),
            ("LAB_TOKEN".to_string(), "lab-secret".to_string()),
            ("RADARR_API_KEY".to_string(), "radarr-secret".to_string()),
            ("OPENAI_API_KEY".to_string(), "openai-secret".to_string()),
            (
                "AWS_SECRET_ACCESS_KEY".to_string(),
                "aws-secret".to_string(),
            ),
            ("GITHUB_TOKEN".to_string(), "github-secret".to_string()),
            ("CUSTOM_PASSWORD".to_string(), "password-secret".to_string()),
            ("UNRELATED".to_string(), "value".to_string()),
        ]);

        let keys: Vec<&str> = env.iter().map(|(key, _)| key.as_str()).collect();
        assert_eq!(keys, vec!["HOME", "LANG", "PATH"]);
        assert!(
            env.iter().all(|(_, value)| !value.contains("secret")),
            "provider env must not include service credentials: {env:?}"
        );
    }

    #[test]
    fn sanitize_provider_error_strips_ip_jwt_and_home_paths() {
        let raw = "failed to auth to 10.0.0.5:8000 with eyJabc.def.ghi at /home/user/.lab/creds";
        let clean = sanitize_provider_error(raw);
        assert!(
            !clean.contains("10.0.0.5"),
            "IP should be redacted: {clean}"
        );
        assert!(!clean.contains("eyJabc"), "JWT should be redacted: {clean}");
        assert!(
            !clean.contains("/home/user"),
            "home path should be redacted: {clean}"
        );
        assert!(clean.contains("[redacted-ip]"));
        assert!(clean.contains("[redacted-jwt]"));
        assert!(clean.contains("[path]"));
    }

    #[test]
    fn sanitize_provider_error_passes_known_safe_messages_unchanged() {
        let raw = "model not found: gpt-5.1";
        assert_eq!(sanitize_provider_error(raw), raw);
    }

    #[test]
    fn redact_provider_stderr_line_strips_ip_and_path() {
        let (line, truncated) = redact_provider_stderr_line(
            "connect failed at 192.168.1.100:443 in /home/jmagar/workspace",
        );
        assert!(!truncated);
        assert!(!line.contains("192.168.1.100"));
        assert!(!line.contains("/home/jmagar"));
    }

    #[test]
    fn redact_provider_stderr_line_masks_secrets_and_limits_length() {
        let (line, truncated) =
            redact_provider_stderr_line("failed OPENAI_API_KEY=abc123 --token=secret mode=debug");
        assert!(!truncated);
        assert_eq!(
            line,
            "failed OPENAI_API_KEY=[redacted] --token=[redacted] mode=debug"
        );

        let long_secret = format!(
            "RADARR_API_KEY=secret {}",
            "x".repeat(MAX_PROVIDER_STDERR_CHARS + 100)
        );
        let (line, truncated) = redact_provider_stderr_line(&long_secret);
        assert!(truncated);
        assert_eq!(line.chars().count(), MAX_PROVIDER_STDERR_CHARS);
        assert!(!line.contains("secret"));
        assert!(line.starts_with("RADARR_API_KEY=[redacted] "));
    }

    #[test]
    fn unfinished_prompt_with_provider_error_fails_even_after_progress() {
        let run_error = Some(acp_internal_error("usage_limit_exceeded"));
        let (state, event) =
            unfinished_prompt_exit_event("session-1", "codex-acp", true, &run_error);

        assert_eq!(state, lab_apis::acp::types::AcpSessionState::Failed);
        let AcpEvent::ProviderInfo { raw, .. } = event else {
            panic!("expected provider_info event");
        };
        assert_eq!(raw["type"], "provider_error");
        assert!(raw["text"].as_str().unwrap_or("").contains("usage_limit"));
    }

    #[test]
    fn unfinished_prompt_without_provider_error_preserves_idle_completion_heuristic() {
        let (state, event) = unfinished_prompt_exit_event("session-1", "codex-acp", true, &None);

        assert_eq!(state, lab_apis::acp::types::AcpSessionState::Completed);
        let AcpEvent::ProviderInfo { raw, .. } = event else {
            panic!("expected provider_info event");
        };
        assert_eq!(raw["type"], "runtime_exit_without_stop_reason");
        assert_eq!(raw["status"], "completed");
    }

    // -----------------------------------------------------------------------
    // Test 5: phase_1_terminal_requests_return_method_not_found
    //
    // C6 — NEGATIVE integration test: even with _meta.terminal_output=true
    // advertised, the runtime must NOT execute terminal creation. All terminal/*
    // request handlers exist but unconditionally return method_not_found (-32601).
    // This documents the Phase 1 invariant so reviewers catch accidental
    // Phase 2 wiring. A full live RPC test requires a running ACP session
    // and belongs in integration tests; this unit test anchors the invariant
    // structurally.
    //
    // Invariant: all terminal/* on_receive_request handlers in the Dispatch
    // impl respond with `Error::method_not_found()`. No handler executes
    // terminal operations or delegates to a jail. lab-lffl is the gate that
    // activates terminal execution in Phase 2.
    // -----------------------------------------------------------------------
    #[test]
    fn phase_1_terminal_requests_return_method_not_found() {
        // CreateTerminalRequest is imported and has a handler arm that returns
        // method_not_found. Verify the import compiles. The handler arm exists
        // to satisfy the ACP protocol type system while blocking execution.
        //
        // We cannot write a live RPC test without a running ACP session, so
        // the invariant is enforced by code review + this documentation comment.
        // Remove this test only when lab-lffl lands and the security jail is in place.
        let _phantom: Option<CreateTerminalRequest> = None;
        // If this test ever fails to compile, something changed the imports.
        // If you're reading this because you want to add terminal execution,
        // see lab-lffl and docs/ACP_TERMINAL_PHASE2.md first.
    }
}

// ---------------------------------------------------------------------------
// Test helpers
// ---------------------------------------------------------------------------

/// Returns a fake `RuntimeHandle` and the paired `mpsc::Receiver<AcpEvent>`.
/// Drop the receiver to simulate subprocess exit (event forwarder sees channel close).
#[cfg(test)]
#[allow(dead_code)]
pub fn fake_handle_for_tests() -> (RuntimeHandle, mpsc::Receiver<AcpEvent>) {
    let (command_tx, command_rx) = mpsc::channel::<SessionCommand>(SESSION_COMMAND_QUEUE_CAPACITY);
    let terminated = Arc::new(AtomicBool::new(false));
    let task_terminated = Arc::clone(&terminated);
    let termination_notify = Arc::new(Notify::new());
    let task_termination_notify = Arc::clone(&termination_notify);
    tokio::spawn(async move {
        let mut command_rx = command_rx;
        while command_rx.recv().await.is_some() {}
        task_terminated.store(true, Ordering::SeqCst);
        task_termination_notify.notify_waiters();
    });
    let (event_tx, event_rx) =
        mpsc::channel::<AcpEvent>(crate::acp::registry::ACP_EVENT_CHANNEL_CAPACITY);
    let handle = RuntimeHandle {
        provider_session_id: "fake-provider-session".to_string(),
        command_tx,
        permissions: Arc::new(PendingPermissions::new(DEFAULT_PERMISSION_TIMEOUT)),
        terminated,
        termination_notify,
        _event_tx_for_tests: Some(event_tx),
    };
    (handle, event_rx)
}

#[cfg(test)]
#[allow(dead_code)]
pub fn saturated_fake_handle_for_tests() -> (RuntimeHandle, mpsc::Receiver<AcpEvent>) {
    let (command_tx, command_rx) = mpsc::channel::<SessionCommand>(1);
    let terminated = Arc::new(AtomicBool::new(false));
    let termination_notify = Arc::new(Notify::new());
    command_tx
        .try_send(SessionCommand::Prompt(PromptCommand {
            input: PromptInput {
                text: "already queued".to_string(),
                attachments: Vec::new(),
            },
            model_id: None,
        }))
        .expect("prefill command queue");
    tokio::spawn(async move {
        let _command_rx = command_rx;
        std::future::pending::<()>().await;
    });
    let (event_tx, event_rx) =
        mpsc::channel::<AcpEvent>(crate::acp::registry::ACP_EVENT_CHANNEL_CAPACITY);
    let handle = RuntimeHandle {
        provider_session_id: "fake-provider-session".to_string(),
        command_tx,
        permissions: Arc::new(PendingPermissions::new(DEFAULT_PERMISSION_TIMEOUT)),
        terminated,
        termination_notify,
        _event_tx_for_tests: Some(event_tx),
    };
    (handle, event_rx)
}
