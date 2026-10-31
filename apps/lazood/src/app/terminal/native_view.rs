//! Read-only native TUI launch plumbing.  This is intentionally below the terminal RPC layer so
//! neither the remote URL nor its bearer capability can enter protocol records.

use std::sync::Arc;

use lazoo_protocol::{
    RpcError, SessionLifecycleState, SessionTerminalViewParams, SessionTerminalViewResult,
    TerminalOpenParams, TerminalRecord, TerminalStopParams,
};
use lazoo_runtime::TerminalCommand;
use tokio::sync::watch;
use uuid::Uuid;

use super::{
    App, MAX_COMMAND_KEYS, ViewBinding, ViewKey, ViewReservation, attach_failed, record,
    release_exited,
};

impl App {
    #[allow(clippy::too_many_lines)] // One ordered reservation, ownership fence, launch, and cleanup transaction.
    pub(crate) async fn session_terminal_view(
        &self,
        params: serde_json::Value,
    ) -> Result<serde_json::Value, RpcError> {
        let params: SessionTerminalViewParams = serde_json::from_value(params)
            .map_err(|error| super::super::invalid_params(error.to_string()))?;
        super::super::require_session_idempotency_key(&params.idempotency_key)?;
        if params.expected_revision == 0 {
            return Err(super::super::session_invalid(
                "expectedRevision must be greater than zero",
            ));
        }
        let session = self
            .store
            .get_session(params.id)
            .map_err(super::super::session_store_error)?;
        self.validate_view_session(&session, &params)?;
        let (mut result, owner) = self.reserve_view(session.id, &params).await?;
        if !owner {
            let terminal = loop {
                if let Some(outcome) = result.borrow().clone() {
                    break outcome;
                }
                result.changed().await.map_err(|_| {
                    attach_failed("native view open operation was cancelled", session.id)
                })?;
            }
            .map_err(|error| attach_failed(&error, session.id))?;
            return Ok(super::super::to_value(SessionTerminalViewResult {
                session_id: session.id,
                terminal,
            }));
        }

        // Acquiring a lease may bind a listener and do provider I/O.  Re-check the exact store
        // record and Arc identity afterwards; a stop/replacement must win this race.
        let live = self.live_sessions.lock().await.get(&session.id).cloned();
        let Some(live) = live else {
            let error = super::super::session_failure(
                "LZ-SESSION-HANDLE-MISSING",
                "the daemon does not own the active session handle",
                session.id,
            );
            self.complete_view(session.id, &params, Err(error.message.clone()))
                .await;
            return Err(error);
        };
        let lease = match live.open_native_view().await {
            Ok(Some(lease)) => Arc::new(lease),
            Ok(None) => {
                self.complete_view(
                    session.id,
                    &params,
                    Err("native views are unsupported by this provider".to_owned()),
                )
                .await;
                return Err(super::super::session_failure(
                    "LZ-SESSION-VIEW-UNSUPPORTED",
                    "the provider does not support a native read-only view",
                    session.id,
                ));
            }
            Err(error) => {
                self.complete_view(session.id, &params, Err(error.to_string()))
                    .await;
                return Err(attach_failed(&error.to_string(), session.id));
            }
        };
        if let Err(error) = self.recheck_view_owner(&session, &params, &live).await {
            lease.close();
            self.complete_view(session.id, &params, Err(error.message.clone()))
                .await;
            return Err(error);
        }
        let descriptor = lease.launch_descriptor();
        let borrowed_terminal = descriptor.existing_terminal();
        let command = descriptor.command();
        let open = TerminalOpenParams {
            executable: command.executable.to_string_lossy().into_owned(),
            args: command
                .args
                .iter()
                .map(|arg| arg.to_string_lossy().into_owned())
                .collect(),
            cwd: command.cwd.to_string_lossy().into_owned(),
            columns: params.columns,
            rows: params.rows,
            idempotency_key: format!("session-view:{}:{}", session.id, params.idempotency_key),
        };
        let opened = if let Some(canonical) = borrowed_terminal {
            self.terminal_open_borrowed(open, canonical, Some(Arc::clone(&lease)))
                .await
        } else {
            let mut terminal_command = TerminalCommand::new(
                &command.executable,
                &command.cwd,
                params.columns,
                params.rows,
            )
            .args(command.args);
            for (name, value) in command.env {
                terminal_command = terminal_command.env(name, value);
            }
            for name in command.env_remove {
                terminal_command = terminal_command.env_remove(name);
            }
            terminal_command = terminal_command
                .env("TERM", "xterm-256color")
                .env("COLORTERM", "truecolor");
            self.terminal_open_with_command(open, terminal_command, Some(Arc::clone(&lease)))
                .await
        };
        let terminal = match opened {
            Ok(terminal) => terminal,
            Err(error) => {
                lease.close();
                self.complete_view(session.id, &params, Err(error.clone()))
                    .await;
                return Err(attach_failed(&error, session.id));
            }
        };
        // The monitor holds only the receiver and lifecycle fence. For a borrowed record,
        // `terminal_stop` releases the logical view and never stops the canonical ConPTY.
        let mut closed = lease.closed_subscription();
        let app = self.clone();
        let monitor_terminal = terminal.clone();
        tokio::spawn(async move {
            if !*closed.borrow_and_update() {
                let _ = closed.changed().await;
            }
            let _ = app
                .terminal_stop(TerminalStopParams {
                    id: monitor_terminal.id,
                    expected_boot_id: monitor_terminal.boot_id,
                    expected_revision: monitor_terminal.revision,
                })
                .await;
        });
        if lease.is_closed() {
            let error = "native view closed while opening".to_owned();
            let _ = self
                .terminal_stop(TerminalStopParams {
                    id: terminal.id,
                    expected_boot_id: terminal.boot_id,
                    expected_revision: terminal.revision,
                })
                .await;
            self.complete_view(session.id, &params, Err(error.clone()))
                .await;
            return Err(attach_failed(&error, session.id));
        }
        if let Err(error) = self.recheck_view_owner(&session, &params, &live).await {
            let _ = self
                .terminal_stop(TerminalStopParams {
                    id: terminal.id,
                    expected_boot_id: terminal.boot_id,
                    expected_revision: terminal.revision,
                })
                .await;
            self.complete_view(session.id, &params, Err(error.message.clone()))
                .await;
            return Err(error);
        }
        if lease.is_closed() {
            let error = "native view closed while opening".to_owned();
            let _ = self
                .terminal_stop(TerminalStopParams {
                    id: terminal.id,
                    expected_boot_id: terminal.boot_id,
                    expected_revision: terminal.revision,
                })
                .await;
            self.complete_view(session.id, &params, Err(error.clone()))
                .await;
            return Err(attach_failed(&error, session.id));
        }
        self.complete_view(session.id, &params, Ok(terminal.clone()))
            .await;
        Ok(super::super::to_value(SessionTerminalViewResult {
            session_id: session.id,
            terminal,
        }))
    }

