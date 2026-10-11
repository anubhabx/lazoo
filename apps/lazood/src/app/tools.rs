//! Lazoo collaboration tools for daemon-managed Codex sessions.
//!
//! The daemon declares these as provider dynamic tools; every call acts as the turn's recipient
//! employee endpoint, so the existing store guards (actor session, authority narrowing, roster
//! revision, self-delegation, depth, fan-out, active descendants) fail closed exactly as they do
//! for the CLI. A refused or failed call answers the model with `success = false` and never ends
//! the provider turn.

use lazoo_domain::MAX_INCIDENT_REMEDIES;
use lazoo_protocol::{
    AuthorityMode, ConversationTurnRecord, IncidentRemedyKind, IncidentSource, IncidentState,
    MessageDeliveryState, MessageEndpoint, MessageRecord, MessageReference, RpcError, TaskDetail,
    TaskReportKind, TaskState, TurnState,
};
use lazoo_providers::{ToolCall, ToolResult, ToolSpec};
use lazoo_store::{
    ApplyIncidentRemedyInput, ConsumePeerMessageInput, ConsumeTaskResultInput, DelegateTaskInput,
    IncidentRemedyAction, ListIncidentsInput, ListTasksInput, RaiseIncidentInput,
    ReplyPeerMessageInput, ReportTaskInput, SendPeerMessageInput, StoreError,
};
use serde::Deserialize;
use serde_json::{Value, json};
use tokio::time::{Duration, Instant, sleep};
use uuid::Uuid;

use super::{App, incident::incident_store_error, task::task_store_error, truncate_utf8};

const MAX_WAIT_SECONDS: u64 = 600;
const DEFAULT_WAIT_SECONDS: u64 = 60;
const MAX_WAIT_TASKS: usize = 64;
const MAX_OUTPUT_BYTES: usize = 16 * 1024;

