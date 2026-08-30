mod emergency;
mod employee;
mod migration;
mod session;

use std::{
    path::{Path, PathBuf},
    sync::{Arc, Mutex, MutexGuard},
    time::Duration,
};

use chrono::Utc;
use lazoo_protocol::{CheckoutMode, EventSummary, ProjectRecord, RegisterProjectResult};
use rusqlite::{Connection, OptionalExtension, TransactionBehavior, params};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use thiserror::Error;
use uuid::Uuid;

pub use emergency::EmergencyIncident;
pub use employee::{
    CreateEmployeeInput, DisableEmployeeInput, RefreshProviderObservationsInput,
    UpdateEmployeeInput,
};
pub use session::{
    BeginSessionInput, BeginSessionStopInput, FailSessionInput, FinishSessionStopInput,
    MarkSessionReadyInput,
};

use crate::{emergency::import_pending_incidents, migration::apply_migrations};

#[derive(Clone, Debug)]
pub struct Store {
    connection: Arc<Mutex<Connection>>,
}

#[derive(Clone, Debug)]
pub struct RegisterProjectInput {
    pub request_id: String,
    pub idempotency_key: String,
    pub repository_identity: String,
    pub canonical_path: PathBuf,
    pub display_name: String,
    pub checkout_mode: CheckoutMode,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StoreStatus {
    pub schema_version: u32,
    pub journal_mode: String,
    pub foreign_keys: bool,
    pub project_count: u64,
    pub event_high_water: u64,
    pub outbox_pending: u64,
    pub emergency_incident_count: u64,
}

#[derive(Debug, Error)]
pub enum StoreError {
    #[error("SQLite operation failed: {0}")]
    Database(#[from] rusqlite::Error),
    #[error("store JSON is invalid: {0}")]
    Json(#[from] serde_json::Error),
    #[error("store mutex is poisoned")]
    Poisoned,
    #[error("applied migration {version} has an unexpected checksum")]
    MigrationChecksum { version: u32 },
    #[error("database schema version {found} is newer than supported version {supported}")]
    UnknownMigration { found: u32, supported: u32 },
    #[error("project repository is already registered as {project_id}")]
    ProjectConflict { project_id: Uuid },
    #[error("employee name is already registered as {employee_id}")]
    EmployeeConflict { employee_id: Uuid },
    #[error("employee {employee_id} was not found")]
    EmployeeNotFound { employee_id: Uuid },
    #[error("employee {employee_id} changed; current configuration revision is {current_revision}")]
    EmployeeStale {
        employee_id: Uuid,
        current_revision: u64,
    },
    #[error("employee {employee_id} is disabled")]
    EmployeeDisabled { employee_id: Uuid },
    #[error("employee {employee_id} already has active session {session_id}")]
    SessionConflict { employee_id: Uuid, session_id: Uuid },
    #[error("session {session_id} was not found")]
    SessionNotFound { session_id: Uuid },
    #[error("session {session_id} changed; current revision is {current_revision}")]
    SessionStale {
        session_id: Uuid,
        current_revision: u64,
    },
    #[error("session {session_id} is {state}, expected {expected}")]
    SessionState {
        session_id: Uuid,
        state: String,
        expected: &'static str,
    },
    #[error("session {session_id} is owned by another daemon boot")]
    SessionOwnership { session_id: Uuid },
    #[error("stored {field} is invalid: {message}")]
    InvalidRecord {
        field: &'static str,
        message: String,
    },
    #[error("event sequence is negative")]
    NegativeSequence,
    #[error("emergency incident segment is invalid: {path}: {message}")]
    InvalidEmergencySegment { path: PathBuf, message: String },
    #[error("I/O failed for {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
}

impl Store {
    pub fn open_path(path: impl AsRef<Path>) -> Result<Self, StoreError> {
        let path = path.as_ref();
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|source| StoreError::Io {
                path: parent.to_path_buf(),
                source,
            })?;
        }
        let mut connection = Connection::open(path)?;
        connection.busy_timeout(Duration::from_secs(5))?;
        connection.pragma_update(None, "foreign_keys", true)?;
        connection.pragma_update(None, "journal_mode", "WAL")?;
        apply_migrations(&mut connection)?;
        Ok(Self {
            connection: Arc::new(Mutex::new(connection)),
        })
    }

