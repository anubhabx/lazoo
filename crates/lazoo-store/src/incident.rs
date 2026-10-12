use std::collections::BTreeMap;

use chrono::{DateTime, TimeDelta, Utc};
use lazoo_domain::{
    DESCRIBED_INCIDENT_CODES, INCIDENT_REMEDIATION_WINDOW_SECONDS, IncidentFacts,
    IncidentThreshold, MAX_INCIDENT_REMEDIES, RemediationVerdict, default_incident_threshold,
    describe_incident, incident_fingerprint, incident_threshold, incident_window,
    remediation_verdict, validate_incident_code, validate_incident_detail,
    validate_incident_list_limit, validate_incident_text, validate_incident_threshold,
    validate_incident_title, validate_message_command_key,
};
use lazoo_protocol::{
    AuthorityMode, INCIDENT_CONTRACT_V1, IncidentEscalation, IncidentEscalationReason,
    IncidentMutationResult, IncidentPriority, IncidentRecord, IncidentRemedyKind,
    IncidentRemedyRecord, IncidentSource, IncidentState, IncidentThresholdMutationResult,
    IncidentThresholdRecord, IncidentThresholdSource, MessageRecord, MessageReference,
    MessageSender, TaskRecord, TaskState,
};
use rusqlite::{Connection, OptionalExtension, Transaction, TransactionBehavior, params};
use serde_json::{Value, json};
use uuid::Uuid;

use crate::{Store, StoreError, conversation, employee, message, task, task::DelegateTaskInput};

const INCIDENT_COLUMNS: &str = "sequence, contract_version, id, code, fingerprint, title,
    explanation, next_action, project_id, employee_id, provider, task_id, session_id, turn_id,
    message_id, source, detail_json, occurrences, window_occurrences, first_occurred_at,
    last_occurred_at, window_started_at, priority, state, revision, created_at, updated_at,
    acknowledged_at, resolved_at, escalation, remediation_deadline_at, escalated_at,
    escalation_reason, user_notified_at, user_notification";
const REMEDY_COLUMNS: &str = "id, incident_id, attempt, kind, lead_employee_id, lead_task_id,
    turn_id, target_task_id, result_task_id, occurrences_at_remedy, note, created_at";

/// One failure occurrence. Equivalent occurrences (same code, employee, and project) fold into
/// the open incident with that fingerprint; a resolved incident never reopens.
#[derive(Clone, Debug)]
pub struct RaiseIncidentInput {
    pub request_id: String,
    pub idempotency_key: String,
    pub code: String,
    pub source: IncidentSource,
    pub title: Option<String>,
    pub explanation: Option<String>,
    pub project_id: Option<Uuid>,
    pub employee_id: Option<Uuid>,
    pub provider: Option<String>,
    pub task_id: Option<Uuid>,
    pub session_id: Option<Uuid>,
    pub turn_id: Option<Uuid>,
    pub message_id: Option<Uuid>,
    pub detail: Value,
    pub occurred_at: Option<String>,
}

#[derive(Clone, Debug)]
pub struct SettleIncidentInput {
    pub request_id: String,
    pub idempotency_key: String,
    pub incident_id: Uuid,
    pub expected_revision: u64,
}

#[derive(Clone, Debug, Default)]
pub struct ListIncidentsInput {
    pub after_sequence: u64,
    pub limit: u32,
    pub state: Option<IncidentState>,
    pub priority: Option<IncidentPriority>,
    pub code: Option<String>,
    pub employee_id: Option<Uuid>,
    pub project_id: Option<Uuid>,
    pub escalation: Option<IncidentEscalation>,
}

/// What a lead asks for, from the fixed remedy set.
#[derive(Clone, Debug)]
pub enum IncidentRemedyAction {
    /// Cancel the affected child task (or request cancellation of its in-flight turn).
    CancelTask,
    /// Delegate the failed or cancelled child's objective again through the ordinary guards.
    RedelegateTask {
        /// Defaults to the affected task's assignee.
        assigned_employee_id: Option<Uuid>,
        roster_revision: u64,
        requested_authority: Option<AuthorityMode>,
    },
}

impl IncidentRemedyAction {
    #[must_use]
    pub const fn kind(&self) -> IncidentRemedyKind {
        match self {
            Self::CancelTask => IncidentRemedyKind::CancelTask,
            Self::RedelegateTask { .. } => IncidentRemedyKind::RedelegateTask,
        }
    }
}

/// One remedy applied by the lead owning `turn_id`, to the incident's task, which must be a
/// direct child of that turn's root task.
#[derive(Clone, Debug)]
pub struct ApplyIncidentRemedyInput {
    pub request_id: String,
    pub idempotency_key: String,
    pub incident_id: Uuid,
    pub turn_id: Uuid,
    pub action: IncidentRemedyAction,
    pub note: String,
}

#[derive(Clone, Debug)]
pub struct IncidentRemedyResult {
    pub incident: IncidentRecord,
    pub remedy: IncidentRemedyRecord,
    /// The cancelled task, or the new child for a redelegation.
    pub task: TaskRecord,
    pub event_sequence: Option<u64>,
}

/// A change to who is expected to act on an incident.
struct EscalationChange {
    escalation: IncidentEscalation,
    /// Set when a lead window opens; kept afterwards as history.
    deadline: Option<String>,
    escalated_at: Option<String>,
    reason: Option<IncidentEscalationReason>,
}

