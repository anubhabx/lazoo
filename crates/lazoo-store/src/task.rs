use chrono::Utc;
use lazoo_domain::{
    derive_availability, effective_authority, validate_message_command_key, validate_task_evidence,
    validate_task_failure, validate_task_objective, validate_task_references,
};
use lazoo_protocol::{
    AuthorityMode, CONVERSATION_CONTRACT_V1, EmployeeAvailability, MESSAGE_ENVELOPE_V1,
    MessageDeliveryState, MessageEndpoint, MessageKind, MessageRecord, MessageReference,
    MessageSender, SessionLifecycleState, TASK_CONTRACT_V1, TaskDetail, TaskDispatchFailureResult,
    TaskDispatchRecord, TaskEvidence, TaskEvidenceKind, TaskMutationResult, TaskReconcileResult,
    TaskRecord, TaskReportKind, TaskResolution, TaskRole, TaskState, TaskSummary,
    TaskWorkspaceKind, TurnMutationResult, TurnState, UserMessageEndpoint, UserMessageKind,
};
use rusqlite::{Connection, OptionalExtension, Transaction, TransactionBehavior, params};
use serde_json::{Value, json};
use uuid::Uuid;

use crate::{Store, StoreError, conversation, employee, message, session};

#[derive(Clone, Debug)]
pub struct ListTasksInput {
    pub after_sequence: u64,
    pub limit: u32,
    pub root_task_id: Option<Uuid>,
    pub parent_task_id: Option<Uuid>,
    pub conversation_id: Option<Uuid>,
    pub project_id: Option<Uuid>,
    pub assigned_employee_id: Option<Uuid>,
    pub state: Option<TaskState>,
}

#[derive(Clone, Copy, Debug)]
pub struct ClaimTaskDispatchInput {
    pub task_id: Uuid,
    pub owner_boot_id: Uuid,
}

#[derive(Clone, Copy, Debug)]
pub struct DeferTaskDispatchInput {
    pub task_id: Uuid,
    pub owner_boot_id: Uuid,
    pub expected_revision: u64,
    pub active_session_id: Uuid,
}

#[derive(Clone, Debug)]
pub struct MarkTaskDispatchWorkspaceReadyInput {
    pub task_id: Uuid,
    pub owner_boot_id: Uuid,
    pub expected_revision: u64,
    pub workspace_kind: TaskWorkspaceKind,
    pub workspace_path: String,
    pub base_commit: Option<String>,
    pub branch_name: Option<String>,
}

#[derive(Clone, Copy, Debug)]
pub struct RecordTaskDispatchTurnInput {
    pub task_id: Uuid,
    pub owner_boot_id: Uuid,
    pub expected_dispatch_revision: u64,
    pub session_id: Uuid,
}

#[derive(Clone, Debug)]
pub struct FailTaskDispatchInput {
    pub task_id: Uuid,
    pub owner_boot_id: Uuid,
    pub failure_code: String,
    pub failure_message: String,
}

#[derive(Clone, Debug)]
pub struct DelegateTaskInput {
    pub request_id: String,
    pub idempotency_key: String,
    pub parent_task_id: Uuid,
    pub actor: MessageEndpoint,
    pub assigned_employee_id: Uuid,
    pub objective: String,
    pub references: Vec<MessageReference>,
    pub requested_authority: Option<AuthorityMode>,
    pub roster_revision: u64,
}

#[derive(Clone, Debug)]
pub struct AcceptTaskInput {
    pub request_id: String,
    pub idempotency_key: String,
    pub task_id: Uuid,
    pub expected_revision: u64,
}

#[derive(Clone, Debug)]
pub struct StartTaskInput {
    pub request_id: String,
    pub idempotency_key: String,
    pub task_id: Uuid,
    pub expected_revision: u64,
    pub actor: MessageEndpoint,
    pub turn_id: Uuid,
    pub evidence: Value,
}

#[derive(Clone, Debug)]
pub struct ReportTaskInput {
    pub request_id: String,
    pub idempotency_key: String,
    pub task_id: Uuid,
    pub expected_revision: u64,
    pub actor: MessageEndpoint,
    pub kind: TaskReportKind,
    pub evidence: Value,
    pub failure_code: Option<String>,
    pub failure_message: Option<String>,
}

#[derive(Clone, Debug)]
pub struct CancelTaskInput {
    pub request_id: String,
    pub idempotency_key: String,
    pub task_id: Uuid,
    pub expected_revision: u64,
    pub actor: MessageSender,
    pub evidence: Value,
}

#[derive(Clone, Debug)]
pub struct ResolveTaskInput {
    pub request_id: String,
    pub idempotency_key: String,
    pub task_id: Uuid,
    pub expected_revision: u64,
    pub resolution: TaskResolution,
    pub evidence: Value,
    pub failure_code: Option<String>,
    pub failure_message: Option<String>,
}

pub(crate) struct InsertRootTaskInput {
    pub id: Uuid,
    pub project_id: Option<Uuid>,
    pub conversation_id: Uuid,
    pub assigned_employee_id: Uuid,
    pub objective: String,
    pub requested_authority: AuthorityMode,
    pub effective_authority: AuthorityMode,
    pub employee_configuration_revision: u64,
    pub roster_revision: u64,
    pub created_at: String,
}

impl Store {
    pub fn pending_task_dispatches(&self, limit: u32) -> Result<Vec<Uuid>, StoreError> {
        if limit == 0 || limit > 1024 {
            return Err(StoreError::TaskListLimit);
        }
        let connection = self.lock()?;
        let mut statement = connection.prepare(
            "SELECT task.id
             FROM tasks task
             LEFT JOIN task_dispatches dispatch ON dispatch.task_id = task.id
             WHERE task.parent_task_id IS NOT NULL
               AND task.state = 'accepted'
               AND task.delegation_message_id IS NOT NULL
               AND dispatch.task_id IS NULL
               AND NOT EXISTS (
                   SELECT 1 FROM employee_sessions session
                   WHERE session.employee_id = task.assigned_employee_id
                     AND session.state IN ('starting', 'idle', 'stopping')
               )
             ORDER BY task.sequence LIMIT ?1",
        )?;
        let rows = statement.query_map([i64::from(limit)], |row| row.get::<_, String>(0))?;
        rows.map(|row| parse_uuid(&row?, "tasks.id")).collect()
    }

    pub fn list_tasks(&self, input: &ListTasksInput) -> Result<Vec<TaskSummary>, StoreError> {
        if input.limit == 0 || input.limit > 1024 {
            return Err(StoreError::TaskListLimit);
        }
        let connection = self.lock()?;
        query_tasks(&connection, input)
    }

    pub fn get_task(&self, task_id: Uuid) -> Result<TaskDetail, StoreError> {
        let connection = self.lock()?;
        let task = required_task(&connection, task_id)?;
        let children = query_tasks(
            &connection,
            &ListTasksInput {
                after_sequence: 0,
                limit: 1024,
                root_task_id: None,
                parent_task_id: Some(task_id),
                conversation_id: None,
                project_id: None,
                assigned_employee_id: None,
                state: None,
            },
        )?;
        let evidence = query_task_evidence(&connection, task_id)?;
        let dispatch = query_task_dispatch(&connection, task_id)?;
        Ok(TaskDetail {
            task,
            children,
            evidence,
            dispatch,
        })
    }

