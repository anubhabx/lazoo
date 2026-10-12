//! Lead notices: durable `result` and `status` messages that carry a child task's outcome or a
//! promoted incident to the lead that owns the task tree, and the turn that delivers one.
//!
//! A notice is derived evidence. It is queued idempotently after the task or incident commit,
//! keyed by the child task or incident id, so a second daemon boot never duplicates it. The
//! failure, task, and incident rows it points at stay authoritative.
//!
//! One notice turn carries every notice queued for the same recipient and task at the time it
//! is recorded: the oldest is the turn's canonical message and the rest are companions linked
//! through their own `turn.record` command, so each keeps its own delivery evidence.

use chrono::Utc;
use lazoo_domain::{
    IncidentNotice, MAX_NOTICE_OUTPUT_BYTES, MAX_TURN_TEXT_BYTES, PeerMessageNotice,
    TaskResultNotice, render_coalesced_notice_text, render_incident_notice_text,
    render_peer_message_text, render_task_result_text, validate_peer_message_content,
    validate_turn_text,
};
use lazoo_protocol::{
    CONVERSATION_CONTRACT_V1, ConversationTurnRecord, IncidentEscalation, IncidentPriority,
    MESSAGE_ENVELOPE_V1, MessageDeliveryState, MessageEndpoint, MessageKind, MessageMutationResult,
    MessagePriority, MessageRecord, MessageReference, MessageSender, TaskRecord, TaskState,
    TurnMutationResult, TurnState,
};
use rusqlite::{Connection, OptionalExtension, Transaction, TransactionBehavior, params};
use serde_json::{Value, json};
use uuid::Uuid;

use crate::{Store, StoreError, conversation, employee, incident, message, session, task};

const TASK_RESULT_SCOPE: &str = "task.result";
const INCIDENT_NOTICE_SCOPE: &str = "incident.notice";
/// Peer messages and replies sent by an employee through `message_send` / `message_reply`.
const PEER_SCOPE: &str = "message.peer";
const NOTICE_MAX_ATTEMPTS: u32 = 3;
const PEER_INBOX_LIMIT: i64 = 64;
/// The `turn.record` command key prefix shared by every notice a turn delivers.
const NOTICE_TURN_PREFIX: &str = "notice-turn:";
/// Queued notices considered for one coalesced turn; the turn text limit bounds it further.
const NOTICE_BATCH_LIMIT: i64 = 128;

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

/// A peer message from the employee owning `turn_id` to another employee working in the same
/// task tree. The daemon fixes the sender to the turn's recipient endpoint.
#[derive(Clone, Debug)]
pub struct SendPeerMessageInput {
    pub request_id: String,
    pub idempotency_key: String,
    pub turn_id: Uuid,
    pub recipient_employee_id: Uuid,
    pub content: String,
    pub references: Vec<MessageReference>,
}

/// A reply, from the employee owning `turn_id`, to a peer message that turn's task received.
#[derive(Clone, Debug)]
pub struct ReplyPeerMessageInput {
    pub request_id: String,
    pub idempotency_key: String,
    pub turn_id: Uuid,
    pub message_id: Uuid,
    pub content: String,
    pub references: Vec<MessageReference>,
}

/// Marks a queued peer message as read inside the recipient's own in-flight turn.
#[derive(Clone, Debug)]
pub struct ConsumePeerMessageInput {
    pub request_id: String,
    pub message_id: Uuid,
    pub turn_id: Uuid,
    /// The tool that read it, recorded as acknowledgement evidence (`task_wait`, `message_send`).
    pub via: &'static str,
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

    /// Queued lead notices and peer messages, oldest first. Only messages this module queued
    /// are returned; messages sent through `message.send` keep their manual claim path.
    pub fn pending_lead_notices(&self, limit: u32) -> Result<Vec<MessageRecord>, StoreError> {
        let connection = self.lock()?;
        let mut statement = connection.prepare(
            "SELECT m.id FROM messages m
             JOIN message_deliveries d ON d.message_id = m.id
             JOIN message_commands mc ON mc.message_id = m.id AND mc.scope IN (?1, ?2, ?3)
             WHERE d.state = 'queued'
             ORDER BY m.sequence LIMIT ?4",
        )?;
        let ids: Vec<String> = statement
            .query_map(
                params![
                    TASK_RESULT_SCOPE,
                    INCIDENT_NOTICE_SCOPE,
                    PEER_SCOPE,
                    i64::from(limit)
                ],
                |row| row.get::<_, String>(0),
            )?
            .collect::<Result<_, _>>()?;
        ids.iter()
            .map(|id| message::required_message(&connection, parse_uuid(id, "messages.id")?))
            .collect()
    }

