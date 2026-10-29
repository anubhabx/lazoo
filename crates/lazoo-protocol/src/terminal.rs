use serde::{Deserialize, Serialize};
use uuid::Uuid;

pub const TERMINAL_CONTRACT_V1: u32 = 1;

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct TerminalOpenParams {
    pub executable: String,
    pub args: Vec<String>,
    pub cwd: String,
    pub columns: u16,
    pub rows: u16,
    pub idempotency_key: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct TerminalShowParams {
    pub id: Uuid,
}
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct TerminalResizeParams {
    pub id: Uuid,
    pub columns: u16,
    pub rows: u16,
    pub expected_revision: u64,
    pub expected_boot_id: Uuid,
}
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct TerminalStopParams {
    pub id: Uuid,
    pub expected_revision: u64,
    pub expected_boot_id: Uuid,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct TerminalRecord {
    pub id: Uuid,
    pub boot_id: Uuid,
    pub pid: u32,
    pub executable: String,
    pub args: Vec<String>,
    pub cwd: String,
    pub columns: u16,
    pub rows: u16,
    pub revision: u64,
    pub live: bool,
    pub exit_code: Option<u32>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct TerminalListResult {
    pub boot_id: Uuid,
    pub terminals: Vec<TerminalRecord>,
}

/// `session.terminal.attach`: open the suspended employee session's native harness in an
/// owner terminal that resumes it. `id` and `expected_revision` name the employee session.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct SessionTerminalAttachParams {
    pub id: Uuid,
    pub expected_revision: u64,
    pub columns: u16,
    pub rows: u16,
    pub idempotency_key: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct SessionTerminalAttachResult {
    pub session_id: Uuid,
    pub terminal: TerminalRecord,
}

/// `session.terminal.view`: open the genuine native TUI for an eligible active session without
/// granting it a second controller path.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct SessionTerminalViewParams {
    pub id: Uuid,
    pub expected_revision: u64,
    pub columns: u16,
    pub rows: u16,
    pub idempotency_key: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct SessionTerminalViewResult {
    pub session_id: Uuid,
    pub terminal: TerminalRecord,
}

/// Read-only recovery state for a session's owner terminal. `blocking_*` are stable UI data;
/// a missing value means attachment is currently eligible.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct SessionTerminalStatusParams {
    pub id: Uuid,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
#[allow(clippy::struct_excessive_bools)] // Independent attach/view eligibility and in-flight state.
pub struct SessionTerminalStatusResult {
    pub boot_id: Uuid,
    pub session_id: Uuid,
    pub session_revision: u64,
    pub opening: bool,
    pub terminal: Option<TerminalRecord>,
    pub attachable: bool,
    pub blocking_code: Option<String>,
    pub blocking_reason: Option<String>,
    pub viewable: bool,
    pub view_blocking_code: Option<String>,
    pub view_blocking_reason: Option<String>,
    pub view_opening: bool,
    pub view_terminal: Option<TerminalRecord>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct TerminalInput {
    pub id: Uuid,
    pub bytes: Vec<u8>,
    pub expected_boot_id: Uuid,
}
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct TerminalOutputRequest {
    pub id: Uuid,
    pub after: u64,
    pub limit: usize,
    pub expected_boot_id: Uuid,
}
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct TerminalOutput {
    pub start: u64,
    pub end: u64,
    pub bytes: Vec<u8>,
    pub gap: bool,
    pub live: bool,
    pub exit_code: Option<u32>,
}
