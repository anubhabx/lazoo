use chrono::Utc;
use lazoo_domain::{
    derive_availability, effective_authority, validate_message_command_key, validate_task_evidence,
    validate_task_failure, validate_task_objective, validate_task_references,
};
use lazoo_protocol::{
    AuthorityMode, EmployeeAvailability, MESSAGE_ENVELOPE_V1, MessageEndpoint, MessageKind,
    MessageRecord, MessageReference, MessageSender, TASK_CONTRACT_V1, TaskDetail, TaskEvidence,
    TaskEvidenceKind, TaskMutationResult, TaskRecord, TaskReportKind, TaskResolution, TaskRole,
    TaskState, TaskSummary, TurnState, UserMessageEndpoint, UserMessageKind,
};
use rusqlite::{Connection, OptionalExtension, Transaction, TransactionBehavior, params};
use serde_json::{Value, json};
use uuid::Uuid;

use crate::{Store, StoreError, employee, message, session};

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
        Ok(TaskDetail {
            task,
            children,
            evidence,
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
        let (target, evidence_kind) = match current.state {
            TaskState::Proposed | TaskState::Accepted => {
                require_no_active_descendants(&transaction, current.id)?;
                (TaskState::Cancelled, TaskEvidenceKind::Cancelled)
            }
            TaskState::Running => (
                TaskState::CancelRequested,
                TaskEvidenceKind::CancelRequested,
            ),
            _ => {
                return Err(StoreError::TaskState {
                    task_id: current.id,
                    state: serialized_name(current.state)?,
                    expected: "proposed, accepted, or running",
                });
            }
        };
        let now = Utc::now().to_rfc3339();
        let terminal = target == TaskState::Cancelled;
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
    if active == 0 {
        Ok(())
    } else {
        Err(StoreError::TaskActiveChildren { task_id })
    }
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

fn require_valid_ancestry(connection: &Connection, task: &TaskRecord) -> Result<(), StoreError> {
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
