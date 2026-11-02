//! Shared single-writer interactive sessions for the native Claude Code and Antigravity TUIs.
//!
//! This is intentionally separate from the machine/NDJSON adapters.  A session has one owned
//! `ConPTY`, one native hook endpoint, and zero writable views.

use std::{
    collections::{HashMap, HashSet},
    fmt::Write as _,
    fs,
    path::{Path, PathBuf},
    sync::{
        Mutex,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};

use lazoo_runtime::{TerminalCommand, TerminalSession};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use tokio::{
    sync::{Mutex as AsyncMutex, mpsc, watch},
    time::timeout,
};
use uuid::Uuid;

#[path = "native_startup.rs"]
mod startup;
#[path = "native_transcript.rs"]
mod transcript;

use crate::mcp_bridge::SessionMcpBridge;
use crate::{
    AdapterFuture, CommandSpec, PermissionKind, PermissionRequest, ProviderError, ProviderEvent,
    ProviderKind, ProviderSession, SessionRequest, TurnEventObserver, TurnRequest,
    native_hook_server::{NativeHookEvent, NativeHookServer, QueuedHook, native_decision},
};

const COLS: u16 = 120;
const ROWS: u16 = 40;
const MAX_PROMPT: usize = 12 * 1024;
const MAX_PRE_TOOL_IDS: usize = 4096;
/// How long to wait for a trust selection redraw before repeating the selection key.
const TRUST_SELECTION_RETRY: Duration = Duration::from_millis(750);
/// How long the native startup screen must stay unchanged before a trust key is sent.
const TRUST_SETTLE: Duration = Duration::from_millis(500);
/// How long Claude Code's screen must stay unchanged before a turn prompt is typed.
const CLAUDE_INPUT_SETTLE: Duration = Duration::from_millis(1500);
/// The longest wait for that quiet screen; a busy screen gets the prompt after this.
const CLAUDE_INPUT_SETTLE_LIMIT: Duration = Duration::from_secs(5);

struct GeneratedFile {
    path: PathBuf,
    digest: String,
}

#[derive(Default)]
struct ClaudeBinding {
    prompt_id: Option<String>,
    turn_id: Option<String>,
    message_indexes: HashMap<String, (u64, bool)>,
}

#[derive(Default)]
struct AntigravityBinding {
    previous_start: Option<u64>,
    current_start: Option<u64>,
    invocation: Option<u64>,
}

impl AntigravityBinding {
    fn begin_turn(&mut self) {
        self.current_start = None;
        self.invocation = None;
    }
    fn accept_invocation(&mut self, payload: &Value) -> Result<(), ProviderError> {
        let start = payload
            .get("initialNumSteps")
            .and_then(Value::as_u64)
            .ok_or_else(|| {
                ProviderError::Protocol("Antigravity invocation omitted step boundary".to_owned())
            })?;
        let invocation = payload
            .get("invocationNum")
            .and_then(Value::as_u64)
            .unwrap_or(0);
        if let Some(current) = self.current_start {
            if start < current || self.invocation.is_some_and(|last| invocation <= last) {
                return Err(ProviderError::Protocol(
                    "replayed Antigravity invocation".to_owned(),
                ));
            }
        } else {
            if self
                .previous_start
                .is_some_and(|previous| start <= previous)
            {
                return Err(ProviderError::Protocol(
                    "stale Antigravity turn boundary".to_owned(),
                ));
            }
            self.previous_start = Some(start);
            self.current_start = Some(start);
        }
        self.invocation = Some(invocation);
        Ok(())
    }
    fn require_tool(&self, payload: &Value) -> Result<(), ProviderError> {
        let step = payload.get("stepIdx").and_then(Value::as_u64);
        if !matches!((self.current_start, step), (Some(start), Some(step)) if step >= start) {
            return Err(ProviderError::Protocol(
                "stale Antigravity tool step".to_owned(),
            ));
        }
        Ok(())
    }
}

impl ClaudeBinding {
    fn require_prompt(&self, payload: &Value) -> Result<(), ProviderError> {
        let incoming = claude_prompt_id(payload)?;
        if self.prompt_id.as_deref() != Some(incoming.as_str()) {
            return Err(ProviderError::Protocol(
                "stale Claude prompt identity".to_owned(),
            ));
        }
        Ok(())
    }

    fn accept_message(&mut self, payload: &Value) -> Result<(), ProviderError> {
        self.require_prompt(payload)?;
        let turn = native_uuid(payload, "turn_id")?;
        if self.turn_id.as_ref().is_some_and(|active| active != &turn) {
            return Err(ProviderError::Protocol(
                "stale Claude display turn".to_owned(),
            ));
        }
        let message = native_uuid(payload, "message_id")?;
        let index = payload
            .get("index")
            .and_then(Value::as_u64)
            .ok_or_else(|| ProviderError::Protocol("Claude display index missing".to_owned()))?;
        let final_batch = payload
            .get("final")
            .and_then(Value::as_bool)
            .ok_or_else(|| {
                ProviderError::Protocol("Claude display final marker missing".to_owned())
            })?;
        if let Some((previous, finalized)) = self.message_indexes.get(&message) {
            if *finalized || previous.checked_add(1) != Some(index) {
                return Err(ProviderError::Protocol(
                    "replayed or out-of-order Claude display batch".to_owned(),
                ));
            }
        } else if index != 0 || self.message_indexes.len() >= 1024 {
            return Err(ProviderError::Protocol(
                "Claude display sequence exceeds bounds".to_owned(),
            ));
        }
        self.turn_id = Some(turn);
        self.message_indexes.insert(message, (index, final_batch));
        Ok(())
    }
}

fn native_uuid(payload: &Value, field: &str) -> Result<String, ProviderError> {
    payload
        .get(field)
        .and_then(Value::as_str)
        .filter(|value| Uuid::parse_str(value).is_ok())
        .map(str::to_owned)
        .ok_or_else(|| ProviderError::Protocol(format!("Claude {field} identity missing")))
}

fn claude_prompt_id(payload: &Value) -> Result<String, ProviderError> {
    native_uuid(payload, "prompt_id")
}

pub struct NativeInteractiveSession {
    provider: ProviderKind,
    executable: PathBuf,
    request: SessionRequest,
    terminal: TerminalSession,
    hooks: NativeHookServer,
    hook_rx: AsyncMutex<mpsc::Receiver<QueuedHook>>,
    bridge: SessionMcpBridge,
    session_id: Mutex<String>,
    active: AtomicBool,
    lifecycle: Mutex<()>,
    cancelled: AtomicBool,
    cancellation: watch::Sender<bool>,
    stopped: AtomicBool,
    read_only: AtomicBool,
    pre_tool_ids: Mutex<HashSet<String>>,
    declared_tools: HashSet<String>,
    startup_events: Mutex<Vec<ProviderEvent>>,
    transcript: Mutex<Option<transcript::TranscriptBinding>>,
    claude_binding: Mutex<ClaudeBinding>,
    antigravity_binding: Mutex<AntigravityBinding>,
    generated: Vec<GeneratedFile>,
}

struct ActiveTurnGuard<'a> {
    active: &'a AtomicBool,
    stopped: &'a AtomicBool,
    terminal: &'a TerminalSession,
    armed: bool,
}
impl Drop for ActiveTurnGuard<'_> {
    fn drop(&mut self) {
        self.active.store(false, Ordering::Release);
        if self.armed {
            self.stopped.store(true, Ordering::Release);
            let _ = self.terminal.stop();
        }
    }
}

impl std::fmt::Debug for NativeInteractiveSession {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("NativeInteractiveSession")
            .field("provider", &self.provider)
            .field("native_session_id", &self.id_locked())
            .field("active", &self.active.load(Ordering::Acquire))
            .field("stopped", &self.stopped.load(Ordering::Acquire))
            .finish_non_exhaustive()
    }
}

