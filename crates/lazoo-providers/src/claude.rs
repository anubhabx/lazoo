use std::{
    cmp,
    path::{Path, PathBuf},
    sync::atomic::{AtomicBool, Ordering},
    time::{Duration, Instant},
};

use serde_json::{Value, json};
use tokio::{process::Command, sync::Mutex, time::timeout};

use crate::{
    ADAPTER_CONTRACT_V1, AdapterFuture, AuthenticationEvidence, AuthenticationState, CommandSpec,
    DiscoveryEnvironment, ExecutableCandidate, ProviderAdapter, ProviderCapabilities,
    ProviderError, ProviderEvent, ProviderKind, ProviderProbe, ProviderSession,
    ProviderVerification, SessionRequest, TurnRequest,
    ndjson::{NdjsonControl, NdjsonProcess},
    redact_json,
};

const CLAUDE_PROTOCOL: &str = "claude-code/stream-json-v1";
const STOP_TIMEOUT: Duration = Duration::from_secs(2);
const VERSION_TIMEOUT: Duration = Duration::from_secs(5);
const AUTH_TIMEOUT: Duration = Duration::from_secs(15);
const MAX_PROBE_BYTES: usize = 1024 * 1024;

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
        let control = process.control();
        Ok(ClaudeSession {
            process: Mutex::new(process),
            control,
            session_id,
            request: request.clone(),
            active: AtomicBool::new(false),
            cancel_requested: AtomicBool::new(false),
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
    request: SessionRequest,
    active: AtomicBool,
    cancel_requested: AtomicBool,
    stopped: AtomicBool,
}

impl ClaudeSession {
    async fn run_turn(&self, request: &TurnRequest) -> Result<Vec<ProviderEvent>, ProviderError> {
        if self.stopped.load(Ordering::Acquire) {
            return Err(ProviderError::Protocol(
                "Claude Code session has already stopped".to_owned(),
            ));
        }
        if self.active.swap(true, Ordering::AcqRel) {
            return Err(ProviderError::Protocol(
                "Claude Code session already has an active turn".to_owned(),
            ));
        }
        self.cancel_requested.store(false, Ordering::Release);
        let mut process = self.process.lock().await;
        let outcome = self.collect_turn(&mut process, request).await;
        self.active.store(false, Ordering::Release);
        outcome
    }

    async fn collect_turn(
        &self,
        process: &mut NdjsonProcess,
        request: &TurnRequest,
    ) -> Result<Vec<ProviderEvent>, ProviderError> {
        self.control
            .send(&ClaudeAdapter::turn_input(request))
            .await?;
        let started = Instant::now();
        let mut last_progress = started;
        let mut turn_started = false;
        let mut events = Vec::new();
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
                    events.push(ProviderEvent::Cancelled {
                        provider: ProviderKind::ClaudeCode,
                        session_id: self.session_id.clone(),
                        turn_id: Some(request.correlation_id.to_string()),
                        raw: None,
                    });
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
            let terminal = is_result_message(&message);
            for event in normalize_claude_message(&message)? {
                if !turn_started && !matches!(event, ProviderEvent::SessionStarted { .. }) {
                    events.push(ProviderEvent::TurnStarted {
                        provider: ProviderKind::ClaudeCode,
                        session_id: self.session_id.clone(),
                        turn_id: Some(request.correlation_id.to_string()),
                        raw: None,
                    });
                    turn_started = true;
                }
                events.push(event);
            }
            if terminal {
                return Ok(events);
            }
        }
    }
}

impl ProviderSession for ClaudeSession {
    fn id(&self) -> String {
        self.session_id.clone()
    }

    fn turn<'a>(&'a self, request: &'a TurnRequest) -> AdapterFuture<'a, Vec<ProviderEvent>> {
        Box::pin(async move { self.run_turn(request).await })
    }

    fn cancel(&self) -> AdapterFuture<'_, ()> {
        Box::pin(async move {
            if self.active.load(Ordering::Acquire) {
                self.cancel_requested.store(true, Ordering::Release);
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
            status: "success".to_owned(),
            output: message
                .get("result")
                .and_then(Value::as_str)
                .map(str::to_owned),
            raw,
        });
    } else {
        events.push(ProviderEvent::Error {
            provider: ProviderKind::ClaudeCode,
            session_id: Some(session_id),
            code: message
                .get("terminal_reason")
                .or_else(|| message.get("subtype"))
                .and_then(Value::as_str)
                .unwrap_or("claude-result")
                .to_owned(),
            message: message
                .get("result")
                .or_else(|| message.get("error"))
                .and_then(Value::as_str)
                .unwrap_or("Claude Code turn failed")
                .to_owned(),
            retryable: false,
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
