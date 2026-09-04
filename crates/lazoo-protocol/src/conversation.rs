use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

use crate::{MessageEndpoint, MessageRecord, MessageSender, TaskRecord};

pub const CONVERSATION_CONTRACT_V1: u32 = 1;

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum TurnState {
    Recorded,
    Submitting,
    Accepted,
    Completed,
    RetryableFailed,
    TerminalFailed,
    Cancelled,
    Uncertain,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct ConversationRecord {
    pub sequence: u64,
    pub version: u32,
    pub id: Uuid,
    pub project_id: Option<Uuid>,
    pub lead_employee_id: Uuid,
    pub lead_employee_configuration_revision: u64,
    pub title: Option<String>,
    pub revision: u64,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct ConversationSummary {
    pub sequence: u64,
    pub id: Uuid,
    pub project_id: Option<Uuid>,
    pub lead_employee_id: Uuid,
    pub title: Option<String>,
    pub revision: u64,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct ConversationTurnEvent {
    pub sequence: u64,
    pub kind: String,
    pub provider: String,
    pub native_session_id: Option<String>,
    pub native_turn_id: Option<String>,
    pub payload: Value,
    pub created_at: String,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct ConversationTurnRecord {
    pub sequence: u64,
    pub version: u32,
    pub id: Uuid,
    pub conversation_id: Uuid,
    pub task_id: Option<Uuid>,
    pub author: MessageSender,
    pub recipient: MessageEndpoint,
    pub recipient_employee_configuration_revision: u64,
    pub text: String,
    pub message_id: Uuid,
    pub message_attempt_id: Option<Uuid>,
    pub correlation_id: Uuid,
    pub state: TurnState,
    pub revision: u64,
    pub owner_boot_id: Option<Uuid>,
    pub provider: Option<String>,
    pub native_session_id: Option<String>,
    pub native_turn_id: Option<String>,
    pub failure_code: Option<String>,
    pub failure_message: Option<String>,
    pub transcript_truncated: bool,
    pub created_at: String,
    pub updated_at: String,
    pub submitted_at: Option<String>,
    pub accepted_at: Option<String>,
    pub completed_at: Option<String>,
    pub cancel_requested_at: Option<String>,
    pub terminal_at: Option<String>,
    pub events: Vec<ConversationTurnEvent>,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct ConversationTurnSummary {
    pub sequence: u64,
    pub id: Uuid,
    pub conversation_id: Uuid,
    pub task_id: Option<Uuid>,
    pub author: MessageSender,
    pub recipient: MessageEndpoint,
    pub state: TurnState,
    pub revision: u64,
    pub native_turn_id: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct ConversationDetail {
    pub conversation: ConversationRecord,
    pub turns: Vec<ConversationTurnSummary>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct ConversationMutationResult {
    pub conversation: ConversationRecord,
    pub event_sequence: Option<u64>,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct TurnMutationResult {
    pub turn: ConversationTurnRecord,
    pub message: MessageRecord,
    pub task: Option<TaskRecord>,
    pub event_sequence: Option<u64>,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct TurnReconcileResult {
    pub turns: Vec<ConversationTurnRecord>,
    pub event_sequence: Option<u64>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct CreateConversationParams {
    pub project_id: Option<Uuid>,
    pub lead_employee_id: Uuid,
    pub title: Option<String>,
    pub idempotency_key: String,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct ConversationShowParams {
    pub id: Uuid,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct ConversationListParams {
    #[serde(default)]
    pub after_sequence: u64,
    #[serde(default = "default_list_limit")]
    pub limit: u32,
}

impl Default for ConversationListParams {
    fn default() -> Self {
        Self {
            after_sequence: 0,
            limit: default_list_limit(),
        }
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct SubmitTurnParams {
    pub conversation_id: Uuid,
    pub author: MessageSender,
    pub recipient: Option<MessageEndpoint>,
    pub text: String,
    #[serde(default)]
    pub activate_if_needed: bool,
    #[serde(default = "default_turn_max_attempts")]
    pub max_attempts: u32,
    pub idempotency_key: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct RetryTurnParams {
    pub id: Uuid,
    pub expected_revision: u64,
    pub idempotency_key: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct CancelTurnParams {
    pub id: Uuid,
    pub expected_revision: u64,
    pub idempotency_key: String,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct TurnShowParams {
    pub id: Uuid,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct TurnListParams {
    pub conversation_id: Uuid,
    #[serde(default)]
    pub after_sequence: u64,
    #[serde(default = "default_list_limit")]
    pub limit: u32,
}

const fn default_turn_max_attempts() -> u32 {
    3
}

const fn default_list_limit() -> u32 {
    100
}
