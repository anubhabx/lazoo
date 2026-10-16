use chrono::{DateTime, Utc};
use lazoo_domain::{derive_availability, idle_session_due};
use lazoo_protocol::{
    EmployeeAvailability, EmployeeSessionRecord, SessionLifecycleState, SessionMutationResult,
    SessionReconcileResult,
};
use rusqlite::{Connection, OptionalExtension, Transaction, TransactionBehavior, params};
use serde_json::json;
use uuid::Uuid;

use crate::{Store, StoreError, employee};

#[derive(Clone, Debug)]
pub struct BeginSessionInput {
    pub request_id: String,
    pub idempotency_key: String,
    pub employee_id: Uuid,
    pub expected_employee_revision: u64,
    pub owner_boot_id: Uuid,
}

#[derive(Clone, Debug)]
pub struct MarkSessionReadyInput {
    pub session_id: Uuid,
    pub expected_revision: u64,
    pub owner_boot_id: Uuid,
    pub native_session_id: String,
    pub native_host_pid: u32,
}

#[derive(Clone, Debug)]
pub struct BeginSessionStopInput {
    pub request_id: String,
    pub idempotency_key: String,
    pub session_id: Uuid,
    pub expected_revision: u64,
}

#[derive(Clone, Debug)]
pub struct BeginIdleSessionStopInput {
    pub session_id: Uuid,
    pub owner_boot_id: Uuid,
    pub now: DateTime<Utc>,
}

#[derive(Clone, Debug)]
pub struct FinishSessionStopInput {
    pub session_id: Uuid,
    pub expected_revision: u64,
}

#[derive(Clone, Debug)]
pub struct FailSessionInput {
    pub session_id: Uuid,
    pub expected_revision: u64,
    pub failure_code: String,
    pub failure_message: String,
}

