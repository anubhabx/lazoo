use chrono::{Duration as ChronoDuration, Utc};
use lazoo_domain::{
    ApprovalPolicyOutcome, approval_fingerprint, approval_policy, validate_approval_action,
    validate_approval_detail, validate_approval_list_limit, validate_message_command_key,
};
use lazoo_protocol::{
    APPROVAL_CONTRACT_V1, ApprovalDecider, ApprovalDecision, ApprovalKind, ApprovalMutationResult,
    ApprovalReconcileResult, ApprovalRecord, ApprovalState, AuthorityMode, TurnState,
};
use rusqlite::{Connection, OptionalExtension, Transaction, TransactionBehavior, params};
use serde_json::{Value, json};
use uuid::Uuid;

use crate::{Store, StoreError, conversation, employee, task};

// ponytail: one week bounds a pending approval however the daemon is configured; a per-project
// expiry policy can replace this constant when the approval Inbox contract lands.
const MAX_PENDING_TIMEOUT_SECONDS: u64 = 7 * 24 * 60 * 60;

const APPROVAL_COLUMNS: &str = "sequence, contract_version, id, turn_id, task_id, conversation_id,
    project_id, employee_id, session_id, owner_boot_id, provider, native_session_id,
    native_turn_id, native_request_id, kind, action, detail_json, fingerprint, authority,
    scope_expansion, state, decider, revision, failure_code, failure_message, created_at,
    expires_at, updated_at, decided_at";

#[derive(Clone, Debug)]
pub struct RequestApprovalInput {
    pub request_id: String,
    pub idempotency_key: String,
    pub turn_id: Uuid,
    pub owner_boot_id: Uuid,
    pub native_request_id: Option<String>,
    pub kind: ApprovalKind,
    pub action: String,
    pub scope_expansion: bool,
    pub detail: Value,
    pub pending_timeout_seconds: u64,
}

#[derive(Clone, Debug)]
pub struct DecideApprovalInput {
    pub request_id: String,
    pub idempotency_key: String,
    pub approval_id: Uuid,
    pub expected_revision: u64,
    pub decision: ApprovalDecision,
}

#[derive(Clone, Debug)]
pub struct CloseApprovalInput {
    pub approval_id: Uuid,
    pub owner_boot_id: Uuid,
    pub expected_revision: u64,
    pub state: ApprovalState,
    pub failure_code: String,
    pub failure_message: String,
}

#[derive(Clone, Copy, Debug)]
pub struct ListApprovalsInput {
    pub after_sequence: u64,
    pub limit: u32,
    pub state: Option<ApprovalState>,
    pub turn_id: Option<Uuid>,
    pub task_id: Option<Uuid>,
    pub conversation_id: Option<Uuid>,
    pub employee_id: Option<Uuid>,
}

