use std::sync::atomic::{AtomicU64, Ordering};

use lazoo_protocol::{
    CancelTurnParams, ConversationDetail, ConversationListParams, ConversationShowParams,
    CreateConversationParams, EmployeeAvailability, JsonRpcRequest, MessageSender, RetryTurnParams,
    RpcError, SessionLifecycleState, SubmitTurnParams, TaskState, TurnListParams,
    TurnMutationResult, TurnShowParams, TurnState,
};
use lazoo_providers::{ProviderError, ProviderEvent, ProviderKind, TurnEventObserver, TurnRequest};
use lazoo_store::{
    BeginTurnSubmissionInput, CreateConversationInput, FailTurnInput, FinishTurnCancelInput,
    MarkTurnUncertainInput, ObserveTurnInput, RecordTurnInput, RequestTurnCancelInput,
    RetryTurnInput, StoreError,
};
use serde_json::{Value, json};
use tokio::time::{Duration, sleep};
use uuid::Uuid;

use super::{App, to_value, truncate_utf8};

struct AppTurnObserver {
    app: App,
    turn_id: Uuid,
    ordinal: AtomicU64,
}

struct DeliveryTarget {
    employee: lazoo_protocol::EmployeeRecord,
    provider: ProviderKind,
    session_id: Option<Uuid>,
}

impl AppTurnObserver {
    fn new(app: App, turn_id: Uuid) -> Self {
        Self {
            app,
            turn_id,
            ordinal: AtomicU64::new(0),
        }
    }

    fn command_identity(&self, action: &str) -> (String, String) {
        let ordinal = self.ordinal.fetch_add(1, Ordering::SeqCst) + 1;
        (
            format!("turn-provider-{}-{action}-{ordinal}", self.turn_id),
            format!("provider:{}:{action}:{ordinal}", self.turn_id),
        )
    }
}

impl TurnEventObserver for AppTurnObserver {
    fn observe(&self, event: &ProviderEvent) -> Result<(), ProviderError> {
        let current = self
            .app
            .store
            .get_turn(self.turn_id)
            .map_err(|error| observer_error(&error))?;
        let payload = bounded_event_payload(event);
        let (kind, provider, native_session_id, native_turn_id) = event_identity(event, &payload);
        let cancellation_evidence = match event {
            ProviderEvent::Cancelled { .. } => true,
            ProviderEvent::TurnCompleted { status, .. } => {
                status == "interrupted" || status == "cancelled"
            }
            _ => false,
        };
        let result = match event {
            _ if cancellation_evidence && current.cancel_requested_at.is_some() => {
                let (request_id, idempotency_key) = self.command_identity("cancelled");
                self.app.store.finish_turn_cancel(&FinishTurnCancelInput {
                    request_id,
                    idempotency_key,
                    turn_id: self.turn_id,
                    expected_revision: current.revision,
                    provider,
                    native_session_id,
                    native_turn_id,
                    payload,
                })
            }
            _ => {
                let transition = match event {
                    ProviderEvent::TurnStarted { .. } => Some(TurnState::Accepted),
                    ProviderEvent::TurnCompleted { status, .. } if status == "completed" => {
                        Some(TurnState::Completed)
                    }
                    ProviderEvent::TurnCompleted { status, .. } if status == "failed" => {
                        Some(TurnState::TerminalFailed)
                    }
                    ProviderEvent::TurnCompleted { status, .. }
                        if status == "interrupted" || status == "cancelled" =>
                    {
                        Some(TurnState::Cancelled)
                    }
                    ProviderEvent::Cancelled { .. } => Some(TurnState::Cancelled),
                    _ => None,
                };
                let (request_id, idempotency_key) = self.command_identity("event");
                self.app.store.observe_turn(&ObserveTurnInput {
                    request_id,
                    idempotency_key,
                    turn_id: self.turn_id,
                    expected_revision: current.revision,
                    kind,
                    provider,
                    native_session_id,
                    native_turn_id,
                    payload,
                    transition,
                })
            }
        }
        .map_err(|error| observer_error(&error))?;
        let _ = result;
        self.app
            .publish_events()
            .map_err(|error| ProviderError::Observer(error.message))?;
        Ok(())
    }
}

