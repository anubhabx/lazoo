//! The desktop workspace read models (`docs/superpowers/specs/2026-10-13-desktop-workspace-
//! contract-design.md`): `workspace.conversations` (one space's history rows with their
//! markers and the remembered lead), `workspace.launcher` (who can lead right now and why not,
//! with the start request as data), and `workspace.conversation` (the lead header, the worker
//! dock, the conversation's own Inbox items, and its newest turns). Derived on every call;
//! nothing is stored.

use std::collections::BTreeMap;

use lazoo_domain::{
    ConversationFacts, InboxCursor, WorkerFacts, conversation_marker, conversation_title,
    lead_actions, lead_lifecycle, lead_startability, turn_in_flight, validate_workspace_limit,
    worker_actions, worker_label, worker_state,
};
use lazoo_protocol::{
    AuthorityMode, ConversationRow, InboxAttention, IncidentEscalation, IncidentState,
    LeadCandidate, LeadHeader, LeadSessionSummary, RosterEntry, RosterSnapshot, RpcError,
    WORKSPACE_CONTRACT_V1, WorkerCard, WorkerSessionSummary, WorkerWorkspace,
    WorkspaceConversationParams, WorkspaceConversationResult, WorkspaceConversationsParams,
    WorkspaceConversationsResult, WorkspaceLauncherParams, WorkspaceLauncherResult, WorkspaceScope,
    WorkspaceStartAction,
};
use lazoo_store::{ConversationWorkspace, InboxSources, WorkerRow};
use serde_json::{Value, json};
use uuid::Uuid;

use super::{App, inbox::unresolved_items, store_error, to_value};

impl App {
    pub(super) fn workspace_conversations(&self, value: &Value) -> Result<Value, RpcError> {
        let params: WorkspaceConversationsParams = serde_json::from_value(value.clone())
            .map_err(|error| workspace_invalid(error.to_string()))?;
        validate_workspace_limit(params.limit)
            .map_err(|error| workspace_invalid(error.to_string()))?;
        self.require_project(params.project_id)?;
        let before = params
            .before
            .as_deref()
            .map(InboxCursor::parse)
            .transpose()
            .map_err(|error| workspace_invalid(error.to_string()))?;
        let (page, event_high_water) = self
            .store
            .space_conversations(
                params.project_id,
                before
                    .as_ref()
                    .map(|cursor| (cursor.updated_at.as_str(), cursor.id)),
                params.limit,
            )
            .map_err(store_error)?;
        let remembered_lead_id = self.remembered_lead(params.project_id)?;
        let sources = self.store.inbox_sources().map_err(store_error)?;
        let mut unresolved: BTreeMap<Uuid, u64> = BTreeMap::new();
        for item in unresolved_items(&sources) {
            if item.attention == InboxAttention::ActionRequired
                && let Some(conversation_id) = item.scope.conversation_id
            {
                *unresolved.entry(conversation_id).or_default() += 1;
            }
        }
        let employees = &sources.employees;
        let limit = params.limit as usize;
        let truncated = page.len() > limit;
        let conversations: Vec<ConversationRow> = page
            .into_iter()
            .take(limit)
            .map(|space| {
                let conversation = space.conversation;
                let (title, title_derived) = conversation_title(
                    conversation.title.as_deref(),
                    space.first_user_text.as_deref(),
                );
                let facts = ConversationFacts {
                    unresolved: unresolved.get(&conversation.id).copied().unwrap_or(0),
                    active_turns: space.active_turns,
                    workers: space.workers,
                    root_task_state: space.root_task_state,
                    last_turn_state: space.last_turn_state,
                };
                let (marker, marker_reason) = conversation_marker(&facts);
                let lead = employees.get(&conversation.lead_employee_id);
                ConversationRow {
                    id: conversation.id,
                    revision: conversation.revision,
                    title,
                    title_derived,
                    lead_employee_id: conversation.lead_employee_id,
                    lead_name: lead.map(|record| record.name.clone()),
                    provider: lead.map(|record| record.provider.clone()),
                    marker,
                    marker_reason,
                    unresolved: facts.unresolved,
                    workers: space.workers,
                    root_task_state: space.root_task_state,
                    last_turn_state: space.last_turn_state,
                    last_result_at: space.last_result_at,
                    created_at: conversation.created_at,
                    updated_at: conversation.updated_at,
                }
            })
            .collect();
        let next_before = truncated
            .then(|| {
                conversations.last().map(|row| {
                    InboxCursor {
                        updated_at: row.updated_at.clone(),
                        id: row.id,
                    }
                    .render()
                })
            })
            .flatten();
        Ok(to_value(WorkspaceConversationsResult {
            version: WORKSPACE_CONTRACT_V1,
            project_id: params.project_id,
            conversations,
            remembered_lead_id,
            // The lowest mark any read reflects: a redundant re-read is harmless, a missed one is
            // not.
            event_high_water: event_high_water.min(sources.event_high_water),
            truncated,
            next_before,
        }))
    }

