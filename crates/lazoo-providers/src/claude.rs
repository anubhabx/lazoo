use std::{
    cmp,
    collections::{HashMap, HashSet},
    path::{Path, PathBuf},
    sync::atomic::{AtomicBool, Ordering},
    time::{Duration, Instant},
};

use serde_json::{Value, json};
use tokio::{
    process::Command,
    sync::{Mutex, Notify},
    time::{self, timeout},
};

use crate::{
    ADAPTER_CONTRACT_V1, AdapterFuture, AuthenticationEvidence, AuthenticationState, CommandSpec,
    DiscoveryEnvironment, ExecutableCandidate, PermissionDecision, PermissionKind,
    PermissionRequest, ProviderAdapter, ProviderCapabilities, ProviderError, ProviderEvent,
    ProviderKind, ProviderProbe, ProviderSession, ProviderVerification, SessionRequest,
    TurnEventObserver, TurnRequest,
    ndjson::{NdjsonControl, NdjsonProcess},
    redact_json,
};

const CLAUDE_PROTOCOL: &str = "claude-code/stream-json-v1";
const STOP_TIMEOUT: Duration = Duration::from_secs(2);
const VERSION_TIMEOUT: Duration = Duration::from_secs(5);
const AUTH_TIMEOUT: Duration = Duration::from_secs(15);
const MAX_PROBE_BYTES: usize = 1024 * 1024;
const MAX_STARTUP_HOOK_FRAMES: usize = 64;
const STARTUP_HOOK_DRAIN_TIMEOUT: Duration = Duration::from_secs(1);
/// How long the first prompt of a session waits for a trailing startup frame before it is written.
/// `SessionStart` hook frames can trail `init` by more than a scheduler tick; one millisecond
/// caught them on an idle machine and missed them under load (five fixture tests failed in a
/// loaded gate on 2026-10-17), which would attribute the frames to the first turn. Later prompts
/// keep the one-millisecond check: hooks fire at session start, and each turn would otherwise pay it.
const FIRST_PROMPT_QUIET_WINDOW: Duration = Duration::from_millis(150);
const LATER_PROMPT_QUIET_WINDOW: Duration = Duration::from_millis(1);

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct ClaudeExternalAuth {
    pub endpoint_configured: bool,
    pub credential_configured: bool,
}

impl ClaudeExternalAuth {
    #[must_use]
    pub fn current() -> Self {
        let present = |name: &str| std::env::var_os(name).is_some_and(|value| !value.is_empty());
        let endpoint_configured = present("ANTHROPIC_BASE_URL")
            || present("CLAUDE_CODE_USE_BEDROCK")
            || present("CLAUDE_CODE_USE_VERTEX")
            || present("CLAUDE_CODE_USE_FOUNDRY");
        let credential_configured = present("ANTHROPIC_API_KEY")
            || present("ANTHROPIC_AUTH_TOKEN")
            || present("AWS_PROFILE")
            || (present("AWS_ACCESS_KEY_ID") && present("AWS_SECRET_ACCESS_KEY"))
            || present("GOOGLE_APPLICATION_CREDENTIALS")
            || present("ANTHROPIC_FOUNDRY_API_KEY");
        Self {
            endpoint_configured,
            credential_configured,
        }
    }
}

#[derive(Clone, Debug, Default)]
pub struct ClaudeAdapter {
    executable: Option<PathBuf>,
}

impl ClaudeAdapter {
    #[must_use]
    pub fn with_executable(executable: impl Into<PathBuf>) -> Self {
        Self {
            executable: Some(executable.into()),
        }
    }

    #[must_use]
    pub fn command(
        executable: &Path,
        request: &SessionRequest,
        new_session_id: &str,
    ) -> CommandSpec {
        let mut command = CommandSpec::new(executable, &request.cwd)
            .arg("--print")
            .arg("--input-format")
            .arg("stream-json")
            .arg("--output-format")
            .arg("stream-json")
            .arg("--verbose")
            .arg("--replay-user-messages")
            .arg("--include-partial-messages")
            .arg("--permission-mode")
            .arg(permission_mode(request.permission_mode.as_deref()))
            // The documented SDK control protocol emits `can_use_tool` requests only when the
            // print host owns prompts.  Capability remains false: mutable native configuration
            // can still auto-allow calls that never reach this bridge.
            .arg("--permission-prompts")
            .arg("host")
            .arg("--no-chrome");
        if let Some(session_id) = &request.session_id {
            command = command.arg("--resume").arg(session_id);
        } else {
            command = command.arg("--session-id").arg(new_session_id);
        }
        for root in &request.workspace_roots {
            command = command.arg("--add-dir").arg(root.as_os_str());
        }
        if let Some(model) = &request.model {
            command = command.arg("--model").arg(model);
        }
        if let Some(effort) = &request.effort {
            command = command.arg("--effort").arg(effort);
        }
        command.apply_environment(&request.launch_environment);
        command
    }

    #[must_use]
    pub fn turn_input(request: &TurnRequest) -> Value {
        json!({
            "type": "user",
            "message": {
                "role": "user",
                "content": [{"type": "text", "text": request.prompt}]
            }
        })
    }

    #[must_use]
    pub const fn capabilities() -> ProviderCapabilities {
        ProviderCapabilities {
            session: true,
            stream: true,
            cancel: true,
            resume: true,
            terminal: true,
            permission: false,
            mcp: false,
            plugins: false,
            skills: false,
            connectors: false,
            native_message: true,
            model_selection: true,
        }
    }

