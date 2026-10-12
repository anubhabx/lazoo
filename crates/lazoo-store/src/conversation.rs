use chrono::Utc;
use lazoo_domain::{
    MAX_TURN_TRANSCRIPT_BYTES, derive_availability, effective_authority,
    validate_activation_request, validate_conversation_command_key,
    validate_conversation_list_limit, validate_conversation_title, validate_max_attempts,
    validate_message_failure, validate_task_objective, validate_turn_event_payload,
    validate_turn_text, validate_turn_transition,
};
use lazoo_protocol::{
    AuthorityMode, CONVERSATION_CONTRACT_V1, ConversationMutationResult, ConversationRecord,
    ConversationSummary, ConversationTurnEvent, ConversationTurnRecord, ConversationTurnSummary,
    EmployeeAvailability, MESSAGE_ENVELOPE_V2, MessageDeliveryState, MessageEndpoint, MessageKind,
    MessagePriority, MessageSender, TaskState, TurnMutationResult, TurnReconcileResult, TurnState,
};
use rusqlite::{Connection, OptionalExtension, Transaction, TransactionBehavior, params};
use serde::Serialize;
use serde_json::{Value, json};
use uuid::Uuid;

use crate::{Store, StoreError, delivery, employee, message, session, task};

#[derive(Clone, Debug)]
pub struct CreateConversationInput {
    pub request_id: String,
    pub idempotency_key: String,
    pub project_id: Option<Uuid>,
    pub lead_employee_id: Uuid,
    pub title: Option<String>,
}

#[derive(Clone, Debug)]
pub struct RecordTurnInput {
    pub request_id: String,
    pub idempotency_key: String,
    pub conversation_id: Uuid,
    pub author: MessageSender,
    pub recipient: Option<MessageEndpoint>,
    pub task_id: Option<Uuid>,
    pub authority_mode: AuthorityMode,
    pub text: String,
    pub activate_if_needed: bool,
    pub max_attempts: u32,
}

#[derive(Clone, Debug)]
pub struct BeginTurnSubmissionInput {
    pub request_id: String,
    pub idempotency_key: String,
    pub turn_id: Uuid,
    pub expected_revision: u64,
    pub owner_boot_id: Uuid,
    pub recipient_session_id: Uuid,
}

#[derive(Clone, Debug)]
pub struct ObserveTurnInput {
    pub request_id: String,
    pub idempotency_key: String,
    pub turn_id: Uuid,
    pub expected_revision: u64,
    pub kind: String,
    pub provider: String,
    pub native_session_id: Option<String>,
    pub native_turn_id: Option<String>,
    pub payload: Value,
    pub transition: Option<TurnState>,
}

#[derive(Clone, Debug)]
pub struct FailTurnInput {
    pub request_id: String,
    pub idempotency_key: String,
    pub turn_id: Uuid,
    pub expected_revision: u64,
    pub retryable: bool,
    pub failure_code: String,
    pub failure_message: String,
}

#[derive(Clone, Debug)]
pub struct RetryTurnInput {
    pub request_id: String,
    pub idempotency_key: String,
    pub turn_id: Uuid,
    pub expected_revision: u64,
    pub owner_boot_id: Uuid,
    pub recipient_session_id: Uuid,
}

#[derive(Clone, Debug)]
pub struct RequestTurnCancelInput {
    pub request_id: String,
    pub idempotency_key: String,
    pub turn_id: Uuid,
    pub expected_revision: u64,
}

#[derive(Clone, Debug)]
pub struct FinishTurnCancelInput {
    pub request_id: String,
    pub idempotency_key: String,
    pub turn_id: Uuid,
    pub expected_revision: u64,
    pub provider: String,
    pub native_session_id: Option<String>,
    pub native_turn_id: Option<String>,
    pub payload: Value,
}

#[derive(Clone, Debug)]
pub struct MarkTurnUncertainInput {
    pub request_id: String,
    pub idempotency_key: String,
    pub turn_id: Uuid,
    pub expected_revision: u64,
    pub failure_code: String,
    pub failure_message: String,
}

/// What the user renames a conversation to; `None` clears the stored title.
#[derive(Clone, Debug)]
pub struct RenameConversationInput {
    pub conversation_id: Uuid,
    pub expected_revision: u64,
    pub title: Option<String>,
}

impl Store {
    /// Renames a conversation (information architecture section 6.3: titles are user-editable).
    /// Naturally idempotent: a title the conversation already has changes nothing and emits no
    /// event; otherwise `expected_revision` must be current, and the revision advances.
    pub fn rename_conversation(
        &self,
        input: &RenameConversationInput,
    ) -> Result<ConversationMutationResult, StoreError> {
        let title = validate_conversation_title(input.title.as_deref())?;
        let mut connection = self.lock()?;
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let current = required_conversation(&transaction, input.conversation_id)?;
        if current.title == title {
            transaction.commit()?;
            return Ok(ConversationMutationResult {
                conversation: current,
                event_sequence: None,
            });
        }
        if current.revision != input.expected_revision {
            return Err(StoreError::ConversationStale {
                conversation_id: current.id,
                current_revision: current.revision,
            });
        }
        let now = Utc::now().to_rfc3339();
        transaction.execute(
            "UPDATE conversations SET title = ?1, revision = revision + 1, updated_at = ?2
             WHERE id = ?3",
            params![title, now, current.id.to_string()],
        )?;
        let conversation = required_conversation(&transaction, current.id)?;
        let event_sequence = employee::insert_event(
            &transaction,
            "conversation.renamed",
            &json!({"conversation": &conversation, "previousTitle": current.title}),
            &now,
        )?;
        transaction.commit()?;
        Ok(ConversationMutationResult {
            conversation,
            event_sequence: Some(event_sequence),
        })
    }

