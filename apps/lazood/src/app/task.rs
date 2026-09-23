use lazoo_protocol::{
    AcceptTaskParams, CancelTaskParams, DelegateTaskParams, JsonRpcRequest, MessageDeliveryState,
    ReportTaskParams, ResolveTaskParams, RpcError, StartTaskParams, TaskListParams, TaskShowParams,
    TaskState, TaskWorkspaceKind,
};
use lazoo_providers::ProviderKind;
use lazoo_store::{
    AcceptTaskInput, BeginSessionInput, BeginTurnSubmissionInput, CancelTaskInput,
    ClaimTaskDispatchInput, DeferTaskDispatchInput, DelegateTaskInput, FailTaskDispatchInput,
    ListTasksInput, MarkTaskDispatchWorkspaceReadyInput, RecordTaskDispatchTurnInput,
    ReportTaskInput, ResolveTaskInput, StartTaskInput, StoreError,
};
use serde_json::{Value, json};

use super::{App, LiveTurn, to_value};

impl App {
    pub async fn run_task_dispatcher(&self) {
        let mut shutdown = self.shutdown_receiver();
        loop {
            if *shutdown.borrow() {
                return;
            }
            if let Err(error) = self.dispatch_pending_tasks_once().await {
                tracing::warn!(code = %error.code, message = %error.message, "automatic task dispatch failed");
            }
            tokio::select! {
                () = self.task_dispatch_notify.notified() => {}
                changed = shutdown.changed() => {
                    if changed.is_err() || *shutdown.borrow() {
                        return;
                    }
                }
            }
        }
    }

    #[doc(hidden)]
    pub async fn dispatch_pending_tasks_once(&self) -> Result<(), RpcError> {
        let task_ids = self
            .store
            .pending_task_dispatches(128)
            .map_err(task_store_error)?;
        let saturated = task_ids.len() == 128;
        let mut first_error = None;
        for task_id in task_ids {
            if let Err(error) = self.dispatch_task(task_id).await {
                let error = self
                    .fail_claimed_dispatch_before_provider_io(task_id, &error)
                    .err()
                    .unwrap_or(error);
                first_error.get_or_insert(error);
            }
        }
        if saturated {
            self.task_dispatch_notify.notify_one();
        }
        first_error.map_or(Ok(()), Err)
    }

    fn fail_claimed_dispatch_before_provider_io(
        &self,
        task_id: uuid::Uuid,
        error: &RpcError,
    ) -> Result<(), RpcError> {
        let detail = self.store.get_task(task_id).map_err(task_store_error)?;
        let Some(dispatch) = detail.dispatch else {
            return Ok(());
        };
        if detail.task.state != TaskState::Accepted || dispatch.owner_boot_id != self.boot_id {
            return Ok(());
        }
        let message = self
            .store
            .get_message(dispatch.message_id)
            .map_err(task_store_error)?;
        if message.delivery.state != MessageDeliveryState::Queued {
            return Ok(());
        }
        let failed = self
            .store
            .fail_task_dispatch(&FailTaskDispatchInput {
                task_id,
                owner_boot_id: self.boot_id,
                failure_code: error.code.clone(),
                failure_message: error.message.clone(),
            })
            .map_err(task_store_error)?;
        self.publish_events()?;
        self.note_incident(
            &format!("dispatch-failure:{task_id}:{}", failed.dispatch.revision),
            super::incident::incident_code_for_dispatch_failure(&error.code),
            super::incident::IncidentScope {
                project_id: failed.task.project_id,
                employee_id: Some(failed.task.assigned_employee_id),
                task_id: Some(task_id),
                message_id: Some(failed.dispatch.message_id),
                ..super::incident::IncidentScope::default()
            },
            json!({
                "failureCode": error.code,
                "failureMessage": error.message,
                "dispatchState": failed.dispatch.state,
            }),
        );
        Ok(())
    }

