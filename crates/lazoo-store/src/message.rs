use chrono::Utc;
use lazoo_domain::{validate_max_attempts, validate_message_command_key, validate_message_content};
use lazoo_protocol::{
    MESSAGE_ENVELOPE_V1, MessageActivationIntent, MessageDeadLetter, MessageDelivery,
    MessageDeliveryAttempt, MessageEndpoint, MessageEnvelope, MessageKind, MessageMutationResult,
    MessageRecord, MessageReference, MessageSummary,
};
use rusqlite::{Connection, OptionalExtension, Transaction, TransactionBehavior, params};
use serde::Serialize;
use serde_json::{Value, json};
use uuid::Uuid;

use crate::{Store, StoreError, employee, session};

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

impl Store {
    pub fn send_message(
        &self,
        input: &SendMessageInput,
    ) -> Result<MessageMutationResult, StoreError> {
        validate_message_command_key(&input.idempotency_key)?;
        validate_message_content(&input.body, &input.references)?;
        validate_max_attempts(input.max_attempts)?;

        let mut connection = self.lock()?;
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        if let Some(message) =
            message_for_command(&transaction, "message.send", &input.idempotency_key)?
        {
            let result = MessageMutationResult {
                message,
                event_sequence: None,
            };
            transaction.commit()?;
            return Ok(result);
        }

        require_endpoint(&transaction, input.sender, "sender")?;
        require_endpoint(&transaction, input.recipient, "recipient")?;
        if let Some(project_id) = input.project_id {
            require_project(&transaction, project_id)?;
        }

        let message_id = Uuid::new_v4();
        let correlation_id = resolve_correlation(
            &transaction,
            message_id,
            input.correlation_id,
            input.causation_id,
        )?;
        let now = Utc::now().to_rfc3339();
        transaction.execute(
            "INSERT INTO messages (
                id, envelope_version, idempotency_key, correlation_id, causation_id,
                sender_employee_id, sender_session_id, recipient_employee_id,
                recipient_session_id, project_id, conversation_id, task_id, kind,
                body_json, references_json, created_at
             ) VALUES (
                ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16
             )",
            params![
                message_id.to_string(),
                i64::from(MESSAGE_ENVELOPE_V1),
                input.idempotency_key,
                correlation_id.to_string(),
                input.causation_id.map(|value| value.to_string()),
                input.sender.employee_id.to_string(),
                input.sender.session_id.map(|value| value.to_string()),
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
            &transaction,
            "message.send",
            &input.idempotency_key,
            &input.request_id,
            message_id,
            &now,
        )?;
        let message = required_message(&transaction, message_id)?;
        let event_sequence = employee::insert_event(
            &transaction,
            "message.queued",
            &json!({"message": &message}),
            &now,
        )?;
        let result = MessageMutationResult {
            message,
            event_sequence: Some(event_sequence),
        };
        transaction.commit()?;
        Ok(result)
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

fn required_message(
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
        m.causation_id, m.sender_employee_id, m.sender_session_id,
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
    sender_employee_id: String,
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
        sender_employee_id: row.get(6)?,
        sender_session_id: row.get(7)?,
        recipient_employee_id: row.get(8)?,
        recipient_session_id: row.get(9)?,
        project_id: row.get(10)?,
        conversation_id: row.get(11)?,
        task_id: row.get(12)?,
        kind: row.get(13)?,
        body_json: row.get(14)?,
        references_json: row.get(15)?,
        created_at: row.get(16)?,
        state: row.get(17)?,
        revision: row.get(18)?,
        attempt_count: row.get(19)?,
        max_attempts: row.get(20)?,
        next_attempt_at: row.get(21)?,
        last_failure_code: row.get(22)?,
        last_failure_message: row.get(23)?,
        acknowledged_at: row.get(24)?,
        terminal_at: row.get(25)?,
        updated_at: row.get(26)?,
    })
}

fn message_from_raw(connection: &Connection, raw: RawMessage) -> Result<MessageRecord, StoreError> {
    let id = parse_uuid(&raw.id, "messages.id")?;
    let delivery = delivery_from_raw(&raw)?;
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
            sender: MessageEndpoint {
                employee_id: parse_uuid(&raw.sender_employee_id, "messages.sender_employee_id")?,
                session_id: optional_uuid(
                    raw.sender_session_id.as_deref(),
                    "messages.sender_session_id",
                )?,
            },
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
        sender: MessageEndpoint {
            employee_id: parse_uuid(&raw.sender_employee_id, "messages.sender_employee_id")?,
            session_id: optional_uuid(
                raw.sender_session_id.as_deref(),
                "messages.sender_session_id",
            )?,
        },
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