    pub fn create_conversation(
        &self,
        input: &CreateConversationInput,
    ) -> Result<ConversationMutationResult, StoreError> {
        validate_conversation_command_key(&input.idempotency_key)?;
        let title = validate_conversation_title(input.title.as_deref())?;
        let mut connection = self.lock()?;
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        if let Some(conversation) =
            conversation_for_command(&transaction, "conversation.create", &input.idempotency_key)?
        {
            transaction.commit()?;
            return Ok(ConversationMutationResult {
                conversation,
                event_sequence: None,
            });
        }
        let lead = require_route_ready_employee(&transaction, input.lead_employee_id)?;
        if let Some(project_id) = input.project_id {
            require_project(&transaction, project_id)?;
        }
        let now = Utc::now().to_rfc3339();
        let conversation_id = Uuid::new_v4();
        transaction.execute(
            "INSERT INTO conversations (
                id, contract_version, project_id, lead_employee_id,
                lead_employee_configuration_revision, title, revision, created_at, updated_at
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, 1, ?7, ?7)",
            params![
                conversation_id.to_string(),
                i64::from(CONVERSATION_CONTRACT_V1),
                input.project_id.map(|value| value.to_string()),
                lead.id.to_string(),
                to_i64(lead.configuration_revision)?,
                title,
                now,
            ],
        )?;
        insert_conversation_command(
            &transaction,
            &input.idempotency_key,
            &input.request_id,
            conversation_id,
            &now,
        )?;
        let conversation = required_conversation(&transaction, conversation_id)?;
        let event_sequence = employee::insert_event(
            &transaction,
            "conversation.created",
            &json!({"conversation": &conversation}),
            &now,
        )?;
        transaction.commit()?;
        Ok(ConversationMutationResult {
            conversation,
            event_sequence: Some(event_sequence),
        })
    }

    pub fn list_conversations(
        &self,
        after_sequence: u64,
        limit: u32,
    ) -> Result<Vec<ConversationSummary>, StoreError> {
        validate_conversation_list_limit(limit)?;
        let connection = self.lock()?;
        let mut statement = connection.prepare(
            "SELECT sequence, id, project_id, lead_employee_id, title, revision,
                    created_at, updated_at
             FROM conversations WHERE sequence > ?1 ORDER BY sequence LIMIT ?2",
        )?;
        let rows =
            statement.query_map(params![to_i64(after_sequence)?, i64::from(limit)], |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, Option<String>>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, Option<String>>(4)?,
                    row.get::<_, i64>(5)?,
                    row.get::<_, String>(6)?,
                    row.get::<_, String>(7)?,
                ))
            })?;
        rows.map(|row| {
            let row = row?;
            Ok(ConversationSummary {
                sequence: unsigned(row.0)?,
                id: parse_uuid(&row.1, "conversations.id")?,
                project_id: optional_uuid(row.2.as_deref(), "conversations.project_id")?,
                lead_employee_id: parse_uuid(&row.3, "conversations.lead_employee_id")?,
                title: row.4,
                revision: unsigned(row.5)?,
                created_at: row.6,
                updated_at: row.7,
            })
        })
        .collect()
    }

    pub fn get_conversation(&self, id: Uuid) -> Result<ConversationRecord, StoreError> {
        let connection = self.lock()?;
        required_conversation(&connection, id)
    }

    #[allow(clippy::too_many_lines)] // Keep the atomic message/turn/root-task write order visible.
    pub fn record_turn(&self, input: &RecordTurnInput) -> Result<TurnMutationResult, StoreError> {
        validate_conversation_command_key(&input.idempotency_key)?;
        let mut connection = self.lock()?;
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        if let Some(turn) = turn_for_command(&transaction, "turn.record", &input.idempotency_key)? {
            let result = turn_result(&transaction, turn, None)?;
            transaction.commit()?;
            return Ok(result);
        }

        validate_turn_text(&input.text)?;
        validate_activation_request(&input.author, input.activate_if_needed)?;
        validate_max_attempts(input.max_attempts)?;
        let conversation = required_conversation(&transaction, input.conversation_id)?;
        validate_author(&transaction, input.author)?;
        let existing_task = input
            .task_id
            .map(|task_id| task::required_task(&transaction, task_id))
            .transpose()?;
        let expected_recipient_employee_id = existing_task
            .as_ref()
            .map_or(conversation.lead_employee_id, |task| {
                task.assigned_employee_id
            });
        let recipient = resolve_recipient(
            &transaction,
            expected_recipient_employee_id,
            input.recipient,
        )?;
        let recipient_employee = require_route_ready_employee(&transaction, recipient.employee_id)?;
        let turn_id = Uuid::new_v4();
        let now = Utc::now().to_rfc3339();
        let linked_task = match (input.author, input.task_id) {
            (MessageSender::User(_), None) => {
                let objective = validate_task_objective(&input.text)?;
                let roster_revision: i64 = transaction.query_row(
                    "SELECT revision FROM roster_state WHERE singleton = 1",
                    [],
                    |row| row.get(0),
                )?;
                let task = task::insert_root_task_tx(
                    &transaction,
                    &task::InsertRootTaskInput {
                        id: turn_id,
                        project_id: conversation.project_id,
                        conversation_id: conversation.id,
                        assigned_employee_id: recipient.employee_id,
                        objective,
                        requested_authority: input.authority_mode,
                        effective_authority: effective_authority(
                            input.authority_mode,
                            input.authority_mode,
                            recipient_employee.authority_mode,
                        ),
                        employee_configuration_revision: recipient_employee.configuration_revision,
                        roster_revision: unsigned(roster_revision)?,
                        created_at: now.clone(),
                    },
                )?;
                Some(task)
            }
            (MessageSender::Employee(_), None) => return Err(StoreError::TurnTaskRequired),
            (_, Some(task_id)) => {
                let task = existing_task.ok_or(StoreError::TaskNotFound { task_id })?;
                require_turn_task_scope(
                    &transaction,
                    &task,
                    &conversation,
                    input.author,
                    recipient,
                )?;
                Some(task)
            }
        };
        let task_id = linked_task.as_ref().map(|task| task.id);
        let message_result = message::insert_message_tx(
            &transaction,
            &message::InsertMessageInput {
                request_id: input.request_id.clone(),
                command_scope: "turn.record",
                idempotency_key: input.idempotency_key.clone(),
                id: None,
                version: MESSAGE_ENVELOPE_V2,
                sender: input.author,
                recipient,
                project_id: conversation.project_id,
                conversation_id: Some(conversation.id),
                task_id,
                correlation_id: None,
                causation_id: None,
                kind: MessageKind::Peer,
                body: json!({"turnId": turn_id, "text": input.text}),
                references: Vec::new(),
                max_attempts: input.max_attempts,
                priority: MessagePriority::Normal,
            },
        )?;
        let (author_kind, author_employee_id, author_session_id) = sender_columns(input.author);
        transaction.execute(
            "INSERT INTO conversation_turns (
                id, contract_version, conversation_id, task_id, author_kind, author_employee_id,
                author_session_id, recipient_employee_id, recipient_session_id,
                recipient_employee_configuration_revision, text, message_id, correlation_id,
                state, revision, created_at, updated_at
             ) VALUES (
                ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13,
                'recorded', 1, ?14, ?14
             )",
            params![
                turn_id.to_string(),
                i64::from(CONVERSATION_CONTRACT_V1),
                conversation.id.to_string(),
                task_id.map(|value| value.to_string()),
                author_kind,
                author_employee_id,
                author_session_id,
                recipient.employee_id.to_string(),
                recipient.session_id.map(|value| value.to_string()),
                to_i64(recipient_employee.configuration_revision)?,
                input.text,
                message_result.message.envelope.id.to_string(),
                message_result.message.envelope.correlation_id.to_string(),
                now,
            ],
        )?;
        insert_turn_command(
            &transaction,
            "turn.record",
            &input.idempotency_key,
            &input.request_id,
            turn_id,
            &now,
        )?;
        let turn = required_turn(&transaction, turn_id)?;
        if linked_task.as_ref().is_some_and(|task| task.id == turn_id) {
            task::insert_root_evidence_tx(
                &transaction,
                turn_id,
                message_result.message.envelope.id,
                &now,
            )?;
            employee::insert_event(
                &transaction,
                "task.created",
                &json!({"task": linked_task.as_ref().expect("root task exists")}),
                &now,
            )?;
        }
        let event_sequence = employee::insert_event(
            &transaction,
            "turn.recorded",
            &json!({"turn": &turn, "message": &message_result.message}),
            &now,
        )?;
        let result = TurnMutationResult {
            turn,
            message: message_result.message,
            task: linked_task,
            event_sequence: Some(event_sequence),
        };
        transaction.commit()?;
        Ok(result)
    }

    pub fn list_turns(
        &self,
        conversation_id: Uuid,
        after_sequence: u64,
        limit: u32,
    ) -> Result<Vec<ConversationTurnSummary>, StoreError> {
        validate_conversation_list_limit(limit)?;
        let connection = self.lock()?;
        required_conversation(&connection, conversation_id)?;
        let mut statement = connection.prepare(
            "SELECT sequence, id, conversation_id, task_id, author_kind, author_employee_id,
                    author_session_id, recipient_employee_id, recipient_session_id, state,
                    revision, native_turn_id, created_at, updated_at
             FROM conversation_turns
             WHERE conversation_id = ?1 AND sequence > ?2 ORDER BY sequence LIMIT ?3",
        )?;
        let rows = statement.query_map(
            params![
                conversation_id.to_string(),
                to_i64(after_sequence)?,
                i64::from(limit)
            ],
            raw_turn_summary,
        )?;
        rows.map(|row| turn_summary_from_raw(row?)).collect()
    }

    pub fn get_turn(&self, id: Uuid) -> Result<ConversationTurnRecord, StoreError> {
        let connection = self.lock()?;
        required_turn(&connection, id)
    }

    pub fn begin_turn_submission(
        &self,
        input: &BeginTurnSubmissionInput,
    ) -> Result<TurnMutationResult, StoreError> {
        validate_conversation_command_key(&input.idempotency_key)?;
        let mut connection = self.lock()?;
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        if let Some(turn) = turn_for_command(
            &transaction,
            "turn.begin-submission",
            &input.idempotency_key,
        )? {
            let result = turn_result(&transaction, turn, None)?;
            transaction.commit()?;
            return Ok(result);
        }
        let current = required_turn(&transaction, input.turn_id)?;
        require_turn_revision(&current, input.expected_revision)?;
        require_turn_state(&current, &[TurnState::Recorded], "recorded")?;
        let owned_session = require_owned_idle_session(
            &transaction,
            &current,
            input.recipient_session_id,
            input.owner_boot_id,
        )?;
        let message_result = message::claim_message_tx(
            &transaction,
            &message::ClaimMessageInput {
                request_id: input.request_id.clone(),
                idempotency_key: message_key(input.turn_id, &input.idempotency_key),
                message_id: current.message_id,
                expected_revision: 1,
            },
        )?;
        let attempt_id = message_result
            .message
            .attempts
            .last()
            .ok_or(StoreError::InvalidRecord {
                field: "message_delivery_attempts",
                message: "claimed message has no delivery attempt".to_owned(),
            })?
            .id;
        for companion in companions_in(&transaction, &current, &[MessageDeliveryState::Queued])? {
            message::claim_message_tx(
                &transaction,
                &message::ClaimMessageInput {
                    request_id: input.request_id.clone(),
                    idempotency_key: companion_key(
                        input.turn_id,
                        &input.idempotency_key,
                        companion.envelope.id,
                    ),
                    message_id: companion.envelope.id,
                    expected_revision: companion.delivery.revision,
                },
            )?;
        }
        let now = Utc::now().to_rfc3339();
        transaction.execute(
            "UPDATE conversation_turns SET
                state = 'submitting', revision = revision + 1, owner_boot_id = ?2,
                recipient_session_id = ?3, message_attempt_id = ?4, provider = ?5,
                native_session_id = ?6, submitted_at = ?7, updated_at = ?7
             WHERE id = ?1",
            params![
                input.turn_id.to_string(),
                input.owner_boot_id.to_string(),
                input.recipient_session_id.to_string(),
                attempt_id.to_string(),
                owned_session.provider,
                owned_session.native_session_id,
                now,
            ],
        )?;
        effective_turn_result(
            &transaction,
            "turn.begin-submission",
            &input.idempotency_key,
            &input.request_id,
            input.turn_id,
            "turn.submitting",
            &now,
        )
        .and_then(|result| commit_turn_result(transaction, result))
    }

    pub fn observe_turn(&self, input: &ObserveTurnInput) -> Result<TurnMutationResult, StoreError> {
        validate_conversation_command_key(&input.idempotency_key)?;
        validate_turn_event_payload(&input.payload)?;
        let mut connection = self.lock()?;
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        if let Some(turn) = turn_for_command(&transaction, "turn.observe", &input.idempotency_key)?
        {
            let result = turn_result(&transaction, turn, None)?;
            transaction.commit()?;
            return Ok(result);
        }
        let current = required_turn(&transaction, input.turn_id)?;
        require_turn_revision(&current, input.expected_revision)?;
        require_turn_state(
            &current,
            &[TurnState::Submitting, TurnState::Accepted],
            "submitting or accepted",
        )?;
        let target = input.transition.unwrap_or(current.state);
        if target != current.state {
            validate_turn_transition(current.state, target)?;
        }
        if current.state == TurnState::Submitting
            && target == TurnState::Accepted
            && input.native_turn_id.as_deref().is_none_or(str::is_empty)
            && input.payload.get("progress").and_then(Value::as_bool) != Some(true)
        {
            return Err(StoreError::TurnAcceptanceEvidence {
                turn_id: input.turn_id,
            });
        }

        if current.state == TurnState::Submitting && target == TurnState::Accepted {
            let attempt_id = current
                .message_attempt_id
                .ok_or(StoreError::InvalidRecord {
                    field: "conversation_turns.message_attempt_id",
                    message: "submitting turn has no message attempt".to_owned(),
                })?;
            let message_record = message::required_message(&transaction, current.message_id)?;
            message::acknowledge_message_tx(
                &transaction,
                &message::AcknowledgeMessageInput {
                    request_id: input.request_id.clone(),
                    idempotency_key: message_key(input.turn_id, &input.idempotency_key),
                    message_id: current.message_id,
                    expected_revision: message_record.delivery.revision,
                    attempt_id,
                    acknowledgement_key: input
                        .native_turn_id
                        .clone()
                        .unwrap_or_else(|| format!("progress:{}", input.turn_id)),
                    evidence: input.payload.clone(),
                },
            )?;
            acknowledge_companions(
                &transaction,
                &current,
                &input.request_id,
                &input.idempotency_key,
            )?;
        }

        let now = Utc::now().to_rfc3339();
        let appended = append_turn_event(
            &transaction,
            &current,
            &input.kind,
            &input.provider,
            input.native_session_id.as_deref(),
            input.native_turn_id.as_deref(),
            &input.payload,
            input.transition.is_some(),
            &now,
        )?;
        update_observed_turn(
            &transaction,
            &current,
            input,
            target,
            appended.truncated,
            &now,
        )?;
        task::project_turn_tx(&transaction, input.turn_id, &input.kind, &now)?;
        effective_turn_result(
            &transaction,
            "turn.observe",
            &input.idempotency_key,
            &input.request_id,
            input.turn_id,
            event_type_for_state(target, "turn.observed"),
            &now,
        )
        .and_then(|result| commit_turn_result(transaction, result))
    }

    pub fn fail_turn(&self, input: &FailTurnInput) -> Result<TurnMutationResult, StoreError> {
        validate_conversation_command_key(&input.idempotency_key)?;
        validate_message_failure(&input.failure_code, &input.failure_message)?;
        let mut connection = self.lock()?;
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        if let Some(turn) = turn_for_command(&transaction, "turn.fail", &input.idempotency_key)? {
            let result = turn_result(&transaction, turn, None)?;
            transaction.commit()?;
            return Ok(result);
        }
        let current = required_turn(&transaction, input.turn_id)?;
        require_turn_revision(&current, input.expected_revision)?;
        require_turn_state(&current, &[TurnState::Submitting], "submitting")?;
        let message_record = message::required_message(&transaction, current.message_id)?;
        let attempt_id = current
            .message_attempt_id
            .ok_or(StoreError::InvalidRecord {
                field: "conversation_turns.message_attempt_id",
                message: "submitting turn has no message attempt".to_owned(),
            })?;
        let message_result = message::fail_message_tx(
            &transaction,
            &message::FailMessageInput {
                request_id: input.request_id.clone(),
                idempotency_key: message_key(input.turn_id, &input.idempotency_key),
                message_id: current.message_id,
                expected_revision: message_record.delivery.revision,
                attempt_id,
                retryable: input.retryable,
                failure_code: input.failure_code.clone(),
                failure_message: input.failure_message.clone(),
            },
        )?;
        for companion in companions_in(&transaction, &current, &[MessageDeliveryState::Delivering])?
        {
            message::fail_message_tx(
                &transaction,
                &message::FailMessageInput {
                    request_id: input.request_id.clone(),
                    idempotency_key: companion_key(
                        input.turn_id,
                        &input.idempotency_key,
                        companion.envelope.id,
                    ),
                    message_id: companion.envelope.id,
                    expected_revision: companion.delivery.revision,
                    attempt_id: active_attempt_id(&companion)?,
                    retryable: input.retryable,
                    failure_code: input.failure_code.clone(),
                    failure_message: input.failure_message.clone(),
                },
            )?;
        }
        let target = match message_result.message.delivery.state {
            MessageDeliveryState::RetryableFailed => TurnState::RetryableFailed,
            MessageDeliveryState::TerminalFailed => TurnState::TerminalFailed,
            _ => {
                return Err(StoreError::InvalidRecord {
                    field: "message_deliveries.state",
                    message: "failed turn did not produce a failed message".to_owned(),
                });
            }
        };
        validate_turn_transition(current.state, target)?;
        let now = Utc::now().to_rfc3339();
        transaction.execute(
            "UPDATE conversation_turns SET
                state = ?2, revision = revision + 1, owner_boot_id = NULL,
                failure_code = ?3, failure_message = ?4, updated_at = ?5,
                terminal_at = ?6
             WHERE id = ?1",
            params![
                input.turn_id.to_string(),
                serialized_name(target)?,
                input.failure_code,
                input.failure_message,
                now,
                (target == TurnState::TerminalFailed).then_some(now.clone()),
            ],
        )?;
        task::project_turn_tx(&transaction, input.turn_id, "turn-failed", &now)?;
        effective_turn_result(
            &transaction,
            "turn.fail",
            &input.idempotency_key,
            &input.request_id,
            input.turn_id,
            event_type_for_state(target, "turn.failed"),
            &now,
        )
        .and_then(|result| commit_turn_result(transaction, result))
    }

    pub fn retry_turn(&self, input: &RetryTurnInput) -> Result<TurnMutationResult, StoreError> {
        validate_conversation_command_key(&input.idempotency_key)?;
        let mut connection = self.lock()?;
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        if let Some(turn) = turn_for_command(&transaction, "turn.retry", &input.idempotency_key)? {
            let result = turn_result(&transaction, turn, None)?;
            transaction.commit()?;
            return Ok(result);
        }
        let current = required_turn(&transaction, input.turn_id)?;
        require_turn_revision(&current, input.expected_revision)?;
        require_turn_state(&current, &[TurnState::RetryableFailed], "retryable-failed")?;
        validate_turn_transition(current.state, TurnState::Recorded)?;
        let owned_session = require_owned_idle_session(
            &transaction,
            &current,
            input.recipient_session_id,
            input.owner_boot_id,
        )?;
        let message_record = message::required_message(&transaction, current.message_id)?;
        let message_result = message::retry_message_tx(
            &transaction,
            &message::RetryMessageInput {
                request_id: input.request_id.clone(),
                idempotency_key: message_key(input.turn_id, &input.idempotency_key),
                message_id: current.message_id,
                expected_revision: message_record.delivery.revision,
            },
        )?;
        let attempt_id = message_result.message.attempts.last().unwrap().id;
        for companion in companions_in(
            &transaction,
            &current,
            &[MessageDeliveryState::RetryableFailed],
        )? {
            message::retry_message_tx(
                &transaction,
                &message::RetryMessageInput {
                    request_id: input.request_id.clone(),
                    idempotency_key: companion_key(
                        input.turn_id,
                        &input.idempotency_key,
                        companion.envelope.id,
                    ),
                    message_id: companion.envelope.id,
                    expected_revision: companion.delivery.revision,
                },
            )?;
        }
        let now = Utc::now().to_rfc3339();
        transaction.execute(
            "UPDATE conversation_turns SET
                state = 'submitting', revision = revision + 1, owner_boot_id = ?2,
                recipient_session_id = ?3, message_attempt_id = ?4, provider = ?5,
                native_session_id = ?6, native_turn_id = NULL, failure_code = NULL,
                failure_message = NULL, submitted_at = ?7, updated_at = ?7, terminal_at = NULL
             WHERE id = ?1",
            params![
                input.turn_id.to_string(),
                input.owner_boot_id.to_string(),
                input.recipient_session_id.to_string(),
                attempt_id.to_string(),
                owned_session.provider,
                owned_session.native_session_id,
                now,
            ],
        )?;
        effective_turn_result(
            &transaction,
            "turn.retry",
            &input.idempotency_key,
            &input.request_id,
            input.turn_id,
            "turn.submitting",
            &now,
        )
        .and_then(|result| commit_turn_result(transaction, result))
    }

    pub fn request_turn_cancel(
        &self,
        input: &RequestTurnCancelInput,
    ) -> Result<TurnMutationResult, StoreError> {
        validate_conversation_command_key(&input.idempotency_key)?;
        let mut connection = self.lock()?;
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        if let Some(turn) =
            turn_for_command(&transaction, "turn.request-cancel", &input.idempotency_key)?
        {
            let result = turn_result(&transaction, turn, None)?;
            transaction.commit()?;
            return Ok(result);
        }
        let current = required_turn(&transaction, input.turn_id)?;
        require_turn_revision(&current, input.expected_revision)?;
        require_turn_state(
            &current,
            &[
                TurnState::Recorded,
                TurnState::Submitting,
                TurnState::Accepted,
                TurnState::RetryableFailed,
            ],
            "recorded, submitting, accepted, or retryable-failed",
        )?;
        let now = Utc::now().to_rfc3339();
        if matches!(
            current.state,
            TurnState::Recorded | TurnState::RetryableFailed
        ) {
            let message_record = message::required_message(&transaction, current.message_id)?;
            message::cancel_message_tx(
                &transaction,
                &message::CancelMessageInput {
                    request_id: input.request_id.clone(),
                    idempotency_key: message_key(input.turn_id, &input.idempotency_key),
                    message_id: current.message_id,
                    expected_revision: message_record.delivery.revision,
                },
            )?;
            cancel_companions(
                &transaction,
                &current,
                &input.request_id,
                &input.idempotency_key,
            )?;
            validate_turn_transition(current.state, TurnState::Cancelled)?;
            transaction.execute(
                "UPDATE conversation_turns SET
                    state = 'cancelled', revision = revision + 1, owner_boot_id = NULL,
                    cancel_requested_at = ?2, terminal_at = ?2, updated_at = ?2
                 WHERE id = ?1",
                params![input.turn_id.to_string(), now],
            )?;
        } else {
            transaction.execute(
                "UPDATE conversation_turns SET
                    revision = revision + 1, cancel_requested_at = ?2, updated_at = ?2
                 WHERE id = ?1",
                params![input.turn_id.to_string(), now],
            )?;
        }
        task::project_turn_tx(&transaction, input.turn_id, "turn-cancel-requested", &now)?;
        effective_turn_result(
            &transaction,
            "turn.request-cancel",
            &input.idempotency_key,
            &input.request_id,
            input.turn_id,
            "turn.cancel-requested",
            &now,
        )
        .and_then(|result| commit_turn_result(transaction, result))
    }

    pub fn finish_turn_cancel(
        &self,
        input: &FinishTurnCancelInput,
    ) -> Result<TurnMutationResult, StoreError> {
        validate_conversation_command_key(&input.idempotency_key)?;
        validate_turn_event_payload(&input.payload)?;
        let mut connection = self.lock()?;
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        if let Some(turn) =
            turn_for_command(&transaction, "turn.finish-cancel", &input.idempotency_key)?
        {
            let result = turn_result(&transaction, turn, None)?;
            transaction.commit()?;
            return Ok(result);
        }
        let current = required_turn(&transaction, input.turn_id)?;
        require_turn_revision(&current, input.expected_revision)?;
        require_turn_state(
            &current,
            &[TurnState::Submitting, TurnState::Accepted],
            "submitting or accepted",
        )?;
        if current.cancel_requested_at.is_none() {
            return Err(StoreError::TurnCancelNotRequested {
                turn_id: input.turn_id,
            });
        }
        validate_turn_transition(current.state, TurnState::Cancelled)?;
        let message_record = message::required_message(&transaction, current.message_id)?;
        if message_record.delivery.state != MessageDeliveryState::Acknowledged {
            message::cancel_message_tx(
                &transaction,
                &message::CancelMessageInput {
                    request_id: input.request_id.clone(),
                    idempotency_key: message_key(input.turn_id, &input.idempotency_key),
                    message_id: current.message_id,
                    expected_revision: message_record.delivery.revision,
                },
            )?;
        }
        cancel_companions(
            &transaction,
            &current,
            &input.request_id,
            &input.idempotency_key,
        )?;
        let now = Utc::now().to_rfc3339();
        let append = append_turn_event(
            &transaction,
            &current,
            "turn-cancelled",
            &input.provider,
            input.native_session_id.as_deref(),
            input.native_turn_id.as_deref(),
            &input.payload,
            true,
            &now,
        )?;
        transaction.execute(
            "UPDATE conversation_turns SET
                state = 'cancelled', revision = revision + 1, owner_boot_id = NULL,
                native_session_id = COALESCE(?2, native_session_id),
                native_turn_id = COALESCE(?3, native_turn_id), transcript_truncated = ?4,
                terminal_at = ?5, updated_at = ?5
             WHERE id = ?1",
            params![
                input.turn_id.to_string(),
                input.native_session_id,
                input.native_turn_id,
                i64::from(append.truncated),
                now,
            ],
        )?;
        task::project_turn_tx(&transaction, input.turn_id, "turn-cancelled", &now)?;
        effective_turn_result(
            &transaction,
            "turn.finish-cancel",
            &input.idempotency_key,
            &input.request_id,
            input.turn_id,
            "turn.cancelled",
            &now,
        )
        .and_then(|result| commit_turn_result(transaction, result))
    }

    pub fn mark_turn_uncertain(
        &self,
        input: &MarkTurnUncertainInput,
    ) -> Result<TurnMutationResult, StoreError> {
        validate_conversation_command_key(&input.idempotency_key)?;
        validate_message_failure(&input.failure_code, &input.failure_message)?;
        let mut connection = self.lock()?;
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        if let Some(turn) =
            turn_for_command(&transaction, "turn.uncertain", &input.idempotency_key)?
        {
            let result = turn_result(&transaction, turn, None)?;
            transaction.commit()?;
            return Ok(result);
        }
        let current = required_turn(&transaction, input.turn_id)?;
        require_turn_revision(&current, input.expected_revision)?;
        require_turn_state(
            &current,
            &[TurnState::Submitting, TurnState::Accepted],
            "submitting or accepted",
        )?;
        validate_turn_transition(current.state, TurnState::Uncertain)?;
        let now = Utc::now().to_rfc3339();
        transaction.execute(
            "UPDATE conversation_turns SET
                state = 'uncertain', revision = revision + 1, owner_boot_id = NULL,
                failure_code = ?2, failure_message = ?3, terminal_at = ?4, updated_at = ?4
             WHERE id = ?1",
            params![
                input.turn_id.to_string(),
                input.failure_code,
                input.failure_message,
                now,
            ],
        )?;
        task::project_turn_tx(&transaction, input.turn_id, "turn-uncertain", &now)?;
        effective_turn_result(
            &transaction,
            "turn.uncertain",
            &input.idempotency_key,
            &input.request_id,
            input.turn_id,
            "turn.uncertain",
            &now,
        )
        .and_then(|result| commit_turn_result(transaction, result))
    }

    pub fn reconcile_turns(
        &self,
        current_boot_id: Uuid,
    ) -> Result<TurnReconcileResult, StoreError> {
        let mut connection = self.lock()?;
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let mut statement = transaction.prepare(
            "SELECT id FROM conversation_turns
             WHERE state IN ('submitting', 'accepted')
               AND (owner_boot_id IS NULL OR owner_boot_id <> ?1)
             ORDER BY sequence",
        )?;
        let ids = statement
            .query_map([current_boot_id.to_string()], |row| row.get::<_, String>(0))?
            .collect::<Result<Vec<_>, _>>()?;
        drop(statement);
        if ids.is_empty() {
            transaction.commit()?;
            return Ok(TurnReconcileResult {
                turns: Vec::new(),
                event_sequence: None,
            });
        }
        let now = Utc::now().to_rfc3339();
        let mut turns = Vec::with_capacity(ids.len());
        for id in ids {
            let id = parse_uuid(&id, "conversation_turns.id")?;
            transaction.execute(
                "UPDATE conversation_turns SET
                    state = 'uncertain', revision = revision + 1, owner_boot_id = NULL,
                    failure_code = 'LZ-TURN-OWNER-LOST',
                    failure_message = 'the daemon stopped while this provider turn was in flight',
                    terminal_at = ?2, updated_at = ?2
                 WHERE id = ?1",
                params![id.to_string(), now],
            )?;
            turns.push(required_turn(&transaction, id)?);
        }
        let event_sequence = employee::insert_event(
            &transaction,
            "turns.reconciled",
            &json!({"turns": &turns}),
            &now,
        )?;
        transaction.commit()?;
        Ok(TurnReconcileResult {
            turns,
            event_sequence: Some(event_sequence),
        })
    }
}

