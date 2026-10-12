use std::{
    cmp,
    path::{Path, PathBuf},
    sync::RwLock,
    time::{Duration, Instant},
};

use serde_json::{Map, Value, json};
use tokio::{process::Command, sync::Mutex, time::timeout};

use crate::{
    ADAPTER_CONTRACT_V1, AdapterFuture, AuthenticationEvidence, AuthenticationState, CommandSpec,
    DiscoveryEnvironment, ExecutableCandidate, PermissionDecision, PermissionKind,
    PermissionRequest, ProviderAdapter, ProviderCapabilities, ProviderError, ProviderEvent,
    ProviderKind, ProviderProbe, ProviderSession, ProviderVerification, SessionRequest, ToolCall,
    ToolResult, ToolSpec, TurnEventObserver, TurnRequest, json_rpc::JsonRpcControl,
    json_rpc::JsonRpcProcess, json_rpc::response_id, redact_json,
};

const MAX_PERMISSION_COMMAND_BYTES: usize = 4096;
const MAX_PERMISSION_ACTION_BYTES: usize = 1024;
/// Tool arguments and results are model-facing text; both are bounded before they cross the
/// control-plane boundary or land in the turn ledger.
const MAX_TOOL_ARGUMENT_BYTES: usize = 64 * 1024;
const MAX_TOOL_RESULT_BYTES: usize = 64 * 1024;

const CODEX_PROTOCOL: &str = "app-server/v2";
const STOP_TIMEOUT: Duration = Duration::from_secs(2);
const VERSION_TIMEOUT: Duration = Duration::from_secs(5);
const MAX_VERSION_BYTES: usize = 64 * 1024;

#[derive(Clone, Debug, Default)]
pub struct CodexAdapter {
    executable: Option<PathBuf>,
}

impl CodexAdapter {
    #[must_use]
    pub fn with_executable(executable: impl Into<PathBuf>) -> Self {
        Self {
            executable: Some(executable.into()),
        }
    }

    #[must_use]
    pub fn command(executable: &Path, cwd: &Path) -> CommandSpec {
        CommandSpec::new(executable, cwd)
            .arg("app-server")
            .arg("--listen")
            .arg("stdio://")
    }

    #[must_use]
    pub fn fallback_command(
        executable: &Path,
        request: &SessionRequest,
        prompt: &str,
    ) -> CommandSpec {
        let mut command = CommandSpec::new(executable, &request.cwd)
            .arg("exec")
            .arg("--json")
            .arg("--cd")
            .arg(request.cwd.as_os_str());
        if let Some(model) = &request.model {
            command = command.arg("--model").arg(model);
        }
        command.arg(prompt)
    }

    #[must_use]
    pub const fn capabilities() -> ProviderCapabilities {
        ProviderCapabilities {
            session: true,
            stream: true,
            cancel: true,
            resume: true,
            terminal: true,
            permission: true,
            mcp: true,
            plugins: true,
            skills: true,
            connectors: true,
            native_message: false,
            model_selection: true,
        }
    }

    #[must_use]
    pub fn interrupt_params(thread_id: &str, turn_id: &str) -> Value {
        json!({"threadId": thread_id, "turnId": turn_id})
    }

    #[must_use]
    pub fn detected_probe(executable: PathBuf, version: String) -> ProviderProbe {
        ProviderProbe {
            contract_version: ADAPTER_CONTRACT_V1,
            provider: ProviderKind::Codex,
            verification: ProviderVerification::Detected,
            executable: Some(executable),
            discovery_source: None,
            version: Some(version),
            protocol: None,
            capabilities: ProviderCapabilities::default(),
            authentication: AuthenticationEvidence::unknown(),
            degradation_reason: None,
            evidence: vec!["version command completed".to_owned()],
            checked_at: chrono::Utc::now().to_rfc3339(),
        }
    }

