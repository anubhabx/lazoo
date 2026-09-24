//! Lead notices: the text a lead receives when a child task it delegated reaches a terminal
//! state, or when an incident about its task tree is promoted to priority.
//!
//! A notice is injected as the text of a provider turn, so it carries only the envelope and
//! bounded evidence; the task, message, and incident ledgers stay authoritative.

use std::fmt::Write;

use lazoo_protocol::{IncidentPriority, TaskState};
use uuid::Uuid;

/// Bytes of a child's native output carried into the lead's turn; the turn ledger keeps the rest.
pub const MAX_NOTICE_OUTPUT_BYTES: usize = 16 * 1024;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TaskResultNotice<'a> {
    pub child_task_id: Uuid,
    pub child_employee_name: &'a str,
    pub state: TaskState,
    pub objective: &'a str,
    pub failure_code: Option<&'a str>,
    pub failure_message: Option<&'a str>,
    pub output: Option<&'a str>,
    pub parent_task_id: Uuid,
    pub remaining_children: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct IncidentNotice<'a> {
    pub incident_id: Uuid,
    pub code: &'a str,
    pub title: &'a str,
    pub explanation: &'a str,
    pub next_action: &'a str,
    pub priority: IncidentPriority,
    pub occurrences: u64,
    pub task_id: Uuid,
    pub employee_name: Option<&'a str>,
}

/// Renders the turn text for a child task outcome. Output is bounded to
/// [`MAX_NOTICE_OUTPUT_BYTES`] on a UTF-8 boundary and marked when truncated.
#[must_use]
pub fn render_task_result_text(notice: &TaskResultNotice<'_>) -> String {
    let mut text = format!(
        "[Lazoo task result] Child task {} assigned to {} finished with state {}.\nObjective: {}\n",
        notice.child_task_id,
        notice.child_employee_name,
        state_name(notice.state),
        notice.objective,
    );
    if notice.failure_code.is_some() || notice.failure_message.is_some() {
        writeln!(
            text,
            "Failure: {}: {}",
            notice.failure_code.unwrap_or("LZ-TASK-FAILED"),
            notice
                .failure_message
                .unwrap_or("no failure message was recorded"),
        )
        .expect("writing to a String cannot fail");
    }
    match notice
        .output
        .map(str::trim)
        .filter(|output| !output.is_empty())
    {
        Some(output) => {
            let bounded = truncate_utf8(output, MAX_NOTICE_OUTPUT_BYTES);
            text.push_str("Output:\n");
            text.push_str(bounded);
            if bounded.len() < output.len() {
                text.push_str("\n… [output truncated; the task ledger holds the rest]");
            }
            text.push('\n');
        }
        None => text.push_str("Output: none recorded.\n"),
    }
    if notice.remaining_children == 0 {
        writeln!(
            text,
            "All child tasks of your task {} are finished.",
            notice.parent_task_id
        )
        .expect("writing to a String cannot fail");
    } else {
        writeln!(
            text,
            "{} other child task(s) of your task {} are still unfinished.",
            notice.remaining_children, notice.parent_task_id
        )
        .expect("writing to a String cannot fail");
    }
    text.push_str(
        "This is a Lazoo result notice, not a user message. Continue your task: delegate \
         follow-up work with task_delegate, wait with task_wait, or finish with task_report.",
    );
    text
}

/// Renders the turn text for a promoted incident about the lead's task tree.
#[must_use]
pub fn render_incident_notice_text(notice: &IncidentNotice<'_>) -> String {
    let priority = match notice.priority {
        IncidentPriority::Priority => "Priority",
        IncidentPriority::Normal => "Normal",
    };
    let subject = notice
        .employee_name
        .map_or_else(String::new, |name| format!(" for {name}"));
    format!(
        "[Lazoo incident] {priority} incident {} on task {}{subject}: {}\n{}\nOccurrences: {}\n\
         Recommended next action: {}\nLazoo will not retry, restart, or change authority on its \
         own. If the work cannot continue, report your task as failed with task_report or \
         delegate it elsewhere with task_delegate; the user sees this incident too. \
         Incident id: {}.",
        notice.code,
        notice.task_id,
        notice.title,
        notice.explanation,
        notice.occurrences,
        notice.next_action,
        notice.incident_id,
    )
}

const fn state_name(state: TaskState) -> &'static str {
    match state {
        TaskState::Proposed => "proposed",
        TaskState::Accepted => "accepted",
        TaskState::Running => "running",
        TaskState::CancelRequested => "cancel-requested",
        TaskState::Completed => "completed",
        TaskState::Failed => "failed",
        TaskState::Cancelled => "cancelled",
        TaskState::Uncertain => "uncertain",
    }
}

fn truncate_utf8(value: &str, maximum: usize) -> &str {
    if value.len() <= maximum {
        return value;
    }
    let mut end = maximum;
    while !value.is_char_boundary(end) {
        end -= 1;
    }
    &value[..end]
}