pub(crate) fn require_route_ready_employee(
    connection: &Connection,
    employee_id: Uuid,
) -> Result<lazoo_protocol::EmployeeRecord, StoreError> {
    let employee = employee::query_employee(connection, employee_id)?
        .ok_or(StoreError::EmployeeNotFound { employee_id })?;
    if !employee.enabled {
        return Err(StoreError::EmployeeDisabled { employee_id });
    }
    let observation = employee::query_provider_observation(connection, &employee.provider)?;
    let (availability, reason) = derive_availability(true, observation.as_ref());
    if availability != EmployeeAvailability::Dormant {
        return Err(StoreError::SessionProviderNotReady {
            employee_id,
            reason: reason.unwrap_or_else(|| "provider is not route ready".to_owned()),
        });
    }
    Ok(employee)
}

fn require_project(connection: &Connection, project_id: Uuid) -> Result<(), StoreError> {
    let exists = connection
        .query_row(
            "SELECT 1 FROM projects WHERE id = ?1",
            [project_id.to_string()],
            |row| row.get::<_, i64>(0),
        )
        .optional()?
        .is_some();
    if exists {
        Ok(())
    } else {
        Err(StoreError::MessageProjectNotFound { project_id })
    }
}

fn resolve_recipient(
    connection: &Connection,
    expected_employee_id: Uuid,
    provided: Option<MessageEndpoint>,
) -> Result<MessageEndpoint, StoreError> {
    let recipient = provided.unwrap_or(MessageEndpoint {
        employee_id: expected_employee_id,
        session_id: session::active_session_id_for_employee(connection, expected_employee_id)?,
    });
    if recipient.employee_id != expected_employee_id {
        return Err(StoreError::TurnRecipient {
            turn_recipient: recipient.employee_id,
            lead_employee_id: expected_employee_id,
        });
    }
    validate_endpoint(connection, recipient, "recipient")?;
    Ok(recipient)
}