impl Store {
    pub fn begin_session(
        &self,
        input: &BeginSessionInput,
    ) -> Result<SessionMutationResult, StoreError> {
        let mut connection = self.lock()?;
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        if let Some(session) =
            session_for_command(&transaction, "session.warm", &input.idempotency_key)?
        {
            let result = SessionMutationResult {
                session,
                roster_revision: employee::roster_revision(&transaction)?,
                event_sequence: None,
            };
            transaction.commit()?;
            return Ok(result);
        }

        let employee = employee::query_employee(&transaction, input.employee_id)?.ok_or(
            StoreError::EmployeeNotFound {
                employee_id: input.employee_id,
            },
        )?;
        if employee.configuration_revision != input.expected_employee_revision {
            return Err(StoreError::EmployeeStale {
                employee_id: employee.id,
                current_revision: employee.configuration_revision,
            });
        }
        if !employee.enabled {
            return Err(StoreError::EmployeeDisabled {
                employee_id: employee.id,
            });
        }
        let observation = employee::query_provider_observation(&transaction, &employee.provider)?;
        let (availability, reason) = derive_availability(true, observation.as_ref());
        if availability != EmployeeAvailability::Dormant {
            return Err(StoreError::SessionProviderNotReady {
                employee_id: employee.id,
                reason: reason.unwrap_or_else(|| "provider is not route ready".to_owned()),
            });
        }
        if let Some(active) = active_session_for_employee(&transaction, employee.id)? {
            return Err(StoreError::SessionConflict {
                employee_id: employee.id,
                session_id: active.id,
            });
        }

        let now = Utc::now().to_rfc3339();
        let session_id = Uuid::new_v4();
        transaction.execute(
            "INSERT INTO employee_sessions (
                id, employee_id, employee_configuration_revision, provider, state, revision,
                owner_boot_id, created_at, updated_at
             ) VALUES (?1, ?2, ?3, ?4, 'starting', 1, ?5, ?6, ?6)",
            params![
                session_id.to_string(),
                employee.id.to_string(),
                i64::try_from(employee.configuration_revision)
                    .map_err(|_| StoreError::NegativeSequence)?,
                employee.provider,
                input.owner_boot_id.to_string(),
                now,
            ],
        )?;
        insert_session_command(
            &transaction,
            "session.warm",
            &input.idempotency_key,
            &input.request_id,
            session_id,
            &now,
        )?;
        let roster_revision = employee::advance_roster(&transaction, &now)?;
        let session = query_session(&transaction, session_id)?
            .ok_or(StoreError::SessionNotFound { session_id })?;
        let event_sequence = employee::insert_event(
            &transaction,
            "session.starting",
            &json!({"session": session, "rosterRevision": roster_revision}),
            &now,
        )?;
        let result = SessionMutationResult {
            session,
            roster_revision,
            event_sequence: Some(event_sequence),
        };
        transaction.commit()?;
        Ok(result)
    }

    pub fn mark_session_ready(
        &self,
        input: &MarkSessionReadyInput,
    ) -> Result<SessionMutationResult, StoreError> {
        let mut connection = self.lock()?;
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let current = required_session(&transaction, input.session_id)?;
        require_session_revision(&current, input.expected_revision)?;
        require_state(&current, SessionLifecycleState::Starting, "starting")?;
        if current.owner_boot_id != Some(input.owner_boot_id) {
            return Err(StoreError::SessionOwnership {
                session_id: current.id,
            });
        }
        let now = Utc::now().to_rfc3339();
        transaction.execute(
            "UPDATE employee_sessions SET
                state = 'idle', revision = revision + 1, native_session_id = ?2,
                native_host_pid = ?3, updated_at = ?4, started_at = ?4
             WHERE id = ?1",
            params![
                current.id.to_string(),
                input.native_session_id,
                i64::from(input.native_host_pid),
                now,
            ],
        )?;
        transition_with_roster_event(&transaction, current.id, "session.ready", &now).and_then(
            |result| {
                transaction.commit()?;
                Ok(result)
            },
        )
    }

    pub fn begin_session_stop(
        &self,
        input: &BeginSessionStopInput,
    ) -> Result<SessionMutationResult, StoreError> {
        let mut connection = self.lock()?;
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        if let Some(session) =
            session_for_command(&transaction, "session.stop", &input.idempotency_key)?
        {
            let result = SessionMutationResult {
                session,
                roster_revision: employee::roster_revision(&transaction)?,
                event_sequence: None,
            };
            transaction.commit()?;
            return Ok(result);
        }
        let current = required_session(&transaction, input.session_id)?;
        require_session_revision(&current, input.expected_revision)?;
        require_state(&current, SessionLifecycleState::Idle, "idle")?;
        let result = mark_stopping(
            &transaction,
            current.id,
            &input.idempotency_key,
            &input.request_id,
        )?;
        transaction.commit()?;
        Ok(result)
    }

    /// Idle sessions owned by `owner_boot_id` that `begin_idle_session_stop` would stop at `now`.
    pub fn idle_sessions_due(
        &self,
        owner_boot_id: Uuid,
        now: DateTime<Utc>,
    ) -> Result<Vec<EmployeeSessionRecord>, StoreError> {
        let connection = self.lock()?;
        let mut statement = connection.prepare(
            "SELECT id, employee_id, employee_configuration_revision, provider, state, revision,
                    owner_boot_id, native_session_id, native_host_pid, failure_code,
                    failure_message, created_at, updated_at, started_at, stopped_at
             FROM employee_sessions WHERE state = 'idle' AND owner_boot_id = ?1
             ORDER BY created_at, id",
        )?;
        let idle = statement
            .query_map([owner_boot_id.to_string()], raw_session)?
            .map(|row| session_from_raw(row?))
            .collect::<Result<Vec<_>, StoreError>>()?;
        let mut due = Vec::new();
        for session in idle {
            if session_idle_due(&connection, &session, owner_boot_id, now)? {
                due.push(session);
            }
        }
        Ok(due)
    }

    /// Begins stopping an idle session for idle suspension, keyed `idle-suspend:<sessionId>` so
    /// the session ledger names why it stopped. Due-ness is re-checked in the same transaction
    /// that marks the session `stopping`, so a turn that reached the session first keeps it, and
    /// a turn that comes later finds it no longer idle. `None` means the session is not due.
    pub fn begin_idle_session_stop(
        &self,
        input: &BeginIdleSessionStopInput,
    ) -> Result<Option<SessionMutationResult>, StoreError> {
        let mut connection = self.lock()?;
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let key = format!("idle-suspend:{}", input.session_id);
        if let Some(session) = session_for_command(&transaction, "session.stop", &key)? {
            let result = SessionMutationResult {
                session,
                roster_revision: employee::roster_revision(&transaction)?,
                event_sequence: None,
            };
            transaction.commit()?;
            return Ok(Some(result));
        }
        let current = required_session(&transaction, input.session_id)?;
        if !session_idle_due(&transaction, &current, input.owner_boot_id, input.now)? {
            return Ok(None);
        }
        let result = mark_stopping(&transaction, current.id, &key, &key)?;
        transaction.commit()?;
        Ok(Some(result))
    }

    pub fn finish_session_stop(
        &self,
        input: &FinishSessionStopInput,
    ) -> Result<SessionMutationResult, StoreError> {
        let mut connection = self.lock()?;
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let current = required_session(&transaction, input.session_id)?;
        require_session_revision(&current, input.expected_revision)?;
        require_state(&current, SessionLifecycleState::Stopping, "stopping")?;
        let now = Utc::now().to_rfc3339();
        transaction.execute(
            "UPDATE employee_sessions SET
                state = 'suspended', revision = revision + 1, owner_boot_id = NULL,
                native_host_pid = NULL, updated_at = ?2, stopped_at = ?2
             WHERE id = ?1",
            params![current.id.to_string(), now],
        )?;
        transition_with_roster_event(&transaction, current.id, "session.suspended", &now).and_then(
            |result| {
                transaction.commit()?;
                Ok(result)
            },
        )
    }

    pub fn fail_session(
        &self,
        input: &FailSessionInput,
    ) -> Result<SessionMutationResult, StoreError> {
        let mut connection = self.lock()?;
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let current = required_session(&transaction, input.session_id)?;
        require_session_revision(&current, input.expected_revision)?;
        if !is_active(current.state) {
            return Err(StoreError::SessionState {
                session_id: current.id,
                state: state_text(current.state).to_owned(),
                expected: "starting, idle, or stopping",
            });
        }
        let now = Utc::now().to_rfc3339();
        transaction.execute(
            "UPDATE employee_sessions SET
                state = 'failed', revision = revision + 1, owner_boot_id = NULL,
                native_host_pid = NULL, failure_code = ?2, failure_message = ?3,
                updated_at = ?4, stopped_at = ?4
             WHERE id = ?1",
            params![
                current.id.to_string(),
                input.failure_code,
                input.failure_message,
                now,
            ],
        )?;
        transition_with_roster_event(&transaction, current.id, "session.failed", &now).and_then(
            |result| {
                transaction.commit()?;
                Ok(result)
            },
        )
    }

    pub fn reconcile_sessions(
        &self,
        current_boot_id: Uuid,
    ) -> Result<SessionReconcileResult, StoreError> {
        let mut connection = self.lock()?;
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let mut statement = transaction.prepare(
            "SELECT id FROM employee_sessions
             WHERE state IN ('starting', 'idle', 'stopping')
               AND (owner_boot_id IS NULL OR owner_boot_id <> ?1)
             ORDER BY created_at, id",
        )?;
        let ids = statement
            .query_map([current_boot_id.to_string()], |row| row.get::<_, String>(0))?
            .collect::<Result<Vec<_>, _>>()?;
        drop(statement);
        if ids.is_empty() {
            let result = SessionReconcileResult {
                sessions: Vec::new(),
                roster_revision: employee::roster_revision(&transaction)?,
                event_sequence: None,
            };
            transaction.commit()?;
            return Ok(result);
        }

        let now = Utc::now().to_rfc3339();
        let mut sessions = Vec::with_capacity(ids.len());
        for id in ids {
            let session_id = employee::parse_uuid(&id, "employee_sessions.id")?;
            transaction.execute(
                "UPDATE employee_sessions SET
                    state = 'failed', revision = revision + 1, owner_boot_id = NULL,
                    native_host_pid = NULL, failure_code = 'LZ-SESSION-OWNER-LOST',
                    failure_message = 'the daemon that owned this native session stopped',
                    updated_at = ?2, stopped_at = ?2
                 WHERE id = ?1",
                params![id, now],
            )?;
            sessions.push(required_session(&transaction, session_id)?);
        }
        let roster_revision = employee::advance_roster(&transaction, &now)?;
        let event_sequence = employee::insert_event(
            &transaction,
            "sessions.reconciled",
            &json!({"sessions": sessions, "rosterRevision": roster_revision}),
            &now,
        )?;
        let result = SessionReconcileResult {
            sessions,
            roster_revision,
            event_sequence: Some(event_sequence),
        };
        transaction.commit()?;
        Ok(result)
    }

    pub fn list_sessions(&self) -> Result<Vec<EmployeeSessionRecord>, StoreError> {
        let connection = self.lock()?;
        query_sessions(&connection)
    }

    pub fn get_session(&self, session_id: Uuid) -> Result<EmployeeSessionRecord, StoreError> {
        let connection = self.lock()?;
        required_session(&connection, session_id)
    }

    /// Audits the owner's terminal takeover of a suspended session. The session row itself is
    /// unchanged: the native harness now runs under the owner, outside Lazoo's turn ledger.
    pub fn record_session_terminal_attached(
        &self,
        session_id: Uuid,
        terminal_id: Uuid,
    ) -> Result<u64, StoreError> {
        let mut connection = self.lock()?;
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let now = Utc::now().to_rfc3339();
        let sequence = employee::insert_event(
            &transaction,
            "session.terminal.attached",
            &json!({"sessionId": session_id, "terminalId": terminal_id}),
            &now,
        )?;
        transaction.commit()?;
        Ok(sequence)
    }
}

