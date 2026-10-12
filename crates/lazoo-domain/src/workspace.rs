//! The desktop workspace read models (sidebar rows and the launcher), derived from rows that
//! stay authoritative. Nothing here is stored: the rules below say what a conversation's
//! marker is, what its title is, and whether an employee can be started as the lead right now.

use lazoo_protocol::{
    ConversationMarker, EmployeeAvailability, RosterEntry, TaskState, TurnState, WorkerCounts,
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