    /// Sends one peer message from the turn's employee to another employee's active task in
    /// the same task tree. Replays by idempotency key return the existing message.
    pub fn send_peer_message(
        &self,
        input: &SendPeerMessageInput,
    ) -> Result<MessageMutationResult, StoreError> {
        let mut connection = self.lock()?;
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let result = send_peer_message_tx(&transaction, input)?;
        transaction.commit()?;
        Ok(result)
    }

    /// Replies to a peer message the turn's task received; the reply goes back to the original
    /// sender's task and threads under the original message.
    pub fn reply_peer_message(
        &self,
        input: &ReplyPeerMessageInput,
    ) -> Result<MessageMutationResult, StoreError> {
        let mut connection = self.lock()?;
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let result = reply_peer_message_tx(&transaction, input)?;
        transaction.commit()?;
        Ok(result)
    }

    /// Queued peer messages and replies addressed to one employee's task, oldest first.
    pub fn pending_peer_messages(
        &self,
        employee_id: Uuid,
        task_id: Uuid,
    ) -> Result<Vec<MessageRecord>, StoreError> {
        let connection = self.lock()?;
        let mut statement = connection.prepare(
            "SELECT m.id FROM messages m
             JOIN message_deliveries d ON d.message_id = m.id
             JOIN message_commands mc ON mc.message_id = m.id AND mc.scope = ?1
             WHERE d.state = 'queued' AND m.recipient_employee_id = ?2 AND m.task_id = ?3
             ORDER BY m.sequence LIMIT ?4",
        )?;
        let ids: Vec<String> = statement
            .query_map(
                params![
                    PEER_SCOPE,
                    employee_id.to_string(),
                    task_id.to_string(),
                    PEER_INBOX_LIMIT
                ],
                |row| row.get::<_, String>(0),
            )?
            .collect::<Result<_, _>>()?;
        ids.iter()
            .map(|id| message::required_message(&connection, parse_uuid(id, "messages.id")?))
            .collect()
    }

    /// Marks a peer message as delivered inside the recipient's own turn: claimed and
    /// acknowledged with that turn as evidence, so no notice turn follows.
    pub fn consume_peer_message(
        &self,
        input: &ConsumePeerMessageInput,
    ) -> Result<MessageMutationResult, StoreError> {
        let mut connection = self.lock()?;
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let result = consume_peer_message_tx(&transaction, input)?;
        transaction.commit()?;
        Ok(result)
    }

