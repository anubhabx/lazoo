use chrono::{DateTime, Duration, Utc};
use lazoo_domain::{
    retry_delay_seconds, validate_acknowledgement, validate_envelope_sender, validate_max_attempts,
    validate_message_command_key, validate_message_content, validate_message_failure,
};
use lazoo_protocol::{
    MESSAGE_ENVELOPE_V1, MessageActivationIntent, MessageDeadLetter, MessageDelivery,
    MessageDeliveryAttempt, MessageDeliveryState, MessageEndpoint, MessageEnvelope, MessageKind,
    MessageMutationResult, MessageReconcileResult, MessageRecord, MessageReference, MessageSender,
    MessageSummary, UserMessageEndpoint, UserMessageKind,
};
use rusqlite::{Connection, OptionalExtension, Transaction, TransactionBehavior, params};
use serde::Serialize;
use serde_json::{Value, json};
use uuid::Uuid;

use crate::{Store, StoreError, employee, session, task};

#[derive(Clone, Debug)]
pub struct SendMessageInput {
    pub request_id: String,
    pub idempotency_key: String,
    pub sender: MessageEndpoint,
    pub recipient: MessageEndpoint,
    pub project_id: Option<Uuid>,
    pub conversation_id: Option<Uuid>,
    pub task_id: Option<Uuid>,
    pub correlation_id: Option<Uuid>,
    pub causation_id: Option<Uuid>,
    pub kind: MessageKind,
    pub body: Value,
    pub references: Vec<MessageReference>,
    pub max_attempts: u32,
}

#[derive(Clone, Debug)]
pub struct ClaimMessageInput {
    pub request_id: String,
    pub idempotency_key: String,
    pub message_id: Uuid,
    pub expected_revision: u64,
}

#[derive(Clone, Debug)]
pub struct AcknowledgeMessageInput {
    pub request_id: String,
    pub idempotency_key: String,
    pub message_id: Uuid,
    pub expected_revision: u64,
    pub attempt_id: Uuid,
    pub acknowledgement_key: String,
    pub evidence: Value,
}

#[derive(Clone, Debug)]
pub struct FailMessageInput {
    pub request_id: String,
    pub idempotency_key: String,
    pub message_id: Uuid,
    pub expected_revision: u64,
    pub attempt_id: Uuid,
    pub retryable: bool,
    pub failure_code: String,
    pub failure_message: String,
}

#[derive(Clone, Debug)]
pub struct RetryMessageInput {
    pub request_id: String,
    pub idempotency_key: String,
    pub message_id: Uuid,
    pub expected_revision: u64,
}

#[derive(Clone, Debug)]
pub struct CancelMessageInput {
    pub request_id: String,
    pub idempotency_key: String,
    pub message_id: Uuid,
    pub expected_revision: u64,
}

impl Store {
    pub fn send_message(
        &self,
        input: &SendMessageInput,
    ) -> Result<MessageMutationResult, StoreError> {
        let mut connection = self.lock()?;
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let result = insert_message_tx(
            &transaction,
            &InsertMessageInput {
                request_id: input.request_id.clone(),
                command_scope: "message.send",
                idempotency_key: input.idempotency_key.clone(),
                version: MESSAGE_ENVELOPE_V1,
                sender: MessageSender::Employee(input.sender),
                recipient: input.recipient,
                project_id: input.project_id,
                conversation_id: input.conversation_id,
                task_id: input.task_id,
                correlation_id: input.correlation_id,
                causation_id: input.causation_id,
                kind: input.kind,
                body: input.body.clone(),
                references: input.references.clone(),
                max_attempts: input.max_attempts,
            },
        )?;
        transaction.commit()?;
        Ok(result)
    }

    pub fn claim_message(
        &self,
        input: &ClaimMessageInput,
    ) -> Result<MessageMutationResult, StoreError> {
        let mut connection = self.lock()?;
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let result = claim_message_tx(&transaction, input)?;
        commit_result(transaction, result)
    }

    pub fn acknowledge_message(
        &self,
        input: &AcknowledgeMessageInput,
    ) -> Result<MessageMutationResult, StoreError> {
        let mut connection = self.lock()?;
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let result = acknowledge_message_tx(&transaction, input)?;
        commit_result(transaction, result)
    }

    pub fn fail_message(
        &self,
        input: &FailMessageInput,
    ) -> Result<MessageMutationResult, StoreError> {
        let mut connection = self.lock()?;
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let result = fail_message_tx(&transaction, input)?;
        commit_result(transaction, result)
    }

    pub fn retry_message(
        &self,
        input: &RetryMessageInput,
    ) -> Result<MessageMutationResult, StoreError> {
        let mut connection = self.lock()?;
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let result = retry_message_tx(&transaction, input)?;
        commit_result(transaction, result)
    }

    pub fn cancel_message(
        &self,
        input: &CancelMessageInput,
    ) -> Result<MessageMutationResult, StoreError> {
        let mut connection = self.lock()?;
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let result = cancel_message_tx(&transaction, input)?;
        commit_result(transaction, result)
    }