fn require_turn_task_scope(
    connection: &Connection,
    task: &lazoo_protocol::TaskRecord,
    conversation: &ConversationRecord,
    author: MessageSender,
    recipient: MessageEndpoint,
) -> Result<(), StoreError> {
    let same_scope = task.conversation_id == conversation.id
        && task.project_id == conversation.project_id
        && task.assigned_employee_id == recipient.employee_id;
    let author_matches = match author {
        MessageSender::User(_) => true,
        MessageSender::Employee(endpoint) => {
            endpoint.employee_id == task.assigned_employee_id || author == task.creator
        }
    };
    let nonterminal = !matches!(
        task.state,
        TaskState::Completed | TaskState::Failed | TaskState::Cancelled
    );
    if same_scope
        && author_matches
        && nonterminal
        && task::require_valid_ancestry(connection, task).is_ok()
    {
        Ok(())
    } else {
        Err(StoreError::TurnTaskScope { task_id: task.id })
    }
}

fn validate_author(connection: &Connection, author: MessageSender) -> Result<(), StoreError> {
    if let MessageSender::Employee(endpoint) = author {
        validate_endpoint(connection, endpoint, "author")?;
    }
    Ok(())
}

fn validate_endpoint(
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

fn require_owned_idle_session(
    connection: &Connection,
    turn: &ConversationTurnRecord,
    session_id: Uuid,
    owner_boot_id: Uuid,
) -> Result<lazoo_protocol::EmployeeSessionRecord, StoreError> {
    let session = session::query_session(connection, session_id)?
        .ok_or(StoreError::SessionNotFound { session_id })?;
    if session.employee_id != turn.recipient.employee_id {
        return Err(StoreError::MessageSessionEmployeeMismatch {
            role: "recipient",
            session_id,
            employee_id: turn.recipient.employee_id,
        });
    }
    if session.employee_configuration_revision != turn.recipient_employee_configuration_revision {
        return Err(StoreError::EmployeeStale {
            employee_id: turn.recipient.employee_id,
            current_revision: session.employee_configuration_revision,
        });
    }
    if session.state != lazoo_protocol::SessionLifecycleState::Idle {
        return Err(StoreError::SessionState {
            session_id,
            state: serialized_name(session.state)?,
            expected: "idle",
        });
    }
    if session.owner_boot_id != Some(owner_boot_id) {
        return Err(StoreError::SessionOwnership { session_id });
    }
    if session.native_session_id.is_none() {
        return Err(StoreError::InvalidRecord {
            field: "employee_sessions.native_session_id",
            message: "idle session is missing its native session id".to_owned(),
        });
    }
    Ok(session)
}

pub(crate) fn sender_columns(
    sender: MessageSender,
) -> (&'static str, Option<String>, Option<String>) {
    match sender {
        MessageSender::Employee(endpoint) => (
            "employee",
            Some(endpoint.employee_id.to_string()),
            endpoint.session_id.map(|value| value.to_string()),
        ),
        MessageSender::User(_) => ("user", None, None),
    }
}

fn sender_from_columns(
    kind: &str,
    employee_id: Option<&str>,
    session_id: Option<&str>,
) -> Result<MessageSender, StoreError> {
    match kind {
        "employee" => Ok(MessageSender::Employee(MessageEndpoint {
            employee_id: parse_uuid(
                employee_id.ok_or(StoreError::InvalidRecord {
                    field: "conversation_turns.author_employee_id",
                    message: "employee author is missing its employee id".to_owned(),
                })?,
                "conversation_turns.author_employee_id",
            )?,
            session_id: optional_uuid(session_id, "conversation_turns.author_session_id")?,
        })),
        "user" => Ok(MessageSender::User(lazoo_protocol::UserMessageEndpoint {
            kind: lazoo_protocol::UserMessageKind::User,
        })),
        other => Err(StoreError::InvalidRecord {
            field: "conversation_turns.author_kind",
            message: format!("unknown turn author kind `{other}`"),
        }),
    }
}

fn insert_conversation_command(
    transaction: &Transaction<'_>,
    idempotency_key: &str,
    request_id: &str,
    conversation_id: Uuid,
    now: &str,
) -> Result<(), StoreError> {
    transaction.execute(
        "INSERT INTO conversation_commands (
            scope, idempotency_key, request_id, conversation_id, created_at
         ) VALUES ('conversation.create', ?1, ?2, ?3, ?4)",
        params![
            idempotency_key,
            request_id,
            conversation_id.to_string(),
            now
        ],
    )?;
    Ok(())
}