    #[must_use]
    pub fn authentication_from_status(
        status: &Value,
        external: ClaudeExternalAuth,
    ) -> AuthenticationEvidence {
        let native = match status.get("loggedIn").and_then(Value::as_bool) {
            Some(true) => AuthenticationState::Ready,
            Some(false) => AuthenticationState::NeedsLogin,
            None => AuthenticationState::Unknown,
        };
        let external_state = if external.credential_configured {
            AuthenticationState::Ready
        } else if external.endpoint_configured {
            AuthenticationState::Blocked
        } else {
            AuthenticationState::Unknown
        };
        let detail = match (native, external_state) {
            (AuthenticationState::Ready, AuthenticationState::Ready) => {
                "native login and external credential are configured"
            }
            (AuthenticationState::Ready, _) => "native login is configured",
            (_, AuthenticationState::Ready) => "external credential is configured",
            (AuthenticationState::NeedsLogin, AuthenticationState::Blocked) => {
                "native login is required and the external endpoint lacks credentials"
            }
            (AuthenticationState::NeedsLogin, _) => "native login is required",
            (_, AuthenticationState::Blocked) => "external endpoint lacks credentials",
            _ => "authentication readiness is unknown",
        };
        AuthenticationEvidence {
            native,
            external: external_state,
            detail: Some(detail.to_owned()),
        }
    }

    fn executable_candidates(
        &self,
        environment: &DiscoveryEnvironment,
    ) -> Vec<ExecutableCandidate> {
        self.executable.as_ref().map_or_else(
            || environment.candidates(ProviderKind::ClaudeCode, &["claude.exe", "claude"]),
            |path| {
                vec![ExecutableCandidate::new(
                    path.clone(),
                    crate::DiscoverySource::KnownLocation,
                )]
            },
        )
    }

    async fn probe_candidate(&self, candidate: ExecutableCandidate) -> ProviderProbe {
        let version = match read_version(&candidate.path).await {
            Ok(version) => version,
            Err(error) => {
                return degraded_probe(candidate, None, format!("version probe failed: {error}"));
            }
        };
        let status = match read_auth_status(&candidate.path).await {
            Ok(status) => status,
            Err(error) => {
                return degraded_probe(
                    candidate,
                    Some(version),
                    format!("native auth probe failed: {error}"),
                );
            }
        };
        let authentication =
            Self::authentication_from_status(&status, ClaudeExternalAuth::current());
        configured_probe(candidate, version, authentication)
    }

    fn start_session(&self, request: &SessionRequest) -> Result<ClaudeSession, ProviderError> {
        validate_request(request)?;
        let environment = DiscoveryEnvironment::current();
        let candidate = self
            .executable_candidates(&environment)
            .into_iter()
            .next()
            .ok_or(ProviderError::NotDetected(ProviderKind::ClaudeCode))?;
        let generated_id = uuid::Uuid::new_v4().to_string();
        let session_id = request
            .session_id
            .clone()
            .unwrap_or_else(|| generated_id.clone());
        let process =
            NdjsonProcess::spawn(&Self::command(&candidate.path, request, &generated_id))?;
        let native_host_pid = process.native_host_pid();
        let job = process.job();
        let control = process.control();
        Ok(ClaudeSession {
            process: Mutex::new(process),
            control,
            session_id,
            native_host_pid,
            job,
            request: request.clone(),
            active: AtomicBool::new(false),
            cancel_requested: AtomicBool::new(false),
            prompted: AtomicBool::new(false),
            lifecycle: std::sync::Mutex::new(false),
            cancellation: Notify::new(),
            stopped: AtomicBool::new(false),
        })
    }
}

impl ProviderAdapter for ClaudeAdapter {
    type Session = ClaudeSession;

    fn kind(&self) -> ProviderKind {
        ProviderKind::ClaudeCode
    }

    fn discover(&self, environment: &DiscoveryEnvironment) -> Vec<ExecutableCandidate> {
        self.executable_candidates(environment)
    }

    fn probe<'a>(
        &'a self,
        environment: &'a DiscoveryEnvironment,
    ) -> AdapterFuture<'a, ProviderProbe> {
        Box::pin(async move {
            let Some(candidate) = self.executable_candidates(environment).into_iter().next() else {
                return Ok(ProviderProbe::not_detected(ProviderKind::ClaudeCode));
            };
            Ok(self.probe_candidate(candidate).await)
        })
    }

    fn start<'a>(&'a self, request: &'a SessionRequest) -> AdapterFuture<'a, Self::Session> {
        Box::pin(std::future::ready(self.start_session(request)))
    }
}

#[derive(Debug)]
pub struct ClaudeSession {
    process: Mutex<NdjsonProcess>,
    control: NdjsonControl,
    session_id: String,
    native_host_pid: Option<u32>,
    job: Option<std::sync::Arc<lazoo_runtime::JobObject>>,
    request: SessionRequest,
    active: AtomicBool,
    cancel_requested: AtomicBool,
    prompted: AtomicBool,
    lifecycle: std::sync::Mutex<bool>,
    cancellation: Notify,
    stopped: AtomicBool,
}