impl Store {
    /// Records one failure occurrence. The first occurrence opens an incident; later equivalent
    /// occurrences advance its counters, reopen an acknowledged row, and promote it to
    /// `priority` once the threshold is crossed inside one window.
    #[allow(clippy::too_many_lines)] // Fold-or-open is one atomic decision with one event.
    pub fn raise_incident(
        &self,
        input: &RaiseIncidentInput,
    ) -> Result<IncidentMutationResult, StoreError> {
        validate_message_command_key(&input.idempotency_key)?;
        validate_incident_code(&input.code)?;
        validate_incident_detail(&input.detail)?;
        let title = input
            .title
            .as_deref()
            .map(validate_incident_title)
            .transpose()?;
        let explanation = input
            .explanation
            .as_deref()
            .map(validate_incident_text)
            .transpose()?
            .filter(|text| !text.is_empty());
        let mut connection = self.lock()?;
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        if let Some(existing) =
            incident_for_command(&transaction, "incident.raise", &input.idempotency_key)?
        {
            if existing.code != input.code {
                return Err(StoreError::IncidentIdempotencyConflict);
            }
            transaction.commit()?;
            return Ok(IncidentMutationResult {
                incident: existing,
                event_sequence: None,
            });
        }
        let clock = Utc::now();
        let now = clock.to_rfc3339();
        let occurred_at = input.occurred_at.clone().unwrap_or_else(|| now.clone());
        let fingerprint = incident_fingerprint(&input.code, input.employee_id, input.project_id);
        let priority_message = priority_dead_letter(&transaction, &input.code, input.message_id)?;
        let configured = configured_threshold(&transaction, &input.code)?;
        let source = serialized_name(input.source)?;
        let detail_json = serde_json::to_string(&input.detail)?;
        let (incident_id, event_type) = if let Some(current) =
            query_open_by_fingerprint(&transaction, &fingerprint)?
        {
            let threshold = incident_threshold(
                &input.code,
                IncidentFacts {
                    names_task: input.task_id.or(current.task_id).is_some(),
                    priority_message,
                },
                configured,
            );
            let elapsed = elapsed_seconds(&current.window_started_at, &occurred_at);
            let (reset, window_occurrences) = incident_window(
                elapsed,
                current.window_occurrences,
                threshold.window_seconds,
            );
            let window_started_at = if reset {
                occurred_at.clone()
            } else {
                current.window_started_at.clone()
            };
            let escalate = current.priority == IncidentPriority::Normal
                && threshold.crossed(window_occurrences);
            let priority = if escalate || current.priority == IncidentPriority::Priority {
                "priority"
            } else {
                "normal"
            };
            transaction.execute(
                "UPDATE incidents SET occurrences = occurrences + 1,
                                window_occurrences = ?2, window_started_at = ?3,
                                last_occurred_at = ?4, priority = ?5, state = 'open',
                                acknowledged_at = NULL, revision = revision + 1,
                                updated_at = ?6, detail_json = ?7,
                                project_id = COALESCE(?8, project_id),
                                employee_id = COALESCE(?9, employee_id),
                                provider = COALESCE(?10, provider),
                                task_id = COALESCE(?11, task_id),
                                session_id = COALESCE(?12, session_id),
                                turn_id = COALESCE(?13, turn_id),
                                message_id = COALESCE(?14, message_id),
                                title = COALESCE(?15, title),
                                explanation = COALESCE(?16, explanation)
                         WHERE id = ?1",
                params![
                    current.id.to_string(),
                    to_i64(window_occurrences)?,
                    window_started_at,
                    occurred_at,
                    priority,
                    now,
                    detail_json,
                    input.project_id.map(|id| id.to_string()),
                    input.employee_id.map(|id| id.to_string()),
                    input.provider,
                    input.task_id.map(|id| id.to_string()),
                    input.session_id.map(|id| id.to_string()),
                    input.turn_id.map(|id| id.to_string()),
                    input.message_id.map(|id| id.to_string()),
                    title,
                    explanation,
                ],
            )?;
            let change = if escalate {
                Some(promotion_escalation(
                    &transaction,
                    input.task_id.or(current.task_id),
                    input.employee_id.or(current.employee_id),
                    clock,
                )?)
            } else {
                recurrence_escalation(&current, clock)
            };
            if let Some(change) = &change {
                write_escalation(&transaction, current.id, change)?;
            }
            let event_type = if escalate {
                "incident.escalated"
            } else if change.is_some() {
                "incident.escalated-to-user"
            } else {
                "incident.recurred"
            };
            (current.id, event_type)
        } else {
            let text = describe_incident(&input.code);
            let id = Uuid::new_v4();
            // A threshold of one (by default a stall, a crash, a death under assigned work, a
            // dead-lettered priority message) is crossed by the first occurrence.
            let born_priority = incident_threshold(
                &input.code,
                IncidentFacts {
                    names_task: input.task_id.is_some(),
                    priority_message,
                },
                configured,
            )
            .crossed(1);
            transaction.execute(
                "INSERT INTO incidents (
                            id, contract_version, code, fingerprint, title, explanation,
                            next_action, project_id, employee_id, provider, task_id, session_id,
                            turn_id, message_id, source, detail_json, occurrences,
                            window_occurrences, first_occurred_at, last_occurred_at,
                            window_started_at, priority, state, revision, created_at,
                            updated_at, acknowledged_at, resolved_at
                         ) VALUES (
                            ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15,
                            ?16, 1, 1, ?17, ?17, ?17, ?19, 'open', 1, ?18, ?18, NULL, NULL
                         )",
                params![
                    id.to_string(),
                    i64::from(INCIDENT_CONTRACT_V1),
                    input.code,
                    fingerprint,
                    title.unwrap_or(text.title),
                    explanation.unwrap_or(text.explanation),
                    text.next_action,
                    input.project_id.map(|id| id.to_string()),
                    input.employee_id.map(|id| id.to_string()),
                    input.provider,
                    input.task_id.map(|id| id.to_string()),
                    input.session_id.map(|id| id.to_string()),
                    input.turn_id.map(|id| id.to_string()),
                    input.message_id.map(|id| id.to_string()),
                    source,
                    detail_json,
                    occurred_at,
                    now,
                    if born_priority { "priority" } else { "normal" },
                ],
            )?;
            if born_priority {
                let change =
                    promotion_escalation(&transaction, input.task_id, input.employee_id, clock)?;
                write_escalation(&transaction, id, &change)?;
                (id, "incident.escalated")
            } else {
                (id, "incident.raised")
            }
        };
        insert_incident_command_tx(
            &transaction,
            "incident.raise",
            &input.idempotency_key,
            &input.request_id,
            incident_id,
            &now,
        )?;
        let incident = required_incident(&transaction, incident_id)?;
        let event_sequence = employee::insert_event(
            &transaction,
            event_type,
            &json!({"incident": &incident}),
            &now,
        )?;
        transaction.commit()?;
        Ok(IncidentMutationResult {
            incident,
            event_sequence: Some(event_sequence),
        })
    }

    pub fn acknowledge_incident(
        &self,
        input: &SettleIncidentInput,
    ) -> Result<IncidentMutationResult, StoreError> {
        self.settle_incident(input, "incident.acknowledge", IncidentState::Acknowledged)
    }

    pub fn resolve_incident(
        &self,
        input: &SettleIncidentInput,
    ) -> Result<IncidentMutationResult, StoreError> {
        self.settle_incident(input, "incident.resolve", IncidentState::Resolved)
    }

    fn settle_incident(
        &self,
        input: &SettleIncidentInput,
        scope: &str,
        target: IncidentState,
    ) -> Result<IncidentMutationResult, StoreError> {
        validate_message_command_key(&input.idempotency_key)?;
        let mut connection = self.lock()?;
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        if let Some(existing) = incident_for_command(&transaction, scope, &input.idempotency_key)? {
            if existing.id != input.incident_id {
                return Err(StoreError::IncidentIdempotencyConflict);
            }
            transaction.commit()?;
            return Ok(IncidentMutationResult {
                incident: existing,
                event_sequence: None,
            });
        }
        let current = required_incident(&transaction, input.incident_id)?;
        if current.revision != input.expected_revision {
            return Err(StoreError::IncidentStale {
                incident_id: current.id,
                current_revision: current.revision,
            });
        }
        let allowed = match target {
            IncidentState::Acknowledged => current.state == IncidentState::Open,
            IncidentState::Resolved => current.state != IncidentState::Resolved,
            IncidentState::Open => false,
        };
        if !allowed {
            return Err(StoreError::IncidentState {
                incident_id: current.id,
                state: serialized_name(current.state)?,
                expected: if target == IncidentState::Acknowledged {
                    "open"
                } else {
                    "open or acknowledged"
                },
            });
        }
        let now = Utc::now().to_rfc3339();
        let state = serialized_name(target)?;
        transaction.execute(
            "UPDATE incidents SET state = ?2, revision = revision + 1, updated_at = ?3,
                    acknowledged_at = CASE WHEN ?2 = 'acknowledged' THEN ?3 ELSE acknowledged_at END,
                    resolved_at = CASE WHEN ?2 = 'resolved' THEN ?3 ELSE resolved_at END
             WHERE id = ?1",
            params![current.id.to_string(), state, now],
        )?;
        insert_incident_command_tx(
            &transaction,
            scope,
            &input.idempotency_key,
            &input.request_id,
            current.id,
            &now,
        )?;
        let incident = required_incident(&transaction, current.id)?;
        let event_type = if target == IncidentState::Acknowledged {
            "incident.acknowledged"
        } else {
            "incident.resolved"
        };
        let event_sequence = employee::insert_event(
            &transaction,
            event_type,
            &json!({"incident": &incident}),
            &now,
        )?;
        transaction.commit()?;
        Ok(IncidentMutationResult {
            incident,
            event_sequence: Some(event_sequence),
        })
    }

    pub fn get_incident(&self, incident_id: Uuid) -> Result<IncidentRecord, StoreError> {
        let connection = self.lock()?;
        required_incident(&connection, incident_id)
    }

    pub fn list_incidents(
        &self,
        input: &ListIncidentsInput,
    ) -> Result<Vec<IncidentRecord>, StoreError> {
        validate_incident_list_limit(input.limit)?;
        let connection = self.lock()?;
        let state = input.state.map(serialized_name).transpose()?;
        let priority = input.priority.map(serialized_name).transpose()?;
        let escalation = input.escalation.map(serialized_name).transpose()?;
        let mut statement = connection.prepare(&format!(
            "SELECT {INCIDENT_COLUMNS} FROM incidents
             WHERE sequence > ?1
               AND (?3 IS NULL OR state = ?3)
               AND (?4 IS NULL OR priority = ?4)
               AND (?5 IS NULL OR code = ?5)
               AND (?6 IS NULL OR employee_id = ?6)
               AND (?7 IS NULL OR project_id = ?7)
               AND (?8 IS NULL OR escalation = ?8)
             ORDER BY sequence LIMIT ?2"
        ))?;
        let rows = statement
            .query_map(
                params![
                    to_i64(input.after_sequence)?,
                    i64::from(input.limit),
                    state,
                    priority,
                    input.code,
                    input.employee_id.map(|value| value.to_string()),
                    input.project_id.map(|value| value.to_string()),
                    escalation,
                ],
                raw_incident,
            )?
            .collect::<Result<Vec<_>, _>>()?;
        rows.into_iter()
            .map(|raw| read_incident(&connection, raw))
            .collect()
    }

    /// Closes every lead remediation window that has passed by `now`. An incident whose lead's
    /// remedy held (no recurrence since it) becomes `remediated`; anything else unresolved is
    /// escalated to the user. Nothing here retries or changes the affected work.
    pub fn escalate_due_incidents(
        &self,
        now: DateTime<Utc>,
    ) -> Result<Vec<IncidentRecord>, StoreError> {
        let mut connection = self.lock()?;
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let raws = {
            let mut statement = transaction.prepare(&format!(
                "SELECT {INCIDENT_COLUMNS} FROM incidents
                 WHERE escalation = 'lead' AND state != 'resolved' ORDER BY sequence"
            ))?;
            statement
                .query_map([], raw_incident)?
                .collect::<Result<Vec<_>, _>>()?
        };
        let stamp = now.to_rfc3339();
        let mut changed = Vec::new();
        for raw in raws {
            let incident = read_incident(&transaction, raw)?;
            if !deadline_passed(incident.remediation_deadline_at.as_deref(), now) {
                continue;
            }
            let recurred = incident
                .remedies
                .last()
                .is_some_and(|last| incident.occurrences > last.occurrences_at_remedy);
            let (change, event_type) =
                match remediation_verdict(true, remedy_count(&incident), recurred) {
                    RemediationVerdict::Remediated => (
                        EscalationChange {
                            escalation: IncidentEscalation::Remediated,
                            deadline: None,
                            escalated_at: None,
                            reason: None,
                        },
                        "incident.remediated",
                    ),
                    RemediationVerdict::EscalateToUser(reason) => {
                        (to_user(now, reason), "incident.escalated-to-user")
                    }
                    RemediationVerdict::Open => continue,
                };
            write_escalation(&transaction, incident.id, &change)?;
            transaction.execute(
                "UPDATE incidents SET revision = revision + 1, updated_at = ?2 WHERE id = ?1",
                params![incident.id.to_string(), stamp],
            )?;
            let record = required_incident(&transaction, incident.id)?;
            employee::insert_event(
                &transaction,
                event_type,
                &json!({"incident": &record}),
                &stamp,
            )?;
            changed.push(record);
        }
        transaction.commit()?;
        Ok(changed)
    }

    /// Claims every open, user-escalated incident not yet announced, oldest first, in one
    /// transaction, so each is announced at most once. Keeper records are settled as `keeper`
    /// here (the keeper already raised its own notification); the rest are marked `claimed`
    /// and returned for the daemon to announce and then settle.
    pub fn claim_user_notifications(
        &self,
        now: DateTime<Utc>,
        limit: u32,
    ) -> Result<Vec<IncidentRecord>, StoreError> {
        let mut connection = self.lock()?;
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let ids = {
            let mut statement = transaction.prepare(
                "SELECT id, source FROM incidents
                 WHERE escalation = 'user' AND state = 'open' AND user_notified_at IS NULL
                 ORDER BY sequence LIMIT ?1",
            )?;
            statement
                .query_map([i64::from(limit)], |row| {
                    Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
                })?
                .collect::<Result<Vec<_>, _>>()?
        };
        let stamp = now.to_rfc3339();
        let mut claimed = Vec::new();
        for (id, source) in ids {
            let keeper = source == "keeper";
            transaction.execute(
                "UPDATE incidents SET user_notified_at = ?2, user_notification = ?3,
                        revision = revision + 1, updated_at = ?2
                 WHERE id = ?1",
                params![id, stamp, if keeper { "keeper" } else { "claimed" }],
            )?;
            let record =
                required_incident(&transaction, employee::parse_uuid(&id, "incidents.id")?)?;
            if keeper {
                employee::insert_event(
                    &transaction,
                    "incident.user-notified",
                    &json!({"incident": &record}),
                    &stamp,
                )?;
            } else {
                claimed.push(record);
            }
        }
        transaction.commit()?;
        Ok(claimed)
    }

    /// Records whether the notification for claimed incidents was shown. Only `claimed` rows
    /// change, so a settled outcome is never rewritten.
    pub fn settle_user_notifications(
        &self,
        incident_ids: &[Uuid],
        shown: bool,
    ) -> Result<Vec<IncidentRecord>, StoreError> {
        let mut connection = self.lock()?;
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let now = Utc::now().to_rfc3339();
        let mut settled = Vec::new();
        for incident_id in incident_ids {
            let changed = transaction.execute(
                "UPDATE incidents SET user_notification = ?2, revision = revision + 1,
                        updated_at = ?3
                 WHERE id = ?1 AND user_notification = 'claimed'",
                params![
                    incident_id.to_string(),
                    if shown { "shown" } else { "failed" },
                    now
                ],
            )?;
            if changed == 0 {
                continue;
            }
            let record = required_incident(&transaction, *incident_id)?;
            employee::insert_event(
                &transaction,
                "incident.user-notified",
                &json!({"incident": &record}),
                &now,
            )?;
            settled.push(record);
        }
        transaction.commit()?;
        Ok(settled)
    }

    /// Applies one remedy for the lead whose turn is `turn_id`. The remedy row, the action it
    /// names (through the ordinary cancellation or delegation guards), and the incident revision
    /// commit together, so a refused action consumes no attempt. Replays by idempotency key
    /// return the recorded remedy without writing.
    #[allow(clippy::too_many_lines)] // Window, scope, action, and record stay one decision.
    pub fn apply_incident_remedy(
        &self,
        input: &ApplyIncidentRemedyInput,
    ) -> Result<IncidentRemedyResult, StoreError> {
        validate_message_command_key(&input.idempotency_key)?;
        let note = validate_incident_text(&input.note)?;
        let kind = input.action.kind();
        let mut connection = self.lock()?;
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        if let Some(remedy) = remedy_for_key(&transaction, &input.idempotency_key)? {
            if remedy.incident_id != input.incident_id || remedy.kind != kind {
                return Err(StoreError::IncidentIdempotencyConflict);
            }
            let incident = required_incident(&transaction, remedy.incident_id)?;
            let task = task::required_task(
                &transaction,
                remedy.result_task_id.unwrap_or(remedy.target_task_id),
            )?;
            transaction.commit()?;
            return Ok(IncidentRemedyResult {
                incident,
                remedy,
                task,
                event_sequence: None,
            });
        }
        let clock = Utc::now();
        let now = clock.to_rfc3339();
        let incident = required_incident(&transaction, input.incident_id)?;
        let closed = |reason| StoreError::IncidentRemedyClosed {
            incident_id: incident.id,
            reason,
        };
        if incident.state == IncidentState::Resolved {
            return Err(closed("the incident is resolved"));
        }
        if incident.escalation != IncidentEscalation::Lead {
            return Err(closed("the incident is not in a lead remediation window"));
        }
        if deadline_passed(incident.remediation_deadline_at.as_deref(), clock) {
            return Err(closed("the remediation window has closed"));
        }
        let attempt = remedy_count(&incident) + 1;
        if attempt > MAX_INCIDENT_REMEDIES {
            return Err(closed(
                "every remedy allowed for this incident was already applied",
            ));
        }
        let turn = conversation::required_turn(&transaction, input.turn_id)?;
        let lead_task = task::required_task(
            &transaction,
            turn.task_id.ok_or(StoreError::TurnTaskRequired)?,
        )?;
        // Only the lead of the incident's tree, on its root task, about a direct child of it.
        let target = incident
            .task_id
            .map(|task_id| task::required_task(&transaction, task_id))
            .transpose()?
            .filter(|target| {
                target.parent_task_id == Some(lead_task.id)
                    && target.root_task_id == lead_task.id
                    && turn.recipient.employee_id == lead_task.assigned_employee_id
            })
            .ok_or(StoreError::IncidentRemedyScope {
                incident_id: incident.id,
            })?;
        if !matches!(lead_task.state, TaskState::Accepted | TaskState::Running) {
            return Err(StoreError::TaskState {
                task_id: lead_task.id,
                state: serialized_name(lead_task.state)?,
                expected: "accepted or running",
            });
        }
        let affected = match &input.action {
            IncidentRemedyAction::CancelTask => {
                task::cancel_task_tx(
                    &transaction,
                    &target,
                    MessageSender::Employee(turn.recipient),
                    &json!({"source": "incident.remedy", "incidentId": incident.id, "attempt": attempt}),
                    &input.idempotency_key,
                    &input.request_id,
                )?
                .task
            }
            IncidentRemedyAction::RedelegateTask {
                assigned_employee_id,
                roster_revision,
                requested_authority,
            } => {
                if !matches!(target.state, TaskState::Failed | TaskState::Cancelled) {
                    return Err(StoreError::IncidentRemedyTarget {
                        task_id: target.id,
                        state: serialized_name(target.state)?,
                    });
                }
                task::delegate_task_tx(
                    &transaction,
                    &DelegateTaskInput {
                        request_id: input.request_id.clone(),
                        idempotency_key: input.idempotency_key.clone(),
                        parent_task_id: lead_task.id,
                        actor: turn.recipient,
                        assigned_employee_id: assigned_employee_id
                            .unwrap_or(target.assigned_employee_id),
                        objective: target.objective.clone(),
                        references: vec![
                            MessageReference {
                                kind: "task".to_owned(),
                                target: target.id.to_string(),
                                metadata: json!({"relation": "redelegated-from"}),
                            },
                            MessageReference {
                                kind: "incident".to_owned(),
                                target: incident.id.to_string(),
                                metadata: json!({}),
                            },
                        ],
                        requested_authority: *requested_authority,
                        roster_revision: *roster_revision,
                    },
                )?
                .task
            }
        };
        transaction.execute(
            &format!(
                "INSERT INTO incident_remedies ({REMEDY_COLUMNS}, idempotency_key, request_id)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14)"
            ),
            params![
                Uuid::new_v4().to_string(),
                incident.id.to_string(),
                i64::from(attempt),
                serialized_name(kind)?,
                turn.recipient.employee_id.to_string(),
                lead_task.id.to_string(),
                turn.id.to_string(),
                target.id.to_string(),
                (kind == IncidentRemedyKind::RedelegateTask).then(|| affected.id.to_string()),
                to_i64(incident.occurrences)?,
                note,
                now,
                input.idempotency_key,
                input.request_id,
            ],
        )?;
        transaction.execute(
            "UPDATE incidents SET revision = revision + 1, updated_at = ?2 WHERE id = ?1",
            params![incident.id.to_string(), now],
        )?;
        let incident = required_incident(&transaction, incident.id)?;
        let remedy = remedy_for_key(&transaction, &input.idempotency_key)?.ok_or(
            StoreError::InvalidRecord {
                field: "incident_remedies",
                message: "the remedy just recorded is missing".to_owned(),
            },
        )?;
        let event_sequence = employee::insert_event(
            &transaction,
            "incident.remedy-applied",
            &json!({"incident": &incident, "remedy": &remedy, "task": &affected}),
            &now,
        )?;
        transaction.commit()?;
        Ok(IncidentRemedyResult {
            incident,
            remedy,
            task: affected,
            event_sequence: Some(event_sequence),
        })
    }

    /// Folds every emergency spool record the keeper wrote into the incident ledger. Raising is
    /// idempotent per spool record, so repeated startups never duplicate an incident.
    pub fn reconcile_emergency_incidents(&self) -> Result<Vec<IncidentRecord>, StoreError> {
        let imports: Vec<(String, String, String, String)> = {
            let connection = self.lock()?;
            let mut statement = connection.prepare(
                "SELECT incident_id, code, message, occurred_at FROM emergency_incident_imports
                 ORDER BY imported_at, incident_id",
            )?;
            let rows = statement.query_map([], |row| {
                Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?))
            })?;
            rows.collect::<Result<Vec<_>, _>>()?
        };
        let mut raised = Vec::new();
        for (incident_id, code, message, occurred_at) in imports {
            let code = if validate_incident_code(&code).is_ok() {
                code
            } else {
                "LZ-DAEMON-UNAVAILABLE".to_owned()
            };
            let mut explanation = message.clone();
            truncate_utf8(&mut explanation, lazoo_domain::MAX_INCIDENT_TEXT_BYTES);
            let mut detail_message = message;
            truncate_utf8(&mut detail_message, 8 * 1024);
            let result = self.raise_incident(&RaiseIncidentInput {
                request_id: format!("emergency:{incident_id}"),
                idempotency_key: format!("emergency:{incident_id}"),
                code,
                source: IncidentSource::Keeper,
                title: None,
                explanation: Some(explanation),
                project_id: None,
                employee_id: None,
                provider: None,
                task_id: None,
                session_id: None,
                turn_id: None,
                message_id: None,
                detail: json!({
                    "emergencyIncidentId": incident_id,
                    "message": detail_message,
                }),
                occurred_at: Some(occurred_at),
            })?;
            if result.event_sequence.is_some() {
                raised.push(result.incident);
            }
        }
        Ok(raised)
    }
}