/// The versioned tool catalog declared to every daemon-managed Codex session.
#[allow(clippy::too_many_lines)] // The catalog is one declaration, read top to bottom.
pub(super) fn collaboration_tools() -> Vec<ToolSpec> {
    vec![
        ToolSpec {
            name: "employees_list".to_owned(),
            description: "List the live Lazoo employee roster: id, name, role, provider, \
                          availability, authority mode, and the rosterRevision that \
                          task_delegate must cite."
                .to_owned(),
            input_schema: json!({"type": "object", "properties": {}, "additionalProperties": false}),
        },
        ToolSpec {
            name: "task_delegate".to_owned(),
            description: "Delegate a scoped child task of your current task to another employee \
                          by id. Cite the rosterRevision from employees_list. Under ask authority \
                          the task is proposed and waits for the user; otherwise it is accepted \
                          and dispatched when the worker is free. Authority never widens."
                .to_owned(),
            input_schema: json!({
                "type": "object",
                "required": ["employeeId", "objective", "rosterRevision"],
                "properties": {
                    "employeeId": {"type": "string"},
                    "objective": {"type": "string"},
                    "rosterRevision": {"type": "integer", "minimum": 0},
                    "requestedAuthority": {
                        "type": "string",
                        "enum": ["ask", "self-decision", "full-autonomy"]
                    },
                    "references": references_schema()
                },
                "additionalProperties": false
            }),
        },
        ToolSpec {
            name: "task_wait".to_owned(),
            description: "Wait up to timeoutSeconds (default 60, maximum 600) until every listed \
                          task (default: your current task's unfinished children) reaches a \
                          terminal state. Returns each task's state, failure, and final output. \
                          Returns early with `messages` when a peer message arrives for you; \
                          answer it with message_reply and call task_wait again to keep waiting."
                .to_owned(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "taskIds": {"type": "array", "items": {"type": "string"}},
                    "timeoutSeconds": {"type": "integer", "minimum": 0, "maximum": 600}
                },
                "additionalProperties": false
            }),
        },
        ToolSpec {
            name: "task_report".to_owned(),
            description: "Report structured progress, completion, or failure for your current \
                          task with optional evidence. Completion requires no unfinished child \
                          tasks."
                .to_owned(),
            input_schema: json!({
                "type": "object",
                "required": ["kind"],
                "properties": {
                    "kind": {"type": "string", "enum": ["progress", "completed", "failed"]},
                    "evidence": {"type": "object"},
                    "failureCode": {"type": "string"},
                    "failureMessage": {"type": "string"}
                },
                "additionalProperties": false
            }),
        },
        ToolSpec {
            name: "incident_report".to_owned(),
            description: "Record an operational problem you cannot resolve inside your task,                           with a short title and an explanation. The user sees it as an                           incident; it never changes authority or approves anything."
                .to_owned(),
            input_schema: json!({
                "type": "object",
                "required": ["title"],
                "properties": {
                    "title": {"type": "string"},
                    "explanation": {"type": "string"},
                    "detail": {"type": "object"}
                },
                "additionalProperties": false
            }),
        },
        ToolSpec {
            name: "incident_list".to_owned(),
            description: "List open and acknowledged incidents visible to you: those in your                           project and those about you, with the recommended next action."
                .to_owned(),
            input_schema: json!({"type": "object", "properties": {}, "additionalProperties": false}),
        },
        ToolSpec {
            name: "incident_remedy".to_owned(),
            description: "Apply one remedy to a priority incident about a child task you \
                          delegated, inside its remediation window (named in the incident \
                          notice and incident_list). At most two remedies per incident, within \
                          five minutes of its promotion; a refused call uses none. cancel-task \
                          cancels the affected child. redelegate-task delegates a failed or \
                          cancelled child's objective again, to employeeId (default: the same \
                          employee), citing rosterRevision from employees_list, under the \
                          ordinary delegation guards. If the failure keeps recurring, Lazoo \
                          escalates the incident to the user; a remedy never resolves an \
                          incident, approves anything, or widens authority."
                .to_owned(),
            input_schema: json!({
                "type": "object",
                "required": ["incidentId", "remedy"],
                "properties": {
                    "incidentId": {"type": "string"},
                    "remedy": {"type": "string", "enum": ["cancel-task", "redelegate-task"]},
                    "employeeId": {"type": "string"},
                    "rosterRevision": {"type": "integer", "minimum": 0},
                    "requestedAuthority": {
                        "type": "string",
                        "enum": ["ask", "self-decision", "full-autonomy"]
                    },
                    "note": {"type": "string"}
                },
                "additionalProperties": false
            }),
        },
        ToolSpec {
            name: "message_send".to_owned(),
            description: "Send a peer message to another employee working in your task tree \
                          (your lead or a sibling), by employee id. It reaches them between \
                          turns or inside their task_wait; set waitForReplySeconds (maximum \
                          600) to block for the reply, which is returned as `reply`. A peer \
                          message is not an approval and never changes task ownership, \
                          authority, or permissions."
                .to_owned(),
            input_schema: json!({
                "type": "object",
                "required": ["employeeId", "text"],
                "properties": {
                    "employeeId": {"type": "string"},
                    "text": {"type": "string"},
                    "waitForReplySeconds": {"type": "integer", "minimum": 0, "maximum": 600},
                    "references": references_schema()
                },
                "additionalProperties": false
            }),
        },
        ToolSpec {
            name: "message_reply".to_owned(),
            description: "Reply to a peer message you received, citing its message id. The \
                          reply returns to the sender's task and threads under that message."
                .to_owned(),
            input_schema: json!({
                "type": "object",
                "required": ["messageId", "text"],
                "properties": {
                    "messageId": {"type": "string"},
                    "text": {"type": "string"},
                    "references": references_schema()
                },
                "additionalProperties": false
            }),
        },
    ]
}