    pub fn claim_task_dispatch(
        &self,
        input: &ClaimTaskDispatchInput,
    ) -> Result<TaskDispatchRecord, StoreError> {
        let mut connection = self.lock()?;
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        if let Some(existing) = query_task_dispatch(&transaction, input.task_id)? {
            if existing.owner_boot_id == input.owner_boot_id {
                transaction.commit()?;
                return Ok(existing);
            }
            return Err(StoreError::TaskDispatchOwned {
                task_id: input.task_id,
                owner_boot_id: existing.owner_boot_id,
            });
        }

        let task = required_task(&transaction, input.task_id)?;
        if task.parent_task_id.is_none() || task.state != TaskState::Accepted {
            return Err(StoreError::TaskState {
                task_id: task.id,
                state: serialized_name(task.state)?,
                expected: "accepted delegated task",
            });
        }
        require_valid_ancestry(&transaction, &task)?;
        let message_id = task
            .delegation_message_id
            .ok_or(StoreError::TaskAncestryMismatch { task_id: task.id })?;
        let delegation_message = message::required_message(&transaction, message_id)?;
        if delegation_message.envelope.task_id != Some(task.id)
            || delegation_message.envelope.recipient.employee_id != task.assigned_employee_id
            || delegation_message.delivery.state != MessageDeliveryState::Queued
        {
            return Err(StoreError::TaskAncestryMismatch { task_id: task.id });
        }
        let project_configuration_revision = task
            .project_id
            .map(|project_id| {
                transaction
                    .query_row(
                        "SELECT configuration_revision FROM projects WHERE id = ?1",
                        [project_id.to_string()],
                        |row| row.get::<_, i64>(0),
                    )
                    .optional()?
                    .map(unsigned)
                    .transpose()?
                    .ok_or(StoreError::MessageProjectNotFound { project_id })
            })
            .transpose()?;
        let now = Utc::now().to_rfc3339();
        transaction.execute(
            "INSERT INTO task_dispatches (
                task_id, message_id, owner_boot_id, project_id,
                project_configuration_revision, state, revision, created_at, updated_at
             ) VALUES (?1, ?2, ?3, ?4, ?5, 'preparing', 1, ?6, ?6)",
            params![
                task.id.to_string(),
                message_id.to_string(),
                input.owner_boot_id.to_string(),
                task.project_id.map(|value| value.to_string()),
                project_configuration_revision.map(to_i64).transpose()?,
                now,
            ],
        )?;
        let dispatch =
            query_task_dispatch(&transaction, task.id)?.ok_or(StoreError::InvalidRecord {
                field: "task_dispatches.task_id",
                message: "inserted dispatch was not found".to_owned(),
            })?;
        employee::insert_event(
            &transaction,
            "task.dispatch-preparing",
            &json!({"task": &task, "dispatch": &dispatch}),
            &now,
        )?;
        transaction.commit()?;
        Ok(dispatch)
    }

    pub fn defer_task_dispatch(&self, input: &DeferTaskDispatchInput) -> Result<u64, StoreError> {
        let mut connection = self.lock()?;
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let dispatch =
            query_task_dispatch(&transaction, input.task_id)?.ok_or(StoreError::TaskNotFound {
                task_id: input.task_id,
            })?;
        if dispatch.owner_boot_id != input.owner_boot_id {
            return Err(StoreError::TaskDispatchOwned {
                task_id: input.task_id,
                owner_boot_id: dispatch.owner_boot_id,
            });
        }
        if dispatch.revision != input.expected_revision {
            return Err(StoreError::TaskDispatchStale {
                task_id: input.task_id,
                current_revision: dispatch.revision,
            });
        }
        if !matches!(
            dispatch.state,
            lazoo_protocol::TaskDispatchState::Preparing
                | lazoo_protocol::TaskDispatchState::WorkspaceReady
        ) || dispatch.turn_id.is_some()
            || dispatch.session_id.is_some()
        {
            return Err(StoreError::TaskDispatchWorkspace {
                task_id: input.task_id,
            });
        }
        let task = required_task(&transaction, input.task_id)?;
        let message = message::required_message(&transaction, dispatch.message_id)?;
        if task.state != TaskState::Accepted
            || message.delivery.state != MessageDeliveryState::Queued
        {
            return Err(StoreError::TaskState {
                task_id: input.task_id,
                state: serialized_name(task.state)?,
                expected: "accepted dispatch with queued canonical message",
            });
        }
        let active = session::query_session(&transaction, input.active_session_id)?.ok_or(
            StoreError::SessionNotFound {
                session_id: input.active_session_id,
            },
        )?;
        if active.employee_id != task.assigned_employee_id
            || !matches!(
                active.state,
                SessionLifecycleState::Starting
                    | SessionLifecycleState::Idle
                    | SessionLifecycleState::Stopping
            )
        {
            return Err(StoreError::TaskActorSession {
                employee_id: task.assigned_employee_id,
            });
        }
        transaction.execute(
            "DELETE FROM task_dispatches WHERE task_id = ?1",
            [input.task_id.to_string()],
        )?;
        let now = Utc::now().to_rfc3339();
        let event_sequence = employee::insert_event(
            &transaction,
            "task.dispatch-deferred",
            &json!({
                "task": &task,
                "dispatch": &dispatch,
                "activeSessionId": input.active_session_id,
                "reason": "employee-session-active"
            }),
            &now,
        )?;
        transaction.commit()?;
        Ok(event_sequence)
    }

    pub fn mark_task_dispatch_workspace_ready(
        &self,
        input: &MarkTaskDispatchWorkspaceReadyInput,
    ) -> Result<TaskDispatchRecord, StoreError> {
        let mut connection = self.lock()?;
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let current =
            query_task_dispatch(&transaction, input.task_id)?.ok_or(StoreError::TaskNotFound {
                task_id: input.task_id,
            })?;
        if current.owner_boot_id != input.owner_boot_id {
            return Err(StoreError::TaskDispatchOwned {
                task_id: input.task_id,
                owner_boot_id: current.owner_boot_id,
            });
        }
        let replay = current.state == lazoo_protocol::TaskDispatchState::WorkspaceReady
            && input.expected_revision.checked_add(1) == Some(current.revision)
            && current.workspace_kind == Some(input.workspace_kind)
            && current.workspace_path.as_deref() == Some(input.workspace_path.as_str())
            && current.base_commit == input.base_commit
            && current.branch_name == input.branch_name;
        if replay {
            transaction.commit()?;
            return Ok(current);
        }
        if current.revision != input.expected_revision {
            return Err(StoreError::TaskDispatchStale {
                task_id: input.task_id,
                current_revision: current.revision,
            });
        }
        let task = required_task(&transaction, input.task_id)?;
        let workspace_valid = !input.workspace_path.trim().is_empty()
            && input.workspace_path.len() <= 32_768
            && match input.workspace_kind {
                TaskWorkspaceKind::Projectless => {
                    task.project_id.is_none()
                        && input.base_commit.is_none()
                        && input.branch_name.is_none()
                }
                TaskWorkspaceKind::GitWorktree => {
                    task.project_id.is_some()
                        && input
                            .base_commit
                            .as_deref()
                            .is_some_and(|value| !value.is_empty() && value.len() <= 128)
                        && input
                            .branch_name
                            .as_deref()
                            .is_some_and(|value| !value.is_empty() && value.len() <= 256)
                }
            };
        if current.state != lazoo_protocol::TaskDispatchState::Preparing || !workspace_valid {
            return Err(StoreError::TaskDispatchWorkspace {
                task_id: input.task_id,
            });
        }
        let now = Utc::now().to_rfc3339();
        transaction.execute(
            "UPDATE task_dispatches SET
                workspace_kind = ?2, workspace_path = ?3, base_commit = ?4,
                branch_name = ?5, state = 'workspace-ready', revision = revision + 1,
                updated_at = ?6
             WHERE task_id = ?1",
            params![
                input.task_id.to_string(),
                serialized_name(input.workspace_kind)?,
                input.workspace_path,
                input.base_commit,
                input.branch_name,
                now,
            ],
        )?;
        let dispatch =
            query_task_dispatch(&transaction, input.task_id)?.ok_or(StoreError::TaskNotFound {
                task_id: input.task_id,
            })?;
        employee::insert_event(
            &transaction,
            "task.workspace-ready",
            &json!({"task": &task, "dispatch": &dispatch}),
            &now,
        )?;
        transaction.commit()?;
        Ok(dispatch)
    }

    #[allow(clippy::too_many_lines)] // Keep the canonical message and turn linkage atomic.
    pub fn record_task_dispatch_turn(
        &self,
        input: &RecordTaskDispatchTurnInput,
    ) -> Result<TurnMutationResult, StoreError> {
        let mut connection = self.lock()?;
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let dispatch =
            query_task_dispatch(&transaction, input.task_id)?.ok_or(StoreError::TaskNotFound {
                task_id: input.task_id,
            })?;
        if dispatch.owner_boot_id != input.owner_boot_id {
            return Err(StoreError::TaskDispatchOwned {
                task_id: input.task_id,
                owner_boot_id: dispatch.owner_boot_id,
            });
        }
        if dispatch.state == lazoo_protocol::TaskDispatchState::TurnRecorded
            && input.expected_dispatch_revision.checked_add(1) == Some(dispatch.revision)
            && dispatch.session_id == Some(input.session_id)
        {
            let turn_id = dispatch.turn_id.ok_or(StoreError::InvalidRecord {
                field: "task_dispatches.turn_id",
                message: "turn-recorded dispatch is missing its turn id".to_owned(),
            })?;
            let turn = crate::conversation::required_turn(&transaction, turn_id)?;
            let message = message::required_message(&transaction, dispatch.message_id)?;
            let task = required_task(&transaction, input.task_id)?;
            transaction.commit()?;
            return Ok(TurnMutationResult {
                turn,
                message,
                task: Some(task),
                event_sequence: None,
            });
        }
        if dispatch.revision != input.expected_dispatch_revision {
            return Err(StoreError::TaskDispatchStale {
                task_id: input.task_id,
                current_revision: dispatch.revision,
            });
        }
        if dispatch.state != lazoo_protocol::TaskDispatchState::WorkspaceReady {
            return Err(StoreError::TaskDispatchWorkspace {
                task_id: input.task_id,
            });
        }
        let task = required_task(&transaction, input.task_id)?;
        if task.state != TaskState::Accepted {
            return Err(StoreError::TaskState {
                task_id: task.id,
                state: serialized_name(task.state)?,
                expected: "accepted",
            });
        }
        let owned_session = session::query_session(&transaction, input.session_id)?.ok_or(
            StoreError::SessionNotFound {
                session_id: input.session_id,
            },
        )?;
        if owned_session.employee_id != task.assigned_employee_id
            || owned_session.employee_configuration_revision != task.employee_configuration_revision
            || owned_session.state != SessionLifecycleState::Idle
        {
            return Err(StoreError::TaskActorSession {
                employee_id: task.assigned_employee_id,
            });
        }
        if owned_session.owner_boot_id != Some(input.owner_boot_id) {
            return Err(StoreError::SessionOwnership {
                session_id: input.session_id,
            });
        }
        let delegation_message = message::required_message(&transaction, dispatch.message_id)?;
        if delegation_message.delivery.state != MessageDeliveryState::Queued
            || delegation_message.envelope.task_id != Some(task.id)
        {
            return Err(StoreError::TaskAncestryMismatch { task_id: task.id });
        }
        let turn_id = Uuid::new_v4();
        let now = Utc::now().to_rfc3339();
        let (author_kind, author_employee_id, author_session_id) = actor_columns(task.creator);
        transaction.execute(
            "INSERT INTO conversation_turns (
                id, contract_version, conversation_id, task_id, author_kind,
                author_employee_id, author_session_id, recipient_employee_id,
                recipient_session_id, recipient_employee_configuration_revision,
                text, message_id, correlation_id, state, revision, created_at, updated_at
             ) VALUES (
                ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13,
                'recorded', 1, ?14, ?14
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
                input.session_id.to_string(),
                to_i64(task.employee_configuration_revision)?,
                task.objective,
                dispatch.message_id.to_string(),
                delegation_message.envelope.correlation_id.to_string(),
                now,
            ],
        )?;
        transaction.execute(
            "UPDATE task_dispatches SET turn_id = ?2, session_id = ?3,
                    state = 'turn-recorded', revision = revision + 1, updated_at = ?4
             WHERE task_id = ?1",
            params![
                task.id.to_string(),
                turn_id.to_string(),
                input.session_id.to_string(),
                now,
            ],
        )?;
        let turn = crate::conversation::required_turn(&transaction, turn_id)?;
        let updated_dispatch = query_task_dispatch(&transaction, task.id)?
            .ok_or(StoreError::TaskNotFound { task_id: task.id })?;
        let event_sequence = employee::insert_event(
            &transaction,
            "task.dispatch-turn-recorded",
            &json!({"task": &task, "dispatch": &updated_dispatch, "turn": &turn}),
            &now,
        )?;
        transaction.commit()?;
        Ok(TurnMutationResult {
            turn,
            message: delegation_message,
            task: Some(task),
            event_sequence: Some(event_sequence),
        })
    }

    #[allow(clippy::too_many_lines)] // Keep dispatch, task, message, and optional turn failure atomic.
    pub fn fail_task_dispatch(
        &self,
        input: &FailTaskDispatchInput,
    ) -> Result<TaskDispatchFailureResult, StoreError> {
        validate_task_failure(&input.failure_code, &input.failure_message)?;
        let mut connection = self.lock()?;
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let current_dispatch =
            query_task_dispatch(&transaction, input.task_id)?.ok_or(StoreError::TaskNotFound {
                task_id: input.task_id,
            })?;
        if current_dispatch.owner_boot_id != input.owner_boot_id {
            return Err(StoreError::TaskDispatchOwned {
                task_id: input.task_id,
                owner_boot_id: current_dispatch.owner_boot_id,
            });
        }
        let current_task = required_task(&transaction, input.task_id)?;
        if current_dispatch.state == lazoo_protocol::TaskDispatchState::Failed
            && current_task.state == TaskState::Failed
            && current_task.failure_code.as_deref() == Some(input.failure_code.as_str())
            && current_task.failure_message.as_deref() == Some(input.failure_message.as_str())
        {
            let message = message::required_message(&transaction, current_dispatch.message_id)?;
            transaction.commit()?;
            return Ok(TaskDispatchFailureResult {
                task: current_task,
                dispatch: current_dispatch,
                message,
                event_sequence: None,
            });
        }
        if current_task.state != TaskState::Accepted
            || !matches!(
                current_dispatch.state,
                lazoo_protocol::TaskDispatchState::Preparing
                    | lazoo_protocol::TaskDispatchState::WorkspaceReady
                    | lazoo_protocol::TaskDispatchState::TurnRecorded
            )
        {
            return Err(StoreError::TaskState {
                task_id: current_task.id,
                state: serialized_name(current_task.state)?,
                expected: "accepted dispatch before provider acceptance",
            });
        }
        let current_message = message::required_message(&transaction, current_dispatch.message_id)?;
        if current_message.delivery.state != MessageDeliveryState::Queued {
            return Err(StoreError::TaskState {
                task_id: current_task.id,
                state: serialized_name(current_task.state)?,
                expected: "queued canonical dispatch message",
            });
        }
        let claimed_message = message::claim_message_tx(
            &transaction,
            &message::ClaimMessageInput {
                request_id: format!("task-dispatch:{}:fail-claim", input.task_id),
                idempotency_key: format!("task-dispatch:{}:fail-claim", input.task_id),
                message_id: current_dispatch.message_id,
                expected_revision: current_message.delivery.revision,
            },
        )?;
        let attempt_id = claimed_message
            .message
            .attempts
            .last()
            .ok_or(StoreError::InvalidRecord {
                field: "message_delivery_attempts",
                message: "failed dispatch claim has no delivery attempt".to_owned(),
            })?
            .id;
        let failed_message = message::fail_message_tx(
            &transaction,
            &message::FailMessageInput {
                request_id: format!("task-dispatch:{}:fail", input.task_id),
                idempotency_key: format!("task-dispatch:{}:fail", input.task_id),
                message_id: current_dispatch.message_id,
                expected_revision: claimed_message.message.delivery.revision,
                attempt_id,
                retryable: false,
                failure_code: input.failure_code.clone(),
                failure_message: input.failure_message.clone(),
            },
        )?
        .message;
        let now = Utc::now().to_rfc3339();
        if let Some(turn_id) = current_dispatch.turn_id {
            transaction.execute(
                "UPDATE conversation_turns SET state = 'terminal-failed', revision = revision + 1,
                        failure_code = ?2, failure_message = ?3, terminal_at = ?4, updated_at = ?4
                 WHERE id = ?1 AND state = 'recorded'",
                params![
                    turn_id.to_string(),
                    input.failure_code,
                    input.failure_message,
                    now,
                ],
            )?;
        }
        transaction.execute(
            "UPDATE tasks SET state = 'failed', revision = revision + 1,
                    active_turn_id = NULL, active_session_id = NULL,
                    failure_code = ?2, failure_message = ?3,
                    terminal_at = ?4, updated_at = ?4
             WHERE id = ?1",
            params![
                input.task_id.to_string(),
                input.failure_code,
                input.failure_message,
                now,
            ],
        )?;
        transaction.execute(
            "UPDATE task_dispatches SET state = 'failed', revision = revision + 1,
                    failure_code = ?2, failure_message = ?3, updated_at = ?4
             WHERE task_id = ?1",
            params![
                input.task_id.to_string(),
                input.failure_code,
                input.failure_message,
                now,
            ],
        )?;
        let actor = MessageSender::Employee(MessageEndpoint {
            employee_id: current_task.assigned_employee_id,
            session_id: current_dispatch.session_id,
        });
        insert_task_evidence_tx(
            &transaction,
            input.task_id,
            current_task.revision + 1,
            TaskEvidenceKind::Failed,
            actor,
            Some(current_dispatch.message_id),
            current_dispatch.turn_id,
            current_dispatch.session_id,
            &json!({"source": "automatic-dispatch", "failureCode": input.failure_code}),
            &now,
        )?;
        let task = required_task(&transaction, input.task_id)?;
        let dispatch =
            query_task_dispatch(&transaction, input.task_id)?.ok_or(StoreError::TaskNotFound {
                task_id: input.task_id,
            })?;
        let event_sequence = employee::insert_event(
            &transaction,
            "task.dispatch-failed",
            &json!({"task": &task, "dispatch": &dispatch, "message": &failed_message}),
            &now,
        )?;
        transaction.commit()?;
        Ok(TaskDispatchFailureResult {
            task,
            dispatch,
            message: failed_message,
            event_sequence: Some(event_sequence),
        })
    }

    pub fn reconcile_task_dispatches(
        &self,
        owner_boot_id: Uuid,
    ) -> Result<TaskReconcileResult, StoreError> {
        let task_ids = {
            let connection = self.lock()?;
            let mut statement = connection.prepare(
                "SELECT dispatch.task_id
                 FROM task_dispatches dispatch
                 JOIN tasks task ON task.id = dispatch.task_id
                 JOIN messages message ON message.id = dispatch.message_id
                 JOIN message_deliveries delivery ON delivery.message_id = message.id
                 LEFT JOIN conversation_turns turn ON turn.id = dispatch.turn_id
                 WHERE dispatch.owner_boot_id <> ?1
                   AND dispatch.state IN ('preparing', 'workspace-ready', 'turn-recorded')
                   AND task.state = 'accepted'
                   AND delivery.state = 'queued'
                   AND (dispatch.turn_id IS NULL OR turn.state = 'recorded')
                 ORDER BY task.sequence",
            )?;
            statement
                .query_map([owner_boot_id.to_string()], |row| row.get::<_, String>(0))?
                .collect::<Result<Vec<_>, _>>()?
        };
        let mut tasks = Vec::with_capacity(task_ids.len());
        let mut event_sequence = None;
        for id in task_ids {
            let task_id = parse_uuid(&id, "task_dispatches.task_id")?;
            let now = Utc::now().to_rfc3339();
            let transferred = {
                let mut connection = self.lock()?;
                let transaction =
                    connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
                let changed = transaction.execute(
                    "UPDATE task_dispatches SET owner_boot_id = ?2, revision = revision + 1,
                            updated_at = ?3
                     WHERE task_id = ?1 AND owner_boot_id <> ?2
                       AND state IN ('preparing', 'workspace-ready', 'turn-recorded')",
                    params![task_id.to_string(), owner_boot_id.to_string(), now],
                )?;
                transaction.commit()?;
                changed == 1
            };
            if !transferred {
                continue;
            }
            let failed = self.fail_task_dispatch(&FailTaskDispatchInput {
                task_id,
                owner_boot_id,
                failure_code: "LZ-TASK-DISPATCH-OWNER-LOST".to_owned(),
                failure_message: "the daemon stopped before provider acceptance".to_owned(),
            })?;
            event_sequence = failed.event_sequence;
            tasks.push(failed.task);
        }
        Ok(TaskReconcileResult {
            tasks,
            event_sequence,
        })
    }

    pub fn reconcile_tasks(&self) -> Result<TaskReconcileResult, StoreError> {
        let mut connection = self.lock()?;
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let mut statement = transaction.prepare(
            "SELECT task.id
             FROM tasks task
             LEFT JOIN task_dispatches dispatch ON dispatch.task_id = task.id
             JOIN conversation_turns turn ON turn.id = COALESCE(task.active_turn_id, dispatch.turn_id)
             WHERE (
                    task.state IN ('running', 'cancel-requested') OR
                    (task.state = 'accepted' AND dispatch.state = 'turn-recorded')
                   )
               AND turn.state = 'uncertain'
               AND turn.failure_code = 'LZ-TURN-OWNER-LOST'
             ORDER BY task.sequence",
        )?;
        let ids = statement
            .query_map([], |row| row.get::<_, String>(0))?
            .collect::<Result<Vec<_>, _>>()?;
        drop(statement);
        if ids.is_empty() {
            transaction.commit()?;
            return Ok(TaskReconcileResult {
                tasks: Vec::new(),
                event_sequence: None,
            });
        }

        let now = Utc::now().to_rfc3339();
        let mut tasks = Vec::with_capacity(ids.len());
        for id in ids {
            let task_id = parse_uuid(&id, "tasks.id")?;
            let current = required_task(&transaction, task_id)?;
            let current_dispatch = query_task_dispatch(&transaction, task_id)?;
            let turn_id = current
                .active_turn_id
                .or_else(|| {
                    current_dispatch
                        .as_ref()
                        .and_then(|dispatch| dispatch.turn_id)
                })
                .ok_or(StoreError::TaskTurnLink {
                    task_id,
                    turn_id: task_id,
                })?;
            let session_id = current.active_session_id.or_else(|| {
                current_dispatch
                    .as_ref()
                    .and_then(|dispatch| dispatch.session_id)
            });
            let actor = MessageSender::Employee(MessageEndpoint {
                employee_id: current.assigned_employee_id,
                session_id,
            });
            transaction.execute(
                "UPDATE tasks SET state = 'uncertain', revision = revision + 1,
                        active_turn_id = NULL, active_session_id = NULL,
                        failure_code = 'LZ-TASK-OWNER-LOST',
                        failure_message = 'the daemon stopped while this task turn was in flight',
                        terminal_at = ?2, updated_at = ?2
                 WHERE id = ?1",
                params![task_id.to_string(), now],
            )?;
            if current_dispatch.is_some() {
                transaction.execute(
                    "UPDATE task_dispatches SET state = 'uncertain', revision = revision + 1,
                            failure_code = 'LZ-TASK-OWNER-LOST',
                            failure_message = 'the daemon stopped while this task turn was in flight',
                            updated_at = ?2
                     WHERE task_id = ?1 AND state = 'turn-recorded'",
                    params![task_id.to_string(), now],
                )?;
            }
            insert_task_evidence_tx(
                &transaction,
                task_id,
                current.revision + 1,
                TaskEvidenceKind::Uncertain,
                actor,
                current_dispatch
                    .as_ref()
                    .map(|dispatch| dispatch.message_id),
                Some(turn_id),
                session_id,
                &json!({"source": "startup-reconciliation", "turnFailureCode": "LZ-TURN-OWNER-LOST"}),
                &now,
            )?;
            tasks.push(required_task(&transaction, task_id)?);
        }
        let event_sequence = employee::insert_event(
            &transaction,
            "tasks.reconciled",
            &json!({"tasks": &tasks}),
            &now,
        )?;
        transaction.commit()?;
        Ok(TaskReconcileResult {
            tasks,
            event_sequence: Some(event_sequence),
        })
    }

    #[allow(clippy::too_many_lines)] // Keep ancestry checks and atomic child/message writes together.
    pub fn delegate_task(
        &self,
        input: &DelegateTaskInput,
    ) -> Result<TaskMutationResult, StoreError> {
        validate_message_command_key(&input.idempotency_key)?;
        let objective = validate_task_objective(&input.objective)?;
        validate_task_references(&input.references)?;
        let mut connection = self.lock()?;
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        if let Some(task) = task_for_command(&transaction, "task.delegate", &input.idempotency_key)?
        {
            require_delegation_replay(&task, input, &objective)?;
            let delegation_message = task
                .delegation_message_id
                .map(|id| message::required_message(&transaction, id))
                .transpose()?;
            transaction.commit()?;
            return Ok(TaskMutationResult {
                task,
                delegation_message,
                event_sequence: None,
            });
        }

        let parent = required_task(&transaction, input.parent_task_id)?;
        if !matches!(parent.state, TaskState::Accepted | TaskState::Running) {
            return Err(StoreError::TaskState {
                task_id: parent.id,
                state: serialized_name(parent.state)?,
                expected: "accepted or running",
            });
        }
        require_valid_ancestry(&transaction, &parent)?;
        require_actor(&transaction, &parent, input.actor)?;
        if input.assigned_employee_id == parent.assigned_employee_id {
            return Err(StoreError::TaskSelfDelegation {
                employee_id: input.assigned_employee_id,
            });
        }
        let current_roster_revision = employee::roster_revision(&transaction)?;
        if input.roster_revision != current_roster_revision {
            return Err(StoreError::TaskRosterStale {
                current_revision: current_roster_revision,
            });
        }
        let assignee = employee::query_employee(&transaction, input.assigned_employee_id)?.ok_or(
            StoreError::TaskAssigneeUnavailable {
                employee_id: input.assigned_employee_id,
            },
        )?;
        require_route_ready_assignee(&transaction, &assignee)?;
        let requested_authority = input
            .requested_authority
            .unwrap_or(parent.effective_authority);
        if authority_rank(requested_authority) > authority_rank(parent.effective_authority)
            || authority_rank(requested_authority) > authority_rank(assignee.authority_mode)
        {
            return Err(StoreError::TaskAuthorityExpansion {
                requested: serialized_name(requested_authority)?,
            });
        }
        let effective_authority = effective_authority(
            parent.effective_authority,
            requested_authority,
            assignee.authority_mode,
        );
        let depth = parent.depth + 1;
        if depth > lazoo_domain::MAX_TASK_DEPTH {
            return Err(StoreError::TaskDepthLimit {
                parent_task_id: parent.id,
            });
        }
        let active_children: i64 = transaction.query_row(
            "SELECT COUNT(*) FROM tasks
             WHERE parent_task_id = ?1
               AND state NOT IN ('completed', 'failed', 'cancelled')",
            [parent.id.to_string()],
            |row| row.get(0),
        )?;
        if active_children >= 8 {
            return Err(StoreError::TaskFanoutLimit {
                parent_task_id: parent.id,
            });
        }

        let child_id = Uuid::new_v4();
        let now = Utc::now().to_rfc3339();
        let state = if effective_authority == AuthorityMode::Ask {
            TaskState::Proposed
        } else {
            TaskState::Accepted
        };
        insert_child_task_tx(
            &transaction,
            child_id,
            &parent,
            input,
            &objective,
            requested_authority,
            effective_authority,
            assignee.configuration_revision,
            depth,
            state,
            &now,
        )?;
        let delegation_message = if state == TaskState::Accepted {
            let created = insert_delegation_message_tx(
                &transaction,
                child_id,
                &parent,
                input.actor,
                assignee.id,
                &objective,
                &input.references,
                &input.request_id,
            )?;
            transaction.execute(
                "UPDATE tasks SET delegation_message_id = ?2 WHERE id = ?1",
                params![child_id.to_string(), created.envelope.id.to_string()],
            )?;
            Some(created)
        } else {
            None
        };
        let evidence_kind = if state == TaskState::Proposed {
            TaskEvidenceKind::Proposed
        } else {
            TaskEvidenceKind::Accepted
        };
        insert_task_evidence_tx(
            &transaction,
            child_id,
            1,
            evidence_kind,
            MessageSender::Employee(input.actor),
            delegation_message
                .as_ref()
                .map(|message| message.envelope.id),
            None,
            input.actor.session_id,
            &json!({"source": "task.delegate"}),
            &now,
        )?;
        insert_task_command_tx(
            &transaction,
            "task.delegate",
            &input.idempotency_key,
            &input.request_id,
            child_id,
            &now,
        )?;
        let task = required_task(&transaction, child_id)?;
        let event_sequence = employee::insert_event(
            &transaction,
            "task.delegated",
            &json!({"task": &task, "delegationMessage": &delegation_message}),
            &now,
        )?;
        transaction.commit()?;
        Ok(TaskMutationResult {
            task,
            delegation_message,
            event_sequence: Some(event_sequence),
        })
    }

    #[allow(clippy::too_many_lines)] // Keep acceptance and canonical-message creation atomic and auditable.
    pub fn accept_task(&self, input: &AcceptTaskInput) -> Result<TaskMutationResult, StoreError> {
        validate_message_command_key(&input.idempotency_key)?;
        let mut connection = self.lock()?;
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        if let Some(task) = task_for_command(&transaction, "task.accept", &input.idempotency_key)? {
            if task.id != input.task_id {
                return Err(StoreError::TaskIdempotencyConflict);
            }
            let delegation_message = task
                .delegation_message_id
                .map(|id| message::required_message(&transaction, id))
                .transpose()?;
            transaction.commit()?;
            return Ok(TaskMutationResult {
                task,
                delegation_message,
                event_sequence: None,
            });
        }
        let current = required_task(&transaction, input.task_id)?;
        require_task_revision(&current, input.expected_revision)?;
        if current.state != TaskState::Proposed {
            return Err(StoreError::TaskState {
                task_id: current.id,
                state: serialized_name(current.state)?,
                expected: "proposed",
            });
        }
        let parent_id = current
            .parent_task_id
            .ok_or(StoreError::TaskAncestryMismatch {
                task_id: current.id,
            })?;
        let parent = required_task(&transaction, parent_id)?;
        require_valid_ancestry(&transaction, &current)?;
        let sender = match current.creator {
            MessageSender::Employee(endpoint)
                if endpoint.employee_id == parent.assigned_employee_id =>
            {
                endpoint
            }
            _ => {
                return Err(StoreError::TaskAncestryMismatch {
                    task_id: current.id,
                });
            }
        };
        let now = Utc::now().to_rfc3339();
        let delegation_message = insert_delegation_message_tx(
            &transaction,
            current.id,
            &parent,
            sender,
            current.assigned_employee_id,
            &current.objective,
            &current.references,
            &input.request_id,
        )?;
        transaction.execute(
            "UPDATE tasks SET state = 'accepted', revision = revision + 1,
                    delegation_message_id = ?2, updated_at = ?3
             WHERE id = ?1",
            params![
                current.id.to_string(),
                delegation_message.envelope.id.to_string(),
                now,
            ],
        )?;
        insert_task_evidence_tx(
            &transaction,
            current.id,
            current.revision + 1,
            TaskEvidenceKind::Accepted,
            MessageSender::User(UserMessageEndpoint {
                kind: UserMessageKind::User,
            }),
            Some(delegation_message.envelope.id),
            None,
            None,
            &json!({"source": "task.accept"}),
            &now,
        )?;
        insert_task_command_tx(
            &transaction,
            "task.accept",
            &input.idempotency_key,
            &input.request_id,
            current.id,
            &now,
        )?;
        let task = required_task(&transaction, current.id)?;
        let event_sequence = employee::insert_event(
            &transaction,
            "task.accepted",
            &json!({"task": &task, "delegationMessage": &delegation_message}),
            &now,
        )?;
        transaction.commit()?;
        Ok(TaskMutationResult {
            task,
            delegation_message: Some(delegation_message),
            event_sequence: Some(event_sequence),
        })
    }

    #[allow(clippy::too_many_lines)] // Keep projected-start replay and standalone validation atomic.
    pub fn start_task(&self, input: &StartTaskInput) -> Result<TaskMutationResult, StoreError> {
        validate_message_command_key(&input.idempotency_key)?;
        validate_task_evidence(&input.evidence)?;
        let mut connection = self.lock()?;
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        if let Some(result) = mutation_for_command(
            &transaction,
            "task.start",
            &input.idempotency_key,
            input.task_id,
        )? {
            transaction.commit()?;
            return Ok(result);
        }
        let current = required_task(&transaction, input.task_id)?;
        if current.state == TaskState::Running
            && current.active_turn_id == Some(input.turn_id)
            && current.active_session_id == input.actor.session_id
            && input.expected_revision.checked_add(1) == Some(current.revision)
        {
            require_actor(&transaction, &current, input.actor)?;
            let turn = crate::conversation::required_turn(&transaction, input.turn_id)?;
            if turn.state != TurnState::Accepted
                || turn.task_id != Some(current.id)
                || turn.conversation_id != current.conversation_id
                || turn.recipient != input.actor
                || turn.native_turn_id.is_none()
            {
                return Err(StoreError::TaskAcceptanceEvidence {
                    task_id: current.id,
                    turn_id: turn.id,
                });
            }
            let now = Utc::now().to_rfc3339();
            insert_task_command_tx(
                &transaction,
                "task.start",
                &input.idempotency_key,
                &input.request_id,
                current.id,
                &now,
            )?;
            let delegation_message = current
                .delegation_message_id
                .map(|id| message::required_message(&transaction, id))
                .transpose()?;
            transaction.commit()?;
            return Ok(TaskMutationResult {
                task: current,
                delegation_message,
                event_sequence: None,
            });
        }
        require_task_revision(&current, input.expected_revision)?;
        if current.state != TaskState::Accepted {
            return Err(StoreError::TaskState {
                task_id: current.id,
                state: serialized_name(current.state)?,
                expected: "accepted",
            });
        }
        require_actor(&transaction, &current, input.actor)?;
        let turn = crate::conversation::required_turn(&transaction, input.turn_id)?;
        let accepted = turn.state == TurnState::Accepted
            && turn.task_id == Some(current.id)
            && turn.conversation_id == current.conversation_id
            && turn.recipient == input.actor
            && turn.native_turn_id.is_some();
        if !accepted {
            return Err(StoreError::TaskAcceptanceEvidence {
                task_id: current.id,
                turn_id: turn.id,
            });
        }
        let now = Utc::now().to_rfc3339();
        transaction.execute(
            "UPDATE tasks SET state = 'running', revision = revision + 1,
                    active_turn_id = ?2, active_session_id = ?3, updated_at = ?4
             WHERE id = ?1",
            params![
                current.id.to_string(),
                turn.id.to_string(),
                input.actor.session_id.map(|value| value.to_string()),
                now,
            ],
        )?;
        insert_task_evidence_tx(
            &transaction,
            current.id,
            current.revision + 1,
            TaskEvidenceKind::Started,
            MessageSender::Employee(input.actor),
            None,
            Some(turn.id),
            input.actor.session_id,
            &input.evidence,
            &now,
        )?;
        insert_task_command_tx(
            &transaction,
            "task.start",
            &input.idempotency_key,
            &input.request_id,
            current.id,
            &now,
        )?;
        let result = effective_task_result(&transaction, current.id, "task.started", &now)?;
        transaction.commit()?;
        Ok(result)
    }

    pub fn report_task(&self, input: &ReportTaskInput) -> Result<TaskMutationResult, StoreError> {
        validate_message_command_key(&input.idempotency_key)?;
        validate_task_evidence(&input.evidence)?;
        validate_failure_fields(
            input.kind,
            input.failure_code.as_deref(),
            input.failure_message.as_deref(),
        )?;
        let mut connection = self.lock()?;
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        if let Some(result) = mutation_for_command(
            &transaction,
            "task.report",
            &input.idempotency_key,
            input.task_id,
        )? {
            transaction.commit()?;
            return Ok(result);
        }
        let current = required_task(&transaction, input.task_id)?;
        require_task_revision(&current, input.expected_revision)?;
        require_actor(&transaction, &current, input.actor)?;
        if current.active_session_id.is_some()
            && current.active_session_id != input.actor.session_id
        {
            return Err(StoreError::TaskActorSession {
                employee_id: input.actor.employee_id,
            });
        }
        let (target, evidence_kind) = match input.kind {
            TaskReportKind::Progress if current.state == TaskState::Running => {
                (TaskState::Running, TaskEvidenceKind::Progress)
            }
            TaskReportKind::Completed if current.state == TaskState::Running => {
                require_no_active_descendants(&transaction, current.id)?;
                (TaskState::Completed, TaskEvidenceKind::Completed)
            }
            TaskReportKind::Failed
                if matches!(
                    current.state,
                    TaskState::Accepted | TaskState::Running | TaskState::CancelRequested
                ) =>
            {
                (TaskState::Failed, TaskEvidenceKind::Failed)
            }
            _ => {
                return Err(StoreError::TaskState {
                    task_id: current.id,
                    state: serialized_name(current.state)?,
                    expected: "a state compatible with the report kind",
                });
            }
        };
        let now = Utc::now().to_rfc3339();
        let terminal = matches!(target, TaskState::Completed | TaskState::Failed);
        transaction.execute(
            "UPDATE tasks SET state = ?2, revision = revision + 1,
                    failure_code = ?3, failure_message = ?4,
                    active_turn_id = CASE WHEN ?5 THEN NULL ELSE active_turn_id END,
                    active_session_id = CASE WHEN ?5 THEN NULL ELSE active_session_id END,
                    terminal_at = CASE WHEN ?5 THEN ?6 ELSE terminal_at END,
                    updated_at = ?6
             WHERE id = ?1",
            params![
                current.id.to_string(),
                serialized_name(target)?,
                input.failure_code,
                input.failure_message,
                terminal,
                now,
            ],
        )?;
        insert_task_evidence_tx(
            &transaction,
            current.id,
            current.revision + 1,
            evidence_kind,
            MessageSender::Employee(input.actor),
            None,
            current.active_turn_id,
            input.actor.session_id,
            &input.evidence,
            &now,
        )?;
        insert_task_command_tx(
            &transaction,
            "task.report",
            &input.idempotency_key,
            &input.request_id,
            current.id,
            &now,
        )?;
        let event_type = match input.kind {
            TaskReportKind::Progress => "task.progressed",
            TaskReportKind::Completed => "task.completed",
            TaskReportKind::Failed => "task.failed",
        };
        let result = effective_task_result(&transaction, current.id, event_type, &now)?;
        transaction.commit()?;
        Ok(result)
    }

    pub fn cancel_task(&self, input: &CancelTaskInput) -> Result<TaskMutationResult, StoreError> {
        validate_message_command_key(&input.idempotency_key)?;
        validate_task_evidence(&input.evidence)?;
        let mut connection = self.lock()?;
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        if let Some(result) = mutation_for_command(
            &transaction,
            "task.cancel",
            &input.idempotency_key,
            input.task_id,
        )? {
            transaction.commit()?;
            return Ok(result);
        }
        let current = required_task(&transaction, input.task_id)?;
        require_task_revision(&current, input.expected_revision)?;
        if let MessageSender::Employee(actor) = input.actor {
            require_actor(&transaction, &current, actor)?;
            if current.active_session_id.is_some() && current.active_session_id != actor.session_id
            {
                return Err(StoreError::TaskActorSession {
                    employee_id: actor.employee_id,
                });
            }
        }
        let (target, evidence_kind) = task_cancel_target(&transaction, &current)?;
        let now = Utc::now().to_rfc3339();
        let terminal = target == TaskState::Cancelled;
        if terminal {
            cancel_queued_task_delivery_tx(&transaction, &current, &now)?;
        }
        transaction.execute(
            "UPDATE tasks SET state = ?2, revision = revision + 1,
                    active_turn_id = CASE WHEN ?3 THEN NULL ELSE active_turn_id END,
                    active_session_id = CASE WHEN ?3 THEN NULL ELSE active_session_id END,
                    terminal_at = CASE WHEN ?3 THEN ?4 ELSE terminal_at END,
                    updated_at = ?4
             WHERE id = ?1",
            params![
                current.id.to_string(),
                serialized_name(target)?,
                terminal,
                now,
            ],
        )?;
        insert_task_evidence_tx(
            &transaction,
            current.id,
            current.revision + 1,
            evidence_kind,
            input.actor,
            None,
            current.active_turn_id,
            current.active_session_id,
            &input.evidence,
            &now,
        )?;
        insert_task_command_tx(
            &transaction,
            "task.cancel",
            &input.idempotency_key,
            &input.request_id,
            current.id,
            &now,
        )?;
        let event_type = if terminal {
            "task.cancelled"
        } else {
            "task.cancel-requested"
        };
        let result = effective_task_result(&transaction, current.id, event_type, &now)?;
        transaction.commit()?;
        Ok(result)
    }

    pub fn resolve_task(&self, input: &ResolveTaskInput) -> Result<TaskMutationResult, StoreError> {
        validate_message_command_key(&input.idempotency_key)?;
        validate_task_evidence(&input.evidence)?;
        validate_resolution_failure_fields(
            input.resolution,
            input.failure_code.as_deref(),
            input.failure_message.as_deref(),
        )?;
        let mut connection = self.lock()?;
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        if let Some(result) = mutation_for_command(
            &transaction,
            "task.resolve",
            &input.idempotency_key,
            input.task_id,
        )? {
            transaction.commit()?;
            return Ok(result);
        }
        let current = required_task(&transaction, input.task_id)?;
        require_task_revision(&current, input.expected_revision)?;
        if current.state != TaskState::Uncertain {
            return Err(StoreError::TaskState {
                task_id: current.id,
                state: serialized_name(current.state)?,
                expected: "uncertain",
            });
        }
        let (target, evidence_kind, event_type) = match input.resolution {
            TaskResolution::Completed => (
                TaskState::Completed,
                TaskEvidenceKind::Completed,
                "task.completed",
            ),
            TaskResolution::Failed => (TaskState::Failed, TaskEvidenceKind::Failed, "task.failed"),
            TaskResolution::Cancelled => (
                TaskState::Cancelled,
                TaskEvidenceKind::Cancelled,
                "task.cancelled",
            ),
        };
        if matches!(target, TaskState::Completed | TaskState::Cancelled) {
            require_no_active_descendants(&transaction, current.id)?;
        }
        let now = Utc::now().to_rfc3339();
        transaction.execute(
            "UPDATE tasks SET state = ?2, revision = revision + 1,
                    failure_code = ?3, failure_message = ?4,
                    active_turn_id = NULL, active_session_id = NULL,
                    terminal_at = ?5, updated_at = ?5
             WHERE id = ?1",
            params![
                current.id.to_string(),
                serialized_name(target)?,
                input.failure_code,
                input.failure_message,
                now,
            ],
        )?;
        insert_task_evidence_tx(
            &transaction,
            current.id,
            current.revision + 1,
            evidence_kind,
            MessageSender::User(UserMessageEndpoint {
                kind: UserMessageKind::User,
            }),
            None,
            current.active_turn_id,
            current.active_session_id,
            &input.evidence,
            &now,
        )?;
        insert_task_command_tx(
            &transaction,
            "task.resolve",
            &input.idempotency_key,
            &input.request_id,
            current.id,
            &now,
        )?;
        let result = effective_task_result(&transaction, current.id, event_type, &now)?;
        transaction.commit()?;
        Ok(result)
    }
}