    /// Records the conversation turn that delivers a queued notice to the lead, reusing the
    /// notice as the turn's canonical message and carrying every other notice queued for the
    /// same recipient and task as companions. Replays by message id return the same turn.
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

/// One `turn.record` command per notice a turn delivers. The turn id is part of the key so a
/// notice whose recorded turn was closed before delivery can be recorded again.
fn notice_turn_key(message_id: Uuid, turn_id: Uuid) -> String {
    format!("{NOTICE_TURN_PREFIX}{message_id}:{turn_id}")
}

/// The newest turn recorded to deliver a notice, if any. Keys were `notice-turn:<message>`
/// before a turn could carry companions; the prefix match covers both forms.
fn latest_notice_turn(
    connection: &Connection,
    message_id: Uuid,
) -> Result<Option<ConversationTurnRecord>, StoreError> {
    connection
        .query_row(
            "SELECT tc.turn_id FROM turn_commands tc
             JOIN conversation_turns t ON t.id = tc.turn_id
             WHERE tc.scope = 'turn.record' AND tc.idempotency_key LIKE ?1
             ORDER BY t.sequence DESC LIMIT 1",
            [format!("{NOTICE_TURN_PREFIX}{message_id}%")],
            |row| row.get::<_, String>(0),
        )
        .optional()?
        .map(|id| {
            conversation::required_turn(connection, parse_uuid(&id, "turn_commands.turn_id")?)
        })
        .transpose()
}

/// Every notice a turn delivers, oldest first: the message id sits right after the key prefix.
fn notice_member_ids(connection: &Connection, turn_id: Uuid) -> Result<Vec<Uuid>, StoreError> {
    let mut statement = connection.prepare(
        "SELECT m.id FROM turn_commands tc
         JOIN messages m ON m.id = substr(tc.idempotency_key, ?2, 36)
         WHERE tc.scope = 'turn.record' AND tc.turn_id = ?1 AND tc.idempotency_key LIKE ?3
         ORDER BY m.sequence",
    )?;
    let prefix_end = i64::try_from(NOTICE_TURN_PREFIX.len() + 1).expect("short prefix");
    let rows = statement.query_map(
        params![
            turn_id.to_string(),
            prefix_end,
            format!("{NOTICE_TURN_PREFIX}%")
        ],
        |row| row.get::<_, String>(0),
    )?;
    rows.map(|row| parse_uuid(&row?, "messages.id")).collect()
}

/// The notices a coalesced turn delivers besides its canonical message, oldest first. Their
/// delivery follows the canonical message through every turn transition.
pub(crate) fn notice_companions(
    connection: &Connection,
    turn: &ConversationTurnRecord,
) -> Result<Vec<MessageRecord>, StoreError> {
    notice_member_ids(connection, turn.id)?
        .into_iter()
        .filter(|id| *id != turn.message_id)
        .map(|id| message::required_message(connection, id))
        .collect()
}

/// Queued notices and peer messages addressed to one employee's task, oldest first.
fn queued_notices_for(
    connection: &Connection,
    employee_id: Uuid,
    task_id: Uuid,
) -> Result<Vec<MessageRecord>, StoreError> {
    let mut statement = connection.prepare(
        "SELECT m.id FROM messages m
         JOIN message_deliveries d ON d.message_id = m.id
         JOIN message_commands mc ON mc.message_id = m.id AND mc.scope IN (?1, ?2, ?3)
         WHERE d.state = 'queued' AND m.recipient_employee_id = ?4 AND m.task_id = ?5
         ORDER BY m.sequence LIMIT ?6",
    )?;
    let ids: Vec<String> = statement
        .query_map(
            params![
                TASK_RESULT_SCOPE,
                INCIDENT_NOTICE_SCOPE,
                PEER_SCOPE,
                employee_id.to_string(),
                task_id.to_string(),
                NOTICE_BATCH_LIMIT
            ],
            |row| row.get::<_, String>(0),
        )?
        .collect::<Result<_, _>>()?;
    ids.iter()
        .map(|id| message::required_message(connection, parse_uuid(id, "messages.id")?))
        .collect()
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
            id: None,
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
            priority: MessagePriority::Normal,
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
    if record.priority != IncidentPriority::Priority {
        return Err(not_applicable);
    }
    let Some(root) = incident::incident_lead_root(transaction, record.task_id, record.employee_id)?
    else {
        return Err(not_applicable);
    };
    let task = task::required_task(transaction, record.task_id.ok_or(not_applicable)?)?;
    // The lead reads its remediation window only while it is still open to it.
    let remediation_deadline_at = (record.escalation == IncidentEscalation::Lead)
        .then_some(record.remediation_deadline_at.as_deref())
        .flatten();
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
        remediation_deadline_at,
    });
    message::insert_message_tx(
        transaction,
        &message::InsertMessageInput {
            request_id: request_id.to_owned(),
            command_scope: INCIDENT_NOTICE_SCOPE,
            idempotency_key: key,
            id: None,
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
                "remediationDeadlineAt": remediation_deadline_at,
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
            // A lead that never reads this notice cannot remediate the incident, so losing it is
            // itself a priority incident.
            priority: MessagePriority::Priority,
        },
    )
}

