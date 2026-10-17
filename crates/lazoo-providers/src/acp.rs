use std::{
    cmp,
    collections::HashSet,
    sync::atomic::{AtomicBool, Ordering},
    time::{Duration, Instant},
};

use agent_client_protocol::schema::{
    ProtocolVersion,
    v1::{
        CancelNotification, CloseSessionRequest, ContentBlock, Implementation, InitializeRequest,
        InitializeResponse, LoadSessionRequest, NewSessionRequest, NewSessionResponse,
        PromptRequest, RequestPermissionOutcome, RequestPermissionResponse,
        SelectedPermissionOutcome, TextContent,
    },
};
use serde::Serialize;
use serde_json::{Value, json};
use tokio::{
    sync::{Mutex, Notify},
    time,
};

use crate::{
    AdapterFuture, CommandSpec, PermissionDecision, PermissionKind, PermissionRequest,
    ProviderError, ProviderEvent, ProviderKind, ProviderSession, SessionRequest, TurnEventObserver,
    TurnRequest,
    json_rpc::{JsonRpcControl, JsonRpcProcess, response_id},
    redact_json,
};

const STOP_TIMEOUT: Duration = Duration::from_secs(2);
const MAX_PERMISSION_ACTION_BYTES: usize = 1024;
const MAX_PERMISSION_DETAIL_BYTES: usize = 4096;

#[derive(Clone, Copy, Debug)]
pub struct ProgressDeadline {
    timeout: Duration,
    last_progress: Option<Instant>,
}

impl ProgressDeadline {
    #[must_use]
    pub const fn new(timeout: Duration) -> Self {
        Self {
            timeout,
            last_progress: None,
        }
    }

    pub fn accepted_at(&mut self, now: Instant) {
        self.last_progress = Some(now);
    }

    pub fn progress_at(&mut self, now: Instant) {
        if self.last_progress.is_some() {
            self.last_progress = Some(now);
        }
    }

    pub fn check_at(&self, now: Instant) -> Result<(), ProviderError> {
        if self
            .last_progress
            .is_some_and(|last_progress| now.duration_since(last_progress) >= self.timeout)
        {
            return Err(ProviderError::Stalled {
                millis: duration_millis(self.timeout),
            });
        }
        Ok(())
    }

    fn remaining_at(&self, now: Instant) -> Duration {
        self.last_progress.map_or(self.timeout, |last_progress| {
            self.timeout
                .saturating_sub(now.duration_since(last_progress))
        })
    }
}

fn duration_millis(duration: Duration) -> u64 {
    u64::try_from(duration.as_millis()).unwrap_or(u64::MAX)
}

#[derive(Debug)]
pub(crate) struct AcpHandshake {
    pub response: InitializeResponse,
    pub raw: Value,
}

pub(crate) async fn initialize_acp(
    process: &mut JsonRpcProcess,
    wait: Duration,
    provider_name: &'static str,
) -> Result<AcpHandshake, ProviderError> {
    let request = InitializeRequest::new(ProtocolVersion::V1).client_info(
        Implementation::new("lazoo", env!("CARGO_PKG_VERSION")).title("Lazoo".to_owned()),
    );
    let raw = process
        .request(
            "initialize",
            serialize_params(&request)?,
            wait,
            provider_name,
        )
        .await?;
    let response: InitializeResponse = serde_json::from_value(raw.clone())?;
    if response.protocol_version != ProtocolVersion::V1 {
        return Err(ProviderError::Protocol(format!(
            "ACP negotiated unsupported protocol version {}",
            response.protocol_version.as_u16()
        )));
    }
    Ok(AcpHandshake { response, raw })
}