pub(crate) fn insert_root_task_tx(
    transaction: &Transaction<'_>,
    input: &InsertRootTaskInput,
) -> Result<TaskRecord, StoreError> {
    transaction.execute(
        "INSERT INTO tasks (
            id, contract_version, root_task_id, parent_task_id, depth, project_id,
            conversation_id, creation_turn_id, delegation_message_id, creator_kind,
            creator_employee_id, creator_session_id, assigned_employee_id, role,
            objective, references_json, requested_authority, effective_authority,
            employee_configuration_revision, roster_revision, state, revision,
            active_turn_id, active_session_id, created_at, updated_at
         ) VALUES (
            ?1, ?2, ?1, NULL, 0, ?3, ?4, ?1, NULL, 'user', NULL, NULL, ?5,
            'lead', ?6, '[]', ?7, ?8, ?9, ?10, 'accepted', 1, ?1, NULL, ?11, ?11
         )",
        params![
            input.id.to_string(),
            i64::from(TASK_CONTRACT_V1),
            input.project_id.map(|value| value.to_string()),
            input.conversation_id.to_string(),
            input.assigned_employee_id.to_string(),
            input.objective,
            serialized_name(input.requested_authority)?,
            serialized_name(input.effective_authority)?,
            to_i64(input.employee_configuration_revision)?,
            to_i64(input.roster_revision)?,
            input.created_at,
        ],
    )?;
    required_task(transaction, input.id)
}

