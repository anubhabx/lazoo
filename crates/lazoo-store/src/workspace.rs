//! Rows the desktop workspace read models are derived from: one space's conversations with
//! the turn and task facts the marker rule reads, and one conversation's header, turn, and
//! worker rows, each under one lock with the event high-water mark they reflect. Nothing is
//! stored.

use lazoo_domain::{active_descendant_ids, validate_workspace_limit};
use lazoo_protocol::{
    ConversationRecord, ConversationTurnSummary, SessionLifecycleState, TaskDispatchRecord,
    TaskRecord, TaskState, TaskSummary, TurnState, WorkerCounts,
};
use rusqlite::{Connection, OptionalExtension, params};
use serde_json::Value;
use uuid::Uuid;

use crate::{
    Store, StoreError,
    conversation::{raw_turn_summary, required_conversation, turn_summary_from_raw},
    employee::parse_uuid,
    scalar_count,
    task::{ListTasksInput, query_task_dispatch, query_tasks, required_task},
};

/// One conversation with the facts its sidebar row is derived from.
#[derive(Clone, Debug)]
pub struct SpaceConversation {
    pub conversation: ConversationRecord,
    pub first_user_text: Option<String>,
    pub last_turn_state: Option<TurnState>,
    pub active_turns: u64,
    pub workers: WorkerCounts,
    pub root_task_state: Option<TaskState>,
    pub last_result_at: Option<String>,
}

impl Store {
    /// The conversations of one space (`None` is Home), newest `updated_at` first, strictly
    /// before the cursor, at most `limit + 1` so the caller can tell there are more.
    pub fn space_conversations(
        &self,
        project_id: Option<Uuid>,
        before: Option<(&str, Uuid)>,
        limit: u32,
    ) -> Result<(Vec<SpaceConversation>, u64), StoreError> {
        let connection = self.lock()?;
        let ids = {
            let mut statement = connection.prepare(
                "SELECT id FROM conversations
                 WHERE ((?1 IS NULL AND project_id IS NULL) OR project_id = ?1)
                   AND (?2 IS NULL OR updated_at < ?2 OR (updated_at = ?2 AND id < ?3))
                 ORDER BY updated_at DESC, id DESC LIMIT ?4",
            )?;
            let rows = statement.query_map(
                params![
                    project_id.map(|value| value.to_string()),
                    before.map(|(updated_at, _)| updated_at.to_owned()),
                    before.map(|(_, id)| id.to_string()),
                    i64::from(limit) + 1,
                ],
                |row| row.get::<_, String>(0),
            )?;
            rows.map(|row| parse_uuid(&row?, "conversations.id"))
                .collect::<Result<Vec<_>, _>>()?
        };
        let conversations = ids
            .into_iter()
            .map(|id| space_conversation(&connection, id))
            .collect::<Result<Vec<_>, _>>()?;
        let event_high_water = scalar_count(
            &connection,
            "SELECT COALESCE(MAX(sequence), 0) FROM audit_events",
        )?;
        Ok((conversations, event_high_water))
    }
}

fn space_conversation(connection: &Connection, id: Uuid) -> Result<SpaceConversation, StoreError> {
    let conversation = required_conversation(connection, id)?;
    let key = id.to_string();
    let first_user_text = connection
        .query_row(
            "SELECT text FROM conversation_turns
             WHERE conversation_id = ?1 AND author_kind = 'user' ORDER BY sequence LIMIT 1",
            [&key],
            |row| row.get::<_, String>(0),
        )
        .optional()?;
    let last_turn_state = connection
        .query_row(
            "SELECT state FROM conversation_turns WHERE conversation_id = ?1
             ORDER BY sequence DESC LIMIT 1",
            [&key],
            |row| row.get::<_, String>(0),
        )
        .optional()?
        .map(|state| serde_json::from_value::<TurnState>(Value::String(state)))
        .transpose()?;
    let active_turns = count(
        connection,
        "SELECT COUNT(*) FROM conversation_turns WHERE conversation_id = ?1
         AND state IN ('recorded', 'submitting', 'accepted')",
        &key,
    )?;
    let worker = |states: &str| {
        count(
            connection,
            &format!(
                "SELECT COUNT(*) FROM tasks WHERE conversation_id = ?1
                 AND parent_task_id IS NOT NULL AND state IN ({states})"
            ),
            &key,
        )
    };
    let workers = WorkerCounts {
        total: worker(
            "'proposed', 'accepted', 'running', 'cancel-requested', 'completed', 'failed', 'cancelled', 'uncertain'",
        )?,
        running: worker("'running', 'cancel-requested'")?,
        waiting: worker("'proposed', 'accepted'")?,
        failed: worker("'failed', 'uncertain'")?,
        completed: worker("'completed'")?,
    };
    let root_task_state = connection
        .query_row(
            "SELECT state FROM tasks WHERE conversation_id = ?1 AND parent_task_id IS NULL
             ORDER BY sequence DESC LIMIT 1",
            [&key],
            |row| row.get::<_, String>(0),
        )
        .optional()?
        .map(|state| serde_json::from_value::<TaskState>(Value::String(state)))
        .transpose()?;
    let last_result_at = connection.query_row(
        "SELECT MAX(terminal_at) FROM (
                 SELECT terminal_at FROM tasks WHERE conversation_id = ?1
                 UNION ALL
                 SELECT terminal_at FROM conversation_turns WHERE conversation_id = ?1
             )",
        [&key],
        |row| row.get::<_, Option<String>>(0),
    )?;
    Ok(SpaceConversation {
        conversation,
        first_user_text,
        last_turn_state,
        active_turns,
        workers,
        root_task_state,
        last_result_at,
    })
}