pub(crate) async fn start_acp_session(
    provider: ProviderKind,
    command: CommandSpec,
    request: &SessionRequest,
) -> Result<(AcpSession, AcpHandshake), ProviderError> {
    validate_workspace(request)?;
    let mut process = JsonRpcProcess::spawn_standard(&command)?;
    let handshake = initialize_acp(
        &mut process,
        request.startup_timeout(),
        "ACP initialization",
    )
    .await?;
    let resumed = request.session_id.is_some();
    let session_id = if let Some(session_id) = &request.session_id {
        if !handshake.response.agent_capabilities.load_session {
            let _ = process.stop(STOP_TIMEOUT).await;
            return Err(ProviderError::Unsupported {
                provider,
                capability: "session resume",
            });
        }
        let load = LoadSessionRequest::new(session_id.clone(), request.cwd.clone())
            .additional_directories(request.workspace_roots.clone());
        process
            .request(
                "session/load",
                serialize_params(&load)?,
                request.startup_timeout(),
                "ACP session load",
            )
            .await?;
        // ACP session/load replays historical session/update notifications before
        // returning. They describe prior turns and must not enter the next turn's
        // output accumulator.
        process.discard_queued();
        session_id.clone()
    } else {
        let create = NewSessionRequest::new(request.cwd.clone())
            .additional_directories(request.workspace_roots.clone());
        let response = process
            .request(
                "session/new",
                serialize_params(&create)?,
                request.startup_timeout(),
                "ACP session creation",
            )
            .await?;
        let response: NewSessionResponse = serde_json::from_value(response)?;
        // ACP agents may emit session/update notifications while answering session/new. They
        // precede any Lazoo turn and must not be attributed to the first prompt.
        process.discard_queued();
        response.session_id.to_string()
    };
    let close_supported = handshake
        .response
        .agent_capabilities
        .session_capabilities
        .close
        .is_some();
    let native_host_pid = process.native_host_pid();
    let job = Some(process.job());
    let control = process.control();
    Ok((
        AcpSession {
            process: Mutex::new(process),
            control,
            provider,
            session_id,
            native_host_pid,
            job,
            resumed,
            request: request.clone(),
            close_supported,
            session_event_pending: AtomicBool::new(true),
            active: AtomicBool::new(false),
            cancelled: AtomicBool::new(false),
            cancellation: Notify::new(),
            stopped: AtomicBool::new(false),
        },
        handshake,
    ))
}

fn validate_workspace(request: &SessionRequest) -> Result<(), ProviderError> {
    if !request.cwd.is_absolute() {
        return Err(ProviderError::Protocol(
            "ACP workspace path must be absolute".to_owned(),
        ));
    }
    if request
        .workspace_roots
        .iter()
        .any(|root| !root.is_absolute())
    {
        return Err(ProviderError::Protocol(
            "ACP additional workspace roots must be absolute".to_owned(),
        ));
    }
    Ok(())
}

#[derive(Debug)]
pub(crate) struct AcpSession {
    process: Mutex<JsonRpcProcess>,
    control: JsonRpcControl,
    provider: ProviderKind,
    session_id: String,
    native_host_pid: Option<u32>,
    job: Option<std::sync::Arc<lazoo_runtime::JobObject>>,
    resumed: bool,
    request: SessionRequest,
    close_supported: bool,
    session_event_pending: AtomicBool,
    active: AtomicBool,
    cancelled: AtomicBool,
    cancellation: Notify,
    stopped: AtomicBool,
}

impl AcpSession {
    async fn run_turn(&self, request: &TurnRequest) -> Result<Vec<ProviderEvent>, ProviderError> {
        if request.read_only {
            return Err(ProviderError::Unsupported {
                provider: self.provider,
                capability: "read-only turns",
            });
        }
        if self.stopped.load(Ordering::Acquire) {
            return Err(ProviderError::Protocol(
                "ACP session has already stopped".to_owned(),
            ));
        }
        let mut process = self.process.lock().await;
        // Clear cancellation while this session is known idle, before making the turn visible to
        // cancel(). A cancellation that observes active=true can therefore never be cleared.
        self.cancelled.store(false, Ordering::Release);
        if self.active.swap(true, Ordering::AcqRel) {
            return Err(ProviderError::Protocol(
                "ACP session already has an active turn".to_owned(),
            ));
        }
        let outcome = self.collect_turn(&mut process, request, None).await;
        self.active.store(false, Ordering::Release);
        outcome
    }