/// The idempotency key of the `LZ-MESSAGE-DEADLETTER` incident for one dead letter. Every path
/// that raises it uses this key, so each dead letter is reported once.
#[must_use]
pub fn dead_letter_incident_key(message_id: Uuid, attempt_id: Uuid) -> String {
    format!("dead-letter:{message_id}:{attempt_id}")
}

impl Store {
    /// Dead-lettered priority messages whose `LZ-MESSAGE-DEADLETTER` incident has not been raised
    /// yet, oldest first. Whatever exhausted the message committed its dead letter first; the
    /// daemon raises the incident from this list, so a crash in between is caught up later.
    pub fn unnoted_priority_dead_letters(
        &self,
        limit: u32,
    ) -> Result<Vec<MessageRecord>, StoreError> {
        let connection = self.lock()?;
        let mut statement = connection.prepare(
            "SELECT m.id FROM message_dead_letters d
             JOIN messages m ON m.id = d.message_id
             WHERE m.priority = 'priority'
               AND NOT EXISTS (
                   SELECT 1 FROM incident_commands c
                   WHERE c.scope = 'incident.raise'
                     AND c.idempotency_key = 'dead-letter:' || d.message_id || ':' || d.attempt_id
               )
             ORDER BY m.sequence
             LIMIT ?1",
        )?;
        let ids = statement
            .query_map([i64::from(limit)], |row| row.get::<_, String>(0))?
            .collect::<Result<Vec<_>, _>>()?;
        ids.iter()
            .map(|id| {
                message::required_message(&connection, employee::parse_uuid(id, "messages.id")?)
            })
            .collect()
    }
}