impl ClaudeSession {
    async fn run_turn(&self, request: &TurnRequest) -> Result<Vec<ProviderEvent>, ProviderError> {
        if request.read_only {
            return Err(ProviderError::Unsupported {
                provider: ProviderKind::ClaudeCode,
                capability: "read-only turns",
                detail: None,
            });
        }
        if self.stopped.load(Ordering::Acquire) {
            return Err(ProviderError::Protocol(
                "Claude Code session has already stopped".to_owned(),
            ));
        }
        let mut process = self.process.lock().await;
        self.begin_turn()?;
        let outcome = self.collect_turn(&mut process, request, None).await;
        self.finish_turn();
        outcome
    }

    async fn run_turn_observed(
        &self,
        request: &TurnRequest,
        observer: &dyn TurnEventObserver,
    ) -> Result<Vec<ProviderEvent>, ProviderError> {
        if request.read_only {
            return Err(ProviderError::Unsupported {
                provider: ProviderKind::ClaudeCode,
                capability: "read-only turns",
                detail: None,
            });
        }
        if self.stopped.load(Ordering::Acquire) {
            return Err(ProviderError::Protocol(
                "Claude Code session has already stopped".to_owned(),
            ));
        }
        let mut process = self.process.lock().await;
        self.begin_turn()?;
        let outcome = self
            .collect_turn(&mut process, request, Some(observer))
            .await;
        self.finish_turn();
        outcome
    }

    fn begin_turn(&self) -> Result<(), ProviderError> {
        let mut lifecycle = self.lifecycle.lock().map_err(|_| {
            ProviderError::Protocol("Claude Code lifecycle lock is poisoned".to_owned())
        })?;
        if *lifecycle {
            return Err(ProviderError::Protocol(
                "Claude Code session already has an active turn".to_owned(),
            ));
        }
        self.cancel_requested.store(false, Ordering::Release);
        *lifecycle = true;
        self.active.store(true, Ordering::Release);
        Ok(())
    }

    fn finish_turn(&self) {
        self.active.store(false, Ordering::Release);
        if let Ok(mut lifecycle) = self.lifecycle.lock() {
            *lifecycle = false;
        }
    }

    #[allow(clippy::too_many_lines)]
    async fn collect_turn(
        &self,
        process: &mut NdjsonProcess,
        request: &TurnRequest,
        observer: Option<&dyn TurnEventObserver>,
    ) -> Result<Vec<ProviderEvent>, ProviderError> {
        self.drain_pre_prompt_startup(process).await?;
        self.control
            .send(&ClaudeAdapter::turn_input(request))
            .await?;
        self.prompted.store(true, Ordering::Release);
        let started = Instant::now();
        let mut last_progress = started;
        let mut turn_started = false;
        let mut events = Vec::new();
        let mut permission_ids = HashSet::new();
        let mut tool_use_ids = HashSet::new();
        let mut permission_responses = HashMap::new();
        let mut startup_bound = false;
        loop {
            let total_remaining = self
                .request
                .turn_timeout()
                .checked_sub(started.elapsed())
                .ok_or(ProviderError::Timeout {
                    phase: "Claude Code turn completion",
                })?;
            let progress_remaining = self
                .request
                .progress_timeout()
                .checked_sub(last_progress.elapsed())
                .ok_or(ProviderError::Stalled {
                    millis: self.request.progress_timeout_millis,
                })?;
            let wait = cmp::min(total_remaining, progress_remaining);
            let message = match process
                .next_message(wait, "Claude Code turn progress")
                .await
            {
                Err(ProviderError::ProcessExited { .. })
                    if self.cancel_requested.load(Ordering::Acquire) =>
                {
                    let cancelled = ProviderEvent::Cancelled {
                        provider: ProviderKind::ClaudeCode,
                        session_id: self.session_id.clone(),
                        turn_id: Some(request.correlation_id.to_string()),
                        raw: None,
                    };
                    // The daemon records a cancellation from its observer, not from the returned
                    // events: a `Cancelled` that only came back in the return value left the turn
                    // `accepted`, and it ended `uncertain` (found by the live gate on 2026-10-17).
                    if let Some(observer) = observer {
                        observer.observe(&cancelled)?;
                    }
                    events.push(cancelled);
                    return Ok(events);
                }
                Err(ProviderError::Timeout { .. }) if progress_remaining <= total_remaining => {
                    return Err(ProviderError::Stalled {
                        millis: self.request.progress_timeout_millis,
                    });
                }
                Err(ProviderError::Timeout { .. }) => {
                    return Err(ProviderError::Timeout {
                        phase: "Claude Code turn completion",
                    });
                }
                result => result?,
            };
            last_progress = Instant::now();
            if !startup_bound {
                if is_startup_hook(&message, &self.session_id) {
                    continue;
                }
                if message.get("type").and_then(Value::as_str) != Some("system")
                    || message.get("subtype").and_then(Value::as_str) != Some("init")
                    || session_id(&message).as_deref() != Some(self.session_id.as_str())
                {
                    return Err(ProviderError::InvalidEvent(format!(
                        "Claude Code emitted a {}/{} frame for session {:?} before correlated startup init",
                        message.get("type").and_then(Value::as_str).unwrap_or("?"),
                        message
                            .get("subtype")
                            .and_then(Value::as_str)
                            .unwrap_or("?"),
                        session_id(&message).unwrap_or_default(),
                    )));
                }
                let init = normalize_claude_message(&message)?;
                if let Some(observer) = observer {
                    observer.observe(&init[0])?;
                }
                events.extend(init);
                startup_bound = true;
                continue;
            }
            if message.get("type").and_then(Value::as_str) == Some("control_request") {
                if let Some(event) = self
                    .handle_control_request(
                        &message,
                        observer,
                        request,
                        started,
                        &mut permission_ids,
                        &mut tool_use_ids,
                        &mut permission_responses,
                    )
                    .await?
                {
                    if let Some(observer) = observer {
                        observer.observe(&event)?;
                    }
                    events.push(event);
                }
                continue;
            }
            let terminal = is_result_message(&message);
            for event in normalize_claude_message(&message)? {
                if let Some(event_session_id) = claude_event_session_id(&event)
                    && event_session_id != self.session_id
                {
                    return Err(ProviderError::InvalidEvent(
                        "Claude Code event targeted a different session".to_owned(),
                    ));
                }
                if !turn_started && !matches!(event, ProviderEvent::SessionStarted { .. }) {
                    events.push(ProviderEvent::TurnStarted {
                        provider: ProviderKind::ClaudeCode,
                        session_id: self.session_id.clone(),
                        turn_id: Some(request.correlation_id.to_string()),
                        raw: None,
                    });
                    turn_started = true;
                    if let Some(observer) = observer {
                        observer.observe(events.last().expect("turn event was appended"))?;
                    }
                }
                if let Some(observer) = observer {
                    observer.observe(&event)?;
                }
                events.push(event);
            }
            if terminal {
                return Ok(events);
            }
        }
    }

