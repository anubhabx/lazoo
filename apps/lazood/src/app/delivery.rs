//! Lead notice delivery: folds terminal child tasks and promoted incidents into durable notice
//! messages and delivers each one to the lead as a provider turn.
//!
//! Activation policy: a notice is employee-authored traffic, so by default it never starts a
//! provider process. It is delivered when the recipient already has an idle session owned by
//! this daemon boot; otherwise it stays queued until the local user resumes the recipient with
//! `session.warm` or a new user turn, and the next pass delivers it. A recipient whose own
//! `activationPolicy` is `notice-turns` is instead activated here under fixed bounds: one
//! session per recorded notice turn, for the recipient's own task only, stopped as soon as that
//! turn ends. A failed activation is never retried; the turn then waits for the user.

use std::collections::HashSet;

use lazoo_protocol::{
    JsonRpcRequest, MessageDeliveryState, MessageRecord, RpcError, TaskState, TurnMutationResult,
    TurnState,
};
use lazoo_providers::ProviderKind;
use lazoo_store::{
    BeginSessionInput, CancelMessageInput, QueueIncidentNoticeInput, QueueTaskResultInput,
    RecordNoticeTurnInput, StoreError,
};
use serde_json::Value;
use tokio::sync::broadcast::error::RecvError;
use uuid::Uuid;

use super::{
    App,
    task::task_store_error,
    turn::{turn_failure, turn_store_error},
};

const PASS_LIMIT: u32 = 128;

impl App {
    /// Runs the notice loop until shutdown: one pass now, then one pass after every published
    /// event. Passes are cheap when nothing is pending, so waking on every event is fine.
    pub async fn run_notice_delivery(&self) {
        let mut shutdown = self.shutdown_receiver();
        let mut events = self.events.subscribe();
        loop {
            if *shutdown.borrow() {
                return;
            }
            if let Err(error) = Box::pin(self.deliver_pending_notices_once()).await {
                tracing::warn!(code = %error.code, message = %error.message, "lead notice delivery failed");
            }
            tokio::select! {
                received = events.recv() => {
                    if matches!(received, Err(RecvError::Closed)) {
                        return;
                    }
                }
                changed = shutdown.changed() => {
                    if changed.is_err() || *shutdown.borrow() {
                        return;
                    }
                }
            }
        }
    }

    /// One delivery pass: queue results for terminal children, queue notices for promoted
    /// incidents, then deliver queued notices whose lead is idle and free, every notice for
    /// one recipient and task as a single turn. Delivery is sequential; a notice whose lead
    /// is busy, or dormant under a `manual` activation policy, is skipped without error.
    #[doc(hidden)]
    pub async fn deliver_pending_notices_once(&self) -> Result<(), RpcError> {
        let mut first_error = None;
        for child_task_id in self
            .store
            .pending_task_results(PASS_LIMIT)
            .map_err(task_store_error)?
        {
            let queued = self.store.queue_task_result(&QueueTaskResultInput {
                request_id: format!("task-result:{child_task_id}"),
                child_task_id,
            });
            match queued {
                Ok(_) => self.publish_events()?,
                Err(error) => {
                    tracing::debug!(%child_task_id, %error, "child result could not be queued");
                    first_error.get_or_insert_with(|| task_store_error(error));
                }
            }
        }
        for incident_id in self
            .store
            .pending_incident_notices(PASS_LIMIT)
            .map_err(task_store_error)?
        {
            let queued = self.store.queue_incident_notice(&QueueIncidentNoticeInput {
                request_id: format!("incident-notice:{incident_id}"),
                incident_id,
            });
            match queued {
                Ok(_) => self.publish_events()?,
                Err(StoreError::IncidentNoticeNotApplicable { .. }) => {}
                Err(error) => {
                    tracing::debug!(%incident_id, %error, "incident notice could not be queued");
                    first_error.get_or_insert_with(|| task_store_error(error));
                }
            }
        }
        // Notices queued for the same recipient and task share one turn; the first member
        // handled in a pass delivers (or holds) the turn for the rest.
        let mut handled_turns = HashSet::new();
        for notice in self
            .store
            .pending_lead_notices(PASS_LIMIT)
            .map_err(task_store_error)?
        {
            if let Err(error) =
                Box::pin(self.deliver_lead_notice(&notice, &mut handled_turns)).await
            {
                tracing::warn!(
                    message_id = %notice.envelope.id,
                    code = %error.code,
                    message = %error.message,
                    "lead notice delivery failed"
                );
                first_error.get_or_insert(error);
            }
        }
        first_error.map_or(Ok(()), Err)
    }