    pub(super) fn workspace_launcher(&self, value: &Value) -> Result<Value, RpcError> {
        let params: WorkspaceLauncherParams = serde_json::from_value(value.clone())
            .map_err(|error| workspace_invalid(error.to_string()))?;
        let project = self.require_project(params.project_id)?;
        let roster = self.store.roster_snapshot().map_err(store_error)?;
        let mut leads: Vec<LeadCandidate> = roster
            .entries
            .iter()
            .map(|entry| {
                let (startable, blocking_reason) = lead_startability(entry);
                LeadCandidate {
                    employee_id: entry.employee.id,
                    name: entry.employee.name.clone(),
                    provider: entry.employee.provider.clone(),
                    role: entry.employee.role.clone(),
                    model: entry.employee.model.clone(),
                    reasoning_effort: entry.employee.reasoning_effort.clone(),
                    default_authority: entry.employee.authority_mode,
                    checkout_policy: entry.employee.checkout_policy,
                    availability: entry.availability,
                    availability_reason: entry.availability_reason.clone(),
                    startable,
                    blocking_reason,
                    configuration_revision: entry.employee.configuration_revision,
                }
            })
            .collect();
        leads.sort_by(|left, right| {
            left.name
                .to_lowercase()
                .cmp(&right.name.to_lowercase())
                .then_with(|| left.employee_id.cmp(&right.employee_id))
        });
        let remembered_lead_id = self.remembered_lead(params.project_id)?;
        let remembered_lead_blocking_reason = remembered_lead_id.map(|id| {
            leads
                .iter()
                .find(|lead| lead.employee_id == id)
                .map_or_else(
                    || Some("the remembered lead was removed".to_owned()),
                    |lead| lead.blocking_reason.clone(),
                )
        });
        let event_high_water = self.store.status().map_err(store_error)?.event_high_water;
        Ok(to_value(WorkspaceLauncherResult {
            version: WORKSPACE_CONTRACT_V1,
            project,
            roster_revision: roster.revision,
            leads,
            remembered_lead_id,
            remembered_lead_blocking_reason: remembered_lead_blocking_reason.flatten(),
            authority_modes: vec![
                AuthorityMode::Ask,
                AuthorityMode::SelfDecision,
                AuthorityMode::FullAutonomy,
            ],
            start: vec![
                WorkspaceStartAction {
                    method: "conversation.create".to_owned(),
                    params: json!({
                        "projectId": params.project_id,
                        "leadEmployeeId": null,
                        "title": null,
                    }),
                    fill: vec!["leadEmployeeId".to_owned(), "idempotencyKey".to_owned()],
                },
                WorkspaceStartAction {
                    method: "turn.submit".to_owned(),
                    params: json!({
                        "conversationId": null,
                        "author": {"kind": "user"},
                        "recipient": null,
                        "authorityMode": null,
                        "text": null,
                        "activateIfNeeded": true,
                        "maxAttempts": 3,
                    }),
                    fill: vec![
                        "conversationId".to_owned(),
                        "authorityMode".to_owned(),
                        "text".to_owned(),
                        "idempotencyKey".to_owned(),
                    ],
                },
            ],
            event_high_water,
        }))
    }