impl NativeInteractiveSession {
    pub(crate) async fn start(
        provider: ProviderKind,
        executable: &Path,
        request: &SessionRequest,
    ) -> Result<Self, ProviderError> {
        if !matches!(
            provider,
            ProviderKind::ClaudeCode | ProviderKind::Antigravity
        ) {
            return Err(ProviderError::Unsupported {
                provider,
                capability: "interactive native sessions",
                detail: None,
            });
        }
        validate_request(provider, request)?;
        let (hooks, hook_rx) = NativeHookServer::open(provider).await?;
        let bridge = SessionMcpBridge::open(provider, request.tools.clone()).await?;
        let generated_paths =
            match prepare_generated_config(provider, executable, request, &hooks, &bridge) {
                Ok(files) => files,
                Err(error) => {
                    hooks.close();
                    bridge.close();
                    return Err(error);
                }
            };
        let generated = generated_paths
            .into_iter()
            .map(seal_generated)
            .collect::<Result<Vec<_>, _>>()?;
        let mut command = request
            .launch_environment
            .apply_to_terminal(TerminalCommand::new(executable, &request.cwd, COLS, ROWS))
            .env("LAZOO_NATIVE_HOOK_URL", hooks.endpoint())
            .env("LAZOO_NATIVE_HOOK_TOKEN", hooks.token());
        command = interactive_command(provider, command, request, &generated);
        let terminal = match TerminalSession::spawn(command) {
            Ok(terminal) => terminal,
            Err(error) => {
                cleanup(&generated);
                hooks.close();
                bridge.close();
                return Err(ProviderError::Protocol(format!(
                    "native terminal start failed: {error}"
                )));
            }
        };
        let (cancellation, _) = watch::channel(false);
        let session = Self {
            provider,
            executable: executable.to_owned(),
            request: request.clone(),
            terminal,
            hooks,
            hook_rx: AsyncMutex::new(hook_rx),
            bridge,
            session_id: Mutex::new(
                request
                    .session_id
                    .clone()
                    .unwrap_or_else(|| format!("pending:{}", Uuid::new_v4())),
            ),
            active: AtomicBool::new(false),
            lifecycle: Mutex::new(()),
            cancelled: AtomicBool::new(false),
            cancellation,
            stopped: AtomicBool::new(false),
            read_only: AtomicBool::new(false),
            pre_tool_ids: Mutex::new(HashSet::new()),
            declared_tools: request.tools.iter().map(|tool| tool.name.clone()).collect(),
            startup_events: Mutex::new(Vec::new()),
            transcript: Mutex::new(None),
            claude_binding: Mutex::new(ClaudeBinding::default()),
            antigravity_binding: Mutex::new(AntigravityBinding::default()),
            generated,
        };
        if provider == ProviderKind::Antigravity
            && let Some(id) = &request.session_id
        {
            *session.transcript.lock().map_err(|_| {
                ProviderError::Protocol("native transcript state is poisoned".to_owned())
            })? = Some(transcript::TranscriptBinding::from_native_id(
                id,
                &request.launch_environment,
            )?);
        }
        let mut events = Vec::new();
        if let Err(error) = session.await_native_start(&mut events).await {
            session.hooks.close();
            session.bridge.close();
            let _ = session.terminal.stop();
            cleanup(&session.generated);
            return Err(error);
        }
        // The native terminal capability probe runs after SessionStart and consumes early
        // stdin. Let its bounded negotiation finish before accepting a canonical prompt.
        // Prompt acceptance is still confirmed by UserPromptSubmit; input is never replayed.
        if provider == ProviderKind::ClaudeCode {
            tokio::time::sleep(Duration::from_secs(2)).await;
        }
        *session.startup_events.lock().map_err(|_| {
            ProviderError::Protocol("native startup event state is poisoned".to_owned())
        })? = events;
        Ok(session)
    }

    fn id_locked(&self) -> String {
        self.session_id
            .lock()
            .map_or_else(|p| p.into_inner().clone(), |v| v.clone())
    }
    fn set_id(&self, value: String) {
        match self.session_id.lock() {
            Ok(mut id) => *id = value,
            Err(p) => *p.into_inner() = value,
        }
    }
    fn command_spec(&self) -> CommandSpec {
        let mut command = CommandSpec::new(&self.executable, &self.request.cwd);
        command.apply_environment(&self.request.launch_environment);
        command
    }

    async fn run(
        &self,
        request: &TurnRequest,
        observer: Option<&dyn TurnEventObserver>,
    ) -> Result<Vec<ProviderEvent>, ProviderError> {
        {
            let _lifecycle = self.lifecycle.lock().map_err(|_| {
                ProviderError::Protocol("native lifecycle state is poisoned".to_owned())
            })?;
            if self.stopped.load(Ordering::Acquire) {
                return Err(ProviderError::Protocol(
                    "native interactive session has stopped".to_owned(),
                ));
            }
            self.verify_generated_config()?;
            if self.active.load(Ordering::Acquire) {
                return Err(ProviderError::Protocol(
                    "native interactive session already has an active turn".to_owned(),
                ));
            }
            self.cancelled.store(false, Ordering::Release);
            self.cancellation.send_replace(false);
            self.read_only.store(request.read_only, Ordering::Release);
            *self.claude_binding.lock().map_err(|_| {
                ProviderError::Protocol("Claude binding state is poisoned".to_owned())
            })? = ClaudeBinding::default();
            self.pre_tool_ids
                .lock()
                .map_err(|_| {
                    ProviderError::Protocol("native hook replay state is poisoned".to_owned())
                })?
                .clear();
            self.antigravity_binding
                .lock()
                .map_err(|_| {
                    ProviderError::Protocol("native invocation state is poisoned".to_owned())
                })?
                .begin_turn();
            self.active.store(true, Ordering::Release);
        }
        let mut active = ActiveTurnGuard {
            active: &self.active,
            stopped: &self.stopped,
            terminal: &self.terminal,
            armed: true,
        };
        let mut result = self.run_active(request, observer).await;
        // Cancellation may interrupt prompt submission or a pending hook, before the main
        // loop reaches its cancellation branch. A stopped owned job is still cancellation
        // evidence; do not turn that race into an unrelated process failure.
        if result.is_err() && self.cancelled.load(Ordering::Acquire) {
            let event = ProviderEvent::Cancelled {
                provider: self.provider,
                session_id: self.id_locked(),
                turn_id: Some(request.correlation_id.to_string()),
                raw: None,
            };
            result = match observer
                .map(|observer| observer.observe(&event))
                .transpose()
            {
                Ok(_) => Ok(vec![event]),
                Err(error) => Err(error),
            };
        }
        if result.is_err() {
            self.stopped.store(true, Ordering::Release);
            let _ = self.terminal.stop();
        }
        active.armed = false;
        result
    }

    #[allow(clippy::too_many_lines)] // One bounded turn loop owns input, callbacks and cancellation.
    async fn run_active(
        &self,
        request: &TurnRequest,
        observer: Option<&dyn TurnEventObserver>,
    ) -> Result<Vec<ProviderEvent>, ProviderError> {
        let mut events = self
            .startup_events
            .lock()
            .map_err(|_| {
                ProviderError::Protocol("native startup event state is poisoned".to_owned())
            })?
            .drain(..)
            .collect::<Vec<_>>();
        if let Some(observer) = observer {
            for event in &events {
                observer.observe(event)?;
            }
        }
        if let Some(transcript) = self
            .transcript
            .lock()
            .map_err(|_| ProviderError::Protocol("native transcript state is poisoned".to_owned()))?
            .as_mut()
        {
            transcript.begin_turn()?;
        }
        if self.provider == ProviderKind::ClaudeCode {
            // Right after Stop, Claude Code is still closing the previous prompt: input typed
            // then joins that prompt without a UserPromptSubmit, and its next hooks carry the
            // old prompt identity. Type only once its screen has settled.
            await_quiet_screen(
                &self.terminal,
                CLAUDE_INPUT_SETTLE,
                CLAUDE_INPUT_SETTLE_LIMIT,
            )
            .await?;
        }
        write_prompt(&self.terminal, &request.prompt).await?;
        let started = Instant::now();
        let turn_id = request.correlation_id.to_string();
        let mut saw_stop = false;
        let mut prompt_accepted = false;
        let mut final_text = false;
        loop {
            if self.cancelled.load(Ordering::Acquire) {
                events.push(ProviderEvent::Cancelled {
                    provider: self.provider,
                    session_id: self.id_locked(),
                    turn_id: Some(turn_id),
                    raw: None,
                });
                if let Some(observer) = observer {
                    observer.observe(events.last().expect("cancel event inserted"))?;
                }
                return Ok(events);
            }
            if started.elapsed() >= self.request.turn_timeout() {
                return Err(ProviderError::Timeout {
                    phase: "native interactive turn",
                });
            }
            let queued = {
                let mut receiver = self.hook_rx.lock().await;
                timeout(Duration::from_millis(250), receiver.recv()).await
            };
            if let Ok(Some(queued)) = queued {
                let event_start = events.len();
                if let Some(quota) = self.hooks.take_quota(&self.id_locked()) {
                    events.push(quota);
                }
                self.handle_hook(
                    queued,
                    observer,
                    &turn_id,
                    request,
                    &mut events,
                    &mut saw_stop,
                    &mut prompt_accepted,
                    &mut final_text,
                    self.request
                        .turn_timeout()
                        .saturating_sub(started.elapsed()),
                )
                .await?;
                if let Some(observer) = observer {
                    for event in &events[event_start..] {
                        observer.observe(event)?;
                    }
                }
                if matches!(events.last(), Some(ProviderEvent::TurnCompleted { status, .. }) if status == "failed")
                {
                    return Ok(events);
                }
                if saw_stop
                    && prompt_accepted
                    && (self.provider == ProviderKind::Antigravity || final_text)
                {
                    events.push(ProviderEvent::TurnCompleted {
                        provider: self.provider,
                        session_id: self.id_locked(),
                        turn_id: Some(turn_id),
                        status: "completed".to_owned(),
                        output: None,
                        raw: None,
                    });
                    if let Some(observer) = observer {
                        observer.observe(events.last().expect("completion event inserted"))?;
                    }
                    return Ok(events);
                }
            }
            if !self
                .terminal
                .exit_status()
                .map_err(|e| {
                    ProviderError::Protocol(format!("native terminal status failed: {e}"))
                })?
                .live
            {
                return Err(ProviderError::ProcessExited { code: None });
            }
        }
    }