    /// Claude Code 2.1.296 emits correlated `SessionStart` hook lifecycle frames before it reads
    /// the first stream-json user record. Those frames do not establish a turn, and are not
    /// durable evidence. Every other pre-prompt frame is refused so it cannot be attributed to
    /// the first Lazoo turn.
    async fn drain_pre_prompt_startup(
        &self,
        process: &mut NdjsonProcess,
    ) -> Result<(), ProviderError> {
        let started = Instant::now();
        let mut frames = 0_usize;
        let quiet_window = if self.prompted.load(Ordering::Acquire) {
            LATER_PROMPT_QUIET_WINDOW
        } else {
            FIRST_PROMPT_QUIET_WINDOW
        };
        while process.wait_for_ready_output(quiet_window).await? {
            if frames == MAX_STARTUP_HOOK_FRAMES || started.elapsed() >= STARTUP_HOOK_DRAIN_TIMEOUT
            {
                let _ = self.control.kill().await;
                return Err(ProviderError::InvalidEvent(
                    "Claude Code emitted too many startup hook frames before the first turn prompt"
                        .to_owned(),
                ));
            }
            let message = match process
                .next_message(Duration::from_millis(50), "Claude Code startup frame")
                .await
            {
                Ok(message) => message,
                Err(ProviderError::Timeout { .. }) => {
                    let _ = self.control.kill().await;
                    return Err(ProviderError::InvalidEvent(
                        "Claude Code emitted a partial frame before the first turn prompt"
                            .to_owned(),
                    ));
                }
                Err(error) => return Err(error),
            };
            if !is_startup_hook(&message, &self.session_id) {
                let _ = self.control.kill().await;
                return Err(ProviderError::InvalidEvent(
                    "Claude Code emitted an unsupported frame before the first turn prompt"
                        .to_owned(),
                ));
            }
            frames += 1;
        }
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    async fn handle_control_request(
        &self,
        message: &Value,
        observer: Option<&dyn TurnEventObserver>,
        turn: &TurnRequest,
        started: Instant,
        permission_ids: &mut HashSet<String>,
        tool_use_ids: &mut HashSet<String>,
        permission_responses: &mut HashMap<String, Value>,
    ) -> Result<Option<ProviderEvent>, ProviderError> {
        let request_id = message
            .get("request_id")
            .and_then(Value::as_str)
            .filter(|id| !id.is_empty())
            .map(str::to_owned);
        if let Some(response) = request_id
            .as_ref()
            .and_then(|request_id| permission_responses.get(request_id))
        {
            self.control.send(response).await?;
            return Ok(None);
        }
        let request = message.get("request").and_then(Value::as_object);
        let permission = request_id
            .as_deref()
            .zip(request)
            .and_then(|(id, request)| (!permission_ids.contains(id)).then_some((id, request)))
            .and_then(|(id, request)| {
                normalize_claude_permission_request(id, request, &self.session_id, turn).ok()
            })
            .filter(|permission| {
                permission
                    .detail
                    .get("toolUseId")
                    .and_then(Value::as_str)
                    .is_some_and(|tool_use_id| !tool_use_ids.contains(tool_use_id))
            });
        if let Some(permission) = &permission {
            permission_ids.insert(permission.request_id.clone());
            if let Some(tool_use_id) = permission.detail.get("toolUseId").and_then(Value::as_str) {
                tool_use_ids.insert(tool_use_id.to_owned());
            }
        }
        let decision = match (observer, permission.as_ref()) {
            (Some(observer), Some(permission))
                if !self.cancel_requested.load(Ordering::Acquire) =>
            {
                self.await_permission(observer, permission, started).await
            }
            _ => PermissionDecision::Decline,
        };
        let decision = if self.cancel_requested.load(Ordering::Acquire) {
            PermissionDecision::Decline
        } else {
            decision
        };
        let response = if matches!(decision, PermissionDecision::Approve) && permission.is_some() {
            json!({"type":"control_response", "response": {"subtype":"success", "request_id": request_id, "response": {"behavior":"allow", "updatedInput": request.and_then(|request| request.get("input")).cloned().unwrap_or(Value::Null)}}})
        } else if let Some(ref request_id) = request_id {
            json!({"type":"control_response", "response": {"subtype":"success", "request_id": request_id, "response": {"behavior":"deny", "message":"Lazoo declined this native permission request"}}})
        } else {
            return Err(ProviderError::InvalidEvent(
                "Claude control request omitted request_id".to_owned(),
            ));
        };
        if let Some(request_id) = request_id.as_ref() {
            permission_responses.insert(request_id.clone(), response.clone());
        }
        self.control.send(&response).await?;
        Ok(
            permission.map(|permission| ProviderEvent::PermissionRequested {
                provider: ProviderKind::ClaudeCode,
                session_id: self.session_id.clone(),
                request_id: Some(permission.request_id),
                options: vec!["allow_once".to_owned(), "reject_once".to_owned()],
                // Native input can carry command text or file contents. Do not preserve arbitrary
                // values in the durable event ledger.
                raw: None,
            }),
        )
    }

    async fn await_permission(
        &self,
        observer: &dyn TurnEventObserver,
        permission: &PermissionRequest,
        started: Instant,
    ) -> PermissionDecision {
        let Some(remaining) = self.request.turn_timeout().checked_sub(started.elapsed()) else {
            return PermissionDecision::Decline;
        };
        tokio::select! {
            result = observer.decide_permission(permission) => result.unwrap_or(PermissionDecision::Decline),
            () = time::sleep(remaining) => PermissionDecision::Decline,
            () = self.wait_for_cancellation() => PermissionDecision::Decline,
        }
    }

    async fn wait_for_cancellation(&self) {
        loop {
            let notified = self.cancellation.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();
            if self.cancel_requested.load(Ordering::Acquire) {
                return;
            }
            notified.await;
            if self.cancel_requested.load(Ordering::Acquire) {
                return;
            }
        }
    }
}

fn is_startup_hook(message: &Value, session: &str) -> bool {
    message.get("type").and_then(Value::as_str) == Some("system")
        && matches!(
            message.get("subtype").and_then(Value::as_str),
            Some("hook_started" | "hook_response")
        )
        && session_id(message).as_deref() == Some(session)
        && message
            .get("hook_id")
            .and_then(Value::as_str)
            .is_some_and(|id| !id.is_empty())
        && message
            .get("hook_name")
            .and_then(Value::as_str)
            .is_some_and(|name| name.starts_with("SessionStart:"))
        && message.get("hook_event").and_then(Value::as_str) == Some("SessionStart")
}

impl ProviderSession for ClaudeSession {
    fn id(&self) -> String {
        self.session_id.clone()
    }