pub(crate) fn insert_root_evidence_tx(
    transaction: &Transaction<'_>,
    task_id: Uuid,
    message_id: Uuid,
    now: &str,
) -> Result<TaskEvidence, StoreError> {
    let evidence_id = Uuid::new_v4();
    transaction.execute(
        "INSERT INTO task_evidence (
            id, task_id, task_revision, kind, actor_kind, actor_employee_id,
            actor_session_id, message_id, turn_id, session_id, detail_json, created_at
         ) VALUES (?1, ?2, 1, 'accepted', 'user', NULL, NULL, ?3, ?2, NULL, ?4, ?5)",
        params![
            evidence_id.to_string(),
            task_id.to_string(),
            message_id.to_string(),
            serde_json::to_string(&json!({"source": "turn.record"}))?,
            now,
        ],
    )?;
    required_task_evidence(transaction, evidence_id)
}

#[allow(clippy::too_many_arguments)]
fn insert_child_task_tx(
    transaction: &Transaction<'_>,
    task_id: Uuid,
    parent: &TaskRecord,
    input: &DelegateTaskInput,
    objective: &str,
    requested_authority: AuthorityMode,
    effective_authority: AuthorityMode,
    employee_configuration_revision: u64,
    depth: u32,
    state: TaskState,
    now: &str,
) -> Result<(), StoreError> {
    transaction.execute(
        "INSERT INTO tasks (
            id, contract_version, root_task_id, parent_task_id, depth, project_id,
            conversation_id, creation_turn_id, delegation_message_id, creator_kind,
            creator_employee_id, creator_session_id, assigned_employee_id, role,
            objective, references_json, requested_authority, effective_authority,
            employee_configuration_revision, roster_revision, state, revision,
            active_turn_id, active_session_id, created_at, updated_at
         ) VALUES (
            ?1, ?2, ?3, ?4, ?5, ?6, ?7, NULL, NULL, 'employee', ?8, ?9, ?10,
            'worker', ?11, ?12, ?13, ?14, ?15, ?16, ?17, 1, NULL, NULL, ?18, ?18
         )",
        params![
            task_id.to_string(),
            i64::from(TASK_CONTRACT_V1),
            parent.root_task_id.to_string(),
            parent.id.to_string(),
            i64::from(depth),
            parent.project_id.map(|value| value.to_string()),
            parent.conversation_id.to_string(),
            input.actor.employee_id.to_string(),
            input.actor.session_id.map(|value| value.to_string()),
            input.assigned_employee_id.to_string(),
            objective,
            serde_json::to_string(&input.references)?,
            serialized_name(requested_authority)?,
            serialized_name(effective_authority)?,
            to_i64(employee_configuration_revision)?,
            to_i64(input.roster_revision)?,
            serialized_name(state)?,
            now,
        ],
    )?;
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn insert_delegation_message_tx(
    transaction: &Transaction<'_>,
    task_id: Uuid,
    parent: &TaskRecord,
    sender: MessageEndpoint,
    assigned_employee_id: Uuid,
    objective: &str,
    references: &[MessageReference],
    request_id: &str,
) -> Result<MessageRecord, StoreError> {
    let recipient = MessageEndpoint {
        employee_id: assigned_employee_id,
        session_id: session::active_session_id_for_employee(transaction, assigned_employee_id)?,
    };
    let result = message::insert_message_tx(
        transaction,
        &message::InsertMessageInput {
            request_id: request_id.to_owned(),
            command_scope: "task.delegation-message",
            idempotency_key: format!("task-delegation-{task_id}"),
            id: None,
            version: MESSAGE_ENVELOPE_V1,
            sender: MessageSender::Employee(sender),
            recipient,
            project_id: parent.project_id,
            conversation_id: Some(parent.conversation_id),
            task_id: Some(task_id),
            correlation_id: None,
            causation_id: parent.delegation_message_id,
            kind: MessageKind::Delegation,
            body: json!({"objective": objective}),
            references: references.to_vec(),
            max_attempts: 3,
        },
    )?;
    Ok(result.message)
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn insert_task_evidence_tx(
    transaction: &Transaction<'_>,
    task_id: Uuid,
    task_revision: u64,
    kind: TaskEvidenceKind,
    actor: MessageSender,
    message_id: Option<Uuid>,
    turn_id: Option<Uuid>,
    session_id: Option<Uuid>,
    detail: &Value,
    now: &str,
) -> Result<(), StoreError> {
    let (actor_kind, actor_employee_id, actor_session_id) = actor_columns(actor);
    transaction.execute(
        "INSERT INTO task_evidence (
            id, task_id, task_revision, kind, actor_kind, actor_employee_id,
            actor_session_id, message_id, turn_id, session_id, detail_json, created_at
         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)",
        params![
            Uuid::new_v4().to_string(),
            task_id.to_string(),
            to_i64(task_revision)?,
            serialized_name(kind)?,
            actor_kind,
            actor_employee_id,
            actor_session_id,
            message_id.map(|value| value.to_string()),
            turn_id.map(|value| value.to_string()),
            session_id.map(|value| value.to_string()),
            serde_json::to_string(detail)?,
            now,
        ],
    )?;
    Ok(())
}