    pub fn reconcile_messages(&self) -> Result<MessageReconcileResult, StoreError> {
        let mut connection = self.lock()?;
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let mut statement = transaction.prepare(
            "SELECT m.id
             FROM messages m
             JOIN message_deliveries d ON d.message_id = m.id
             WHERE d.state = 'delivering'
             ORDER BY m.sequence",
        )?;
        let ids = statement
            .query_map([], |row| row.get::<_, String>(0))?
            .collect::<Result<Vec<_>, _>>()?;
        drop(statement);
        if ids.is_empty() {
            transaction.commit()?;
            return Ok(MessageReconcileResult {
                messages: Vec::new(),
                event_sequence: None,
            });
        }

        let now = Utc::now();
        let now_text = now.to_rfc3339();
        let mut messages = Vec::with_capacity(ids.len());
        for id in ids {
            let message_id = parse_uuid(&id, "messages.id")?;
            let current = required_message(&transaction, message_id)?;
            let attempt = current
                .attempts
                .last()
                .filter(|attempt| {
                    attempt.state == MessageDeliveryState::Delivering
                        && attempt.finished_at.is_none()
                })
                .ok_or(StoreError::InvalidRecord {
                    field: "message_delivery_attempts",
                    message: "delivering message has no active attempt".to_owned(),
                })?;
            let terminal = current.delivery.attempt_count >= current.delivery.max_attempts;
            let state = if terminal {
                MessageDeliveryState::TerminalFailed
            } else {
                MessageDeliveryState::RetryableFailed
            };
            let state_name = serialized_name(state)?;
            let next_attempt_at = if terminal {
                None
            } else {
                Some(
                    (now + Duration::seconds(
                        retry_delay_seconds(current.delivery.attempt_count)
                            .expect("remaining retry budget has a fixed delay"),
                    ))
                    .to_rfc3339(),
                )
            };
            transaction.execute(
                "UPDATE message_delivery_attempts SET
                    state = ?2, finished_at = ?3, failure_code = 'LZ-MESSAGE-OWNER-LOST',
                    failure_message = 'the daemon stopped while this delivery attempt was in flight'
                 WHERE id = ?1",
                params![attempt.id.to_string(), state_name, now_text],
            )?;
            transaction.execute(
                "UPDATE message_deliveries SET
                    state = ?2, revision = revision + 1, next_attempt_at = ?3,
                    last_failure_code = 'LZ-MESSAGE-OWNER-LOST',
                    last_failure_message = 'the daemon stopped while this delivery attempt was in flight',
                    terminal_at = ?4, updated_at = ?5
                 WHERE message_id = ?1",
                params![
                    message_id.to_string(),
                    state_name,
                    next_attempt_at,
                    terminal.then_some(now_text.clone()),
                    now_text,
                ],
            )?;
            if terminal {
                insert_dead_letter(
                    &transaction,
                    message_id,
                    attempt.id,
                    "LZ-MESSAGE-OWNER-LOST",
                    "the daemon stopped while this delivery attempt was in flight",
                    &now_text,
                )?;
            }
            messages.push(required_message(&transaction, message_id)?);
        }
        let event_sequence = employee::insert_event(
            &transaction,
            "messages.reconciled",
            &json!({"messages": &messages}),
            &now_text,
        )?;
        transaction.commit()?;
        Ok(MessageReconcileResult {
            messages,
            event_sequence: Some(event_sequence),
        })
    }

    pub fn list_messages(
        &self,
        after_sequence: u64,
        limit: u32,
    ) -> Result<Vec<MessageSummary>, StoreError> {
        let connection = self.lock()?;
        let after_sequence =
            i64::try_from(after_sequence).map_err(|_| StoreError::NegativeSequence)?;
        let mut statement = connection.prepare(&format!(
            "{} WHERE m.sequence > ?1 ORDER BY m.sequence LIMIT ?2",
            message_select()
        ))?;
        let rows = statement.query_map(params![after_sequence, i64::from(limit)], raw_message)?;
        rows.map(|row| message_summary_from_raw(row?)).collect()
    }

    pub fn get_message(&self, message_id: Uuid) -> Result<MessageRecord, StoreError> {
        let connection = self.lock()?;
        required_message(&connection, message_id)
    }
}

fn begin_attempt(
    transaction: &Transaction<'_>,
    current: &MessageRecord,
    scope: &str,
    idempotency_key: &str,
    request_id: &str,
    now: &str,
) -> Result<MessageMutationResult, StoreError> {
    let attempt_id = Uuid::new_v4();
    let attempt_number = current.delivery.attempt_count + 1;
    transaction.execute(
        "INSERT INTO message_delivery_attempts (
            id, message_id, attempt_number, state, started_at
         ) VALUES (?1, ?2, ?3, 'delivering', ?4)",
        params![
            attempt_id.to_string(),
            current.envelope.id.to_string(),
            i64::from(attempt_number),
            now,
        ],
    )?;
    transaction.execute(
        "UPDATE message_deliveries SET
            state = 'delivering', revision = revision + 1, attempt_count = ?2,
            next_attempt_at = NULL, terminal_at = NULL, updated_at = ?3
         WHERE message_id = ?1",
        params![
            current.envelope.id.to_string(),
            i64::from(attempt_number),
            now
        ],
    )?;
    transition_result(
        transaction,
        current.envelope.id,
        scope,
        idempotency_key,
        request_id,
        "message.delivering",
        now,
    )
}