impl Store {
    /// Records a provider approval request first, then applies the authority policy. The row is
    /// `pending` only when the local user must decide; policy outcomes are durable immediately.
    #[allow(clippy::too_many_lines)] // Record-first policy evaluation is kept in one transaction.
    pub fn request_approval(
        &self,
        input: &RequestApprovalInput,
    ) -> Result<ApprovalMutationResult, StoreError> {
        validate_message_command_key(&input.idempotency_key)?;
        let action = validate_approval_action(&input.action)?;
        validate_approval_detail(&input.detail)?;
        let mut connection = self.lock()?;
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        if let Some(approval) =
            approval_for_command(&transaction, "approval.request", &input.idempotency_key)?
        {
            if approval.turn_id != input.turn_id {
                return Err(StoreError::ApprovalIdempotencyConflict);
            }
            transaction.commit()?;
            return Ok(ApprovalMutationResult {
                approval,
                event_sequence: None,
            });
        }
        let turn = conversation::required_turn(&transaction, input.turn_id)?;
        if turn.owner_boot_id != Some(input.owner_boot_id) {
            return Err(StoreError::ApprovalTurnOwnership { turn_id: turn.id });
        }
        if !matches!(turn.state, TurnState::Submitting | TurnState::Accepted) {
            return Err(StoreError::ApprovalTurnState {
                turn_id: turn.id,
                state: serialized_name(turn.state)?,
            });
        }
        let authority = match turn.task_id {
            Some(task_id) => task::required_task(&transaction, task_id)?.effective_authority,
            None => employee_authority(&transaction, turn.recipient.employee_id)?,
        };
        let project_id = conversation_project_id(&transaction, turn.conversation_id)?;
        let now_time = Utc::now();
        let now = now_time.to_rfc3339();
        let timeout_seconds = i64::try_from(
            input
                .pending_timeout_seconds
                .min(MAX_PENDING_TIMEOUT_SECONDS),
        )
        .unwrap_or(0);
        let expires_at = (now_time + ChronoDuration::seconds(timeout_seconds)).to_rfc3339();
        let (state, decider, failure) = if turn.cancel_requested_at.is_some() {
            (
                ApprovalState::Cancelled,
                Some(ApprovalDecider::System),
                Some((
                    "LZ-APPROVAL-TURN-CANCELLED",
                    "the governed turn was already cancelled",
                )),
            )
        } else {
            match approval_policy(authority, input.scope_expansion) {
                ApprovalPolicyOutcome::AutoApprove => {
                    (ApprovalState::Approved, Some(ApprovalDecider::Policy), None)
                }
                ApprovalPolicyOutcome::RequireUser => (ApprovalState::Pending, None, None),
            }
        };
        let decided_at = (state != ApprovalState::Pending).then(|| now.clone());
        let id = Uuid::new_v4();
        transaction.execute(
            "INSERT INTO approvals (
                id, contract_version, turn_id, task_id, conversation_id, project_id, employee_id,
                session_id, owner_boot_id, provider, native_session_id, native_turn_id,
                native_request_id, kind, action, detail_json, fingerprint, authority,
                scope_expansion, state, decider, revision, failure_code, failure_message,
                created_at, expires_at, updated_at, decided_at
             ) VALUES (
                ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18,
                ?19, ?20, ?21, 1, ?22, ?23, ?24, ?25, ?24, ?26
             )",
            params![
                id.to_string(),
                APPROVAL_CONTRACT_V1,
                turn.id.to_string(),
                turn.task_id.map(|value| value.to_string()),
                turn.conversation_id.to_string(),
                project_id.map(|value| value.to_string()),
                turn.recipient.employee_id.to_string(),
                turn.recipient.session_id.map(|value| value.to_string()),
                input.owner_boot_id.to_string(),
                turn.provider.clone().unwrap_or_default(),
                turn.native_session_id,
                turn.native_turn_id,
                input.native_request_id,
                serialized_name(input.kind)?,
                action,
                serde_json::to_string(&input.detail)?,
                approval_fingerprint(input.kind, &input.action, input.scope_expansion),
                serialized_name(authority)?,
                i32::from(input.scope_expansion),
                serialized_name(state)?,
                decider.map(serialized_name).transpose()?,
                failure.map(|(code, _)| code),
                failure.map(|(_, message)| message),
                now,
                expires_at,
                decided_at,
            ],
        )?;
        insert_approval_command_tx(
            &transaction,
            "approval.request",
            &input.idempotency_key,
            &input.request_id,
            id,
            &now,
        )?;
        let approval = required_approval(&transaction, id)?;
        let event_sequence = employee::insert_event(
            &transaction,
            "approval.requested",
            &json!({"approval": &approval}),
            &now,
        )?;
        transaction.commit()?;
        Ok(ApprovalMutationResult {
            approval,
            event_sequence: Some(event_sequence),
        })
    }

    /// Records the local user's decision. Only a pending approval at the expected revision can be
    /// decided, and a decision never widens beyond the one fingerprinted action.
    pub fn decide_approval(
        &self,
        input: &DecideApprovalInput,
    ) -> Result<ApprovalMutationResult, StoreError> {
        validate_message_command_key(&input.idempotency_key)?;
        let mut connection = self.lock()?;
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        if let Some(approval) =
            approval_for_command(&transaction, "approval.decide", &input.idempotency_key)?
        {
            if approval.id != input.approval_id {
                return Err(StoreError::ApprovalIdempotencyConflict);
            }
            transaction.commit()?;
            return Ok(ApprovalMutationResult {
                approval,
                event_sequence: None,
            });
        }
        let current = required_approval(&transaction, input.approval_id)?;
        require_approval_revision(&current, input.expected_revision)?;
        require_pending(&current)?;
        let state = match input.decision {
            ApprovalDecision::Approve => ApprovalState::Approved,
            ApprovalDecision::Decline => ApprovalState::Declined,
        };
        let now = Utc::now().to_rfc3339();
        transaction.execute(
            "UPDATE approvals SET state = ?2, decider = 'user', revision = revision + 1,
                    updated_at = ?3, decided_at = ?3
             WHERE id = ?1",
            params![current.id.to_string(), serialized_name(state)?, now],
        )?;
        insert_approval_command_tx(
            &transaction,
            "approval.decide",
            &input.idempotency_key,
            &input.request_id,
            current.id,
            &now,
        )?;
        let approval = required_approval(&transaction, current.id)?;
        let event_sequence = employee::insert_event(
            &transaction,
            "approval.decided",
            &json!({"approval": &approval}),
            &now,
        )?;
        transaction.commit()?;
        Ok(ApprovalMutationResult {
            approval,
            event_sequence: Some(event_sequence),
        })
    }

    /// Closes a pending approval on the owning daemon's behalf when it expires or its turn ends.
    pub fn close_approval(
        &self,
        input: &CloseApprovalInput,
    ) -> Result<ApprovalMutationResult, StoreError> {
        if !matches!(
            input.state,
            ApprovalState::Expired | ApprovalState::Cancelled
        ) {
            return Err(StoreError::InvalidRecord {
                field: "approval close state",
                message: "system closure must be expired or cancelled".to_owned(),
            });
        }
        let mut connection = self.lock()?;
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let current = required_approval(&transaction, input.approval_id)?;
        if current.owner_boot_id != input.owner_boot_id {
            return Err(StoreError::ApprovalOwnership {
                approval_id: current.id,
            });
        }
        require_approval_revision(&current, input.expected_revision)?;
        require_pending(&current)?;
        let now = Utc::now().to_rfc3339();
        transaction.execute(
            "UPDATE approvals SET state = ?2, decider = 'system', revision = revision + 1,
                    failure_code = ?3, failure_message = ?4, updated_at = ?5, decided_at = ?5
             WHERE id = ?1",
            params![
                current.id.to_string(),
                serialized_name(input.state)?,
                input.failure_code,
                input.failure_message,
                now
            ],
        )?;
        let approval = required_approval(&transaction, current.id)?;
        let event_type = if input.state == ApprovalState::Expired {
            "approval.expired"
        } else {
            "approval.cancelled"
        };
        let event_sequence = employee::insert_event(
            &transaction,
            event_type,
            &json!({"approval": &approval}),
            &now,
        )?;
        transaction.commit()?;
        Ok(ApprovalMutationResult {
            approval,
            event_sequence: Some(event_sequence),
        })
    }

    pub fn get_approval(&self, approval_id: Uuid) -> Result<ApprovalRecord, StoreError> {
        let connection = self.lock()?;
        required_approval(&connection, approval_id)
    }

    pub fn list_approvals(
        &self,
        input: &ListApprovalsInput,
    ) -> Result<Vec<ApprovalRecord>, StoreError> {
        validate_approval_list_limit(input.limit)?;
        let connection = self.lock()?;
        let state = input.state.map(serialized_name).transpose()?;
        let mut statement = connection.prepare(&format!(
            "SELECT {APPROVAL_COLUMNS} FROM approvals
             WHERE sequence > ?1
               AND (?3 IS NULL OR state = ?3)
               AND (?4 IS NULL OR turn_id = ?4)
               AND (?5 IS NULL OR task_id = ?5)
               AND (?6 IS NULL OR conversation_id = ?6)
               AND (?7 IS NULL OR employee_id = ?7)
             ORDER BY sequence LIMIT ?2"
        ))?;
        let rows = statement.query_map(
            params![
                to_i64(input.after_sequence)?,
                i64::from(input.limit),
                state,
                input.turn_id.map(|value| value.to_string()),
                input.task_id.map(|value| value.to_string()),
                input.conversation_id.map(|value| value.to_string()),
                input.employee_id.map(|value| value.to_string()),
            ],
            raw_approval,
        )?;
        rows.map(|row| approval_from_raw(row?)).collect()
    }

    /// Startup truth: a pending approval from another daemon boot can never be answered because
    /// its provider request died with that boot. It closes as expired with owner-loss evidence.
    pub fn reconcile_approvals(
        &self,
        owner_boot_id: Uuid,
    ) -> Result<ApprovalReconcileResult, StoreError> {
        let mut connection = self.lock()?;
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let ids = {
            let mut statement = transaction.prepare(
                "SELECT id FROM approvals
                 WHERE state = 'pending' AND owner_boot_id <> ?1 ORDER BY sequence",
            )?;
            let rows =
                statement.query_map([owner_boot_id.to_string()], |row| row.get::<_, String>(0))?;
            rows.collect::<Result<Vec<_>, _>>()?
        };
        let now = Utc::now().to_rfc3339();
        let mut approvals = Vec::with_capacity(ids.len());
        for id in ids {
            transaction.execute(
                "UPDATE approvals SET state = 'expired', decider = 'system',
                        revision = revision + 1, failure_code = 'LZ-APPROVAL-OWNER-LOST',
                        failure_message = ?2, updated_at = ?3, decided_at = ?3
                 WHERE id = ?1",
                params![
                    id,
                    "the daemon that owned this approval request is no longer running",
                    now
                ],
            )?;
            approvals.push(required_approval(
                &transaction,
                parse_uuid(&id, "approvals.id")?,
            )?);
        }
        let event_sequence = if approvals.is_empty() {
            None
        } else {
            Some(employee::insert_event(
                &transaction,
                "approvals.reconciled",
                &json!({"approvals": &approvals}),
                &now,
            )?)
        };
        transaction.commit()?;
        Ok(ApprovalReconcileResult {
            approvals,
            event_sequence,
        })
    }
}

