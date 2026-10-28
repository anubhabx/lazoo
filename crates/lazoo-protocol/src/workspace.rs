use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

use crate::{
    AuthorityMode, CheckoutMode, ConversationRecord, ConversationTurnSummary, EmployeeAvailability,
    EmployeeCheckoutPolicy, InboxAction, InboxItem, ProjectRecord, SessionLifecycleState,
    TaskState, TaskSummary, TaskWorkspaceKind, TurnState,
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
    /// Authority modes that a new session for this lead can actually serve. This is specific to
    /// the provider and the employee's configured authority, not the launcher's global
    /// vocabulary.
    pub authority_modes: Vec<AuthorityMode>,
    /// Why no authority mode is available, when `authority_modes` is empty.
    pub authority_reason: Option<String>,
    /// Whether the session receives Lazoo's collaboration tools. A conversational session can
    /// be startable without being able to orchestrate workers.
    pub collaboration_tools_available: bool,
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

/// The lead terminal header's truthful lifecycle (information architecture section 8.1),
/// from the lead's latest session and its latest turn.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum LeadLifecycle {
    Dormant,
    Starting,
    Busy,
    Idle,
    Stopping,
    Suspended,
    Failed,
    Unknown,
}

/// The worker dock's eight states (information architecture section 8.2), derived from
/// durable task, dispatch, session, approval, and incident rows.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum WorkerState {
    Running,
    Waiting,
    NeedsApproval,
    Blocked,
    Failed,
    Complete,
    Interrupted,
    Unknown,
}

/// The lead's latest session as the header shows it; the working directory is the private
/// per-session directory the daemon starts the lead in.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct LeadSessionSummary {
    pub id: Uuid,
    pub state: SessionLifecycleState,
    pub native_host_pid: Option<u32>,
    pub working_directory: Option<String>,
}

/// The terminal header's identity line: who leads, on what, under which authority, and the
/// truthful lifecycle of the lead's session.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct LeadHeader {
    pub employee_id: Uuid,
    pub name: String,
    pub role: String,
    pub provider: String,
    pub model: Option<String>,
    pub reasoning_effort: Option<String>,
    pub availability: EmployeeAvailability,
    pub availability_reason: Option<String>,
    /// The newest root task's effective authority; null before the first user turn.
    pub authority: Option<AuthorityMode>,
    pub session: Option<LeadSessionSummary>,
    pub lifecycle: LeadLifecycle,
    /// The lead's newest turn while it is unfinished.
    pub active_turn_id: Option<Uuid>,
    /// What the header offers now, as data naming an existing revision-checked RPC; the
    /// caller adds a fresh `idempotencyKey` (and the new `title` for `rename`).
    pub actions: Vec<InboxAction>,
}

/// Where the conversation lives: a registered project, or Home.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct WorkspaceScope {
    pub project_id: Option<Uuid>,
    pub project_name: Option<String>,
    pub canonical_path: Option<String>,
    pub checkout_mode: Option<CheckoutMode>,
}

/// A worker's dispatched workspace, from its dispatch record.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct WorkerWorkspace {
    pub kind: Option<TaskWorkspaceKind>,
    pub path: Option<String>,
    pub branch_name: Option<String>,
    pub base_commit: Option<String>,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct WorkerSessionSummary {
    pub id: Uuid,
    pub state: SessionLifecycleState,
}

/// One worker dock card: a task below a root with its state in the dock's words. Completed
/// cards stay in the list; the desktop dismisses them locally.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct WorkerCard {
    pub task_id: Uuid,
    pub revision: u64,
    pub root_task_id: Uuid,
    pub parent_task_id: Option<Uuid>,
    pub depth: u32,
    pub employee_id: Uuid,
    pub name: Option<String>,
    pub provider: Option<String>,
    pub model: Option<String>,
    /// The objective's first line, bounded.
    pub label: String,
    pub state: WorkerState,
    pub state_reason: String,
    /// The row's own state, unrenamed.
    pub task_state: TaskState,
    pub effective_authority: AuthorityMode,
    pub session: Option<WorkerSessionSummary>,
    pub active_turn_id: Option<Uuid>,
    pub workspace: Option<WorkerWorkspace>,
    pub failure_code: Option<String>,
    pub failure_message: Option<String>,
    pub created_at: String,
    pub updated_at: String,
    pub terminal_at: Option<String>,
    /// What the card offers now, as data naming an existing revision-checked RPC; the caller
    /// adds a fresh `idempotencyKey`.
    pub actions: Vec<InboxAction>,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct WorkspaceConversationParams {
    pub id: Uuid,
    /// How many of the newest turns to return (1 to 500).
    #[serde(default = "default_turn_limit")]
    pub turn_limit: u32,
    /// Exclusive sequence cursor for older history; absent starts at the newest turn.
    #[serde(default)]
    pub before_turn_sequence: Option<u64>,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct WorkspaceConversationResult {
    pub version: u32,
    pub conversation: ConversationRecord,
    pub title: String,
    pub title_derived: bool,
    pub scope: WorkspaceScope,
    pub lead: LeadHeader,
    /// The newest root task.
    pub root_task: Option<TaskSummary>,
    /// The newest `turnLimit` turns, oldest first.
    pub turns: Vec<ConversationTurnSummary>,
    pub turns_truncated: bool,
    /// Exclusive cursor for the next older history page, or null at the beginning.
    pub next_before_turn_sequence: Option<u64>,
    /// One card per task below a root, oldest first.
    pub workers: Vec<WorkerCard>,
    /// The conversation's own unresolved Inbox items, in Inbox order.
    pub attention: Vec<InboxItem>,
    pub event_high_water: u64,
}

const fn default_turn_limit() -> u32 {
    50
}