    async fn deliver_lead_notice(
        &self,
        notice: &MessageRecord,
        handled_turns: &mut HashSet<Uuid>,
    ) -> Result<(), RpcError> {
        let Some(task_id) = notice.envelope.task_id else {
            return Ok(());
        };
        // An earlier turn in this pass may have carried this notice already.
        let notice = &self
            .store
            .get_message(notice.envelope.id)
            .map_err(task_store_error)?;
        if notice.delivery.state != MessageDeliveryState::Queued {
            return Ok(());
        }
        let task = self.store.get_task(task_id).map_err(task_store_error)?.task;
        if matches!(
            task.state,
            TaskState::Completed | TaskState::Failed | TaskState::Cancelled | TaskState::Uncertain
        ) {
            // Nobody is left to act on the notice; keep the record, close the delivery.
            return self.cancel_notice(notice, "the lead task is already terminal");
        }
        if !matches!(task.state, TaskState::Accepted | TaskState::Running)
            || task.active_turn_id.is_some()
        {
            return Ok(());
        }
        let recorded = match self.store.record_notice_turn(&RecordNoticeTurnInput {
            request_id: format!("notice-turn:{}", notice.envelope.id),
            message_id: notice.envelope.id,
        }) {
            Ok(recorded) => recorded,
            // Another turn on the lead's task is still unfinished; deliver after it.
            Err(StoreError::TurnTaskScope { .. }) => return Ok(()),
            Err(error) => return Err(turn_store_error(error)),
        };
        self.publish_events()?;
        if recorded.turn.state != TurnState::Recorded || !handled_turns.insert(recorded.turn.id) {
            return Ok(());
        }
        let request = JsonRpcRequest {
            jsonrpc: "2.0".to_owned(),
            id: format!("notice-delivery:{}", notice.envelope.id),
            method: "turn.deliver".to_owned(),
            params: Value::Null,
        };
        match self
            .deliver_recorded_turn(&request, recorded.clone(), false)
            .await
        {
            Ok(_) => Ok(()),
            // The lead is dormant: its own activation policy decides whether this turn wakes it.
            Err(error) if error.code == "LZ-TURN-SESSION-NOT-READY" => {
                Box::pin(self.activate_for_notice_turn(&request, notice, recorded)).await
            }
            // The lead is not route ready, or its session is not owned by this boot: wait for
            // the user to resume it.
            Err(error)
                if matches!(
                    error.code.as_str(),
                    "LZ-TURN-EMPLOYEE-NOT-READY" | "LZ-TURN-SESSION-HANDLE-MISSING"
                ) =>
            {
                tracing::debug!(
                    message_id = %notice.envelope.id,
                    code = %error.code,
                    "lead notice waits for the lead to be resumed"
                );
                Ok(())
            }
            // The lead was reconfigured or disabled underneath the notice; it cannot be
            // delivered to that recipient any more, and the user made that change.
            Err(error)
                if matches!(
                    error.code.as_str(),
                    "LZ-TURN-EMPLOYEE-STALE" | "LZ-TURN-EMPLOYEE-DISABLED"
                ) =>
            {
                self.cancel_notice(notice, &error.message)
            }
            // Provider failures are already on the turn, message, and incident ledgers.
            Err(error) => Err(error),
        }
    }

    /// Bounded automatic activation for one recorded notice turn.
    ///
    /// The recipient's own `activationPolicy` decides; `manual` waits for the user exactly as
    /// before. The `begin_session` command key `notice-activation:<turnId>` makes a second
    /// attempt for the same turn a replay, so a turn that is still `recorded` after its one
    /// activation (the start failed, or the daemon restarted mid-turn) waits for the user
    /// instead of being retried. The session serves only this turn, whose task
    /// `record_notice_turn` already fixed to the recipient, and is stopped right after it.
    async fn activate_for_notice_turn(
        &self,
        request: &JsonRpcRequest,
        notice: &MessageRecord,
        recorded: TurnMutationResult,
    ) -> Result<(), RpcError> {
        let turn_id = recorded.turn.id;
        let employee = self
            .store
            .get_employee(recorded.turn.recipient.employee_id)
            .map_err(turn_store_error)?;
        if !lazoo_domain::notice_turn_may_activate(employee.activation_policy) {
            tracing::debug!(
                message_id = %notice.envelope.id,
                %turn_id,
                "lead notice waits for the lead to be resumed"
            );
            return Ok(());
        }
        let provider = employee.provider.parse::<ProviderKind>().map_err(|_| {
            turn_failure(
                "LZ-TURN-PROVIDER-UNSUPPORTED",
                "the recipient employee has an unknown provider",
                turn_id,
            )
        })?;
        let activation_key = format!("notice-activation:{turn_id}");
        let starting = match self.store.begin_session(&BeginSessionInput {
            request_id: format!("{}:activate", request.id),
            idempotency_key: activation_key.clone(),
            employee_id: employee.id,
            expected_employee_revision: employee.configuration_revision,
            owner_boot_id: self.boot_id,
        }) {
            Ok(starting) => starting,
            // A session of the recipient is still starting or stopping, or the provider is no
            // longer route ready: the next pass delivers on that session or keeps waiting.
            Err(
                StoreError::SessionConflict { .. } | StoreError::SessionProviderNotReady { .. },
            ) => return Ok(()),
            Err(error) => return Err(turn_store_error(error)),
        };
        self.publish_events()?;
        if starting.event_sequence.is_none() {
            // This turn already used its one activation; what became of that session is on
            // the session and incident ledgers, and only the user resumes the recipient now.
            tracing::debug!(
                message_id = %notice.envelope.id,
                %turn_id,
                session_id = %starting.session.id,
                "lead notice already activated its recipient once and waits for the user"
            );
            return Ok(());
        }
        tracing::info!(
            employee_id = %employee.id,
            %turn_id,
            session_id = %starting.session.id,
            "notice turn activates its dormant recipient by policy"
        );
        let ready = self
            .start_new_session(employee, provider, starting, None)
            .await?;
        let delivered = self.deliver_recorded_turn(request, recorded, false).await;
        // The activated session serves this one turn; stop it whether or not the turn succeeded.
        let _ = self
            .stop_session_command(
                ready.session.id,
                ready.session.revision,
                format!("{activation_key}:stop"),
                format!("{activation_key}:stop"),
            )
            .await;
        delivered.map(|_| ())
    }

    fn cancel_notice(&self, notice: &MessageRecord, reason: &str) -> Result<(), RpcError> {
        tracing::info!(message_id = %notice.envelope.id, reason, "lead notice cancelled");
        self.store
            .cancel_message(&CancelMessageInput {
                request_id: format!("notice-cancel:{}", notice.envelope.id),
                idempotency_key: format!("notice-cancel:{}", notice.envelope.id),
                message_id: notice.envelope.id,
                expected_revision: notice.delivery.revision,
            })
            .map_err(task_store_error)?;
        self.publish_events()
    }
}