    fn native_host_pid(&self) -> Option<u32> {
        self.native_host_pid
    }

    fn resources(&self) -> Option<lazoo_runtime::JobAccounting> {
        self.job.as_ref().and_then(|job| job.accounting().ok())
    }

    fn turn<'a>(&'a self, request: &'a TurnRequest) -> AdapterFuture<'a, Vec<ProviderEvent>> {
        Box::pin(async move { self.run_turn(request).await })
    }

    fn turn_observed<'a>(
        &'a self,
        request: &'a TurnRequest,
        observer: &'a dyn TurnEventObserver,
    ) -> AdapterFuture<'a, Vec<ProviderEvent>> {
        Box::pin(async move { self.run_turn_observed(request, observer).await })
    }

    fn cancel(&self) -> AdapterFuture<'_, ()> {
        Box::pin(async move {
            let active = {
                let active = self.lifecycle.lock().map_err(|_| {
                    ProviderError::Protocol("Claude Code lifecycle lock is poisoned".to_owned())
                })?;
                *active
            };
            if active {
                self.cancel_requested.store(true, Ordering::Release);
                self.cancellation.notify_waiters();
                self.stopped.store(true, Ordering::Release);
                self.control.kill().await?;
            }
            Ok(())
        })
    }

    fn stop(&self) -> AdapterFuture<'_, ()> {
        Box::pin(async move {
            if self.stopped.swap(true, Ordering::AcqRel) {
                return Ok(());
            }
            if self.active.load(Ordering::Acquire) {
                self.control.kill().await?;
            }
            self.process.lock().await.stop(STOP_TIMEOUT).await
        })
    }
}

fn claude_event_session_id(event: &ProviderEvent) -> Option<&str> {
    match event {
        ProviderEvent::SessionStarted { session_id, .. }
        | ProviderEvent::TurnStarted { session_id, .. }
        | ProviderEvent::TextDelta { session_id, .. }
        | ProviderEvent::ThoughtDelta { session_id, .. }
        | ProviderEvent::ToolUpdate { session_id, .. }
        | ProviderEvent::TerminalUpdate { session_id, .. }
        | ProviderEvent::Usage { session_id, .. }
        | ProviderEvent::PermissionRequested { session_id, .. }
        | ProviderEvent::Progress { session_id, .. }
        | ProviderEvent::TurnCompleted { session_id, .. }
        | ProviderEvent::Cancelled { session_id, .. } => Some(session_id),
        ProviderEvent::Error { session_id, .. }
        | ProviderEvent::ProcessExited { session_id, .. } => session_id.as_deref(),
    }
}