/// The local user's override of one code's threshold, set through `incident.threshold.set`.
#[derive(Clone, Debug)]
pub struct SetIncidentThresholdInput {
    pub code: String,
    pub occurrences: u64,
    pub window_seconds: u64,
}

impl Store {
    /// Every described code and every configured one, sorted by code, with its effective
    /// threshold for an ordinary occurrence and whether the user configured it.
    pub fn list_incident_thresholds(&self) -> Result<Vec<IncidentThresholdRecord>, StoreError> {
        let connection = self.lock()?;
        let mut configured = BTreeMap::new();
        let mut statement = connection.prepare(
            "SELECT code, occurrences, window_seconds, updated_at FROM incident_thresholds",
        )?;
        for row in statement.query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, i64>(1)?,
                row.get::<_, i64>(2)?,
                row.get::<_, String>(3)?,
            ))
        })? {
            let (code, occurrences, window_seconds, updated_at) = row?;
            configured.insert(
                code.clone(),
                threshold_record(code, occurrences, window_seconds, Some(updated_at))?,
            );
        }
        for code in DESCRIBED_INCIDENT_CODES {
            configured
                .entry((*code).to_owned())
                .or_insert_with(|| default_threshold_record(code));
        }
        Ok(configured.into_values().collect())
    }

    /// Overrides one code's threshold. Writing the value it already has changes nothing and
    /// emits no event.
    pub fn set_incident_threshold(
        &self,
        input: &SetIncidentThresholdInput,
    ) -> Result<IncidentThresholdMutationResult, StoreError> {
        validate_incident_code(&input.code)?;
        let threshold = validate_incident_threshold(input.occurrences, input.window_seconds)?;
        let mut connection = self.lock()?;
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        if configured_threshold(&transaction, &input.code)? == Some(threshold) {
            let record = current_threshold_record(&transaction, &input.code)?;
            transaction.commit()?;
            return Ok(IncidentThresholdMutationResult {
                threshold: record,
                event_sequence: None,
            });
        }
        let now = Utc::now().to_rfc3339();
        transaction.execute(
            "INSERT INTO incident_thresholds (code, occurrences, window_seconds, updated_at)
             VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT(code) DO UPDATE SET
                occurrences = excluded.occurrences,
                window_seconds = excluded.window_seconds,
                updated_at = excluded.updated_at",
            params![
                input.code,
                to_i64(threshold.occurrences)?,
                threshold.window_seconds,
                now
            ],
        )?;
        threshold_change(transaction, &input.code, "incident.threshold-set", &now)
    }

    /// Returns one code to section 14.2's default. Resetting a default changes nothing.
    pub fn reset_incident_threshold(
        &self,
        code: &str,
    ) -> Result<IncidentThresholdMutationResult, StoreError> {
        validate_incident_code(code)?;
        let mut connection = self.lock()?;
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let removed =
            transaction.execute("DELETE FROM incident_thresholds WHERE code = ?1", [code])?;
        if removed == 0 {
            transaction.commit()?;
            return Ok(IncidentThresholdMutationResult {
                threshold: default_threshold_record(code),
                event_sequence: None,
            });
        }
        let now = Utc::now().to_rfc3339();
        threshold_change(transaction, code, "incident.threshold-reset", &now)
    }
}