    fn validate_view_session(
        &self,
        session: &lazoo_protocol::EmployeeSessionRecord,
        params: &SessionTerminalViewParams,
    ) -> Result<(), RpcError> {
        if session.revision != params.expected_revision {
            return Err(super::super::session_store_error(
                lazoo_store::StoreError::SessionStale {
                    session_id: session.id,
                    current_revision: session.revision,
                },
            ));
        }
        if session.state != SessionLifecycleState::Idle
            || session.owner_boot_id != Some(self.boot_id)
        {
            return Err(super::super::session_failure(
                "LZ-SESSION-STATE",
                "only daemon-owned idle sessions can be viewed",
                session.id,
            ));
        }
        if self
            .store
            .is_memory_summary_session(session.id)
            .map_err(super::super::session_store_error)?
        {
            return Err(super::super::session_failure(
                "LZ-MEMORY-SESSION-RESERVED",
                "memory summary sessions are reserved and cannot be viewed",
                session.id,
            ));
        }
        Ok(())
    }

    async fn recheck_view_owner(
        &self,
        original: &lazoo_protocol::EmployeeSessionRecord,
        params: &SessionTerminalViewParams,
        live: &Arc<dyn super::super::ManagedSession>,
    ) -> Result<(), RpcError> {
        let session = self
            .store
            .get_session(original.id)
            .map_err(super::super::session_store_error)?;
        self.validate_view_session(&session, params)?;
        let current = self.live_sessions.lock().await.get(&original.id).cloned();
        if current.is_none_or(|current| !Arc::ptr_eq(&current, live)) {
            return Err(super::super::session_failure(
                "LZ-SESSION-HANDLE-REPLACED",
                "the daemon session was replaced while opening the native view",
                original.id,
            ));
        }
        Ok(())
    }

