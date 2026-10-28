//! The desktop Inbox read model, derived from approval, task, and incident rows that stay
//! authoritative. Nothing here is stored: the rules below say which rows are Inbox items, what
//! the user must do about them, which ones count toward the badge, and how they order.

use std::collections::BTreeMap;

use lazoo_protocol::{
    ApprovalRecord, ApprovalState, EmployeeRecord, InboxAction, InboxAttention, InboxCounts,
    InboxDeadline, InboxDeadlineKind, InboxFilter, InboxItem, InboxKind, InboxScope, InboxSeverity,
    IncidentEscalation, IncidentPriority, IncidentRecord, IncidentState, TaskRecord, TaskState,
};
use serde::Serialize;
use serde_json::{Value, json};
use thiserror::Error;
use uuid::Uuid;

use crate::delivery::truncate_utf8;

pub const MAX_INBOX_LIMIT: u32 = 500;
pub const MAX_INBOX_TITLE_BYTES: usize = 256;

#[derive(Clone, Debug, Eq, Error, PartialEq)]
pub enum InboxValidationError {
    #[error("inbox limit must be between 1 and 500")]
    Limit,
    #[error("inbox cursor must be the nextBefore of a previous resolved page")]
    Cursor,
    #[error("before applies only to the resolved filter")]
    CursorFilter,
}

/// Employees by id, so an item's scope can name the worker without another read.
pub type InboxEmployees = BTreeMap<Uuid, EmployeeRecord>;

/// Where an archive page starts: strictly before this (`updatedAt`, id) pair.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct InboxCursor {
    pub updated_at: String,
    pub id: Uuid,
}

impl InboxCursor {
    /// # Errors
    /// Returns `Cursor` when the value is not `<updatedAt>|<id>`.
    pub fn parse(value: &str) -> Result<Self, InboxValidationError> {
        let (updated_at, id) = value.rsplit_once('|').ok_or(InboxValidationError::Cursor)?;
        if updated_at.is_empty() {
            return Err(InboxValidationError::Cursor);
        }
        Ok(Self {
            updated_at: updated_at.to_owned(),
            id: id.parse().map_err(|_| InboxValidationError::Cursor)?,
        })
    }

    #[must_use]
    pub fn of(item: &InboxItem) -> Self {
        Self {
            updated_at: item.updated_at.clone(),
            id: item.id,
        }
    }

    #[must_use]
    pub fn render(&self) -> String {
        format!("{}|{}", self.updated_at, self.id)
    }
}

/// # Errors
/// Returns `Limit` outside 1 to 500.
pub const fn validate_inbox_limit(limit: u32) -> Result<(), InboxValidationError> {
    if limit == 0 || limit > MAX_INBOX_LIMIT {
        Err(InboxValidationError::Limit)
    } else {
        Ok(())
    }
}

/// A pending approval needs the user; any other state is the archive.
#[must_use]
pub fn approval_inbox_item(approval: &ApprovalRecord, employees: &InboxEmployees) -> InboxItem {
    let pending = approval.state == ApprovalState::Pending;
    let employee = employees.get(&approval.employee_id);
    InboxItem {
        kind: InboxKind::Approval,
        id: approval.id,
        revision: approval.revision,
        attention: if pending {
            InboxAttention::ActionRequired
        } else {
            InboxAttention::Resolved
        },
        severity: InboxSeverity::Priority,
        subtype: kebab(approval.kind),
        title: title(&format!("Approval requested: {}", approval.action)),
        required_action: title(&format!("Approve or decline: {}", approval.action)),
        state: kebab(approval.state),
        scope: InboxScope {
            project_id: approval.project_id,
            conversation_id: Some(approval.conversation_id),
            employee_id: Some(approval.employee_id),
            employee_name: employee.map(|record| record.name.clone()),
            provider: Some(approval.provider.clone()),
            task_id: approval.task_id,
            turn_id: Some(approval.turn_id),
            session_id: approval.session_id,
        },
        created_at: approval.created_at.clone(),
        updated_at: approval.updated_at.clone(),
        deadline: pending.then(|| InboxDeadline {
            kind: InboxDeadlineKind::Expires,
            at: approval.expires_at.clone(),
        }),
        detail: json!({
            "approvalKind": approval.kind,
            "action": approval.action,
            "fingerprint": approval.fingerprint,
            "authority": approval.authority,
            "scopeExpansion": approval.scope_expansion,
            "detail": approval.detail,
        }),
        actions: if pending {
            approval_actions(approval)
        } else {
            Vec::new()
        },
    }
}