    fn executable_candidates(
        &self,
        environment: &DiscoveryEnvironment,
    ) -> Vec<ExecutableCandidate> {
        self.executable.as_ref().map_or_else(
            || environment.candidates(ProviderKind::Codex, &["codex.exe"]),
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
        let mut process = match JsonRpcProcess::spawn(&Self::command(&candidate.path, &cwd)) {
            Ok(process) => process,
            Err(error) => {
                return degraded_probe(
                    candidate,
                    Some(version),
                    format!("app-server spawn failed: {error}"),
                );
            }
        };
        let handshake = initialize(&mut process, Duration::from_secs(15), false).await;
        let (initialized, account) = match handshake {
            Ok(initialized) => {
                let account = process
                    .request(
                        "account/read",
                        json!({"refreshToken": false}),
                        Duration::from_secs(15),
                        "Codex account probe",
                    )
                    .await;
                (initialized, account)
            }
            Err(error) => {
                let stderr = process.stderr_tail().await;
                let _ = process.stop(STOP_TIMEOUT).await;
                let suffix = if stderr.is_empty() {
                    String::new()
                } else {
                    " (provider stderr was captured but omitted from public evidence)".to_owned()
                };
                return degraded_probe(
                    candidate,
                    Some(version),
                    format!("app-server handshake failed: {error}{suffix}"),
                );
            }
        };
        let _ = process.stop(STOP_TIMEOUT).await;

        match account {
            Ok(account) => configured_probe(candidate, version, &initialized, &account),
            Err(error) => degraded_probe(
                candidate,
                Some(version),
                format!("account readiness probe failed: {error}"),
            ),
        }
    }

    async fn start_session(&self, request: &SessionRequest) -> Result<CodexSession, ProviderError> {
        if !request.cwd.is_absolute() {
            return Err(ProviderError::Protocol(
                "Codex workspace path must be absolute".to_owned(),
            ));
        }
        let environment = DiscoveryEnvironment::current();
        let candidate = self
            .executable_candidates(&environment)
            .into_iter()
            .next()
            .ok_or(ProviderError::NotDetected(ProviderKind::Codex))?;
        let mut process = JsonRpcProcess::spawn(&Self::command(&candidate.path, &request.cwd))?;
        // Dynamic tools are an experimental app-server field; opt in only when Lazoo declares them.
        initialize(
            &mut process,
            request.startup_timeout(),
            !request.tools.is_empty(),
        )
        .await?;

        let (method, params, resumed) = if let Some(session_id) = &request.session_id {
            (
                "thread/resume",
                thread_resume_params(session_id, request)?,
                true,
            )
        } else {
            ("thread/start", thread_start_params(request)?, false)
        };
        let result = process
            .request(
                method,
                params,
                request.startup_timeout(),
                "Codex thread start",
            )
            .await?;
        let session_id = result
            .pointer("/thread/id")
            .and_then(Value::as_str)
            .ok_or_else(|| {
                ProviderError::InvalidEvent("Codex thread response omitted thread.id".to_owned())
            })?
            .to_owned();
        let native_host_pid = process.native_host_pid();
        let job = Some(process.job());
        let control = process.control();
        Ok(CodexSession {
            process: Mutex::new(process),
            control,
            session_id,
            native_host_pid,
            job,
            resumed,
            request: request.clone(),
            active_turn: RwLock::new(None),
        })
    }
}

impl ProviderAdapter for CodexAdapter {
    type Session = CodexSession;

    fn kind(&self) -> ProviderKind {
        ProviderKind::Codex
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
                return Ok(ProviderProbe::not_detected(ProviderKind::Codex));
            };
            Ok(self.probe_candidate(candidate).await)
        })
    }

    fn start<'a>(&'a self, request: &'a SessionRequest) -> AdapterFuture<'a, Self::Session> {
        Box::pin(async move { self.start_session(request).await })
    }
}

#[derive(Debug)]
pub struct CodexSession {
    process: Mutex<JsonRpcProcess>,
    control: JsonRpcControl,
    session_id: String,
    native_host_pid: Option<u32>,
    job: Option<std::sync::Arc<lazoo_runtime::JobObject>>,
    resumed: bool,
    request: SessionRequest,
    active_turn: RwLock<Option<String>>,
}

impl CodexSession {
    async fn run_turn(&self, request: &TurnRequest) -> Result<Vec<ProviderEvent>, ProviderError> {
        let mut process = self.process.lock().await;
        let result = process
            .request(
                "turn/start",
                turn_start_params(&self.session_id, &self.request, request)?,
                self.request.startup_timeout(),
                "Codex turn acceptance",
            )
            .await?;
        let turn_id = result
            .pointer("/turn/id")
            .and_then(Value::as_str)
            .ok_or_else(|| {
                ProviderError::InvalidEvent("Codex turn response omitted turn.id".to_owned())
            })?
            .to_owned();
        self.set_active_turn(Some(turn_id));
        let outcome = self
            .collect_turn(&mut process, self.initial_turn_events(), None, None)
            .await;
        self.set_active_turn(None);
        outcome
    }

    async fn run_turn_observed(
        &self,
        request: &TurnRequest,
        observer: &dyn TurnEventObserver,
    ) -> Result<Vec<ProviderEvent>, ProviderError> {
        let mut process = self.process.lock().await;
        let result = process
            .request(
                "turn/start",
                turn_start_params(&self.session_id, &self.request, request)?,
                self.request.startup_timeout(),
                "Codex turn acceptance",
            )
            .await?;
        let turn_id = result
            .pointer("/turn/id")
            .and_then(Value::as_str)
            .ok_or_else(|| {
                ProviderError::InvalidEvent("Codex turn response omitted turn.id".to_owned())
            })?
            .to_owned();
        self.set_active_turn(Some(turn_id.clone()));

        let mut events = self.initial_turn_events();
        let started = ProviderEvent::TurnStarted {
            provider: ProviderKind::Codex,
            session_id: self.session_id.clone(),
            turn_id: Some(turn_id.clone()),
            raw: None,
        };
        let initial_observation = events
            .iter()
            .chain(std::iter::once(&started))
            .try_for_each(|event| observer.observe(event));
        if let Err(error) = initial_observation {
            self.interrupt_after_observer_error(&turn_id, &error).await;
            self.set_active_turn(None);
            return Err(error);
        }
        events.push(started);

        let outcome = self
            .collect_turn(&mut process, events, Some(observer), Some(&turn_id))
            .await;
        if let Err(error @ ProviderError::Observer(_)) = &outcome {
            self.interrupt_after_observer_error(&turn_id, error).await;
        }
        self.set_active_turn(None);
        outcome
    }

