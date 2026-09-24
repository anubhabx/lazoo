//! Lead notices: durable `result` and `status` messages that carry a child task's outcome or a
//! promoted incident to the lead that owns the task tree, and the turn that delivers one.
//!
//! A notice is derived evidence. It is queued idempotently after the task or incident commit,
//! keyed by the child task or incident id, so a second daemon boot never duplicates it. The
//! failure, task, and incident rows it points at stay authoritative.

use chrono::Utc;
use lazoo_domain::{
    IncidentNotice, MAX_NOTICE_OUTPUT_BYTES, TaskResultNotice, render_incident_notice_text,
    render_task_result_text, validate_turn_text,
};
use lazoo_protocol::{
    CONVERSATION_CONTRACT_V1, IncidentPriority, MESSAGE_ENVELOPE_V1, MessageDeliveryState,
    MessageEndpoint, MessageKind, MessageMutationResult, MessageRecord, MessageReference,
    MessageSender, TaskState, TurnMutationResult,
};
use rusqlite::{Connection, Transaction, TransactionBehavior, params};
use serde_json::{Value, json};
use uuid::Uuid;

use crate::{Store, StoreError, conversation, employee, incident, message, session, task};

const TASK_RESULT_SCOPE: &str = "task.result";
const INCIDENT_NOTICE_SCOPE: &str = "incident.notice";
const NOTICE_MAX_ATTEMPTS: u32 = 3;

#[derive(Clone, Debug)]
pub struct QueueTaskResultInput {
    pub request_id: String,
    pub child_task_id: Uuid,
}

#[derive(Clone, Debug)]
pub struct QueueIncidentNoticeInput {
    pub request_id: String,
    pub incident_id: Uuid,
}

#[derive(Clone, Debug)]
pub struct RecordNoticeTurnInput {
    pub request_id: String,
    pub message_id: Uuid,
}

#[derive(Clone, Debug)]
pub struct ConsumeTaskResultInput {
    pub request_id: String,
    pub child_task_id: Uuid,
    pub turn_id: Uuid,
}

impl Store {
    /// Queues one `result` message from a terminal child task to the employee that owns its
    /// parent task. Replays by child id return the existing message without writing.
    pub fn queue_task_result(
        &self,
        input: &QueueTaskResultInput,
    ) -> Result<MessageMutationResult, StoreError> {
        let mut connection = self.lock()?;
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let result = queue_task_result_tx(&transaction, &input.request_id, input.child_task_id)?;
        transaction.commit()?;
        Ok(result)
    }

    /// Terminal child tasks whose result has not been queued yet, oldest first.
    pub fn pending_task_results(&self, limit: u32) -> Result<Vec<Uuid>, StoreError> {
        let connection = self.lock()?;
        let mut statement = connection.prepare(
            "SELECT t.id FROM tasks t
             WHERE t.parent_task_id IS NOT NULL
               AND t.state IN ('completed', 'failed', 'cancelled', 'uncertain')
               AND NOT EXISTS (
                   SELECT 1 FROM message_commands mc
                   WHERE mc.scope = ?1 AND mc.idempotency_key = 'task-result:' || t.id
               )
             ORDER BY t.sequence LIMIT ?2",
        )?;
        let rows = statement.query_map(params![TASK_RESULT_SCOPE, i64::from(limit)], |row| {
            row.get::<_, String>(0)
        })?;
        rows.map(|row| parse_uuid(&row?, "tasks.id")).collect()
    }

    /// Queues one `status` message about a priority incident to the lead of the task tree it
    /// belongs to. Incidents about the lead itself, about the root task, without a task, or
    /// below priority are not applicable.
    pub fn queue_incident_notice(
        &self,
        input: &QueueIncidentNoticeInput,
    ) -> Result<MessageMutationResult, StoreError> {
        let mut connection = self.lock()?;
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let result = queue_incident_notice_tx(&transaction, &input.request_id, input.incident_id)?;
        transaction.commit()?;
        Ok(result)
    }

