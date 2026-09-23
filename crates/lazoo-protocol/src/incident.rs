use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

pub const INCIDENT_CONTRACT_V1: u32 = 1;

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum IncidentState {
    Open,
    Acknowledged,
    Resolved,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum IncidentPriority {
    Normal,
    Priority,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum IncidentSource {
    Daemon,
    Keeper,
    Employee,
}

/// One human-readable operational problem. Equivalent failures (same code, employee, and
/// project) fold into one row whose occurrence counters and latest scope advance.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct IncidentRecord {
    pub sequence: u64,
    pub version: u32,
    pub id: Uuid,
    pub code: String,
    pub fingerprint: String,
    pub title: String,
    pub explanation: String,
    pub next_action: String,
    pub project_id: Option<Uuid>,
    pub employee_id: Option<Uuid>,
    pub provider: Option<String>,
    pub task_id: Option<Uuid>,
    pub session_id: Option<Uuid>,
    pub turn_id: Option<Uuid>,
    pub message_id: Option<Uuid>,
    pub source: IncidentSource,
    pub detail: Value,
    pub occurrences: u64,
    pub window_occurrences: u64,
    pub first_occurred_at: String,
    pub last_occurred_at: String,
    pub window_started_at: String,
    pub priority: IncidentPriority,
    pub state: IncidentState,
    pub revision: u64,
    pub created_at: String,
    pub updated_at: String,
    pub acknowledged_at: Option<String>,
    pub resolved_at: Option<String>,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct IncidentMutationResult {
    pub incident: IncidentRecord,
    pub event_sequence: Option<u64>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct IncidentListParams {
    #[serde(default)]
    pub after_sequence: u64,
    #[serde(default = "default_list_limit")]
    pub limit: u32,
    pub state: Option<IncidentState>,
    pub priority: Option<IncidentPriority>,
    pub code: Option<String>,
    pub employee_id: Option<Uuid>,
    pub project_id: Option<Uuid>,
}

impl Default for IncidentListParams {
    fn default() -> Self {
        Self {
            after_sequence: 0,
            limit: default_list_limit(),
            state: None,
            priority: None,
            code: None,
            employee_id: None,
            project_id: None,
        }
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct IncidentShowParams {
    pub id: Uuid,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct AcknowledgeIncidentParams {
    pub id: Uuid,
    pub expected_revision: u64,
    pub idempotency_key: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct ResolveIncidentParams {
    pub id: Uuid,
    pub expected_revision: u64,
    pub idempotency_key: String,
}

const fn default_list_limit() -> u32 {
    100
}
