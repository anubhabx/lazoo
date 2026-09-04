use lazoo_protocol::{
    AcceptTaskParams, CancelTaskParams, DelegateTaskParams, JsonRpcRequest, ReportTaskParams,
    ResolveTaskParams, RpcError, StartTaskParams, TaskListParams, TaskShowParams,
};
use lazoo_store::{
    AcceptTaskInput, CancelTaskInput, DelegateTaskInput, ListTasksInput, ReportTaskInput,
    ResolveTaskInput, StartTaskInput, StoreError,
};
use serde_json::{Value, json};

use super::{App, to_value};

impl App {
    pub(super) fn delegate_task(&self, request: &JsonRpcRequest) -> Result<Value, RpcError> {
        let params: DelegateTaskParams = task_params(&request.params)?;
        let result = self
            .store
            .delegate_task(&DelegateTaskInput {
                request_id: request.id.clone(),
                idempotency_key: params.idempotency_key,
                parent_task_id: params.parent_task_id,
                actor: params.actor,
                assigned_employee_id: params.assigned_employee_id,
                objective: params.objective,
                references: params.references,
                requested_authority: params.requested_authority,
                roster_revision: params.roster_revision,
            })
            .map_err(task_store_error)?;
        self.publish_events()?;
        Ok(to_value(result))
    }

    pub(super) fn accept_task(&self, request: &JsonRpcRequest) -> Result<Value, RpcError> {
        let params: AcceptTaskParams = task_params(&request.params)?;
        require_revision(params.expected_revision)?;
        let result = self
            .store
            .accept_task(&AcceptTaskInput {
                request_id: request.id.clone(),
                idempotency_key: params.idempotency_key,
                task_id: params.id,
                expected_revision: params.expected_revision,
            })
            .map_err(task_store_error)?;
        self.publish_events()?;
        Ok(to_value(result))
    }

    pub(super) fn start_task(&self, request: &JsonRpcRequest) -> Result<Value, RpcError> {
        let params: StartTaskParams = task_params(&request.params)?;
        require_revision(params.expected_revision)?;
        let result = self
            .store
            .start_task(&StartTaskInput {
                request_id: request.id.clone(),
                idempotency_key: params.idempotency_key,
                task_id: params.id,
                expected_revision: params.expected_revision,
                actor: params.actor,
                turn_id: params.turn_id,
                evidence: params.evidence,
            })
            .map_err(task_store_error)?;
        self.publish_events()?;
        Ok(to_value(result))
    }

    pub(super) fn report_task(&self, request: &JsonRpcRequest) -> Result<Value, RpcError> {
        let params: ReportTaskParams = task_params(&request.params)?;
        require_revision(params.expected_revision)?;
        let result = self
            .store
            .report_task(&ReportTaskInput {
                request_id: request.id.clone(),
                idempotency_key: params.idempotency_key,
                task_id: params.id,
                expected_revision: params.expected_revision,
                actor: params.actor,
                kind: params.kind,
                evidence: params.evidence,
                failure_code: params.failure_code,
                failure_message: params.failure_message,
            })
            .map_err(task_store_error)?;
        self.publish_events()?;
        Ok(to_value(result))
    }

    pub(super) fn cancel_task(&self, request: &JsonRpcRequest) -> Result<Value, RpcError> {
        let params: CancelTaskParams = task_params(&request.params)?;
        require_revision(params.expected_revision)?;
        let result = self
            .store
            .cancel_task(&CancelTaskInput {
                request_id: request.id.clone(),
                idempotency_key: params.idempotency_key,
                task_id: params.id,
                expected_revision: params.expected_revision,
                actor: params.actor,
                evidence: params.evidence,
            })
            .map_err(task_store_error)?;
        self.publish_events()?;
        Ok(to_value(result))
    }

    pub(super) fn resolve_task(&self, request: &JsonRpcRequest) -> Result<Value, RpcError> {
        let params: ResolveTaskParams = task_params(&request.params)?;
        require_revision(params.expected_revision)?;
        let result = self
            .store
            .resolve_task(&ResolveTaskInput {
                request_id: request.id.clone(),
                idempotency_key: params.idempotency_key,
                task_id: params.id,
                expected_revision: params.expected_revision,
                resolution: params.resolution,
                evidence: params.evidence,
                failure_code: params.failure_code,
                failure_message: params.failure_message,
            })
            .map_err(task_store_error)?;
        self.publish_events()?;
        Ok(to_value(result))
    }

    pub(super) fn task_list(&self, value: &Value) -> Result<Value, RpcError> {
        let params: TaskListParams = task_params(value)?;
        if params.limit == 0 || params.limit > 1024 {
            return Err(task_invalid("limit must be between 1 and 1024"));
        }
        self.store
            .list_tasks(&ListTasksInput {
                after_sequence: params.after_sequence,
                limit: params.limit,
                root_task_id: params.root_task_id,
                parent_task_id: params.parent_task_id,
                conversation_id: params.conversation_id,
                project_id: params.project_id,
                assigned_employee_id: params.assigned_employee_id,
                state: params.state,
            })
            .map(to_value)
            .map_err(task_store_error)
    }