fn count(connection: &Connection, sql: &str, key: &str) -> Result<u64, StoreError> {
    let value: i64 = connection.query_row(sql, [key], |row| row.get(0))?;
    u64::try_from(value).map_err(|_| StoreError::NegativeSequence)
}

/// One task below a root with the rows its dock card is derived from.
#[derive(Clone, Debug)]
pub struct WorkerRow {
    pub task: TaskRecord,
    pub dispatch: Option<TaskDispatchRecord>,
    pub active_turn_state: Option<TurnState>,
    pub session_state: Option<SessionLifecycleState>,
    /// A descendant of this task is not yet terminal.
    pub active_descendants: bool,
}

/// One conversation with the rows its header, turns, and worker dock are derived from.
#[derive(Clone, Debug)]
pub struct ConversationWorkspace {
    pub conversation: ConversationRecord,
    pub first_user_text: Option<String>,
    /// The newest root task.
    pub root_task: Option<TaskSummary>,
    /// The newest `turn_limit` turns, oldest first.
    pub turns: Vec<ConversationTurnSummary>,
    pub turns_truncated: bool,
    pub next_before_turn_sequence: Option<u64>,
    /// The lead's newest turn (id, state, revision).
    pub latest_lead_turn: Option<(Uuid, TurnState, u64)>,
    /// Every task below a root, oldest first.
    pub workers: Vec<WorkerRow>,
    pub event_high_water: u64,
}

impl Store {
    /// The rows of one conversation under one lock. `turn_limit` bounds the turns to the
    /// newest ones.
    pub fn conversation_workspace(
        &self,
        id: Uuid,
        turn_limit: u32,
    ) -> Result<ConversationWorkspace, StoreError> {
        self.conversation_workspace_page(id, turn_limit, None)
    }