impl App {
    pub(super) fn create_conversation(&self, request: &JsonRpcRequest) -> Result<Value, RpcError> {
        let params: CreateConversationParams = serde_json::from_value(request.params.clone())
            .map_err(|error| conversation_invalid(error.to_string()))?;
        let result = self
            .store
            .create_conversation(&CreateConversationInput {
                request_id: request.id.clone(),
                idempotency_key: params.idempotency_key,
                project_id: params.project_id,
                lead_employee_id: params.lead_employee_id,
                title: params.title,
            })
            .map_err(conversation_store_error)?;
        self.publish_events()?;
        Ok(to_value(result))
    }

    pub(super) fn conversation_list(&self, value: Value) -> Result<Value, RpcError> {
        let params: ConversationListParams = serde_json::from_value(value)
            .map_err(|error| conversation_invalid(error.to_string()))?;
        self.store
            .list_conversations(params.after_sequence, params.limit)
            .map(to_value)
            .map_err(conversation_store_error)
    }

    pub(super) fn conversation_show(&self, value: Value) -> Result<Value, RpcError> {
        let params: ConversationShowParams = serde_json::from_value(value)
            .map_err(|error| conversation_invalid(error.to_string()))?;
        let conversation = self
            .store
            .get_conversation(params.id)
            .map_err(conversation_store_error)?;
        let turns = self
            .store
            .list_turns(params.id, 0, 100)
            .map_err(conversation_store_error)?;
        Ok(to_value(ConversationDetail {
            conversation,
            turns,
        }))
    }

    pub(super) async fn submit_turn(&self, request: &JsonRpcRequest) -> Result<Value, RpcError> {
        let params: SubmitTurnParams = serde_json::from_value(request.params.clone())
            .map_err(|error| turn_invalid(error.to_string()))?;
        let activate_if_needed = params.activate_if_needed;
        let recorded = self
            .store
            .record_turn(&RecordTurnInput {
                request_id: request.id.clone(),
                idempotency_key: params.idempotency_key,
                conversation_id: params.conversation_id,
                author: params.author,
                recipient: params.recipient,
                task_id: params.task_id,
                authority_mode: params.authority_mode,
                text: params.text,
                activate_if_needed: params.activate_if_needed,
                max_attempts: params.max_attempts,
            })
            .map_err(turn_store_error)?;
        self.publish_events()?;
        if recorded.turn.state != TurnState::Recorded {
            return Ok(to_value(recorded));
        }
        require_task_delivery_available(&recorded)?;
        self.deliver_recorded_turn(request, recorded, activate_if_needed)
            .await
    }

    async fn deliver_recorded_turn(
        &self,
        request: &JsonRpcRequest,
        recorded: TurnMutationResult,
        activate_if_needed: bool,
    ) -> Result<Value, RpcError> {
        let turn = &recorded.turn;
        let target = self.delivery_target(turn)?;
        let (session_id, session) = self
            .resolve_delivery_session(&request.id, turn, target, activate_if_needed)
            .await?;
        let submitting = self
            .store
            .begin_turn_submission(&BeginTurnSubmissionInput {
                request_id: format!("{}:begin", request.id),
                idempotency_key: format!("turn:{}:begin", turn.id),
                turn_id: turn.id,
                expected_revision: turn.revision,
                owner_boot_id: self.boot_id,
                recipient_session_id: session_id,
            })
            .map_err(turn_store_error)?;
        self.publish_events()?;
        self.deliver_submitting_turn(&request.id, submitting, session)
            .await
    }