fn transition_result(
    transaction: &Transaction<'_>,
    message_id: Uuid,
    scope: &str,
    idempotency_key: &str,
    request_id: &str,
    event_type: &str,
    now: &str,
) -> Result<MessageMutationResult, StoreError> {
    insert_message_command(
        transaction,
        scope,
        idempotency_key,
        request_id,
        message_id,
        now,
    )?;
    let message = required_message(transaction, message_id)?;
    let event_sequence =
        employee::insert_event(transaction, event_type, &json!({"message": &message}), now)?;
    Ok(MessageMutationResult {
        message,
        event_sequence: Some(event_sequence),
    })
}

fn commit_result(
    transaction: Transaction<'_>,
    result: MessageMutationResult,
) -> Result<MessageMutationResult, StoreError> {
    transaction.commit()?;
    Ok(result)
}

fn require_message_revision(
    message: &MessageRecord,
    expected_revision: u64,
) -> Result<(), StoreError> {
    if message.delivery.revision == expected_revision {
        Ok(())
    } else {
        Err(StoreError::MessageStale {
            message_id: message.envelope.id,
            current_revision: message.delivery.revision,
        })
    }
}

fn require_message_state(
    message: &MessageRecord,
    expected_states: &[MessageDeliveryState],
    expected: &'static str,
) -> Result<(), StoreError> {
    if expected_states.contains(&message.delivery.state) {
        Ok(())
    } else {
        Err(StoreError::MessageState {
            message_id: message.envelope.id,
            state: serialized_name(message.delivery.state)?,
            expected,
        })
    }
}

fn require_active_attempt(
    message: &MessageRecord,
    attempt_id: Uuid,
) -> Result<&MessageDeliveryAttempt, StoreError> {
    message
        .attempts
        .last()
        .filter(|attempt| {
            attempt.id == attempt_id
                && attempt.state == MessageDeliveryState::Delivering
                && attempt.finished_at.is_none()
        })
        .ok_or(StoreError::MessageAttempt {
            message_id: message.envelope.id,
            attempt_id,
        })
}

fn insert_dead_letter(
    transaction: &Transaction<'_>,
    message_id: Uuid,
    attempt_id: Uuid,
    failure_code: &str,
    failure_message: &str,
    now: &str,
) -> Result<(), StoreError> {
    transaction.execute(
        "INSERT INTO message_dead_letters (
            message_id, attempt_id, failure_code, failure_message, created_at
         ) VALUES (?1, ?2, ?3, ?4, ?5)",
        params![
            message_id.to_string(),
            attempt_id.to_string(),
            failure_code,
            failure_message,
            now
        ],
    )?;
    Ok(())
}

fn parse_timestamp(value: &str) -> Result<DateTime<Utc>, StoreError> {
    DateTime::parse_from_rfc3339(value)
        .map(|value| value.with_timezone(&Utc))
        .map_err(|error| StoreError::InvalidRecord {
            field: "message timestamp",
            message: error.to_string(),
        })
}

fn require_endpoint(
    connection: &Connection,
    endpoint: MessageEndpoint,
    role: &'static str,
) -> Result<(), StoreError> {
    let employee = employee::query_employee(connection, endpoint.employee_id)?.ok_or(
        StoreError::MessageEmployeeNotFound {
            role,
            employee_id: endpoint.employee_id,
        },
    )?;
    if !employee.enabled {
        return Err(StoreError::MessageEmployeeDisabled {
            role,
            employee_id: endpoint.employee_id,
        });
    }
    if let Some(session_id) = endpoint.session_id {
        let session = session::query_session(connection, session_id)?
            .ok_or(StoreError::MessageSessionNotFound { role, session_id })?;
        if session.employee_id != endpoint.employee_id {
            return Err(StoreError::MessageSessionEmployeeMismatch {
                role,
                session_id,
                employee_id: endpoint.employee_id,
            });
        }
    }
    Ok(())
}

fn require_project(connection: &Connection, project_id: Uuid) -> Result<(), StoreError> {
    let present = connection
        .query_row(
            "SELECT 1 FROM projects WHERE id = ?1",
            [project_id.to_string()],
            |row| row.get::<_, i64>(0),
        )
        .optional()?
        .is_some();
    if present {
        Ok(())
    } else {
        Err(StoreError::MessageProjectNotFound { project_id })
    }
}

fn resolve_correlation(
    connection: &Connection,
    message_id: Uuid,
    provided: Option<Uuid>,
    causation_id: Option<Uuid>,
) -> Result<Uuid, StoreError> {
    let Some(causation_id) = causation_id else {
        return Ok(provided.unwrap_or(message_id));
    };
    let cause = query_message_raw(connection, causation_id)?.ok_or(
        StoreError::MessageCausationNotFound {
            message_id: causation_id,
        },
    )?;
    let expected = parse_uuid(&cause.correlation_id, "messages.correlation_id")?;
    if let Some(provided) = provided
        && provided != expected
    {
        return Err(StoreError::MessageCorrelation { provided, expected });
    }
    Ok(expected)
}

