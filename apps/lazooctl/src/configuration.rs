//! CLI request construction for the server-owned MCP configuration workflow.

use serde_json::{Value, json};
use uuid::Uuid;

/// Keep receipts server-owned: neither this CLI helper nor its arguments accept one.
pub(super) fn preview_request(selection: &Value) -> (&'static str, Value) {
    (
        "provider.config.select.preview",
        json!({"selection": selection}),
    )
}

pub(super) fn apply_request(operation_id: Uuid, expected_revision: u64) -> (&'static str, Value) {
    (
        "provider.config.apply",
        json!({"operationId": operation_id, "expectedRevision": expected_revision}),
    )
}

pub(super) fn rollback_request(
    operation_id: Uuid,
    expected_revision: u64,
) -> (&'static str, Value) {
    (
        "provider.config.rollback",
        json!({"operationId": operation_id, "expectedRevision": expected_revision}),
    )
}

pub(super) fn show_request(operation_id: Uuid) -> (&'static str, Value) {
    (
        "provider.config.operations.show",
        json!({"operationId": operation_id}),
    )
}