    /// Unresolved priority incidents on an active task tree whose lead has not been notified.
    pub fn pending_incident_notices(&self, limit: u32) -> Result<Vec<Uuid>, StoreError> {
        let connection = self.lock()?;
        let mut statement = connection.prepare(
            "SELECT i.id FROM incidents i
             JOIN tasks t ON t.id = i.task_id
             JOIN tasks r ON r.id = t.root_task_id
             WHERE i.priority = 'priority'
               AND i.state != 'resolved'
               AND t.id != r.id
               AND r.state IN ('accepted', 'running')
               AND (i.employee_id IS NULL OR i.employee_id != r.assigned_employee_id)
               AND NOT EXISTS (
                   SELECT 1 FROM message_commands mc
                   WHERE mc.scope = ?1 AND mc.idempotency_key = 'incident-notice:' || i.id
               )
             ORDER BY i.sequence LIMIT ?2",
        )?;
        let rows = statement
            .query_map(params![INCIDENT_NOTICE_SCOPE, i64::from(limit)], |row| {
                row.get::<_, String>(0)
            })?;
        rows.map(|row| parse_uuid(&row?, "incidents.id")).collect()
    }

    /// Queued lead notices, oldest first. Only messages this module queued are returned; peer
    /// messages sent through `message.send` keep their manual claim and acknowledge path.
    pub fn pending_lead_notices(&self, limit: u32) -> Result<Vec<MessageRecord>, StoreError> {
        let connection = self.lock()?;
        let mut statement = connection.prepare(
            "SELECT m.id FROM messages m
             JOIN message_deliveries d ON d.message_id = m.id
             JOIN message_commands mc ON mc.message_id = m.id AND mc.scope IN (?1, ?2)
             WHERE d.state = 'queued'
             ORDER BY m.sequence LIMIT ?3",
        )?;
        let ids: Vec<String> = statement
            .query_map(
                params![TASK_RESULT_SCOPE, INCIDENT_NOTICE_SCOPE, i64::from(limit)],
                |row| row.get::<_, String>(0),
            )?
            .collect::<Result<_, _>>()?;
        ids.iter()
            .map(|id| message::required_message(&connection, parse_uuid(id, "messages.id")?))
            .collect()
    }

    /// Records the conversation turn that delivers a queued notice to the lead, reusing the
    /// notice as the turn's canonical message. Replays by message id return the same turn.
    /// Fails closed while the lead's task has any other unfinished turn.
    pub fn record_notice_turn(
        &self,
        input: &RecordNoticeTurnInput,
    ) -> Result<TurnMutationResult, StoreError> {
        let mut connection = self.lock()?;
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let result = record_notice_turn_tx(&transaction, input)?;
        transaction.commit()?;
        Ok(result)
    }

    /// Marks a child's result as delivered through the lead's own `task_wait` call: the notice
    /// is queued if missing, then claimed and acknowledged with the waiting turn as evidence.
    pub fn consume_task_result(
        &self,
        input: &ConsumeTaskResultInput,
    ) -> Result<MessageMutationResult, StoreError> {
        let mut connection = self.lock()?;
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let result = consume_task_result_tx(&transaction, input)?;
        transaction.commit()?;
        Ok(result)
    }
}

fn task_result_key(child_task_id: Uuid) -> String {
    format!("task-result:{child_task_id}")
}

fn incident_notice_key(incident_id: Uuid) -> String {
    format!("incident-notice:{incident_id}")
}

fn notice_turn_key(message_id: Uuid) -> String {
    format!("notice-turn:{message_id}")
}

const fn task_is_terminal(state: TaskState) -> bool {
    matches!(
        state,
        TaskState::Completed | TaskState::Failed | TaskState::Cancelled | TaskState::Uncertain
    )
}