    pub fn register_project(
        &self,
        input: &RegisterProjectInput,
    ) -> Result<RegisterProjectResult, StoreError> {
        let mut connection = self.lock()?;
        if let Some(response) = connection
            .query_row(
                "SELECT response_json FROM processed_commands WHERE scope = 'project.register' AND idempotency_key = ?1",
                [&input.idempotency_key],
                |row| row.get::<_, String>(0),
            )
            .optional()?
        {
            return Ok(serde_json::from_str(&response)?);
        }

        if let Some(project_id) = connection
            .query_row(
                "SELECT id FROM projects WHERE repository_identity = ?1",
                [&input.repository_identity],
                |row| row.get::<_, String>(0),
            )
            .optional()?
        {
            return Err(StoreError::ProjectConflict {
                project_id: Uuid::parse_str(&project_id).map_err(|error| {
                    StoreError::InvalidEmergencySegment {
                        path: PathBuf::from("projects.id"),
                        message: error.to_string(),
                    }
                })?,
            });
        }

        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let project_id = Uuid::new_v4();
        let event_id = Uuid::new_v4();
        let outbox_id = Uuid::new_v4();
        let now = Utc::now().to_rfc3339();
        let canonical_path = input.canonical_path.to_string_lossy().into_owned();
        let checkout_mode = serde_json::to_string(&input.checkout_mode)?;
        transaction.execute(
            "INSERT INTO projects (id, repository_identity, canonical_path, display_name, checkout_mode, configuration_revision, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, 1, ?6)",
            params![
                project_id.to_string(),
                input.repository_identity,
                canonical_path,
                input.display_name,
                checkout_mode,
                now
            ],
        )?;

        let project = ProjectRecord {
            id: project_id,
            repository_identity: input.repository_identity.clone(),
            canonical_path,
            display_name: input.display_name.clone(),
            checkout_mode: input.checkout_mode,
            configuration_revision: 1,
            created_at: now.clone(),
        };
        let payload = serde_json::to_string(&project)?;
        transaction.execute(
            "INSERT INTO audit_events (id, event_type, payload_json, created_at)
             VALUES (?1, 'project.registered', ?2, ?3)",
            params![event_id.to_string(), payload, now],
        )?;
        let event_sequence = u64::try_from(transaction.last_insert_rowid())
            .map_err(|_| StoreError::NegativeSequence)?;
        transaction.execute(
            "INSERT INTO outbox (id, audit_sequence, created_at) VALUES (?1, ?2, ?3)",
            params![
                outbox_id.to_string(),
                i64::try_from(event_sequence).map_err(|_| StoreError::NegativeSequence)?,
                now
            ],
        )?;
        let result = RegisterProjectResult {
            project,
            event_sequence,
        };
        let response = serde_json::to_string(&result)?;
        transaction.execute(
            "INSERT INTO processed_commands (scope, idempotency_key, request_id, response_json, created_at)
             VALUES ('project.register', ?1, ?2, ?3, ?4)",
            params![input.idempotency_key, input.request_id, response, now],
        )?;
        transaction.commit()?;
        Ok(result)
    }

    pub fn list_projects(&self) -> Result<Vec<ProjectRecord>, StoreError> {
        let connection = self.lock()?;
        let mut statement = connection.prepare(
            "SELECT id, repository_identity, canonical_path, display_name, checkout_mode,
                    configuration_revision, created_at
             FROM projects ORDER BY created_at, id",
        )?;
        let rows = statement.query_map([], |row| {
            let id: String = row.get(0)?;
            let checkout_mode: String = row.get(4)?;
            Ok((
                id,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
                checkout_mode,
                row.get::<_, i64>(5)?,
                row.get::<_, String>(6)?,
            ))
        })?;
        rows.map(|row| {
            let (
                id,
                repository_identity,
                canonical_path,
                display_name,
                checkout_mode,
                configuration_revision,
                created_at,
            ) = row?;
            Ok(ProjectRecord {
                id: Uuid::parse_str(&id).map_err(|error| StoreError::InvalidEmergencySegment {
                    path: PathBuf::from("projects.id"),
                    message: error.to_string(),
                })?,
                repository_identity,
                canonical_path,
                display_name,
                checkout_mode: serde_json::from_str(&checkout_mode)?,
                configuration_revision: u64::try_from(configuration_revision)
                    .map_err(|_| StoreError::NegativeSequence)?,
                created_at,
            })
        })
        .collect()
    }

