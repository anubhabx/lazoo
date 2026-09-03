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
    DiscoveryEnvironment, ExecutableCandidate, ProviderAdapter, ProviderCapabilities,
    ProviderError, ProviderEvent, ProviderKind, ProviderProbe, ProviderSession,
    ProviderVerification, SessionRequest, TurnEventObserver, TurnRequest, json_rpc::JsonRpcControl,
    json_rpc::JsonRpcProcess, redact_json,
};

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
        let handshake = initialize(&mut process, Duration::from_secs(15)).await;
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
        initialize(&mut process, request.startup_timeout()).await?;

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
        let control = process.control();
        Ok(CodexSession {
            process: Mutex::new(process),
            control,
            session_id,
            native_host_pid,
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
        let started = Instant::now();
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
            if let Some(permission) = handle_server_request(&self.control, &message).await? {
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

async fn handle_server_request(
    control: &JsonRpcControl,
    message: &Value,
) -> Result<Option<ProviderEvent>, ProviderError> {
    let (Some(id), Some(method)) = (
        message.get("id").cloned(),
        message.get("method").and_then(Value::as_str),
    ) else {
        return Ok(None);
    };
    if !method.contains("requestApproval") && method != "item/tool/requestUserInput" {
        return Ok(None);
    }
    let params = message.get("params").cloned().unwrap_or_else(|| json!({}));
    let thread_id = string_at(&params, "/threadId").unwrap_or_default();
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
    let response = match method {
        "item/commandExecution/requestApproval" | "item/fileChange/requestApproval" => {
            Ok(json!({"decision": "decline"}))
        }
        _ => Err((-32_001, "Lazoo approval flow is not attached")),
    };
    control.respond(id.clone(), response).await?;
    Ok(Some(ProviderEvent::PermissionRequested {
        provider: ProviderKind::Codex,
        session_id: thread_id,
        request_id: Some(id.to_string()),
        options,
        raw: Some(redact_json(params)),
    }))
}

async fn initialize(process: &mut JsonRpcProcess, wait: Duration) -> Result<Value, ProviderError> {
    let result = process
        .request(
            "initialize",
            json!({
                "clientInfo": {
                    "name": "lazoo",
                    "title": "Lazoo",
                    "version": env!("CARGO_PKG_VERSION")
                },
                "capabilities": {"experimentalApi": false}
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
    Ok(Value::Object(params))
}

fn thread_resume_params(
    session_id: &str,
    request: &SessionRequest,
) -> Result<Value, ProviderError> {
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
    insert_approval_policy(&mut params, session.permission_mode.as_deref())?;
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
        "ask" | "default" | "on-request" => "on-request",
        "untrusted" => "untrusted",
        "never" => "never",
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