    async fn run_turn_observed(
        &self,
        request: &TurnRequest,
        observer: &dyn TurnEventObserver,
    ) -> Result<Vec<ProviderEvent>, ProviderError> {
        if request.read_only {
            return Err(ProviderError::Unsupported {
                provider: self.provider,
                capability: "read-only turns",
            });
        }
        if self.stopped.load(Ordering::Acquire) {
            return Err(ProviderError::Protocol(
                "ACP session has already stopped".to_owned(),
            ));
        }
        let mut process = self.process.lock().await;
        self.cancelled.store(false, Ordering::Release);
        if self.active.swap(true, Ordering::AcqRel) {
            return Err(ProviderError::Protocol(
                "ACP session already has an active turn".to_owned(),
            ));
        }
        let outcome = self
            .collect_turn(&mut process, request, Some(observer))
            .await;
        self.active.store(false, Ordering::Release);
        outcome
    }

    #[allow(clippy::too_many_lines)]
    async fn collect_turn(
        &self,
        process: &mut JsonRpcProcess,
        request: &TurnRequest,
        observer: Option<&dyn TurnEventObserver>,
    ) -> Result<Vec<ProviderEvent>, ProviderError> {
        let prompt = PromptRequest::new(
            self.session_id.clone(),
            vec![ContentBlock::Text(TextContent::new(request.prompt.clone()))],
        );
        let prompt_id = self
            .control
            .send_request("session/prompt", serialize_params(&prompt)?)
            .await?;
        let started = Instant::now();
        let mut progress = ProgressDeadline::new(self.request.progress_timeout());
        progress.accepted_at(started);
        let mut output = String::new();
        let mut events = Vec::new();
        if self.session_event_pending.swap(false, Ordering::AcqRel) {
            events.push(ProviderEvent::SessionStarted {
                provider: self.provider,
                session_id: self.session_id.clone(),
                resumed: self.resumed,
                raw: None,
            });
        }
        events.push(ProviderEvent::TurnStarted {
            provider: self.provider,
            session_id: self.session_id.clone(),
            turn_id: Some(request.correlation_id.to_string()),
            raw: None,
        });
        if let Some(observer) = observer {
            for event in &events {
                observer.observe(event)?;
            }
        }
        let mut permission_ids = HashSet::new();
        let mut authorization_keys = HashSet::new();

        loop {
            let now = Instant::now();
            progress.check_at(now)?;
            let total_remaining = self
                .request
                .turn_timeout()
                .checked_sub(started.elapsed())
                .ok_or(ProviderError::Timeout {
                    phase: "ACP turn completion",
                })?;
            let progress_remaining = progress.remaining_at(now);
            let wait = cmp::min(total_remaining, progress_remaining);
            let message = match process.next_message(wait, "ACP turn progress").await {
                Err(ProviderError::Timeout { .. }) if progress_remaining <= total_remaining => {
                    return Err(ProviderError::Stalled {
                        millis: self.request.progress_timeout_millis,
                    });
                }
                Err(ProviderError::Timeout { .. }) => {
                    return Err(ProviderError::Timeout {
                        phase: "ACP turn completion",
                    });
                }
                result => result?,
            };

            if response_id(&message).as_deref() == Some(prompt_id.as_str()) {
                let result = response_result(&message, "session/prompt")?;
                let status = result
                    .get("stopReason")
                    .map_or_else(|| "completed".to_owned(), stop_reason_string);
                if status == "cancelled" {
                    events.push(ProviderEvent::Cancelled {
                        provider: self.provider,
                        session_id: self.session_id.clone(),
                        turn_id: Some(request.correlation_id.to_string()),
                        raw: Some(redact_json(result)),
                    });
                } else {
                    events.push(ProviderEvent::TurnCompleted {
                        provider: self.provider,
                        session_id: self.session_id.clone(),
                        turn_id: Some(request.correlation_id.to_string()),
                        status,
                        output: (!output.is_empty()).then_some(output),
                        raw: Some(redact_json(result)),
                    });
                }
                if let Some(observer) = observer {
                    observer.observe(events.last().expect("terminal event was appended"))?;
                }
                return Ok(events);
            }

            if message.get("method").and_then(Value::as_str) == Some("session/request_permission") {
                if let Some(permission) = self
                    .handle_permission_request(
                        &message,
                        observer,
                        request,
                        started,
                        &mut permission_ids,
                        &mut authorization_keys,
                    )
                    .await?
                {
                    progress.progress_at(Instant::now());
                    if let Some(observer) = observer {
                        observer.observe(&permission)?;
                    }
                    events.push(permission);
                }
                continue;
            }
            let Some(event) = normalize_acp_message(self.provider, &message)? else {
                continue;
            };
            if event_session_id(&event) != Some(self.session_id.as_str()) {
                return Err(ProviderError::InvalidEvent(
                    "ACP event targeted a different session".to_owned(),
                ));
            }
            if let ProviderEvent::TextDelta { text, .. } = &event {
                output.push_str(text);
            }
            progress.progress_at(Instant::now());
            if let Some(observer) = observer {
                observer.observe(&event)?;
            }
            events.push(event);
        }
    }

