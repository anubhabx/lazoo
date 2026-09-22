use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

use crate::AuthorityMode;

pub const APPROVAL_CONTRACT_V1: u32 = 1;

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum ApprovalKind {
    CommandExecution,
    FileChange,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum ApprovalState {
    Pending,
    Approved,
    Declined,
    Expired,
    Cancelled,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum ApprovalDecider {
    Policy,
    User,
    System,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum ApprovalDecision {
    Approve,
    Decline,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct ApprovalRecord {
    pub sequence: u64,
    pub version: u32,
    pub id: Uuid,
    pub turn_id: Uuid,
    pub task_id: Option<Uuid>,
    pub conversation_id: Uuid,
    pub project_id: Option<Uuid>,
    pub employee_id: Uuid,
    pub session_id: Option<Uuid>,
    pub owner_boot_id: Uuid,
    pub provider: String,
    pub native_session_id: Option<String>,
    pub native_turn_id: Option<String>,
    pub native_request_id: Option<String>,
    pub kind: ApprovalKind,
    pub action: String,
    pub detail: Value,
    pub fingerprint: String,
    pub authority: AuthorityMode,
    pub scope_expansion: bool,
    pub state: ApprovalState,
    pub decider: Option<ApprovalDecider>,
    pub revision: u64,
    pub failure_code: Option<String>,
    pub failure_message: Option<String>,
    pub created_at: String,
    pub expires_at: String,
    pub updated_at: String,
    pub decided_at: Option<String>,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct ApprovalMutationResult {
    pub approval: ApprovalRecord,
    pub event_sequence: Option<u64>,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct ApprovalReconcileResult {
    pub approvals: Vec<ApprovalRecord>,
    pub event_sequence: Option<u64>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct DecideApprovalParams {
    pub id: Uuid,
    pub expected_revision: u64,
    pub decision: ApprovalDecision,
    pub idempotency_key: String,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct ApprovalListParams {
    #[serde(default)]
    pub after_sequence: u64,
    #[serde(default = "default_list_limit")]
    pub limit: u32,
    pub state: Option<ApprovalState>,
    pub turn_id: Option<Uuid>,
    pub task_id: Option<Uuid>,
    pub conversation_id: Option<Uuid>,
    pub employee_id: Option<Uuid>,
}

impl Default for ApprovalListParams {
    fn default() -> Self {
        Self {
            after_sequence: 0,
            limit: default_list_limit(),
            state: None,
            turn_id: None,
            task_id: None,
            conversation_id: None,
            employee_id: None,
        }
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct ApprovalShowParams {
    pub id: Uuid,
}

const fn default_list_limit() -> u32 {
    100
}