pub(crate) fn insert_turn_command(
    transaction: &Transaction<'_>,
    scope: &str,
    idempotency_key: &str,
    request_id: &str,
    turn_id: Uuid,
    now: &str,
) -> Result<(), StoreError> {
    transaction.execute(
        "INSERT INTO turn_commands (
            scope, idempotency_key, request_id, turn_id, created_at
         ) VALUES (?1, ?2, ?3, ?4, ?5)",
        params![scope, idempotency_key, request_id, turn_id.to_string(), now],
    )?;
    Ok(())
}

fn conversation_for_command(
    connection: &Connection,
    scope: &str,
    idempotency_key: &str,
) -> Result<Option<ConversationRecord>, StoreError> {
    connection
        .query_row(
            "SELECT conversation_id FROM conversation_commands
             WHERE scope = ?1 AND idempotency_key = ?2",
            params![scope, idempotency_key],
            |row| row.get::<_, String>(0),
        )
        .optional()?
        .map(|id| {
            required_conversation(
                connection,
                parse_uuid(&id, "conversation_commands.conversation_id")?,
            )
        })
        .transpose()
}

pub(crate) fn turn_for_command(
    connection: &Connection,
    scope: &str,
    idempotency_key: &str,
) -> Result<Option<ConversationTurnRecord>, StoreError> {
    connection
        .query_row(
            "SELECT turn_id FROM turn_commands WHERE scope = ?1 AND idempotency_key = ?2",
            params![scope, idempotency_key],
            |row| row.get::<_, String>(0),
        )
        .optional()?
        .map(|id| required_turn(connection, parse_uuid(&id, "turn_commands.turn_id")?))
        .transpose()
}