#[allow(clippy::too_many_lines)] // The explicit projection table is kept together for auditability.
pub(crate) fn project_turn_tx(
    transaction: &Transaction<'_>,
    turn_id: Uuid,
    cause: &str,
    now: &str,
) -> Result<(), StoreError> {
    let turn = crate::conversation::required_turn(transaction, turn_id)?;
    let Some(task_id) = turn.task_id else {
        return Ok(());
    };
    let current = required_task(transaction, task_id)?;
    require_valid_ancestry(transaction, &current)?;
    let conversation_project_id = transaction.query_row(
        "SELECT project_id FROM conversations WHERE id = ?1",
        [turn.conversation_id.to_string()],
        |row| row.get::<_, Option<String>>(0),
    )?;
    let conversation_project_id = conversation_project_id
        .as_deref()
        .map(|value| parse_uuid(value, "conversations.project_id"))
        .transpose()?;
    let exact_scope = current.conversation_id == turn.conversation_id
        && current.project_id == conversation_project_id
        && current.assigned_employee_id == turn.recipient.employee_id
        && current
            .active_turn_id
            .is_none_or(|active_turn_id| active_turn_id == turn.id)
        && current
            .active_session_id
            .is_none_or(|active_session_id| turn.recipient.session_id == Some(active_session_id));
    if !exact_scope {
        return Err(StoreError::TaskTurnLink {
            task_id: current.id,
            turn_id: turn.id,
        });
    }

    let active_descendants = has_active_descendants(transaction, current.id)?;
    // Answering a peer message is not finishing the task: the result notices that follow (or
    // the employee's own turn) complete it, so a peer turn never closes a task early.
    let peer_turn = crate::delivery::is_peer_only_turn(transaction, &turn)?;
    let projection = match (current.state, turn.state, cause) {
        (TaskState::Accepted, TurnState::Accepted, _) => Some((
            TaskState::Running,
            TaskEvidenceKind::Started,
            "task.started",
            false,
        )),
        (TaskState::Running | TaskState::CancelRequested, TurnState::Accepted, _)
            if cause != "turn-cancel-requested" =>
        {
            Some((
                current.state,
                TaskEvidenceKind::Progress,
                "task.progressed",
                false,
            ))
        }
        (TaskState::Running, TurnState::Completed, _) if active_descendants || peer_turn => Some((
            TaskState::Running,
            TaskEvidenceKind::Progress,
            "task.progressed",
            true,
        )),
        (TaskState::Running | TaskState::CancelRequested, TurnState::Completed, _) => Some((
            TaskState::Completed,
            TaskEvidenceKind::Completed,
            "task.completed",
            true,
        )),
        (
            TaskState::Accepted | TaskState::Running | TaskState::CancelRequested,
            TurnState::TerminalFailed,
            _,
        ) => Some((
            TaskState::Failed,
            TaskEvidenceKind::Failed,
            "task.failed",
            true,
        )),
        (
            TaskState::Accepted | TaskState::Running | TaskState::CancelRequested,
            TurnState::Cancelled,
            _,
        ) => Some((
            TaskState::Cancelled,
            TaskEvidenceKind::Cancelled,
            "task.cancelled",
            true,
        )),
        (TaskState::Running | TaskState::CancelRequested, TurnState::Uncertain, _) => Some((
            TaskState::Uncertain,
            TaskEvidenceKind::Uncertain,
            "task.uncertain",
            true,
        )),
        (TaskState::Running, _, "turn-cancel-requested") => Some((
            TaskState::CancelRequested,
            TaskEvidenceKind::CancelRequested,
            "task.cancel-requested",
            false,
        )),
        _ => None,
    };
    let Some((target, evidence_kind, event_type, clear_active)) = projection else {
        return Ok(());
    };
    let terminal = matches!(
        target,
        TaskState::Completed | TaskState::Failed | TaskState::Cancelled | TaskState::Uncertain
    );
    let (failure_code, failure_message) = if target == TaskState::Failed {
        (
            turn.failure_code
                .clone()
                .or_else(|| Some("LZ-TASK-TURN-FAILED".to_owned())),
            turn.failure_message
                .clone()
                .or_else(|| Some("the linked provider turn failed".to_owned())),
        )
    } else if target == TaskState::Uncertain {
        (turn.failure_code.clone(), turn.failure_message.clone())
    } else {
        (None, None)
    };
    transaction.execute(
        "UPDATE tasks SET state = ?2, revision = revision + 1,
                active_turn_id = CASE WHEN ?3 THEN NULL ELSE COALESCE(active_turn_id, ?4) END,
                active_session_id = CASE WHEN ?3 THEN NULL ELSE COALESCE(active_session_id, ?5) END,
                failure_code = ?6, failure_message = ?7,
                terminal_at = CASE WHEN ?8 THEN ?9 ELSE terminal_at END,
                updated_at = ?9
         WHERE id = ?1",
        params![
            current.id.to_string(),
            serialized_name(target)?,
            clear_active,
            turn.id.to_string(),
            turn.recipient.session_id.map(|value| value.to_string()),
            failure_code,
            failure_message,
            terminal,
            now,
        ],
    )?;
    let detail = json!({
        "turnEventKind": cause,
        "turnState": serialized_name(turn.state)?,
    });
    insert_task_evidence_tx(
        transaction,
        current.id,
        current.revision + 1,
        evidence_kind,
        MessageSender::Employee(turn.recipient),
        Some(turn.message_id),
        Some(turn.id),
        turn.recipient.session_id,
        &detail,
        now,
    )?;
    let task = required_task(transaction, current.id)?;
    employee::insert_event(transaction, event_type, &json!({"task": &task}), now)?;
    Ok(())
}