    pub fn list_events(&self, after: u64, limit: u32) -> Result<Vec<EventSummary>, StoreError> {
        let connection = self.lock()?;
        query_events(&connection, after, limit)
    }

    pub fn publish_pending_outbox(&self) -> Result<Vec<EventSummary>, StoreError> {
        let mut connection = self.lock()?;
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let events = query_pending_events(&transaction)?;
        let now = Utc::now().to_rfc3339();
        transaction.execute(
            "UPDATE outbox SET published_at = ?1 WHERE published_at IS NULL",
            [&now],
        )?;
        transaction.commit()?;
        Ok(events)
    }

    pub fn import_pending_incidents(
        &self,
        root: &lazoo_runtime::DataRoot,
    ) -> Result<usize, StoreError> {
        let mut connection = self.lock()?;
        import_pending_incidents(&mut connection, root)
    }

    pub fn status(&self) -> Result<StoreStatus, StoreError> {
        let connection = self.lock()?;
        let schema_version = connection.query_row(
            "SELECT COALESCE(MAX(version), 0) FROM schema_migrations",
            [],
            |row| row.get(0),
        )?;
        let journal_mode = connection.pragma_query_value(None, "journal_mode", |row| row.get(0))?;
        let foreign_keys: i64 =
            connection.pragma_query_value(None, "foreign_keys", |row| row.get(0))?;
        Ok(StoreStatus {
            schema_version,
            journal_mode,
            foreign_keys: foreign_keys == 1,
            project_count: scalar_count(&connection, "SELECT COUNT(*) FROM projects")?,
            event_high_water: scalar_count(
                &connection,
                "SELECT COALESCE(MAX(sequence), 0) FROM audit_events",
            )?,
            outbox_pending: scalar_count(
                &connection,
                "SELECT COUNT(*) FROM outbox WHERE published_at IS NULL",
            )?,
            emergency_incident_count: scalar_count(
                &connection,
                "SELECT COUNT(*) FROM emergency_incident_imports",
            )?,
        })
    }

    #[doc(hidden)]
    pub fn execute_for_test(&self, sql: &str) -> Result<(), StoreError> {
        self.lock()?.execute_batch(sql)?;
        Ok(())
    }

    pub(crate) fn lock(&self) -> Result<MutexGuard<'_, Connection>, StoreError> {
        self.connection.lock().map_err(|_| StoreError::Poisoned)
    }
}

fn scalar_count(connection: &Connection, sql: &str) -> Result<u64, StoreError> {
    let value: i64 = connection.query_row(sql, [], |row| row.get(0))?;
    u64::try_from(value).map_err(|_| StoreError::NegativeSequence)
}

fn query_events(
    connection: &Connection,
    after: u64,
    limit: u32,
) -> Result<Vec<EventSummary>, StoreError> {
    let mut statement = connection.prepare(
        "SELECT sequence, id, event_type, payload_json, created_at
         FROM audit_events WHERE sequence > ?1 ORDER BY sequence LIMIT ?2",
    )?;
    let after = i64::try_from(after).map_err(|_| StoreError::NegativeSequence)?;
    collect_events(statement.query(params![after, i64::from(limit)])?)
}

fn query_pending_events(
    transaction: &rusqlite::Transaction<'_>,
) -> Result<Vec<EventSummary>, StoreError> {
    let mut statement = transaction.prepare(
        "SELECT e.sequence, e.id, e.event_type, e.payload_json, e.created_at
         FROM outbox o JOIN audit_events e ON e.sequence = o.audit_sequence
         WHERE o.published_at IS NULL ORDER BY e.sequence",
    )?;
    collect_events(statement.query([])?)
}

fn collect_events(mut rows: rusqlite::Rows<'_>) -> Result<Vec<EventSummary>, StoreError> {
    let mut events = Vec::new();
    while let Some(row) = rows.next()? {
        let sequence: i64 = row.get(0)?;
        let id: String = row.get(1)?;
        let payload: String = row.get(3)?;
        events.push(EventSummary {
            sequence: u64::try_from(sequence).map_err(|_| StoreError::NegativeSequence)?,
            id: Uuid::parse_str(&id).map_err(|error| StoreError::InvalidEmergencySegment {
                path: PathBuf::from("audit_events.id"),
                message: error.to_string(),
            })?,
            event_type: row.get(2)?,
            payload: serde_json::from_str::<Value>(&payload)?,
            created_at: row.get(4)?,
        });
    }
    Ok(events)
}
