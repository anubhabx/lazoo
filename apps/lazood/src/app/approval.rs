use chrono::{DateTime, Utc};
use lazoo_protocol::{
    ApprovalKind, ApprovalListParams, ApprovalShowParams, ApprovalState, DecideApprovalParams,
    JsonRpcRequest, RpcError, TurnState,
};
use lazoo_providers::{PermissionDecision, PermissionKind, PermissionRequest};
use lazoo_store::{
    CloseApprovalInput, DecideApprovalInput, ListApprovalsInput, RequestApprovalInput, StoreError,
};
use serde_json::{Value, json};
use tokio::time::{Duration, sleep};
use uuid::Uuid;

use super::{App, to_value};

/// Pending approvals are re-read on this cadence even without a notification so that turn
/// cancellation and expiry are observed promptly.
const APPROVAL_POLL_INTERVAL: Duration = Duration::from_millis(250);

/// Async cancellation can drop the observer future before its polling loop sees the turn state.
/// Close the durable pending record synchronously in that case; a concurrent human decision is
/// protected by the revision check and wins harmlessly.
struct PendingApprovalGuard {
    app: App,
    approval_id: Uuid,
    owner_boot_id: Uuid,
    expected_revision: u64,
    armed: bool,
}

impl PendingApprovalGuard {
    fn disarm(&mut self) {
        self.armed = false;
    }
}

impl Drop for PendingApprovalGuard {
    fn drop(&mut self) {
        if !self.armed {
            return;
        }
        if self
            .app
            .store
            .close_approval(&CloseApprovalInput {
                approval_id: self.approval_id,
                owner_boot_id: self.owner_boot_id,
                expected_revision: self.expected_revision,
                state: ApprovalState::Cancelled,
                failure_code: "LZ-APPROVAL-OBSERVER-DROPPED".to_owned(),
                failure_message: "the provider approval observer stopped waiting".to_owned(),
            })
            .is_ok()
        {
            let _ = self.app.publish_events();
        }
    }
}

impl App {
    /// Governs one provider approval request for a daemon-owned turn. The request is durable
    /// before any decision; policy answers immediately, otherwise the provider waits for the local
    /// user until the approval expires or the governed turn ends. Every path fails closed.
    pub(super) async fn govern_permission(
        &self,
        turn_id: Uuid,
        request: &PermissionRequest,
    ) -> Result<PermissionDecision, RpcError> {
        if self
            .store
            .is_memory_summary_turn(turn_id)
            .map_err(approval_store_error)?
        {
            // Scheduled summarization consumes the supplied source only. It cannot request
            // commands or edits, even if the employee's ordinary task policy is broader.
            return Ok(PermissionDecision::Decline);
        }
        let kind = match request.kind {
            PermissionKind::CommandExecution => ApprovalKind::CommandExecution,
            PermissionKind::FileChange => ApprovalKind::FileChange,
        };
        let requested = self
            .store
            .request_approval(&RequestApprovalInput {
                request_id: format!("turn-approval-{turn_id}-{}", request.request_id),
                idempotency_key: format!("approval:{turn_id}:{}", request.request_id),
                turn_id,
                owner_boot_id: self.boot_id,
                native_request_id: Some(request.request_id.clone()),
                kind,
                action: request.action.clone(),
                scope_expansion: request.scope_expansion,
                detail: request.detail.clone(),
                pending_timeout_seconds: self.approval_timeout.as_secs(),
            })
            .map_err(approval_store_error)?;
        self.publish_events()?;
        let mut approval = requested.approval;
        let mut pending_guard = PendingApprovalGuard {
            app: self.clone(),
            approval_id: approval.id,
            owner_boot_id: self.boot_id,
            expected_revision: approval.revision,
            armed: true,
        };
        while approval.state == ApprovalState::Pending {
            tokio::select! {
                () = self.approval_notify.notified() => {}
                () = sleep(APPROVAL_POLL_INTERVAL) => {}
            }
            approval = self
                .store
                .get_approval(approval.id)
                .map_err(approval_store_error)?;
            if approval.state != ApprovalState::Pending {
                break;
            }
            let turn = self.store.get_turn(turn_id).map_err(approval_store_error)?;
            let turn_live = turn.cancel_requested_at.is_none()
                && matches!(turn.state, TurnState::Submitting | TurnState::Accepted);
            let expired = DateTime::parse_from_rfc3339(&approval.expires_at)
                .is_ok_and(|expires_at| Utc::now() >= expires_at);
            let closure = if !turn_live {
                Some((
                    ApprovalState::Cancelled,
                    "LZ-APPROVAL-TURN-CANCELLED",
                    "the governed turn ended while the approval was pending",
                ))
            } else if expired {
                Some((
                    ApprovalState::Expired,
                    "LZ-APPROVAL-TIMEOUT",
                    "no user decision arrived before the approval expired",
                ))
            } else {
                None
            };
            let Some((state, failure_code, failure_message)) = closure else {
                continue;
            };
            match self.store.close_approval(&CloseApprovalInput {
                approval_id: approval.id,
                owner_boot_id: self.boot_id,
                expected_revision: approval.revision,
                state,
                failure_code: failure_code.to_owned(),
                failure_message: failure_message.to_owned(),
            }) {
                Ok(closed) => {
                    approval = closed.approval;
                    self.publish_events()?;
                }
                // A concurrent user decision won the race; the next iteration reads it back.
                Err(StoreError::ApprovalStale { .. } | StoreError::ApprovalState { .. }) => {}
                Err(error) => return Err(approval_store_error(error)),
            }
        }
        pending_guard.disarm();
        Ok(if approval.state == ApprovalState::Approved {
            PermissionDecision::Approve
        } else {
            PermissionDecision::Decline
        })
    }