fn insert_task_command_tx(
    transaction: &Transaction<'_>,
    scope: &str,
    idempotency_key: &str,
    request_id: &str,
    task_id: Uuid,
    now: &str,
) -> Result<(), StoreError> {
    transaction.execute(
        "INSERT INTO task_commands (scope, idempotency_key, request_id, task_id, created_at)
         VALUES (?1, ?2, ?3, ?4, ?5)",
        params![scope, idempotency_key, request_id, task_id.to_string(), now],
    )?;
    Ok(())
}

fn mutation_for_command(
    connection: &Connection,
    scope: &str,
    idempotency_key: &str,
    expected_task_id: Uuid,
) -> Result<Option<TaskMutationResult>, StoreError> {
    task_for_command(connection, scope, idempotency_key)?
        .map(|task| {
            if task.id != expected_task_id {
                return Err(StoreError::TaskIdempotencyConflict);
            }
            let delegation_message = task
                .delegation_message_id
                .map(|id| message::required_message(connection, id))
                .transpose()?;
            Ok(TaskMutationResult {
                task,
                delegation_message,
                event_sequence: None,
            })
        })
        .transpose()
}

fn effective_task_result(
    transaction: &Transaction<'_>,
    task_id: Uuid,
    event_type: &str,
    now: &str,
) -> Result<TaskMutationResult, StoreError> {
    let task = required_task(transaction, task_id)?;
    let delegation_message = task
        .delegation_message_id
        .map(|id| message::required_message(transaction, id))
        .transpose()?;
    let event_sequence =
        employee::insert_event(transaction, event_type, &json!({"task": &task}), now)?;
    Ok(TaskMutationResult {
        task,
        delegation_message,
        event_sequence: Some(event_sequence),
    })
}