    async fn handle_permission_request(
        &self,
        message: &Value,
        observer: Option<&dyn TurnEventObserver>,
        turn: &TurnRequest,
        started: Instant,
        permission_ids: &mut HashSet<String>,
        authorization_keys: &mut HashSet<(String, String, String)>,
    ) -> Result<Option<ProviderEvent>, ProviderError> {
        if message.get("method").and_then(Value::as_str) != Some("session/request_permission") {
            return Ok(None);
        }
        let Some(id) = message.get("id").cloned() else {
            return Ok(None);
        };
        let request_id = permission_request_id(&id);
        let permission = request_id
            .as_ref()
            .and_then(|(key, request_id)| {
                (!permission_ids.contains(key)).then(|| {
                    normalize_acp_permission_request(
                        self.provider,
                        request_id,
                        message,
                        &self.session_id,
                        turn,
                    )
                })
            })
            .transpose()
            .ok()
            .flatten();
        let authorization_key = permission.as_ref().and_then(permission_authorization_key);
        let permission = authorization_key
            .as_ref()
            .filter(|key| !authorization_keys.contains(*key))
            .and(permission);
        if let (Some((request_key, _)), Some(authorization_key), Some(_)) =
            (&request_id, &authorization_key, &permission)
        {
            permission_ids.insert(request_key.clone());
            authorization_keys.insert(authorization_key.clone());
        }
        let decision = match (observer, permission.as_ref()) {
            (Some(observer), Some(permission)) if !self.cancelled.load(Ordering::Acquire) => {
                let remaining = self.request.turn_timeout().checked_sub(started.elapsed());
                match remaining {
                    Some(remaining) => tokio::select! {
                        result = observer.decide_permission(permission) => result.unwrap_or(PermissionDecision::Decline),
                        () = time::sleep(remaining) => PermissionDecision::Decline,
                        () = self.wait_for_cancellation() => PermissionDecision::Decline,
                    },
                    None => PermissionDecision::Decline,
                }
            }
            _ => PermissionDecision::Decline,
        };
        let valid_permission = permission.is_some();
        let decision = if self.cancelled.load(Ordering::Acquire) {
            PermissionDecision::Decline
        } else {
            decision
        };
        let response = match decision {
            PermissionDecision::Approve => permission
                .as_ref()
                .and_then(|_| {
                    allow_once_option(message).map(|option_id| {
                        RequestPermissionResponse::new(RequestPermissionOutcome::Selected(
                            SelectedPermissionOutcome::new(option_id),
                        ))
                    })
                })
                .unwrap_or_else(|| {
                    RequestPermissionResponse::new(RequestPermissionOutcome::Cancelled)
                }),
            PermissionDecision::Decline => {
                RequestPermissionResponse::new(RequestPermissionOutcome::Cancelled)
            }
        };
        self.control
            .respond(id, Ok(serialize_params(&response)?))
            .await?;
        // A malformed or duplicate permission request is answered above but is not admitted to
        // the ledger, and therefore cannot be mistaken for an approval request by the observer.
        if valid_permission {
            normalize_acp_message(self.provider, message)
        } else {
            Ok(None)
        }
    }

