use serde::{Deserialize, Serialize};
use uuid::Uuid;

pub const NATIVE_TRUST_CONTRACT_V1: u32 = 1;

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum NativeTrustState {
    Pending,
    Approved,
    Declined,
    Cancelled,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum NativeTrustDecision {
    Approve,
    Decline,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum NativeTrustDecider {
    User,
    Policy,
    System,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum NativeTrustNotificationState {
    Pending,
    Claimed,
    Settled,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct NativeTrustRequestRecord {
    pub sequence: u64,
    pub version: u32,
    pub id: Uuid,
    pub provider: String,
    pub canonical_workspace_path: String,
    pub managed_workspace: bool,
    pub session_id: Uuid,
    pub employee_id: Uuid,
    pub owner_boot_id: Uuid,
    pub state: NativeTrustState,
    pub decider: Option<NativeTrustDecider>,
    pub revision: u64,
    pub notification_state: NativeTrustNotificationState,
    pub notification_claim_id: Option<Uuid>,
    pub created_at: String,
    pub updated_at: String,
    pub decided_at: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct NativeTrustPolicy {
    pub provider: String,
    pub auto_accept_managed_next_time: bool,
    pub revision: u64,
    pub updated_at: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct NativeTrustMutationResult {
    pub trust: NativeTrustRequestRecord,
    pub event_sequence: Option<u64>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct NativeTrustReconcileResult {
    pub trusts: Vec<NativeTrustRequestRecord>,
    pub event_sequence: Option<u64>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct ResolveNativeTrustParams {
    pub id: Uuid,
    pub expected_revision: u64,
    pub decision: NativeTrustDecision,
    pub auto_accept_managed_next_time: bool,
    pub idempotency_key: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct NativeTrustPolicyParams {
    pub provider: String,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct NativeTrustShowParams {
    pub id: Uuid,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct SetNativeTrustPolicyParams {
    pub provider: String,
    pub expected_revision: u64,
    pub auto_accept_managed_next_time: bool,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct ResetNativeTrustPolicyParams {
    pub provider: String,
    pub expected_revision: u64,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct NativeTrustListParams {
    #[serde(default)]
    pub after_sequence: u64,
    #[serde(default = "default_list_limit")]
    pub limit: u32,
    pub state: Option<NativeTrustState>,
}

impl Default for NativeTrustListParams {
    fn default() -> Self {
        Self {
            after_sequence: 0,
            limit: default_list_limit(),
            state: None,
        }
    }
}

const fn default_list_limit() -> u32 {
    100
}
