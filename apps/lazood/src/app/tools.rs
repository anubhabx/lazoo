//! Lazoo collaboration tools for daemon-managed Codex sessions.
//!
//! The daemon declares these as provider dynamic tools; every call acts as the turn's recipient
//! employee endpoint, so the existing store guards (actor session, authority narrowing, roster
//! revision, self-delegation, depth, fan-out, active descendants) fail closed exactly as they do
//! for the CLI. A refused or failed call answers the model with `success = false` and never ends
//! the provider turn.

use lazoo_protocol::{
    AuthorityMode, ConversationTurnRecord, IncidentSource, IncidentState, MessageEndpoint,
    MessageReference, RpcError, TaskDetail, TaskReportKind, TaskState, TurnState,
};
use lazoo_providers::{ToolCall, ToolResult, ToolSpec};
use lazoo_store::{
    DelegateTaskInput, ListIncidentsInput, ListTasksInput, RaiseIncidentInput, ReportTaskInput,
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
// ponytail: task_wait polls the store; switch to a task-change notify if leads wait at scale.
const WAIT_POLL_INTERVAL: Duration = Duration::from_millis(250);

/// The versioned tool catalog declared to every daemon-managed Codex session.
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
                    "references": {
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
                    }
                },
                "additionalProperties": false
            }),
        },
        ToolSpec {
            name: "task_wait".to_owned(),
            description: "Wait up to timeoutSeconds (default 60, maximum 600) until every listed \
                          task (default: your current task's unfinished children) reaches a \
                          terminal state. Returns each task's state, failure, and final output."
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
    ]
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
            "task_wait" => self.tool_task_wait(turn_id, call, task_id?).await,
            "task_report" => self.tool_task_report(turn_id, call, task_id?, actor),
            "incident_report" => self.tool_incident_report(call, &turn, task_id?),
            "incident_list" => self.tool_incident_list(&turn),
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
            let now = Instant::now();
            if all_terminal || now >= deadline {
                let tasks: Vec<Value> = details
                    .iter()
                    .map(|detail| self.task_outcome(detail))
                    .collect();
                return Ok(json!({"timedOut": !all_terminal, "tasks": tasks}));
            }
            let turn = self.store.get_turn(turn_id).map_err(task_store_error)?;
            if turn.cancel_requested_at.is_some()
                || !matches!(turn.state, TurnState::Submitting | TurnState::Accepted)
            {
                return Err(tool_error(
                    "LZ-TOOL-TURN-CANCELLED",
                    "the calling turn ended while waiting",
                ));
            }
            sleep(WAIT_POLL_INTERVAL.min(deadline - now)).await;
        }
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
                })
            })
            .collect();
        Ok(json!({"incidents": visible}))
    }
}
