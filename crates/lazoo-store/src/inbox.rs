//! Rows the desktop Inbox is derived from. The Inbox stores nothing: these reads return the
//! approval, task, and incident rows that `lazoo_domain` turns into items, under one lock with
//! the event high-water mark they reflect.

use std::collections::BTreeMap;

use lazoo_protocol::{ApprovalRecord, EmployeeRecord, IncidentRecord, TaskRecord};
use rusqlite::{Connection, OptionalExtension, params};
use uuid::Uuid;

use crate::{
    Store, StoreError,
    approval::{APPROVAL_COLUMNS, approval_from_raw, raw_approval},
    employee::{parse_uuid, query_employees},
    incident::{INCIDENT_COLUMNS, raw_incident, read_incident},
    scalar_count,
    task::{TASK_RECORD_COLUMNS, raw_task, task_from_raw},
};

/// Rows that can be unresolved Inbox items: pending approvals, `proposed` and `uncertain`
/// tasks, and open or acknowledged incidents escalated to the lead or the user, each with the
/// conversation its task belongs to.
#[derive(Clone, Debug)]
pub struct InboxSources {
    pub approvals: Vec<ApprovalRecord>,
    pub tasks: Vec<TaskRecord>,
    pub incidents: Vec<(IncidentRecord, Option<Uuid>)>,
    pub employees: BTreeMap<Uuid, EmployeeRecord>,
    pub event_high_water: u64,
}

/// Rows that left the unresolved list: decided, expired, or cancelled approvals; terminal
/// tasks that were once `proposed` or `uncertain` (the flag says which); and resolved or
/// remediated escalations. Each source holds at most `limit + 1` rows, newest `updated_at`
/// first, strictly before the cursor.
#[derive(Clone, Debug)]
pub struct InboxArchive {
    pub approvals: Vec<ApprovalRecord>,
    pub tasks: Vec<(TaskRecord, bool)>,
    pub incidents: Vec<(IncidentRecord, Option<Uuid>)>,
    pub employees: BTreeMap<Uuid, EmployeeRecord>,
    pub event_high_water: u64,
}

// ponytail: each unresolved source is capped here; an Inbox with more pending rows than this
// needs paging upstream, not a bigger number.
const SOURCE_LIMIT: i64 = 1000;

impl Store {
    pub fn inbox_sources(&self) -> Result<InboxSources, StoreError> {
        let connection = self.lock()?;
        let approvals = {
            let mut statement = connection.prepare(&format!(
                "SELECT {APPROVAL_COLUMNS} FROM approvals WHERE state = 'pending'
                 ORDER BY sequence LIMIT ?1"
            ))?;
            let rows = statement.query_map([SOURCE_LIMIT], raw_approval)?;
            rows.map(|row| approval_from_raw(row?))
                .collect::<Result<Vec<_>, _>>()?
        };
        let tasks = {
            let mut statement = connection.prepare(&format!(
                "SELECT {TASK_RECORD_COLUMNS} FROM tasks
                 WHERE state IN ('proposed', 'uncertain') ORDER BY sequence LIMIT ?1"
            ))?;
            let rows = statement.query_map([SOURCE_LIMIT], raw_task)?;
            rows.map(|row| task_from_raw(row?))
                .collect::<Result<Vec<_>, _>>()?
        };
        let incidents = {
            let mut statement = connection.prepare(&format!(
                "SELECT {INCIDENT_COLUMNS} FROM incidents
                 WHERE state IN ('open', 'acknowledged') AND escalation IN ('lead', 'user')
                 ORDER BY sequence LIMIT ?1"
            ))?;
            let raws = statement
                .query_map([SOURCE_LIMIT], raw_incident)?
                .collect::<Result<Vec<_>, _>>()?;
            raws.into_iter()
                .map(|raw| incident_with_conversation(&connection, raw))
                .collect::<Result<Vec<_>, _>>()?
        };
        Ok(InboxSources {
            approvals,
            tasks,
            incidents,
            employees: employees_by_id(&connection)?,
            event_high_water: event_high_water(&connection)?,
        })
    }