    fn delivery_target(
        &self,
        turn: &lazoo_protocol::ConversationTurnRecord,
    ) -> Result<DeliveryTarget, RpcError> {
        let employee = self
            .store
            .get_employee(turn.recipient.employee_id)
            .map_err(turn_store_error)?;
        if !employee.enabled {
            return Err(turn_failure(
                "LZ-TURN-EMPLOYEE-DISABLED",
                "the recipient employee is disabled",
                turn.id,
            ));
        }
        if employee.configuration_revision != turn.recipient_employee_configuration_revision {
            return Err(RpcError {
                code: "LZ-TURN-EMPLOYEE-STALE".to_owned(),
                message: "the recipient employee configuration changed before delivery".to_owned(),
                data: Some(json!({
                    "turnId": turn.id,
                    "currentRevision": employee.configuration_revision
                })),
            });
        }
        let roster = self.store.roster_snapshot().map_err(turn_store_error)?;
        let roster_entry = roster
            .entries
            .into_iter()
            .find(|entry| entry.employee.id == employee.id)
            .ok_or_else(|| {
                turn_failure(
                    "LZ-TURN-EMPLOYEE-NOT-FOUND",
                    "the recipient employee is no longer in the roster",
                    turn.id,
                )
            })?;
        let (route_availability, _) = lazoo_domain::derive_availability(
            employee.enabled,
            roster_entry.provider_observation.as_ref(),
        );
        if route_availability != EmployeeAvailability::Dormant {
            return Err(turn_failure(
                "LZ-TURN-EMPLOYEE-NOT-READY",
                "the recipient employee is not route ready",
                turn.id,
            ));
        }
        let provider = employee
            .provider
            .parse::<ProviderKind>()
            .map_err(|_| turn_invalid("the recipient employee has an unknown provider"))?;
        if provider != ProviderKind::Codex {
            return Err(turn_failure(
                if turn.task_id.is_some() {
                    "LZ-TASK-AUTHORITY-UNSUPPORTED"
                } else {
                    "LZ-TURN-PROVIDER-UNSUPPORTED"
                },
                if turn.task_id.is_some() {
                    "the selected provider cannot enforce this task authority ceiling"
                } else {
                    "conversation turn delivery currently supports only Codex"
                },
                turn.id,
            ));
        }
        let current_session_id = turn.recipient.session_id.or_else(|| {
            roster_entry.session.as_ref().and_then(|session| {
                (session.state == SessionLifecycleState::Idle
                    && session.owner_boot_id == Some(self.boot_id)
                    && session.employee_configuration_revision == employee.configuration_revision)
                    .then_some(session.id)
            })
        });
        Ok(DeliveryTarget {
            employee,
            provider,
            session_id: current_session_id,
        })
    }

    async fn resolve_delivery_session(
        &self,
        request_id: &str,
        turn: &lazoo_protocol::ConversationTurnRecord,
        target: DeliveryTarget,
        activate_if_needed: bool,
    ) -> Result<(Uuid, std::sync::Arc<dyn super::ManagedSession>), RpcError> {
        let session_id = if let Some(session_id) = target.session_id {
            session_id
        } else if activate_if_needed && matches!(turn.author, MessageSender::User(_)) {
            let starting = self
                .store
                .begin_session(&super::BeginSessionInput {
                    request_id: format!("{request_id}:activate"),
                    idempotency_key: format!("turn-activation:{}", turn.id),
                    employee_id: target.employee.id,
                    expected_employee_revision: target.employee.configuration_revision,
                    owner_boot_id: self.boot_id,
                })
                .map_err(turn_store_error)?;
            self.publish_events()?;
            let ready = if starting.event_sequence.is_some() {
                self.start_new_session(target.employee, target.provider, starting)
                    .await
                    .map_err(|_| {
                        turn_failure(
                            "LZ-TURN-ACTIVATION-FAILED",
                            "the recipient session could not be activated",
                            turn.id,
                        )
                    })?
            } else {
                starting
            };
            ready.session.id
        } else {
            return Err(turn_failure(
                "LZ-TURN-SESSION-NOT-READY",
                "the recipient employee has no active session",
                turn.id,
            ));
        };
        let session = self
            .live_sessions
            .lock()
            .await
            .get(&session_id)
            .cloned()
            .ok_or_else(|| {
                turn_failure(
                    "LZ-TURN-SESSION-HANDLE-MISSING",
                    "the daemon does not own the recipient session",
                    turn.id,
                )
            })?;
        Ok((session_id, session))
    }