#[allow(clippy::too_many_lines)] // Keep the lookups and the record-first message insert together.
fn queue_task_result_tx(
    transaction: &Transaction<'_>,
    request_id: &str,
    child_task_id: Uuid,
) -> Result<MessageMutationResult, StoreError> {
    let key = task_result_key(child_task_id);
    if let Some(message) = message::message_for_command(transaction, TASK_RESULT_SCOPE, &key)? {
        return Ok(MessageMutationResult {
            message,
            event_sequence: None,
        });
    }
    let child = task::required_task(transaction, child_task_id)?;
    let parent_id = child
        .parent_task_id
        .ok_or(StoreError::TaskResultWithoutParent {
            task_id: child_task_id,
        })?;
    if !task_is_terminal(child.state) {
        return Err(StoreError::TaskState {
            task_id: child.id,
            state: serialized_name(child.state)?,
            expected: "terminal",
        });
    }
    let parent = task::required_task(transaction, parent_id)?;
    let child_employee = employee::query_employee(transaction, child.assigned_employee_id)?.ok_or(
        StoreError::EmployeeNotFound {
            employee_id: child.assigned_employee_id,
        },
    )?;
    let dispatch = task::query_task_dispatch(transaction, child.id)?;
    let output = dispatch
        .as_ref()
        .and_then(|dispatch| dispatch.turn_id)
        .map(|turn_id| latest_turn_output(transaction, turn_id))
        .transpose()?
        .flatten()
        .map(|mut output| {
            truncate_utf8(&mut output, MAX_NOTICE_OUTPUT_BYTES);
            output
        });
    let remaining: i64 = transaction.query_row(
        "SELECT COUNT(*) FROM tasks
         WHERE parent_task_id = ?1
           AND state IN ('proposed', 'accepted', 'running', 'cancel-requested')",
        [parent.id.to_string()],
        |row| row.get(0),
    )?;
    let remaining_children = u64::try_from(remaining).unwrap_or(0);
    let text = render_task_result_text(&TaskResultNotice {
        child_task_id: child.id,
        child_employee_name: &child_employee.name,
        state: child.state,
        objective: &child.objective,
        failure_code: child.failure_code.as_deref(),
        failure_message: child.failure_message.as_deref(),
        output: output.as_deref(),
        parent_task_id: parent.id,
        remaining_children,
    });
    let sender_session_id = dispatch
        .as_ref()
        .and_then(|dispatch| dispatch.session_id)
        .filter(|session_id| {
            session::query_session(transaction, *session_id)
                .ok()
                .flatten()
                .is_some_and(|session| session.employee_id == child.assigned_employee_id)
        });
    message::insert_message_tx(
        transaction,
        &message::InsertMessageInput {
            request_id: request_id.to_owned(),
            command_scope: TASK_RESULT_SCOPE,
            idempotency_key: key,
            version: MESSAGE_ENVELOPE_V1,
            sender: MessageSender::Employee(MessageEndpoint {
                employee_id: child.assigned_employee_id,
                session_id: sender_session_id,
            }),
            recipient: MessageEndpoint {
                employee_id: parent.assigned_employee_id,
                session_id: None,
            },
            project_id: parent.project_id,
            conversation_id: Some(parent.conversation_id),
            task_id: Some(parent.id),
            correlation_id: None,
            causation_id: child.delegation_message_id,
            kind: MessageKind::Result,
            body: json!({
                "kind": "task-result",
                "childTaskId": child.id,
                "state": child.state,
                "assignedEmployeeId": child.assigned_employee_id,
                "objective": child.objective,
                "failureCode": child.failure_code,
                "failureMessage": child.failure_message,
                "output": output,
                "remainingChildren": remaining_children,
                "text": text,
            }),
            references: vec![MessageReference {
                kind: "task".to_owned(),
                target: child.id.to_string(),
                metadata: json!({}),
            }],
            max_attempts: NOTICE_MAX_ATTEMPTS,
        },
    )
}

fn queue_incident_notice_tx(
    transaction: &Transaction<'_>,
    request_id: &str,
    incident_id: Uuid,
) -> Result<MessageMutationResult, StoreError> {
    let key = incident_notice_key(incident_id);
    if let Some(message) = message::message_for_command(transaction, INCIDENT_NOTICE_SCOPE, &key)? {
        return Ok(MessageMutationResult {
            message,
            event_sequence: None,
        });
    }
    let not_applicable = StoreError::IncidentNoticeNotApplicable { incident_id };
    let record = incident::required_incident(transaction, incident_id)?;
    let Some(task_id) = record.task_id else {
        return Err(not_applicable);
    };
    if record.priority != IncidentPriority::Priority {
        return Err(not_applicable);
    }
    let task = task::required_task(transaction, task_id)?;
    let root = task::required_task(transaction, task.root_task_id)?;
    if task.id == root.id || record.employee_id == Some(root.assigned_employee_id) {
        return Err(not_applicable);
    }
    let employee_name = record
        .employee_id
        .map(|employee_id| employee::query_employee(transaction, employee_id))
        .transpose()?
        .flatten()
        .map(|employee| employee.name);
    let text = render_incident_notice_text(&IncidentNotice {
        incident_id: record.id,
        code: &record.code,
        title: &record.title,
        explanation: &record.explanation,
        next_action: &record.next_action,
        priority: record.priority,
        occurrences: record.occurrences,
        task_id: task.id,
        employee_name: employee_name.as_deref(),
    });
    message::insert_message_tx(
        transaction,
        &message::InsertMessageInput {
            request_id: request_id.to_owned(),
            command_scope: INCIDENT_NOTICE_SCOPE,
            idempotency_key: key,
            version: MESSAGE_ENVELOPE_V1,
            sender: MessageSender::Employee(MessageEndpoint {
                employee_id: task.assigned_employee_id,
                session_id: None,
            }),
            recipient: MessageEndpoint {
                employee_id: root.assigned_employee_id,
                session_id: None,
            },
            project_id: root.project_id,
            conversation_id: Some(root.conversation_id),
            task_id: Some(root.id),
            correlation_id: None,
            causation_id: None,
            kind: MessageKind::Status,
            body: json!({
                "kind": "incident-notice",
                "incidentId": record.id,
                "code": record.code,
                "title": record.title,
                "explanation": record.explanation,
                "nextAction": record.next_action,
                "priority": record.priority,
                "occurrences": record.occurrences,
                "taskId": task.id,
                "employeeId": record.employee_id,
                "text": text,
            }),
            references: vec![
                MessageReference {
                    kind: "incident".to_owned(),
                    target: record.id.to_string(),
                    metadata: json!({}),
                },
                MessageReference {
                    kind: "task".to_owned(),
                    target: task.id.to_string(),
                    metadata: json!({}),
                },
            ],
            max_attempts: NOTICE_MAX_ATTEMPTS,
        },
    )
}