#[allow(clippy::too_many_lines)] // Guards, batch gathering, and the record-first insert stay together.
fn record_notice_turn_tx(
    transaction: &Transaction<'_>,
    input: &RecordNoticeTurnInput,
) -> Result<TurnMutationResult, StoreError> {
    let message = message::required_message(transaction, input.message_id)?;
    // A turn closed before delivery (its notices were read inside another turn) releases the
    // notices still queued for a fresh turn; every other recorded turn is the replay.
    if let Some(turn) = latest_notice_turn(transaction, input.message_id)?
        && !(turn.state == TurnState::Cancelled
            && message.delivery.state == MessageDeliveryState::Queued)
    {
        return conversation::turn_result(transaction, turn, None);
    }
    let not_notice = StoreError::MessageNotNotice {
        message_id: input.message_id,
    };
    let Some(scope) = notice_scope(transaction, input.message_id)? else {
        return Err(not_notice);
    };
    if !matches!(
        message.envelope.kind,
        MessageKind::Result | MessageKind::Status | MessageKind::Peer | MessageKind::Reply
    ) {
        return Err(not_notice);
    }
    message::require_message_state(&message, &[MessageDeliveryState::Queued], "queued")?;
    let Some(task_id) = message.envelope.task_id else {
        return Err(not_notice);
    };
    let task = task::required_task(transaction, task_id)?;
    require_recipient_free(
        transaction,
        &task,
        message.envelope.recipient.employee_id,
        scope == PEER_SCOPE,
    )?;
    task::require_valid_ancestry(transaction, &task)?;
    let recipient =
        conversation::require_route_ready_employee(transaction, task.assigned_employee_id)?;
    let mut texts = vec![notice_text(&message)?];
    let mut companions = Vec::new();
    // The requested notice leads; the others queued for this recipient and task follow oldest
    // first until the turn text limit. A peer message still waits for a running task.
    for candidate in queued_notices_for(transaction, task.assigned_employee_id, task.id)? {
        if candidate.envelope.id == message.envelope.id
            || (task.state != TaskState::Running
                && notice_scope(transaction, candidate.envelope.id)? == Some(PEER_SCOPE))
        {
            continue;
        }
        texts.push(notice_text(&candidate)?);
        // ponytail: re-rendering per candidate is quadratic, but the 256 KiB text limit caps a
        // batch at about sixteen notices.
        if render_coalesced_notice_text(&as_strs(&texts)).len() > MAX_TURN_TEXT_BYTES {
            texts.pop();
            break;
        }
        companions.push(candidate.envelope.id);
    }
    let text = render_coalesced_notice_text(&as_strs(&texts));
    validate_turn_text(&text)?;
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
    for member in std::iter::once(message.envelope.id).chain(companions.iter().copied()) {
        conversation::insert_turn_command(
            transaction,
            "turn.record",
            &notice_turn_key(member, turn_id),
            &input.request_id,
            turn_id,
            &now,
        )?;
    }
    let turn = conversation::required_turn(transaction, turn_id)?;
    let event_sequence = employee::insert_event(
        transaction,
        "turn.recorded",
        &json!({
            "turn": &turn,
            "message": &message,
            "notice": true,
            "companions": companions,
        }),
        &now,
    )?;
    Ok(TurnMutationResult {
        turn,
        message,
        task: Some(task),
        event_sequence: Some(event_sequence),
    })
}

fn notice_text(message: &MessageRecord) -> Result<String, StoreError> {
    message
        .envelope
        .body
        .get("text")
        .and_then(Value::as_str)
        .map(str::to_owned)
        .ok_or(StoreError::InvalidRecord {
            field: "messages.body_json",
            message: "lead notice has no text".to_owned(),
        })
}

fn as_strs(texts: &[String]) -> Vec<&str> {
    texts.iter().map(String::as_str).collect()
}

