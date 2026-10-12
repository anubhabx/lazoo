use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

pub const INBOX_CONTRACT_V1: u32 = 1;

/// The desktop Inbox filters from the information architecture. `resolved` is the archive.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum InboxFilter {
    All,
    Approvals,
    Permissions,
    Incidents,
    Resolved,
}

/// What an item is. `permission` is reserved for provider-native prompts that stay inside a
/// harness terminal; nothing produces it until terminal passthrough exists.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum InboxKind {
    Approval,
    TaskProposal,
    TaskUncertain,
    Incident,
    Permission,
}

/// Whether the item counts toward the badge (`action-required`), is visible but someone else's
/// to act on (`watching`), was acknowledged by the user, or left the unresolved list.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum InboxAttention {
    ActionRequired,
    Watching,
    Acknowledged,
    Resolved,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum InboxSeverity {
    Priority,
    Normal,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum InboxDeadlineKind {
    /// An approval expires at this time.
    Expires,
    /// A lead's remediation window closes at this time.
    Remediation,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct InboxDeadline {
    pub kind: InboxDeadlineKind,
    pub at: String,
}

/// The deep link: `projectId` selects the space (null is Home), `conversationId` opens the
/// conversation, `taskId` or `employeeId` focuses the worker.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct InboxScope {
    pub project_id: Option<Uuid>,
    pub conversation_id: Option<Uuid>,
    pub employee_id: Option<Uuid>,
    pub employee_name: Option<String>,
    pub provider: Option<String>,
    pub task_id: Option<Uuid>,
    pub turn_id: Option<Uuid>,
    pub session_id: Option<Uuid>,
}

/// One action that applies to an item now, as data naming an existing revision-checked RPC.
/// The caller adds a fresh `idempotencyKey` and sends exactly `params`.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct InboxAction {
    pub action: String,
    pub label: String,
    pub method: String,
    pub params: Value,
}

/// One derived Inbox row. `id` and `revision` are the source row's own, so every action's
/// `expectedRevision` matches what the user saw.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct InboxItem {
    pub kind: InboxKind,
    pub id: Uuid,
    pub revision: u64,
    pub attention: InboxAttention,
    pub severity: InboxSeverity,
    pub subtype: String,
    pub title: String,
    pub required_action: String,
    /// The source row's own state, unrenamed.
    pub state: String,
    pub scope: InboxScope,
    pub created_at: String,
    pub updated_at: String,
    pub deadline: Option<InboxDeadline>,
    pub detail: Value,
    pub actions: Vec<InboxAction>,
}

/// `unresolved` is the rail badge: items with `attention: action-required`. `approvals`
/// includes task proposals; `watching` is never on the badge.
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct InboxCounts {
    pub unresolved: u64,
    pub approvals: u64,
    pub permissions: u64,
    pub incidents: u64,
    pub watching: u64,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct InboxListParams {
    pub filter: InboxFilter,
    #[serde(default = "default_inbox_limit")]
    pub limit: u32,
    /// Opaque archive cursor from a previous `nextBefore`; only the `resolved` filter pages.
    pub before: Option<String>,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct InboxListResult {
    pub version: u32,
    pub items: Vec<InboxItem>,
    pub counts: InboxCounts,
    /// The event sequence the result reflects; subscribe from here.
    pub event_high_water: u64,
    pub truncated: bool,
    pub next_before: Option<String>,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct InboxCountsResult {
    pub version: u32,
    pub counts: InboxCounts,
    pub event_high_water: u64,
}

const fn default_inbox_limit() -> u32 {
    200
}
