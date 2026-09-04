use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

use crate::{AuthorityMode, MessageEndpoint, MessageRecord, MessageReference, MessageSender};

pub const TASK_CONTRACT_V1: u32 = 1;

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum TaskRole {
    Lead,
    Worker,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum TaskState {
    Proposed,
    Accepted,
    Running,
    CancelRequested,
    Completed,
    Failed,
    Cancelled,
    Uncertain,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum TaskEvidenceKind {
    Proposed,
    Accepted,
    Started,
    Progress,
    Completed,
    Failed,
    CancelRequested,
    Cancelled,
    Uncertain,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum TaskReportKind {
    Progress,
    Completed,
    Failed,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum TaskResolution {
    Completed,
    Failed,
    Cancelled,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct TaskRecord {
    pub sequence: u64,
    pub version: u32,
    pub id: Uuid,
    pub root_task_id: Uuid,
    pub parent_task_id: Option<Uuid>,
    pub depth: u32,
    pub project_id: Option<Uuid>,
    pub conversation_id: Uuid,
    pub creation_turn_id: Option<Uuid>,
    pub delegation_message_id: Option<Uuid>,
    pub creator: MessageSender,
    pub assigned_employee_id: Uuid,
    pub role: TaskRole,
    pub objective: String,
    pub references: Vec<MessageReference>,
    pub requested_authority: AuthorityMode,
    pub effective_authority: AuthorityMode,
    pub employee_configuration_revision: u64,
    pub roster_revision: u64,
    pub state: TaskState,
    pub revision: u64,
    pub active_turn_id: Option<Uuid>,
    pub active_session_id: Option<Uuid>,
    pub failure_code: Option<String>,
    pub failure_message: Option<String>,
    pub created_at: String,
    pub updated_at: String,
    pub terminal_at: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct TaskEvidence {
    pub sequence: u64,
    pub id: Uuid,
    pub task_id: Uuid,
    pub task_revision: u64,
    pub kind: TaskEvidenceKind,
    pub actor: MessageSender,
    pub message_id: Option<Uuid>,
    pub turn_id: Option<Uuid>,
    pub session_id: Option<Uuid>,
    pub detail: Value,
    pub created_at: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct TaskSummary {
    pub sequence: u64,
    pub id: Uuid,
    pub root_task_id: Uuid,
    pub parent_task_id: Option<Uuid>,
    pub depth: u32,
    pub project_id: Option<Uuid>,
    pub conversation_id: Uuid,
    pub assigned_employee_id: Uuid,
    pub role: TaskRole,
    pub objective: String,
    pub requested_authority: AuthorityMode,
    pub effective_authority: AuthorityMode,
    pub state: TaskState,
    pub revision: u64,
    pub created_at: String,
    pub updated_at: String,
    pub terminal_at: Option<String>,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct TaskDetail {
    pub task: TaskRecord,
    pub children: Vec<TaskSummary>,
    pub evidence: Vec<TaskEvidence>,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct TaskMutationResult {
    pub task: TaskRecord,
    pub delegation_message: Option<MessageRecord>,
    pub event_sequence: Option<u64>,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct TaskReconcileResult {
    pub tasks: Vec<TaskRecord>,
    pub event_sequence: Option<u64>,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct DelegateTaskParams {
    pub parent_task_id: Uuid,
    pub actor: MessageEndpoint,
    pub assigned_employee_id: Uuid,
    pub objective: String,
    #[serde(default)]
    pub references: Vec<MessageReference>,
    pub requested_authority: Option<AuthorityMode>,
    pub roster_revision: u64,
    pub idempotency_key: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct AcceptTaskParams {
    pub id: Uuid,
    pub expected_revision: u64,
    pub idempotency_key: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct StartTaskParams {
    pub id: Uuid,
    pub expected_revision: u64,
    pub actor: MessageEndpoint,
    pub turn_id: Uuid,
    #[serde(default = "empty_object")]
    pub evidence: Value,
    pub idempotency_key: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct ReportTaskParams {
    pub id: Uuid,
    pub expected_revision: u64,
    pub actor: MessageEndpoint,
    pub kind: TaskReportKind,
    #[serde(default = "empty_object")]
    pub evidence: Value,
    pub failure_code: Option<String>,
    pub failure_message: Option<String>,
    pub idempotency_key: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct CancelTaskParams {
    pub id: Uuid,
    pub expected_revision: u64,
    pub actor: MessageSender,
    #[serde(default = "empty_object")]
    pub evidence: Value,
    pub idempotency_key: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct ResolveTaskParams {
    pub id: Uuid,
    pub expected_revision: u64,
    pub resolution: TaskResolution,
    #[serde(default = "empty_object")]
    pub evidence: Value,
    pub failure_code: Option<String>,
    pub failure_message: Option<String>,
    pub idempotency_key: String,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct TaskListParams {
    #[serde(default)]
    pub after_sequence: u64,
    #[serde(default = "default_list_limit")]
    pub limit: u32,
    pub root_task_id: Option<Uuid>,
    pub parent_task_id: Option<Uuid>,
    pub conversation_id: Option<Uuid>,
    pub project_id: Option<Uuid>,
    pub assigned_employee_id: Option<Uuid>,
    pub state: Option<TaskState>,
}

impl Default for TaskListParams {
    fn default() -> Self {
        Self {
            after_sequence: 0,
            limit: default_list_limit(),
            root_task_id: None,
            parent_task_id: None,
            conversation_id: None,
            project_id: None,
            assigned_employee_id: None,
            state: None,
        }
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct TaskShowParams {
    pub id: Uuid,
}

fn empty_object() -> Value {
    Value::Object(serde_json::Map::new())
}

const fn default_list_limit() -> u32 {
    100
}
