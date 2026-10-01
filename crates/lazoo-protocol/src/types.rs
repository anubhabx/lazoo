use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum CheckoutMode {
    #[default]
    Isolated,
    LeadMain,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum AuthorityMode {
    #[default]
    Ask,
    SelfDecision,
    FullAutonomy,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum EmployeeCheckoutPolicy {
    #[default]
    Isolated,
    ReadOnly,
}

/// Whether the daemon may start a provider session for this employee on its own. `manual`
/// leaves every activation to the local user; `notice-turns` lets a recorded notice turn start
/// one bounded session for the employee's own task and stop it after the turn.
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum ActivationPolicy {
    #[default]
    Manual,
    NoticeTurns,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum EmployeeAvailability {
    Disabled,
    Offline,
    Unverified,
    Dormant,
    Starting,
    Idle,
    Suspended,
    Failed,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum SessionLifecycleState {
    Starting,
    Idle,
    Stopping,
    Suspended,
    Failed,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct EmployeeSessionRecord {
    pub id: Uuid,
    pub employee_id: Uuid,
    pub employee_configuration_revision: u64,
    pub provider: String,
    pub state: SessionLifecycleState,
    pub revision: u64,
    pub owner_boot_id: Option<Uuid>,
    pub native_session_id: Option<String>,
    pub native_host_pid: Option<u32>,
    pub failure_code: Option<String>,
    pub failure_message: Option<String>,
    pub created_at: String,
    pub updated_at: String,
    pub started_at: Option<String>,
    pub stopped_at: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct EmployeeRecord {
    pub id: Uuid,
    pub name: String,
    pub provider: String,
    pub role: String,
    pub instructions: String,
    pub model: Option<String>,
    pub reasoning_effort: Option<String>,
    pub native_settings: Value,
    pub authority_mode: AuthorityMode,
    pub checkout_policy: EmployeeCheckoutPolicy,
    pub activation_policy: ActivationPolicy,
    pub enabled: bool,
    pub configuration_revision: u64,
    pub created_at: String,
    pub updated_at: String,
    pub disabled_at: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct ProviderObservation {
    pub provider: String,
    pub verification: String,
    pub version: Option<String>,
    pub protocol: Option<String>,
    pub capabilities: Value,
    pub authentication: Value,
    pub degradation_reason: Option<String>,
    pub checked_at: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct RosterEntry {
    pub employee: EmployeeRecord,
    pub availability: EmployeeAvailability,
    pub availability_reason: Option<String>,
    pub provider_observation: Option<ProviderObservation>,
    pub session: Option<EmployeeSessionRecord>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct RosterSnapshot {
    pub revision: u64,
    pub entries: Vec<RosterEntry>,
    pub updated_at: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct EmployeeMutationResult {
    pub employee: EmployeeRecord,
    pub roster_revision: u64,
    pub event_sequence: Option<u64>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct RosterRefreshResult {
    pub roster: RosterSnapshot,
    pub event_sequence: Option<u64>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct CreateEmployeeParams {
    pub name: String,
    pub provider: String,
    #[serde(default)]
    pub role: String,
    #[serde(default)]
    pub instructions: String,
    pub model: Option<String>,
    pub reasoning_effort: Option<String>,
    #[serde(default = "empty_object")]
    pub native_settings: Value,
    #[serde(default)]
    pub authority_mode: AuthorityMode,
    #[serde(default)]
    pub checkout_policy: EmployeeCheckoutPolicy,
    #[serde(default)]
    pub activation_policy: ActivationPolicy,
    #[serde(default = "default_enabled")]
    pub enabled: bool,
    pub idempotency_key: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct UpdateEmployeeParams {
    pub id: Uuid,
    pub expected_revision: u64,
    pub name: Option<String>,
    pub provider: Option<String>,
    pub role: Option<String>,
    pub instructions: Option<String>,
    pub model: Option<String>,
    pub reasoning_effort: Option<String>,
    pub native_settings: Option<Value>,
    pub authority_mode: Option<AuthorityMode>,
    pub checkout_policy: Option<EmployeeCheckoutPolicy>,
    pub activation_policy: Option<ActivationPolicy>,
    pub enabled: Option<bool>,
    pub idempotency_key: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct DisableEmployeeParams {
    pub id: Uuid,
    pub expected_revision: u64,
    pub idempotency_key: String,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct EmployeeShowParams {
    pub id: Uuid,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct RosterRefreshParams {
    pub idempotency_key: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct WarmSessionParams {
    pub employee_id: Uuid,
    pub expected_employee_revision: u64,
    pub idempotency_key: String,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct SessionShowParams {
    pub id: Uuid,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct StopSessionParams {
    pub id: Uuid,
    pub expected_revision: u64,
    pub idempotency_key: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct SessionMutationResult {
    pub session: EmployeeSessionRecord,
    pub roster_revision: u64,
    pub event_sequence: Option<u64>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct SessionReconcileResult {
    pub sessions: Vec<EmployeeSessionRecord>,
    pub roster_revision: u64,
    pub event_sequence: Option<u64>,
}

fn empty_object() -> Value {
    Value::Object(serde_json::Map::new())
}

const fn default_enabled() -> bool {
    true
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct ProjectRecord {
    pub id: Uuid,
    pub repository_identity: String,
    pub canonical_path: String,
    pub display_name: String,
    pub checkout_mode: CheckoutMode,
    pub configuration_revision: u64,
    pub created_at: String,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct EventSummary {
    pub sequence: u64,
    pub id: Uuid,
    pub event_type: String,
    pub payload: Value,
    pub created_at: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct RegisterProjectResult {
    pub project: ProjectRecord,
    pub event_sequence: u64,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct RegisterProjectParams {
    pub path: PathBuf,
    pub display_name: Option<String>,
    #[serde(default)]
    pub checkout_mode: CheckoutMode,
    pub idempotency_key: String,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct EventsListParams {
    #[serde(default)]
    pub after: u64,
    #[serde(default = "default_event_limit")]
    pub limit: u32,
}

impl Default for EventsListParams {
    fn default() -> Self {
        Self {
            after: 0,
            limit: default_event_limit(),
        }
    }
}

const fn default_event_limit() -> u32 {
    100
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct SystemStatus {
    pub daemon_version: String,
    pub boot_id: Uuid,
    pub uptime_millis: u64,
    pub protocol_version: u32,
    pub data_root: PathBuf,
    pub schema_version: u32,
    pub journal_mode: String,
    pub project_count: u64,
    pub event_high_water: u64,
    pub pending_outbox: u64,
    pub emergency_incident_count: u64,
    pub keeper: Option<Value>,
    pub degraded: Vec<String>,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct DesktopSnapshot {
    pub captured_at: String,
    pub status: SystemStatus,
    pub projects: Vec<ProjectRecord>,
    pub events: Vec<EventSummary>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct ProviderListEntry {
    pub provider: String,
    pub detected: bool,
    pub executable: Option<PathBuf>,
    pub discovery_source: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct ProviderProbeParams {
    pub provider: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct ProviderSmokeParams {
    pub provider: String,
    pub prompt: String,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct ProviderSmokeResult {
    pub provider: String,
    pub session_id: String,
    pub events: Vec<Value>,
    pub transcript_truncated: bool,
    pub checked_at: String,
}