    /// `before` is the (`updated_at`, id) pair of the last item on the previous page.
    pub fn inbox_archive(
        &self,
        before: Option<(&str, Uuid)>,
        limit: u32,
    ) -> Result<InboxArchive, StoreError> {
        let connection = self.lock()?;
        let before_at = before.map(|(updated_at, _)| updated_at.to_owned());
        let before_id = before.map(|(_, id)| id.to_string());
        let page = i64::from(limit) + 1;
        let cursor = "(?1 IS NULL OR updated_at < ?1 OR (updated_at = ?1 AND id < ?2))";
        let approvals = {
            let mut statement = connection.prepare(&format!(
                "SELECT {APPROVAL_COLUMNS} FROM approvals
                 WHERE state != 'pending' AND {cursor}
                 ORDER BY updated_at DESC, id DESC LIMIT ?3"
            ))?;
            let rows = statement.query_map(params![before_at, before_id, page], raw_approval)?;
            rows.map(|row| approval_from_raw(row?))
                .collect::<Result<Vec<_>, _>>()?
        };
        let tasks = {
            let mut statement = connection.prepare(&format!(
                "SELECT {TASK_RECORD_COLUMNS},
                        EXISTS (SELECT 1 FROM task_evidence
                                WHERE task_id = tasks.id AND kind = 'uncertain')
                 FROM tasks
                 WHERE state IN ('completed', 'failed', 'cancelled')
                   AND EXISTS (SELECT 1 FROM task_evidence
                               WHERE task_id = tasks.id AND kind IN ('proposed', 'uncertain'))
                   AND {cursor}
                 ORDER BY updated_at DESC, id DESC LIMIT ?3"
            ))?;
            let rows = statement.query_map(params![before_at, before_id, page], |row| {
                Ok((raw_task(row)?, row.get::<_, bool>(30)?))
            })?;
            rows.map(|row| {
                let (raw, once_uncertain) = row?;
                Ok((task_from_raw(raw)?, once_uncertain))
            })
            .collect::<Result<Vec<_>, StoreError>>()?
        };
        let incidents = {
            let mut statement = connection.prepare(&format!(
                "SELECT {INCIDENT_COLUMNS} FROM incidents
                 WHERE ((state = 'resolved' AND escalation != 'none')
                        OR escalation = 'remediated')
                   AND {cursor}
                 ORDER BY updated_at DESC, id DESC LIMIT ?3"
            ))?;
            let raws = statement
                .query_map(params![before_at, before_id, page], raw_incident)?
                .collect::<Result<Vec<_>, _>>()?;
            raws.into_iter()
                .map(|raw| incident_with_conversation(&connection, raw))
                .collect::<Result<Vec<_>, _>>()?
        };
        Ok(InboxArchive {
            approvals,
            tasks,
            incidents,
            employees: employees_by_id(&connection)?,
            event_high_water: event_high_water(&connection)?,
        })
    }
}

fn incident_with_conversation(
    connection: &Connection,
    raw: crate::incident::RawIncident,
) -> Result<(IncidentRecord, Option<Uuid>), StoreError> {
    let incident = read_incident(connection, raw)?;
    let conversation_id = match incident.task_id {
        Some(task_id) => connection
            .query_row(
                "SELECT conversation_id FROM tasks WHERE id = ?1",
                [task_id.to_string()],
                |row| row.get::<_, String>(0),
            )
            .optional()?
            .map(|value| parse_uuid(&value, "tasks.conversation_id"))
            .transpose()?,
        None => None,
    };
    Ok((incident, conversation_id))
}

fn employees_by_id(connection: &Connection) -> Result<BTreeMap<Uuid, EmployeeRecord>, StoreError> {
    Ok(query_employees(connection)?
        .into_iter()
        .map(|employee| (employee.id, employee))
        .collect())
}

fn event_high_water(connection: &Connection) -> Result<u64, StoreError> {
    scalar_count(
        connection,
        "SELECT COALESCE(MAX(sequence), 0) FROM audit_events",
    )
}