    /// Home has no project; a project space must exist.
    fn require_project(
        &self,
        project_id: Option<Uuid>,
    ) -> Result<Option<lazoo_protocol::ProjectRecord>, RpcError> {
        let Some(project_id) = project_id else {
            return Ok(None);
        };
        self.store
            .list_projects()
            .map_err(store_error)?
            .into_iter()
            .find(|project| project.id == project_id)
            .map(Some)
            .ok_or_else(|| RpcError {
                code: "LZ-WORKSPACE-PROJECT-NOT-FOUND".to_owned(),
                message: "the project is not registered".to_owned(),
                data: Some(json!({"projectId": project_id})),
            })
    }

    /// The lead of the space's newest conversation.
    fn remembered_lead(&self, project_id: Option<Uuid>) -> Result<Option<Uuid>, RpcError> {
        let (newest, _) = self
            .store
            .space_conversations(project_id, None, 1)
            .map_err(store_error)?;
        Ok(newest
            .first()
            .map(|space| space.conversation.lead_employee_id))
    }
}

fn workspace_invalid(message: impl Into<String>) -> RpcError {
    RpcError {
        code: "LZ-WORKSPACE-INVALID".to_owned(),
        message: message.into(),
        data: None,
    }
}

impl App {
    /// Phase 2 of the workspace contract: the lead header, the worker dock, the conversation's
    /// own Inbox items, and its newest turns, derived on every call.
    pub(super) fn workspace_conversation(&self, value: &Value) -> Result<Value, RpcError> {
        let params: WorkspaceConversationParams = serde_json::from_value(value.clone())
            .map_err(|error| workspace_invalid(error.to_string()))?;
        validate_workspace_limit(params.turn_limit)
            .map_err(|error| workspace_invalid(error.to_string()))?;
        let workspace = self
            .store
            .conversation_workspace(params.id, params.turn_limit)
            .map_err(conversation_store_error)?;
        let conversation = &workspace.conversation;
        let project = self.require_project(conversation.project_id)?;
        let roster = self.store.roster_snapshot().map_err(store_error)?;
        let sources = self.store.inbox_sources().map_err(store_error)?;
        let lead_entry = roster_entry(&roster, conversation.lead_employee_id)
            .ok_or_else(|| workspace_invalid("the lead employee no longer exists"))?;
        let (title, title_derived) = conversation_title(
            conversation.title.as_deref(),
            workspace.first_user_text.as_deref(),
        );
        let lead = self.lead_header(lead_entry, &workspace);
        let workers = workspace
            .workers
            .iter()
            .map(|row| worker_card(row, &roster, &sources))
            .collect();
        let attention = unresolved_items(&sources)
            .into_iter()
            .filter(|item| item.scope.conversation_id == Some(conversation.id))
            .collect();
        Ok(to_value(WorkspaceConversationResult {
            version: WORKSPACE_CONTRACT_V1,
            scope: WorkspaceScope {
                project_id: conversation.project_id,
                project_name: project.as_ref().map(|record| record.display_name.clone()),
                canonical_path: project.as_ref().map(|record| record.canonical_path.clone()),
                checkout_mode: project.as_ref().map(|record| record.checkout_mode),
            },
            conversation: workspace.conversation.clone(),
            title,
            title_derived,
            lead,
            root_task: workspace.root_task,
            turns: workspace.turns,
            turns_truncated: workspace.turns_truncated,
            workers,
            attention,
            event_high_water: workspace.event_high_water.min(sources.event_high_water),
        }))
    }