    #[allow(clippy::too_many_lines)] // One startup loop, read top to bottom.
    async fn await_native_start(
        &self,
        events: &mut Vec<ProviderEvent>,
    ) -> Result<(), ProviderError> {
        let mut receiver = self.hook_rx.lock().await;
        let started = Instant::now();
        let mut screen = startup::NativeStartupScreen::default();
        let mut after = 0;
        let mut trust_sent = false;
        let mut trust_selected_at: Option<Instant> = None;
        let mut last_output = Instant::now();
        let queued = loop {
            if started.elapsed() >= self.request.startup_timeout() {
                return Err(ProviderError::Timeout {
                    phase: "native interactive startup",
                });
            }
            if let Ok(Some(queued)) = timeout(Duration::from_millis(100), receiver.recv()).await {
                break queued;
            }
            let output = self
                .terminal
                .read_output(after, 64 * 1024)
                .map_err(|e| ProviderError::Protocol(format!("native startup read failed: {e}")))?;
            after = output.end;
            if !output.bytes.is_empty() {
                last_output = Instant::now();
            }
            let state = screen.consume(self.provider, &self.request.cwd, &output)?;
            if !trust_sent
                && matches!(
                    state,
                    startup::StartupState::TrustPrompt { .. }
                        | startup::StartupState::TrustSelected
                )
                && !self.request.native_workspace_trust_approved
            {
                return Err(ProviderError::Protocol(
                    "native workspace trust requires user confirmation".to_owned(),
                ));
            }
            match state {
                startup::StartupState::TrustPrompt { input }
                    if !trust_sent && self.provider == ProviderKind::Antigravity =>
                {
                    self.terminal.write_input(&input).map_err(|e| {
                        ProviderError::Protocol(format!("native trust input failed: {e}"))
                    })?;
                    tokio::time::sleep(Duration::from_millis(150)).await;
                    self.terminal.write_input(b"\r").map_err(|e| {
                        ProviderError::Protocol(format!("native trust confirmation failed: {e}"))
                    })?;
                    trust_sent = true;
                }
                // Claude Code draws its trust dialog before it reads keys and can redraw it
                // with No selected again, so an early Down is lost and Enter would choose
                // "No, exit". Act only on a settled screen: repeat the selection until Yes
                // shows, and confirm only once that has stayed on screen.
                startup::StartupState::TrustPrompt { input }
                    if !trust_sent
                        && last_output.elapsed() >= TRUST_SETTLE
                        && trust_selected_at
                            .is_none_or(|at| at.elapsed() >= TRUST_SELECTION_RETRY) =>
                {
                    self.terminal.write_input(&input).map_err(|e| {
                        ProviderError::Protocol(format!("native trust input failed: {e}"))
                    })?;
                    trust_selected_at = Some(Instant::now());
                }
                startup::StartupState::TrustSelected
                    if !trust_sent && last_output.elapsed() >= TRUST_SETTLE =>
                {
                    self.terminal.write_input(b"\r").map_err(|e| {
                        ProviderError::Protocol(format!("native trust confirmation failed: {e}"))
                    })?;
                    trust_sent = true;
                }
                startup::StartupState::Ready if self.provider == ProviderKind::Antigravity => {
                    // The native CLI creates its conversation on the first prompt. The
                    // authenticated SessionStart/PreInvocation callbacks bind that id later.
                    return Ok(());
                }
                _ => {}
            }
            let status = self.terminal.exit_status().map_err(|e| {
                ProviderError::Protocol(format!("native startup status failed: {e}"))
            })?;
            if !status.live {
                return Err(ProviderError::ProcessExited {
                    code: status
                        .exit_code
                        .map(|code| i32::from_ne_bytes(code.to_ne_bytes())),
                });
            }
        };
        drop(receiver);
        if queued.event.event != "SessionStart" {
            let _ = queued.reply.send(native_decision(
                self.provider,
                &queued.event.event,
                crate::PermissionDecision::Decline,
            ));
            return Err(ProviderError::Protocol(
                "native hook arrived before SessionStart".to_owned(),
            ));
        }
        let mut ignored_stop = false;
        let mut ignored_prompt = false;
        let mut ignored_final = false;
        self.handle_hook(
            queued,
            None,
            "startup",
            &TurnRequest::new("startup"),
            events,
            &mut ignored_stop,
            &mut ignored_prompt,
            &mut ignored_final,
            Duration::ZERO,
        )
        .await
    }