/// The recipient's task must belong to the notice's recipient, be deliverable, and have no
/// unfinished turn. A peer message also waits until that task is running: delivering it to
/// an accepted task would start the task with a peer message instead of its objective.
fn require_recipient_free(
    transaction: &Transaction<'_>,
    task: &TaskRecord,
    recipient_employee_id: Uuid,
    peer: bool,
) -> Result<(), StoreError> {
    let another_turn_unfinished: bool = transaction.query_row(
        "SELECT EXISTS (
            SELECT 1 FROM conversation_turns
            WHERE task_id = ?1 AND state IN ('recorded', 'submitting', 'accepted')
         )",
        [task.id.to_string()],
        |row| row.get(0),
    )?;
    let state_allows = if peer {
        task.state == TaskState::Running
    } else {
        matches!(task.state, TaskState::Accepted | TaskState::Running)
    };
    if task.assigned_employee_id != recipient_employee_id
        || !state_allows
        || task.active_turn_id.is_some()
        || another_turn_unfinished
    {
        return Err(StoreError::TurnTaskScope { task_id: task.id });
    }
    Ok(())
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
    consume_queued_message_tx(
        transaction,
        message,
        &turn,
        &input.request_id,
        "task-result",
        "task_wait",
    )
}

fn send_peer_message_tx(
    transaction: &Transaction<'_>,
    input: &SendPeerMessageInput,
) -> Result<MessageMutationResult, StoreError> {
    if let Some(message) =
        message::message_for_command(transaction, PEER_SCOPE, &input.idempotency_key)?
    {
        return Ok(MessageMutationResult {
            message,
            event_sequence: None,
        });
    }
    validate_peer_message_content(&input.content)?;
    let (sender, sender_task) = peer_sender(transaction, input.turn_id)?;
    if input.recipient_employee_id == sender.employee_id {
        return Err(StoreError::PeerRecipientSelf {
            employee_id: sender.employee_id,
        });
    }
    let recipient_task = active_task_in_tree(
        transaction,
        sender_task.root_task_id,
        input.recipient_employee_id,
    )?
    .ok_or(StoreError::PeerRecipientTaskNotFound {
        employee_id: input.recipient_employee_id,
    })?;
    insert_peer_message_tx(
        transaction,
        &input.request_id,
        &input.idempotency_key,
        sender,
        &sender_task,
        &recipient_task,
        None,
        &input.content,
        &input.references,
    )
}

fn reply_peer_message_tx(
    transaction: &Transaction<'_>,
    input: &ReplyPeerMessageInput,
) -> Result<MessageMutationResult, StoreError> {
    if let Some(message) =
        message::message_for_command(transaction, PEER_SCOPE, &input.idempotency_key)?
    {
        return Ok(MessageMutationResult {
            message,
            event_sequence: None,
        });
    }
    validate_peer_message_content(&input.content)?;
    let (sender, sender_task) = peer_sender(transaction, input.turn_id)?;
    let original = message::required_message(transaction, input.message_id)?;
    let scope_error = StoreError::PeerMessageScope {
        message_id: input.message_id,
    };
    if notice_scope(transaction, input.message_id)? != Some(PEER_SCOPE)
        || original.envelope.recipient.employee_id != sender.employee_id
        || original.envelope.task_id != Some(sender_task.id)
    {
        return Err(scope_error);
    }
    let MessageSender::Employee(original_sender) = original.envelope.sender else {
        return Err(scope_error);
    };
    let original_task_id = original
        .envelope
        .body
        .get("senderTaskId")
        .and_then(Value::as_str)
        .map(|id| parse_uuid(id, "messages.body_json.senderTaskId"))
        .transpose()?
        .ok_or(StoreError::InvalidRecord {
            field: "messages.body_json",
            message: "peer message has no sender task".to_owned(),
        })?;
    let recipient_task = task::required_task(transaction, original_task_id)?;
    if !matches!(
        recipient_task.state,
        TaskState::Accepted | TaskState::Running
    ) || recipient_task.assigned_employee_id != original_sender.employee_id
    {
        return Err(StoreError::PeerRecipientTaskNotFound {
            employee_id: original_sender.employee_id,
        });
    }
    insert_peer_message_tx(
        transaction,
        &input.request_id,
        &input.idempotency_key,
        sender,
        &sender_task,
        &recipient_task,
        Some(&original),
        &input.content,
        &input.references,
    )
}

