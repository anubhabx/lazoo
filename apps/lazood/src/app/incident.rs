//! Human-readable incidents derived from durable failure records.
//!
//! The failure row (session, turn, message, task dispatch, or keeper spool record) is always
//! committed first and stays authoritative; the incident ledger folds equivalent failures into
//! one row per code, employee, and project so operators and employees read a concise problem
//! instead of a flood of raw failure codes.

use chrono::{DateTime, Utc};
use lazoo_protocol::{
    AcknowledgeIncidentParams, ConversationTurnRecord, EmployeeSessionRecord, IncidentListParams,
    IncidentShowParams, IncidentSource, JsonRpcRequest, MessageRecord, ResolveIncidentParams,
    RpcError, TaskRecord,
};
use lazoo_providers::ProviderError;
use lazoo_store::{ListIncidentsInput, RaiseIncidentInput, SettleIncidentInput, StoreError};
use serde_json::{Value, json};
use uuid::Uuid;

use super::{App, to_value, truncate_utf8};

const MAX_PROVIDER_ERROR_BYTES: usize = 1024;

#[derive(Clone, Debug, Default)]
pub(super) struct IncidentScope {
    pub project_id: Option<Uuid>,
    pub employee_id: Option<Uuid>,
    pub provider: Option<String>,
    pub task_id: Option<Uuid>,
    pub session_id: Option<Uuid>,
    pub turn_id: Option<Uuid>,
    pub message_id: Option<Uuid>,
}

impl App {
    /// Folds one committed failure into the incident ledger. The failure record is already
    /// durable, so a ledger problem here is logged and never fails the caller.
    // ponytail: raised after the failure commit; move into the failure transaction if a crash
    // between the two ever matters in practice.
    pub(super) fn note_incident(&self, key: &str, code: &str, scope: IncidentScope, detail: Value) {
        let input = RaiseIncidentInput {
            request_id: key.to_owned(),
            idempotency_key: key.to_owned(),
            code: code.to_owned(),
            source: IncidentSource::Daemon,
            title: None,
            explanation: None,
            project_id: scope.project_id,
            employee_id: scope.employee_id,
            provider: scope.provider,
            task_id: scope.task_id,
            session_id: scope.session_id,
            turn_id: scope.turn_id,
            message_id: scope.message_id,
            detail,
            occurred_at: None,
        };
        match self.store.raise_incident(&input) {
            Ok(_) => {
                if let Err(error) = self.publish_events() {
                    tracing::warn!(code, message = %error.message, "incident events not published");
                }
            }
            Err(error) => tracing::warn!(code, %error, "incident could not be recorded"),
        }
    }

    pub(super) fn turn_scope(&self, turn: &ConversationTurnRecord) -> IncidentScope {
        IncidentScope {
            project_id: self
                .store
                .get_conversation(turn.conversation_id)
                .ok()
                .and_then(|conversation| conversation.project_id),
            employee_id: Some(turn.recipient.employee_id),
            provider: turn.provider.clone(),
            task_id: turn.task_id,
            session_id: turn.recipient.session_id,
            turn_id: Some(turn.id),
            message_id: Some(turn.message_id),
        }
    }

    pub(super) fn note_session_incident(&self, session: &EmployeeSessionRecord, code: &str) {
        self.note_incident(
            &format!("session-failure:{}:{}", session.id, session.revision),
            code,
            IncidentScope {
                employee_id: Some(session.employee_id),
                provider: Some(session.provider.clone()),
                session_id: Some(session.id),
                ..IncidentScope::default()
            },
            json!({
                "failureCode": session.failure_code,
                "failureMessage": session.failure_message,
            }),
        );
    }

    pub(super) fn note_provider_incident(
        &self,
        turn: &ConversationTurnRecord,
        key_kind: &str,
        code: &str,
        error: Option<&ProviderError>,
        recorded_code: &str,
        recorded_message: &str,
    ) {
        let mut provider_error = error.map(ToString::to_string).unwrap_or_default();
        truncate_utf8(&mut provider_error, MAX_PROVIDER_ERROR_BYTES);
        self.note_incident(
            &format!("{key_kind}:{}:{}", turn.id, turn.revision),
            code,
            self.turn_scope(turn),
            json!({
                "failureCode": recorded_code,
                "failureMessage": recorded_message,
                "providerError": provider_error,
                "nativeTurnId": turn.native_turn_id,
            }),
        );
    }