    /// The terminal header's identity line from the lead's roster entry and the conversation's
    /// newest root task and lead turn.
    fn lead_header(&self, entry: &RosterEntry, workspace: &ConversationWorkspace) -> LeadHeader {
        LeadHeader {
            employee_id: entry.employee.id,
            name: entry.employee.name.clone(),
            role: entry.employee.role.clone(),
            provider: entry.employee.provider.clone(),
            model: entry.employee.model.clone(),
            reasoning_effort: entry.employee.reasoning_effort.clone(),
            availability: entry.availability,
            availability_reason: entry.availability_reason.clone(),
            authority: workspace
                .root_task
                .as_ref()
                .map(|task| task.effective_authority),
            session: entry.session.as_ref().map(|session| LeadSessionSummary {
                id: session.id,
                state: session.state,
                native_host_pid: session.native_host_pid,
                working_directory: Some(
                    self.session_directory(session.id)
                        .unwrap_or_else(|_| self.root.session_working_dir(session.id))
                        .to_string_lossy()
                        .into_owned(),
                ),
            }),
            lifecycle: lead_lifecycle(
                entry.session.as_ref(),
                workspace.latest_lead_turn.map(|(_, state, _)| state),
            ),
            active_turn_id: workspace
                .latest_lead_turn
                .filter(|(_, state, _)| turn_in_flight(*state))
                .map(|(id, _, _)| id),
            actions: lead_actions(
                &workspace.conversation,
                entry.session.as_ref(),
                workspace.latest_lead_turn,
            )
            .into_iter()
            // A retry is delivered on a session this daemon owns; one left by a prior boot
            // is reconciled away, not retried on.
            .filter(|action| {
                action.action != "retry-turn"
                    || entry
                        .session
                        .as_ref()
                        .is_some_and(|session| session.owner_boot_id == Some(self.boot_id))
            })
            .collect(),
        }
    }
}

fn roster_entry(roster: &RosterSnapshot, employee_id: Uuid) -> Option<&RosterEntry> {
    roster
        .entries
        .iter()
        .find(|entry| entry.employee.id == employee_id)
}

/// One dock card from a worker's rows, the roster, and the Inbox sources (pending approvals
/// and escalated incidents).
fn worker_card(row: &WorkerRow, roster: &RosterSnapshot, sources: &InboxSources) -> WorkerCard {
    let task = &row.task;
    let employee = sources.employees.get(&task.assigned_employee_id);
    let pending_approval = sources.approvals.iter().find(|approval| {
        approval.task_id == Some(task.id)
            || task
                .active_turn_id
                .is_some_and(|turn_id| approval.turn_id == turn_id)
    });
    let blocking_incident =
        sources
            .incidents
            .iter()
            .map(|(incident, _)| incident)
            .find(|incident| {
                incident.task_id == Some(task.id)
                    && incident.state != IncidentState::Resolved
                    && matches!(
                        incident.escalation,
                        IncidentEscalation::Lead | IncidentEscalation::User
                    )
            });
    let (state, state_reason) = worker_state(&WorkerFacts {
        task,
        roster: roster_entry(roster, task.assigned_employee_id),
        dispatch: row.dispatch.as_ref(),
        active_turn_state: row.active_turn_state,
        pending_approval,
        blocking_incident,
    });
    WorkerCard {
        task_id: task.id,
        revision: task.revision,
        root_task_id: task.root_task_id,
        parent_task_id: task.parent_task_id,
        depth: task.depth,
        employee_id: task.assigned_employee_id,
        name: employee.map(|record| record.name.clone()),
        provider: employee.map(|record| record.provider.clone()),
        model: employee.and_then(|record| record.model.clone()),
        label: worker_label(&task.objective),
        state,
        state_reason,
        task_state: task.state,
        effective_authority: task.effective_authority,
        session: task.active_session_id.and_then(|id| {
            row.session_state
                .map(|state| WorkerSessionSummary { id, state })
        }),
        active_turn_id: task.active_turn_id,
        workspace: row.dispatch.as_ref().map(|dispatch| WorkerWorkspace {
            kind: dispatch.workspace_kind,
            path: dispatch.workspace_path.clone(),
            branch_name: dispatch.branch_name.clone(),
            base_commit: dispatch.base_commit.clone(),
        }),
        failure_code: task.failure_code.clone(),
        failure_message: task.failure_message.clone(),
        created_at: task.created_at.clone(),
        updated_at: task.updated_at.clone(),
        terminal_at: task.terminal_at.clone(),
        actions: worker_actions(task, pending_approval, row.active_descendants),
    }
}

fn conversation_store_error(error: lazoo_store::StoreError) -> RpcError {
    match error {
        lazoo_store::StoreError::ConversationNotFound { conversation_id } => RpcError {
            code: "LZ-CONVERSATION-NOT-FOUND".to_owned(),
            message: error.to_string(),
            data: Some(json!({"conversationId": conversation_id})),
        },
        other => store_error(other),
    }
}