fn required_conversation(
    connection: &Connection,
    id: Uuid,
) -> Result<ConversationRecord, StoreError> {
    connection
        .query_row(
            "SELECT sequence, contract_version, id, project_id, lead_employee_id,
                    lead_employee_configuration_revision, title, revision, created_at, updated_at
             FROM conversations WHERE id = ?1",
            [id.to_string()],
            |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, i64>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, Option<String>>(3)?,
                    row.get::<_, String>(4)?,
                    row.get::<_, i64>(5)?,
                    row.get::<_, Option<String>>(6)?,
                    row.get::<_, i64>(7)?,
                    row.get::<_, String>(8)?,
                    row.get::<_, String>(9)?,
                ))
            },
        )
        .optional()?
        .map(|row| -> Result<ConversationRecord, StoreError> {
            Ok(ConversationRecord {
                sequence: unsigned(row.0)?,
                version: unsigned32(row.1)?,
                id: parse_uuid(&row.2, "conversations.id")?,
                project_id: optional_uuid(row.3.as_deref(), "conversations.project_id")?,
                lead_employee_id: parse_uuid(&row.4, "conversations.lead_employee_id")?,
                lead_employee_configuration_revision: unsigned(row.5)?,
                title: row.6,
                revision: unsigned(row.7)?,
                created_at: row.8,
                updated_at: row.9,
            })
        })
        .transpose()?
        .ok_or(StoreError::ConversationNotFound {
            conversation_id: id,
        })
}

type RawTurn = (
    i64,
    i64,
    String,
    String,
    Option<String>,
    String,
    Option<String>,
    Option<String>,
    String,
    Option<String>,
    i64,
    String,
    String,
    Option<String>,
    String,
    String,
    i64,
    Option<String>,
    Option<String>,
    Option<String>,
    Option<String>,
    Option<String>,
    Option<String>,
    i64,
    String,
    String,
    Option<String>,
    Option<String>,
    Option<String>,
    Option<String>,
    Option<String>,
);

pub(crate) fn required_turn(
    connection: &Connection,
    id: Uuid,
) -> Result<ConversationTurnRecord, StoreError> {
    let raw = connection
        .query_row(
            "SELECT sequence, contract_version, id, conversation_id, task_id, author_kind,
                    author_employee_id, author_session_id, recipient_employee_id,
                    recipient_session_id, recipient_employee_configuration_revision, text,
                    message_id, message_attempt_id, correlation_id, state, revision,
                    owner_boot_id, provider, native_session_id, native_turn_id, failure_code,
                    failure_message, transcript_truncated, created_at, updated_at, submitted_at,
                    accepted_at, completed_at, cancel_requested_at, terminal_at
             FROM conversation_turns WHERE id = ?1",
            [id.to_string()],
            raw_turn,
        )
        .optional()?
        .ok_or(StoreError::TurnNotFound { turn_id: id })?;
    turn_from_raw(connection, raw)
}

fn raw_turn(row: &rusqlite::Row<'_>) -> rusqlite::Result<RawTurn> {
    Ok((
        row.get(0)?,
        row.get(1)?,
        row.get(2)?,
        row.get(3)?,
        row.get(4)?,
        row.get(5)?,
        row.get(6)?,
        row.get(7)?,
        row.get(8)?,
        row.get(9)?,
        row.get(10)?,
        row.get(11)?,
        row.get(12)?,
        row.get(13)?,
        row.get(14)?,
        row.get(15)?,
        row.get(16)?,
        row.get(17)?,
        row.get(18)?,
        row.get(19)?,
        row.get(20)?,
        row.get(21)?,
        row.get(22)?,
        row.get(23)?,
        row.get(24)?,
        row.get(25)?,
        row.get(26)?,
        row.get(27)?,
        row.get(28)?,
        row.get(29)?,
        row.get(30)?,
    ))
}

