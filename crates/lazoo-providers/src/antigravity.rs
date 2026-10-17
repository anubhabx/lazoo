use std::{
    cmp,
    path::{Path, PathBuf},
    sync::{
        RwLock,
        atomic::{AtomicBool, Ordering},
    },
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

const ANTIGRAVITY_PROTOCOL: &str = "agy/stream-json-v1";
const STOP_TIMEOUT: Duration = Duration::from_secs(2);
const VERSION_TIMEOUT: Duration = Duration::from_secs(5);
const PROBE_TIMEOUT: Duration = Duration::from_secs(30);
const MAX_PROBE_BYTES: usize = 1024 * 1024;

#[derive(Clone, Debug, Default)]
pub struct AntigravityAdapter {
    executable: Option<PathBuf>,
}

impl AntigravityAdapter {
    #[must_use]
    pub fn with_executable(executable: impl Into<PathBuf>) -> Self {
        Self {
            executable: Some(executable.into()),
        }
    }

    #[must_use]
    pub fn command(executable: &Path, request: &SessionRequest) -> CommandSpec {
        let mut command = CommandSpec::new(executable, &request.cwd)
            .arg("--input-format")
            .arg("stream-json")
            .arg("--output-format")
            .arg("stream-json")
            .arg("--print-timeout")
            .arg(format!("{}ms", request.turn_timeout_millis))
            .arg("--add-dir")
            .arg(request.cwd.as_os_str());
        for root in &request.workspace_roots {
            command = command.arg("--add-dir").arg(root.as_os_str());
        }
        if let Some(session_id) = &request.session_id {
            command = command.arg("--conversation").arg(session_id);
        }
        if let Some(model) = &request.model {
            command = command.arg("--model").arg(model);
        }
        if let Some(effort) = &request.effort {
            command = command.arg("--effort").arg(effort);
        }
        match request.permission_mode.as_deref() {
            Some("plan" | "never" | "untrusted") => command = command.arg("--mode").arg("plan"),
            Some("accept-edits" | "acceptEdits") => {
                command = command.arg("--mode").arg("accept-edits");
            }
            _ => {}
        }
        command
    }

    #[must_use]
    pub fn turn_input(request: &TurnRequest) -> Value {
        json!({
            "event": "user",
            "message": {"content": [{"type": "text", "text": request.prompt}]}
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

    fn executable_candidates(
        &self,
        environment: &DiscoveryEnvironment,
    ) -> Vec<ExecutableCandidate> {
        self.executable.as_ref().map_or_else(
            || environment.candidates(ProviderKind::Antigravity, &["agy.exe", "agy"]),
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
        let cwd = match std::env::current_dir() {
            Ok(cwd) => cwd,
            Err(error) => {
                return degraded_probe(
                    candidate,
                    Some(version),
                    format!("current directory was unavailable: {error}"),
                );
            }
        };
        match read_usage(&candidate.path, &cwd).await {
            Ok(usage) => configured_probe(candidate, version, &usage),
            Err(error) => degraded_probe(
                candidate,
                Some(version),
                format!("native usage probe failed: {error}"),
            ),
        }
    }

    fn start_session(&self, request: &SessionRequest) -> Result<AntigravitySession, ProviderError> {
        validate_request(request)?;
        let environment = DiscoveryEnvironment::current();
        let candidate = self
            .executable_candidates(&environment)
            .into_iter()
            .next()
            .ok_or(ProviderError::NotDetected(ProviderKind::Antigravity))?;
        let process = NdjsonProcess::spawn(&Self::command(&candidate.path, request))?;
        let native_host_pid = process.native_host_pid();
        let job = process.job();
        let control = process.control();
        let initial_id = request
            .session_id
            .clone()
            .unwrap_or_else(|| format!("pending:{}", uuid::Uuid::new_v4()));
        Ok(AntigravitySession {
            process: Mutex::new(process),
            control,
            session_id: RwLock::new(initial_id),
            native_host_pid,
            job,
            request: request.clone(),
            active: AtomicBool::new(false),
            cancel_requested: AtomicBool::new(false),
            stopped: AtomicBool::new(false),
        })
    }
}

impl ProviderAdapter for AntigravityAdapter {
    type Session = AntigravitySession;

    fn kind(&self) -> ProviderKind {
        ProviderKind::Antigravity
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
                return Ok(ProviderProbe::not_detected(ProviderKind::Antigravity));
            };
            Ok(self.probe_candidate(candidate).await)
        })
    }

    fn start<'a>(&'a self, request: &'a SessionRequest) -> AdapterFuture<'a, Self::Session> {
        Box::pin(std::future::ready(self.start_session(request)))
    }
}

#[derive(Debug)]
pub struct AntigravitySession {
    process: Mutex<NdjsonProcess>,
    control: NdjsonControl,
    session_id: RwLock<String>,
    native_host_pid: Option<u32>,
    job: Option<std::sync::Arc<lazoo_runtime::JobObject>>,
    request: SessionRequest,
    active: AtomicBool,
    cancel_requested: AtomicBool,
    stopped: AtomicBool,
}

impl AntigravitySession {
    async fn run_turn(&self, request: &TurnRequest) -> Result<Vec<ProviderEvent>, ProviderError> {
        if request.read_only {
            return Err(ProviderError::Unsupported {
                provider: ProviderKind::Antigravity,
                capability: "read-only turns",
                detail: None,
            });
        }
        if self.stopped.load(Ordering::Acquire) {
            return Err(ProviderError::Protocol(
                "Antigravity session has already stopped".to_owned(),
            ));
        }
        if self.active.swap(true, Ordering::AcqRel) {
            return Err(ProviderError::Protocol(
                "Antigravity session already has an active turn".to_owned(),
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
            .send(&AntigravityAdapter::turn_input(request))
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
                    phase: "Antigravity turn completion",
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
                .next_message(wait, "Antigravity turn progress")
                .await
            {
                Err(ProviderError::ProcessExited { .. })
                    if self.cancel_requested.load(Ordering::Acquire) =>
                {
                    events.push(ProviderEvent::Cancelled {
                        provider: ProviderKind::Antigravity,
                        session_id: self.current_session_id(),
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
                        phase: "Antigravity turn completion",
                    });
                }
                result => result?,
            };
            last_progress = Instant::now();
            let terminal = is_result_message(&message);
            let normalized = normalize_antigravity_message(&message)?;
            for event in normalized {
                if let ProviderEvent::SessionStarted { session_id, .. } = &event {
                    self.set_session_id(session_id.clone());
                }
                if !turn_started && !matches!(event, ProviderEvent::SessionStarted { .. }) {
                    events.push(ProviderEvent::TurnStarted {
                        provider: ProviderKind::Antigravity,
                        session_id: self.current_session_id(),
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

    fn current_session_id(&self) -> String {
        self.session_id
            .read()
            .map_or_else(|poisoned| poisoned.into_inner().clone(), |id| id.clone())
    }

    fn set_session_id(&self, value: String) {
        match self.session_id.write() {
            Ok(mut id) => *id = value,
            Err(poisoned) => *poisoned.into_inner() = value,
        }
    }
}

impl ProviderSession for AntigravitySession {
    fn id(&self) -> String {
        self.current_session_id()
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

pub fn normalize_antigravity_message(message: &Value) -> Result<Vec<ProviderEvent>, ProviderError> {
    let event = message
        .get("event")
        .or_else(|| message.get("type"))
        .and_then(Value::as_str)
        .ok_or_else(|| {
            ProviderError::InvalidEvent("Antigravity record omitted event".to_owned())
        })?;
    match event {
        "init" => normalize_init(message),
        "step_update" => normalize_step_update(message),
        "result" => normalize_result(message),
        _ => Ok(vec![ProviderEvent::Progress {
            provider: ProviderKind::Antigravity,
            session_id: conversation_id(message).unwrap_or_default(),
            message: Some(event.to_owned()),
            raw: Some(redact_json(message.clone())),
        }]),
    }
}

fn normalize_init(message: &Value) -> Result<Vec<ProviderEvent>, ProviderError> {
    Ok(vec![ProviderEvent::SessionStarted {
        provider: ProviderKind::Antigravity,
        session_id: required_conversation_id(message, "init")?,
        resumed: false,
        raw: Some(redact_json(message.clone())),
    }])
}

fn normalize_step_update(message: &Value) -> Result<Vec<ProviderEvent>, ProviderError> {
    let update = message.get("step_update").unwrap_or(message);
    let session_id = required_conversation_id(message, "step_update")?;
    let step_type = update
        .get("step_type")
        .and_then(Value::as_str)
        .ok_or_else(|| {
            ProviderError::InvalidEvent("Antigravity step_update omitted step_type".to_owned())
        })?;
    let raw = Some(redact_json(message.clone()));
    let events = match step_type {
        "user_input" => Vec::new(),
        "agent_response" => vec![ProviderEvent::TextDelta {
            provider: ProviderKind::Antigravity,
            session_id,
            turn_id: None,
            text: step_text(update, step_type)?,
            raw,
        }],
        "thinking" => vec![ProviderEvent::ThoughtDelta {
            provider: ProviderKind::Antigravity,
            session_id,
            turn_id: None,
            text: step_text(update, step_type)?,
            raw,
        }],
        "tool_use" => normalize_tool_step(update, session_id, raw),
        "subagent" => vec![ProviderEvent::ToolUpdate {
            provider: ProviderKind::Antigravity,
            session_id,
            turn_id: None,
            tool_name: update
                .pointer("/subagent_info/agent_name")
                .and_then(Value::as_str)
                .map(str::to_owned),
            status: update
                .get("state")
                .and_then(Value::as_str)
                .unwrap_or("updated")
                .to_ascii_lowercase(),
            raw,
        }],
        _ => vec![ProviderEvent::Progress {
            provider: ProviderKind::Antigravity,
            session_id,
            message: Some(step_type.to_owned()),
            raw,
        }],
    };
    Ok(events)
}

fn normalize_tool_step(
    update: &Value,
    session_id: String,
    raw: Option<Value>,
) -> Vec<ProviderEvent> {
    let tool = update.get("tool_info").unwrap_or(&Value::Null);
    let tool_name = tool
        .get("tool_name")
        .and_then(Value::as_str)
        .map(str::to_owned);
    let status = update
        .get("state")
        .and_then(Value::as_str)
        .unwrap_or("updated")
        .to_ascii_lowercase();
    let mut events = vec![ProviderEvent::ToolUpdate {
        provider: ProviderKind::Antigravity,
        session_id: session_id.clone(),
        turn_id: None,
        tool_name: tool_name.clone(),
        status,
        raw: raw.clone(),
    }];
    if tool_name.as_deref() == Some("run_command") {
        events.push(ProviderEvent::TerminalUpdate {
            provider: ProviderKind::Antigravity,
            session_id,
            terminal_id: None,
            output: tool
                .get("output")
                .and_then(Value::as_str)
                .map(str::to_owned),
            raw,
        });
    }
    events
}

fn normalize_result(message: &Value) -> Result<Vec<ProviderEvent>, ProviderError> {
    let result = message.get("result").unwrap_or(message);
    let session_id = required_conversation_id(message, "result")?;
    let status = result
        .get("status")
        .and_then(Value::as_str)
        .unwrap_or("ERROR");
    let raw = Some(redact_json(message.clone()));
    let mut events = Vec::new();
    if let Some(usage) = result.get("usage").or_else(|| result.get("metadata")) {
        events.push(ProviderEvent::Usage {
            provider: ProviderKind::Antigravity,
            session_id: session_id.clone(),
            input_tokens: usage.get("input_tokens").and_then(Value::as_u64),
            output_tokens: usage.get("output_tokens").and_then(Value::as_u64),
            total_tokens: usage
                .get("total_tokens")
                .or_else(|| usage.get("tokens_used"))
                .and_then(Value::as_u64),
            cost_usd: None,
            raw: raw.clone(),
        });
    }
    if status.eq_ignore_ascii_case("success") {
        events.push(ProviderEvent::TurnCompleted {
            provider: ProviderKind::Antigravity,
            session_id,
            turn_id: None,
            status: status.to_ascii_lowercase(),
            output: result
                .get("response")
                .or_else(|| result.get("result"))
                .and_then(Value::as_str)
                .map(str::to_owned),
            raw,
        });
    } else {
        events.push(ProviderEvent::Error {
            provider: ProviderKind::Antigravity,
            session_id: Some(session_id),
            code: "antigravity-result".to_owned(),
            message: result
                .get("error")
                .or_else(|| result.get("response"))
                .and_then(Value::as_str)
                .unwrap_or("Antigravity turn failed")
                .to_owned(),
            retryable: false,
            raw,
        });
    }
    Ok(events)
}

fn conversation_id(message: &Value) -> Option<String> {
    message
        .get("conversation_id")
        .or_else(|| message.pointer("/init/conversation_id"))
        .or_else(|| message.pointer("/step_update/conversation_id"))
        .or_else(|| message.pointer("/result/conversation_id"))
        .or_else(|| message.get("session_id"))
        .and_then(Value::as_str)
        .map(str::to_owned)
}

fn required_conversation_id(message: &Value, event: &str) -> Result<String, ProviderError> {
    conversation_id(message).ok_or_else(|| {
        ProviderError::InvalidEvent(format!(
            "Antigravity {event} record omitted conversation_id"
        ))
    })
}

fn step_text(update: &Value, step_type: &str) -> Result<String, ProviderError> {
    update
        .get("text_delta")
        .or_else(|| update.get("content"))
        .and_then(Value::as_str)
        .map(str::to_owned)
        .ok_or_else(|| {
            ProviderError::InvalidEvent(format!("Antigravity {step_type} step omitted text_delta"))
        })
}

fn is_result_message(message: &Value) -> bool {
    message
        .get("event")
        .or_else(|| message.get("type"))
        .and_then(Value::as_str)
        == Some("result")
}

fn validate_request(request: &SessionRequest) -> Result<(), ProviderError> {
    if !request.cwd.is_absolute() {
        return Err(ProviderError::Protocol(
            "Antigravity workspace path must be absolute".to_owned(),
        ));
    }
    if request
        .workspace_roots
        .iter()
        .any(|root| !root.is_absolute())
    {
        return Err(ProviderError::Protocol(
            "Antigravity additional workspace roots must be absolute".to_owned(),
        ));
    }
    if let Some(effort) = request.effort.as_deref()
        && !matches!(effort, "low" | "medium" | "high")
    {
        return Err(ProviderError::Protocol(format!(
            "unsupported Antigravity effort: {effort}"
        )));
    }
    if let Some(mode) = request.permission_mode.as_deref()
        && !matches!(
            mode,
            "ask"
                | "default"
                | "on-request"
                | "plan"
                | "never"
                | "untrusted"
                | "accept-edits"
                | "acceptEdits"
        )
    {
        return Err(ProviderError::Protocol(format!(
            "unsupported Antigravity permission mode: {mode}"
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
        phase: "Antigravity version probe",
    })??;
    checked_output(&output, "Antigravity version", 64 * 1024)
}

async fn read_usage(path: &Path, cwd: &Path) -> Result<Value, ProviderError> {
    let output = timeout(
        PROBE_TIMEOUT,
        Command::new(path)
            .arg("--print")
            .arg("/usage")
            .arg("--output-format")
            .arg("json")
            .arg("--print-timeout")
            .arg("30s")
            .arg("--mode")
            .arg("plan")
            .arg("--add-dir")
            .arg(cwd)
            .current_dir(cwd)
            .output(),
    )
    .await
    .map_err(|_| ProviderError::Timeout {
        phase: "Antigravity native usage probe",
    })??;
    let json = checked_output(&output, "Antigravity usage", MAX_PROBE_BYTES)?;
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
    usage: &Value,
) -> ProviderProbe {
    let success = usage
        .get("status")
        .and_then(Value::as_str)
        .is_some_and(|status| status.eq_ignore_ascii_case("success"));
    if !success {
        return degraded_probe(
            candidate,
            Some(version),
            "native usage probe did not report success".to_owned(),
        );
    }
    ProviderProbe {
        contract_version: ADAPTER_CONTRACT_V1,
        provider: ProviderKind::Antigravity,
        verification: ProviderVerification::Configured,
        executable: Some(candidate.path),
        discovery_source: Some(candidate.source),
        version: Some(version),
        protocol: Some(ANTIGRAVITY_PROTOCOL.to_owned()),
        capabilities: AntigravityAdapter::capabilities(),
        authentication: AuthenticationEvidence {
            native: AuthenticationState::Ready,
            external: AuthenticationState::Unknown,
            detail: Some("Antigravity native /usage command completed".to_owned()),
        },
        degradation_reason: None,
        evidence: vec![
            "version command completed".to_owned(),
            "native /usage probe completed without exposing quota data".to_owned(),
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
        provider: ProviderKind::Antigravity,
        verification: ProviderVerification::Degraded,
        executable: Some(candidate.path),
        discovery_source: Some(candidate.source),
        version,
        protocol: Some(ANTIGRAVITY_PROTOCOL.to_owned()),
        capabilities: ProviderCapabilities::default(),
        authentication: AuthenticationEvidence::unknown(),
        degradation_reason: Some(reason),
        evidence: Vec::new(),
        checked_at: chrono::Utc::now().to_rfc3339(),
    }
}