    async fn wait_for_cancellation(&self) {
        loop {
            let notified = self.cancellation.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();
            if self.cancelled.load(Ordering::Acquire) {
                return;
            }
            notified.await;
            if self.cancelled.load(Ordering::Acquire) {
                return;
            }
        }
    }
}

fn event_session_id(event: &ProviderEvent) -> Option<&str> {
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

fn permission_request_id(id: &Value) -> Option<(String, String)> {
    match id {
        Value::String(value) if !value.is_empty() => Some((format!("s:{value}"), value.clone())),
        Value::Number(value) => Some((format!("n:{value}"), value.to_string())),
        _ => None,
    }
}

impl ProviderSession for AcpSession {
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
            if self.active.load(Ordering::Acquire) {
                self.cancelled.store(true, Ordering::Release);
                self.cancellation.notify_waiters();
                let cancel = CancelNotification::new(self.session_id.clone());
                self.control
                    .notify("session/cancel", Some(serialize_params(&cancel)?))
                    .await?;
            }
            Ok(())
        })
    }

    fn stop(&self) -> AdapterFuture<'_, ()> {
        Box::pin(async move {
            if self.stopped.swap(true, Ordering::AcqRel) {
                return Ok(());
            }
            let mut process = self.process.lock().await;
            if self.close_supported {
                let close = CloseSessionRequest::new(self.session_id.clone());
                let _ = process
                    .request(
                        "session/close",
                        serialize_params(&close)?,
                        STOP_TIMEOUT,
                        "ACP session close",
                    )
                    .await;
            }
            process.stop(STOP_TIMEOUT).await
        })
    }
}

pub fn normalize_acp_message(
    provider: ProviderKind,
    message: &Value,
) -> Result<Option<ProviderEvent>, ProviderError> {
    let Some(method) = message.get("method").and_then(Value::as_str) else {
        return Ok(None);
    };
    let params = message.get("params").cloned().unwrap_or_else(|| json!({}));
    let session_id = params
        .get("sessionId")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_owned();
    let raw = Some(redact_json(params.clone()));
    match method {
        "session/request_permission" => {
            let options =
                params
                    .get("options")
                    .and_then(Value::as_array)
                    .map_or_else(Vec::new, |options| {
                        options
                            .iter()
                            .filter_map(|option| option.get("optionId").and_then(Value::as_str))
                            .map(str::to_owned)
                            .collect()
                    });
            Ok(Some(ProviderEvent::PermissionRequested {
                provider,
                session_id: required_session(session_id, method)?,
                request_id: message.get("id").map(json_id_string),
                options,
                raw,
            }))
        }
        "session/update" => normalize_session_update(provider, &params, session_id, raw),
        _ => Ok(None),
    }
}