/// The sending endpoint is always the turn's recipient employee on the turn's task.
fn peer_sender(
    transaction: &Transaction<'_>,
    turn_id: Uuid,
) -> Result<(MessageEndpoint, TaskRecord), StoreError> {
    let turn = conversation::required_turn(transaction, turn_id)?;
    let task_id = turn.task_id.ok_or(StoreError::TurnTaskRequired)?;
    let task = task::required_task(transaction, task_id)?;
    if !matches!(task.state, TaskState::Accepted | TaskState::Running) {
        return Err(StoreError::TaskState {
            task_id: task.id,
            state: serialized_name(task.state)?,
            expected: "accepted or running",
        });
    }
    Ok((turn.recipient, task))
}

/// The recipient's unfinished task in the sender's tree: its running task, else its oldest
/// accepted one. An employee outside the tree has none and cannot be messaged.
fn active_task_in_tree(
    transaction: &Transaction<'_>,
    root_task_id: Uuid,
    employee_id: Uuid,
) -> Result<Option<TaskRecord>, StoreError> {
    transaction
        .query_row(
            "SELECT id FROM tasks
             WHERE root_task_id = ?1 AND assigned_employee_id = ?2
               AND state IN ('accepted', 'running')
             ORDER BY CASE state WHEN 'running' THEN 0 ELSE 1 END, sequence LIMIT 1",
            params![root_task_id.to_string(), employee_id.to_string()],
            |row| row.get::<_, String>(0),
        )
        .optional()?
        .map(|id| task::required_task(transaction, parse_uuid(&id, "tasks.id")?))
        .transpose()
}

#[allow(clippy::too_many_arguments)] // One record-first insert names every party of the exchange.
fn insert_peer_message_tx(
    transaction: &Transaction<'_>,
    request_id: &str,
    idempotency_key: &str,
    sender: MessageEndpoint,
    sender_task: &TaskRecord,
    recipient_task: &TaskRecord,
    reply_to: Option<&MessageRecord>,
    content: &str,
    references: &[MessageReference],
) -> Result<MessageMutationResult, StoreError> {
    let sender_name = employee::query_employee(transaction, sender.employee_id)?
        .ok_or(StoreError::EmployeeNotFound {
            employee_id: sender.employee_id,
        })?
        .name;
    let message_id = Uuid::new_v4();
    let reply_to_id = reply_to.map(|message| message.envelope.id);
    let text = render_peer_message_text(&PeerMessageNotice {
        message_id,
        sender_employee_name: &sender_name,
        sender_task_id: sender_task.id,
        recipient_task_id: recipient_task.id,
        reply_to_message_id: reply_to_id,
        content,
    });
    validate_turn_text(&text)?;
    let task_reference = |task: &TaskRecord| MessageReference {
        kind: "task".to_owned(),
        target: task.id.to_string(),
        metadata: json!({}),
    };
    let mut all_references = vec![task_reference(sender_task), task_reference(recipient_task)];
    all_references.extend_from_slice(references);
    message::insert_message_tx(
        transaction,
        &message::InsertMessageInput {
            request_id: request_id.to_owned(),
            command_scope: PEER_SCOPE,
            idempotency_key: idempotency_key.to_owned(),
            id: Some(message_id),
            version: MESSAGE_ENVELOPE_V1,
            sender: MessageSender::Employee(sender),
            recipient: MessageEndpoint {
                employee_id: recipient_task.assigned_employee_id,
                session_id: None,
            },
            project_id: recipient_task.project_id,
            conversation_id: Some(recipient_task.conversation_id),
            task_id: Some(recipient_task.id),
            correlation_id: None,
            causation_id: reply_to_id,
            kind: if reply_to.is_some() {
                MessageKind::Reply
            } else {
                MessageKind::Peer
            },
            body: json!({
                "kind": "peer-message",
                "content": content,
                "senderEmployeeId": sender.employee_id,
                "senderEmployeeName": sender_name,
                "senderTaskId": sender_task.id,
                "recipientTaskId": recipient_task.id,
                "replyToMessageId": reply_to_id,
                "text": text,
            }),
            references: all_references,
            max_attempts: NOTICE_MAX_ATTEMPTS,
            priority: MessagePriority::Normal,
        },
    )
}