fn record_notice_turn_tx(
    transaction: &Transaction<'_>,
    input: &RecordNoticeTurnInput,
) -> Result<TurnMutationResult, StoreError> {
    let key = notice_turn_key(input.message_id);
    if let Some(turn) = conversation::turn_for_command(transaction, "turn.record", &key)? {
        return conversation::turn_result(transaction, turn, None);
    }
    let message = message::required_message(transaction, input.message_id)?;
    let not_notice = StoreError::MessageNotNotice {
        message_id: input.message_id,
    };
    if !matches!(
        message.envelope.kind,
        MessageKind::Result | MessageKind::Status
    ) || !is_lead_notice(transaction, input.message_id)?
    {
        return Err(not_notice);
    }
    message::require_message_state(&message, &[MessageDeliveryState::Queued], "queued")?;
    let Some(task_id) = message.envelope.task_id else {
        return Err(not_notice);
    };
    let task = task::required_task(transaction, task_id)?;
    let another_turn_unfinished: bool = transaction.query_row(
        "SELECT EXISTS (
            SELECT 1 FROM conversation_turns
            WHERE task_id = ?1 AND state IN ('recorded', 'submitting', 'accepted')
         )",
        [task.id.to_string()],
        |row| row.get(0),
    )?;
    if task.assigned_employee_id != message.envelope.recipient.employee_id
        || !matches!(task.state, TaskState::Accepted | TaskState::Running)
        || task.active_turn_id.is_some()
        || another_turn_unfinished
    {
        return Err(StoreError::TurnTaskScope { task_id: task.id });
    }
    task::require_valid_ancestry(transaction, &task)?;
    let recipient =
        conversation::require_route_ready_employee(transaction, task.assigned_employee_id)?;
    let text = message
        .envelope
        .body
        .get("text")
        .and_then(Value::as_str)
        .ok_or(StoreError::InvalidRecord {
            field: "messages.body_json",
            message: "lead notice has no text".to_owned(),
        })?;
    validate_turn_text(text)?;
    let turn_id = Uuid::new_v4();
    let now = Utc::now().to_rfc3339();
    let (author_kind, author_employee_id, author_session_id) =
        conversation::sender_columns(message.envelope.sender);
    transaction.execute(
        "INSERT INTO conversation_turns (
            id, contract_version, conversation_id, task_id, author_kind, author_employee_id,
            author_session_id, recipient_employee_id, recipient_session_id,
            recipient_employee_configuration_revision, text, message_id, correlation_id,
            state, revision, created_at, updated_at
         ) VALUES (
            ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, NULL, ?9, ?10, ?11, ?12, 'recorded', 1, ?13, ?13
         )",
        params![
            turn_id.to_string(),
            i64::from(CONVERSATION_CONTRACT_V1),
            task.conversation_id.to_string(),
            task.id.to_string(),
            author_kind,
            author_employee_id,
            author_session_id,
            task.assigned_employee_id.to_string(),
            to_i64(recipient.configuration_revision)?,
            text,
            message.envelope.id.to_string(),
            message.envelope.correlation_id.to_string(),
            now,
        ],
    )?;
    conversation::insert_turn_command(
        transaction,
        "turn.record",
        &key,
        &input.request_id,
        turn_id,
        &now,
    )?;
    let turn = conversation::required_turn(transaction, turn_id)?;
    let event_sequence = employee::insert_event(
        transaction,
        "turn.recorded",
        &json!({"turn": &turn, "message": &message, "notice": true}),
        &now,
    )?;
    Ok(TurnMutationResult {
        turn,
        message,
        task: Some(task),
        event_sequence: Some(event_sequence),
    })
}