/// `approval.decide` once each way, with the revision the user saw.
pub(crate) fn approval_actions(approval: &ApprovalRecord) -> Vec<InboxAction> {
    [("approve", "Approve once"), ("decline", "Decline")]
        .into_iter()
        .map(|(decision, label)| {
            action(
                decision,
                label,
                "approval.decide",
                json!({
                    "id": approval.id,
                    "expectedRevision": approval.revision,
                    "decision": decision,
                }),
            )
        })
        .collect()
}

/// A `proposed` task waits for `task.accept`; an `uncertain` one waits for `task.resolve`; a
/// terminal task that was once either is the archive. Anything else is not an Inbox item.
/// An uncertain task with an unfinished descendant can only be resolved as failed, so that is
/// the one resolution it offers.
#[must_use]
pub fn task_inbox_item(
    task: &TaskRecord,
    once_uncertain: bool,
    active_descendants: bool,
    employees: &InboxEmployees,
) -> Option<InboxItem> {
    let (kind, attention) = match task.state {
        TaskState::Proposed => (InboxKind::TaskProposal, InboxAttention::ActionRequired),
        TaskState::Uncertain => (InboxKind::TaskUncertain, InboxAttention::ActionRequired),
        TaskState::Completed | TaskState::Failed | TaskState::Cancelled => (
            if once_uncertain {
                InboxKind::TaskUncertain
            } else {
                InboxKind::TaskProposal
            },
            InboxAttention::Resolved,
        ),
        TaskState::Accepted | TaskState::Running | TaskState::CancelRequested => return None,
    };
    let employee = employees.get(&task.assigned_employee_id);
    let (title_text, required, subtype, severity, detail) = match kind {
        InboxKind::TaskProposal => (
            format!("Task proposed: {}", task.objective),
            "Accept or cancel the task",
            "task-proposal",
            InboxSeverity::Normal,
            json!({
                "objective": task.objective,
                "requestedAuthority": task.requested_authority,
                "effectiveAuthority": task.effective_authority,
                "parentTaskId": task.parent_task_id,
                "depth": task.depth,
                "creator": task.creator,
            }),
        ),
        _ => (
            format!("Task outcome uncertain: {}", task.objective),
            "Decide how the task ended",
            "task-uncertain",
            InboxSeverity::Priority,
            json!({
                "objective": task.objective,
                "failureCode": task.failure_code,
                "failureMessage": task.failure_message,
                "activeTurnId": task.active_turn_id,
            }),
        ),
    };
    Some(InboxItem {
        kind,
        id: task.id,
        revision: task.revision,
        attention,
        severity,
        subtype: subtype.to_owned(),
        title: title(&title_text),
        required_action: required.to_owned(),
        state: kebab(task.state),
        scope: InboxScope {
            project_id: task.project_id,
            conversation_id: Some(task.conversation_id),
            employee_id: Some(task.assigned_employee_id),
            employee_name: employee.map(|record| record.name.clone()),
            provider: employee.map(|record| record.provider.clone()),
            task_id: Some(task.id),
            turn_id: task.active_turn_id,
            session_id: task.active_session_id,
        },
        created_at: task.created_at.clone(),
        updated_at: task.updated_at.clone(),
        deadline: None,
        detail,
        actions: if attention == InboxAttention::ActionRequired {
            let mut actions = task_actions(kind, task);
            if active_descendants {
                actions.retain(|action| {
                    !matches!(
                        action.action.as_str(),
                        "cancel" | "resolve-completed" | "resolve-cancelled"
                    )
                });
            }
            actions
        } else {
            Vec::new()
        },
    })
}

