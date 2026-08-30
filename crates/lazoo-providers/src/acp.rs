use std::{
    cmp,
    sync::atomic::{AtomicBool, Ordering},
    time::{Duration, Instant},
};

use agent_client_protocol::schema::{
    ProtocolVersion,
    v1::{
        CancelNotification, CloseSessionRequest, ContentBlock, Implementation, InitializeRequest,
        InitializeResponse, LoadSessionRequest, NewSessionRequest, NewSessionResponse,
        PromptRequest, RequestPermissionOutcome, RequestPermissionResponse, TextContent,
    },
};
use serde::Serialize;
use serde_json::{Value, json};
use tokio::sync::Mutex;

use crate::{
    AdapterFuture, CommandSpec, ProviderError, ProviderEvent, ProviderKind, ProviderSession,
    SessionRequest, TurnRequest,
    json_rpc::{JsonRpcControl, JsonRpcProcess, response_id},
    redact_json,
};

const STOP_TIMEOUT: Duration = Duration::from_secs(2);

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
        response.session_id.to_string()
    };
    let close_supported = handshake
        .response
        .agent_capabilities
        .session_capabilities
        .close
        .is_some();
    let control = process.control();
    Ok((
        AcpSession {
            process: Mutex::new(process),
            control,
            provider,
            session_id,
            resumed,
            request: request.clone(),
            close_supported,
            session_event_pending: AtomicBool::new(true),
            active: AtomicBool::new(false),
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
    resumed: bool,
    request: SessionRequest,
    close_supported: bool,
    session_event_pending: AtomicBool,
    active: AtomicBool,
    stopped: AtomicBool,
}

impl AcpSession {
    async fn run_turn(&self, request: &TurnRequest) -> Result<Vec<ProviderEvent>, ProviderError> {
        if self.stopped.load(Ordering::Acquire) {
            return Err(ProviderError::Protocol(
                "ACP session has already stopped".to_owned(),
            ));
        }
        let mut process = self.process.lock().await;
        if self.active.swap(true, Ordering::AcqRel) {
            return Err(ProviderError::Protocol(
                "ACP session already has an active turn".to_owned(),
            ));
        }
        let outcome = self.collect_turn(&mut process, request).await;
        self.active.store(false, Ordering::Release);
        outcome
    }

    async fn collect_turn(
        &self,
        process: &mut JsonRpcProcess,
        request: &TurnRequest,
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
                return Ok(events);
            }

            if let Some(permission) = self.handle_permission_request(&message).await? {
                progress.progress_at(Instant::now());
                events.push(permission);
                continue;
            }
            let Some(event) = normalize_acp_message(self.provider, &message)? else {
                continue;
            };
            if let ProviderEvent::TextDelta { text, .. } = &event {
                output.push_str(text);
            }
            progress.progress_at(Instant::now());
            events.push(event);
        }
    }

    async fn handle_permission_request(
        &self,
        message: &Value,
    ) -> Result<Option<ProviderEvent>, ProviderError> {
        if message.get("method").and_then(Value::as_str) != Some("session/request_permission") {
            return Ok(None);
        }
        let Some(id) = message.get("id").cloned() else {
            return Err(ProviderError::InvalidEvent(
                "ACP permission request omitted its JSON-RPC id".to_owned(),
            ));
        };
        let event = normalize_acp_message(self.provider, message)?.ok_or_else(|| {
            ProviderError::InvalidEvent("ACP permission request did not normalize".to_owned())
        })?;
        let response = RequestPermissionResponse::new(RequestPermissionOutcome::Cancelled);
        self.control
            .respond(id, Ok(serialize_params(&response)?))
            .await?;
        Ok(Some(event))
    }
}

impl ProviderSession for AcpSession {
    fn id(&self) -> String {
        self.session_id.clone()
    }

    fn turn<'a>(&'a self, request: &'a TurnRequest) -> AdapterFuture<'a, Vec<ProviderEvent>> {
        Box::pin(async move { self.run_turn(request).await })
    }

    fn cancel(&self) -> AdapterFuture<'_, ()> {
        Box::pin(async move {
            if self.active.load(Ordering::Acquire) {
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

fn stop_reason_string(value: &Value) -> String {
    value
        .as_str()
        .map_or_else(|| value.to_string(), str::to_owned)
}

fn serialize_params<T: Serialize>(value: &T) -> Result<Value, ProviderError> {
    serde_json::to_value(value).map_err(ProviderError::from)
}