pub(crate) fn latest_session_for_employee(
    connection: &Connection,
    employee_id: Uuid,
    employee_configuration_revision: u64,
) -> Result<Option<EmployeeSessionRecord>, StoreError> {
    let revision =
        i64::try_from(employee_configuration_revision).map_err(|_| StoreError::NegativeSequence)?;
    connection
        .query_row(
            "SELECT id, employee_id, employee_configuration_revision, provider, state, revision,
                    owner_boot_id, native_session_id, native_host_pid, failure_code,
                    failure_message, created_at, updated_at, started_at, stopped_at
             FROM employee_sessions
             WHERE employee_id = ?1 AND employee_configuration_revision = ?2
             ORDER BY rowid DESC LIMIT 1",
            params![employee_id.to_string(), revision],
            raw_session,
        )
        .optional()?
        .map(session_from_raw)
        .transpose()
}

pub(crate) fn active_session_id_for_employee(
    connection: &Connection,
    employee_id: Uuid,
) -> Result<Option<Uuid>, StoreError> {
    Ok(active_session_for_employee(connection, employee_id)?.map(|session| session.id))
}

fn mark_stopping(
    transaction: &Transaction<'_>,
    session_id: Uuid,
    idempotency_key: &str,
    request_id: &str,
) -> Result<SessionMutationResult, StoreError> {
    let now = Utc::now().to_rfc3339();
    transaction.execute(
        "UPDATE employee_sessions
         SET state = 'stopping', revision = revision + 1, updated_at = ?2
         WHERE id = ?1",
        params![session_id.to_string(), now],
    )?;
    insert_session_command(
        transaction,
        "session.stop",
        idempotency_key,
        request_id,
        session_id,
        &now,
    )?;
    let session = required_session(transaction, session_id)?;
    let roster_revision = employee::roster_revision(transaction)?;
    let event_sequence = employee::insert_event(
        transaction,
        "session.stopping",
        &json!({"session": session, "rosterRevision": roster_revision}),
        &now,
    )?;
    Ok(SessionMutationResult {
        session,
        roster_revision,
        event_sequence: Some(event_sequence),
    })
}