    #[allow(clippy::too_many_lines, clippy::too_many_arguments)] // Exhaustive native event transition with shared turn state and one reply owner.
    async fn handle_hook(
        &self,
        queued: QueuedHook,
        observer: Option<&dyn TurnEventObserver>,
        turn_id: &str,
        turn: &TurnRequest,
        events: &mut Vec<ProviderEvent>,
        saw_stop: &mut bool,
        prompt_accepted: &mut bool,
        final_text: &mut bool,
        approval_timeout: Duration,
    ) -> Result<(), ProviderError> {
        let NativeHookEvent { event, payload } = queued.event;
        self.verify_generated_config()?;
        let native_id = native_session_id(&payload).ok_or_else(|| {
            ProviderError::Protocol("native hook omitted session identity".to_owned())
        })?;
        let existing = self.id_locked();
        if self.provider == ProviderKind::Antigravity
            && matches!(event.as_str(), "SessionStart" | "PreInvocation")
        {
            self.bind_antigravity_paths(&payload)?;
        }
        if existing.starts_with("pending:") && event == "SessionStart" {
            if self.provider == ProviderKind::Antigravity && observer.is_some() {
                self.bridge.rebind_pending_session(&existing, &native_id)?;
            }
            self.set_id(native_id.clone());
            events.push(ProviderEvent::SessionStarted {
                provider: self.provider,
                session_id: native_id.clone(),
                resumed: self.request.session_id.is_some(),
                raw: None,
            });
        } else if existing != native_id {
            let _ = queued.reply.send(native_decision(
                self.provider,
                &event,
                crate::PermissionDecision::Decline,
            ));
            return Err(ProviderError::Protocol(
                "stale native hook session identity".to_owned(),
            ));
        }
        if self.provider == ProviderKind::Antigravity && event != "SessionStart" {
            if payload.get("lastUserInput").and_then(Value::as_str) != Some(turn.prompt.as_str()) {
                let _ = queued
                    .reply
                    .send(json!({"decision":"deny","reason":"Lazoo canonical prompt mismatch"}));
                return Err(ProviderError::Protocol(
                    "stale Antigravity prompt identity".to_owned(),
                ));
            }
            if !*prompt_accepted && event != "PreInvocation" {
                return Err(ProviderError::Protocol(
                    "Antigravity hook preceded canonical prompt acceptance".to_owned(),
                ));
            }
            if matches!(event.as_str(), "PreToolUse" | "PostToolUse") {
                self.antigravity_binding
                    .lock()
                    .map_err(|_| {
                        ProviderError::Protocol("native invocation state is poisoned".to_owned())
                    })?
                    .require_tool(&payload)?;
            }
        }
        if self.provider == ProviderKind::ClaudeCode
            && matches!(
                event.as_str(),
                "MessageDisplay"
                    | "PreToolUse"
                    | "PermissionRequest"
                    | "PostToolUse"
                    | "PostToolUseFailure"
                    | "Stop"
                    | "StopFailure"
            )
        {
            self.claude_binding
                .lock()
                .map_err(|_| {
                    ProviderError::Protocol("Claude binding state is poisoned".to_owned())
                })?
                .require_prompt(&payload)?;
        }
        match event.as_str() {
            "UserPromptSubmit" => {
                if self.provider == ProviderKind::ClaudeCode {
                    if !matches!(
                        payload.get("source").and_then(Value::as_str),
                        None | Some("user")
                    ) {
                        return Err(ProviderError::Protocol(
                            "foreign Claude UserPromptSubmit refused".to_owned(),
                        ));
                    }
                    let prompt_id = claude_prompt_id(&payload)?;
                    *self.claude_binding.lock().map_err(|_| {
                        ProviderError::Protocol("Claude binding state is poisoned".to_owned())
                    })? = ClaudeBinding {
                        prompt_id: Some(prompt_id),
                        ..ClaudeBinding::default()
                    };
                }
                let reported = payload.get("prompt").and_then(Value::as_str);
                if !reported.is_some_and(|reported| {
                    reported == turn.prompt
                        || (self.provider == ProviderKind::ClaudeCode
                            && claude_pasted_prompt(reported) == Some(turn.prompt.as_str()))
                }) {
                    let _ = queued.reply.send(
                        json!({"decision":"block","reason":"Lazoo canonical prompt mismatch"}),
                    );
                    return Err(ProviderError::Protocol(
                        "native prompt hook did not match the canonical prompt".to_owned(),
                    ));
                }
                *prompt_accepted = true;
                events.push(ProviderEvent::TurnStarted {
                    provider: self.provider,
                    session_id: native_id.clone(),
                    turn_id: Some(turn_id.to_owned()),
                    raw: None,
                });
                events.push(ProviderEvent::Progress {
                    provider: self.provider,
                    session_id: native_id,
                    message: Some("native prompt accepted".to_owned()),
                    raw: None,
                });
                let _ = queued.reply.send(json!({}));
            }
            "PreInvocation" if self.provider == ProviderKind::Antigravity => {
                self.antigravity_binding
                    .lock()
                    .map_err(|_| {
                        ProviderError::Protocol("native invocation state is poisoned".to_owned())
                    })?
                    .accept_invocation(&payload)?;
                if payload.get("lastUserInput").and_then(Value::as_str)
                    != Some(turn.prompt.as_str())
                {
                    let _ = queued.reply.send(
                        json!({"decision":"deny","reason":"Lazoo canonical prompt mismatch"}),
                    );
                    return Err(ProviderError::Protocol(
                        "Antigravity invocation did not match canonical prompt".to_owned(),
                    ));
                }
                if !*prompt_accepted {
                    *prompt_accepted = true;
                    events.push(ProviderEvent::TurnStarted {
                        provider: self.provider,
                        session_id: native_id,
                        turn_id: Some(turn_id.to_owned()),
                        raw: None,
                    });
                }
                let _ = queued.reply.send(json!({}));
            }
            "MessageDisplay" if self.provider == ProviderKind::ClaudeCode => {
                if !*prompt_accepted {
                    let _ = queued.reply.send(json!({}));
                    return Err(ProviderError::Protocol(
                        "Claude text arrived before canonical prompt acceptance".to_owned(),
                    ));
                }
                let delta = payload
                    .get("delta")
                    .and_then(Value::as_str)
                    .filter(|value| value.len() <= 16 * 1024)
                    .ok_or_else(|| {
                        ProviderError::Protocol(
                            "Claude MessageDisplay omitted bounded delta".to_owned(),
                        )
                    })?;
                self.claude_binding
                    .lock()
                    .map_err(|_| {
                        ProviderError::Protocol("Claude binding state is poisoned".to_owned())
                    })?
                    .accept_message(&payload)?;
                *final_text |= payload.get("final").and_then(Value::as_bool) == Some(true);
                events.push(ProviderEvent::TextDelta {
                    provider: self.provider,
                    session_id: native_id,
                    turn_id: Some(turn_id.to_owned()),
                    text: delta.to_owned(),
                    raw: None,
                });
                let _ = queued.reply.send(json!({}));
            }
            "PreToolUse" => {
                if !*prompt_accepted {
                    let _ = queued.reply.send(native_decision(
                        self.provider,
                        "PreToolUse",
                        crate::PermissionDecision::Decline,
                    ));
                    return Err(ProviderError::Protocol(
                        "native tool hook arrived before canonical prompt acceptance".to_owned(),
                    ));
                }
                let request_id = native_tool_id(&payload).ok_or_else(|| {
                    ProviderError::Protocol("native tool hook omitted tool identity".to_owned())
                })?;
                events.push(ProviderEvent::ToolUpdate {
                    provider: self.provider,
                    session_id: native_id.clone(),
                    turn_id: Some(turn_id.to_owned()),
                    tool_name: native_tool_name(&payload),
                    status: "permission-requested".to_owned(),
                    raw: None,
                });
                let fresh = {
                    let mut seen = self.pre_tool_ids.lock().map_err(|_| {
                        ProviderError::Protocol("native hook replay state is poisoned".to_owned())
                    })?;
                    if seen.len() >= MAX_PRE_TOOL_IDS {
                        let _ = queued.reply.send(native_decision(
                            self.provider,
                            "PreToolUse",
                            crate::PermissionDecision::Decline,
                        ));
                        return Err(ProviderError::Protocol(
                            "native tool replay capacity exceeded".to_owned(),
                        ));
                    }
                    seen.insert(request_id.clone())
                };
                if !fresh {
                    let _ = queued.reply.send(native_decision(
                        self.provider,
                        "PreToolUse",
                        crate::PermissionDecision::Decline,
                    ));
                    return Err(ProviderError::Protocol(
                        "replayed native tool hook".to_owned(),
                    ));
                }
                let approval = PermissionRequest {
                    provider: self.provider,
                    session_id: self.id_locked(),
                    turn_id: Some(turn_id.to_owned()),
                    request_id: request_id.clone(),
                    kind: native_permission_kind(&payload),
                    action: native_tool_name(&payload).unwrap_or_else(|| "native-tool".to_owned()),
                    scope_expansion: native_scope_expansion(&payload, &self.request.cwd),
                    detail: redact_payload(&payload),
                };
                events.push(ProviderEvent::PermissionRequested {
                    provider: self.provider,
                    session_id: self.id_locked(),
                    request_id: Some(request_id),
                    options: vec!["approve".to_owned(), "decline".to_owned()],
                    raw: None,
                });
                let lazoo_tool = self.is_declared_tool_call(&payload);
                let decision = if (!lazoo_tool && !known_native_tool(&payload))
                    || (self.read_only.load(Ordering::Acquire) && !native_read_only_safe(&payload))
                {
                    crate::PermissionDecision::Decline
                } else if lazoo_tool || native_tool_name(&payload).as_deref() == Some("ToolSearch")
                {
                    crate::PermissionDecision::Approve
                } else {
                    match observer {
                        Some(observer) => tokio::select! {
                            decision = timeout(approval_timeout, observer.decide_permission(&approval)) => decision.ok().and_then(Result::ok).unwrap_or(crate::PermissionDecision::Decline),
                            () = cancelled(&self.cancellation) => crate::PermissionDecision::Decline,
                        },
                        None => crate::PermissionDecision::Decline,
                    }
                };
                let _ = queued
                    .reply
                    .send(native_decision(self.provider, "PreToolUse", decision));
            }
            "PermissionRequest" if self.provider == ProviderKind::ClaudeCode => {
                let request_id =
                    native_tool_id(&payload).unwrap_or_else(|| queued.request_id.clone());
                let approval = PermissionRequest {
                    provider: self.provider,
                    session_id: self.id_locked(),
                    turn_id: Some(turn_id.to_owned()),
                    request_id,
                    kind: native_permission_kind(&payload),
                    action: native_tool_name(&payload)
                        .unwrap_or_else(|| "native-permission".to_owned()),
                    scope_expansion: native_scope_expansion(&payload, &self.request.cwd),
                    detail: redact_payload(&payload),
                };
                let lazoo_tool = self.is_declared_tool_call(&payload);
                let decision = if (!lazoo_tool && !known_native_tool(&payload))
                    || (self.read_only.load(Ordering::Acquire) && !native_read_only_safe(&payload))
                {
                    crate::PermissionDecision::Decline
                } else {
                    match observer {
                        Some(observer) => tokio::select! {
                            decision = timeout(approval_timeout, observer.decide_permission(&approval)) => decision.ok().and_then(Result::ok).unwrap_or(crate::PermissionDecision::Decline),
                            () = cancelled(&self.cancellation) => crate::PermissionDecision::Decline,
                        },
                        None => crate::PermissionDecision::Decline,
                    }
                };
                let _ = queued.reply.send(native_decision(
                    self.provider,
                    "PermissionRequest",
                    decision,
                ));
            }
            "PostToolUse" => {
                events.push(ProviderEvent::ToolUpdate {
                    provider: self.provider,
                    session_id: self.id_locked(),
                    turn_id: Some(turn_id.to_owned()),
                    tool_name: native_tool_name(&payload),
                    status: "completed".to_owned(),
                    raw: None,
                });
                let _ = queued.reply.send(json!({}));
            }
            "Stop" => {
                if self.provider == ProviderKind::Antigravity
                    && payload.get("fullyIdle").and_then(Value::as_bool) != Some(true)
                {
                    let _ = queued.reply.send(json!({}));
                    return Err(ProviderError::Protocol(
                        "native Stop did not prove fullyIdle completion".to_owned(),
                    ));
                }
                if self.provider == ProviderKind::Antigravity {
                    if payload
                        .get("error")
                        .and_then(Value::as_str)
                        .is_some_and(|error| !error.is_empty())
                    {
                        let _ = queued.reply.send(json!({}));
                        return Err(ProviderError::Protocol(
                            "Antigravity reported a native turn failure".to_owned(),
                        ));
                    }
                    // executionNum is local to the native execution loop and resets on each
                    // user turn. The canonical prompt plus the new transcript tail bind Stop.
                    if payload
                        .get("executionNum")
                        .and_then(Value::as_u64)
                        .is_none()
                    {
                        return Err(ProviderError::Protocol(
                            "Antigravity Stop omitted execution identity".to_owned(),
                        ));
                    }
                    if !matches!(
                        payload.get("terminationReason").and_then(Value::as_str),
                        Some("NO_TOOL_CALL" | "TERMINAL_STEP_TYPE" | "INJECTED_RESPONSE")
                    ) {
                        return Err(ProviderError::Protocol(
                            "Antigravity did not complete the native turn normally".to_owned(),
                        ));
                    }
                }
                *saw_stop = true;
                if self.provider == ProviderKind::Antigravity
                    && let Some(text) = self.antigravity_transcript_text(&payload, &turn.prompt)?
                {
                    events.push(ProviderEvent::TextDelta {
                        provider: self.provider,
                        session_id: self.id_locked(),
                        turn_id: Some(turn_id.to_owned()),
                        text,
                        raw: None,
                    });
                }
                events.push(ProviderEvent::Progress {
                    provider: self.provider,
                    session_id: self.id_locked(),
                    message: Some("native stop observed".to_owned()),
                    raw: None,
                });
                let _ = queued.reply.send(json!({}));
            }
            "StopFailure" if self.provider == ProviderKind::ClaudeCode => {
                let _ = queued.reply.send(json!({}));
                if payload.get("error").and_then(Value::as_str) == Some("rate_limit") {
                    events.push(ProviderEvent::Quota {
                        provider: self.provider,
                        session_id: self.id_locked(),
                        windows: Vec::new(),
                        exhausted: true,
                        raw: None,
                    });
                }
                events.push(ProviderEvent::TurnCompleted {
                    provider: self.provider,
                    session_id: self.id_locked(),
                    turn_id: Some(turn_id.to_owned()),
                    status: "failed".into(),
                    output: None,
                    raw: None,
                });
            }
            _ => {
                let _ = queued.reply.send(json!({}));
            }
        }
        Ok(())
    }