    /// Startup truth: every record the restart reconciliation closed or marked uncertain, plus
    /// every emergency spool record the keeper wrote, becomes one folded incident per scope.
    pub(super) fn note_startup_incidents(
        &self,
        sessions: &[EmployeeSessionRecord],
        messages: &[MessageRecord],
        turns: &[ConversationTurnRecord],
        tasks: &[TaskRecord],
    ) {
        for session in sessions {
            self.note_session_incident(
                session,
                session
                    .failure_code
                    .as_deref()
                    .unwrap_or("LZ-SESSION-OWNER-LOST"),
            );
        }
        for message in messages {
            let code = message
                .delivery
                .last_failure_code
                .as_deref()
                .unwrap_or("LZ-MESSAGE-OWNER-LOST");
            self.note_incident(
                &format!("owner-lost:message:{}", message.envelope.id),
                code,
                IncidentScope {
                    project_id: message.envelope.project_id,
                    employee_id: Some(message.envelope.recipient.employee_id),
                    task_id: message.envelope.task_id,
                    message_id: Some(message.envelope.id),
                    ..IncidentScope::default()
                },
                json!({
                    "failureCode": code,
                    "failureMessage": message.delivery.last_failure_message,
                    "deliveryState": message.delivery.state,
                }),
            );
        }
        for turn in turns {
            let code = turn.failure_code.as_deref().unwrap_or("LZ-TURN-OWNER-LOST");
            self.note_incident(
                &format!("owner-lost:turn:{}", turn.id),
                code,
                self.turn_scope(turn),
                json!({
                    "failureCode": code,
                    "failureMessage": turn.failure_message,
                    "turnState": turn.state,
                }),
            );
        }
        for task in tasks {
            let code = task
                .failure_code
                .as_deref()
                .unwrap_or("LZ-TASK-DISPATCH-OWNER-LOST");
            self.note_incident(
                &format!("owner-lost:task:{}", task.id),
                code,
                IncidentScope {
                    project_id: task.project_id,
                    employee_id: Some(task.assigned_employee_id),
                    task_id: Some(task.id),
                    ..IncidentScope::default()
                },
                json!({
                    "failureCode": code,
                    "failureMessage": task.failure_message,
                    "taskState": task.state,
                }),
            );
        }
        match self.store.reconcile_emergency_incidents() {
            Ok(raised) if raised.is_empty() => {}
            Ok(_) => {
                if let Err(error) = self.publish_events() {
                    tracing::warn!(message = %error.message, "emergency incident events not published");
                }
            }
            Err(error) => tracing::warn!(%error, "emergency incidents could not be folded"),
        }
    }

    pub(super) fn incident_list(&self, value: &Value) -> Result<Value, RpcError> {
        let params: IncidentListParams = incident_params(value)?;
        self.store
            .list_incidents(&ListIncidentsInput {
                after_sequence: params.after_sequence,
                limit: params.limit,
                state: params.state,
                priority: params.priority,
                code: params.code,
                employee_id: params.employee_id,
                project_id: params.project_id,
                escalation: params.escalation,
            })
            .map(to_value)
            .map_err(incident_store_error)
    }

    /// Closes every lead remediation window that passed by `now`: what the lead's remedy held
    /// becomes `remediated`, everything else unresolved becomes a priority notification for the
    /// user. Nothing here retries, restarts, or cancels anything.
    #[doc(hidden)]
    pub fn escalate_due_incidents_once(&self, now: DateTime<Utc>) -> Result<(), RpcError> {
        let changed = self
            .store
            .escalate_due_incidents(now)
            .map_err(incident_store_error)?;
        for incident in &changed {
            tracing::warn!(
                incident_id = %incident.id,
                code = %incident.code,
                escalation = ?incident.escalation,
                reason = ?incident.escalation_reason,
                "lead remediation window closed"
            );
        }
        if changed.is_empty() {
            Ok(())
        } else {
            self.publish_events()
        }
    }

    pub(super) fn incident_show(&self, value: &Value) -> Result<Value, RpcError> {
        let params: IncidentShowParams = incident_params(value)?;
        self.store
            .get_incident(params.id)
            .map(to_value)
            .map_err(incident_store_error)
    }

