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