    /// As `conversation_workspace`, but page backward from an exclusive turn sequence.
    pub fn conversation_workspace_page(
        &self,
        id: Uuid,
        turn_limit: u32,
        before_turn_sequence: Option<u64>,
    ) -> Result<ConversationWorkspace, StoreError> {
        validate_workspace_limit(turn_limit)?;
        if before_turn_sequence.is_some_and(|value| value == 0 || value > i64::MAX as u64) {
            return Err(StoreError::NegativeSequence);
        }
        let connection = self.lock()?;
        let conversation = required_conversation(&connection, id)?;
        let key = id.to_string();
        let first_user_text = connection
            .query_row(
                "SELECT text FROM conversation_turns
                 WHERE conversation_id = ?1 AND author_kind = 'user' ORDER BY sequence LIMIT 1",
                [&key],
                |row| row.get::<_, String>(0),
            )
            .optional()?;
        let mut turns = {
            let mut statement = connection.prepare(
                "SELECT sequence, id, conversation_id, task_id, author_kind, author_employee_id,
                        author_session_id, recipient_employee_id, recipient_session_id, state,
                        revision, native_turn_id, created_at, updated_at
                 FROM conversation_turns
                 WHERE conversation_id = ?1 AND (?2 IS NULL OR sequence < ?2)
                 ORDER BY sequence DESC LIMIT ?3",
            )?;
            let before = before_turn_sequence
                .map(|value| i64::try_from(value).map_err(|_| StoreError::NegativeSequence))
                .transpose()?;
            let rows = statement.query_map(
                params![key, before, i64::from(turn_limit) + 1],
                raw_turn_summary,
            )?;
            rows.map(|row| turn_summary_from_raw(row?))
                .collect::<Result<Vec<_>, _>>()?
        };
        let turns_truncated = turns.len() > turn_limit as usize;
        turns.truncate(turn_limit as usize);
        turns.reverse();
        let next_before_turn_sequence = turns_truncated
            .then(|| turns.first().map(|turn| turn.sequence))
            .flatten();
        let latest_lead_turn = connection
            .query_row(
                "SELECT id, state, revision FROM conversation_turns
                 WHERE conversation_id = ?1 AND recipient_employee_id = ?2
                 ORDER BY sequence DESC LIMIT 1",
                params![key, conversation.lead_employee_id.to_string()],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, i64>(2)?,
                    ))
                },
            )
            .optional()?
            .map(|(id, state, revision)| {
                Ok::<_, StoreError>((
                    parse_uuid(&id, "conversation_turns.id")?,
                    serde_json::from_value::<TurnState>(Value::String(state))?,
                    u64::try_from(revision).map_err(|_| StoreError::NegativeSequence)?,
                ))
            })
            .transpose()?;
        let tasks = conversation_tasks(&connection, id)?;
        let root_task = tasks
            .iter()
            .rfind(|task| task.parent_task_id.is_none())
            .cloned();
        let workers = worker_rows(&connection, &tasks)?;
        let event_high_water = scalar_count(
            &connection,
            "SELECT COALESCE(MAX(sequence), 0) FROM audit_events",
        )?;
        Ok(ConversationWorkspace {
            conversation,
            first_user_text,
            root_task,
            turns,
            turns_truncated,
            next_before_turn_sequence,
            latest_lead_turn,
            workers,
            event_high_water,
        })
    }
}

// ponytail: one small read per worker; the tree is bounded (fan-out 8, depth 4).
/// Every task of one conversation, oldest first, however many there are.
fn conversation_tasks(connection: &Connection, id: Uuid) -> Result<Vec<TaskSummary>, StoreError> {
    const PAGE: u32 = 1024;
    let mut tasks = Vec::new();
    let mut after_sequence = 0;
    loop {
        let page = query_tasks(
            connection,
            &ListTasksInput {
                after_sequence,
                limit: PAGE,
                root_task_id: None,
                parent_task_id: None,
                conversation_id: Some(id),
                project_id: None,
                assigned_employee_id: None,
                state: None,
            },
        )?;
        let short = page.len() < PAGE as usize;
        if let Some(last) = page.last() {
            after_sequence = last.sequence;
        }
        tasks.extend(page);
        if short {
            return Ok(tasks);
        }
    }
}

fn worker_rows(
    connection: &Connection,
    tasks: &[TaskSummary],
) -> Result<Vec<WorkerRow>, StoreError> {
    let blocked = active_descendant_ids(tasks);
    tasks
        .iter()
        .filter(|task| task.parent_task_id.is_some())
        .map(|summary| {
            let task = required_task(connection, summary.id)?;
            let dispatch = query_task_dispatch(connection, summary.id)?;
            let active_turn_state = task
                .active_turn_id
                .map(|turn_id| {
                    state_of(
                        connection,
                        "SELECT state FROM conversation_turns WHERE id = ?1",
                        turn_id,
                    )
                })
                .transpose()?
                .flatten();
            let session_state = task
                .active_session_id
                .map(|session_id| {
                    state_of(
                        connection,
                        "SELECT state FROM employee_sessions WHERE id = ?1",
                        session_id,
                    )
                })
                .transpose()?
                .flatten();
            Ok(WorkerRow {
                active_descendants: blocked.contains(&summary.id),
                task,
                dispatch,
                active_turn_state,
                session_state,
            })
        })
        .collect()
}

fn state_of<T: serde::de::DeserializeOwned>(
    connection: &Connection,
    sql: &str,
    id: Uuid,
) -> Result<Option<T>, StoreError> {
    connection
        .query_row(sql, [id.to_string()], |row| row.get::<_, String>(0))
        .optional()?
        .map(|state| serde_json::from_value::<T>(Value::String(state)))
        .transpose()
        .map_err(StoreError::from)
}