    async fn reserve_view(
        &self,
        session_id: Uuid,
        params: &SessionTerminalViewParams,
    ) -> Result<
        (
            watch::Receiver<Option<Result<TerminalRecord, String>>>,
            bool,
        ),
        RpcError,
    > {
        let mut state = self.terminals.lock().await;
        release_exited(&mut state, self.boot_id)
            .map_err(|error| attach_failed(&error, session_id))?;
        if let Some(binding) = state.views.get(&session_id) {
            if binding.body == *params {
                let terminal = state
                    .sessions
                    .get(&binding.terminal)
                    .and_then(|live| record(self.boot_id, binding.terminal, live).ok())
                    .ok_or_else(|| attach_failed("native view terminal disappeared", session_id))?;
                let (sender, receiver) = watch::channel(Some(Ok(terminal)));
                drop(sender);
                return Ok((receiver, false));
            }
            return Err(super::super::session_failure(
                "LZ-SESSION-VIEW-CONFLICT",
                "a native view is already open with a different request",
                session_id,
            ));
        }
        let key = (session_id, params.idempotency_key.clone());
        if let Some(completed) = state.view_keys.get(&key) {
            if completed.body != *params {
                return Err(super::super::session_failure(
                    "LZ-SESSION-VIEW-CONFLICT",
                    "the native view idempotency key was already used with a different request",
                    session_id,
                ));
            }
            let (sender, receiver) = watch::channel(Some(Ok(completed.last.clone())));
            drop(sender);
            return Ok((receiver, false));
        }
        if let Some(reservation) = state.view_reservations.get(&session_id) {
            if reservation.body == *params {
                return Ok((reservation.outcome.subscribe(), false));
            }
            return Err(super::super::session_failure(
                "LZ-SESSION-VIEW-CONFLICT",
                "a native view is already opening with a different request",
                session_id,
            ));
        }
        let (outcome, receiver) = watch::channel(None);
        state.view_reservations.insert(
            session_id,
            ViewReservation {
                body: params.clone(),
                outcome,
            },
        );
        Ok((receiver, true))
    }

    async fn complete_view(
        &self,
        session_id: Uuid,
        params: &SessionTerminalViewParams,
        outcome: Result<TerminalRecord, String>,
    ) {
        let mut state = self.terminals.lock().await;
        let reservation = state.view_reservations.remove(&session_id);
        if let Ok(terminal) = &outcome {
            if state.view_keys.len() >= MAX_COMMAND_KEYS
                && let Some(key) = state.view_keys.iter().find_map(|(key, value)| {
                    (!state.sessions.contains_key(&value.last.id)).then(|| key.clone())
                })
            {
                state.view_keys.remove(&key);
            }
            if state.view_keys.len() < MAX_COMMAND_KEYS {
                state.view_keys.insert(
                    (session_id, params.idempotency_key.clone()),
                    ViewKey {
                        body: params.clone(),
                        last: terminal.clone(),
                    },
                );
            }
            state.views.insert(
                session_id,
                ViewBinding {
                    body: params.clone(),
                    terminal: terminal.id,
                },
            );
        }
        if let Some(reservation) = reservation {
            reservation.outcome.send_replace(Some(outcome));
        }
    }
}