    fn initial_turn_events(&self) -> Vec<ProviderEvent> {
        if self.resumed {
            vec![ProviderEvent::SessionStarted {
                provider: ProviderKind::Codex,
                session_id: self.session_id.clone(),
                resumed: true,
                raw: None,
            }]
        } else {
            Vec::new()
        }
    }

    async fn interrupt_after_observer_error(&self, turn_id: &str, _error: &ProviderError) {
        let _ = self
            .control
            .send_uncorrelated_request(
                "turn/interrupt",
                CodexAdapter::interrupt_params(&self.session_id, turn_id),
            )
            .await;
    }

    async fn collect_turn(
        &self,
        process: &mut JsonRpcProcess,
        mut events: Vec<ProviderEvent>,
        observer: Option<&dyn TurnEventObserver>,
        accepted_turn_id: Option<&str>,
    ) -> Result<Vec<ProviderEvent>, ProviderError> {
        let mut started = Instant::now();
        let mut last_progress = started;
        let mut output = String::new();
        loop {
            let total_remaining = self
                .request
                .turn_timeout()
                .checked_sub(started.elapsed())
                .ok_or(ProviderError::Timeout {
                    phase: "Codex turn completion",
                })?;
            let progress_remaining = self
                .request
                .progress_timeout()
                .checked_sub(last_progress.elapsed())
                .ok_or(ProviderError::Stalled {
                    millis: self.request.progress_timeout_millis,
                })?;
            let wait = cmp::min(total_remaining, progress_remaining);
            let message = match process.next_message(wait, "Codex turn progress").await {
                Err(ProviderError::Timeout { .. }) if progress_remaining <= total_remaining => {
                    return Err(ProviderError::Stalled {
                        millis: self.request.progress_timeout_millis,
                    });
                }
                Err(ProviderError::Timeout { .. }) => {
                    return Err(ProviderError::Timeout {
                        phase: "Codex turn completion",
                    });
                }
                result => result?,
            };
            let paused_at = Instant::now();
            if let Some(permission) = self
                .handle_server_request(&message, observer, accepted_turn_id)
                .await?
            {
                // Waiting for a Lazoo approval decision never counts against the turn budget.
                started += paused_at.elapsed();
                last_progress = Instant::now();
                if let Some(observer) = observer {
                    observer.observe(&permission)?;
                }
                events.push(permission);
                continue;
            }
            let Some(mut event) = normalize_codex_message(&message)? else {
                continue;
            };
            last_progress = Instant::now();
            if let ProviderEvent::TextDelta { text, .. } = &event {
                output.push_str(text);
            }
            let completed = matches!(event, ProviderEvent::TurnCompleted { .. });
            if let ProviderEvent::TurnCompleted {
                output: final_output,
                ..
            } = &mut event
            {
                *final_output = (!output.is_empty()).then_some(output.clone());
            }
            let duplicate_started = matches!(
                &event,
                ProviderEvent::TurnStarted {
                    session_id,
                    turn_id: Some(turn_id),
                    ..
                } if accepted_turn_id == Some(turn_id.as_str()) && session_id == &self.session_id
            );
            if duplicate_started {
                continue;
            }
            if let Some(observer) = observer {
                observer.observe(&event)?;
            }
            events.push(event);
            if completed {
                return Ok(events);
            }
        }
    }

    fn set_active_turn(&self, value: Option<String>) {
        if let Ok(mut active) = self.active_turn.write() {
            *active = value;
        }
    }
}

impl ProviderSession for CodexSession {
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
            let active = self
                .active_turn
                .read()
                .ok()
                .and_then(|active| active.clone());
            if let Some(turn_id) = active {
                self.control
                    .send_uncorrelated_request(
                        "turn/interrupt",
                        CodexAdapter::interrupt_params(&self.session_id, &turn_id),
                    )
                    .await?;
            }
            Ok(())
        })
    }

    fn stop(&self) -> AdapterFuture<'_, ()> {
        Box::pin(async move { self.process.lock().await.stop(STOP_TIMEOUT).await })
    }
}