    pub(super) fn task_show(&self, value: &Value) -> Result<Value, RpcError> {
        let params: TaskShowParams = task_params(value)?;
        self.store
            .get_task(params.id)
            .map(to_value)
            .map_err(task_store_error)
    }
}

fn task_params<T: serde::de::DeserializeOwned>(value: &Value) -> Result<T, RpcError> {
    serde_json::from_value(value.clone()).map_err(|error| task_invalid(error.to_string()))
}

fn require_revision(revision: u64) -> Result<(), RpcError> {
    if revision == 0 {
        Err(task_invalid("expectedRevision must be greater than zero"))
    } else {
        Ok(())
    }
}

fn task_invalid(message: impl Into<String>) -> RpcError {
    RpcError {
        code: "LZ-TASK-INVALID".to_owned(),
        message: message.into(),
        data: None,
    }
}

#[allow(clippy::needless_pass_by_value, clippy::too_many_lines)]
fn task_store_error(error: StoreError) -> RpcError {
    let (code, data, message) = match &error {
        StoreError::TaskNotFound { task_id } => (
            "LZ-TASK-NOT-FOUND",
            Some(json!({"taskId": task_id})),
            error.to_string(),
        ),
        StoreError::TaskStale {
            task_id,
            current_revision,
        } => (
            "LZ-TASK-STALE",
            Some(json!({"taskId": task_id, "currentRevision": current_revision})),
            error.to_string(),
        ),
        StoreError::TaskState { task_id, .. } => (
            "LZ-TASK-STATE",
            Some(json!({"taskId": task_id})),
            error.to_string(),
        ),
        StoreError::TaskActiveChildren { task_id } => (
            "LZ-TASK-CHILDREN-ACTIVE",
            Some(json!({"taskId": task_id})),
            error.to_string(),
        ),
        StoreError::TaskDepthLimit { parent_task_id } => (
            "LZ-TASK-DEPTH-LIMIT",
            Some(json!({"parentTaskId": parent_task_id})),
            error.to_string(),
        ),
        StoreError::TaskFanoutLimit { parent_task_id } => (
            "LZ-TASK-FANOUT-LIMIT",
            Some(json!({"parentTaskId": parent_task_id})),
            error.to_string(),
        ),
        StoreError::TaskAncestryMismatch { task_id } => (
            "LZ-TASK-ANCESTRY",
            Some(json!({"taskId": task_id})),
            error.to_string(),
        ),
        StoreError::TaskActorMismatch {
            task_id,
            employee_id,
        } => (
            "LZ-TASK-ACTOR",
            Some(json!({"taskId": task_id, "employeeId": employee_id})),
            error.to_string(),
        ),
        StoreError::TaskActorSession { employee_id } => (
            "LZ-TASK-ACTOR-SESSION",
            Some(json!({"employeeId": employee_id})),
            error.to_string(),
        ),
        StoreError::TaskAssigneeUnavailable { employee_id } => (
            "LZ-TASK-ASSIGNEE-UNAVAILABLE",
            Some(json!({"employeeId": employee_id})),
            error.to_string(),
        ),
        StoreError::TaskSelfDelegation { employee_id } => (
            "LZ-TASK-SELF-DELEGATION",
            Some(json!({"employeeId": employee_id})),
            error.to_string(),
        ),
        StoreError::TaskEmployeeStale {
            employee_id,
            current_revision,
        } => (
            "LZ-TASK-EMPLOYEE-STALE",
            Some(json!({"employeeId": employee_id, "currentRevision": current_revision})),
            error.to_string(),
        ),
        StoreError::TaskRosterStale { current_revision } => (
            "LZ-TASK-ROSTER-STALE",
            Some(json!({"currentRevision": current_revision})),
            error.to_string(),
        ),
        StoreError::TaskAuthorityExpansion { requested } => (
            "LZ-TASK-AUTHORITY-EXPANSION",
            Some(json!({"requestedAuthority": requested})),
            error.to_string(),
        ),
        StoreError::TaskAcceptanceEvidence { task_id, turn_id } => (
            "LZ-TASK-ACCEPTANCE-EVIDENCE",
            Some(json!({"taskId": task_id, "turnId": turn_id})),
            error.to_string(),
        ),
        StoreError::TaskIdempotencyConflict => {
            ("LZ-TASK-IDEMPOTENCY-CONFLICT", None, error.to_string())
        }
        StoreError::TaskTurnLink { task_id, turn_id } => (
            "LZ-TASK-SCOPE",
            Some(json!({"taskId": task_id, "turnId": turn_id})),
            error.to_string(),
        ),
        StoreError::TaskValidation(_)
        | StoreError::TaskFailureDetails
        | StoreError::TaskListLimit => ("LZ-TASK-INVALID", None, error.to_string()),
        _ => (
            "LZ-TASK-STORE",
            None,
            "task store operation failed".to_owned(),
        ),
    };
    RpcError {
        code: code.to_owned(),
        message,
        data,
    }
}