fn employee_authority(
    connection: &Connection,
    employee_id: Uuid,
) -> Result<AuthorityMode, StoreError> {
    let encoded: String = connection
        .query_row(
            "SELECT authority_mode FROM employees WHERE id = ?1",
            [employee_id.to_string()],
            |row| row.get(0),
        )
        .optional()?
        .ok_or(StoreError::EmployeeNotFound { employee_id })?;
    Ok(serde_json::from_str(&encoded)?)
}

fn conversation_project_id(
    connection: &Connection,
    conversation_id: Uuid,
) -> Result<Option<Uuid>, StoreError> {
    let project_id: Option<String> = connection
        .query_row(
            "SELECT project_id FROM conversations WHERE id = ?1",
            [conversation_id.to_string()],
            |row| row.get(0),
        )
        .optional()?
        .ok_or(StoreError::ConversationNotFound { conversation_id })?;
    optional_uuid(project_id.as_deref(), "conversations.project_id")
}

fn approval_for_command(
    connection: &Connection,
    scope: &str,
    idempotency_key: &str,
) -> Result<Option<ApprovalRecord>, StoreError> {
    connection
        .query_row(
            "SELECT approval_id FROM approval_commands WHERE scope = ?1 AND idempotency_key = ?2",
            params![scope, idempotency_key],
            |row| row.get::<_, String>(0),
        )
        .optional()?
        .map(|id| {
            required_approval(
                connection,
                parse_uuid(&id, "approval_commands.approval_id")?,
            )
        })
        .transpose()
}