/// An idle session owned by this boot is due once its employee's idle timeout has passed since
/// the session started or a turn on it last changed, and no turn on it is in flight. A turn
/// still `recorded` resets that clock but does not hold the session: delivery finds the
/// recipient's current session, and a turn left `recorded` must not keep a process up forever.
// ponytail: scans the turns of each idle session without an index; add one on
// recipient_session_id if the turn ledger ever makes the sweep measurable.
fn session_idle_due(
    connection: &Connection,
    session: &EmployeeSessionRecord,
    owner_boot_id: Uuid,
    now: DateTime<Utc>,
) -> Result<bool, StoreError> {
    if session.state != SessionLifecycleState::Idle || session.owner_boot_id != Some(owner_boot_id)
    {
        return Ok(false);
    }
    let idle_timeout_seconds: i64 = connection.query_row(
        "SELECT idle_timeout_seconds FROM employees WHERE id = ?1",
        [session.employee_id.to_string()],
        |row| row.get(0),
    )?;
    let idle_timeout_seconds =
        u32::try_from(idle_timeout_seconds).map_err(|_| StoreError::NegativeSequence)?;
    if idle_timeout_seconds == 0 {
        return Ok(false);
    }
    let (unfinished, last_turn_change): (i64, Option<String>) = connection.query_row(
        "SELECT COUNT(*) FILTER (WHERE state IN ('submitting', 'accepted')),
                MAX(updated_at)
         FROM conversation_turns WHERE recipient_session_id = ?1",
        [session.id.to_string()],
        |row| Ok((row.get(0)?, row.get(1)?)),
    )?;
    if unfinished > 0 {
        return Ok(false);
    }
    let mut quiet_since = parse_time(
        session.started_at.as_deref().unwrap_or(&session.updated_at),
        "employee_sessions.started_at",
    )?;
    if let Some(changed) = last_turn_change {
        quiet_since = quiet_since.max(parse_time(&changed, "conversation_turns.updated_at")?);
    }
    Ok(idle_session_due(
        idle_timeout_seconds,
        (now - quiet_since).num_seconds(),
    ))
}