    fn is_declared_tool_call(&self, payload: &Value) -> bool {
        let Some(name) = native_tool_name(payload) else {
            return false;
        };
        if self.provider == ProviderKind::ClaudeCode {
            return name
                .strip_prefix("mcp__lazoo__")
                .is_some_and(|tool| self.declared_tools.contains(tool));
        }
        if matches!(name.as_str(), "call_mcp_tool" | "mcp_tool") {
            let args = &payload["toolCall"]["args"];
            return args["ServerName"] == "lazoo"
                && args["ToolName"]
                    .as_str()
                    .is_some_and(|tool| self.declared_tools.contains(tool));
        }
        false
    }

    fn verify_generated_config(&self) -> Result<(), ProviderError> {
        for file in &self.generated {
            if digest(&fs::read(&file.path)?) != file.digest {
                return Err(ProviderError::Protocol(
                    "native control configuration changed during the session".to_owned(),
                ));
            }
        }
        Ok(())
    }

    fn bind_antigravity_paths(&self, payload: &Value) -> Result<(), ProviderError> {
        let id = payload
            .get("conversationId")
            .and_then(Value::as_str)
            .ok_or_else(|| {
                ProviderError::Protocol("Antigravity conversation identity missing".to_owned())
            })?;
        let workspaces = payload
            .get("workspacePaths")
            .and_then(Value::as_array)
            .ok_or_else(|| {
                ProviderError::Protocol("Antigravity workspace identity missing".to_owned())
            })?;
        let cwd = fs::canonicalize(&self.request.cwd)?;
        if workspaces.len() != 1
            || workspaces[0]
                .as_str()
                .and_then(|path| fs::canonicalize(path).ok())
                .as_ref()
                != Some(&cwd)
        {
            return Err(ProviderError::Protocol(
                "Antigravity workspace does not match canonical workspace".to_owned(),
            ));
        }
        let mut bound = self.transcript.lock().map_err(|_| {
            ProviderError::Protocol("native transcript state is poisoned".to_owned())
        })?;
        if bound.is_none() {
            *bound = Some(transcript::TranscriptBinding::from_native_id(
                id,
                &self.request.launch_environment,
            )?);
        }
        bound
            .as_ref()
            .expect("transcript inserted")
            .validate_paths(payload)
    }

    fn antigravity_transcript_text(
        &self,
        payload: &Value,
        prompt: &str,
    ) -> Result<Option<String>, ProviderError> {
        let bound = self.transcript.lock().map_err(|_| {
            ProviderError::Protocol("native transcript state is poisoned".to_owned())
        })?;
        let bound = bound
            .as_ref()
            .ok_or_else(|| ProviderError::Protocol("native transcript was not bound".to_owned()))?;
        bound.validate_paths(payload)?;
        bound.text(prompt)
    }
}

impl ProviderSession for NativeInteractiveSession {
    fn take_usage_windows(&self) -> Vec<lazoo_protocol::UsageWindow> {
        match self.hooks.take_quota(&self.id_locked()) {
            Some(ProviderEvent::Quota { windows, .. }) => windows,
            _ => Vec::new(),
        }
    }
    fn id(&self) -> String {
        self.id_locked()
    }
    fn native_host_pid(&self) -> Option<u32> {
        Some(self.terminal.pid())
    }
    fn resources(&self) -> Option<lazoo_runtime::JobAccounting> {
        self.terminal.resources().ok()
    }
    fn native_view_available(&self) -> bool {
        !self.stopped.load(Ordering::Acquire)
            && self.terminal.exit_status().is_ok_and(|status| status.live)
    }
    fn open_native_view(&self) -> AdapterFuture<'_, Option<crate::NativeViewLease>> {
        Box::pin(async move {
            if !self.native_view_available() {
                return Ok(None);
            }
            Ok(Some(crate::NativeViewLease::borrow_terminal(
                &self.terminal,
                self.command_spec(),
            )))
        })
    }
    fn turn<'a>(&'a self, request: &'a TurnRequest) -> AdapterFuture<'a, Vec<ProviderEvent>> {
        Box::pin(async move { self.run(request, None).await })
    }
    fn turn_observed<'a>(
        &'a self,
        request: &'a TurnRequest,
        observer: &'a dyn TurnEventObserver,
    ) -> AdapterFuture<'a, Vec<ProviderEvent>> {
        Box::pin(async move {
            let events = self
                .bridge
                .during_turn(
                    &self.id_locked(),
                    Some(&request.correlation_id.to_string()),
                    observer,
                    self.run(request, Some(observer)),
                )
                .await?;
            Ok(events)
        })
    }
    fn cancel(&self) -> AdapterFuture<'_, ()> {
        Box::pin(async move {
            let _lifecycle = self.lifecycle.lock().map_err(|_| {
                ProviderError::Protocol("native lifecycle state is poisoned".to_owned())
            })?;
            if self.active.load(Ordering::Acquire) && !self.cancelled.swap(true, Ordering::AcqRel) {
                self.cancellation.send_replace(true);
                self.stopped.store(true, Ordering::Release);
                self.terminal.stop().map_err(|e| {
                    ProviderError::Protocol(format!("native terminal cancel failed: {e}"))
                })?;
            }
            Ok(())
        })
    }
    fn stop(&self) -> AdapterFuture<'_, ()> {
        Box::pin(async move {
            self.stopped.store(true, Ordering::Release);
            self.hooks.close();
            self.bridge.close();
            let _ = self.terminal.stop();
            cleanup(&self.generated);
            Ok(())
        })
    }
}

impl Drop for NativeInteractiveSession {
    fn drop(&mut self) {
        self.hooks.close();
        self.bridge.close();
        cleanup(&self.generated);
    }
}