fn turn_from_raw(
    connection: &Connection,
    raw: RawTurn,
) -> Result<ConversationTurnRecord, StoreError> {
    let id = parse_uuid(&raw.2, "conversation_turns.id")?;
    Ok(ConversationTurnRecord {
        sequence: unsigned(raw.0)?,
        version: unsigned32(raw.1)?,
        id,
        conversation_id: parse_uuid(&raw.3, "conversation_turns.conversation_id")?,
        task_id: optional_uuid(raw.4.as_deref(), "conversation_turns.task_id")?,
        author: sender_from_columns(&raw.5, raw.6.as_deref(), raw.7.as_deref())?,
        recipient: MessageEndpoint {
            employee_id: parse_uuid(&raw.8, "conversation_turns.recipient_employee_id")?,
            session_id: optional_uuid(raw.9.as_deref(), "conversation_turns.recipient_session_id")?,
        },
        recipient_employee_configuration_revision: unsigned(raw.10)?,
        text: raw.11,
        message_id: parse_uuid(&raw.12, "conversation_turns.message_id")?,
        message_attempt_id: optional_uuid(
            raw.13.as_deref(),
            "conversation_turns.message_attempt_id",
        )?,
        correlation_id: parse_uuid(&raw.14, "conversation_turns.correlation_id")?,
        state: serde_json::from_value(Value::String(raw.15))?,
        revision: unsigned(raw.16)?,
        owner_boot_id: optional_uuid(raw.17.as_deref(), "conversation_turns.owner_boot_id")?,
        provider: raw.18,
        native_session_id: raw.19,
        native_turn_id: raw.20,
        failure_code: raw.21,
        failure_message: raw.22,
        transcript_truncated: raw.23 != 0,
        created_at: raw.24,
        updated_at: raw.25,
        submitted_at: raw.26,
        accepted_at: raw.27,
        completed_at: raw.28,
        cancel_requested_at: raw.29,
        terminal_at: raw.30,
        events: query_turn_events(connection, id)?,
    })
}

type RawTurnSummary = (
    i64,
    String,
    String,
    Option<String>,
    String,
    Option<String>,
    Option<String>,
    String,
    Option<String>,
    String,
    i64,
    Option<String>,
    String,
    String,
);

fn raw_turn_summary(row: &rusqlite::Row<'_>) -> rusqlite::Result<RawTurnSummary> {
    Ok((
        row.get(0)?,
        row.get(1)?,
        row.get(2)?,
        row.get(3)?,
        row.get(4)?,
        row.get(5)?,
        row.get(6)?,
        row.get(7)?,
        row.get(8)?,
        row.get(9)?,
        row.get(10)?,
        row.get(11)?,
        row.get(12)?,
        row.get(13)?,
    ))
}

fn turn_summary_from_raw(raw: RawTurnSummary) -> Result<ConversationTurnSummary, StoreError> {
    Ok(ConversationTurnSummary {
        sequence: unsigned(raw.0)?,
        id: parse_uuid(&raw.1, "conversation_turns.id")?,
        conversation_id: parse_uuid(&raw.2, "conversation_turns.conversation_id")?,
        task_id: optional_uuid(raw.3.as_deref(), "conversation_turns.task_id")?,
        author: sender_from_columns(&raw.4, raw.5.as_deref(), raw.6.as_deref())?,
        recipient: MessageEndpoint {
            employee_id: parse_uuid(&raw.7, "conversation_turns.recipient_employee_id")?,
            session_id: optional_uuid(raw.8.as_deref(), "conversation_turns.recipient_session_id")?,
        },
        state: serde_json::from_value(Value::String(raw.9))?,
        revision: unsigned(raw.10)?,
        native_turn_id: raw.11,
        created_at: raw.12,
        updated_at: raw.13,
    })
}

fn query_turn_events(
    connection: &Connection,
    turn_id: Uuid,
) -> Result<Vec<ConversationTurnEvent>, StoreError> {
    let mut statement = connection.prepare(
        "SELECT sequence, kind, provider, native_session_id, native_turn_id, payload_json,
                created_at
         FROM conversation_turn_events WHERE turn_id = ?1 ORDER BY sequence",
    )?;
    let rows = statement.query_map([turn_id.to_string()], |row| {
        Ok((
            row.get::<_, i64>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, String>(2)?,
            row.get::<_, Option<String>>(3)?,
            row.get::<_, Option<String>>(4)?,
            row.get::<_, String>(5)?,
            row.get::<_, String>(6)?,
        ))
    })?;
    rows.map(|row| {
        let row = row?;
        Ok(ConversationTurnEvent {
            sequence: unsigned(row.0)?,
            kind: row.1,
            provider: row.2,
            native_session_id: row.3,
            native_turn_id: row.4,
            payload: serde_json::from_str(&row.5)?,
            created_at: row.6,
        })
    })
    .collect()
}

struct AppendEventResult {
    truncated: bool,
}

#[allow(clippy::too_many_arguments)]
fn append_turn_event(
    transaction: &Transaction<'_>,
    turn: &ConversationTurnRecord,
    kind: &str,
    provider: &str,
    native_session_id: Option<&str>,
    native_turn_id: Option<&str>,
    payload: &Value,
    lifecycle: bool,
    now: &str,
) -> Result<AppendEventResult, StoreError> {
    let payload_json = serde_json::to_string(payload)?;
    let current_bytes: i64 = transaction.query_row(
        "SELECT transcript_bytes FROM conversation_turns WHERE id = ?1",
        [turn.id.to_string()],
        |row| row.get(0),
    )?;
    let mut current_bytes =
        usize::try_from(current_bytes).map_err(|_| StoreError::NegativeSequence)?;
    let additional = kind.len() + provider.len() + payload_json.len();
    let would_overflow = current_bytes.saturating_add(additional) > MAX_TURN_TRANSCRIPT_BYTES;
    let mut truncated = turn.transcript_truncated;
    let mut next_sequence = turn.events.last().map_or(1, |event| event.sequence + 1);
    if would_overflow && !truncated {
        transaction.execute(
            "INSERT INTO conversation_turn_events (
                turn_id, sequence, kind, provider, payload_json, created_at
             ) VALUES (?1, ?2, 'transcript-truncated', ?3, ?4, ?5)",
            params![
                turn.id.to_string(),
                to_i64(next_sequence)?,
                provider,
                serde_json::to_string(&json!({"limitBytes": MAX_TURN_TRANSCRIPT_BYTES}))?,
                now,
            ],
        )?;
        next_sequence += 1;
        truncated = true;
    }
    if !would_overflow || lifecycle {
        transaction.execute(
            "INSERT INTO conversation_turn_events (
                turn_id, sequence, kind, provider, native_session_id, native_turn_id,
                payload_json, created_at
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![
                turn.id.to_string(),
                to_i64(next_sequence)?,
                kind,
                provider,
                native_session_id,
                native_turn_id,
                payload_json,
                now,
            ],
        )?;
        if !would_overflow {
            current_bytes += additional;
        }
    }
    transaction.execute(
        "UPDATE conversation_turns SET transcript_bytes = ?2, transcript_truncated = ?3
         WHERE id = ?1",
        params![
            turn.id.to_string(),
            to_i64(u64::try_from(current_bytes).map_err(|_| StoreError::NegativeSequence)?)?,
            i64::from(truncated),
        ],
    )?;
    Ok(AppendEventResult { truncated })
}