/// Converts the documented Claude Agent SDK `control_request/can_use_tool` subset into a
/// durable approval.  This is intentionally only a bridge for prompts that Claude emits; the
/// provider's mutable permission settings can still permit actions without emitting one, so
/// `ClaudeAdapter::capabilities().permission` remains false.
fn normalize_claude_permission_request(
    request_id: &str,
    request: &serde_json::Map<String, Value>,
    session_id: &str,
    turn: &TurnRequest,
) -> Result<PermissionRequest, ProviderError> {
    if request.get("subtype").and_then(Value::as_str) != Some("can_use_tool") {
        return Err(ProviderError::InvalidEvent(
            "unsupported Claude control request subtype".to_owned(),
        ));
    }
    let (kind, action) = match request.get("tool_name").and_then(Value::as_str) {
        Some("Bash") => (PermissionKind::CommandExecution, "execute a Claude command"),
        Some("Write" | "Edit") => (PermissionKind::FileChange, "edit Claude files"),
        _ => {
            return Err(ProviderError::InvalidEvent(
                "unsupported Claude permission tool".to_owned(),
            ));
        }
    };
    let tool_use_id = request
        .get("tool_use_id")
        .and_then(Value::as_str)
        .filter(|id| !id.is_empty())
        .ok_or_else(|| {
            ProviderError::InvalidEvent("Claude permission request omitted tool_use_id".to_owned())
        })?;
    let input = request
        .get("input")
        .and_then(Value::as_object)
        .ok_or_else(|| {
            ProviderError::InvalidEvent("Claude permission request omitted input".to_owned())
        })?;
    // Inputs can contain source text, tokens, and path-like claims; retain only identity fields.
    let detail = redact_json(json!({
        "toolUseId": tool_use_id,
        "tool": request.get("tool_name").cloned().unwrap_or(Value::Null),
        "inputKeys": input.keys().collect::<Vec<_>>(),
    }));
    Ok(PermissionRequest {
        provider: ProviderKind::ClaudeCode,
        session_id: session_id.to_owned(),
        turn_id: Some(turn.correlation_id.to_string()),
        request_id: request_id.to_owned(),
        kind,
        action: action.to_owned(),
        scope_expansion: true,
        detail,
    })
}

pub fn normalize_claude_message(message: &Value) -> Result<Vec<ProviderEvent>, ProviderError> {
    let record_type = message
        .get("type")
        .and_then(Value::as_str)
        .ok_or_else(|| ProviderError::InvalidEvent("Claude Code record omitted type".to_owned()))?;
    match record_type {
        "system" if message.get("subtype").and_then(Value::as_str) == Some("init") => {
            Ok(vec![ProviderEvent::SessionStarted {
                provider: ProviderKind::ClaudeCode,
                session_id: required_session_id(message, "system init")?,
                resumed: message
                    .get("resumed")
                    .and_then(Value::as_bool)
                    .unwrap_or(false),
                raw: Some(redact_json(message.clone())),
            }])
        }
        "stream_event" => normalize_stream_event(message),
        "assistant" => normalize_assistant_message(message),
        "user" => normalize_user_message(message),
        "result" => normalize_result(message),
        _ => Ok(vec![ProviderEvent::Progress {
            provider: ProviderKind::ClaudeCode,
            session_id: session_id(message).unwrap_or_default(),
            message: Some(
                message
                    .get("subtype")
                    .and_then(Value::as_str)
                    .unwrap_or(record_type)
                    .to_owned(),
            ),
            raw: Some(redact_json(message.clone())),
        }]),
    }
}

fn normalize_stream_event(message: &Value) -> Result<Vec<ProviderEvent>, ProviderError> {
    let session_id = required_session_id(message, "stream event")?;
    let delta = message.pointer("/event/delta").unwrap_or(&Value::Null);
    let delta_type = delta.get("type").and_then(Value::as_str).unwrap_or("");
    let raw = Some(redact_json(message.clone()));
    let event = match delta_type {
        "text_delta" => ProviderEvent::TextDelta {
            provider: ProviderKind::ClaudeCode,
            session_id,
            turn_id: None,
            text: required_string(delta, "text", "Claude Code text delta")?,
            raw,
        },
        "thinking_delta" => ProviderEvent::ThoughtDelta {
            provider: ProviderKind::ClaudeCode,
            session_id,
            turn_id: None,
            text: required_string(delta, "thinking", "Claude Code thinking delta")?,
            raw,
        },
        _ => ProviderEvent::Progress {
            provider: ProviderKind::ClaudeCode,
            session_id,
            message: message
                .pointer("/event/type")
                .and_then(Value::as_str)
                .map(str::to_owned),
            raw,
        },
    };
    Ok(vec![event])
}