    async fn deliver_submitting_turn(
        &self,
        request_id: &str,
        submitting: TurnMutationResult,
        session: std::sync::Arc<dyn super::ManagedSession>,
    ) -> Result<Value, RpcError> {
        let turn = &submitting.turn;
        let permission_mode = submitting
            .task
            .as_ref()
            .map(|task| super::authority_permission_mode(task.effective_authority).to_owned());
        self.live_turns
            .lock()
            .await
            .insert(turn.id, session.clone());
        let observer = AppTurnObserver::new(self.clone(), turn.id);
        let provider_result = session
            .turn_observed(
                &TurnRequest {
                    correlation_id: turn.correlation_id,
                    prompt: turn.text.clone(),
                    permission_mode,
                },
                &observer,
            )
            .await;
        self.live_turns.lock().await.remove(&turn.id);

        match provider_result {
            Ok(_) => self.finish_provider_delivery(turn.id, request_id),
            Err(error) => self.fail_provider_delivery(turn.id, request_id, &error),
        }
    }

    fn finish_provider_delivery(&self, turn_id: Uuid, request_id: &str) -> Result<Value, RpcError> {
        let current = self.store.get_turn(turn_id).map_err(turn_store_error)?;
        if matches!(
            current.state,
            TurnState::Completed
                | TurnState::Cancelled
                | TurnState::RetryableFailed
                | TurnState::TerminalFailed
                | TurnState::Uncertain
        ) {
            return self.turn_result(current, None);
        }
        let uncertain = self
            .store
            .mark_turn_uncertain(&MarkTurnUncertainInput {
                request_id: format!("{request_id}:uncertain"),
                idempotency_key: format!("turn:{turn_id}:no-evidence"),
                turn_id,
                expected_revision: current.revision,
                failure_code: "LZ-TURN-NO-ACCEPTANCE-EVIDENCE".to_owned(),
                failure_message: "provider returned without durable completion evidence".to_owned(),
            })
            .map_err(turn_store_error)?;
        self.publish_events()?;
        Err(turn_failure(
            "LZ-TURN-UNCERTAIN",
            "provider delivery outcome is uncertain",
            uncertain.turn.id,
        ))
    }

    fn fail_provider_delivery(
        &self,
        turn_id: Uuid,
        request_id: &str,
        error: &ProviderError,
    ) -> Result<Value, RpcError> {
        let current = self.store.get_turn(turn_id).map_err(turn_store_error)?;
        if current.state == TurnState::Submitting && !matches!(error, ProviderError::Observer(_)) {
            let retryable = provider_error_is_retryable(error);
            self.store
                .fail_turn(&FailTurnInput {
                    request_id: format!("{request_id}:failed"),
                    idempotency_key: format!("turn:{turn_id}:provider-failed"),
                    turn_id,
                    expected_revision: current.revision,
                    retryable,
                    failure_code: "LZ-TURN-PROVIDER".to_owned(),
                    failure_message: "provider rejected the turn before acceptance".to_owned(),
                })
                .map_err(turn_store_error)?;
            self.publish_events()?;
            return Err(turn_failure(
                "LZ-TURN-PROVIDER",
                "provider rejected the turn before acceptance",
                turn_id,
            ));
        }
        if matches!(current.state, TurnState::Submitting | TurnState::Accepted) {
            self.store
                .mark_turn_uncertain(&MarkTurnUncertainInput {
                    request_id: format!("{request_id}:uncertain"),
                    idempotency_key: format!("turn:{turn_id}:provider-uncertain"),
                    turn_id,
                    expected_revision: current.revision,
                    failure_code: "LZ-TURN-PROVIDER-UNCERTAIN".to_owned(),
                    failure_message: "provider delivery could not be proven after input began"
                        .to_owned(),
                })
                .map_err(turn_store_error)?;
            self.publish_events()?;
            return Err(turn_failure(
                "LZ-TURN-UNCERTAIN",
                "provider delivery outcome is uncertain",
                turn_id,
            ));
        }
        self.turn_result(current, None)
    }