fn validate_request(provider: ProviderKind, request: &SessionRequest) -> Result<(), ProviderError> {
    if !request.cwd.is_absolute() {
        return Err(ProviderError::Protocol(
            "interactive workspace must be absolute".to_owned(),
        ));
    }
    if provider == ProviderKind::Antigravity && !request.managed_workspace {
        return Err(ProviderError::Unsupported {
            provider,
            capability: "managed Antigravity workspace hooks outside a Lazoo-owned disposable worktree",
            detail: Some(request.cwd.display().to_string()),
        });
    }
    Ok(())
}
fn interactive_command(
    provider: ProviderKind,
    mut command: TerminalCommand,
    request: &SessionRequest,
    generated: &[GeneratedFile],
) -> TerminalCommand {
    match provider {
        ProviderKind::ClaudeCode => {
            command = command
                .env("DISABLE_AUTOUPDATER", "1")
                .env("CLAUDE_CODE_DISABLE_BACKGROUND_TASKS", "1")
                .args([
                    "--restricted",
                    "--tools",
                    "Bash,Read,Write,Edit,Glob,Grep,ToolSearch",
                    "--strict-mcp-config",
                    "--permission-mode",
                    "manual",
                    "--no-chrome",
                    "--settings",
                ])
                .arg(generated[0].path.as_os_str())
                .arg("--mcp-config")
                .arg(generated[1].path.as_os_str());
            if let Some(id) = &request.session_id {
                command = command.arg("--resume").arg(id);
            }
            if let Some(model) = &request.model {
                command = command.arg("--model").arg(model);
            }
            if let Some(effort) = &request.effort {
                command = command.arg("--effort").arg(effort);
            }
        }
        ProviderKind::Antigravity => {
            command = command
                .env("AGY_CLI_DISABLE_AUTO_UPDATE", "true")
                .arg("--new-project");
            if let Some(id) = &request.session_id {
                command = command.arg("--conversation").arg(id);
            }
            if let Some(model) = &request.model {
                command = command.arg("--model").arg(model);
            }
            if let Some(effort) = &request.effort {
                command = command.arg("--effort").arg(effort);
            }
        }
        _ => unreachable!(),
    }
    command
}
fn prepare_generated_config(
    provider: ProviderKind,
    executable: &Path,
    request: &SessionRequest,
    _hooks: &NativeHookServer,
    bridge: &SessionMcpBridge,
) -> Result<Vec<PathBuf>, ProviderError> {
    let ctl = sibling_lazooctl(executable)?;
    let command = format!("\"{}\" __native-hook {}", ctl.display(), provider.as_str());
    match provider {
        ProviderKind::ClaudeCode => {
            let root = std::env::temp_dir().join(format!("lazoo-native-{}", Uuid::new_v4()));
            fs::create_dir_all(&root)?;
            let settings = root.join("settings.json");
            let mcp = root.join("mcp.json");
            let hook = |event: &str| json!({"type":"command","command":format!("{command} {event}"),"timeout":600,"onFailure":"block"});
            let hooks_json = json!({"hooks":{"SessionStart":[{"hooks":[hook("SessionStart")] }],"UserPromptSubmit":[{"hooks":[hook("UserPromptSubmit")] }],"MessageDisplay":[{"hooks":[hook("MessageDisplay")] }],"PreToolUse":[{"matcher":"*","hooks":[hook("PreToolUse")] }],"PostToolUse":[{"matcher":"*","hooks":[hook("PostToolUse")] }],"Stop":[{"hooks":[hook("Stop")] }],"StopFailure":[{"hooks":[hook("StopFailure")] }]}});
            let mut hooks_json = hooks_json;
            hooks_json["statusLine"] =
                json!({"type":"command","command":format!("{command} StatusLine")});
            hooks_json["disableRemoteControl"] = json!(true);
            hooks_json["remoteControlAtStartup"] = json!(false);
            fs::write(&settings, serde_json::to_vec(&hooks_json)?)?;
            fs::write(
                &mcp,
                serde_json::to_vec(
                    &json!({"mcpServers":{"lazoo":{"type":"http","url":bridge.endpoint(),"headers":{"Authorization":bridge.authorization()}}}}),
                )?,
            )?;
            Ok(vec![settings, mcp])
        }
        ProviderKind::Antigravity => {
            let dir = request.cwd.join(".agents");
            fs::create_dir_all(&dir)?;
            if fs::canonicalize(&dir)?.parent() != Some(fs::canonicalize(&request.cwd)?.as_path()) {
                return Err(ProviderError::Protocol(
                    "native configuration directory escapes managed workspace".to_owned(),
                ));
            }
            let hooks_file = dir.join("hooks.json");
            let mcp_file = dir.join("mcp_config.json");
            if hooks_file.exists() || mcp_file.exists() {
                return Err(ProviderError::Unsupported {
                    provider,
                    capability: "overwriting Antigravity workspace configuration",
                    detail: Some(dir.display().to_string()),
                });
            }
            // The installed Windows CLI escapes quoted executables incorrectly through cmd /c.
            // An encoded PowerShell invocation has no shell metacharacters in its argument,
            // preserves JSON stdin, and supports spaces/unicode in the protected helper path.
            let agy_hook = |event: &str| -> Result<Value, ProviderError> {
                Ok(
                    json!({"type":"command","command":antigravity_hook_command(&ctl,event)?,"timeout":600}),
                )
            };
            fs::write(
                &hooks_file,
                serde_json::to_vec(
                    &json!({"lazoo":{"SessionStart":[agy_hook("SessionStart")?],"PreInvocation":[agy_hook("PreInvocation")?],"PostInvocation":[agy_hook("PostInvocation")?],"PreToolUse":[{"matcher":"*","hooks":[agy_hook("PreToolUse")?]}],"PostToolUse":[{"matcher":"*","hooks":[agy_hook("PostToolUse")?] }],"Stop":[agy_hook("Stop")?]}}),
                )?,
            )?;
            fs::write(
                &mcp_file,
                serde_json::to_vec(
                    &json!({"mcpServers":{"lazoo":{"url":bridge.endpoint(),"headers":{"Authorization":bridge.authorization()}}}}),
                )?,
            )?;
            Ok(vec![hooks_file, mcp_file])
        }
        _ => unreachable!(),
    }
}
fn antigravity_hook_command(helper: &Path, event: &str) -> Result<String, ProviderError> {
    let system_root = std::env::var("SystemRoot")
        .map_err(|_| ProviderError::Protocol("SystemRoot is unavailable".to_owned()))?;
    let powershell =
        PathBuf::from(system_root).join("System32/WindowsPowerShell/v1.0/powershell.exe");
    if powershell
        .to_string_lossy()
        .chars()
        .any(|c| c.is_whitespace() || matches!(c, '&' | '|' | '%' | '!' | '"'))
    {
        return Err(ProviderError::Protocol(
            "native hook shell path requires unsupported cmd quoting".to_owned(),
        ));
    }
    let path = helper.to_string_lossy().replace('\'', "''");
    let script = format!("& '{path}' __native-hook antigravity {event}; exit $LASTEXITCODE");
    let bytes: Vec<u8> = script.encode_utf16().flat_map(u16::to_le_bytes).collect();
    let encoded = encode_base64(&bytes);
    Ok(format!(
        "{} -NoProfile -NonInteractive -EncodedCommand {encoded}",
        powershell.display()
    ))
}