fn parse_time(value: &str, field: &'static str) -> Result<DateTime<Utc>, StoreError> {
    DateTime::parse_from_rfc3339(value)
        .map(|time| time.with_timezone(&Utc))
        .map_err(|error| StoreError::InvalidRecord {
            field,
            message: error.to_string(),
        })
}

fn transition_with_roster_event(
    transaction: &Transaction<'_>,
    session_id: Uuid,
    event_type: &str,
    now: &str,
) -> Result<SessionMutationResult, StoreError> {
    let roster_revision = employee::advance_roster(transaction, now)?;
    let session = required_session(transaction, session_id)?;
    let event_sequence = employee::insert_event(
        transaction,
        event_type,
        &json!({"session": session, "rosterRevision": roster_revision}),
        now,
    )?;
    Ok(SessionMutationResult {
        session,
        roster_revision,
        event_sequence: Some(event_sequence),
    })
}

fn insert_session_command(
    transaction: &Transaction<'_>,
    scope: &str,
    idempotency_key: &str,
    request_id: &str,
    session_id: Uuid,
    now: &str,
) -> Result<(), StoreError> {
    transaction.execute(
        "INSERT INTO session_commands (scope, idempotency_key, request_id, session_id, created_at)
         VALUES (?1, ?2, ?3, ?4, ?5)",
        params![
            scope,
            idempotency_key,
            request_id,
            session_id.to_string(),
            now
        ],
    )?;
    Ok(())
}

fn session_for_command(
    connection: &Connection,
    scope: &str,
    idempotency_key: &str,
) -> Result<Option<EmployeeSessionRecord>, StoreError> {
    let id = connection
        .query_row(
            "SELECT session_id FROM session_commands WHERE scope = ?1 AND idempotency_key = ?2",
            params![scope, idempotency_key],
            |row| row.get::<_, String>(0),
        )
        .optional()?;
    id.map(|id| {
        let id = employee::parse_uuid(&id, "session_commands.session_id")?;
        required_session(connection, id)
    })
    .transpose()
}

fn active_session_for_employee(
    connection: &Connection,
    employee_id: Uuid,
) -> Result<Option<EmployeeSessionRecord>, StoreError> {
    connection
        .query_row(
            "SELECT id, employee_id, employee_configuration_revision, provider, state, revision,
                    owner_boot_id, native_session_id, native_host_pid, failure_code,
                    failure_message, created_at, updated_at, started_at, stopped_at
             FROM employee_sessions
             WHERE employee_id = ?1 AND state IN ('starting', 'idle', 'stopping')
             LIMIT 1",
            [employee_id.to_string()],
            raw_session,
        )
        .optional()?
        .map(session_from_raw)
        .transpose()
}