fn threshold_change(
    transaction: Transaction<'_>,
    code: &str,
    event_type: &str,
    now: &str,
) -> Result<IncidentThresholdMutationResult, StoreError> {
    let record = current_threshold_record(&transaction, code)?;
    let event_sequence = employee::insert_event(
        &transaction,
        event_type,
        &json!({"threshold": &record}),
        now,
    )?;
    transaction.commit()?;
    Ok(IncidentThresholdMutationResult {
        threshold: record,
        event_sequence: Some(event_sequence),
    })
}

fn configured_threshold(
    connection: &Connection,
    code: &str,
) -> Result<Option<IncidentThreshold>, StoreError> {
    connection
        .query_row(
            "SELECT occurrences, window_seconds FROM incident_thresholds WHERE code = ?1",
            [code],
            |row| Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?)),
        )
        .optional()?
        .map(|(occurrences, window_seconds)| {
            Ok(IncidentThreshold {
                occurrences: unsigned(occurrences)?,
                window_seconds,
            })
        })
        .transpose()
}

fn current_threshold_record(
    connection: &Connection,
    code: &str,
) -> Result<IncidentThresholdRecord, StoreError> {
    connection
        .query_row(
            "SELECT occurrences, window_seconds, updated_at FROM incident_thresholds
             WHERE code = ?1",
            [code],
            |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, i64>(1)?,
                    row.get::<_, String>(2)?,
                ))
            },
        )
        .optional()?
        .map_or_else(
            || Ok(default_threshold_record(code)),
            |(occurrences, window_seconds, updated_at)| {
                threshold_record(
                    code.to_owned(),
                    occurrences,
                    window_seconds,
                    Some(updated_at),
                )
            },
        )
}