    async fn dispatch_task(&self, task_id: uuid::Uuid) -> Result<(), RpcError> {
        let claimed = self
            .store
            .claim_task_dispatch(&ClaimTaskDispatchInput {
                task_id,
                owner_boot_id: self.boot_id,
            })
            .map_err(task_store_error)?;
        self.publish_events()?;
        let task = self.store.get_task(task_id).map_err(task_store_error)?.task;
        let employee = self
            .store
            .get_employee(task.assigned_employee_id)
            .map_err(task_store_error)?;
        if employee.configuration_revision != task.employee_configuration_revision {
            return Err(task_failure(
                "LZ-TASK-EMPLOYEE-STALE",
                "the assigned employee configuration changed before dispatch",
                task_id,
            ));
        }
        let (workspace_kind, workspace_path, base_commit, branch_name) = self
            .prepare_task_dispatch_workspace(
                task_id,
                task.project_id,
                claimed.project_configuration_revision,
            )?;
        let workspace_ready = self
            .store
            .mark_task_dispatch_workspace_ready(&MarkTaskDispatchWorkspaceReadyInput {
                task_id,
                owner_boot_id: self.boot_id,
                expected_revision: claimed.revision,
                workspace_kind,
                workspace_path: workspace_path.to_string_lossy().into_owned(),
                base_commit,
                branch_name,
            })
            .map_err(task_store_error)?;
        self.publish_events()?;

        let provider = employee.provider.parse::<ProviderKind>().map_err(|_| {
            task_failure(
                "LZ-TASK-PROVIDER-UNSUPPORTED",
                "the assigned employee provider is unsupported",
                task_id,
            )
        })?;
        if provider != ProviderKind::Codex {
            return Err(task_failure(
                "LZ-TASK-AUTHORITY-UNSUPPORTED",
                "the selected provider cannot enforce this task authority ceiling",
                task_id,
            ));
        }
        let starting = match self.store.begin_session(&BeginSessionInput {
            request_id: format!("task-dispatch:{task_id}:session"),
            idempotency_key: format!("task-dispatch:{task_id}:session"),
            employee_id: employee.id,
            expected_employee_revision: employee.configuration_revision,
            owner_boot_id: self.boot_id,
        }) {
            Ok(starting) => starting,
            Err(StoreError::SessionConflict { session_id, .. }) => {
                self.store
                    .defer_task_dispatch(&DeferTaskDispatchInput {
                        task_id,
                        owner_boot_id: self.boot_id,
                        expected_revision: workspace_ready.revision,
                        active_session_id: session_id,
                    })
                    .map_err(task_store_error)?;
                self.publish_events()?;
                return Ok(());
            }
            Err(error) => return Err(task_store_error(error)),
        };
        self.publish_events()?;
        let ready = if starting.event_sequence.is_some() {
            self.start_new_session(employee, provider, starting, Some(workspace_path))
                .await?
        } else {
            starting
        };
        let dispatch = self
            .deliver_ready_dispatch(task_id, workspace_ready.revision, &ready)
            .await;
        let _ = self
            .stop_session_command(
                ready.session.id,
                ready.session.revision,
                format!("task-dispatch:{task_id}:stop"),
                format!("task-dispatch:{task_id}:stop"),
            )
            .await;
        dispatch
    }

    fn prepare_task_dispatch_workspace(
        &self,
        task_id: uuid::Uuid,
        project_id: Option<uuid::Uuid>,
        project_configuration_revision: Option<u64>,
    ) -> Result<
        (
            TaskWorkspaceKind,
            std::path::PathBuf,
            Option<String>,
            Option<String>,
        ),
        RpcError,
    > {
        let Some(project_id) = project_id else {
            let path = self
                .root
                .ensure_projectless_task_working_dir(task_id)
                .map_err(|_| {
                    task_failure(
                        "LZ-TASK-WORKSPACE",
                        "the projectless task workspace could not be prepared",
                        task_id,
                    )
                })?;
            return Ok((TaskWorkspaceKind::Projectless, path, None, None));
        };
        let project = self
            .store
            .list_projects()
            .map_err(task_store_error)?
            .into_iter()
            .find(|project| project.id == project_id)
            .ok_or_else(|| {
                task_failure(
                    "LZ-TASK-PROJECT-NOT-FOUND",
                    "the dispatch project no longer exists",
                    task_id,
                )
            })?;
        if Some(project.configuration_revision) != project_configuration_revision {
            return Err(task_failure(
                "LZ-TASK-PROJECT-STALE",
                "the dispatch project configuration changed",
                task_id,
            ));
        }
        let prepared = lazoo_runtime::prepare_git_task_worktree(
            std::path::Path::new(&project.canonical_path),
            &self.root.git_task_working_dir(project_id, task_id),
            task_id,
        )
        .map_err(|_| {
            task_failure(
                "LZ-TASK-WORKSPACE",
                "the isolated Git task worktree could not be prepared",
                task_id,
            )
        })?;
        Ok((
            TaskWorkspaceKind::GitWorktree,
            prepared.path,
            Some(prepared.base_commit),
            Some(prepared.branch_name),
        ))
    }