fn normalize_assistant_message(message: &Value) -> Result<Vec<ProviderEvent>, ProviderError> {
    let session_id = required_session_id(message, "assistant message")?;
    let raw = Some(redact_json(message.clone()));
    let mut events = Vec::new();
    for block in message
        .pointer("/message/content")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        if block.get("type").and_then(Value::as_str) == Some("tool_use") {
            let tool_name = block.get("name").and_then(Value::as_str).map(str::to_owned);
            events.push(ProviderEvent::ToolUpdate {
                provider: ProviderKind::ClaudeCode,
                session_id: session_id.clone(),
                turn_id: None,
                tool_name: tool_name.clone(),
                status: "started".to_owned(),
                raw: raw.clone(),
            });
            if tool_name.as_deref() == Some("Bash") {
                events.push(ProviderEvent::TerminalUpdate {
                    provider: ProviderKind::ClaudeCode,
                    session_id: session_id.clone(),
                    terminal_id: block.get("id").and_then(Value::as_str).map(str::to_owned),
                    output: None,
                    raw: raw.clone(),
                });
            }
        }
    }
    if events.is_empty() {
        events.push(ProviderEvent::Progress {
            provider: ProviderKind::ClaudeCode,
            session_id,
            message: Some("assistant".to_owned()),
            raw,
        });
    }
    Ok(events)
}

fn normalize_user_message(message: &Value) -> Result<Vec<ProviderEvent>, ProviderError> {
    let session_id = required_session_id(message, "user message")?;
    let raw = Some(redact_json(message.clone()));
    let mut events = Vec::new();
    for block in message
        .pointer("/message/content")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        if block.get("type").and_then(Value::as_str) == Some("tool_result") {
            let is_error = block
                .get("is_error")
                .and_then(Value::as_bool)
                .unwrap_or(false);
            events.push(ProviderEvent::ToolUpdate {
                provider: ProviderKind::ClaudeCode,
                session_id: session_id.clone(),
                turn_id: None,
                tool_name: None,
                status: if is_error { "failed" } else { "completed" }.to_owned(),
                raw: raw.clone(),
            });
        }
    }
    Ok(events)
}

fn normalize_result(message: &Value) -> Result<Vec<ProviderEvent>, ProviderError> {
    let session_id = required_session_id(message, "result")?;
    let raw = Some(redact_json(message.clone()));
    let mut events = Vec::new();
    if let Some(usage) = message.get("usage") {
        events.push(ProviderEvent::Usage {
            provider: ProviderKind::ClaudeCode,
            session_id: session_id.clone(),
            input_tokens: usage.get("input_tokens").and_then(Value::as_u64),
            output_tokens: usage.get("output_tokens").and_then(Value::as_u64),
            total_tokens: usage
                .get("total_tokens")
                .and_then(Value::as_u64)
                .or_else(|| {
                    Some(
                        usage.get("input_tokens").and_then(Value::as_u64)?
                            + usage.get("output_tokens").and_then(Value::as_u64)?,
                    )
                }),
            cost_usd: message.get("total_cost_usd").and_then(Value::as_f64),
            raw: raw.clone(),
        });
    }
    let successful = !message
        .get("is_error")
        .and_then(Value::as_bool)
        .unwrap_or(false)
        && message
            .get("subtype")
            .and_then(Value::as_str)
            .is_none_or(|subtype| subtype == "success");
    if successful {
        events.push(ProviderEvent::TurnCompleted {
            provider: ProviderKind::ClaudeCode,
            session_id,
            turn_id: None,
            // The daemon's provider-event state machine accepts only `completed` for a
            // successful terminal turn; `success` would leave the turn non-terminal.
            status: "completed".to_owned(),
            output: message
                .get("result")
                .and_then(Value::as_str)
                .map(str::to_owned),
            raw,
        });
    } else {
        let code = message
            .get("terminal_reason")
            .or_else(|| message.get("subtype"))
            .and_then(Value::as_str)
            .unwrap_or("claude-result")
            .to_owned();
        let failure = message
            .get("result")
            .or_else(|| message.get("error"))
            .and_then(Value::as_str)
            .unwrap_or("Claude Code turn failed")
            .to_owned();
        events.push(ProviderEvent::Error {
            provider: ProviderKind::ClaudeCode,
            session_id: Some(session_id.clone()),
            code: code.clone(),
            message: failure.clone(),
            retryable: false,
            raw: raw.clone(),
        });
        events.push(ProviderEvent::TurnCompleted {
            provider: ProviderKind::ClaudeCode,
            session_id,
            turn_id: None,
            status: "failed".to_owned(),
            output: Some(failure),
            raw,
        });
    }
    Ok(events)
}

fn session_id(message: &Value) -> Option<String> {
    message
        .get("session_id")
        .and_then(Value::as_str)
        .map(str::to_owned)
}

fn required_session_id(message: &Value, label: &str) -> Result<String, ProviderError> {
    session_id(message).ok_or_else(|| {
        ProviderError::InvalidEvent(format!("Claude Code {label} omitted session_id"))
    })
}

fn required_string(value: &Value, field: &str, label: &str) -> Result<String, ProviderError> {
    value
        .get(field)
        .and_then(Value::as_str)
        .map(str::to_owned)
        .ok_or_else(|| ProviderError::InvalidEvent(format!("{label} omitted {field}")))
}

fn is_result_message(message: &Value) -> bool {
    message.get("type").and_then(Value::as_str) == Some("result")
}

fn permission_mode(value: Option<&str>) -> &'static str {
    match value {
        Some("ask" | "default" | "on-request" | "manual") => "manual",
        Some("plan" | "never" | "untrusted") => "plan",
        Some("accept-edits" | "acceptEdits") => "acceptEdits",
        Some("auto") => "auto",
        _ => "dontAsk",
    }
}