fn cancel_queued_task_delivery_tx(
    transaction: &Transaction<'_>,
    task: &TaskRecord,
    now: &str,
) -> Result<(), StoreError> {
    if let Some(message_id) = task.delegation_message_id {
        let canonical = message::required_message(transaction, message_id)?;
        if matches!(
            canonical.delivery.state,
            MessageDeliveryState::Queued
                | MessageDeliveryState::Delivering
                | MessageDeliveryState::RetryableFailed
        ) {
            message::cancel_message_tx(
                transaction,
                &message::CancelMessageInput {
                    request_id: format!("task-cancel:{}:message", task.id),
                    idempotency_key: format!("task-cancel:{}:message", task.id),
                    message_id,
                    expected_revision: canonical.delivery.revision,
                },
            )?;
        }
    }
    let Some(dispatch) = query_task_dispatch(transaction, task.id)? else {
        return Ok(());
    };
    if !matches!(
        dispatch.state,
        lazoo_protocol::TaskDispatchState::Preparing
            | lazoo_protocol::TaskDispatchState::WorkspaceReady
            | lazoo_protocol::TaskDispatchState::TurnRecorded
    ) {
        return Ok(());
    }
    if let Some(turn_id) = dispatch.turn_id {
        transaction.execute(
            "UPDATE conversation_turns SET state = 'cancelled',
                    revision = revision + 1, terminal_at = ?2, updated_at = ?2
             WHERE id = ?1 AND state = 'recorded'",
            params![turn_id.to_string(), now],
        )?;
    }
    transaction.execute(
        "UPDATE task_dispatches SET state = 'cancelled', revision = revision + 1,
                updated_at = ?2 WHERE task_id = ?1",
        params![task.id.to_string(), now],
    )?;
    Ok(())
}

fn task_cancel_target(
    transaction: &Transaction<'_>,
    task: &TaskRecord,
) -> Result<(TaskState, TaskEvidenceKind), StoreError> {
    let target = match task.state {
        TaskState::Accepted if automatic_dispatch_turn_is_in_flight(transaction, task.id)? => (
            TaskState::CancelRequested,
            TaskEvidenceKind::CancelRequested,
        ),
        TaskState::Proposed | TaskState::Accepted => {
            (TaskState::Cancelled, TaskEvidenceKind::Cancelled)
        }
        TaskState::Running => (
            TaskState::CancelRequested,
            TaskEvidenceKind::CancelRequested,
        ),
        _ => {
            return Err(StoreError::TaskState {
                task_id: task.id,
                state: serialized_name(task.state)?,
                expected: "proposed, accepted, or running",
            });
        }
    };
    require_no_active_descendants(transaction, task.id)?;
    Ok(target)
}

fn automatic_dispatch_turn_is_in_flight(
    transaction: &Transaction<'_>,
    task_id: Uuid,
) -> Result<bool, StoreError> {
    let Some(turn_id) =
        query_task_dispatch(transaction, task_id)?.and_then(|dispatch| dispatch.turn_id)
    else {
        return Ok(false);
    };
    Ok(matches!(
        conversation::required_turn(transaction, turn_id)?.state,
        TurnState::Submitting | TurnState::Accepted
    ))
}

fn validate_failure_fields(
    kind: TaskReportKind,
    failure_code: Option<&str>,
    failure_message: Option<&str>,
) -> Result<(), StoreError> {
    match (kind, failure_code, failure_message) {
        (TaskReportKind::Failed, Some(code), Some(message)) => {
            validate_task_failure(code, message)?;
            Ok(())
        }
        (TaskReportKind::Failed, _, _) => Err(StoreError::TaskFailureDetails),
        (_, None, None) => Ok(()),
        _ => Err(StoreError::TaskFailureDetails),
    }
}

fn validate_resolution_failure_fields(
    resolution: TaskResolution,
    failure_code: Option<&str>,
    failure_message: Option<&str>,
) -> Result<(), StoreError> {
    match (resolution, failure_code, failure_message) {
        (TaskResolution::Failed, Some(code), Some(message)) => {
            validate_task_failure(code, message)?;
            Ok(())
        }
        (TaskResolution::Failed, _, _) => Err(StoreError::TaskFailureDetails),
        (_, None, None) => Ok(()),
        _ => Err(StoreError::TaskFailureDetails),
    }
}

fn require_no_active_descendants(connection: &Connection, task_id: Uuid) -> Result<(), StoreError> {
    if has_active_descendants(connection, task_id)? {
        Err(StoreError::TaskActiveChildren { task_id })
    } else {
        Ok(())
    }
}

fn has_active_descendants(connection: &Connection, task_id: Uuid) -> Result<bool, StoreError> {
    let active: i64 = connection.query_row(
        "WITH RECURSIVE descendants(id, state) AS (
            SELECT id, state FROM tasks WHERE parent_task_id = ?1
            UNION ALL
            SELECT child.id, child.state
            FROM tasks child
            JOIN descendants parent ON child.parent_task_id = parent.id
         )
         SELECT COUNT(*) FROM descendants
         WHERE state NOT IN ('completed', 'failed', 'cancelled')",
        [task_id.to_string()],
        |row| row.get(0),
    )?;
    Ok(active != 0)
}

fn task_for_command(
    connection: &Connection,
    scope: &str,
    idempotency_key: &str,
) -> Result<Option<TaskRecord>, StoreError> {
    connection
        .query_row(
            "SELECT task_id FROM task_commands WHERE scope = ?1 AND idempotency_key = ?2",
            params![scope, idempotency_key],
            |row| row.get::<_, String>(0),
        )
        .optional()?
        .map(|id| required_task(connection, parse_uuid(&id, "task_commands.task_id")?))
        .transpose()
}

fn require_delegation_replay(
    task: &TaskRecord,
    input: &DelegateTaskInput,
    objective: &str,
) -> Result<(), StoreError> {
    let requested_matches = input
        .requested_authority
        .is_none_or(|requested| requested == task.requested_authority);
    if task.parent_task_id == Some(input.parent_task_id)
        && task.creator == MessageSender::Employee(input.actor)
        && task.assigned_employee_id == input.assigned_employee_id
        && task.objective == objective
        && task.references == input.references
        && requested_matches
    {
        Ok(())
    } else {
        Err(StoreError::TaskIdempotencyConflict)
    }
}

fn require_actor(
    connection: &Connection,
    parent: &TaskRecord,
    actor: MessageEndpoint,
) -> Result<(), StoreError> {
    if actor.employee_id != parent.assigned_employee_id {
        return Err(StoreError::TaskActorMismatch {
            task_id: parent.id,
            employee_id: actor.employee_id,
        });
    }
    let current = employee::query_employee(connection, actor.employee_id)?.ok_or(
        StoreError::TaskActorMismatch {
            task_id: parent.id,
            employee_id: actor.employee_id,
        },
    )?;
    if current.configuration_revision != parent.employee_configuration_revision {
        return Err(StoreError::TaskEmployeeStale {
            employee_id: current.id,
            current_revision: current.configuration_revision,
        });
    }
    let session_id = actor.session_id.ok_or(StoreError::TaskActorSession {
        employee_id: actor.employee_id,
    })?;
    let actor_session =
        session::query_session(connection, session_id)?.ok_or(StoreError::TaskActorSession {
            employee_id: actor.employee_id,
        })?;
    if actor_session.employee_id != actor.employee_id
        || actor_session.employee_configuration_revision != current.configuration_revision
    {
        return Err(StoreError::TaskActorSession {
            employee_id: actor.employee_id,
        });
    }
    Ok(())
}

fn require_route_ready_assignee(
    connection: &Connection,
    assignee: &lazoo_protocol::EmployeeRecord,
) -> Result<(), StoreError> {
    if !assignee.enabled {
        return Err(StoreError::TaskAssigneeUnavailable {
            employee_id: assignee.id,
        });
    }
    let observation = employee::query_provider_observation(connection, &assignee.provider)?;
    let (availability, _) = derive_availability(true, observation.as_ref());
    if availability == EmployeeAvailability::Dormant {
        Ok(())
    } else {
        Err(StoreError::TaskAssigneeUnavailable {
            employee_id: assignee.id,
        })
    }
}

pub(crate) fn require_valid_ancestry(
    connection: &Connection,
    task: &TaskRecord,
) -> Result<(), StoreError> {
    let root = required_task(connection, task.root_task_id)?;
    let valid_root = root.id == root.root_task_id
        && root.parent_task_id.is_none()
        && root.depth == 0
        && root.role == TaskRole::Lead;
    let same_scope =
        root.project_id == task.project_id && root.conversation_id == task.conversation_id;
    let valid_parent = match task.parent_task_id {
        Some(parent_id) => {
            let parent = required_task(connection, parent_id)?;
            parent.root_task_id == task.root_task_id
                && parent.project_id == task.project_id
                && parent.conversation_id == task.conversation_id
                && parent.depth + 1 == task.depth
        }
        None => task.id == root.id && task.depth == 0 && task.role == TaskRole::Lead,
    };
    if valid_root && same_scope && valid_parent {
        Ok(())
    } else {
        Err(StoreError::TaskAncestryMismatch { task_id: task.id })
    }
}

fn require_task_revision(task: &TaskRecord, expected_revision: u64) -> Result<(), StoreError> {
    if task.revision == expected_revision {
        Ok(())
    } else {
        Err(StoreError::TaskStale {
            task_id: task.id,
            current_revision: task.revision,
        })
    }
}

const fn authority_rank(authority: AuthorityMode) -> u8 {
    match authority {
        AuthorityMode::Ask => 0,
        AuthorityMode::SelfDecision => 1,
        AuthorityMode::FullAutonomy => 2,
    }
}

fn actor_columns(actor: MessageSender) -> (&'static str, Option<String>, Option<String>) {
    match actor {
        MessageSender::Employee(endpoint) => (
            "employee",
            Some(endpoint.employee_id.to_string()),
            endpoint.session_id.map(|value| value.to_string()),
        ),
        MessageSender::User(_) => ("user", None, None),
    }
}

pub(crate) fn required_task(
    connection: &Connection,
    task_id: Uuid,
) -> Result<TaskRecord, StoreError> {
    connection
        .query_row(
            "SELECT sequence, contract_version, id, root_task_id, parent_task_id,
                    depth, project_id, conversation_id, creation_turn_id,
                    delegation_message_id, creator_kind, creator_employee_id,
                    creator_session_id, assigned_employee_id, role, objective,
                    references_json, requested_authority, effective_authority,
                    employee_configuration_revision, roster_revision, state, revision,
                    active_turn_id, active_session_id, failure_code, failure_message,
                    created_at, updated_at, terminal_at
             FROM tasks WHERE id = ?1",
            [task_id.to_string()],
            raw_task,
        )
        .optional()?
        .map(task_from_raw)
        .transpose()?
        .ok_or(StoreError::TaskNotFound { task_id })
}