    fn turn_result(
        &self,
        turn: lazoo_protocol::ConversationTurnRecord,
        event_sequence: Option<u64>,
    ) -> Result<Value, RpcError> {
        let message = self
            .store
            .get_message(turn.message_id)
            .map_err(turn_store_error)?;
        let task = turn
            .task_id
            .map(|task_id| {
                self.store
                    .get_task(task_id)
                    .map(|detail| detail.task)
                    .map_err(turn_store_error)
            })
            .transpose()?;
        Ok(to_value(TurnMutationResult {
            turn,
            message,
            task,
            event_sequence,
        }))
    }

    pub(super) async fn retry_turn(&self, request: &JsonRpcRequest) -> Result<Value, RpcError> {
        let params: RetryTurnParams = serde_json::from_value(request.params.clone())
            .map_err(|error| turn_invalid(error.to_string()))?;
        let current = self.store.get_turn(params.id).map_err(turn_store_error)?;
        let target = self.delivery_target(&current)?;
        let session_id = target.session_id.ok_or_else(|| {
            turn_failure(
                "LZ-TURN-SESSION-NOT-READY",
                "the recipient employee has no active session",
                current.id,
            )
        })?;
        let session = self
            .live_sessions
            .lock()
            .await
            .get(&session_id)
            .cloned()
            .ok_or_else(|| {
                turn_failure(
                    "LZ-TURN-SESSION-HANDLE-MISSING",
                    "the daemon does not own the recipient session",
                    current.id,
                )
            })?;
        let submitting = self
            .store
            .retry_turn(&RetryTurnInput {
                request_id: request.id.clone(),
                idempotency_key: params.idempotency_key,
                turn_id: params.id,
                expected_revision: params.expected_revision,
                owner_boot_id: self.boot_id,
                recipient_session_id: session_id,
            })
            .map_err(turn_store_error)?;
        self.publish_events()?;
        if submitting.event_sequence.is_none() {
            return Ok(to_value(submitting));
        }
        self.deliver_submitting_turn(&request.id, submitting, session)
            .await
    }

    pub(super) async fn cancel_turn(&self, request: &JsonRpcRequest) -> Result<Value, RpcError> {
        let params: CancelTurnParams = serde_json::from_value(request.params.clone())
            .map_err(|error| turn_invalid(error.to_string()))?;
        let requested = self
            .store
            .request_turn_cancel(&RequestTurnCancelInput {
                request_id: request.id.clone(),
                idempotency_key: params.idempotency_key,
                turn_id: params.id,
                expected_revision: params.expected_revision,
            })
            .map_err(turn_store_error)?;
        self.publish_events()?;
        if requested.turn.state == TurnState::Cancelled || requested.event_sequence.is_none() {
            return Ok(to_value(requested));
        }
        let Some(session) = self.live_turns.lock().await.get(&params.id).cloned() else {
            return self.mark_cancel_uncertain(
                params.id,
                requested.turn.revision,
                &request.id,
                "the daemon no longer owns the in-flight provider turn",
            );
        };
        if session.cancel().await.is_err() {
            return self.mark_cancel_uncertain(
                params.id,
                requested.turn.revision,
                &request.id,
                "the provider cancellation request could not be confirmed",
            );
        }
        for _ in 0..200 {
            let current = self.store.get_turn(params.id).map_err(turn_store_error)?;
            if current.state == TurnState::Cancelled {
                return self.turn_result(current, None);
            }
            if current.state == TurnState::Uncertain {
                return Err(turn_failure(
                    "LZ-TURN-UNCERTAIN",
                    "provider cancellation outcome is uncertain",
                    params.id,
                ));
            }
            sleep(Duration::from_millis(10)).await;
        }
        let current = self.store.get_turn(params.id).map_err(turn_store_error)?;
        self.mark_cancel_uncertain(
            params.id,
            current.revision,
            &request.id,
            "the provider did not emit cancellation evidence before the deadline",
        )
    }