pub fn normalize_codex_message(message: &Value) -> Result<Option<ProviderEvent>, ProviderError> {
    let Some(method) = message.get("method").and_then(Value::as_str) else {
        return Ok(None);
    };
    let params = message.get("params").cloned().unwrap_or_else(|| json!({}));
    let thread_id = string_at(&params, "/threadId")
        .or_else(|| string_at(&params, "/thread/id"))
        .unwrap_or_default();
    let turn_id = string_at(&params, "/turnId").or_else(|| string_at(&params, "/turn/id"));
    match method {
        "thread/started" | "turn/started" | "turn/completed" => {
            normalize_codex_lifecycle(method, &params, thread_id, turn_id)
        }
        "item/agentMessage/delta"
        | "item/reasoning/textDelta"
        | "item/reasoning/summaryTextDelta"
        | "item/started"
        | "item/completed"
        | "item/commandExecution/outputDelta"
        | "command/exec/outputDelta"
        | "item/commandExecution/terminalInteraction"
        | "thread/tokenUsage/updated"
        | "item/plan/delta"
        | "item/fileChange/outputDelta"
        | "item/fileChange/patchUpdated"
        | "item/mcpToolCall/progress"
        | "turn/plan/updated"
        | "turn/diff/updated"
        | "warning" => normalize_codex_stream(method, &params, thread_id, turn_id),
        "error" | "process/exited" => normalize_codex_failure(method, &params, thread_id),
        _ => Ok(None),
    }
}

fn normalize_codex_lifecycle(
    method: &str,
    params: &Value,
    thread_id: String,
    turn_id: Option<String>,
) -> Result<Option<ProviderEvent>, ProviderError> {
    let raw = Some(codex_raw(params));
    let event = match method {
        "thread/started" => {
            let session_id = required_string(params, "/thread/id", method)?;
            Some(ProviderEvent::SessionStarted {
                provider: ProviderKind::Codex,
                session_id,
                resumed: false,
                raw,
            })
        }
        "turn/started" => Some(ProviderEvent::TurnStarted {
            provider: ProviderKind::Codex,
            session_id: required_nonempty(thread_id, "turn/started threadId")?,
            turn_id,
            raw,
        }),
        "turn/completed" => Some(ProviderEvent::TurnCompleted {
            provider: ProviderKind::Codex,
            session_id: required_nonempty(thread_id, "turn/completed threadId")?,
            turn_id,
            status: required_string(params, "/turn/status", method)?,
            output: None,
            raw,
        }),
        _ => None,
    };
    Ok(event)
}

fn normalize_codex_stream(
    method: &str,
    params: &Value,
    thread_id: String,
    turn_id: Option<String>,
) -> Result<Option<ProviderEvent>, ProviderError> {
    let raw = Some(codex_raw(params));
    let event = match method {
        "item/agentMessage/delta" => Some(ProviderEvent::TextDelta {
            provider: ProviderKind::Codex,
            session_id: required_nonempty(thread_id, "agent delta threadId")?,
            turn_id,
            text: required_string(params, "/delta", method)?,
            raw,
        }),
        "item/reasoning/textDelta" | "item/reasoning/summaryTextDelta" => {
            Some(ProviderEvent::ThoughtDelta {
                provider: ProviderKind::Codex,
                session_id: required_nonempty(thread_id, "reasoning delta threadId")?,
                turn_id,
                text: required_string(params, "/delta", method)?,
                raw,
            })
        }
        "item/started" | "item/completed" => {
            normalize_codex_item(method, params, thread_id, turn_id, raw)?
        }
        "item/commandExecution/outputDelta" | "command/exec/outputDelta" => {
            Some(ProviderEvent::TerminalUpdate {
                provider: ProviderKind::Codex,
                session_id: required_nonempty(thread_id, "terminal delta threadId")?,
                terminal_id: string_at(params, "/itemId")
                    .or_else(|| string_at(params, "/processId")),
                output: string_at(params, "/delta"),
                raw,
            })
        }
        "item/commandExecution/terminalInteraction" => Some(ProviderEvent::TerminalUpdate {
            provider: ProviderKind::Codex,
            session_id: required_nonempty(thread_id, "terminal interaction threadId")?,
            terminal_id: string_at(params, "/processId"),
            output: string_at(params, "/stdin"),
            raw,
        }),
        "thread/tokenUsage/updated" => {
            let total = params.pointer("/tokenUsage/total").unwrap_or(&Value::Null);
            Some(ProviderEvent::Usage {
                provider: ProviderKind::Codex,
                session_id: required_nonempty(thread_id, "usage threadId")?,
                input_tokens: total.get("inputTokens").and_then(Value::as_u64),
                output_tokens: total.get("outputTokens").and_then(Value::as_u64),
                total_tokens: total.get("totalTokens").and_then(Value::as_u64),
                cost_usd: None,
                raw,
            })
        }
        "item/plan/delta"
        | "item/fileChange/outputDelta"
        | "item/fileChange/patchUpdated"
        | "item/mcpToolCall/progress"
        | "turn/plan/updated"
        | "turn/diff/updated"
        | "warning" => Some(ProviderEvent::Progress {
            provider: ProviderKind::Codex,
            session_id: thread_id,
            message: string_at(params, "/delta").or_else(|| string_at(params, "/message")),
            raw,
        }),
        _ => None,
    };
    Ok(event)
}