fn validate_request(request: &SessionRequest) -> Result<(), ProviderError> {
    if !request.cwd.is_absolute() {
        return Err(ProviderError::Protocol(
            "Claude Code workspace path must be absolute".to_owned(),
        ));
    }
    if request
        .workspace_roots
        .iter()
        .any(|root| !root.is_absolute())
    {
        return Err(ProviderError::Protocol(
            "Claude Code additional workspace roots must be absolute".to_owned(),
        ));
    }
    if let Some(effort) = request.effort.as_deref()
        && !matches!(effort, "low" | "medium" | "high" | "xhigh" | "max")
    {
        return Err(ProviderError::Protocol(format!(
            "unsupported Claude Code effort: {effort}"
        )));
    }
    if let Some(mode) = request.permission_mode.as_deref()
        && !matches!(
            mode,
            "ask"
                | "default"
                | "on-request"
                | "manual"
                | "plan"
                | "never"
                | "untrusted"
                | "accept-edits"
                | "acceptEdits"
                | "auto"
                | "dont-ask"
                | "dontAsk"
        )
    {
        return Err(ProviderError::Protocol(format!(
            "unsupported Claude Code permission mode: {mode}"
        )));
    }
    Ok(())
}

async fn read_version(path: &Path) -> Result<String, ProviderError> {
    let output = timeout(
        VERSION_TIMEOUT,
        Command::new(path).arg("--version").output(),
    )
    .await
    .map_err(|_| ProviderError::Timeout {
        phase: "Claude Code version probe",
    })??;
    checked_output(&output, "Claude Code version", 64 * 1024)
}

async fn read_auth_status(path: &Path) -> Result<Value, ProviderError> {
    let output = timeout(
        AUTH_TIMEOUT,
        Command::new(path)
            .arg("auth")
            .arg("status")
            .arg("--json")
            .output(),
    )
    .await
    .map_err(|_| ProviderError::Timeout {
        phase: "Claude Code auth probe",
    })??;
    if output.status.code() == Some(1) {
        let bytes = if output.stdout.is_empty() {
            &output.stderr
        } else {
            &output.stdout
        };
        if bytes.len() > MAX_PROBE_BYTES {
            return Err(ProviderError::Protocol(
                "Claude Code auth output exceeded the limit".to_owned(),
            ));
        }
        let status: Value = serde_json::from_slice(bytes).map_err(ProviderError::from)?;
        if status.get("loggedIn").and_then(Value::as_bool) == Some(false) {
            return Ok(status);
        }
    }
    let json = checked_output(&output, "Claude Code auth", MAX_PROBE_BYTES)?;
    serde_json::from_str(&json).map_err(ProviderError::from)
}

fn checked_output(
    output: &std::process::Output,
    label: &str,
    maximum: usize,
) -> Result<String, ProviderError> {
    if !output.status.success() {
        return Err(ProviderError::Protocol(format!(
            "{label} command exited with code {:?}",
            output.status.code()
        )));
    }
    let bytes = if output.stdout.is_empty() {
        &output.stderr
    } else {
        &output.stdout
    };
    if bytes.len() > maximum {
        return Err(ProviderError::Protocol(format!(
            "{label} output exceeded the limit"
        )));
    }
    let value = String::from_utf8_lossy(bytes).trim().to_owned();
    if value.is_empty() {
        Err(ProviderError::Protocol(format!(
            "{label} command returned empty output"
        )))
    } else {
        Ok(value)
    }
}

fn configured_probe(
    candidate: ExecutableCandidate,
    version: String,
    authentication: AuthenticationEvidence,
) -> ProviderProbe {
    let ready = authentication.native == AuthenticationState::Ready
        || authentication.external == AuthenticationState::Ready;
    ProviderProbe {
        contract_version: ADAPTER_CONTRACT_V1,
        provider: ProviderKind::ClaudeCode,
        verification: if ready {
            ProviderVerification::Configured
        } else {
            ProviderVerification::Degraded
        },
        executable: Some(candidate.path),
        discovery_source: Some(candidate.source),
        version: Some(version),
        protocol: Some(CLAUDE_PROTOCOL.to_owned()),
        capabilities: ClaudeAdapter::capabilities(),
        authentication,
        degradation_reason: (!ready)
            .then(|| "no ready authentication channel was detected".to_owned()),
        evidence: vec![
            "version command completed".to_owned(),
            "native auth status was parsed without retaining identity fields".to_owned(),
            "external auth was checked by variable presence without reading values".to_owned(),
            "model execution was not attempted by this probe".to_owned(),
        ],
        checked_at: chrono::Utc::now().to_rfc3339(),
    }
}

fn degraded_probe(
    candidate: ExecutableCandidate,
    version: Option<String>,
    reason: String,
) -> ProviderProbe {
    ProviderProbe {
        contract_version: ADAPTER_CONTRACT_V1,
        provider: ProviderKind::ClaudeCode,
        verification: ProviderVerification::Degraded,
        executable: Some(candidate.path),
        discovery_source: Some(candidate.source),
        version,
        protocol: Some(CLAUDE_PROTOCOL.to_owned()),
        capabilities: ProviderCapabilities::default(),
        authentication: AuthenticationEvidence::unknown(),
        degradation_reason: Some(reason),
        evidence: Vec::new(),
        checked_at: chrono::Utc::now().to_rfc3339(),
    }
}