fn insert_approval_command_tx(
    transaction: &Transaction<'_>,
    scope: &str,
    idempotency_key: &str,
    request_id: &str,
    approval_id: Uuid,
    now: &str,
) -> Result<(), StoreError> {
    transaction.execute(
        "INSERT INTO approval_commands (scope, idempotency_key, request_id, approval_id, created_at)
         VALUES (?1, ?2, ?3, ?4, ?5)",
        params![scope, idempotency_key, request_id, approval_id.to_string(), now],
    )?;
    Ok(())
}

fn require_approval_revision(
    approval: &ApprovalRecord,
    expected_revision: u64,
) -> Result<(), StoreError> {
    if approval.revision == expected_revision {
        Ok(())
    } else {
        Err(StoreError::ApprovalStale {
            approval_id: approval.id,
            current_revision: approval.revision,
        })
    }
}

fn require_pending(approval: &ApprovalRecord) -> Result<(), StoreError> {
    if approval.state == ApprovalState::Pending {
        Ok(())
    } else {
        Err(StoreError::ApprovalState {
            approval_id: approval.id,
            state: serialized_name(approval.state)?,
            expected: "pending",
        })
    }
}

pub(crate) fn required_approval(
    connection: &Connection,
    approval_id: Uuid,
) -> Result<ApprovalRecord, StoreError> {
    let raw = connection
        .query_row(
            &format!("SELECT {APPROVAL_COLUMNS} FROM approvals WHERE id = ?1"),
            [approval_id.to_string()],
            raw_approval,
        )
        .optional()?
        .ok_or(StoreError::ApprovalNotFound { approval_id })?;
    approval_from_raw(raw)
}