fn normalize_codex_failure(
    method: &str,
    params: &Value,
    thread_id: String,
) -> Result<Option<ProviderEvent>, ProviderError> {
    let event = match method {
        "error" => Some(ProviderEvent::Error {
            provider: ProviderKind::Codex,
            session_id: (!thread_id.is_empty()).then_some(thread_id),
            code: params
                .pointer("/error/codexErrorInfo")
                .map_or_else(|| "codex-error".to_owned(), Value::to_string),
            message: required_string(params, "/error/message", method)?,
            retryable: params
                .get("willRetry")
                .and_then(Value::as_bool)
                .unwrap_or(false),
            raw: Some(codex_raw(params)),
        }),
        "process/exited" => Some(ProviderEvent::ProcessExited {
            provider: ProviderKind::Codex,
            session_id: (!thread_id.is_empty()).then_some(thread_id),
            code: params
                .get("exitCode")
                .and_then(Value::as_i64)
                .and_then(|code| i32::try_from(code).ok()),
        }),
        _ => None,
    };
    Ok(event)
}

fn codex_raw(params: &Value) -> Value {
    redact_json(params.clone())
}

fn normalize_codex_item(
    method: &str,
    params: &Value,
    thread_id: String,
    turn_id: Option<String>,
    raw: Option<Value>,
) -> Result<Option<ProviderEvent>, ProviderError> {
    let item_type = required_string(params, "/item/type", method)?;
    if matches!(
        item_type.as_str(),
        "userMessage" | "agentMessage" | "reasoning" | "plan"
    ) {
        return Ok(None);
    }
    let status = string_at(params, "/item/status").unwrap_or_else(|| {
        if method == "item/started" {
            "started".to_owned()
        } else {
            "completed".to_owned()
        }
    });
    Ok(Some(ProviderEvent::ToolUpdate {
        provider: ProviderKind::Codex,
        session_id: required_nonempty(thread_id, "item update threadId")?,
        turn_id,
        tool_name: Some(item_type),
        status,
        raw,
    }))
}

impl CodexSession {
    /// Answers Codex server-to-client requests. Approval requests are governed by the observer
    /// (the Lazoo control plane); without an observer they fail closed with `decline`.
    async fn handle_server_request(
        &self,
        message: &Value,
        observer: Option<&dyn TurnEventObserver>,
        accepted_turn_id: Option<&str>,
    ) -> Result<Option<ProviderEvent>, ProviderError> {
        let (Some(id), Some(method)) = (
            message.get("id").cloned(),
            message.get("method").and_then(Value::as_str),
        ) else {
            return Ok(None);
        };
        if !method.contains("requestApproval")
            && method != "item/tool/requestUserInput"
            && method != "item/tool/call"
        {
            return Ok(None);
        }
        let params = message.get("params").cloned().unwrap_or_else(|| json!({}));
        let thread_id = string_at(&params, "/threadId").unwrap_or_default();
        if method == "item/tool/call" {
            return self
                .handle_tool_call(id, &params, observer, accepted_turn_id)
                .await
                .map(Some);
        }
        let options =
            params
                .get("options")
                .and_then(Value::as_array)
                .map_or_else(Vec::new, |options| {
                    options
                        .iter()
                        .filter_map(|option| {
                            option
                                .get("optionId")
                                .and_then(Value::as_str)
                                .map(str::to_owned)
                        })
                        .collect()
                });
        let mut raw = redact_json(params.clone());
        match method {
            "item/commandExecution/requestApproval" | "item/fileChange/requestApproval" => {
                let active_turn_id = accepted_turn_id.map(str::to_owned).or_else(|| {
                    self.active_turn
                        .read()
                        .ok()
                        .and_then(|active| active.clone())
                });
                let request = normalize_codex_permission_request(
                    method,
                    &response_id(message).unwrap_or_else(|| id.to_string()),
                    &params,
                    active_turn_id,
                );
                let decision = match observer {
                    Some(observer) => observer.decide_permission(&request).await,
                    None => Ok(PermissionDecision::Decline),
                };
                let wire = match decision {
                    Ok(PermissionDecision::Approve) => "accept",
                    Ok(PermissionDecision::Decline) | Err(_) => "decline",
                };
                // Codex is always answered before a decision failure is surfaced, so the native
                // turn never hangs on an unanswered request.
                self.control
                    .respond(id.clone(), Ok(json!({"decision": wire})))
                    .await?;
                if let Value::Object(object) = &mut raw {
                    object.insert("lazooDecision".to_owned(), json!(wire));
                    object.insert("lazooKind".to_owned(), json!(request.kind));
                    object.insert("lazooAction".to_owned(), json!(request.action));
                }
                decision?;
            }
            _ => {
                self.control
                    .respond(
                        id.clone(),
                        Err((-32_001, "Lazoo approval flow is not attached")),
                    )
                    .await?;
            }
        }
        Ok(Some(ProviderEvent::PermissionRequested {
            provider: ProviderKind::Codex,
            session_id: thread_id,
            request_id: Some(id.to_string()),
            options,
            raw: Some(raw),
        }))
    }
}