    pub(super) fn acknowledge_incident(&self, request: &JsonRpcRequest) -> Result<Value, RpcError> {
        let params: AcknowledgeIncidentParams = incident_params(&request.params)?;
        require_revision(params.expected_revision)?;
        let result = self
            .store
            .acknowledge_incident(&SettleIncidentInput {
                request_id: request.id.clone(),
                idempotency_key: params.idempotency_key,
                incident_id: params.id,
                expected_revision: params.expected_revision,
            })
            .map_err(incident_store_error)?;
        self.publish_events()?;
        Ok(to_value(result))
    }

    pub(super) fn resolve_incident(&self, request: &JsonRpcRequest) -> Result<Value, RpcError> {
        let params: ResolveIncidentParams = incident_params(&request.params)?;
        require_revision(params.expected_revision)?;
        let result = self
            .store
            .resolve_incident(&SettleIncidentInput {
                request_id: request.id.clone(),
                idempotency_key: params.idempotency_key,
                incident_id: params.id,
                expected_revision: params.expected_revision,
            })
            .map_err(incident_store_error)?;
        self.publish_events()?;
        Ok(to_value(result))
    }
}

/// Classifies a provider failure into the brief's incident taxonomy. The turn keeps its own
/// failure code; the incident code names what an operator should read first.
pub(super) const fn incident_code_for_provider_error(error: &ProviderError) -> &'static str {
    match error {
        ProviderError::Stalled { .. } => "LZ-SESSION-STALLED",
        ProviderError::Timeout { .. } => "LZ-MESSAGE-TIMEOUT",
        ProviderError::ProcessExited { .. } => "LZ-PROCESS-EXIT",
        ProviderError::Authentication(_) => "LZ-PROVIDER-AUTH",
        ProviderError::Protocol(_)
        | ProviderError::InvalidEvent(_)
        | ProviderError::Unsupported { .. } => "LZ-PROVIDER-INCOMPATIBLE",
        _ => "LZ-TURN-PROVIDER",
    }
}

pub(super) fn incident_code_for_dispatch_failure(code: &str) -> &str {
    match code {
        "LZ-TASK-WORKSPACE" => "LZ-WORKTREE-CONFLICT",
        "LZ-TASK-EMPLOYEE-STALE"
        | "LZ-TASK-PROJECT-STALE"
        | "LZ-TASK-PROJECT-NOT-FOUND"
        | "LZ-TASK-PROVIDER-UNSUPPORTED"
        | "LZ-TASK-AUTHORITY-UNSUPPORTED" => "LZ-CONFIG-INCOMPATIBLE",
        other => other,
    }
}

fn incident_params<T: serde::de::DeserializeOwned>(value: &Value) -> Result<T, RpcError> {
    serde_json::from_value(value.clone()).map_err(|error| incident_invalid(error.to_string()))
}

fn require_revision(revision: u64) -> Result<(), RpcError> {
    if revision == 0 {
        Err(incident_invalid(
            "expectedRevision must be greater than zero",
        ))
    } else {
        Ok(())
    }
}

fn incident_invalid(message: impl Into<String>) -> RpcError {
    RpcError {
        code: "LZ-INCIDENT-INVALID".to_owned(),
        message: message.into(),
        data: None,
    }
}

#[allow(clippy::needless_pass_by_value)] // Mirrors the other ledgers' error mappers.
pub(super) fn incident_store_error(error: StoreError) -> RpcError {
    let (code, data, message) = match &error {
        StoreError::IncidentNotFound { incident_id } => (
            "LZ-INCIDENT-NOT-FOUND",
            Some(json!({"incidentId": incident_id})),
            error.to_string(),
        ),
        StoreError::IncidentStale {
            incident_id,
            current_revision,
        } => (
            "LZ-INCIDENT-STALE",
            Some(json!({"incidentId": incident_id, "currentRevision": current_revision})),
            error.to_string(),
        ),
        StoreError::IncidentState { incident_id, .. } => (
            "LZ-INCIDENT-STATE",
            Some(json!({"incidentId": incident_id})),
            error.to_string(),
        ),
        StoreError::IncidentIdempotencyConflict => {
            ("LZ-INCIDENT-IDEMPOTENCY-CONFLICT", None, error.to_string())
        }
        StoreError::IncidentValidation(_) | StoreError::MessageValidation(_) => {
            ("LZ-INCIDENT-INVALID", None, error.to_string())
        }
        _ => (
            "LZ-INCIDENT-STORE",
            None,
            "incident store operation failed".to_owned(),
        ),
    };
    RpcError {
        code: code.to_owned(),
        message,
        data,
    }
}