/// `task.accept` or a user `task.cancel` for a proposal; one `task.resolve` per resolution for
/// an uncertain task.
pub(crate) fn task_actions(kind: InboxKind, task: &TaskRecord) -> Vec<InboxAction> {
    let with = |extra: Value| {
        let mut params = json!({"id": task.id, "expectedRevision": task.revision});
        params
            .as_object_mut()
            .expect("object")
            .extend(extra.as_object().expect("object").clone());
        params
    };
    if kind == InboxKind::TaskProposal {
        return vec![
            action("accept", "Accept", "task.accept", with(json!({}))),
            action(
                "cancel",
                "Cancel",
                "task.cancel",
                with(json!({"actor": {"kind": "user"}})),
            ),
        ];
    }
    ["completed", "failed", "cancelled"]
        .into_iter()
        .map(|resolution| {
            let mut extra = json!({"resolution": resolution});
            if resolution == "failed" {
                // `task.resolve` needs a failure; the task's own, or Lazoo's words for the
                // user's verdict.
                extra["failureCode"] =
                    json!(task.failure_code.as_deref().unwrap_or("LZ-TASK-UNCERTAIN"));
                extra["failureMessage"] = json!(
                    task.failure_message
                        .as_deref()
                        .unwrap_or("The user decided the uncertain task failed.")
                );
            }
            action(
                &format!("resolve-{resolution}"),
                &format!("Mark {resolution}"),
                "task.resolve",
                with(extra),
            )
        })
        .collect()
}

/// An open incident escalated to the user needs them; one in the lead's window is watched;
/// an acknowledged escalation is visible but not counted; a resolved or remediated escalation
/// is the archive. An incident never escalated is not an Inbox item.
#[must_use]
pub fn incident_inbox_item(
    incident: &IncidentRecord,
    conversation_id: Option<Uuid>,
    employees: &InboxEmployees,
) -> Option<InboxItem> {
    let attention = match (incident.state, incident.escalation) {
        (_, IncidentEscalation::None) => return None,
        (IncidentState::Resolved, _) | (_, IncidentEscalation::Remediated) => {
            InboxAttention::Resolved
        }
        (IncidentState::Acknowledged, _) => InboxAttention::Acknowledged,
        (IncidentState::Open, IncidentEscalation::Lead) => InboxAttention::Watching,
        (IncidentState::Open, IncidentEscalation::User) => InboxAttention::ActionRequired,
    };
    let employee = incident.employee_id.and_then(|id| employees.get(&id));
    let settle = |verb: &str, label: &str| {
        action(
            verb,
            label,
            &format!("incident.{verb}"),
            json!({"id": incident.id, "expectedRevision": incident.revision}),
        )
    };
    let actions = match attention {
        InboxAttention::ActionRequired => vec![
            settle("acknowledge", "Acknowledge"),
            settle("resolve", "Resolve"),
        ],
        InboxAttention::Acknowledged => vec![settle("resolve", "Resolve")],
        InboxAttention::Watching | InboxAttention::Resolved => Vec::new(),
    };
    Some(InboxItem {
        kind: InboxKind::Incident,
        id: incident.id,
        revision: incident.revision,
        attention,
        severity: match incident.priority {
            IncidentPriority::Priority => InboxSeverity::Priority,
            IncidentPriority::Normal => InboxSeverity::Normal,
        },
        subtype: incident.code.clone(),
        title: title(&incident.title),
        required_action: title(&incident.next_action),
        state: kebab(incident.state),
        scope: InboxScope {
            project_id: incident.project_id,
            conversation_id,
            employee_id: incident.employee_id,
            employee_name: employee.map(|record| record.name.clone()),
            provider: incident
                .provider
                .clone()
                .or_else(|| employee.map(|record| record.provider.clone())),
            task_id: incident.task_id,
            turn_id: incident.turn_id,
            session_id: incident.session_id,
        },
        created_at: incident.created_at.clone(),
        updated_at: incident.updated_at.clone(),
        deadline: (attention == InboxAttention::Watching)
            .then(|| incident.remediation_deadline_at.clone())
            .flatten()
            .map(|at| InboxDeadline {
                kind: InboxDeadlineKind::Remediation,
                at,
            }),
        detail: json!({
            "code": incident.code,
            "explanation": incident.explanation,
            "occurrences": incident.occurrences,
            "firstOccurredAt": incident.first_occurred_at,
            "lastOccurredAt": incident.last_occurred_at,
            "escalation": incident.escalation,
            "escalationReason": incident.escalation_reason,
            "remedies": incident.remedies,
            "userNotification": incident.user_notification,
            "source": incident.source,
        }),
        actions,
    })
}