type RawApproval = (
    i64,
    i64,
    String,
    String,
    Option<String>,
    String,
    Option<String>,
    String,
    Option<String>,
    String,
    String,
    Option<String>,
    Option<String>,
    Option<String>,
    String,
    String,
    String,
    String,
    String,
    i64,
    String,
    Option<String>,
    i64,
    Option<String>,
    Option<String>,
    String,
    String,
    String,
    Option<String>,
);

fn raw_approval(row: &rusqlite::Row<'_>) -> rusqlite::Result<RawApproval> {
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
    ))
}

fn approval_from_raw(raw: RawApproval) -> Result<ApprovalRecord, StoreError> {
    Ok(ApprovalRecord {
        sequence: unsigned(raw.0)?,
        version: unsigned32(raw.1)?,
        id: parse_uuid(&raw.2, "approvals.id")?,
        turn_id: parse_uuid(&raw.3, "approvals.turn_id")?,
        task_id: optional_uuid(raw.4.as_deref(), "approvals.task_id")?,
        conversation_id: parse_uuid(&raw.5, "approvals.conversation_id")?,
        project_id: optional_uuid(raw.6.as_deref(), "approvals.project_id")?,
        employee_id: parse_uuid(&raw.7, "approvals.employee_id")?,
        session_id: optional_uuid(raw.8.as_deref(), "approvals.session_id")?,
        owner_boot_id: parse_uuid(&raw.9, "approvals.owner_boot_id")?,
        provider: raw.10,
        native_session_id: raw.11,
        native_turn_id: raw.12,
        native_request_id: raw.13,
        kind: serde_json::from_value(Value::String(raw.14))?,
        action: raw.15,
        detail: serde_json::from_str(&raw.16)?,
        fingerprint: raw.17,
        authority: serde_json::from_value(Value::String(raw.18))?,
        scope_expansion: raw.19 != 0,
        state: serde_json::from_value(Value::String(raw.20))?,
        decider: raw
            .21
            .map(|value| serde_json::from_value(Value::String(value)))
            .transpose()?,
        revision: unsigned(raw.22)?,
        failure_code: raw.23,
        failure_message: raw.24,
        created_at: raw.25,
        expires_at: raw.26,
        updated_at: raw.27,
        decided_at: raw.28,
    })
}

fn serialized_name<T: serde::Serialize>(value: T) -> Result<String, StoreError> {
    match serde_json::to_value(value)? {
        Value::String(value) => Ok(value),
        _ => Err(StoreError::InvalidRecord {
            field: "approval enum",
            message: "enum did not serialize to a string".to_owned(),
        }),
    }
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

fn to_i64(value: u64) -> Result<i64, StoreError> {
    i64::try_from(value).map_err(|_| StoreError::NegativeSequence)
}

fn unsigned(value: i64) -> Result<u64, StoreError> {
    u64::try_from(value).map_err(|_| StoreError::NegativeSequence)
}

fn unsigned32(value: i64) -> Result<u32, StoreError> {
    u32::try_from(value).map_err(|_| StoreError::NegativeSequence)
}