    fn mark_cancel_uncertain(
        &self,
        turn_id: Uuid,
        expected_revision: u64,
        request_id: &str,
        reason: &str,
    ) -> Result<Value, RpcError> {
        self.store
            .mark_turn_uncertain(&MarkTurnUncertainInput {
                request_id: format!("{request_id}:uncertain"),
                idempotency_key: format!("turn:{turn_id}:cancel-uncertain"),
                turn_id,
                expected_revision,
                failure_code: "LZ-TURN-CANCEL-UNCERTAIN".to_owned(),
                failure_message: reason.to_owned(),
            })
            .map_err(turn_store_error)?;
        self.publish_events()?;
        Err(turn_failure(
            "LZ-TURN-UNCERTAIN",
            "provider cancellation outcome is uncertain",
            turn_id,
        ))
    }

    pub(super) fn turn_list(&self, value: Value) -> Result<Value, RpcError> {
        let params: TurnListParams =
            serde_json::from_value(value).map_err(|error| turn_invalid(error.to_string()))?;
        self.store
            .list_turns(params.conversation_id, params.after_sequence, params.limit)
            .map(to_value)
            .map_err(turn_store_error)
    }

    pub(super) fn turn_show(&self, value: Value) -> Result<Value, RpcError> {
        let params: TurnShowParams =
            serde_json::from_value(value).map_err(|error| turn_invalid(error.to_string()))?;
        self.store
            .get_turn(params.id)
            .map(to_value)
            .map_err(turn_store_error)
    }
}

fn require_task_delivery_available(recorded: &TurnMutationResult) -> Result<(), RpcError> {
    let Some(task) = &recorded.task else {
        return Ok(());
    };
    let delivery_eligible = matches!(task.state, TaskState::Accepted | TaskState::Running);
    let another_turn_is_active = task
        .active_turn_id
        .is_some_and(|active_turn_id| active_turn_id != recorded.turn.id);
    if !delivery_eligible || another_turn_is_active {
        Err(RpcError {
            code: "LZ-TURN-TASK-SCOPE".to_owned(),
            message: "the linked task cannot accept this turn".to_owned(),
            data: Some(json!({"taskId": task.id, "turnId": recorded.turn.id})),
        })
    } else {
        Ok(())
    }
}

fn observer_error(error: &StoreError) -> ProviderError {
    ProviderError::Observer(error.to_string())
}

fn event_identity(
    event: &ProviderEvent,
    payload: &Value,
) -> (String, String, Option<String>, Option<String>) {
    let kind = payload
        .get("type")
        .and_then(Value::as_str)
        .unwrap_or("provider-event")
        .to_owned();
    let (provider, session_id, turn_id) = match event {
        ProviderEvent::SessionStarted {
            provider,
            session_id,
            ..
        }
        | ProviderEvent::TurnStarted {
            provider,
            session_id,
            ..
        }
        | ProviderEvent::TextDelta {
            provider,
            session_id,
            ..
        }
        | ProviderEvent::ThoughtDelta {
            provider,
            session_id,
            ..
        }
        | ProviderEvent::ToolUpdate {
            provider,
            session_id,
            ..
        }
        | ProviderEvent::TerminalUpdate {
            provider,
            session_id,
            ..
        }
        | ProviderEvent::PermissionRequested {
            provider,
            session_id,
            ..
        }
        | ProviderEvent::Progress {
            provider,
            session_id,
            ..
        }
        | ProviderEvent::Usage {
            provider,
            session_id,
            ..
        }
        | ProviderEvent::TurnCompleted {
            provider,
            session_id,
            ..
        }
        | ProviderEvent::Cancelled {
            provider,
            session_id,
            ..
        } => (*provider, Some(session_id.clone()), event_turn_id(event)),
        ProviderEvent::Error {
            provider,
            session_id,
            ..
        }
        | ProviderEvent::ProcessExited {
            provider,
            session_id,
            ..
        } => (*provider, session_id.clone(), None),
    };
    (kind, provider.to_string(), session_id, turn_id)
}

