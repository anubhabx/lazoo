//! The desktop workspace read models (sidebar rows, the launcher, the lead header, and the
//! worker dock), derived from rows that stay authoritative. Nothing here is stored: the rules
//! below say what a conversation's marker is, what its title is, whether an employee can be
//! started as the lead right now, what the lead's lifecycle is, and which of the dock's eight
//! states a worker is in.

use lazoo_protocol::{
    ApprovalRecord, ConversationMarker, EmployeeAvailability, EmployeeSessionRecord,
    IncidentEscalation, IncidentRecord, LeadLifecycle, RosterEntry, SessionLifecycleState,
    TaskDispatchRecord, TaskDispatchState, TaskRecord, TaskState, TurnState, WorkerCounts,
    WorkerState,
};
use thiserror::Error;

use crate::delivery::truncate_utf8;

pub const MAX_WORKSPACE_LIMIT: u32 = 500;
pub const MAX_DERIVED_TITLE_BYTES: usize = 80;
pub const UNTITLED_CONVERSATION: &str = "New work";

#[derive(Clone, Debug, Eq, Error, PartialEq)]
pub enum WorkspaceValidationError {
    #[error("workspace limit must be between 1 and 500")]
    Limit,
}

/// # Errors
/// Returns `Limit` outside 1 to 500.
pub const fn validate_workspace_limit(limit: u32) -> Result<(), WorkspaceValidationError> {
    if limit == 0 || limit > MAX_WORKSPACE_LIMIT {
        Err(WorkspaceValidationError::Limit)
    } else {
        Ok(())
    }
}

/// What the marker rule reads about one conversation.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct ConversationFacts {
    /// Counted Inbox items scoped to the conversation.
    pub unresolved: u64,
    /// Turns that are `recorded`, `submitting`, or `accepted`.
    pub active_turns: u64,
    pub workers: WorkerCounts,
    /// The newest root task's state.
    pub root_task_state: Option<TaskState>,
    pub last_turn_state: Option<TurnState>,
}

/// The sidebar marker, first rule that holds: something needs the user; work is in flight;
/// the plan or the last turn failed; the plan completed. Otherwise no marker.
#[must_use]
pub fn conversation_marker(
    facts: &ConversationFacts,
) -> (Option<ConversationMarker>, Option<String>) {
    if facts.unresolved > 0 {
        let noun = if facts.unresolved == 1 {
            "item needs"
        } else {
            "items need"
        };
        return (
            Some(ConversationMarker::NeedsAttention),
            Some(format!("{} {noun} you", facts.unresolved)),
        );
    }
    if facts.workers.running > 0 {
        let noun = if facts.workers.running == 1 {
            "worker"
        } else {
            "workers"
        };
        return (
            Some(ConversationMarker::Active),
            Some(format!("{} {noun} running", facts.workers.running)),
        );
    }
    if facts.active_turns > 0 {
        return (
            Some(ConversationMarker::Active),
            Some("a turn is in flight".to_owned()),
        );
    }
    if facts.workers.waiting > 0 {
        return (
            Some(ConversationMarker::Active),
            Some(format!("{} waiting to run", facts.workers.waiting)),
        );
    }
    match (facts.root_task_state, facts.last_turn_state) {
        (Some(TaskState::Failed), _) => (
            Some(ConversationMarker::Failed),
            Some("the plan failed".to_owned()),
        ),
        (_, Some(TurnState::TerminalFailed)) => (
            Some(ConversationMarker::Failed),
            Some("the last turn failed".to_owned()),
        ),
        (Some(TaskState::Completed), _) => (
            Some(ConversationMarker::Completed),
            Some("the plan completed".to_owned()),
        ),
        _ => (None, None),
    }
}

/// The stored title when set; otherwise the first user turn's first line, bounded; otherwise
/// "New work". The flag says whether the title was derived.
#[must_use]
pub fn conversation_title(stored: Option<&str>, first_user_text: Option<&str>) -> (String, bool) {
    if let Some(title) = stored.map(str::trim).filter(|title| !title.is_empty()) {
        return (title.to_owned(), false);
    }
    let derived = first_user_text
        .and_then(|text| text.lines().map(str::trim).find(|line| !line.is_empty()))
        .map_or_else(
            || UNTITLED_CONVERSATION.to_owned(),
            |line| {
                truncate_utf8(line, MAX_DERIVED_TITLE_BYTES)
                    .trim_end()
                    .to_owned()
            },
        );
    (derived, true)
}

/// Whether a start with this employee as the lead would succeed now, and Lazoo's words for why
/// not (information architecture section 7.5: the launcher explains the first blocking reason).
#[must_use]
pub fn lead_startability(entry: &RosterEntry) -> (bool, Option<String>) {
    let name = entry.employee.name.as_str();
    let reason = entry.availability_reason.clone();
    match entry.availability {
        EmployeeAvailability::Dormant
        | EmployeeAvailability::Idle
        | EmployeeAvailability::Suspended => (true, None),
        EmployeeAvailability::Disabled => (false, Some(format!("{name} is disabled"))),
        EmployeeAvailability::Starting => (false, Some(format!("{name} is starting"))),
        EmployeeAvailability::Offline => (
            false,
            Some(format!(
                "{name}'s harness is offline: {}",
                reason.unwrap_or_else(|| "not detected".to_owned())
            )),
        ),
        EmployeeAvailability::Unverified => (
            false,
            Some(format!(
                "{name}'s harness is unverified: {}",
                reason.unwrap_or_else(|| "not authenticated or compatible".to_owned())
            )),
        ),
        EmployeeAvailability::Failed => (
            false,
            Some(format!(
                "{name}'s last session failed: {}",
                reason.unwrap_or_else(|| "see the incident".to_owned())
            )),
        ),
    }
}