    async fn deliver_ready_dispatch(
        &self,
        task_id: uuid::Uuid,
        dispatch_revision: u64,
        ready: &lazoo_protocol::SessionMutationResult,
    ) -> Result<(), RpcError> {
        let recorded = self
            .store
            .record_task_dispatch_turn(&RecordTaskDispatchTurnInput {
                task_id,
                owner_boot_id: self.boot_id,
                expected_dispatch_revision: dispatch_revision,
                session_id: ready.session.id,
            })
            .map_err(task_store_error)?;
        self.publish_events()?;
        let session = self
            .live_sessions
            .lock()
            .await
            .get(&ready.session.id)
            .cloned()
            .ok_or_else(|| {
                task_failure(
                    "LZ-TASK-SESSION-HANDLE-MISSING",
                    "the daemon does not own the dispatched task session",
                    task_id,
                )
            })?;
        let live_turn = std::sync::Arc::new(LiveTurn::new(session));
        self.live_turns
            .lock()
            .await
            .insert(recorded.turn.id, live_turn.clone());
        let submitting = match self.store.begin_turn_submission(&BeginTurnSubmissionInput {
            request_id: format!("task-dispatch:{task_id}:begin"),
            idempotency_key: format!("task-dispatch:{task_id}:begin"),
            turn_id: recorded.turn.id,
            expected_revision: recorded.turn.revision,
            owner_boot_id: self.boot_id,
            recipient_session_id: ready.session.id,
        }) {
            Ok(submitting) => submitting,
            Err(error) => {
                self.live_turns.lock().await.remove(&recorded.turn.id);
                return Err(task_store_error(error));
            }
        };
        self.publish_events()?;
        let delivery = self
            .deliver_registered_submitting_turn(
                &format!("task-dispatch:{task_id}:deliver"),
                submitting,
                live_turn,
            )
            .await;
        delivery.map(|_| ())
    }

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
        if result.task.state == lazoo_protocol::TaskState::Accepted {
            self.task_dispatch_notify.notify_one();
        }
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
        if result.task.state == lazoo_protocol::TaskState::Accepted {
            self.task_dispatch_notify.notify_one();
        }
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

    pub(super) async fn cancel_task(&self, request: &JsonRpcRequest) -> Result<Value, RpcError> {
        let params: CancelTaskParams = task_params(&request.params)?;
        require_revision(params.expected_revision)?;
        let mut result = self
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
        if result.task.state == TaskState::CancelRequested {
            let detail = self
                .store
                .get_task(result.task.id)
                .map_err(task_store_error)?;
            if let Some(dispatch) = detail.dispatch
                && let Some(turn_id) = result.task.active_turn_id.or(dispatch.turn_id)
                && dispatch.turn_id == Some(turn_id)
            {
                let turn = self.store.get_turn(turn_id).map_err(task_store_error)?;
                let cancel_request = JsonRpcRequest {
                    jsonrpc: "2.0".to_owned(),
                    id: format!("{}:automatic-turn-cancel", request.id),
                    method: "turn.cancel".to_owned(),
                    params: json!({
                        "id": turn_id,
                        "expectedRevision": turn.revision,
                        "idempotencyKey": format!("task:{}:automatic-turn-cancel", result.task.id)
                    }),
                };
                self.cancel_turn(&cancel_request).await?;
                result.task = self
                    .store
                    .get_task(result.task.id)
                    .map_err(task_store_error)?
                    .task;
            }
        }
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

fn task_failure(code: &str, message: &str, task_id: uuid::Uuid) -> RpcError {
    RpcError {
        code: code.to_owned(),
        message: message.to_owned(),
        data: Some(json!({"taskId": task_id})),
    }
}

#[allow(clippy::needless_pass_by_value, clippy::too_many_lines)]
pub(super) fn task_store_error(error: StoreError) -> RpcError {
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