fn threshold_record(
    code: String,
    occurrences: i64,
    window_seconds: i64,
    updated_at: Option<String>,
) -> Result<IncidentThresholdRecord, StoreError> {
    Ok(IncidentThresholdRecord {
        code,
        occurrences: unsigned(occurrences)?,
        window_seconds: unsigned(window_seconds)?,
        source: IncidentThresholdSource::Configured,
        updated_at,
    })
}

fn default_threshold_record(code: &str) -> IncidentThresholdRecord {
    let threshold = default_incident_threshold(code);
    IncidentThresholdRecord {
        code: code.to_owned(),
        occurrences: threshold.occurrences,
        window_seconds: threshold.window_seconds.unsigned_abs(),
        source: IncidentThresholdSource::Default,
        updated_at: None,
    }
}

/// Whether this failure dead-lettered a priority message. It is read from the message and its
/// dead letter, never taken from the caller.
fn priority_dead_letter(
    transaction: &Transaction<'_>,
    code: &str,
    message_id: Option<Uuid>,
) -> Result<bool, StoreError> {
    let Some(message_id) = message_id.filter(|_| code == "LZ-MESSAGE-DEADLETTER") else {
        return Ok(false);
    };
    let priority: Option<String> = transaction
        .query_row(
            "SELECT m.priority FROM messages m
             JOIN message_dead_letters d ON d.message_id = m.id
             WHERE m.id = ?1",
            [message_id.to_string()],
            |row| row.get(0),
        )
        .optional()?;
    Ok(priority.as_deref() == Some("priority"))
}