fn insert_message_command(
    transaction: &Transaction<'_>,
    scope: &str,
    idempotency_key: &str,
    request_id: &str,
    message_id: Uuid,
    now: &str,
) -> Result<(), StoreError> {
    transaction.execute(
        "INSERT INTO message_commands (
            scope, idempotency_key, request_id, message_id, created_at
         ) VALUES (?1, ?2, ?3, ?4, ?5)",
        params![
            scope,
            idempotency_key,
            request_id,
            message_id.to_string(),
            now
        ],
    )?;
    Ok(())
}

fn message_for_command(
    connection: &Connection,
    scope: &str,
    idempotency_key: &str,
) -> Result<Option<MessageRecord>, StoreError> {
    let id = connection
        .query_row(
            "SELECT message_id FROM message_commands WHERE scope = ?1 AND idempotency_key = ?2",
            params![scope, idempotency_key],
            |row| row.get::<_, String>(0),
        )
        .optional()?;
    id.map(|id| required_message(connection, parse_uuid(&id, "message_commands.message_id")?))
        .transpose()
}

pub(crate) fn required_message(
    connection: &Connection,
    message_id: Uuid,
) -> Result<MessageRecord, StoreError> {
    let raw = query_message_raw(connection, message_id)?
        .ok_or(StoreError::MessageNotFound { message_id })?;
    message_from_raw(connection, raw)
}

fn query_message_raw(
    connection: &Connection,
    message_id: Uuid,
) -> Result<Option<RawMessage>, StoreError> {
    connection
        .query_row(
            &format!("{} WHERE m.id = ?1", message_select()),
            [message_id.to_string()],
            raw_message,
        )
        .optional()
        .map_err(StoreError::from)
}

fn message_select() -> &'static str {
    "SELECT
        m.sequence, m.id, m.envelope_version, m.idempotency_key, m.correlation_id,
        m.causation_id, m.sender_kind, m.sender_employee_id, m.sender_session_id,
        m.recipient_employee_id, m.recipient_session_id, m.project_id,
        m.conversation_id, m.task_id, m.kind, m.body_json, m.references_json,
        m.created_at, d.state, d.revision, d.attempt_count, d.max_attempts,
        d.next_attempt_at, d.last_failure_code, d.last_failure_message,
        d.acknowledged_at, d.terminal_at, d.updated_at
     FROM messages m
     JOIN message_deliveries d ON d.message_id = m.id"
}

#[derive(Debug)]
struct RawMessage {
    sequence: i64,
    id: String,
    envelope_version: i64,
    idempotency_key: String,
    correlation_id: String,
    causation_id: Option<String>,
    sender_kind: String,
    sender_employee_id: Option<String>,
    sender_session_id: Option<String>,
    recipient_employee_id: String,
    recipient_session_id: Option<String>,
    project_id: Option<String>,
    conversation_id: Option<String>,
    task_id: Option<String>,
    kind: String,
    body_json: String,
    references_json: String,
    created_at: String,
    state: String,
    revision: i64,
    attempt_count: i64,
    max_attempts: i64,
    next_attempt_at: Option<String>,
    last_failure_code: Option<String>,
    last_failure_message: Option<String>,
    acknowledged_at: Option<String>,
    terminal_at: Option<String>,
    updated_at: String,
}

fn raw_message(row: &rusqlite::Row<'_>) -> rusqlite::Result<RawMessage> {
    Ok(RawMessage {
        sequence: row.get(0)?,
        id: row.get(1)?,
        envelope_version: row.get(2)?,
        idempotency_key: row.get(3)?,
        correlation_id: row.get(4)?,
        causation_id: row.get(5)?,
        sender_kind: row.get(6)?,
        sender_employee_id: row.get(7)?,
        sender_session_id: row.get(8)?,
        recipient_employee_id: row.get(9)?,
        recipient_session_id: row.get(10)?,
        project_id: row.get(11)?,
        conversation_id: row.get(12)?,
        task_id: row.get(13)?,
        kind: row.get(14)?,
        body_json: row.get(15)?,
        references_json: row.get(16)?,
        created_at: row.get(17)?,
        state: row.get(18)?,
        revision: row.get(19)?,
        attempt_count: row.get(20)?,
        max_attempts: row.get(21)?,
        next_attempt_at: row.get(22)?,
        last_failure_code: row.get(23)?,
        last_failure_message: row.get(24)?,
        acknowledged_at: row.get(25)?,
        terminal_at: row.get(26)?,
        updated_at: row.get(27)?,
    })
}