fn consume_task_result_tx(
    transaction: &Transaction<'_>,
    input: &ConsumeTaskResultInput,
) -> Result<MessageMutationResult, StoreError> {
    let turn = conversation::required_turn(transaction, input.turn_id)?;
    let queued = queue_task_result_tx(transaction, &input.request_id, input.child_task_id)?;
    let message = queued.message;
    if message.envelope.recipient.employee_id != turn.recipient.employee_id
        || message.envelope.task_id != turn.task_id
    {
        return Err(StoreError::TurnTaskScope {
            task_id: message.envelope.task_id.unwrap_or(input.child_task_id),
        });
    }
    if message.delivery.state != MessageDeliveryState::Queued {
        return Ok(MessageMutationResult {
            message,
            event_sequence: None,
        });
    }
    let claimed = message::claim_message_tx(
        transaction,
        &message::ClaimMessageInput {
            request_id: input.request_id.clone(),
            idempotency_key: format!("task-result-claim:{}:{}", input.child_task_id, turn.id),
            message_id: message.envelope.id,
            expected_revision: message.delivery.revision,
        },
    )?;
    let attempt_id = claimed
        .message
        .attempts
        .last()
        .ok_or(StoreError::InvalidRecord {
            field: "message_delivery_attempts",
            message: "claimed notice has no delivery attempt".to_owned(),
        })?
        .id;
    message::acknowledge_message_tx(
        transaction,
        &message::AcknowledgeMessageInput {
            request_id: input.request_id.clone(),
            idempotency_key: format!("task-result-ack:{}:{}", input.child_task_id, turn.id),
            message_id: message.envelope.id,
            expected_revision: claimed.message.delivery.revision,
            attempt_id,
            acknowledgement_key: format!("task-wait:{}", turn.id),
            evidence: json!({"via": "task_wait", "turnId": turn.id}),
        },
    )
}

fn is_lead_notice(connection: &Connection, message_id: Uuid) -> Result<bool, StoreError> {
    Ok(connection.query_row(
        "SELECT EXISTS (
            SELECT 1 FROM message_commands WHERE message_id = ?1 AND scope IN (?2, ?3)
         )",
        params![
            message_id.to_string(),
            TASK_RESULT_SCOPE,
            INCIDENT_NOTICE_SCOPE
        ],
        |row| row.get(0),
    )?)
}

fn latest_turn_output(
    connection: &Connection,
    turn_id: Uuid,
) -> Result<Option<String>, StoreError> {
    let mut statement = connection.prepare(
        "SELECT payload_json FROM conversation_turn_events
         WHERE turn_id = ?1 ORDER BY sequence DESC",
    )?;
    let rows = statement.query_map([turn_id.to_string()], |row| row.get::<_, String>(0))?;
    for payload in rows {
        let payload: Value = serde_json::from_str(&payload?)?;
        if let Some(output) = payload.get("output").and_then(Value::as_str) {
            return Ok(Some(output.to_owned()));
        }
    }
    Ok(None)
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

fn serialized_name<T: serde::Serialize>(value: T) -> Result<String, StoreError> {
    match serde_json::to_value(value)? {
        Value::String(name) => Ok(name),
        other => Err(StoreError::InvalidRecord {
            field: "serialized_name",
            message: format!("expected a string, found {other}"),
        }),
    }
}

fn parse_uuid(value: &str, field: &'static str) -> Result<Uuid, StoreError> {
    Uuid::parse_str(value).map_err(|error| StoreError::InvalidRecord {
        field,
        message: error.to_string(),
    })
}

fn to_i64(value: u64) -> Result<i64, StoreError> {
    i64::try_from(value).map_err(|error| StoreError::InvalidRecord {
        field: "u64",
        message: error.to_string(),
    })
}