fn update_observed_turn(
    transaction: &Transaction<'_>,
    current: &ConversationTurnRecord,
    input: &ObserveTurnInput,
    target: TurnState,
    truncated: bool,
    now: &str,
) -> Result<(), StoreError> {
    let accepted_at =
        (target == TurnState::Accepted && current.accepted_at.is_none()).then_some(now.to_owned());
    let completed_at = (target == TurnState::Completed).then_some(now.to_owned());
    let terminal_at = matches!(
        target,
        TurnState::Completed
            | TurnState::TerminalFailed
            | TurnState::Cancelled
            | TurnState::Uncertain
    )
    .then_some(now.to_owned());
    transaction.execute(
        "UPDATE conversation_turns SET
            state = ?2, revision = revision + 1, provider = ?3,
            native_session_id = COALESCE(?4, native_session_id),
            native_turn_id = COALESCE(?5, native_turn_id), transcript_truncated = ?6,
            owner_boot_id = CASE WHEN ?7 IS NULL THEN owner_boot_id ELSE NULL END,
            accepted_at = COALESCE(?8, accepted_at), completed_at = COALESCE(?9, completed_at),
            terminal_at = COALESCE(?10, terminal_at), updated_at = ?11
         WHERE id = ?1",
        params![
            input.turn_id.to_string(),
            serialized_name(target)?,
            input.provider,
            input.native_session_id,
            input.native_turn_id,
            i64::from(truncated),
            terminal_at,
            accepted_at,
            completed_at,
            terminal_at,
            now,
        ],
    )?;
    Ok(())
}

fn effective_turn_result(
    transaction: &Transaction<'_>,
    scope: &str,
    idempotency_key: &str,
    request_id: &str,
    turn_id: Uuid,
    event_type: &str,
    now: &str,
) -> Result<TurnMutationResult, StoreError> {
    insert_turn_command(
        transaction,
        scope,
        idempotency_key,
        request_id,
        turn_id,
        now,
    )?;
    let turn = required_turn(transaction, turn_id)?;
    let message = message::required_message(transaction, turn.message_id)?;
    let task = turn
        .task_id
        .map(|task_id| task::required_task(transaction, task_id))
        .transpose()?;
    let event_sequence = employee::insert_event(
        transaction,
        event_type,
        &json!({"turn": &turn, "message": &message}),
        now,
    )?;
    Ok(TurnMutationResult {
        turn,
        message,
        task,
        event_sequence: Some(event_sequence),
    })
}

pub(crate) fn turn_result(
    connection: &Connection,
    turn: ConversationTurnRecord,
    event_sequence: Option<u64>,
) -> Result<TurnMutationResult, StoreError> {
    let message = message::required_message(connection, turn.message_id)?;
    let task = turn
        .task_id
        .map(|task_id| task::required_task(connection, task_id))
        .transpose()?;
    Ok(TurnMutationResult {
        turn,
        message,
        task,
        event_sequence,
    })
}

fn commit_turn_result(
    transaction: Transaction<'_>,
    result: TurnMutationResult,
) -> Result<TurnMutationResult, StoreError> {
    transaction.commit()?;
    Ok(result)
}

fn require_turn_revision(
    turn: &ConversationTurnRecord,
    expected_revision: u64,
) -> Result<(), StoreError> {
    if turn.revision == expected_revision {
        Ok(())
    } else {
        Err(StoreError::TurnStale {
            turn_id: turn.id,
            current_revision: turn.revision,
        })
    }
}

fn require_turn_state(
    turn: &ConversationTurnRecord,
    expected_states: &[TurnState],
    expected: &'static str,
) -> Result<(), StoreError> {
    if expected_states.contains(&turn.state) {
        Ok(())
    } else {
        Err(StoreError::TurnState {
            turn_id: turn.id,
            state: serialized_name(turn.state)?,
            expected,
        })
    }
}

fn message_key(turn_id: Uuid, key: &str) -> String {
    format!("turn:{turn_id}:{key}")
}

/// The companion notices a coalesced turn carries that are in one of `states`; each follows
/// the canonical message through the transition under its own command key.
fn companions_in(
    transaction: &Transaction<'_>,
    turn: &ConversationTurnRecord,
    states: &[MessageDeliveryState],
) -> Result<Vec<lazoo_protocol::MessageRecord>, StoreError> {
    Ok(delivery::notice_companions(transaction, turn)?
        .into_iter()
        .filter(|message| states.contains(&message.delivery.state))
        .collect())
}

fn companion_key(turn_id: Uuid, key: &str, message_id: Uuid) -> String {
    message_key(turn_id, &format!("{key}:{message_id}"))
}

/// Acknowledges every companion notice a coalesced turn delivered, citing that turn and its
/// canonical message; the canonical message keeps the provider's acceptance evidence.
fn acknowledge_companions(
    transaction: &Transaction<'_>,
    turn: &ConversationTurnRecord,
    request_id: &str,
    key: &str,
) -> Result<(), StoreError> {
    for companion in companions_in(transaction, turn, &[MessageDeliveryState::Delivering])? {
        message::acknowledge_message_tx(
            transaction,
            &message::AcknowledgeMessageInput {
                request_id: request_id.to_owned(),
                idempotency_key: companion_key(turn.id, key, companion.envelope.id),
                message_id: companion.envelope.id,
                expected_revision: companion.delivery.revision,
                attempt_id: active_attempt_id(&companion)?,
                acknowledgement_key: format!("coalesced:{}", turn.id),
                evidence: json!({"turnId": turn.id, "canonicalMessageId": turn.message_id}),
            },
        )?;
    }
    Ok(())
}

/// Closes every companion notice that the cancelled turn had not delivered yet.
fn cancel_companions(
    transaction: &Transaction<'_>,
    turn: &ConversationTurnRecord,
    request_id: &str,
    key: &str,
) -> Result<(), StoreError> {
    for companion in companions_in(
        transaction,
        turn,
        &[
            MessageDeliveryState::Queued,
            MessageDeliveryState::Delivering,
            MessageDeliveryState::RetryableFailed,
        ],
    )? {
        message::cancel_message_tx(
            transaction,
            &message::CancelMessageInput {
                request_id: request_id.to_owned(),
                idempotency_key: companion_key(turn.id, key, companion.envelope.id),
                message_id: companion.envelope.id,
                expected_revision: companion.delivery.revision,
            },
        )?;
    }
    Ok(())
}

fn active_attempt_id(message: &lazoo_protocol::MessageRecord) -> Result<Uuid, StoreError> {
    message
        .attempts
        .last()
        .map(|attempt| attempt.id)
        .ok_or(StoreError::InvalidRecord {
            field: "message_delivery_attempts",
            message: "delivering companion notice has no delivery attempt".to_owned(),
        })
}

fn event_type_for_state(state: TurnState, fallback: &'static str) -> &'static str {
    match state {
        TurnState::Accepted => "turn.accepted",
        TurnState::Completed => "turn.completed",
        TurnState::RetryableFailed => "turn.retryable-failed",
        TurnState::TerminalFailed => "turn.terminal-failed",
        TurnState::Cancelled => "turn.cancelled",
        TurnState::Uncertain => "turn.uncertain",
        TurnState::Recorded | TurnState::Submitting => fallback,
    }
}

fn serialized_name<T: Serialize>(value: T) -> Result<String, StoreError> {
    serde_json::to_value(value)?
        .as_str()
        .map(ToOwned::to_owned)
        .ok_or(StoreError::InvalidRecord {
            field: "serialized enum",
            message: "serialized value was not a string".to_owned(),
        })
}

fn to_i64(value: u64) -> Result<i64, StoreError> {
    i64::try_from(value).map_err(|_| StoreError::NegativeSequence)
}

fn unsigned(value: i64) -> Result<u64, StoreError> {
    u64::try_from(value).map_err(|_| StoreError::NegativeSequence)
}

fn unsigned32(value: i64) -> Result<u32, StoreError> {
    u32::try_from(value).map_err(|_| StoreError::NegativeSequence)
}

fn parse_uuid(value: &str, field: &'static str) -> Result<Uuid, StoreError> {
    Uuid::parse_str(value).map_err(|error| StoreError::InvalidRecord {
        field,
        message: error.to_string(),
    })
}

fn optional_uuid(value: Option<&str>, field: &'static str) -> Result<Option<Uuid>, StoreError> {
    value.map(|value| parse_uuid(value, field)).transpose()
}