fn message_from_raw(connection: &Connection, raw: RawMessage) -> Result<MessageRecord, StoreError> {
    let id = parse_uuid(&raw.id, "messages.id")?;
    let delivery = delivery_from_raw(&raw)?;
    let sender = sender_from_raw(&raw)?;
    let recipient_employee_id =
        parse_uuid(&raw.recipient_employee_id, "messages.recipient_employee_id")?;
    let recipient_session_id = optional_uuid(
        raw.recipient_session_id.as_deref(),
        "messages.recipient_session_id",
    )?;
    Ok(MessageRecord {
        sequence: unsigned(raw.sequence)?,
        envelope: MessageEnvelope {
            version: u32::try_from(raw.envelope_version)
                .map_err(|_| StoreError::NegativeSequence)?,
            id,
            idempotency_key: raw.idempotency_key,
            correlation_id: parse_uuid(&raw.correlation_id, "messages.correlation_id")?,
            causation_id: optional_uuid(raw.causation_id.as_deref(), "messages.causation_id")?,
            sender,
            recipient: MessageEndpoint {
                employee_id: recipient_employee_id,
                session_id: recipient_session_id,
            },
            project_id: optional_uuid(raw.project_id.as_deref(), "messages.project_id")?,
            conversation_id: optional_uuid(
                raw.conversation_id.as_deref(),
                "messages.conversation_id",
            )?,
            task_id: optional_uuid(raw.task_id.as_deref(), "messages.task_id")?,
            kind: serde_json::from_value(Value::String(raw.kind))?,
            body: serde_json::from_str(&raw.body_json)?,
            references: serde_json::from_str(&raw.references_json)?,
            created_at: raw.created_at,
        },
        activation_intent: MessageActivationIntent {
            employee_id: recipient_employee_id,
            session_id: recipient_session_id,
            requires_activation: recipient_session_id.is_none(),
        },
        delivery,
        attempts: query_attempts(connection, id)?,
        dead_letter: query_dead_letter(connection, id)?,
    })
}

fn message_summary_from_raw(raw: RawMessage) -> Result<MessageSummary, StoreError> {
    Ok(MessageSummary {
        sequence: unsigned(raw.sequence)?,
        id: parse_uuid(&raw.id, "messages.id")?,
        correlation_id: parse_uuid(&raw.correlation_id, "messages.correlation_id")?,
        causation_id: optional_uuid(raw.causation_id.as_deref(), "messages.causation_id")?,
        sender: sender_from_raw(&raw)?,
        recipient: MessageEndpoint {
            employee_id: parse_uuid(&raw.recipient_employee_id, "messages.recipient_employee_id")?,
            session_id: optional_uuid(
                raw.recipient_session_id.as_deref(),
                "messages.recipient_session_id",
            )?,
        },
        project_id: optional_uuid(raw.project_id.as_deref(), "messages.project_id")?,
        conversation_id: optional_uuid(raw.conversation_id.as_deref(), "messages.conversation_id")?,
        task_id: optional_uuid(raw.task_id.as_deref(), "messages.task_id")?,
        kind: serde_json::from_value(Value::String(raw.kind))?,
        state: serde_json::from_value(Value::String(raw.state))?,
        revision: unsigned(raw.revision)?,
        attempt_count: unsigned32(raw.attempt_count)?,
        max_attempts: unsigned32(raw.max_attempts)?,
        next_attempt_at: raw.next_attempt_at,
        created_at: raw.created_at,
        updated_at: raw.updated_at,
    })
}

#[derive(Clone, Debug)]
pub(crate) struct InsertMessageInput {
    pub request_id: String,
    pub command_scope: &'static str,
    pub idempotency_key: String,
    pub version: u32,
    pub sender: MessageSender,
    pub recipient: MessageEndpoint,
    pub project_id: Option<Uuid>,
    pub conversation_id: Option<Uuid>,
    pub task_id: Option<Uuid>,
    pub correlation_id: Option<Uuid>,
    pub causation_id: Option<Uuid>,
    pub kind: MessageKind,
    pub body: Value,
    pub references: Vec<MessageReference>,
    pub max_attempts: u32,
}

#[allow(clippy::too_many_lines)] // Keep validation and the complete record-first transaction together.
pub(crate) fn insert_message_tx(
    transaction: &Transaction<'_>,
    input: &InsertMessageInput,
) -> Result<MessageMutationResult, StoreError> {
    validate_message_command_key(&input.idempotency_key)?;
    validate_message_content(&input.body, &input.references)?;
    validate_max_attempts(input.max_attempts)?;
    validate_envelope_sender(input.version, &input.sender)?;
    if let Some(message) =
        message_for_command(transaction, input.command_scope, &input.idempotency_key)?
    {
        return Ok(MessageMutationResult {
            message,
            event_sequence: None,
        });
    }

    if let MessageSender::Employee(sender) = input.sender {
        require_endpoint(transaction, sender, "sender")?;
    }
    require_endpoint(transaction, input.recipient, "recipient")?;
    if let Some(project_id) = input.project_id {
        require_project(transaction, project_id)?;
    }
    if let Some(task_id) = input.task_id {
        let linked_task =
            task::required_task(transaction, task_id).map_err(|error| match error {
                StoreError::TaskNotFound { .. } => StoreError::MessageTaskNotFound { task_id },
                other => other,
            })?;
        if linked_task.project_id != input.project_id
            || Some(linked_task.conversation_id) != input.conversation_id
        {
            return Err(StoreError::MessageTaskScope { task_id });
        }
    }
    let (sender_kind, sender_employee_id, sender_session_id) = match input.sender {
        MessageSender::Employee(sender) => (
            "employee",
            Some(sender.employee_id.to_string()),
            sender.session_id.map(|value| value.to_string()),
        ),
        MessageSender::User(_) => ("user", None, None),
    };
    let message_id = Uuid::new_v4();
    let correlation_id = resolve_correlation(
        transaction,
        message_id,
        input.correlation_id,
        input.causation_id,
    )?;
    let now = Utc::now().to_rfc3339();
    transaction.execute(
        "INSERT INTO messages (
            id, envelope_version, idempotency_key, correlation_id, causation_id,
            sender_kind, sender_employee_id, sender_session_id, recipient_employee_id,
            recipient_session_id, project_id, conversation_id, task_id, kind,
            body_json, references_json, created_at
         ) VALUES (
            ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17
         )",
        params![
            message_id.to_string(),
            i64::from(input.version),
            input.idempotency_key,
            correlation_id.to_string(),
            input.causation_id.map(|value| value.to_string()),
            sender_kind,
            sender_employee_id,
            sender_session_id,
            input.recipient.employee_id.to_string(),
            input.recipient.session_id.map(|value| value.to_string()),
            input.project_id.map(|value| value.to_string()),
            input.conversation_id.map(|value| value.to_string()),
            input.task_id.map(|value| value.to_string()),
            serialized_name(input.kind)?,
            serde_json::to_string(&input.body)?,
            serde_json::to_string(&input.references)?,
            now,
        ],
    )?;
    transaction.execute(
        "INSERT INTO message_deliveries (
            message_id, state, revision, attempt_count, max_attempts,
            next_attempt_at, updated_at
         ) VALUES (?1, 'queued', 1, 0, ?2, ?3, ?3)",
        params![message_id.to_string(), i64::from(input.max_attempts), now],
    )?;
    insert_message_command(
        transaction,
        input.command_scope,
        &input.idempotency_key,
        &input.request_id,
        message_id,
        &now,
    )?;
    let message = required_message(transaction, message_id)?;
    let event_sequence = employee::insert_event(
        transaction,
        "message.queued",
        &json!({"message": &message}),
        &now,
    )?;
    Ok(MessageMutationResult {
        message,
        event_sequence: Some(event_sequence),
    })
}