    pub(super) fn decide_approval(&self, request: &JsonRpcRequest) -> Result<Value, RpcError> {
        let params: DecideApprovalParams = approval_params(&request.params)?;
        if params.expected_revision == 0 {
            return Err(approval_invalid(
                "expectedRevision must be greater than zero",
            ));
        }
        let result = self
            .store
            .decide_approval(&DecideApprovalInput {
                request_id: request.id.clone(),
                idempotency_key: params.idempotency_key,
                approval_id: params.id,
                expected_revision: params.expected_revision,
                decision: params.decision,
            })
            .map_err(approval_store_error)?;
        self.publish_events()?;
        self.approval_notify.notify_waiters();
        Ok(to_value(result))
    }

    pub(super) fn approval_list(&self, value: &Value) -> Result<Value, RpcError> {
        let params: ApprovalListParams = approval_params(value)?;
        self.store
            .list_approvals(&ListApprovalsInput {
                after_sequence: params.after_sequence,
                limit: params.limit,
                state: params.state,
                turn_id: params.turn_id,
                task_id: params.task_id,
                conversation_id: params.conversation_id,
                employee_id: params.employee_id,
            })
            .map(to_value)
            .map_err(approval_store_error)
    }

    pub(super) fn approval_show(&self, value: &Value) -> Result<Value, RpcError> {
        let params: ApprovalShowParams = approval_params(value)?;
        self.store
            .get_approval(params.id)
            .map(to_value)
            .map_err(approval_store_error)
    }
}

fn approval_params<T: serde::de::DeserializeOwned>(value: &Value) -> Result<T, RpcError> {
    serde_json::from_value(value.clone()).map_err(|error| approval_invalid(error.to_string()))
}

fn approval_invalid(message: impl Into<String>) -> RpcError {
    RpcError {
        code: "LZ-APPROVAL-INVALID".to_owned(),
        message: message.into(),
        data: None,
    }
}

#[allow(clippy::needless_pass_by_value)]
fn approval_store_error(error: StoreError) -> RpcError {
    let (code, data, message) = match &error {
        StoreError::ApprovalNotFound { approval_id } => (
            "LZ-APPROVAL-NOT-FOUND",
            Some(json!({"approvalId": approval_id})),
            error.to_string(),
        ),
        StoreError::ApprovalStale {
            approval_id,
            current_revision,
        } => (
            "LZ-APPROVAL-STALE",
            Some(json!({"approvalId": approval_id, "currentRevision": current_revision})),
            error.to_string(),
        ),
        StoreError::ApprovalState { approval_id, .. } => (
            "LZ-APPROVAL-STATE",
            Some(json!({"approvalId": approval_id})),
            error.to_string(),
        ),
        StoreError::ApprovalOwnership { approval_id } => (
            "LZ-APPROVAL-OWNERSHIP",
            Some(json!({"approvalId": approval_id})),
            error.to_string(),
        ),
        StoreError::ApprovalTurnOwnership { turn_id } => (
            "LZ-APPROVAL-OWNERSHIP",
            Some(json!({"turnId": turn_id})),
            error.to_string(),
        ),
        StoreError::ApprovalTurnState { turn_id, .. } => (
            "LZ-APPROVAL-TURN-STATE",
            Some(json!({"turnId": turn_id})),
            error.to_string(),
        ),
        StoreError::TurnNotFound { turn_id } => (
            "LZ-APPROVAL-TURN-NOT-FOUND",
            Some(json!({"turnId": turn_id})),
            error.to_string(),
        ),
        StoreError::ApprovalIdempotencyConflict => {
            ("LZ-APPROVAL-IDEMPOTENCY-CONFLICT", None, error.to_string())
        }
        StoreError::ApprovalValidation(_) | StoreError::MessageValidation(_) => {
            ("LZ-APPROVAL-INVALID", None, error.to_string())
        }
        _ => (
            "LZ-APPROVAL-STORE",
            None,
            "approval store operation failed".to_owned(),
        ),
    };
    RpcError {
        code: code.to_owned(),
        message,
        data,
    }
}