fn event_turn_id(event: &ProviderEvent) -> Option<String> {
    match event {
        ProviderEvent::TurnStarted { turn_id, .. }
        | ProviderEvent::TextDelta { turn_id, .. }
        | ProviderEvent::ThoughtDelta { turn_id, .. }
        | ProviderEvent::ToolUpdate { turn_id, .. }
        | ProviderEvent::TurnCompleted { turn_id, .. }
        | ProviderEvent::Cancelled { turn_id, .. } => turn_id.clone(),
        _ => None,
    }
}

fn bounded_event_payload(event: &ProviderEvent) -> Value {
    const MAX_EVENT_STRING_BYTES: usize = 60 * 1024;
    const MAX_EVENT_PAYLOAD_BYTES: usize = 64 * 1024;
    let mut payload = to_value(event);
    if let Some(object) = payload.as_object_mut() {
        object.remove("raw");
        for field in ["text", "output", "message"] {
            if let Some(Value::String(value)) = object.get_mut(field)
                && value.len() > MAX_EVENT_STRING_BYTES
            {
                truncate_utf8(value, MAX_EVENT_STRING_BYTES);
                object.insert("truncated".to_owned(), Value::Bool(true));
            }
        }
    }
    if serde_json::to_vec(&payload).is_ok_and(|bytes| bytes.len() <= MAX_EVENT_PAYLOAD_BYTES) {
        payload
    } else {
        let (kind, provider, native_session_id, native_turn_id) = event_identity(event, &payload);
        json!({
            "type": kind,
            "provider": provider,
            "sessionId": native_session_id,
            "turnId": native_turn_id,
            "truncated": true
        })
    }
}

fn provider_error_is_retryable(error: &ProviderError) -> bool {
    matches!(
        error,
        ProviderError::Timeout { .. }
            | ProviderError::Stalled { .. }
            | ProviderError::ProcessExited { .. }
            | ProviderError::Io(_)
            | ProviderError::Runtime(_)
    )
}

fn turn_failure(code: &str, message: &str, turn_id: Uuid) -> RpcError {
    RpcError {
        code: code.to_owned(),
        message: message.to_owned(),
        data: Some(json!({"turnId": turn_id})),
    }
}

fn turn_invalid(message: impl Into<String>) -> RpcError {
    RpcError {
        code: "LZ-TURN-INVALID".to_owned(),
        message: message.into(),
        data: None,
    }
}

fn conversation_invalid(message: impl Into<String>) -> RpcError {
    RpcError {
        code: "LZ-CONVERSATION-INVALID".to_owned(),
        message: message.into(),
        data: None,
    }
}