fn elapsed_seconds(window_started_at: &str, occurred_at: &str) -> i64 {
    match (
        DateTime::parse_from_rfc3339(window_started_at),
        DateTime::parse_from_rfc3339(occurred_at),
    ) {
        (Ok(started), Ok(occurred)) => (occurred - started).num_seconds(),
        _ => i64::MAX,
    }
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

fn query_open_by_fingerprint(
    connection: &Connection,
    fingerprint: &str,
) -> Result<Option<IncidentRecord>, StoreError> {
    connection
        .query_row(
            &format!(
                "SELECT {INCIDENT_COLUMNS} FROM incidents
                 WHERE fingerprint = ?1 AND state IN ('open', 'acknowledged')
                 ORDER BY sequence DESC LIMIT 1"
            ),
            [fingerprint],
            raw_incident,
        )
        .optional()?
        .map(|raw| read_incident(connection, raw))
        .transpose()
}

/// The root task whose lead is offered a promoted incident: one about a task below an active
/// root, and not about the root's own employee. Anything else has no lead to remediate it.
pub(crate) fn incident_lead_root(
    connection: &Connection,
    task_id: Option<Uuid>,
    employee_id: Option<Uuid>,
) -> Result<Option<TaskRecord>, StoreError> {
    let Some(task_id) = task_id else {
        return Ok(None);
    };
    let task = task::required_task(connection, task_id)?;
    let root = task::required_task(connection, task.root_task_id)?;
    Ok((task.id != root.id
        && matches!(root.state, TaskState::Accepted | TaskState::Running)
        && employee_id != Some(root.assigned_employee_id))
    .then_some(root))
}

/// A promotion opens the lead's remediation window, or goes to the user when there is no lead.
fn promotion_escalation(
    connection: &Connection,
    task_id: Option<Uuid>,
    employee_id: Option<Uuid>,
    clock: DateTime<Utc>,
) -> Result<EscalationChange, StoreError> {
    Ok(
        if incident_lead_root(connection, task_id, employee_id)?.is_some() {
            EscalationChange {
                escalation: IncidentEscalation::Lead,
                deadline: Some(
                    (clock + TimeDelta::seconds(INCIDENT_REMEDIATION_WINDOW_SECONDS)).to_rfc3339(),
                ),
                escalated_at: None,
                reason: None,
            }
        } else {
            to_user(clock, IncidentEscalationReason::NoLead)
        },
    )
}

/// A new occurrence of a promoted incident, judged against the lead's window. The occurrence
/// follows every remedy already recorded, so it counts as a recurrence after the last one.
fn recurrence_escalation(
    current: &IncidentRecord,
    clock: DateTime<Utc>,
) -> Option<EscalationChange> {
    let window_closed = match current.escalation {
        IncidentEscalation::Lead => {
            deadline_passed(current.remediation_deadline_at.as_deref(), clock)
        }
        IncidentEscalation::Remediated => true,
        IncidentEscalation::None | IncidentEscalation::User => return None,
    };
    let remedies = remedy_count(current);
    match remediation_verdict(window_closed, remedies, remedies > 0) {
        RemediationVerdict::EscalateToUser(reason) => Some(to_user(clock, reason)),
        RemediationVerdict::Open | RemediationVerdict::Remediated => None,
    }
}

fn to_user(clock: DateTime<Utc>, reason: IncidentEscalationReason) -> EscalationChange {
    EscalationChange {
        escalation: IncidentEscalation::User,
        deadline: None,
        escalated_at: Some(clock.to_rfc3339()),
        reason: Some(reason),
    }
}

fn write_escalation(
    transaction: &Transaction<'_>,
    incident_id: Uuid,
    change: &EscalationChange,
) -> Result<(), StoreError> {
    transaction.execute(
        "UPDATE incidents SET escalation = ?2,
                remediation_deadline_at = COALESCE(?3, remediation_deadline_at),
                escalated_at = ?4, escalation_reason = ?5
         WHERE id = ?1",
        params![
            incident_id.to_string(),
            serialized_name(change.escalation)?,
            change.deadline,
            change.escalated_at,
            change.reason.map(serialized_name).transpose()?,
        ],
    )?;
    Ok(())
}

/// A missing or unreadable deadline counts as passed, so the incident reaches the user.
fn deadline_passed(deadline: Option<&str>, clock: DateTime<Utc>) -> bool {
    deadline
        .and_then(|deadline| DateTime::parse_from_rfc3339(deadline).ok())
        .is_none_or(|deadline| clock >= deadline)
}

fn remedy_count(incident: &IncidentRecord) -> u32 {
    u32::try_from(incident.remedies.len()).unwrap_or(u32::MAX)
}

fn remedies_for(
    connection: &Connection,
    incident_id: Uuid,
) -> Result<Vec<IncidentRemedyRecord>, StoreError> {
    let mut statement = connection.prepare(&format!(
        "SELECT {REMEDY_COLUMNS} FROM incident_remedies WHERE incident_id = ?1 ORDER BY attempt"
    ))?;
    let rows = statement
        .query_map([incident_id.to_string()], raw_remedy)?
        .collect::<Result<Vec<_>, _>>()?;
    rows.into_iter().map(remedy_from_raw).collect()
}

fn remedy_for_key(
    connection: &Connection,
    idempotency_key: &str,
) -> Result<Option<IncidentRemedyRecord>, StoreError> {
    connection
        .query_row(
            &format!("SELECT {REMEDY_COLUMNS} FROM incident_remedies WHERE idempotency_key = ?1"),
            [idempotency_key],
            raw_remedy,
        )
        .optional()?
        .map(remedy_from_raw)
        .transpose()
}

type RawRemedy = (
    String,
    String,
    i64,
    String,
    String,
    String,
    String,
    String,
    Option<String>,
    i64,
    String,
    String,
);

fn raw_remedy(row: &rusqlite::Row<'_>) -> rusqlite::Result<RawRemedy> {
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
    ))
}

fn remedy_from_raw(raw: RawRemedy) -> Result<IncidentRemedyRecord, StoreError> {
    let (
        id,
        incident_id,
        attempt,
        kind,
        lead_employee_id,
        lead_task_id,
        turn_id,
        target_task_id,
        result_task_id,
        occurrences_at_remedy,
        note,
        created_at,
    ) = raw;
    Ok(IncidentRemedyRecord {
        id: employee::parse_uuid(&id, "incident_remedies.id")?,
        incident_id: employee::parse_uuid(&incident_id, "incident_remedies.incident_id")?,
        attempt: u32::try_from(attempt).map_err(|_| StoreError::NegativeSequence)?,
        kind: serde_json::from_value(Value::String(kind))?,
        lead_employee_id: employee::parse_uuid(
            &lead_employee_id,
            "incident_remedies.lead_employee_id",
        )?,
        lead_task_id: employee::parse_uuid(&lead_task_id, "incident_remedies.lead_task_id")?,
        turn_id: employee::parse_uuid(&turn_id, "incident_remedies.turn_id")?,
        target_task_id: employee::parse_uuid(&target_task_id, "incident_remedies.target_task_id")?,
        result_task_id: optional_uuid(
            result_task_id.as_deref(),
            "incident_remedies.result_task_id",
        )?,
        occurrences_at_remedy: unsigned(occurrences_at_remedy)?,
        note,
        created_at,
    })
}

fn incident_for_command(
    connection: &Connection,
    scope: &str,
    idempotency_key: &str,
) -> Result<Option<IncidentRecord>, StoreError> {
    connection
        .query_row(
            "SELECT incident_id FROM incident_commands WHERE scope = ?1 AND idempotency_key = ?2",
            params![scope, idempotency_key],
            |row| row.get::<_, String>(0),
        )
        .optional()?
        .map(|id| {
            required_incident(
                connection,
                employee::parse_uuid(&id, "incident_commands.incident_id")?,
            )
        })
        .transpose()
}