fn normalize_session_update(
    provider: ProviderKind,
    params: &Value,
    session_id: String,
    raw: Option<Value>,
) -> Result<Option<ProviderEvent>, ProviderError> {
    let update = params.get("update").ok_or_else(|| {
        ProviderError::InvalidEvent("ACP session/update omitted update".to_owned())
    })?;
    let update_type = update
        .get("sessionUpdate")
        .and_then(Value::as_str)
        .ok_or_else(|| {
            ProviderError::InvalidEvent(
                "ACP session/update omitted update.sessionUpdate".to_owned(),
            )
        })?;
    let session_id = required_session(session_id, "session/update")?;
    let event = match update_type {
        "agent_message_chunk" => ProviderEvent::TextDelta {
            provider,
            session_id,
            turn_id: None,
            text: required_text(update, update_type)?,
            raw,
        },
        "agent_thought_chunk" => ProviderEvent::ThoughtDelta {
            provider,
            session_id,
            turn_id: None,
            text: required_text(update, update_type)?,
            raw,
        },
        "tool_call_update" if provider == ProviderKind::GrokBuild && is_terminal_update(update) => {
            ProviderEvent::TerminalUpdate {
                provider,
                session_id,
                terminal_id: update
                    .get("toolCallId")
                    .and_then(Value::as_str)
                    .map(str::to_owned),
                output: terminal_output(update),
                raw,
            }
        }
        "tool_call" | "tool_call_update" => ProviderEvent::ToolUpdate {
            provider,
            session_id,
            turn_id: None,
            tool_name: update
                .get("title")
                .or_else(|| update.get("kind"))
                .and_then(Value::as_str)
                .map(str::to_owned),
            status: update
                .get("status")
                .and_then(Value::as_str)
                .unwrap_or("updated")
                .to_owned(),
            raw,
        },
        "usage_update" => ProviderEvent::Usage {
            provider,
            session_id,
            input_tokens: None,
            output_tokens: None,
            total_tokens: update.get("used").and_then(Value::as_u64),
            cost_usd: update.get("cost").and_then(Value::as_f64),
            raw,
        },
        "turn_completed" => {
            let usage = update.get("usage").unwrap_or(&Value::Null);
            ProviderEvent::Usage {
                provider,
                session_id,
                input_tokens: usage.get("inputTokens").and_then(Value::as_u64),
                output_tokens: usage.get("outputTokens").and_then(Value::as_u64),
                total_tokens: usage.get("totalTokens").and_then(Value::as_u64),
                cost_usd: usage
                    .get("costUsdTicks")
                    .and_then(Value::as_u64)
                    .and_then(usd_ticks),
                raw,
            }
        }
        _ => ProviderEvent::Progress {
            provider,
            session_id,
            message: Some(update_type.to_owned()),
            raw,
        },
    };
    Ok(Some(event))
}

fn usd_ticks(ticks: u64) -> Option<f64> {
    let whole = u32::try_from(ticks / 1_000_000_000).ok()?;
    let fractional = u32::try_from(ticks % 1_000_000_000).ok()?;
    Some(f64::from(whole) + f64::from(fractional) / 1_000_000_000.0)
}

fn is_terminal_update(update: &Value) -> bool {
    update.get("kind").and_then(Value::as_str) == Some("execute")
        || update.pointer("/rawOutput/variant").and_then(Value::as_str) == Some("Bash")
        || update
            .pointer("/_meta/x.ai~1tool/kind")
            .and_then(Value::as_str)
            == Some("execute")
}

fn terminal_output(update: &Value) -> Option<String> {
    update
        .pointer("/rawOutput/output")
        .and_then(Value::as_str)
        .or_else(|| {
            update
                .get("content")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .find_map(|content| content.pointer("/content/text").and_then(Value::as_str))
        })
        .map(str::to_owned)
}

fn required_text(update: &Value, update_type: &str) -> Result<String, ProviderError> {
    update
        .pointer("/content/text")
        .and_then(Value::as_str)
        .map(str::to_owned)
        .ok_or_else(|| {
            ProviderError::InvalidEvent(format!("ACP {update_type} update omitted content.text"))
        })
}

fn required_session(session_id: String, method: &str) -> Result<String, ProviderError> {
    if session_id.is_empty() {
        Err(ProviderError::InvalidEvent(format!(
            "ACP {method} omitted sessionId"
        )))
    } else {
        Ok(session_id)
    }
}

fn json_id_string(value: &Value) -> String {
    value
        .as_str()
        .map_or_else(|| value.to_string(), str::to_owned)
}

fn response_result(message: &Value, method: &str) -> Result<Value, ProviderError> {
    if let Some(result) = message.get("result") {
        return Ok(result.clone());
    }
    if let Some(error) = message.get("error") {
        let code = error
            .get("code")
            .map_or_else(|| "unknown".to_owned(), Value::to_string);
        return Err(ProviderError::Protocol(format!(
            "provider request {method} failed with code {code}"
        )));
    }
    Err(ProviderError::Protocol(format!(
        "provider response for {method} contained neither result nor error"
    )))
}

