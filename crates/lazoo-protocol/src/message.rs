use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

pub const MESSAGE_ENVELOPE_V1: u32 = 1;
pub const MESSAGE_ENVELOPE_V2: u32 = 2;

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum MessageKind {
    Delegation,
    Peer,
    Reply,
    Result,
    Status,
    System,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum MessageDeliveryState {
    Queued,
    Delivering,
    Acknowledged,
    RetryableFailed,
    TerminalFailed,
    Cancelled,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct MessageEndpoint {
    pub employee_id: Uuid,
    pub session_id: Option<Uuid>,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum UserMessageKind {
    User,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct UserMessageEndpoint {
    pub kind: UserMessageKind,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(untagged)]
pub enum MessageSender {
    Employee(MessageEndpoint),
    User(UserMessageEndpoint),
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct MessageReference {
    pub kind: String,
    pub target: String,
    #[serde(default = "empty_object")]
    pub metadata: Value,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct MessageEnvelope {
    pub version: u32,
    pub id: Uuid,
    pub idempotency_key: String,
    pub correlation_id: Uuid,
    pub causation_id: Option<Uuid>,
    pub sender: MessageSender,
    pub recipient: MessageEndpoint,
    pub project_id: Option<Uuid>,
    pub conversation_id: Option<Uuid>,
    pub task_id: Option<Uuid>,
    pub kind: MessageKind,
    pub body: Value,
    pub references: Vec<MessageReference>,
    pub created_at: String,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct MessageActivationIntent {
    pub employee_id: Uuid,
    pub session_id: Option<Uuid>,
    pub requires_activation: bool,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct MessageDelivery {
    pub state: MessageDeliveryState,
    pub revision: u64,
    pub attempt_count: u32,
    pub max_attempts: u32,
    pub next_attempt_at: Option<String>,
    pub last_failure_code: Option<String>,
    pub last_failure_message: Option<String>,
    pub acknowledged_at: Option<String>,
    pub terminal_at: Option<String>,
    pub updated_at: String,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct MessageDeliveryAttempt {
    pub id: Uuid,
    pub number: u32,
    pub state: MessageDeliveryState,
    pub started_at: String,
    pub finished_at: Option<String>,
    pub failure_code: Option<String>,
    pub failure_message: Option<String>,
    pub acknowledgement_key: Option<String>,
    pub acknowledgement_evidence: Option<Value>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct MessageDeadLetter {
    pub attempt_id: Uuid,
    pub failure_code: String,
    pub failure_message: String,
    pub created_at: String,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct MessageRecord {
    pub sequence: u64,
    pub envelope: MessageEnvelope,
    pub activation_intent: MessageActivationIntent,
    pub delivery: MessageDelivery,
    pub attempts: Vec<MessageDeliveryAttempt>,
    pub dead_letter: Option<MessageDeadLetter>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct MessageSummary {
    pub sequence: u64,
    pub id: Uuid,
    pub correlation_id: Uuid,
    pub causation_id: Option<Uuid>,
    pub sender: MessageSender,
    pub recipient: MessageEndpoint,
    pub project_id: Option<Uuid>,
    pub conversation_id: Option<Uuid>,
    pub task_id: Option<Uuid>,
    pub kind: MessageKind,
    pub state: MessageDeliveryState,
    pub revision: u64,
    pub attempt_count: u32,
    pub max_attempts: u32,
    pub next_attempt_at: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct MessageMutationResult {
    pub message: MessageRecord,
    pub event_sequence: Option<u64>,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct MessageReconcileResult {
    pub messages: Vec<MessageRecord>,
    pub event_sequence: Option<u64>,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct SendMessageParams {
    pub sender: MessageEndpoint,
    pub recipient: MessageEndpoint,
    pub project_id: Option<Uuid>,
    pub conversation_id: Option<Uuid>,
    pub task_id: Option<Uuid>,
    pub correlation_id: Option<Uuid>,
    pub causation_id: Option<Uuid>,
    pub kind: MessageKind,
    pub body: Value,
    #[serde(default)]
    pub references: Vec<MessageReference>,
    #[serde(default = "default_max_attempts")]
    pub max_attempts: u32,
    pub idempotency_key: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct ClaimMessageParams {
    pub id: Uuid,
    pub expected_revision: u64,
    pub idempotency_key: String,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct AcknowledgeMessageParams {
    pub id: Uuid,
    pub expected_revision: u64,
    pub attempt_id: Uuid,
    pub acknowledgement_key: String,
    #[serde(default = "empty_object")]
    pub evidence: Value,
    pub idempotency_key: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct FailMessageParams {
    pub id: Uuid,
    pub expected_revision: u64,
    pub attempt_id: Uuid,
    pub retryable: bool,
    pub failure_code: String,
    pub failure_message: String,
    pub idempotency_key: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct RetryMessageParams {
    pub id: Uuid,
    pub expected_revision: u64,
    pub idempotency_key: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct CancelMessageParams {
    pub id: Uuid,
    pub expected_revision: u64,
    pub idempotency_key: String,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct MessageShowParams {
    pub id: Uuid,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct MessageListParams {
    #[serde(default)]
    pub after_sequence: u64,
    #[serde(default = "default_list_limit")]
    pub limit: u32,
}

impl Default for MessageListParams {
    fn default() -> Self {
        Self {
            after_sequence: 0,
            limit: default_list_limit(),
        }
    }
}

fn empty_object() -> Value {
    Value::Object(serde_json::Map::new())
}

const fn default_max_attempts() -> u32 {
    3
}

const fn default_list_limit() -> u32 {
    100
}
