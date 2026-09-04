use lazoo_protocol::{
    AuthorityMode, MessageEndpoint, MessageReference, MessageSender, TASK_CONTRACT_V1, TaskDetail,
    TaskEvidence, TaskRecord, TaskState, TaskSummary, UserMessageEndpoint, UserMessageKind,
};
use rusqlite::{Connection, OptionalExtension, Transaction, params};
use serde_json::{Value, json};
use uuid::Uuid;

use crate::{Store, StoreError};

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
