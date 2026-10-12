use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

use crate::{
    AuthorityMode, EmployeeAvailability, EmployeeCheckoutPolicy, ProjectRecord, TaskState,
    TurnState,
};

pub const WORKSPACE_CONTRACT_V1: u32 = 1;

/// The sidebar's state marker (information architecture section 6.3): shown only when a
/// conversation is active, needs the user, failed, or completed.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum ConversationMarker {
    NeedsAttention,
    Active,
    Failed,
    Completed,
}

/// Delegated workers in a conversation by state.
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct WorkerCounts {
    pub total: u64,
    pub running: u64,
    pub waiting: u64,
    pub failed: u64,
    pub completed: u64,
}

/// One conversation history row for a space, derived from the conversation, turn, task, and
/// Inbox ledgers on every read.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct ConversationRow {
    pub id: Uuid,
    pub revision: u64,
    pub title: String,
    pub title_derived: bool,
    pub lead_employee_id: Uuid,
    pub lead_name: Option<String>,
    pub provider: Option<String>,
    pub marker: Option<ConversationMarker>,
    pub marker_reason: Option<String>,
    /// Counted Inbox items scoped to this conversation.
    pub unresolved: u64,
    pub workers: WorkerCounts,
    pub root_task_state: Option<TaskState>,
    pub last_turn_state: Option<TurnState>,
    /// The newest terminal time across the conversation's tasks and turns; the desktop keeps
    /// its own "seen" mark against it.
    pub last_result_at: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct WorkspaceConversationsParams {
    /// `null` or absent is Home (projectless conversations).
    #[serde(default)]
    pub project_id: Option<Uuid>,
    #[serde(default = "default_workspace_limit")]
    pub limit: u32,
    /// Opaque cursor from a previous `nextBefore`.
    pub before: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct WorkspaceConversationsResult {
    pub version: u32,
    pub project_id: Option<Uuid>,
    pub conversations: Vec<ConversationRow>,
    /// The lead of the space's newest conversation (information architecture section 7.3).
    pub remembered_lead_id: Option<Uuid>,
    pub event_high_water: u64,
    pub truncated: bool,
    pub next_before: Option<String>,
}

/// One employee the launcher may offer as the lead, with the truth about whether a start
/// would succeed right now and, when it would not, Lazoo's words for why.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct LeadCandidate {
    pub employee_id: Uuid,
    pub name: String,
    pub provider: String,
    pub role: String,
    pub model: Option<String>,
    pub reasoning_effort: Option<String>,
    pub default_authority: AuthorityMode,
    pub checkout_policy: EmployeeCheckoutPolicy,
    pub availability: EmployeeAvailability,
    pub availability_reason: Option<String>,
    pub startable: bool,
    pub blocking_reason: Option<String>,
    pub configuration_revision: u64,
}

/// One request the desktop sends to start work, as data; the caller fills the fields named in
/// `fill` and adds a fresh `idempotencyKey`.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct WorkspaceStartAction {
    pub method: String,
    pub params: Value,
    pub fill: Vec<String>,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct WorkspaceLauncherParams {
    #[serde(default)]
    pub project_id: Option<Uuid>,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct WorkspaceLauncherResult {
    pub version: u32,
    pub project: Option<ProjectRecord>,
    pub roster_revision: u64,
    pub leads: Vec<LeadCandidate>,
    pub remembered_lead_id: Option<Uuid>,
    pub remembered_lead_blocking_reason: Option<String>,
    pub authority_modes: Vec<AuthorityMode>,
    pub start: Vec<WorkspaceStartAction>,
    pub event_high_water: u64,
}

const fn default_workspace_limit() -> u32 {
    200
}