#[allow(clippy::needless_pass_by_value)]
fn conversation_store_error(error: StoreError) -> RpcError {
    let (code, data, public_message) = match &error {
        StoreError::EmployeeNotFound { employee_id } => (
            "LZ-CONVERSATION-LEAD-NOT-FOUND",
            Some(json!({"employeeId": employee_id})),
            error.to_string(),
        ),
        StoreError::EmployeeDisabled { employee_id } => (
            "LZ-CONVERSATION-LEAD-DISABLED",
            Some(json!({"employeeId": employee_id})),
            error.to_string(),
        ),
        StoreError::SessionProviderNotReady { employee_id, .. } => (
            "LZ-CONVERSATION-LEAD-NOT-READY",
            Some(json!({"employeeId": employee_id})),
            error.to_string(),
        ),
        StoreError::MessageProjectNotFound { project_id } => (
            "LZ-CONVERSATION-PROJECT-NOT-FOUND",
            Some(json!({"projectId": project_id})),
            error.to_string(),
        ),
        StoreError::ConversationNotFound { conversation_id } => (
            "LZ-CONVERSATION-NOT-FOUND",
            Some(json!({"conversationId": conversation_id})),
            error.to_string(),
        ),
        StoreError::ConversationValidation(_) => {
            ("LZ-CONVERSATION-INVALID", None, error.to_string())
        }
        _ => (
            "LZ-CONVERSATION-STORE",
            None,
            "conversation store operation failed".to_owned(),
        ),
    };
    RpcError {
        code: code.to_owned(),
        message: public_message,
        data,
    }
}

#[allow(clippy::needless_pass_by_value)]
fn turn_store_error(error: StoreError) -> RpcError {
    let (code, data, public_message) = match &error {
        StoreError::TurnNotFound { turn_id } => (
            "LZ-TURN-NOT-FOUND",
            Some(json!({"turnId": turn_id})),
            error.to_string(),
        ),
        StoreError::ConversationNotFound { conversation_id } => (
            "LZ-TURN-CONVERSATION-NOT-FOUND",
            Some(json!({"conversationId": conversation_id})),
            error.to_string(),
        ),
        StoreError::TurnStale {
            turn_id,
            current_revision,
        } => (
            "LZ-TURN-STALE",
            Some(json!({"turnId": turn_id, "currentRevision": current_revision})),
            error.to_string(),
        ),
        StoreError::TurnState { turn_id, .. } => (
            "LZ-TURN-STATE",
            Some(json!({"turnId": turn_id})),
            error.to_string(),
        ),
        StoreError::TurnAcceptanceEvidence { turn_id } => (
            "LZ-TURN-ACCEPTANCE-EVIDENCE",
            Some(json!({"turnId": turn_id})),
            error.to_string(),
        ),
        StoreError::TaskNotFound { task_id } => (
            "LZ-TURN-TASK-NOT-FOUND",
            Some(json!({"taskId": task_id})),
            error.to_string(),
        ),
        StoreError::TurnTaskScope { task_id } => (
            "LZ-TURN-TASK-SCOPE",
            Some(json!({"taskId": task_id})),
            error.to_string(),
        ),
        StoreError::TaskTurnLink { task_id, turn_id } => (
            "LZ-TURN-TASK-SCOPE",
            Some(json!({"taskId": task_id, "turnId": turn_id})),
            error.to_string(),
        ),
        StoreError::ConversationValidation(_) | StoreError::MessageValidation(_) => {
            ("LZ-TURN-INVALID", None, error.to_string())
        }
        StoreError::MessageRetryNotDue {
            message_id,
            next_attempt_at,
        } => (
            "LZ-TURN-RETRY-NOT-DUE",
            Some(json!({"messageId": message_id, "nextAttemptAt": next_attempt_at})),
            error.to_string(),
        ),
        StoreError::SessionNotFound { session_id }
        | StoreError::MessageSessionNotFound { session_id, .. } => (
            "LZ-TURN-SESSION-NOT-FOUND",
            Some(json!({"sessionId": session_id})),
            error.to_string(),
        ),
        StoreError::SessionOwnership { session_id } => (
            "LZ-TURN-SESSION-OWNERSHIP",
            Some(json!({"sessionId": session_id})),
            error.to_string(),
        ),
        StoreError::SessionState { session_id, .. } => (
            "LZ-TURN-SESSION-STATE",
            Some(json!({"sessionId": session_id})),
            error.to_string(),
        ),
        _ => (
            "LZ-TURN-STORE",
            None,
            "turn store operation failed".to_owned(),
        ),
    };
    RpcError {
        code: code.to_owned(),
        message: public_message,
        data,
    }
}