impl CodexSession {
    /// Answers one `item/tool/call` for a Lazoo-declared dynamic tool. The control plane decides;
    /// Codex is always answered (fail closed) before an observer failure is surfaced, and the
    /// bounded call plus outcome is surfaced as a tool event for the turn ledger.
    async fn handle_tool_call(
        &self,
        id: Value,
        params: &Value,
        observer: Option<&dyn TurnEventObserver>,
        accepted_turn_id: Option<&str>,
    ) -> Result<ProviderEvent, ProviderError> {
        let active_turn_id = accepted_turn_id.map(str::to_owned).or_else(|| {
            self.active_turn
                .read()
                .ok()
                .and_then(|active| active.clone())
        });
        let call = normalize_codex_tool_call(params, active_turn_id);
        let oversized = serde_json::to_vec(&call.arguments)
            .is_ok_and(|bytes| bytes.len() > MAX_TOOL_ARGUMENT_BYTES);
        let outcome = if oversized {
            Ok(ToolResult::failure(
                "LZ-TOOL-ARGUMENTS-TOO-LARGE",
                "tool arguments exceed the 64 KiB limit",
            ))
        } else {
            match observer {
                Some(observer) => observer.call_tool(&call).await,
                None => Ok(ToolResult::failure(
                    "LZ-TOOL-UNATTACHED",
                    "Lazoo collaboration tools are not attached to this turn",
                )),
            }
        };
        let (success, mut text) = match &outcome {
            Ok(result) => (
                result.success,
                serde_json::to_string(&result.content).unwrap_or_default(),
            ),
            Err(error) => (
                false,
                json!({"error": {"code": "LZ-TOOL-OBSERVER", "message": error.to_string()}})
                    .to_string(),
            ),
        };
        truncate_utf8(&mut text, MAX_TOOL_RESULT_BYTES);
        self.control
            .respond(
                id,
                Ok(json!({
                    "contentItems": [{"type": "inputText", "text": text}],
                    "success": success
                })),
            )
            .await?;
        let mut raw = redact_json(json!({
            "callId": call.call_id,
            "tool": call.name,
            "arguments": call.arguments,
            "success": success,
            "result": text,
        }));
        if let Value::Object(object) = &mut raw
            && oversized
        {
            object.insert("arguments".to_owned(), json!({"truncated": true}));
        }
        let event = ProviderEvent::ToolUpdate {
            provider: ProviderKind::Codex,
            session_id: call.session_id,
            turn_id: call.turn_id,
            tool_name: Some(format!("lazoo:{}", call.name)),
            status: if success { "completed" } else { "failed" }.to_owned(),
            raw: Some(raw),
        };
        outcome?;
        Ok(event)
    }
}

/// Normalizes a Codex `item/tool/call` server request into the provider-neutral tool call.
#[must_use]
pub fn normalize_codex_tool_call(params: &Value, active_turn_id: Option<String>) -> ToolCall {
    ToolCall {
        provider: ProviderKind::Codex,
        session_id: string_at(params, "/threadId").unwrap_or_default(),
        turn_id: string_at(params, "/turnId").or(active_turn_id),
        call_id: string_at(params, "/callId").unwrap_or_default(),
        name: string_at(params, "/tool").unwrap_or_default(),
        arguments: params.get("arguments").cloned().unwrap_or(Value::Null),
    }
}

/// Normalizes a Codex `requestApproval` server request into the provider-neutral shape.
/// Only bounded, redacted evidence is retained; the raw approval payload never reaches the ledger.
#[must_use]
pub fn normalize_codex_permission_request(
    method: &str,
    request_id: &str,
    params: &Value,
    active_turn_id: Option<String>,
) -> PermissionRequest {
    let kind = if method == "item/fileChange/requestApproval" {
        PermissionKind::FileChange
    } else {
        PermissionKind::CommandExecution
    };
    let command = params
        .get("command")
        .and_then(|command| match command {
            Value::String(text) => Some(text.clone()),
            Value::Array(parts) => Some(
                parts
                    .iter()
                    .filter_map(Value::as_str)
                    .collect::<Vec<_>>()
                    .join(" "),
            ),
            _ => None,
        })
        .or_else(|| string_at(params, "/item/command"))
        .map(|mut command| {
            truncate_utf8(&mut command, MAX_PERMISSION_COMMAND_BYTES);
            command
        });
    let grant_root = string_at(params, "/grantRoot");
    let scope_expansion = grant_root.is_some()
        || params
            .get("additionalPermissions")
            .is_some_and(|value| !value.is_null())
        || params
            .get("networkApprovalContext")
            .is_some_and(|value| !value.is_null());
    let mut action = match kind {
        PermissionKind::CommandExecution => command
            .clone()
            .filter(|command| !command.trim().is_empty())
            .unwrap_or_else(|| "run a command".to_owned()),
        PermissionKind::FileChange => grant_root.as_ref().map_or_else(
            || "apply file changes in the workspace".to_owned(),
            |root| format!("apply file changes under {root}"),
        ),
    };
    truncate_utf8(&mut action, MAX_PERMISSION_ACTION_BYTES);
    let mut detail = Map::new();
    for key in [
        "threadId",
        "turnId",
        "itemId",
        "approvalId",
        "reason",
        "cwd",
        "grantRoot",
        "additionalPermissions",
        "networkApprovalContext",
    ] {
        if let Some(value) = params.get(key).filter(|value| !value.is_null()) {
            detail.insert(key.to_owned(), value.clone());
        }
    }
    if let Some(command) = command {
        detail.insert("command".to_owned(), Value::String(command));
    }
    PermissionRequest {
        provider: ProviderKind::Codex,
        session_id: string_at(params, "/threadId").unwrap_or_default(),
        turn_id: string_at(params, "/turnId").or(active_turn_id),
        request_id: request_id.to_owned(),
        kind,
        action,
        scope_expansion,
        detail: redact_json(Value::Object(detail)),
    }
}

