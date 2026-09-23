use chrono::{DateTime, Utc};
use lazoo_domain::{
    describe_incident, incident_fingerprint, incident_threshold_crossed, incident_window,
    validate_incident_code, validate_incident_detail, validate_incident_list_limit,
    validate_incident_text, validate_incident_title, validate_message_command_key,
};
use lazoo_protocol::{
    INCIDENT_CONTRACT_V1, IncidentMutationResult, IncidentPriority, IncidentRecord, IncidentSource,
    IncidentState,
};
use rusqlite::{Connection, OptionalExtension, Transaction, TransactionBehavior, params};
use serde_json::{Value, json};
use uuid::Uuid;

use crate::{Store, StoreError, employee};

const INCIDENT_COLUMNS: &str = "sequence, contract_version, id, code, fingerprint, title,
    explanation, next_action, project_id, employee_id, provider, task_id, session_id, turn_id,
    message_id, source, detail_json, occurrences, window_occurrences, first_occurred_at,
    last_occurred_at, window_started_at, priority, state, revision, created_at, updated_at,
    acknowledged_at, resolved_at";

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
        let now = Utc::now().to_rfc3339();
        let occurred_at = input.occurred_at.clone().unwrap_or_else(|| now.clone());
        let fingerprint = incident_fingerprint(&input.code, input.employee_id, input.project_id);
        let source = serialized_name(input.source)?;
        let detail_json = serde_json::to_string(&input.detail)?;
        let (incident_id, event_type) = if let Some(current) =
            query_open_by_fingerprint(&transaction, &fingerprint)?
        {
            let elapsed = elapsed_seconds(&current.window_started_at, &occurred_at);
            let (reset, window_occurrences) = incident_window(elapsed, current.window_occurrences);
            let window_started_at = if reset {
                occurred_at.clone()
            } else {
                current.window_started_at.clone()
            };
            let escalate = current.priority == IncidentPriority::Normal
                && incident_threshold_crossed(window_occurrences);
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
            (
                current.id,
                if escalate {
                    "incident.escalated"
                } else {
                    "incident.recurred"
                },
            )
        } else {
            let text = describe_incident(&input.code);
            let id = Uuid::new_v4();
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
                            ?16, 1, 1, ?17, ?17, ?17, 'normal', 'open', 1, ?18, ?18, NULL, NULL
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
                ],
            )?;
            (id, "incident.raised")
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
        let mut statement = connection.prepare(&format!(
            "SELECT {INCIDENT_COLUMNS} FROM incidents
             WHERE sequence > ?1
               AND (?3 IS NULL OR state = ?3)
               AND (?4 IS NULL OR priority = ?4)
               AND (?5 IS NULL OR code = ?5)
               AND (?6 IS NULL OR employee_id = ?6)
               AND (?7 IS NULL OR project_id = ?7)
             ORDER BY sequence LIMIT ?2"
        ))?;
        let rows = statement.query_map(
            params![
                to_i64(input.after_sequence)?,
                i64::from(input.limit),
                state,
                priority,
                input.code,
                input.employee_id.map(|value| value.to_string()),
                input.project_id.map(|value| value.to_string()),
            ],
            raw_incident,
        )?;
        rows.map(|row| incident_from_raw(row?)).collect()
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
        .map(incident_from_raw)
        .transpose()
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
    incident_from_raw(raw)
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