fn consume_peer_message_tx(
    transaction: &Transaction<'_>,
    input: &ConsumePeerMessageInput,
) -> Result<MessageMutationResult, StoreError> {
    let turn = conversation::required_turn(transaction, input.turn_id)?;
    let message = message::required_message(transaction, input.message_id)?;
    if notice_scope(transaction, input.message_id)? != Some(PEER_SCOPE)
        || message.envelope.recipient.employee_id != turn.recipient.employee_id
        || message.envelope.task_id != turn.task_id
    {
        return Err(StoreError::PeerMessageScope {
            message_id: input.message_id,
        });
    }
    consume_queued_message_tx(
        transaction,
        message,
        &turn,
        &input.request_id,
        "peer",
        input.via,
    )
}

/// Claims and acknowledges a queued notice with the reading turn as evidence. A notice turn
/// recorded earlier for the same message (the recipient was dormant at the time) is closed
/// here, so it can neither deliver the message twice nor block later notice turns; any other
/// notice it carried stays queued and is recorded again once this turn is finished.
fn consume_queued_message_tx(
    transaction: &Transaction<'_>,
    message: MessageRecord,
    turn: &ConversationTurnRecord,
    request_id: &str,
    key_prefix: &str,
    via: &str,
) -> Result<MessageMutationResult, StoreError> {
    if message.delivery.state != MessageDeliveryState::Queued {
        return Ok(MessageMutationResult {
            message,
            event_sequence: None,
        });
    }
    let now = Utc::now().to_rfc3339();
    if let Some(orphan) = latest_notice_turn(transaction, message.envelope.id)?
        && orphan.state == TurnState::Recorded
    {
        transaction.execute(
            "UPDATE conversation_turns SET state = 'cancelled', revision = revision + 1,
                    owner_boot_id = NULL, terminal_at = ?2, updated_at = ?2
             WHERE id = ?1 AND state = 'recorded'",
            params![orphan.id.to_string(), now],
        )?;
        let cancelled = conversation::required_turn(transaction, orphan.id)?;
        employee::insert_event(
            transaction,
            "turn.cancelled",
            &json!({"turn": &cancelled, "reason": "the notice was read inside another turn"}),
            &now,
        )?;
    }
    let claimed = message::claim_message_tx(
        transaction,
        &message::ClaimMessageInput {
            request_id: request_id.to_owned(),
            idempotency_key: format!("{key_prefix}-claim:{}:{}", message.envelope.id, turn.id),
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
            request_id: request_id.to_owned(),
            idempotency_key: format!("{key_prefix}-ack:{}:{}", message.envelope.id, turn.id),
            message_id: message.envelope.id,
            expected_revision: claimed.message.delivery.revision,
            attempt_id,
            acknowledgement_key: format!("{}:{}", via.replace('_', "-"), turn.id),
            evidence: json!({"via": via, "turnId": turn.id}),
        },
    )
}

/// The auto-delivery scope a message was queued under, if any.
fn notice_scope(
    connection: &Connection,
    message_id: Uuid,
) -> Result<Option<&'static str>, StoreError> {
    let scope: Option<String> = connection
        .query_row(
            "SELECT scope FROM message_commands
             WHERE message_id = ?1 AND scope IN (?2, ?3, ?4) LIMIT 1",
            params![
                message_id.to_string(),
                TASK_RESULT_SCOPE,
                INCIDENT_NOTICE_SCOPE,
                PEER_SCOPE
            ],
            |row| row.get(0),
        )
        .optional()?;
    Ok(scope.and_then(|scope| {
        [TASK_RESULT_SCOPE, INCIDENT_NOTICE_SCOPE, PEER_SCOPE]
            .into_iter()
            .find(|known| *known == scope)
    }))
}

/// Whether a turn delivers only peer messages; such a turn never completes the recipient's
/// task. A turn that also carries a result or incident notice projects like any other turn.
pub(crate) fn is_peer_only_turn(
    connection: &Connection,
    turn: &ConversationTurnRecord,
) -> Result<bool, StoreError> {
    if notice_scope(connection, turn.message_id)? != Some(PEER_SCOPE) {
        return Ok(false);
    }
    for member in notice_member_ids(connection, turn.id)? {
        if notice_scope(connection, member)? != Some(PEER_SCOPE) {
            return Ok(false);
        }
    }
    Ok(true)
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