fn encode_base64(bytes: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut encoded = String::new();
    for chunk in bytes.chunks(3) {
        let value = (u32::from(chunk[0]) << 16)
            | (u32::from(*chunk.get(1).unwrap_or(&0)) << 8)
            | u32::from(*chunk.get(2).unwrap_or(&0));
        for shift in [18, 12, 6, 0] {
            encoded.push(ALPHABET[((value >> shift) & 63) as usize] as char);
        }
        for _ in chunk.len()..3 {
            encoded.pop();
        }
        for _ in chunk.len()..3 {
            encoded.push('=');
        }
    }
    encoded
}
fn sibling_lazooctl(_executable: &Path) -> Result<PathBuf, ProviderError> {
    if let Some(value) = std::env::var_os("LAZOO_NATIVE_HOOK_HELPER") {
        return Ok(PathBuf::from(value));
    }
    let directory = std::env::current_exe()?
        .parent()
        .map(Path::to_path_buf)
        .ok_or_else(|| {
            ProviderError::Protocol("cannot locate lazooctl beside current executable".to_owned())
        })?;
    let name = if cfg!(windows) {
        "lazooctl.exe"
    } else {
        "lazooctl"
    };
    let path = directory.join(name);
    if path.is_file() {
        Ok(path)
    } else {
        Err(ProviderError::Protocol(format!(
            "native hook helper not found beside daemon: {}",
            path.display()
        )))
    }
}
/// The prompt inside Claude Code's paste wrapper. Claude Code 2.1.316 reports a bracketed
/// paste (every multi-line prompt) to `UserPromptSubmit` as
/// `\n\n<pasted_content id="N">\n{prompt}\n</pasted_content id="N">\n`, where N is a short hex
/// id (seen: `2864`, `78ea`); nothing looser matches.
fn claude_pasted_prompt(reported: &str) -> Option<&str> {
    let rest = reported.strip_prefix("\n\n<pasted_content id=\"")?;
    let (id, rest) = rest.split_once("\">\n")?;
    if id.is_empty() || id.len() > 32 || !id.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return None;
    }
    rest.strip_suffix(&format!("\n</pasted_content id=\"{id}\">\n"))
}
/// Waits until the native screen has written nothing for `quiet`, or until `limit` passes.
async fn await_quiet_screen(
    terminal: &TerminalSession,
    quiet: Duration,
    limit: Duration,
) -> Result<(), ProviderError> {
    let screen_end = || {
        terminal
            .read_output(u64::MAX, 0)
            .map(|output| output.end)
            .map_err(|e| ProviderError::Protocol(format!("native screen read failed: {e}")))
    };
    let started = Instant::now();
    let mut end = screen_end()?;
    let mut quiet_since = Instant::now();
    while quiet_since.elapsed() < quiet && started.elapsed() < limit {
        tokio::time::sleep(Duration::from_millis(50)).await;
        let current = screen_end()?;
        if current != end {
            end = current;
            quiet_since = Instant::now();
        }
    }
    Ok(())
}
async fn write_prompt(terminal: &TerminalSession, prompt: &str) -> Result<(), ProviderError> {
    validate_prompt(prompt)?;
    let mut bytes = Vec::new();
    if prompt.contains('\n') {
        bytes.extend_from_slice(b"\x1b[200~");
        bytes.extend_from_slice(prompt.as_bytes());
        bytes.extend_from_slice(b"\x1b[201~");
    } else {
        bytes.extend_from_slice(prompt.as_bytes());
    }
    terminal
        .write_input(bytes)
        .map_err(|e| ProviderError::Protocol(format!("native terminal input failed: {e}")))?;
    // Native TUIs classify a multi-character input chunk as pasted text. Submit separately.
    tokio::time::sleep(Duration::from_millis(250)).await;
    terminal
        .write_input(b"\r")
        .map_err(|e| ProviderError::Protocol(format!("native terminal submit failed: {e}")))
}

fn validate_prompt(prompt: &str) -> Result<(), ProviderError> {
    if prompt.is_empty()
        || prompt.len() > MAX_PROMPT
        || prompt
            .chars()
            .any(|c| c.is_control() && !matches!(c, '\n' | '\t'))
        || prompt.trim_start().starts_with(['/', '!'])
    {
        return Err(ProviderError::Protocol(
            "interactive prompt contains reserved native commands, control bytes, or exceeds the limit".to_owned(),
        ));
    }
    Ok(())
}
fn native_session_id(value: &Value) -> Option<String> {
    for key in ["session_id", "sessionId", "conversationId"] {
        if let Some(v) = value
            .get(key)
            .and_then(Value::as_str)
            .filter(|v| !v.is_empty() && v.len() <= 200)
        {
            return Some(v.to_owned());
        }
    }
    None
}
fn native_tool_id(value: &Value) -> Option<String> {
    for key in ["tool_use_id", "toolUseId", "toolCallId", "id"] {
        if let Some(v) = value
            .get(key)
            .and_then(Value::as_str)
            .filter(|v| !v.is_empty() && v.len() <= 200)
        {
            return Some(v.to_owned());
        }
    }
    let conversation = value.get("conversationId").and_then(Value::as_str)?;
    let step = value.get("stepIdx").and_then(Value::as_u64)?;
    let name = value
        .pointer("/toolCall/name")
        .or_else(|| value.pointer("/toolCall/toolName"))
        .and_then(Value::as_str)?;
    Some(format!("{conversation}:{step}:{name}"))
}
fn native_tool_name(value: &Value) -> Option<String> {
    value
        .get("tool_name")
        .or_else(|| value.pointer("/toolCall/name"))
        .or_else(|| value.pointer("/toolCall/toolName"))
        .and_then(Value::as_str)
        .filter(|v| v.len() <= 200)
        .map(str::to_owned)
}
fn redact_payload(value: &Value) -> Value {
    crate::redact_json(value.clone())
}
fn native_permission_kind(payload: &Value) -> PermissionKind {
    let tool = native_tool_name(payload)
        .unwrap_or_default()
        .to_ascii_lowercase();
    if tool.contains("write")
        || tool.contains("edit")
        || tool.contains("patch")
        || tool.contains("delete")
        || tool.contains("move")
        || tool.contains("replace")
    {
        PermissionKind::FileChange
    } else {
        PermissionKind::CommandExecution
    }
}
fn native_scope_expansion(payload: &Value, workspace: &Path) -> bool {
    let tool = native_tool_name(payload)
        .unwrap_or_default()
        .to_ascii_lowercase();
    if tool == "bash"
        || tool.contains("command")
        || tool.contains("shell")
        || tool.contains("terminal")
        || tool.contains("mcp")
    {
        // These native hooks do not establish a filesystem sandbox for arbitrary commands.
        return true;
    }
    let paths = [
        "/tool_input/file_path",
        "/tool_input/path",
        "/toolCall/args/TargetFile",
        "/toolCall/args/AbsolutePath",
        "/toolCall/args/path",
    ]
    .into_iter()
    .filter_map(|key| payload.pointer(key).and_then(Value::as_str))
    .collect::<Vec<_>>();
    if paths.is_empty() {
        return native_permission_kind(payload) == PermissionKind::FileChange;
    }
    paths
        .iter()
        .any(|path| !path_within_workspace(path, workspace))
}

fn path_within_workspace(path: &str, workspace: &Path) -> bool {
    let Ok(root) = workspace.canonicalize() else {
        return false;
    };
    let path = Path::new(path);
    let candidate = if path.is_absolute() {
        path.to_owned()
    } else {
        workspace.join(path)
    };
    // Canonicalize the nearest existing ancestor so new files are supported and junctions
    // cannot hide an external destination. Unresolved `..` must never be assumed in scope.
    if candidate
        .components()
        .any(|part| part == std::path::Component::ParentDir)
    {
        return false;
    }
    candidate
        .ancestors()
        .find(|ancestor| ancestor.exists())
        .and_then(|ancestor| ancestor.canonicalize().ok())
        .is_some_and(|ancestor| ancestor.starts_with(root))
}
fn native_read_only_safe(payload: &Value) -> bool {
    let tool = native_tool_name(payload)
        .unwrap_or_default()
        .to_ascii_lowercase();
    matches!(
        tool.as_str(),
        "read_file"
            | "view_file"
            | "read"
            | "glob"
            | "grep"
            | "grep_search"
            | "find_by_name"
            | "list_dir"
            | "search"
            | "list_files"
            | "ls"
            | "toolsearch"
    )
}
fn known_native_tool(payload: &Value) -> bool {
    matches!(
        native_tool_name(payload)
            .unwrap_or_default()
            .to_ascii_lowercase()
            .as_str(),
        "read_file"
            | "view_file"
            | "grep_search"
            | "find_by_name"
            | "list_dir"
            | "write_to_file"
            | "replace_file_content"
            | "multi_replace_file_content"
            | "read"
            | "glob"
            | "grep"
            | "search"
            | "list_files"
            | "ls"
            | "write_file"
            | "write"
            | "edit"
            | "multiedit"
            | "notebookedit"
            | "toolsearch"
            | "edit_file"
            | "patch"
            | "delete_file"
            | "move_file"
            | "run_command"
            | "bash"
            | "shell"
            | "terminal"
    )
}
fn seal_generated(path: PathBuf) -> Result<GeneratedFile, ProviderError> {
    let bytes = fs::read(&path)?;
    Ok(GeneratedFile {
        path,
        digest: digest(&bytes),
    })
}
fn cleanup(files: &[GeneratedFile]) {
    for file in files {
        // Do not delete a generated workspace file that was changed after launch.  That is a
        // collision/drift signal, not authorization to overwrite the user's state.
        if fs::read(&file.path).is_ok_and(|bytes| digest(&bytes) == file.digest) {
            let _ = fs::remove_file(&file.path);
        }
        if let Some(parent) = file.path.parent() {
            let _ = fs::remove_dir(parent);
        }
    }
}
fn digest(bytes: &[u8]) -> String {
    let mut value = String::with_capacity(64);
    for byte in Sha256::digest(bytes) {
        write!(&mut value, "{byte:02x}").expect("writing to String cannot fail");
    }
    value
}

async fn cancelled(sender: &watch::Sender<bool>) {
    let mut receiver = sender.subscribe();
    if *receiver.borrow() {
        return;
    }
    let _ = receiver.changed().await;
}

#[cfg(test)]
mod tests {
    use super::{
        ClaudeBinding, native_permission_kind, native_read_only_safe, native_scope_expansion,
    };
    use crate::PermissionKind;
    use serde_json::json;

