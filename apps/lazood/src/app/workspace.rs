//! The desktop workspace read models, phase 1 (`docs/superpowers/specs/2026-10-13-desktop-
//! workspace-contract-design.md`): `workspace.conversations` (one space's history rows with
//! their markers and the remembered lead) and `workspace.launcher` (who can lead right now and
//! why not, with the start request as data). Derived on every call; nothing is stored.

use std::collections::BTreeMap;

use lazoo_domain::{
    ConversationFacts, InboxCursor, conversation_marker, conversation_title, lead_startability,
    validate_workspace_limit,
};
use lazoo_protocol::{
    AuthorityMode, ConversationRow, InboxAttention, LeadCandidate, RpcError, WORKSPACE_CONTRACT_V1,
    WorkspaceConversationsParams, WorkspaceConversationsResult, WorkspaceLauncherParams,
    WorkspaceLauncherResult, WorkspaceStartAction,
};
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
            event_high_water: event_high_water.max(sources.event_high_water),
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