/// Which filter an item belongs to. Failures stay incidents with a subtype, so `uncertain`
/// tasks sit under Incidents and proposals under Approvals.
#[must_use]
pub fn inbox_filter_matches(filter: InboxFilter, item: &InboxItem) -> bool {
    let resolved = item.attention == InboxAttention::Resolved;
    match filter {
        InboxFilter::All => !resolved,
        InboxFilter::Approvals => {
            !resolved && matches!(item.kind, InboxKind::Approval | InboxKind::TaskProposal)
        }
        InboxFilter::Permissions => !resolved && item.kind == InboxKind::Permission,
        InboxFilter::Incidents => {
            !resolved && matches!(item.kind, InboxKind::Incident | InboxKind::TaskUncertain)
        }
        InboxFilter::Resolved => resolved,
    }
}

/// The badge counts only `action-required` items; `watching` is reported beside it.
#[must_use]
pub fn inbox_counts(items: &[InboxItem]) -> InboxCounts {
    let mut counts = InboxCounts::default();
    for item in items {
        match item.attention {
            InboxAttention::ActionRequired => {
                counts.unresolved += 1;
                match item.kind {
                    InboxKind::Approval | InboxKind::TaskProposal => counts.approvals += 1,
                    InboxKind::Permission => counts.permissions += 1,
                    InboxKind::Incident | InboxKind::TaskUncertain => counts.incidents += 1,
                }
            }
            InboxAttention::Watching => counts.watching += 1,
            InboxAttention::Acknowledged | InboxAttention::Resolved => {}
        }
    }
    counts
}

/// Urgency order: counted items first, then the nearest deadline, then oldest first.
pub fn sort_unresolved_inbox(items: &mut [InboxItem]) {
    items.sort_by(|left, right| {
        attention_rank(left.attention)
            .cmp(&attention_rank(right.attention))
            .then_with(|| {
                deadline_key(left)
                    .cmp(&deadline_key(right))
                    .then_with(|| left.created_at.cmp(&right.created_at))
                    .then_with(|| left.id.cmp(&right.id))
            })
    });
}

/// Archive order: newest change first.
// ponytail: timestamps compare as strings; a millisecond and a second-precision stamp from two
// tables can swap neighbours. Parse them if the archive order ever matters that finely.
pub fn sort_inbox_archive(items: &mut [InboxItem]) {
    items.sort_by(|left, right| {
        right
            .updated_at
            .cmp(&left.updated_at)
            .then_with(|| right.id.cmp(&left.id))
    });
}

const fn attention_rank(attention: InboxAttention) -> u8 {
    match attention {
        InboxAttention::ActionRequired => 0,
        InboxAttention::Watching => 1,
        InboxAttention::Acknowledged => 2,
        InboxAttention::Resolved => 3,
    }
}

/// Items with a deadline sort before those without, soonest first.
fn deadline_key(item: &InboxItem) -> (bool, &str) {
    item.deadline
        .as_ref()
        .map_or((true, ""), |deadline| (false, deadline.at.as_str()))
}

pub(crate) fn action(action: &str, label: &str, method: &str, params: Value) -> InboxAction {
    InboxAction {
        action: action.to_owned(),
        label: label.to_owned(),
        method: method.to_owned(),
        params,
    }
}

fn title(text: &str) -> String {
    truncate_utf8(text, MAX_INBOX_TITLE_BYTES).to_owned()
}

fn kebab<T: Serialize>(value: T) -> String {
    serde_json::to_value(value)
        .ok()
        .and_then(|value| value.as_str().map(str::to_owned))
        .unwrap_or_default()
}