pub(crate) fn claim_message_tx(
    transaction: &Transaction<'_>,
    input: &ClaimMessageInput,
) -> Result<MessageMutationResult, StoreError> {
    validate_message_command_key(&input.idempotency_key)?;
    if let Some(message) =
        message_for_command(transaction, "message.claim", &input.idempotency_key)?
    {
        return Ok(MessageMutationResult {
            message,
            event_sequence: None,
        });
    }
    let current = required_message(transaction, input.message_id)?;
    require_message_revision(&current, input.expected_revision)?;
    require_message_state(&current, &[MessageDeliveryState::Queued], "queued")?;
    begin_attempt(
        transaction,
        &current,
        "message.claim",
        &input.idempotency_key,
        &input.request_id,
        &Utc::now().to_rfc3339(),
    )
}

pub(crate) fn acknowledge_message_tx(
    transaction: &Transaction<'_>,
    input: &AcknowledgeMessageInput,
) -> Result<MessageMutationResult, StoreError> {
    validate_message_command_key(&input.idempotency_key)?;
    validate_acknowledgement(&input.acknowledgement_key, &input.evidence)?;
    if let Some(message) =
        message_for_command(transaction, "message.acknowledge", &input.idempotency_key)?
    {
        return Ok(MessageMutationResult {
            message,
            event_sequence: None,
        });
    }
    let current = required_message(transaction, input.message_id)?;
    require_message_revision(&current, input.expected_revision)?;
    require_message_state(&current, &[MessageDeliveryState::Delivering], "delivering")?;
    require_active_attempt(&current, input.attempt_id)?;
    let now = Utc::now().to_rfc3339();
    transaction.execute(
        "UPDATE message_delivery_attempts SET
            state = 'acknowledged', finished_at = ?2, acknowledgement_key = ?3,
            acknowledgement_evidence_json = ?4
         WHERE id = ?1",
        params![
            input.attempt_id.to_string(),
            now,
            input.acknowledgement_key,
            serde_json::to_string(&input.evidence)?,
        ],
    )?;
    transaction.execute(
        "UPDATE message_deliveries SET
            state = 'acknowledged', revision = revision + 1, next_attempt_at = NULL,
            acknowledged_at = ?2, terminal_at = ?2, updated_at = ?2
         WHERE message_id = ?1",
        params![input.message_id.to_string(), now],
    )?;
    transition_result(
        transaction,
        input.message_id,
        "message.acknowledge",
        &input.idempotency_key,
        &input.request_id,
        "message.acknowledged",
        &now,
    )
}