fn references_schema() -> Value {
    json!({
        "type": "array",
        "items": {
            "type": "object",
            "required": ["kind", "target"],
            "properties": {
                "kind": {"type": "string"},
                "target": {"type": "string"},
                "metadata": {"type": "object"}
            }
        }
    })
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct DelegateArguments {
    employee_id: Uuid,
    objective: String,
    roster_revision: u64,
    requested_authority: Option<AuthorityMode>,
    #[serde(default)]
    references: Vec<MessageReference>,
}

#[derive(Default, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct WaitArguments {
    #[serde(default)]
    task_ids: Vec<Uuid>,
    timeout_seconds: Option<u64>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct ReportArguments {
    kind: TaskReportKind,
    #[serde(default = "empty_object")]
    evidence: Value,
    failure_code: Option<String>,
    failure_message: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct SendArguments {
    employee_id: Uuid,
    text: String,
    wait_for_reply_seconds: Option<u64>,
    #[serde(default)]
    references: Vec<MessageReference>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct ReplyArguments {
    message_id: Uuid,
    text: String,
    #[serde(default)]
    references: Vec<MessageReference>,
}

impl App {
    /// Answers one collaboration tool call for a daemon-owned turn. Every error becomes a
    /// `success = false` result so the provider turn continues; nothing here can approve,
    /// widen, or bypass the store's policy guards.
    pub(super) async fn call_collaboration_tool(
        &self,
        turn_id: Uuid,
        call: &ToolCall,
    ) -> ToolResult {
        match self.dispatch_collaboration_tool(turn_id, call).await {
            Ok(content) => ToolResult::success(content),
            Err(error) => ToolResult {
                success: false,
                content: json!({
                    "error": {"code": error.code, "message": error.message, "data": error.data}
                }),
            },
        }
    }

    async fn dispatch_collaboration_tool(
        &self,
        turn_id: Uuid,
        call: &ToolCall,
    ) -> Result<Value, RpcError> {
        if call.call_id.is_empty() || call.call_id.len() > 128 {
            return Err(tool_error(
                "LZ-TOOL-INVALID",
                "tool call id is missing or too long",
            ));
        }
        let turn = self.store.get_turn(turn_id).map_err(task_store_error)?;
        let actor = turn.recipient;
        let task_id = turn.task_id.ok_or_else(|| {
            tool_error(
                "LZ-TOOL-NO-TASK",
                "the current turn is not linked to a task",
            )
        });
        match call.name.as_str() {
            "employees_list" => self.tool_employees_list(actor),
            "task_delegate" => self.tool_task_delegate(turn_id, call, task_id?, actor),
            "task_wait" => self.tool_task_wait(turn_id, call, task_id?, actor).await,
            "task_report" => self.tool_task_report(turn_id, call, task_id?, actor),
            "incident_report" => self.tool_incident_report(call, &turn, task_id?),
            "incident_list" => self.tool_incident_list(&turn),
            "incident_remedy" => self.tool_incident_remedy(turn_id, call).await,
            "message_send" => self.tool_message_send(turn_id, call, task_id?, actor).await,
            "message_reply" => self.tool_message_reply(turn_id, call),
            other => Err(tool_error(
                "LZ-TOOL-UNKNOWN",
                format!("unknown Lazoo tool: {other}"),
            )),
        }
    }

    fn tool_employees_list(&self, actor: MessageEndpoint) -> Result<Value, RpcError> {
        let roster = self.store.roster_snapshot().map_err(task_store_error)?;
        let employees: Vec<Value> = roster
            .entries
            .iter()
            .map(|entry| {
                json!({
                    "id": entry.employee.id,
                    "name": entry.employee.name,
                    "role": entry.employee.role,
                    "provider": entry.employee.provider,
                    "availability": entry.availability,
                    "availabilityReason": entry.availability_reason,
                    "authorityMode": entry.employee.authority_mode,
                    "self": entry.employee.id == actor.employee_id,
                })
            })
            .collect();
        Ok(json!({"rosterRevision": roster.revision, "employees": employees}))
    }

    fn tool_task_delegate(
        &self,
        turn_id: Uuid,
        call: &ToolCall,
        parent_task_id: Uuid,
        actor: MessageEndpoint,
    ) -> Result<Value, RpcError> {
        let arguments: DelegateArguments = tool_arguments(call)?;
        let key = tool_command_key(turn_id, call);
        let result = self
            .store
            .delegate_task(&DelegateTaskInput {
                request_id: key.clone(),
                idempotency_key: key,
                parent_task_id,
                actor,
                assigned_employee_id: arguments.employee_id,
                objective: arguments.objective,
                references: arguments.references,
                requested_authority: arguments.requested_authority,
                roster_revision: arguments.roster_revision,
            })
            .map_err(task_store_error)?;
        self.publish_events()?;
        if result.task.state == TaskState::Accepted {
            self.task_dispatch_notify.notify_one();
        }
        let note = if result.task.state == TaskState::Proposed {
            "the task waits for the local user to accept it before any work starts"
        } else {
            "the task is accepted and will be dispatched when the assigned employee is free"
        };
        Ok(json!({
            "taskId": result.task.id,
            "state": result.task.state,
            "effectiveAuthority": result.task.effective_authority,
            "revision": result.task.revision,
            "note": note,
        }))
    }

    async fn tool_task_wait(
        &self,
        turn_id: Uuid,
        call: &ToolCall,
        task_id: Uuid,
        actor: MessageEndpoint,
    ) -> Result<Value, RpcError> {
        let arguments: WaitArguments = tool_arguments(call)?;
        let caller = self.store.get_task(task_id).map_err(task_store_error)?.task;
        let timeout = Duration::from_secs(
            arguments
                .timeout_seconds
                .unwrap_or(DEFAULT_WAIT_SECONDS)
                .min(MAX_WAIT_SECONDS),
        );
        let mut task_ids = arguments.task_ids;
        if task_ids.is_empty() {
            task_ids = self
                .store
                .list_tasks(&ListTasksInput {
                    after_sequence: 0,
                    limit: 64,
                    root_task_id: None,
                    parent_task_id: Some(task_id),
                    conversation_id: None,
                    project_id: None,
                    assigned_employee_id: None,
                    state: None,
                })
                .map_err(task_store_error)?
                .into_iter()
                .filter(|task| !task_is_terminal(task.state))
                .map(|task| task.id)
                .collect();
        }
        if task_ids.len() > MAX_WAIT_TASKS {
            return Err(tool_error(
                "LZ-TOOL-INVALID",
                "task_wait accepts at most 64 task ids",
            ));
        }
        let deadline = Instant::now() + timeout;
        loop {
            // Registered before the reads, so a write landing between them still wakes the wait.
            let changed = self.store_changed.notified();
            let mut details = Vec::with_capacity(task_ids.len());
            for id in &task_ids {
                let detail = self.store.get_task(*id).map_err(task_store_error)?;
                if detail.task.root_task_id != caller.root_task_id {
                    return Err(tool_error(
                        "LZ-TOOL-SCOPE",
                        format!("task {id} is outside the current task tree"),
                    ));
                }
                details.push(detail);
            }
            let all_terminal = details
                .iter()
                .all(|detail| task_is_terminal(detail.task.state));
            // A peer message for the waiting employee ends the wait early: the lead answers
            // it and calls task_wait again, instead of the sender blocking on a busy lead.
            let inbox = self
                .store
                .pending_peer_messages(actor.employee_id, task_id)
                .map_err(peer_store_error)?;
            let now = Instant::now();
            if all_terminal || !inbox.is_empty() || now >= deadline {
                // The lead is reading these outcomes now, so their result notices are delivered
                // here rather than as a later notice turn.
                for detail in details
                    .iter()
                    .filter(|detail| task_is_terminal(detail.task.state))
                    .filter(|detail| detail.task.parent_task_id == Some(task_id))
                {
                    let consumed = self.store.consume_task_result(&ConsumeTaskResultInput {
                        request_id: format!(
                            "{}:consume:{}",
                            tool_command_key(turn_id, call),
                            detail.task.id
                        ),
                        child_task_id: detail.task.id,
                        turn_id,
                    });
                    if let Err(error) = consumed {
                        tracing::debug!(task_id = %detail.task.id, %error, "child result was not consumed");
                    }
                }
                let messages = self.consume_inbox(turn_id, call, &inbox, "task_wait");
                self.publish_events()?;
                let tasks: Vec<Value> = details
                    .iter()
                    .map(|detail| self.task_outcome(detail))
                    .collect();
                return Ok(json!({
                    "timedOut": !all_terminal && messages.is_empty(),
                    "tasks": tasks,
                    "messages": messages,
                }));
            }
            self.ensure_turn_still_waiting(turn_id)?;
            tokio::select! {
                () = changed => {}
                () = sleep(deadline - now) => {}
            }
        }
    }

    /// A wait ends with an error once its calling turn was cancelled or finished.
    fn ensure_turn_still_waiting(&self, turn_id: Uuid) -> Result<(), RpcError> {
        let turn = self.store.get_turn(turn_id).map_err(task_store_error)?;
        if turn.cancel_requested_at.is_some()
            || !matches!(turn.state, TurnState::Submitting | TurnState::Accepted)
        {
            return Err(tool_error(
                "LZ-TOOL-TURN-CANCELLED",
                "the calling turn ended while waiting",
            ));
        }
        Ok(())
    }

    fn task_outcome(&self, detail: &TaskDetail) -> Value {
        let output = detail
            .dispatch
            .as_ref()
            .and_then(|dispatch| dispatch.turn_id)
            .and_then(|turn_id| self.store.get_turn(turn_id).ok())
            .and_then(|turn| {
                turn.events.iter().rev().find_map(|event| {
                    event
                        .payload
                        .get("output")
                        .and_then(Value::as_str)
                        .map(str::to_owned)
                })
            })
            .map(|mut output| {
                truncate_utf8(&mut output, MAX_OUTPUT_BYTES);
                output
            });
        json!({
            "taskId": detail.task.id,
            "state": detail.task.state,
            "revision": detail.task.revision,
            "assignedEmployeeId": detail.task.assigned_employee_id,
            "objective": detail.task.objective,
            "failureCode": detail.task.failure_code,
            "failureMessage": detail.task.failure_message,
            "output": output,
        })
    }

    fn tool_task_report(
        &self,
        turn_id: Uuid,
        call: &ToolCall,
        task_id: Uuid,
        actor: MessageEndpoint,
    ) -> Result<Value, RpcError> {
        let arguments: ReportArguments = tool_arguments(call)?;
        let current = self.store.get_task(task_id).map_err(task_store_error)?.task;
        let (failure_code, failure_message) = if arguments.kind == TaskReportKind::Failed {
            (
                Some(
                    arguments
                        .failure_code
                        .unwrap_or_else(|| "LZ-TASK-REPORTED-FAILURE".to_owned()),
                ),
                Some(
                    arguments
                        .failure_message
                        .unwrap_or_else(|| "the employee reported failure".to_owned()),
                ),
            )
        } else {
            (arguments.failure_code, arguments.failure_message)
        };
        let key = tool_command_key(turn_id, call);
        let result = self
            .store
            .report_task(&ReportTaskInput {
                request_id: key.clone(),
                idempotency_key: key,
                task_id,
                expected_revision: current.revision,
                actor,
                kind: arguments.kind,
                evidence: arguments.evidence,
                failure_code,
                failure_message,
            })
            .map_err(task_store_error)?;
        self.publish_events()?;
        Ok(json!({
            "taskId": result.task.id,
            "state": result.task.state,
            "revision": result.task.revision,
        }))
    }
}

impl App {
    /// Sends a peer message and, when asked, waits for its reply. Whatever else arrived for
    /// the caller meanwhile is read here too, so nothing is delivered twice.
    async fn tool_message_send(
        &self,
        turn_id: Uuid,
        call: &ToolCall,
        task_id: Uuid,
        actor: MessageEndpoint,
    ) -> Result<Value, RpcError> {
        let arguments: SendArguments = tool_arguments(call)?;
        let key = tool_command_key(turn_id, call);
        let sent = self
            .store
            .send_peer_message(&SendPeerMessageInput {
                request_id: key.clone(),
                idempotency_key: key,
                turn_id,
                recipient_employee_id: arguments.employee_id,
                content: arguments.text,
                references: arguments.references,
            })
            .map_err(peer_store_error)?;
        self.publish_events()?;
        let message_id = sent.message.envelope.id;
        let wait = Duration::from_secs(
            arguments
                .wait_for_reply_seconds
                .unwrap_or(0)
                .min(MAX_WAIT_SECONDS),
        );
        let deadline = Instant::now() + wait;
        loop {
            let changed = self.store_changed.notified();
            let current = self
                .store
                .get_message(message_id)
                .map_err(peer_store_error)?;
            let inbox = self
                .store
                .pending_peer_messages(actor.employee_id, task_id)
                .map_err(peer_store_error)?;
            let replied = inbox
                .iter()
                .any(|message| message.envelope.causation_id == Some(message_id));
            // The recipient's task ended before it read the message; nobody will answer.
            let closed = matches!(
                current.delivery.state,
                MessageDeliveryState::Cancelled | MessageDeliveryState::TerminalFailed
            );
            let now = Instant::now();
            if replied || closed || now >= deadline {
                let messages = self.consume_inbox(turn_id, call, &inbox, "message_send");
                self.publish_events()?;
                let reply = messages
                    .iter()
                    .find(|message| message["replyToMessageId"] == json!(message_id))
                    .cloned();
                return Ok(json!({
                    "messageId": message_id,
                    "state": current.delivery.state,
                    "recipientEmployeeId": current.envelope.recipient.employee_id,
                    "recipientTaskId": current.envelope.task_id,
                    "timedOut": !wait.is_zero() && reply.is_none() && !closed,
                    "reply": reply,
                    "messages": messages,
                }));
            }
            self.ensure_turn_still_waiting(turn_id)?;
            tokio::select! {
                () = changed => {}
                () = sleep(deadline - now) => {}
            }
        }
    }

    fn tool_message_reply(&self, turn_id: Uuid, call: &ToolCall) -> Result<Value, RpcError> {
        let arguments: ReplyArguments = tool_arguments(call)?;
        let key = tool_command_key(turn_id, call);
        let sent = self
            .store
            .reply_peer_message(&ReplyPeerMessageInput {
                request_id: key.clone(),
                idempotency_key: key,
                turn_id,
                message_id: arguments.message_id,
                content: arguments.text,
                references: arguments.references,
            })
            .map_err(peer_store_error)?;
        self.publish_events()?;
        Ok(json!({
            "messageId": sent.message.envelope.id,
            "state": sent.message.delivery.state,
            "recipientEmployeeId": sent.message.envelope.recipient.employee_id,
            "recipientTaskId": sent.message.envelope.task_id,
            "replyToMessageId": arguments.message_id,
        }))
    }

    /// Marks queued peer messages as read inside the calling turn and renders them for the
    /// model. A message that cannot be consumed (already read elsewhere) is left out.
    fn consume_inbox(
        &self,
        turn_id: Uuid,
        call: &ToolCall,
        inbox: &[MessageRecord],
        via: &'static str,
    ) -> Vec<Value> {
        inbox
            .iter()
            .filter_map(|message| {
                let consumed = self.store.consume_peer_message(&ConsumePeerMessageInput {
                    request_id: format!(
                        "{}:inbox:{}",
                        tool_command_key(turn_id, call),
                        message.envelope.id
                    ),
                    message_id: message.envelope.id,
                    turn_id,
                    via,
                });
                match consumed {
                    Ok(_) => Some(peer_message_value(message)),
                    Err(error) => {
                        tracing::debug!(message_id = %message.envelope.id, %error, "peer message was not consumed");
                        None
                    }
                }
            })
            .collect()
    }
}

fn peer_message_value(message: &MessageRecord) -> Value {
    let body = &message.envelope.body;
    json!({
        "messageId": message.envelope.id,
        "kind": message.envelope.kind,
        "fromEmployeeId": body["senderEmployeeId"],
        "fromEmployeeName": body["senderEmployeeName"],
        "senderTaskId": body["senderTaskId"],
        "replyToMessageId": body["replyToMessageId"],
        "text": body["content"],
        "createdAt": message.envelope.created_at,
    })
}

#[allow(clippy::needless_pass_by_value)]
fn peer_store_error(error: StoreError) -> RpcError {
    let code = match &error {
        StoreError::PeerRecipientSelf { .. }
        | StoreError::MessageValidation(_)
        | StoreError::ConversationValidation(_) => "LZ-TOOL-INVALID",
        StoreError::PeerRecipientTaskNotFound { .. }
        | StoreError::PeerMessageScope { .. }
        | StoreError::MessageNotFound { .. }
        | StoreError::MessageEmployeeNotFound { .. }
        | StoreError::MessageEmployeeDisabled { .. } => "LZ-TOOL-SCOPE",
        _ => return task_store_error(error),
    };
    tool_error(code, error.to_string())
}

fn tool_arguments<T: serde::de::DeserializeOwned>(call: &ToolCall) -> Result<T, RpcError> {
    let value = if call.arguments.is_null() {
        json!({})
    } else {
        call.arguments.clone()
    };
    serde_json::from_value(value).map_err(|error| tool_error("LZ-TOOL-INVALID", error.to_string()))
}

fn tool_command_key(turn_id: Uuid, call: &ToolCall) -> String {
    format!("tool:{turn_id}:{}", call.call_id)
}

const fn task_is_terminal(state: TaskState) -> bool {
    matches!(
        state,
        TaskState::Completed | TaskState::Failed | TaskState::Cancelled | TaskState::Uncertain
    )
}

fn empty_object() -> Value {
    Value::Object(serde_json::Map::new())
}

fn tool_error(code: &str, message: impl Into<String>) -> RpcError {
    RpcError {
        code: code.to_owned(),
        message: message.into(),
        data: None,
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct ReportIncidentArguments {
    title: String,
    #[serde(default)]
    explanation: String,
    #[serde(default = "empty_object")]
    detail: Value,
}

impl App {
    fn tool_incident_report(
        &self,
        call: &ToolCall,
        turn: &ConversationTurnRecord,
        task_id: Uuid,
    ) -> Result<Value, RpcError> {
        let arguments: ReportIncidentArguments = tool_arguments(call)?;
        let key = tool_command_key(turn.id, call);
        let scope = self.turn_scope(turn);
        let result = self
            .store
            .raise_incident(&RaiseIncidentInput {
                request_id: key.clone(),
                idempotency_key: key,
                code: "LZ-EMPLOYEE-REPORTED".to_owned(),
                source: IncidentSource::Employee,
                title: Some(arguments.title),
                explanation: Some(arguments.explanation),
                project_id: scope.project_id,
                employee_id: Some(turn.recipient.employee_id),
                provider: turn.provider.clone(),
                task_id: Some(task_id),
                session_id: turn.recipient.session_id,
                turn_id: Some(turn.id),
                message_id: None,
                detail: arguments.detail,
                occurred_at: None,
            })
            .map_err(incident_store_error)?;
        self.publish_events()?;
        Ok(json!({
            "incidentId": result.incident.id,
            "state": result.incident.state,
            "priority": result.incident.priority,
            "occurrences": result.incident.occurrences,
            "revision": result.incident.revision,
        }))
    }

    fn tool_incident_list(&self, turn: &ConversationTurnRecord) -> Result<Value, RpcError> {
        let project_id = self.turn_scope(turn).project_id;
        let mut incidents = Vec::new();
        for state in [IncidentState::Open, IncidentState::Acknowledged] {
            incidents.extend(
                self.store
                    .list_incidents(&ListIncidentsInput {
                        after_sequence: 0,
                        limit: 128,
                        state: Some(state),
                        ..ListIncidentsInput::default()
                    })
                    .map_err(incident_store_error)?,
            );
        }
        incidents.sort_by_key(|incident| incident.sequence);
        let visible: Vec<Value> = incidents
            .into_iter()
            .filter(|incident| {
                incident.project_id == project_id
                    || incident.employee_id == Some(turn.recipient.employee_id)
            })
            .take(50)
            .map(|incident| {
                json!({
                    "id": incident.id,
                    "code": incident.code,
                    "title": incident.title,
                    "explanation": incident.explanation,
                    "nextAction": incident.next_action,
                    "priority": incident.priority,
                    "state": incident.state,
                    "occurrences": incident.occurrences,
                    "employeeId": incident.employee_id,
                    "taskId": incident.task_id,
                    "lastOccurredAt": incident.last_occurred_at,
                    "escalation": incident.escalation,
                    "remediationDeadlineAt": incident.remediation_deadline_at,
                    "remediesApplied": incident.remedies.len(),
                })
            })
            .collect();
        Ok(json!({"incidents": visible}))
    }

    /// Applies one bounded remedy for the turn's employee. The store fixes the actor to the
    /// turn's recipient and checks the window, the attempt bound, and the task-tree scope; a
    /// cancellation that lands on an in-flight dispatch turn asks the provider to cancel it,
    /// exactly as the user's `task.cancel` does.
    async fn tool_incident_remedy(
        &self,
        turn_id: Uuid,
        call: &ToolCall,
    ) -> Result<Value, RpcError> {
        let arguments: RemedyArguments = tool_arguments(call)?;
        let action = match arguments.remedy {
            IncidentRemedyKind::CancelTask => {
                if arguments.employee_id.is_some()
                    || arguments.roster_revision.is_some()
                    || arguments.requested_authority.is_some()
                {
                    return Err(tool_error(
                        "LZ-TOOL-INVALID",
                        "employeeId, rosterRevision, and requestedAuthority apply only to \
                         redelegate-task",
                    ));
                }
                IncidentRemedyAction::CancelTask
            }
            IncidentRemedyKind::RedelegateTask => IncidentRemedyAction::RedelegateTask {
                assigned_employee_id: arguments.employee_id,
                roster_revision: arguments.roster_revision.ok_or_else(|| {
                    tool_error(
                        "LZ-TOOL-INVALID",
                        "redelegate-task requires the rosterRevision from employees_list",
                    )
                })?,
                requested_authority: arguments.requested_authority,
            },
        };
        let key = tool_command_key(turn_id, call);
        let applied = self
            .store
            .apply_incident_remedy(&ApplyIncidentRemedyInput {
                request_id: key.clone(),
                idempotency_key: key.clone(),
                incident_id: arguments.incident_id,
                turn_id,
                action,
                note: arguments.note,
            })
            .map_err(remedy_store_error)?;
        self.publish_events()?;
        let mut task = applied.task;
        if applied.event_sequence.is_some() {
            match task.state {
                TaskState::Accepted => self.task_dispatch_notify.notify_one(),
                TaskState::CancelRequested => task = self.cancel_dispatch_turn(&key, task).await?,
                _ => {}
            }
        }
        let note = match (applied.remedy.kind, task.state) {
            (IncidentRemedyKind::RedelegateTask, TaskState::Proposed) => {
                "the new task waits for the local user to accept it before any work starts"
            }
            (IncidentRemedyKind::RedelegateTask, _) => {
                "the new task is accepted and will be dispatched when the assigned employee is free"
            }
            (IncidentRemedyKind::CancelTask, TaskState::Cancelled) => "the child task is cancelled",
            (IncidentRemedyKind::CancelTask, _) => {
                "cancellation was requested; the child stops when its provider confirms it"
            }
        };
        Ok(json!({
            "incidentId": applied.incident.id,
            "remedy": applied.remedy.kind,
            "attempt": applied.remedy.attempt,
            "remediesRemaining": MAX_INCIDENT_REMEDIES.saturating_sub(applied.remedy.attempt),
            "remediationDeadlineAt": applied.incident.remediation_deadline_at,
            "taskId": task.id,
            "state": task.state,
            "revision": task.revision,
            "note": note,
        }))
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct RemedyArguments {
    incident_id: Uuid,
    remedy: IncidentRemedyKind,
    employee_id: Option<Uuid>,
    roster_revision: Option<u64>,
    requested_authority: Option<AuthorityMode>,
    #[serde(default)]
    note: String,
}

#[allow(clippy::needless_pass_by_value)]
fn remedy_store_error(error: StoreError) -> RpcError {
    let code = match &error {
        StoreError::IncidentRemedyClosed { .. } => "LZ-INCIDENT-REMEDY-CLOSED",
        StoreError::IncidentRemedyScope { .. } => "LZ-INCIDENT-REMEDY-SCOPE",
        StoreError::IncidentRemedyTarget { .. } => "LZ-INCIDENT-REMEDY-TARGET",
        StoreError::IncidentNotFound { .. }
        | StoreError::IncidentIdempotencyConflict
        | StoreError::IncidentValidation(_) => return incident_store_error(error),
        _ => return task_store_error(error),
    };
    tool_error(code, error.to_string())
}