fn truncate_utf8(value: &mut String, maximum: usize) {
    if value.len() <= maximum {
        return;
    }
    let mut end = maximum;
    while !value.is_char_boundary(end) {
        end -= 1;
    }
    value.truncate(end);
}

async fn initialize(
    process: &mut JsonRpcProcess,
    wait: Duration,
    experimental_api: bool,
) -> Result<Value, ProviderError> {
    let result = process
        .request(
            "initialize",
            json!({
                "clientInfo": {
                    "name": "lazoo",
                    "title": "Lazoo",
                    "version": env!("CARGO_PKG_VERSION")
                },
                "capabilities": {"experimentalApi": experimental_api}
            }),
            wait,
            "Codex initialization",
        )
        .await?;
    process.control().notify("initialized", None).await?;
    Ok(result)
}

fn thread_start_params(request: &SessionRequest) -> Result<Value, ProviderError> {
    let mut params = Map::new();
    params.insert(
        "cwd".to_owned(),
        Value::String(request.cwd.to_string_lossy().into_owned()),
    );
    insert_optional(&mut params, "model", request.model.as_deref());
    insert_approval_policy(&mut params, request.permission_mode.as_deref())?;
    if !request.tools.is_empty() {
        // Codex forwards dynamic tool names to the Responses API, which rejects dots; failing
        // here names the offending tool instead of a generic session-start failure.
        if let Some(tool) = request.tools.iter().find(|tool| {
            tool.name.is_empty()
                || !tool
                    .name
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_' || byte == b'-')
        }) {
            return Err(ProviderError::Protocol(format!(
                "Codex dynamic tool name must match ^[a-zA-Z0-9_-]+$: {}",
                tool.name
            )));
        }
        params.insert(
            "dynamicTools".to_owned(),
            Value::Array(request.tools.iter().map(dynamic_tool_spec).collect()),
        );
    }
    Ok(Value::Object(params))
}

fn dynamic_tool_spec(tool: &ToolSpec) -> Value {
    json!({
        "type": "function",
        "name": tool.name,
        "description": tool.description,
        "inputSchema": tool.input_schema,
    })
}

fn thread_resume_params(
    session_id: &str,
    request: &SessionRequest,
) -> Result<Value, ProviderError> {
    if !request.tools.is_empty() {
        // Codex 0.153 accepts dynamicTools only on thread/start; a resumed thread would silently
        // lose the Lazoo tools, so declaring them on resume fails closed instead.
        return Err(ProviderError::Unsupported {
            provider: ProviderKind::Codex,
            capability: "dynamic tools on thread/resume",
        });
    }
    let mut params = Map::new();
    params.insert("threadId".to_owned(), Value::String(session_id.to_owned()));
    params.insert(
        "cwd".to_owned(),
        Value::String(request.cwd.to_string_lossy().into_owned()),
    );
    insert_optional(&mut params, "model", request.model.as_deref());
    insert_approval_policy(&mut params, request.permission_mode.as_deref())?;
    Ok(Value::Object(params))
}

fn turn_start_params(
    session_id: &str,
    session: &SessionRequest,
    request: &TurnRequest,
) -> Result<Value, ProviderError> {
    let mut params = Map::new();
    params.insert("threadId".to_owned(), Value::String(session_id.to_owned()));
    params.insert(
        "clientUserMessageId".to_owned(),
        Value::String(request.correlation_id.to_string()),
    );
    params.insert(
        "input".to_owned(),
        json!([{"type": "text", "text": request.prompt}]),
    );
    insert_optional(&mut params, "model", session.model.as_deref());
    insert_optional(&mut params, "effort", session.effort.as_deref());
    insert_approval_policy(
        &mut params,
        request
            .permission_mode
            .as_deref()
            .or(session.permission_mode.as_deref()),
    )?;
    Ok(Value::Object(params))
}