/// Maps an ACP v1 `stopReason` onto the contract's completion vocabulary. The daemon records a
/// turn as completed only for `completed` (and as cancelled or failed for those), so a raw `ACP`
/// `end_turn` left an `OpenCode` turn `uncertain` until this mapping existed (found by the live
/// `OpenCode` lifecycle on 2026-10-17). An unknown reason passes through unchanged, which the daemon
/// treats as no completion evidence; the raw stop reason stays in the event's `raw` payload.
fn stop_reason_string(value: &Value) -> String {
    match value.as_str() {
        Some("end_turn") => "completed".to_owned(),
        Some("cancelled") => "cancelled".to_owned(),
        Some("max_tokens" | "max_turn_requests" | "refusal") => "failed".to_owned(),
        Some(other) => other.to_owned(),
        None => value.to_string(),
    }
}

/// Converts the small, understood ACP v1 permission subset into the durable approval shape.
/// ACP tool-call arguments have no portable confinement proof, so every accepted request is
/// conservatively marked as a scope expansion.
fn normalize_acp_permission_request(
    provider: ProviderKind,
    request_id: &str,
    message: &Value,
    active_session_id: &str,
    turn: &TurnRequest,
) -> Result<PermissionRequest, ProviderError> {
    if request_id.is_empty() {
        return Err(ProviderError::InvalidEvent(
            "ACP permission request id must be a nonempty string".to_owned(),
        ));
    }
    let params = message.get("params").ok_or_else(|| {
        ProviderError::InvalidEvent("ACP permission request omitted params".to_owned())
    })?;
    let session_id = params
        .get("sessionId")
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| {
            ProviderError::InvalidEvent("ACP permission request omitted sessionId".to_owned())
        })?;
    if session_id != active_session_id {
        return Err(ProviderError::InvalidEvent(
            "ACP permission request targeted a different session".to_owned(),
        ));
    }
    let tool_call = params
        .get("toolCall")
        .and_then(Value::as_object)
        .ok_or_else(|| {
            ProviderError::InvalidEvent("ACP permission request omitted toolCall".to_owned())
        })?;
    let kind = match tool_call.get("kind").and_then(Value::as_str) {
        Some("execute") => PermissionKind::CommandExecution,
        Some("edit") => PermissionKind::FileChange,
        _ => {
            return Err(ProviderError::InvalidEvent(
                "ACP permission request has an unsupported tool kind".to_owned(),
            ));
        }
    };
    let _tool_call_id = tool_call
        .get("toolCallId")
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| {
            ProviderError::InvalidEvent("ACP permission request omitted toolCallId".to_owned())
        })?;
    allow_once_option(message).ok_or_else(|| {
        ProviderError::InvalidEvent(
            "ACP permission request did not offer one unambiguous allow_once option".to_owned(),
        )
    })?;
    let mut action = match kind {
        PermissionKind::CommandExecution => "execute an ACP command".to_owned(),
        PermissionKind::FileChange => "edit ACP files".to_owned(),
    };
    truncate_utf8(&mut action, MAX_PERMISSION_ACTION_BYTES);
    let mut detail = json!({
        "toolCallId": tool_call.get("toolCallId").cloned().unwrap_or(Value::Null),
        "kind": tool_call.get("kind").cloned().unwrap_or(Value::Null),
        "status": tool_call.get("status").cloned().unwrap_or(Value::Null),
    });
    detail = redact_json(detail);
    bound_json(&mut detail, MAX_PERMISSION_DETAIL_BYTES);
    Ok(PermissionRequest {
        provider,
        session_id: session_id.to_owned(),
        turn_id: Some(turn.correlation_id.to_string()),
        request_id: request_id.to_owned(),
        kind,
        action,
        scope_expansion: true,
        detail,
    })
}

fn permission_authorization_key(request: &PermissionRequest) -> Option<(String, String, String)> {
    Some((
        request.session_id.clone(),
        request.turn_id.clone()?,
        request.detail.get("toolCallId")?.as_str()?.to_owned(),
    ))
}