pub(crate) fn fail_message_tx(
    transaction: &Transaction<'_>,
    input: &FailMessageInput,
) -> Result<MessageMutationResult, StoreError> {
    validate_message_command_key(&input.idempotency_key)?;
    validate_message_failure(&input.failure_code, &input.failure_message)?;
    if let Some(message) = message_for_command(transaction, "message.fail", &input.idempotency_key)?
    {
        return Ok(MessageMutationResult {
            message,
            event_sequence: None,
        });
    }
    let current = required_message(transaction, input.message_id)?;
    require_message_revision(&current, input.expected_revision)?;
    require_message_state(&current, &[MessageDeliveryState::Delivering], "delivering")?;
    require_active_attempt(&current, input.attempt_id)?;
    let now_value = Utc::now();
    let terminal =
        !input.retryable || current.delivery.attempt_count >= current.delivery.max_attempts;
    let next_attempt_at = if terminal {
        None
    } else {
        Some(
            (now_value
                + Duration::seconds(
                    retry_delay_seconds(current.delivery.attempt_count)
                        .expect("remaining retry budget has a fixed delay"),
                ))
            .to_rfc3339(),
        )
    };
    let now = now_value.to_rfc3339();
    let state = if terminal {
        MessageDeliveryState::TerminalFailed
    } else {
        MessageDeliveryState::RetryableFailed
    };
    let state_name = serialized_name(state)?;
    transaction.execute(
        "UPDATE message_delivery_attempts SET
            state = ?2, finished_at = ?3, failure_code = ?4, failure_message = ?5
         WHERE id = ?1",
        params![
            input.attempt_id.to_string(),
            state_name,
            now,
            input.failure_code,
            input.failure_message,
        ],
    )?;
    transaction.execute(
        "UPDATE message_deliveries SET
            state = ?2, revision = revision + 1, next_attempt_at = ?3,
            last_failure_code = ?4, last_failure_message = ?5,
            terminal_at = ?6, updated_at = ?7
         WHERE message_id = ?1",
        params![
            input.message_id.to_string(),
            state_name,
            next_attempt_at,
            input.failure_code,
            input.failure_message,
            terminal.then_some(now.clone()),
            now,
        ],
    )?;
    if terminal {
        insert_dead_letter(
            transaction,
            input.message_id,
            input.attempt_id,
            &input.failure_code,
            &input.failure_message,
            &now,
        )?;
    }
    transition_result(
        transaction,
        input.message_id,
        "message.fail",
        &input.idempotency_key,
        &input.request_id,
        if terminal {
            "message.terminal-failed"
        } else {
            "message.retryable-failed"
        },
        &now,
    )
}

pub(crate) fn retry_message_tx(
    transaction: &Transaction<'_>,
    input: &RetryMessageInput,
) -> Result<MessageMutationResult, StoreError> {
    validate_message_command_key(&input.idempotency_key)?;
    if let Some(message) =
        message_for_command(transaction, "message.retry", &input.idempotency_key)?
    {
        return Ok(MessageMutationResult {
            message,
            event_sequence: None,
        });
    }
    let current = required_message(transaction, input.message_id)?;
    require_message_revision(&current, input.expected_revision)?;
    require_message_state(
        &current,
        &[MessageDeliveryState::RetryableFailed],
        "retryable-failed",
    )?;
    let next_attempt_at =
        current
            .delivery
            .next_attempt_at
            .as_deref()
            .ok_or_else(|| StoreError::InvalidRecord {
                field: "message_deliveries.next_attempt_at",
                message: "retryable message has no retry timestamp".to_owned(),
            })?;
    if parse_timestamp(next_attempt_at)? > Utc::now() {
        return Err(StoreError::MessageRetryNotDue {
            message_id: current.envelope.id,
            next_attempt_at: next_attempt_at.to_owned(),
        });
    }
    begin_attempt(
        transaction,
        &current,
        "message.retry",
        &input.idempotency_key,
        &input.request_id,
        &Utc::now().to_rfc3339(),
    )
}

pub(crate) fn cancel_message_tx(
    transaction: &Transaction<'_>,
    input: &CancelMessageInput,
) -> Result<MessageMutationResult, StoreError> {
    validate_message_command_key(&input.idempotency_key)?;
    if let Some(message) =
        message_for_command(transaction, "message.cancel", &input.idempotency_key)?
    {
        return Ok(MessageMutationResult {
            message,
            event_sequence: None,
        });
    }
    let current = required_message(transaction, input.message_id)?;
    require_message_revision(&current, input.expected_revision)?;
    require_message_state(
        &current,
        &[
            MessageDeliveryState::Queued,
            MessageDeliveryState::Delivering,
            MessageDeliveryState::RetryableFailed,
        ],
        "queued, delivering, or retryable-failed",
    )?;
    let now = Utc::now().to_rfc3339();
    if current.delivery.state == MessageDeliveryState::Delivering {
        let attempt = current
            .attempts
            .last()
            .filter(|attempt| {
                attempt.state == MessageDeliveryState::Delivering && attempt.finished_at.is_none()
            })
            .ok_or(StoreError::InvalidRecord {
                field: "message_delivery_attempts",
                message: "delivering message has no active attempt".to_owned(),
            })?;
        transaction.execute(
            "UPDATE message_delivery_attempts
             SET state = 'cancelled', finished_at = ?2
             WHERE id = ?1",
            params![attempt.id.to_string(), now],
        )?;
    }
    transaction.execute(
        "UPDATE message_deliveries SET
            state = 'cancelled', revision = revision + 1, next_attempt_at = NULL,
            terminal_at = ?2, updated_at = ?2
         WHERE message_id = ?1",
        params![input.message_id.to_string(), now],
    )?;
    transition_result(
        transaction,
        input.message_id,
        "message.cancel",
        &input.idempotency_key,
        &input.request_id,
        "message.cancelled",
        &now,
    )
}

