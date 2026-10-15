use serde::{Deserialize, Serialize};
use uuid::Uuid;

pub const MEMORY_CONTRACT_V1: u32 = 1;

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum MemoryRecordType {
    Decision,
    Action,
    Achievement,
    Question,
    Failure,
    Constraint,
    Fact,
    Summary,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum MemoryQueueState {
    Pending,
    Claimed,
    Completed,
    Failed,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct MemoryRecord {
    pub id: Uuid,
    pub revision: u64,
    pub project_id: Option<Uuid>,
    pub source_turn_id: Uuid,
    pub source_hash: String,
    pub record_type: MemoryRecordType,
    pub text: String,
    pub confidence: f64,
    pub importance: u8,
    pub created_at: String,
    pub active: bool,
    pub superseded_at: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct MemorySourceArchive {
    pub source_turn_id: Uuid,
    pub source_hash: String,
    pub project_id: Option<Uuid>,
    pub archive_name: String,
    pub compressed_bytes: u64,
    pub uncompressed_bytes: u64,
    pub truncated: bool,
    pub created_at: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct MemoryPolicy {
    pub project_id: Option<Uuid>,
    pub revision: u64,
    pub retention_enabled: bool,
    pub retention_days: u32,
    pub quota_bytes: u64,
    pub max_source_bytes: u64,
    pub allowed_types: Vec<MemoryRecordType>,
    pub inject_enabled: bool,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct MemorySummaryJob {
    pub id: Uuid,
    pub source_hash: String,
    pub source_turn_id: Uuid,
    pub project_id: Option<Uuid>,
    pub state: MemoryQueueState,
    pub attempts: u32,
    pub due_at: String,
    pub lease_owner_boot_id: Option<Uuid>,
    pub created_at: String,
    pub updated_at: String,
    pub failure_message: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct MemorySearchParams {
    pub project_id: Option<Uuid>,
    pub query: String,
    #[serde(default)]
    pub employee_id: Option<Uuid>,
    #[serde(default)]
    pub conversation_id: Option<Uuid>,
    #[serde(default)]
    pub task_id: Option<Uuid>,
    #[serde(default)]
    pub root_task_id: Option<Uuid>,
    /// Matches only a durable task-dispatch worktree branch.
    #[serde(default)]
    pub branch: Option<String>,
    #[serde(default)]
    pub record_types: Vec<MemoryRecordType>,
    #[serde(default = "default_memory_limit")]
    pub limit: u32,
}
const fn default_memory_limit() -> u32 {
    20
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct MemoryContextPack {
    pub records: Vec<MemoryRecord>,
    pub truncated: bool,
    pub byte_budget: u32,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct MemoryCleanupPreview {
    pub project_id: Option<Uuid>,
    pub archive_names: Vec<String>,
    pub source_hashes: Vec<String>,
    pub bytes: u64,
}