fn insert_optional(params: &mut Map<String, Value>, key: &str, value: Option<&str>) {
    if let Some(value) = value {
        params.insert(key.to_owned(), Value::String(value.to_owned()));
    }
}

fn insert_approval_policy(
    params: &mut Map<String, Value>,
    mode: Option<&str>,
) -> Result<(), ProviderError> {
    let Some(mode) = mode else {
        return Ok(());
    };
    let policy = match mode {
        "ask" | "default" | "on-request" | "accept-edits" | "acceptEdits" => "on-request",
        "auto" | "never" => "never",
        "untrusted" => "untrusted",
        _ => {
            return Err(ProviderError::Protocol(format!(
                "unsupported Codex permission mode: {mode}"
            )));
        }
    };
    params.insert(
        "approvalPolicy".to_owned(),
        Value::String(policy.to_owned()),
    );
    Ok(())
}

async fn read_version(path: &Path) -> Result<String, ProviderError> {
    let output = timeout(
        VERSION_TIMEOUT,
        Command::new(path).arg("--version").output(),
    )
    .await
    .map_err(|_| ProviderError::Timeout {
        phase: "Codex version probe",
    })??;
    if !output.status.success() {
        return Err(ProviderError::Protocol(format!(
            "Codex version command exited with code {:?}",
            output.status.code()
        )));
    }
    let bytes = if output.stdout.is_empty() {
        &output.stderr
    } else {
        &output.stdout
    };
    if bytes.len() > MAX_VERSION_BYTES {
        return Err(ProviderError::Protocol(
            "Codex version output exceeded the limit".to_owned(),
        ));
    }
    let version = String::from_utf8_lossy(bytes).trim().to_owned();
    if version.is_empty() {
        Err(ProviderError::Protocol(
            "Codex version command returned empty output".to_owned(),
        ))
    } else {
        Ok(version)
    }
}

fn configured_probe(
    candidate: ExecutableCandidate,
    version: String,
    initialized: &Value,
    account: &Value,
) -> ProviderProbe {
    let requires_openai_auth = account
        .get("requiresOpenaiAuth")
        .and_then(Value::as_bool)
        .unwrap_or(true);
    let has_account = account.get("account").is_some_and(|value| !value.is_null());
    let authentication = if has_account {
        AuthenticationEvidence {
            native: AuthenticationState::Ready,
            external: AuthenticationState::Unknown,
            detail: Some("Codex App Server reported a configured account".to_owned()),
        }
    } else if requires_openai_auth {
        AuthenticationEvidence {
            native: AuthenticationState::NeedsLogin,
            external: AuthenticationState::Unknown,
            detail: Some("Codex App Server requires OpenAI authentication".to_owned()),
        }
    } else {
        AuthenticationEvidence {
            native: AuthenticationState::Unknown,
            external: AuthenticationState::Ready,
            detail: Some("Codex App Server does not require OpenAI authentication".to_owned()),
        }
    };
    let verification = if has_account || !requires_openai_auth {
        ProviderVerification::Configured
    } else {
        ProviderVerification::Unverified
    };
    let mut evidence = vec![
        "version command completed".to_owned(),
        "App Server initialize/initialized handshake completed".to_owned(),
        "account/read completed without exposing account data".to_owned(),
    ];
    if let Some(user_agent) = initialized.get("userAgent").and_then(Value::as_str) {
        evidence.push(format!("server user agent: {user_agent}"));
    }
    ProviderProbe {
        contract_version: ADAPTER_CONTRACT_V1,
        provider: ProviderKind::Codex,
        verification,
        executable: Some(candidate.path),
        discovery_source: Some(candidate.source),
        version: Some(version),
        protocol: Some(CODEX_PROTOCOL.to_owned()),
        capabilities: CodexAdapter::capabilities(),
        authentication,
        degradation_reason: None,
        evidence,
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
        provider: ProviderKind::Codex,
        verification: ProviderVerification::Degraded,
        executable: Some(candidate.path),
        discovery_source: Some(candidate.source),
        version,
        protocol: Some(CODEX_PROTOCOL.to_owned()),
        capabilities: ProviderCapabilities::default(),
        authentication: AuthenticationEvidence::unknown(),
        degradation_reason: Some(reason),
        evidence: Vec::new(),
        checked_at: chrono::Utc::now().to_rfc3339(),
    }
}

fn string_at(value: &Value, pointer: &str) -> Option<String> {
    value
        .pointer(pointer)
        .and_then(Value::as_str)
        .map(str::to_owned)
}

fn required_string(value: &Value, pointer: &str, method: &str) -> Result<String, ProviderError> {
    string_at(value, pointer)
        .ok_or_else(|| ProviderError::InvalidEvent(format!("Codex {method} omitted {pointer}")))
}

fn required_nonempty(value: String, field: &str) -> Result<String, ProviderError> {
    if value.is_empty() {
        Err(ProviderError::InvalidEvent(format!(
            "Codex event omitted {field}"
        )))
    } else {
        Ok(value)
    }
}