fn insert_incident_command_tx(
    transaction: &Transaction<'_>,
    scope: &str,
    idempotency_key: &str,
    request_id: &str,
    incident_id: Uuid,
    now: &str,
) -> Result<(), StoreError> {
    transaction.execute(
        "INSERT INTO incident_commands (scope, idempotency_key, request_id, incident_id, created_at)
         VALUES (?1, ?2, ?3, ?4, ?5)",
        params![scope, idempotency_key, request_id, incident_id.to_string(), now],
    )?;
    Ok(())
}

pub(crate) fn required_incident(
    connection: &Connection,
    incident_id: Uuid,
) -> Result<IncidentRecord, StoreError> {
    let raw = connection
        .query_row(
            &format!("SELECT {INCIDENT_COLUMNS} FROM incidents WHERE id = ?1"),
            [incident_id.to_string()],
            raw_incident,
        )
        .optional()?
        .ok_or(StoreError::IncidentNotFound { incident_id })?;
    read_incident(connection, raw)
}

fn read_incident(connection: &Connection, raw: RawIncident) -> Result<IncidentRecord, StoreError> {
    let mut incident = incident_from_raw(raw)?;
    incident.remedies = remedies_for(connection, incident.id)?;
    Ok(incident)
}

struct RawIncident {
    sequence: i64,
    contract_version: i64,
    id: String,
    code: String,
    fingerprint: String,
    title: String,
    explanation: String,
    next_action: String,
    project_id: Option<String>,
    employee_id: Option<String>,
    provider: Option<String>,
    task_id: Option<String>,
    session_id: Option<String>,
    turn_id: Option<String>,
    message_id: Option<String>,
    source: String,
    detail_json: String,
    occurrences: i64,
    window_occurrences: i64,
    first_occurred_at: String,
    last_occurred_at: String,
    window_started_at: String,
    priority: String,
    state: String,
    revision: i64,
    created_at: String,
    updated_at: String,
    acknowledged_at: Option<String>,
    resolved_at: Option<String>,
    escalation: String,
    remediation_deadline_at: Option<String>,
    escalated_at: Option<String>,
    escalation_reason: Option<String>,
    user_notified_at: Option<String>,
    user_notification: Option<String>,
}

fn raw_incident(row: &rusqlite::Row<'_>) -> rusqlite::Result<RawIncident> {
    Ok(RawIncident {
        sequence: row.get(0)?,
        contract_version: row.get(1)?,
        id: row.get(2)?,
        code: row.get(3)?,
        fingerprint: row.get(4)?,
        title: row.get(5)?,
        explanation: row.get(6)?,
        next_action: row.get(7)?,
        project_id: row.get(8)?,
        employee_id: row.get(9)?,
        provider: row.get(10)?,
        task_id: row.get(11)?,
        session_id: row.get(12)?,
        turn_id: row.get(13)?,
        message_id: row.get(14)?,
        source: row.get(15)?,
        detail_json: row.get(16)?,
        occurrences: row.get(17)?,
        window_occurrences: row.get(18)?,
        first_occurred_at: row.get(19)?,
        last_occurred_at: row.get(20)?,
        window_started_at: row.get(21)?,
        priority: row.get(22)?,
        state: row.get(23)?,
        revision: row.get(24)?,
        created_at: row.get(25)?,
        updated_at: row.get(26)?,
        acknowledged_at: row.get(27)?,
        resolved_at: row.get(28)?,
        escalation: row.get(29)?,
        remediation_deadline_at: row.get(30)?,
        escalated_at: row.get(31)?,
        escalation_reason: row.get(32)?,
        user_notified_at: row.get(33)?,
        user_notification: row.get(34)?,
    })
}

fn incident_from_raw(raw: RawIncident) -> Result<IncidentRecord, StoreError> {
    Ok(IncidentRecord {
        sequence: unsigned(raw.sequence)?,
        version: u32::try_from(raw.contract_version).map_err(|_| StoreError::NegativeSequence)?,
        id: employee::parse_uuid(&raw.id, "incidents.id")?,
        code: raw.code,
        fingerprint: raw.fingerprint,
        title: raw.title,
        explanation: raw.explanation,
        next_action: raw.next_action,
        project_id: optional_uuid(raw.project_id.as_deref(), "incidents.project_id")?,
        employee_id: optional_uuid(raw.employee_id.as_deref(), "incidents.employee_id")?,
        provider: raw.provider,
        task_id: optional_uuid(raw.task_id.as_deref(), "incidents.task_id")?,
        session_id: optional_uuid(raw.session_id.as_deref(), "incidents.session_id")?,
        turn_id: optional_uuid(raw.turn_id.as_deref(), "incidents.turn_id")?,
        message_id: optional_uuid(raw.message_id.as_deref(), "incidents.message_id")?,
        source: serde_json::from_value(Value::String(raw.source))?,
        detail: serde_json::from_str(&raw.detail_json)?,
        occurrences: unsigned(raw.occurrences)?,
        window_occurrences: unsigned(raw.window_occurrences)?,
        first_occurred_at: raw.first_occurred_at,
        last_occurred_at: raw.last_occurred_at,
        window_started_at: raw.window_started_at,
        priority: serde_json::from_value(Value::String(raw.priority))?,
        state: serde_json::from_value(Value::String(raw.state))?,
        revision: unsigned(raw.revision)?,
        created_at: raw.created_at,
        updated_at: raw.updated_at,
        acknowledged_at: raw.acknowledged_at,
        resolved_at: raw.resolved_at,
        escalation: serde_json::from_value(Value::String(raw.escalation))?,
        remediation_deadline_at: raw.remediation_deadline_at,
        escalated_at: raw.escalated_at,
        escalation_reason: raw
            .escalation_reason
            .map(|reason| serde_json::from_value(Value::String(reason)))
            .transpose()?,
        remedies: Vec::new(),
        user_notified_at: raw.user_notified_at,
        user_notification: raw
            .user_notification
            .map(|value| serde_json::from_value(Value::String(value)))
            .transpose()?,
    })
}

fn serialized_name<T: serde::Serialize>(value: T) -> Result<String, StoreError> {
    match serde_json::to_value(value)? {
        Value::String(value) => Ok(value),
        _ => Err(StoreError::InvalidRecord {
            field: "incident enum",
            message: "enum did not serialize to a string".to_owned(),
        }),
    }
}

fn optional_uuid(value: Option<&str>, field: &'static str) -> Result<Option<Uuid>, StoreError> {
    value
        .map(|value| employee::parse_uuid(value, field))
        .transpose()
}

fn to_i64(value: u64) -> Result<i64, StoreError> {
    i64::try_from(value).map_err(|_| StoreError::NegativeSequence)
}

fn unsigned(value: i64) -> Result<u64, StoreError> {
    u64::try_from(value).map_err(|_| StoreError::NegativeSequence)
}
