//! Rows the desktop workspace read models are derived from: one space's conversations with
//! the turn and task facts the marker rule reads, under one lock with the event high-water
//! mark they reflect. Nothing is stored.

use lazoo_protocol::{ConversationRecord, TaskState, TurnState, WorkerCounts};
use rusqlite::{Connection, OptionalExtension, params};
use serde_json::Value;
use uuid::Uuid;

use crate::{
    Store, StoreError, conversation::required_conversation, employee::parse_uuid, scalar_count,
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