/// Returns the sole offered one-shot option. All option ids must be nonempty strings, and an
/// unknown/malformed option is a refusal rather than an opportunity to broaden authority.
fn allow_once_option(message: &Value) -> Option<String> {
    let options = message.get("params")?.get("options")?.as_array()?;
    let mut allow_once = None;
    let mut ids = HashSet::new();
    for option in options {
        let option = option.as_object()?;
        let id = option.get("optionId")?.as_str()?.trim();
        let name = option.get("name")?.as_str()?.trim();
        let kind = option.get("kind")?.as_str()?;
        if id.is_empty() || name.is_empty() || !ids.insert(id.to_owned()) {
            return None;
        }
        match kind {
            "allow_once" => {
                if allow_once.replace(id.to_owned()).is_some() {
                    return None;
                }
            }
            "allow_always" | "reject_once" | "reject_always" => {}
            _ => return None,
        }
    }
    allow_once
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

fn bound_json(value: &mut Value, maximum: usize) {
    if serde_json::to_vec(value).map_or(true, |encoded| encoded.len() > maximum) {
        *value = json!({"truncated": true});
    }
}

fn serialize_params<T: Serialize>(value: &T) -> Result<Value, ProviderError> {
    serde_json::to_value(value).map_err(ProviderError::from)
}

#[cfg(test)]
mod tests {
    #[test]
    fn acp_stop_reasons_normalize_to_contract_statuses() {
        assert_eq!(
            super::stop_reason_string(&serde_json::json!("end_turn")),
            "completed"
        );
        assert_eq!(
            super::stop_reason_string(&serde_json::json!("cancelled")),
            "cancelled"
        );
        for reason in ["max_tokens", "max_turn_requests", "refusal"] {
            assert_eq!(
                super::stop_reason_string(&serde_json::json!(reason)),
                "failed"
            );
        }
        assert_eq!(
            super::stop_reason_string(&serde_json::json!("something_new")),
            "something_new"
        );
        assert_eq!(super::stop_reason_string(&serde_json::json!(7)), "7");
    }

    use super::*;

    fn permission(kind: &str, options: &Value) -> Value {
        json!({
            "jsonrpc": "2.0", "id": "permission-1", "method": "session/request_permission",
            "params": {"sessionId": "session-1", "toolCall": {"toolCallId": "call-1", "title": "Write file", "kind": kind}, "options": options}
        })
    }

    #[test]
    fn acp_permission_requires_a_single_valid_one_shot_option_and_typed_kind() {
        let message = permission(
            "edit",
            &json!([
                {"optionId":"allow-one", "name":"Allow once", "kind":"allow_once"},
                {"optionId":"always", "name":"Always", "kind":"allow_always"}
            ]),
        );
        let request = normalize_acp_permission_request(
            ProviderKind::OpenCode,
            "permission-1",
            &message,
            "session-1",
            &TurnRequest::new("x"),
        )
        .unwrap();
        assert_eq!(request.kind, PermissionKind::FileChange);
        assert_eq!(allow_once_option(&message).as_deref(), Some("allow-one"));
        assert!(request.scope_expansion);

        let unknown = permission(
            "network",
            &json!([{"optionId":"allow-one", "kind":"allow_once"}]),
        );
        assert!(
            normalize_acp_permission_request(
                ProviderKind::OpenCode,
                "permission-1",
                &unknown,
                "session-1",
                &TurnRequest::new("x"),
            )
            .is_err()
        );
        let ambiguous = permission(
            "edit",
            &json!([
                {"optionId":"a", "kind":"allow_once"}, {"optionId":"b", "kind":"allow_once"}
            ]),
        );
        assert!(allow_once_option(&ambiguous).is_none());
    }

    #[test]
    fn acp_permission_rejects_another_active_session() {
        let message = permission(
            "execute",
            &json!([{"optionId":"allow-one", "kind":"allow_once"}]),
        );
        assert!(
            normalize_acp_permission_request(
                ProviderKind::GrokBuild,
                "permission-1",
                &message,
                "other-session",
                &TurnRequest::new("x"),
            )
            .is_err()
        );
    }
}