fn query_tasks(
    connection: &Connection,
    input: &ListTasksInput,
) -> Result<Vec<TaskSummary>, StoreError> {
    let state = input.state.map(serialized_name).transpose()?;
    let mut statement = connection.prepare(
        "SELECT sequence, id, root_task_id, parent_task_id, depth, project_id,
                conversation_id, assigned_employee_id, role, objective,
                requested_authority, effective_authority, state, revision,
                created_at, updated_at, terminal_at
         FROM tasks
         WHERE sequence > ?1
           AND (?3 IS NULL OR root_task_id = ?3)
           AND (?4 IS NULL OR parent_task_id = ?4)
           AND (?5 IS NULL OR conversation_id = ?5)
           AND (?6 IS NULL OR project_id = ?6)
           AND (?7 IS NULL OR assigned_employee_id = ?7)
           AND (?8 IS NULL OR state = ?8)
         ORDER BY sequence LIMIT ?2",
    )?;
    let rows = statement.query_map(
        params![
            to_i64(input.after_sequence)?,
            i64::from(input.limit),
            input.root_task_id.map(|value| value.to_string()),
            input.parent_task_id.map(|value| value.to_string()),
            input.conversation_id.map(|value| value.to_string()),
            input.project_id.map(|value| value.to_string()),
            input.assigned_employee_id.map(|value| value.to_string()),
            state,
        ],
        |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, Option<String>>(3)?,
                row.get::<_, i64>(4)?,
                row.get::<_, Option<String>>(5)?,
                row.get::<_, String>(6)?,
                row.get::<_, String>(7)?,
                row.get::<_, String>(8)?,
                row.get::<_, String>(9)?,
                row.get::<_, String>(10)?,
                row.get::<_, String>(11)?,
                row.get::<_, String>(12)?,
                row.get::<_, i64>(13)?,
                row.get::<_, String>(14)?,
                row.get::<_, String>(15)?,
                row.get::<_, Option<String>>(16)?,
            ))
        },
    )?;
    rows.map(|row| {
        let row = row?;
        Ok(TaskSummary {
            sequence: unsigned(row.0)?,
            id: parse_uuid(&row.1, "tasks.id")?,
            root_task_id: parse_uuid(&row.2, "tasks.root_task_id")?,
            parent_task_id: optional_uuid(row.3.as_deref(), "tasks.parent_task_id")?,
            depth: unsigned32(row.4)?,
            project_id: optional_uuid(row.5.as_deref(), "tasks.project_id")?,
            conversation_id: parse_uuid(&row.6, "tasks.conversation_id")?,
            assigned_employee_id: parse_uuid(&row.7, "tasks.assigned_employee_id")?,
            role: serde_json::from_value(Value::String(row.8))?,
            objective: row.9,
            requested_authority: serde_json::from_value(Value::String(row.10))?,
            effective_authority: serde_json::from_value(Value::String(row.11))?,
            state: serde_json::from_value(Value::String(row.12))?,
            revision: unsigned(row.13)?,
            created_at: row.14,
            updated_at: row.15,
            terminal_at: row.16,
        })
    })
    .collect()
}

type RawTask = (
    i64,
    i64,
    String,
    String,
    Option<String>,
    i64,
    Option<String>,
    String,
    Option<String>,
    Option<String>,
    String,
    Option<String>,
    Option<String>,
    String,
    String,
    String,
    String,
    String,
    String,
    i64,
    i64,
    String,
    i64,
    Option<String>,
    Option<String>,
    Option<String>,
    Option<String>,
    String,
    String,
    Option<String>,
);

fn raw_task(row: &rusqlite::Row<'_>) -> rusqlite::Result<RawTask> {
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
    ))
}

fn task_from_raw(raw: RawTask) -> Result<TaskRecord, StoreError> {
    Ok(TaskRecord {
        sequence: unsigned(raw.0)?,
        version: unsigned32(raw.1)?,
        id: parse_uuid(&raw.2, "tasks.id")?,
        root_task_id: parse_uuid(&raw.3, "tasks.root_task_id")?,
        parent_task_id: optional_uuid(raw.4.as_deref(), "tasks.parent_task_id")?,
        depth: unsigned32(raw.5)?,
        project_id: optional_uuid(raw.6.as_deref(), "tasks.project_id")?,
        conversation_id: parse_uuid(&raw.7, "tasks.conversation_id")?,
        creation_turn_id: optional_uuid(raw.8.as_deref(), "tasks.creation_turn_id")?,
        delegation_message_id: optional_uuid(raw.9.as_deref(), "tasks.delegation_message_id")?,
        creator: sender_from_columns(&raw.10, raw.11.as_deref(), raw.12.as_deref())?,
        assigned_employee_id: parse_uuid(&raw.13, "tasks.assigned_employee_id")?,
        role: serde_json::from_value(Value::String(raw.14))?,
        objective: raw.15,
        references: serde_json::from_str::<Vec<MessageReference>>(&raw.16)?,
        requested_authority: serde_json::from_value(Value::String(raw.17))?,
        effective_authority: serde_json::from_value(Value::String(raw.18))?,
        employee_configuration_revision: unsigned(raw.19)?,
        roster_revision: unsigned(raw.20)?,
        state: serde_json::from_value(Value::String(raw.21))?,
        revision: unsigned(raw.22)?,
        active_turn_id: optional_uuid(raw.23.as_deref(), "tasks.active_turn_id")?,
        active_session_id: optional_uuid(raw.24.as_deref(), "tasks.active_session_id")?,
        failure_code: raw.25,
        failure_message: raw.26,
        created_at: raw.27,
        updated_at: raw.28,
        terminal_at: raw.29,
    })
}

fn query_task_evidence(
    connection: &Connection,
    task_id: Uuid,
) -> Result<Vec<TaskEvidence>, StoreError> {
    let mut statement = connection.prepare(
        "SELECT sequence, id, task_id, task_revision, kind, actor_kind,
                actor_employee_id, actor_session_id, message_id, turn_id,
                session_id, detail_json, created_at
         FROM task_evidence WHERE task_id = ?1 ORDER BY sequence",
    )?;
    let rows = statement.query_map([task_id.to_string()], |row| {
        Ok((
            row.get::<_, i64>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, String>(2)?,
            row.get::<_, i64>(3)?,
            row.get::<_, String>(4)?,
            row.get::<_, String>(5)?,
            row.get::<_, Option<String>>(6)?,
            row.get::<_, Option<String>>(7)?,
            row.get::<_, Option<String>>(8)?,
            row.get::<_, Option<String>>(9)?,
            row.get::<_, Option<String>>(10)?,
            row.get::<_, String>(11)?,
            row.get::<_, String>(12)?,
        ))
    })?;
    rows.map(|row| evidence_from_raw(row?)).collect()
}

pub(crate) fn query_task_dispatch(
    connection: &Connection,
    task_id: Uuid,
) -> Result<Option<TaskDispatchRecord>, StoreError> {
    let raw = connection
        .query_row(
            "SELECT task_id, message_id, owner_boot_id, project_id,
                    project_configuration_revision, workspace_kind, workspace_path,
                    base_commit, branch_name, turn_id, session_id, state, revision,
                    failure_code, failure_message, created_at, updated_at
             FROM task_dispatches WHERE task_id = ?1",
            [task_id.to_string()],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, Option<String>>(3)?,
                    row.get::<_, Option<i64>>(4)?,
                    row.get::<_, Option<String>>(5)?,
                    row.get::<_, Option<String>>(6)?,
                    row.get::<_, Option<String>>(7)?,
                    row.get::<_, Option<String>>(8)?,
                    row.get::<_, Option<String>>(9)?,
                    row.get::<_, Option<String>>(10)?,
                    row.get::<_, String>(11)?,
                    row.get::<_, i64>(12)?,
                    row.get::<_, Option<String>>(13)?,
                    row.get::<_, Option<String>>(14)?,
                    row.get::<_, String>(15)?,
                    row.get::<_, String>(16)?,
                ))
            },
        )
        .optional()?;
    raw.map(|raw| {
        Ok(TaskDispatchRecord {
            task_id: parse_uuid(&raw.0, "task_dispatches.task_id")?,
            message_id: parse_uuid(&raw.1, "task_dispatches.message_id")?,
            owner_boot_id: parse_uuid(&raw.2, "task_dispatches.owner_boot_id")?,
            project_id: optional_uuid(raw.3.as_deref(), "task_dispatches.project_id")?,
            project_configuration_revision: raw.4.map(unsigned).transpose()?,
            workspace_kind: raw
                .5
                .map(|value| serde_json::from_value(Value::String(value)))
                .transpose()?,
            workspace_path: raw.6,
            base_commit: raw.7,
            branch_name: raw.8,
            turn_id: optional_uuid(raw.9.as_deref(), "task_dispatches.turn_id")?,
            session_id: optional_uuid(raw.10.as_deref(), "task_dispatches.session_id")?,
            state: serde_json::from_value(Value::String(raw.11))?,
            revision: unsigned(raw.12)?,
            failure_code: raw.13,
            failure_message: raw.14,
            created_at: raw.15,
            updated_at: raw.16,
        })
    })
    .transpose()
}

fn required_task_evidence(
    connection: &Connection,
    evidence_id: Uuid,
) -> Result<TaskEvidence, StoreError> {
    connection
        .query_row(
            "SELECT sequence, id, task_id, task_revision, kind, actor_kind,
                    actor_employee_id, actor_session_id, message_id, turn_id,
                    session_id, detail_json, created_at
             FROM task_evidence WHERE id = ?1",
            [evidence_id.to_string()],
            |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, i64>(3)?,
                    row.get::<_, String>(4)?,
                    row.get::<_, String>(5)?,
                    row.get::<_, Option<String>>(6)?,
                    row.get::<_, Option<String>>(7)?,
                    row.get::<_, Option<String>>(8)?,
                    row.get::<_, Option<String>>(9)?,
                    row.get::<_, Option<String>>(10)?,
                    row.get::<_, String>(11)?,
                    row.get::<_, String>(12)?,
                ))
            },
        )
        .optional()?
        .map(evidence_from_raw)
        .transpose()?
        .ok_or(StoreError::InvalidRecord {
            field: "task_evidence.id",
            message: "inserted task evidence was not found".to_owned(),
        })
}

#[allow(clippy::type_complexity)]
fn evidence_from_raw(
    raw: (
        i64,
        String,
        String,
        i64,
        String,
        String,
        Option<String>,
        Option<String>,
        Option<String>,
        Option<String>,
        Option<String>,
        String,
        String,
    ),
) -> Result<TaskEvidence, StoreError> {
    Ok(TaskEvidence {
        sequence: unsigned(raw.0)?,
        id: parse_uuid(&raw.1, "task_evidence.id")?,
        task_id: parse_uuid(&raw.2, "task_evidence.task_id")?,
        task_revision: unsigned(raw.3)?,
        kind: serde_json::from_value(Value::String(raw.4))?,
        actor: sender_from_columns(&raw.5, raw.6.as_deref(), raw.7.as_deref())?,
        message_id: optional_uuid(raw.8.as_deref(), "task_evidence.message_id")?,
        turn_id: optional_uuid(raw.9.as_deref(), "task_evidence.turn_id")?,
        session_id: optional_uuid(raw.10.as_deref(), "task_evidence.session_id")?,
        detail: serde_json::from_str(&raw.11)?,
        created_at: raw.12,
    })
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
                    field: "task actor employee_id",
                    message: "employee actor is missing employee id".to_owned(),
                })?,
                "task actor employee_id",
            )?,
            session_id: optional_uuid(session_id, "task actor session_id")?,
        })),
        "user" => Ok(MessageSender::User(UserMessageEndpoint {
            kind: UserMessageKind::User,
        })),
        _ => Err(StoreError::InvalidRecord {
            field: "task actor kind",
            message: format!("unsupported actor kind {kind}"),
        }),
    }
}

fn serialized_name<T: serde::Serialize>(value: T) -> Result<String, StoreError> {
    match serde_json::to_value(value)? {
        Value::String(value) => Ok(value),
        _ => Err(StoreError::InvalidRecord {
            field: "task enum",
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