fn sender_from_raw(raw: &RawMessage) -> Result<MessageSender, StoreError> {
    match raw.sender_kind.as_str() {
        "employee" => {
            let employee_id =
                raw.sender_employee_id
                    .as_deref()
                    .ok_or_else(|| StoreError::InvalidRecord {
                        field: "messages.sender_employee_id",
                        message: "employee sender is missing its employee id".to_owned(),
                    })?;
            Ok(MessageSender::Employee(MessageEndpoint {
                employee_id: parse_uuid(employee_id, "messages.sender_employee_id")?,
                session_id: optional_uuid(
                    raw.sender_session_id.as_deref(),
                    "messages.sender_session_id",
                )?,
            }))
        }
        "user" => Ok(MessageSender::User(UserMessageEndpoint {
            kind: UserMessageKind::User,
        })),
        other => Err(StoreError::InvalidRecord {
            field: "messages.sender_kind",
            message: format!("unknown message sender kind `{other}`"),
        }),
    }
}

fn delivery_from_raw(raw: &RawMessage) -> Result<MessageDelivery, StoreError> {
    Ok(MessageDelivery {
        state: serde_json::from_value(Value::String(raw.state.clone()))?,
        revision: unsigned(raw.revision)?,
        attempt_count: unsigned32(raw.attempt_count)?,
        max_attempts: unsigned32(raw.max_attempts)?,
        next_attempt_at: raw.next_attempt_at.clone(),
        last_failure_code: raw.last_failure_code.clone(),
        last_failure_message: raw.last_failure_message.clone(),
        acknowledged_at: raw.acknowledged_at.clone(),
        terminal_at: raw.terminal_at.clone(),
        updated_at: raw.updated_at.clone(),
    })
}

fn query_attempts(
    connection: &Connection,
    message_id: Uuid,
) -> Result<Vec<MessageDeliveryAttempt>, StoreError> {
    let mut statement = connection.prepare(
        "SELECT id, attempt_number, state, started_at, finished_at, failure_code,
                failure_message, acknowledgement_key, acknowledgement_evidence_json
         FROM message_delivery_attempts
         WHERE message_id = ?1
         ORDER BY attempt_number",
    )?;
    let rows = statement.query_map([message_id.to_string()], |row| {
        Ok(RawAttempt {
            id: row.get(0)?,
            number: row.get(1)?,
            state: row.get(2)?,
            started_at: row.get(3)?,
            finished_at: row.get(4)?,
            failure_code: row.get(5)?,
            failure_message: row.get(6)?,
            acknowledgement_key: row.get(7)?,
            acknowledgement_evidence_json: row.get(8)?,
        })
    })?;
    rows.map(|row| attempt_from_raw(row?)).collect()
}

struct RawAttempt {
    id: String,
    number: i64,
    state: String,
    started_at: String,
    finished_at: Option<String>,
    failure_code: Option<String>,
    failure_message: Option<String>,
    acknowledgement_key: Option<String>,
    acknowledgement_evidence_json: Option<String>,
}

fn attempt_from_raw(raw: RawAttempt) -> Result<MessageDeliveryAttempt, StoreError> {
    Ok(MessageDeliveryAttempt {
        id: parse_uuid(&raw.id, "message_delivery_attempts.id")?,
        number: unsigned32(raw.number)?,
        state: serde_json::from_value(Value::String(raw.state))?,
        started_at: raw.started_at,
        finished_at: raw.finished_at,
        failure_code: raw.failure_code,
        failure_message: raw.failure_message,
        acknowledgement_key: raw.acknowledgement_key,
        acknowledgement_evidence: raw
            .acknowledgement_evidence_json
            .map(|value| serde_json::from_str(&value))
            .transpose()?,
    })
}

fn query_dead_letter(
    connection: &Connection,
    message_id: Uuid,
) -> Result<Option<MessageDeadLetter>, StoreError> {
    connection
        .query_row(
            "SELECT attempt_id, failure_code, failure_message, created_at
             FROM message_dead_letters WHERE message_id = ?1",
            [message_id.to_string()],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                ))
            },
        )
        .optional()?
        .map(|(attempt_id, failure_code, failure_message, created_at)| {
            Ok(MessageDeadLetter {
                attempt_id: parse_uuid(&attempt_id, "message_dead_letters.attempt_id")?,
                failure_code,
                failure_message,
                created_at,
            })
        })
        .transpose()
}

fn serialized_name(value: impl Serialize) -> Result<String, StoreError> {
    serde_json::to_value(value)?
        .as_str()
        .map(str::to_owned)
        .ok_or_else(|| StoreError::InvalidRecord {
            field: "serialized enum",
            message: "expected string representation".to_owned(),
        })
}

fn optional_uuid(value: Option<&str>, field: &'static str) -> Result<Option<Uuid>, StoreError> {
    value.map(|value| parse_uuid(value, field)).transpose()
}

fn parse_uuid(value: &str, field: &'static str) -> Result<Uuid, StoreError> {
    employee::parse_uuid(value, field)
}

fn unsigned(value: i64) -> Result<u64, StoreError> {
    u64::try_from(value).map_err(|_| StoreError::NegativeSequence)
}

fn unsigned32(value: i64) -> Result<u32, StoreError> {
    u32::try_from(value).map_err(|_| StoreError::NegativeSequence)
}