pub const MAX_WORKER_LABEL_BYTES: usize = 120;

/// The terminal header's lifecycle (contract section 4.5): no session is dormant; an idle
/// session with the lead's newest turn unfinished is busy; an uncertain newest turn is unknown;
/// otherwise the session's own state.
#[must_use]
pub fn lead_lifecycle(
    session: Option<&EmployeeSessionRecord>,
    latest_lead_turn: Option<TurnState>,
) -> LeadLifecycle {
    if latest_lead_turn == Some(TurnState::Uncertain) {
        return LeadLifecycle::Unknown;
    }
    match session.map(|session| session.state) {
        None => LeadLifecycle::Dormant,
        Some(SessionLifecycleState::Starting) => LeadLifecycle::Starting,
        Some(SessionLifecycleState::Idle) => {
            if latest_lead_turn.is_some_and(turn_in_flight) {
                LeadLifecycle::Busy
            } else {
                LeadLifecycle::Idle
            }
        }
        Some(SessionLifecycleState::Stopping) => LeadLifecycle::Stopping,
        Some(SessionLifecycleState::Suspended) => LeadLifecycle::Suspended,
        Some(SessionLifecycleState::Failed) => LeadLifecycle::Failed,
    }
}

/// A turn that has not reached a terminal state.
#[must_use]
pub const fn turn_in_flight(state: TurnState) -> bool {
    matches!(
        state,
        TurnState::Recorded | TurnState::Submitting | TurnState::Accepted
    )
}

/// The objective's first non-empty line, bounded, for a worker card.
#[must_use]
pub fn worker_label(objective: &str) -> String {
    let line = objective
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .unwrap_or_default();
    truncate_utf8(line, MAX_WORKER_LABEL_BYTES)
        .trim_end()
        .to_owned()
}

/// What the worker card rule reads about one task below a root.
#[derive(Clone, Copy, Debug)]
pub struct WorkerFacts<'a> {
    pub task: &'a TaskRecord,
    /// The worker's roster entry, when the employee still exists.
    pub roster: Option<&'a RosterEntry>,
    pub dispatch: Option<&'a TaskDispatchRecord>,
    /// The state of the task's active turn, when it has one.
    pub active_turn_state: Option<TurnState>,
    /// A pending approval naming the task or its active turn.
    pub pending_approval: Option<&'a ApprovalRecord>,
    /// An unresolved incident on the task escalated to the lead or the user.
    pub blocking_incident: Option<&'a IncidentRecord>,
}

/// The dock state, first rule that holds (contract section 4.4), with Lazoo's words for it.
#[must_use]
pub fn worker_state(facts: &WorkerFacts<'_>) -> (WorkerState, String) {
    let task = facts.task;
    if task.state == TaskState::Uncertain {
        return (
            WorkerState::Unknown,
            "The outcome is uncertain: decide how the task ended".to_owned(),
        );
    }
    if let Some(approval) = facts.pending_approval {
        return (
            WorkerState::NeedsApproval,
            format!("Waiting for your approval: {}", approval.action),
        );
    }
    match task.state {
        TaskState::CancelRequested => {
            return (
                WorkerState::Interrupted,
                "Being interrupted: cancel requested".to_owned(),
            );
        }
        TaskState::Cancelled => {
            return (
                WorkerState::Interrupted,
                "Interrupted: the task was cancelled".to_owned(),
            );
        }
        TaskState::Failed => {
            return (
                WorkerState::Failed,
                format!(
                    "Failed: {}",
                    task.failure_code.as_deref().unwrap_or("see the task")
                ),
            );
        }
        TaskState::Completed => return (WorkerState::Complete, "Completed".to_owned()),
        TaskState::Proposed => {
            return (
                WorkerState::Blocked,
                "Waiting for you to accept the task".to_owned(),
            );
        }
        TaskState::Accepted | TaskState::Running | TaskState::Uncertain => {}
    }
    if let Some(entry) = facts.roster
        && matches!(
            entry.availability,
            EmployeeAvailability::Disabled
                | EmployeeAvailability::Offline
                | EmployeeAvailability::Unverified
                | EmployeeAvailability::Failed
        )
        && let (false, Some(reason)) = lead_startability(entry)
    {
        return (WorkerState::Blocked, format!("Blocked: {reason}"));
    }
    if let Some(incident) = facts.blocking_incident {
        let whom = if incident.escalation == IncidentEscalation::Lead {
            "the lead"
        } else {
            "you"
        };
        return (
            WorkerState::Blocked,
            format!("Blocked: {} (escalated to {whom})", incident.title),
        );
    }
    if task.state == TaskState::Running {
        return match facts.active_turn_state {
            Some(TurnState::Accepted) => (
                WorkerState::Running,
                "Running: turn accepted by the harness".to_owned(),
            ),
            Some(TurnState::Submitting) => {
                (WorkerState::Running, "Running: turn submitting".to_owned())
            }
            _ => (WorkerState::Waiting, "Waiting between turns".to_owned()),
        };
    }
    let reason = match facts.dispatch.map(|dispatch| dispatch.state) {
        None => "Waiting to be dispatched when the worker is free",
        Some(TaskDispatchState::Preparing) => "Preparing the workspace",
        Some(TaskDispatchState::WorkspaceReady) => "Starting the worker's session",
        Some(TaskDispatchState::TurnRecorded) => "Waiting for the first turn to start",
        Some(
            TaskDispatchState::Failed | TaskDispatchState::Cancelled | TaskDispatchState::Uncertain,
        ) => "Waiting after an unfinished dispatch",
    };
    (WorkerState::Waiting, reason.to_owned())
}
