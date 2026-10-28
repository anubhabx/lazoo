//! Metadata frames for the dedicated terminal pipe. Raw bytes follow separately.
use serde::{Deserialize, Serialize};
use uuid::Uuid;

pub const TERMINAL_INPUT_MAX_BYTES: usize = 16 * 1024;
pub const TERMINAL_OUTPUT_MAX_BYTES: usize = 64 * 1024;

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(
    tag = "operation",
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum TerminalIoRequest {
    Read {
        id: Uuid,
        expected_boot_id: Uuid,
        after: u64,
        limit: usize,
    },
    Write {
        id: Uuid,
        expected_boot_id: Uuid,
    },
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TerminalIoResponse {
    pub error: Option<String>,
    pub output: Option<TerminalOutputMetadata>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TerminalOutputMetadata {
    pub start: u64,
    pub end: u64,
    pub length: usize,
    pub gap: bool,
    pub live: bool,
    pub exit_code: Option<u32>,
}