fn query_sessions(connection: &Connection) -> Result<Vec<EmployeeSessionRecord>, StoreError> {
    let mut statement = connection.prepare(
        "SELECT id, employee_id, employee_configuration_revision, provider, state, revision,
                owner_boot_id, native_session_id, native_host_pid, failure_code,
                failure_message, created_at, updated_at, started_at, stopped_at
         FROM employee_sessions ORDER BY created_at, id",
    )?;
    let rows = statement.query_map([], raw_session)?;
    rows.map(|row| session_from_raw(row?)).collect()
}

pub(crate) fn query_session(
    connection: &Connection,
    session_id: Uuid,
) -> Result<Option<EmployeeSessionRecord>, StoreError> {
    connection
        .query_row(
            "SELECT id, employee_id, employee_configuration_revision, provider, state, revision,
                    owner_boot_id, native_session_id, native_host_pid, failure_code,
                    failure_message, created_at, updated_at, started_at, stopped_at
             FROM employee_sessions WHERE id = ?1",
            [session_id.to_string()],
            raw_session,
        )
        .optional()?
        .map(session_from_raw)
        .transpose()
}

fn required_session(
    connection: &Connection,
    session_id: Uuid,
) -> Result<EmployeeSessionRecord, StoreError> {
    query_session(connection, session_id)?.ok_or(StoreError::SessionNotFound { session_id })
}

fn require_session_revision(
    session: &EmployeeSessionRecord,
    expected_revision: u64,
) -> Result<(), StoreError> {
    if session.revision == expected_revision {
        Ok(())
    } else {
        Err(StoreError::SessionStale {
            session_id: session.id,
            current_revision: session.revision,
        })
    }
}

fn require_state(
    session: &EmployeeSessionRecord,
    expected_state: SessionLifecycleState,
    expected: &'static str,
) -> Result<(), StoreError> {
    if session.state == expected_state {
        Ok(())
    } else {
        Err(StoreError::SessionState {
            session_id: session.id,
            state: state_text(session.state).to_owned(),
            expected,
        })
    }
}

const fn is_active(state: SessionLifecycleState) -> bool {
    matches!(
        state,
        SessionLifecycleState::Starting
            | SessionLifecycleState::Idle
            | SessionLifecycleState::Stopping
    )
}

const fn state_text(state: SessionLifecycleState) -> &'static str {
    match state {
        SessionLifecycleState::Starting => "starting",
        SessionLifecycleState::Idle => "idle",
        SessionLifecycleState::Stopping => "stopping",
        SessionLifecycleState::Suspended => "suspended",
        SessionLifecycleState::Failed => "failed",
    }
}

type RawSession = (
    String,
    String,
    i64,
    String,
    String,
    i64,
    Option<String>,
    Option<String>,
    Option<i64>,
    Option<String>,
    Option<String>,
    String,
    String,
    Option<String>,
    Option<String>,
);

fn raw_session(row: &rusqlite::Row<'_>) -> rusqlite::Result<RawSession> {
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
    ))
}

fn session_from_raw(row: RawSession) -> Result<EmployeeSessionRecord, StoreError> {
    Ok(EmployeeSessionRecord {
        id: employee::parse_uuid(&row.0, "employee_sessions.id")?,
        employee_id: employee::parse_uuid(&row.1, "employee_sessions.employee_id")?,
        employee_configuration_revision: u64::try_from(row.2)
            .map_err(|_| StoreError::NegativeSequence)?,
        provider: row.3,
        state: serde_json::from_value(serde_json::Value::String(row.4))?,
        revision: u64::try_from(row.5).map_err(|_| StoreError::NegativeSequence)?,
        owner_boot_id: row
            .6
            .map(|value| employee::parse_uuid(&value, "employee_sessions.owner_boot_id"))
            .transpose()?,
        native_session_id: row.7,
        native_host_pid: row
            .8
            .map(|value| u32::try_from(value).map_err(|_| StoreError::NegativeSequence))
            .transpose()?,
        failure_code: row.9,
        failure_message: row.10,
        created_at: row.11,
        updated_at: row.12,
        started_at: row.13,
        stopped_at: row.14,
    })
}