    #[test]
    fn antigravity_repeated_prompt_cannot_replay_prior_invocation_or_tool() {
        let mut binding = super::AntigravityBinding::default();
        binding
            .accept_invocation(&json!({"initialNumSteps":1,"invocationNum":0}))
            .unwrap();
        binding.require_tool(&json!({"stepIdx":2})).unwrap();
        binding.begin_turn();
        assert!(
            binding
                .accept_invocation(&json!({"initialNumSteps":1,"invocationNum":0}))
                .is_err()
        );
        assert!(binding.require_tool(&json!({"stepIdx":2})).is_err());
        binding
            .accept_invocation(&json!({"initialNumSteps":5,"invocationNum":0}))
            .unwrap();
        assert!(binding.require_tool(&json!({"stepIdx":2})).is_err());
        binding.require_tool(&json!({"stepIdx":6})).unwrap();
        assert!(
            binding
                .accept_invocation(&json!({"initialNumSteps":5,"invocationNum":0}))
                .is_err()
        );
    }

    #[test]
    fn native_prompt_cannot_enter_a_shell_or_native_command_mode() {
        for prompt in [
            "/settings",
            " ! echo bypass",
            "hello\u{8}bye",
            "hi\rthere",
            "\u{1b}[2J",
        ] {
            assert!(super::validate_prompt(prompt).is_err());
        }
        super::validate_prompt("Read the file /src/lib.rs\nand explain it.").unwrap();
    }

    #[test]
    fn windows_hook_encoding_preserves_literal_bytes() {
        assert_eq!(super::encode_base64(b""), "");
        assert_eq!(super::encode_base64(b"f"), "Zg==");
        assert_eq!(super::encode_base64(b"fo"), "Zm8=");
        assert_eq!(super::encode_base64(b"foo"), "Zm9v");
        let command = super::antigravity_hook_command(
            std::path::Path::new("C:\\a b\\it's-safe.exe"),
            "SessionStart",
        )
        .unwrap();
        assert!(command.contains(" -NoProfile -NonInteractive -EncodedCommand "));
        assert!(!command.contains("it's-safe"));
    }

    #[tokio::test]
    #[ignore = "installed Claude startup diagnostic; sends no model prompt"]
    async fn claude_native_startup_probe() {
        use super::*;
        use crate::ProviderAdapter;
        assert_eq!(
            std::env::var("LAZOO_LIVE_CLAUDE_INTERACTIVE").as_deref(),
            Ok("1")
        );
        let candidate = crate::ClaudeAdapter::default()
            .discover(&crate::DiscoveryEnvironment::current())
            .remove(0);
        let directory = match std::env::var_os("LAZOO_LIVE_NATIVE_WORKSPACE_PARENT") {
            Some(parent) => tempfile::tempdir_in(parent),
            None => tempfile::tempdir(),
        }
        .unwrap();
        let request = SessionRequest::new(directory.path());
        let (hooks, mut receiver) = NativeHookServer::open(ProviderKind::ClaudeCode)
            .await
            .unwrap();
        let bridge = SessionMcpBridge::open(ProviderKind::ClaudeCode, vec![])
            .await
            .unwrap();
        let generated = prepare_generated_config(
            ProviderKind::ClaudeCode,
            &candidate.path,
            &request,
            &hooks,
            &bridge,
        )
        .unwrap()
        .into_iter()
        .map(seal_generated)
        .collect::<Result<Vec<_>, _>>()
        .unwrap();
        let command = TerminalCommand::new(&candidate.path, directory.path(), COLS, ROWS)
            .env("LAZOO_NATIVE_HOOK_URL", hooks.endpoint())
            .env("LAZOO_NATIVE_HOOK_TOKEN", hooks.token());
        let terminal = TerminalSession::spawn(interactive_command(
            ProviderKind::ClaudeCode,
            command,
            &request,
            &generated,
        ))
        .unwrap();
        let callback = timeout(Duration::from_secs(20), receiver.recv()).await;
        if let Ok(Some(hook)) = &callback {
            eprintln!(
                "hook={} fields={:?}",
                hook.event.event,
                hook.event
                    .payload
                    .as_object()
                    .unwrap()
                    .keys()
                    .collect::<Vec<_>>()
            );
        }
        if let Ok(Some(hook)) = callback {
            let _ = hook.reply.send(json!({}));
        }
        tokio::time::sleep(Duration::from_secs(2)).await;
        terminal.write_input(b"NATIVE_DIAGNOSTIC_INPUT").unwrap();
        tokio::time::sleep(Duration::from_millis(100)).await;
        eprintln!(
            "STARTUP_SCREEN={}",
            String::from_utf8_lossy(&terminal.read_output(0, 64 * 1024).unwrap().bytes)
        );
        terminal.stop().unwrap();
        hooks.close();
        bridge.close();
        cleanup(&generated);
    }

    #[test]
    fn claude_paste_wrapper_unwraps_only_the_exact_form() {
        let prompt = "[Lazoo notices] first\n\nsecond \">\n line";
        let wrapped =
            format!("\n\n<pasted_content id=\"2864\">\n{prompt}\n</pasted_content id=\"2864\">\n");
        assert_eq!(super::claude_pasted_prompt(&wrapped), Some(prompt));
        let hex = wrapped.replace("2864", "78ea");
        assert_eq!(super::claude_pasted_prompt(&hex), Some(prompt));
        let other_id = wrapped.replace(
            "</pasted_content id=\"2864\">",
            "</pasted_content id=\"1\">",
        );
        assert_eq!(super::claude_pasted_prompt(&other_id), None);
        let named = wrapped.replace("2864", "x");
        assert_eq!(super::claude_pasted_prompt(&named), None);
        assert_eq!(
            super::claude_pasted_prompt(&format!("{wrapped} extra")),
            None
        );
        assert_eq!(super::claude_pasted_prompt(prompt), None);
    }

    #[test]
    fn native_tool_policy_never_treats_edits_as_read_only() {
        let edit = json!({"tool_name":"edit_file"});
        assert_eq!(native_permission_kind(&edit), PermissionKind::FileChange);
        assert!(!native_read_only_safe(&edit));
        let read = json!({"tool_name":"read_file"});
        assert!(native_read_only_safe(&read));
    }

    #[test]
    fn shell_like_tools_are_scope_expansions() {
        let cwd = tempfile::tempdir().unwrap();
        assert!(native_scope_expansion(
            &json!({"tool_name":"run_command"}),
            cwd.path()
        ));
        assert!(native_scope_expansion(
            &json!({"tool_name":"Bash"}),
            cwd.path()
        ));
        assert!(!native_scope_expansion(
            &json!({"tool_name":"read_file"}),
            cwd.path()
        ));
    }

    #[test]
    fn native_file_actions_require_confirmation_outside_the_workspace() {
        let cwd = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        assert!(!native_scope_expansion(
            &json!({"tool_name":"Write","tool_input":{"file_path":cwd.path().join("new.txt")}}),
            cwd.path()
        ));
        assert!(native_scope_expansion(
            &json!({"tool_name":"Edit","tool_input":{"file_path":outside.path().join("new.txt")}}),
            cwd.path()
        ));
        assert!(native_scope_expansion(
            &json!({"toolCall":{"name":"replace_file_content","args":{"TargetFile":"../outside.txt"}}}),
            cwd.path()
        ));
        assert!(native_scope_expansion(
            &json!({"tool_name":"Write","tool_input":{}}),
            cwd.path()
        ));
    }

    #[test]
    fn claude_callbacks_require_the_current_prompt_and_ordered_message_batches() {
        let prompt = uuid::Uuid::new_v4().to_string();
        let mut binding = ClaudeBinding {
            prompt_id: Some(prompt.clone()),
            ..Default::default()
        };
        for event in ["MessageDisplay", "PreToolUse", "Stop"] {
            assert!(
                binding
                    .require_prompt(
                        &json!({"hook_event_name":event,"prompt_id":uuid::Uuid::new_v4()})
                    )
                    .is_err()
            );
        }
        let mut message = json!({"prompt_id":prompt,"turn_id":uuid::Uuid::new_v4(),"message_id":uuid::Uuid::new_v4(),"index":0,"final":false});
        binding.accept_message(&message).unwrap();
        assert!(binding.accept_message(&message).is_err());
        message["index"] = json!(1);
        message["final"] = json!(true);
        binding.accept_message(&message).unwrap();
        message["index"] = json!(2);
        assert!(binding.accept_message(&message).is_err());
        message["message_id"] = json!(uuid::Uuid::new_v4());
        message["turn_id"] = json!(uuid::Uuid::new_v4());
        message["index"] = json!(0);
        assert!(binding.accept_message(&message).is_err());
    }
}
