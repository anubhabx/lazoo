use std::{
    collections::BTreeMap,
    os::windows::fs::MetadataExt,
    path::{Path, PathBuf},
    time::Duration,
};

use lazoo_protocol::{
    EmployeeSessionRecord, RpcError, SessionLifecycleState, SessionTerminalAttachParams,
    SessionTerminalAttachResult, SessionTerminalStatusParams, SessionTerminalStatusResult,
    TerminalInput, TerminalListResult, TerminalOpenParams, TerminalOutput, TerminalOutputRequest,
    TerminalRecord, TerminalResizeParams, TerminalShowParams, TerminalStopParams,
};
use lazoo_providers::{ProviderError, ProviderKind, attach_args};
use lazoo_runtime::{TerminalCommand, TerminalError, TerminalSession};
use serde_json::{Value, json, to_value};
use tokio::{sync::watch, time::timeout};
use uuid::Uuid;

use super::App;

const MAX_TERMINALS: usize = 8;
const MAX_COMMAND_KEYS: usize = 1024;

#[derive(Default)]
pub(super) struct TerminalState {
    sessions: BTreeMap<Uuid, LiveTerminal>,
    command_keys: BTreeMap<String, CommandKey>,
    reservations: BTreeMap<String, Reservation>,
    /// Employee session to the owner terminal attached to it; `None` while that attach is still
    /// opening. The binding is in memory only and dies with the daemon, like every terminal.
    attachments: BTreeMap<Uuid, Option<Uuid>>,
}
struct LiveTerminal {
    command: TerminalOpenParams,
    session: TerminalSession,
    revision: u64,
    stopping: bool,
}
struct CommandKey {
    body: TerminalOpenParams,
    id: Uuid,
    last: TerminalRecord,
}
struct Reservation {
    body: TerminalOpenParams,
    outcome: watch::Sender<Option<Result<TerminalRecord, String>>>,
}

impl App {
    /// `session.terminal.status` is observational: it reap-checks terminal exit state like other
    /// reads but never reserves a slot, starts a process, or publishes an event.
    pub(super) async fn session_terminal_status(&self, params: Value) -> Result<Value, RpcError> {
        let params: SessionTerminalStatusParams = serde_json::from_value(params)
            .map_err(|error| super::invalid_params(error.to_string()))?;
        let session = self
            .store
            .get_session(params.id)
            .map_err(super::session_store_error)?;
        let (opening, terminal, attached) = {
            let mut state = self.terminals.lock().await;
            release_exited(&mut state, self.boot_id)
                .map_err(|error| attach_failed(&error, session.id))?;
            let binding = state.attachments.get(&session.id).copied();
            let terminal = binding.flatten().and_then(|id| {
                state
                    .sessions
                    .get(&id)
                    .and_then(|live| record(self.boot_id, id, live).ok())
                    .or_else(|| {
                        state
                            .command_keys
                            .values()
                            .find(|key| key.id == id)
                            .map(|key| key.last.clone())
                    })
            });
            let attached = binding.is_some_and(|bound| {
                bound.is_none_or(|terminal_id| state.sessions.contains_key(&terminal_id))
            });
            (binding == Some(None), terminal, attached)
        };
        let blocked: Option<(String, String)> = if attached {
            Some((
                "LZ-SESSION-ATTACHED".to_owned(),
                "an owner terminal is already attached to this session".to_owned(),
            ))
        } else if session.state != SessionLifecycleState::Suspended {
            Some((
                "LZ-SESSION-STATE".to_owned(),
                "only suspended sessions can be attached".to_owned(),
            ))
        } else if self
            .store
            .is_memory_summary_session(session.id)
            .map_err(super::session_store_error)?
        {
            Some((
                "LZ-MEMORY-SESSION-RESERVED".to_owned(),
                "memory summary sessions are reserved and cannot be taken over".to_owned(),
            ))
        } else if self
            .store
            .active_session_for_employee(session.employee_id)
            .map_err(super::session_store_error)?
            .is_some()
        {
            Some((
                "LZ-SESSION-CONFLICT".to_owned(),
                "an active session already owns this employee".to_owned(),
            ))
        } else {
            let probe = SessionTerminalAttachParams {
                id: session.id,
                expected_revision: session.revision,
                columns: 80,
                rows: 25,
                idempotency_key: "status-probe".to_owned(),
            };
            match self.attach_command(&session, &probe) {
                Err(error) => Some((error.code, error.message)),
                Ok(open) => validate_open(&open)
                    .err()
                    .map(|reason| ("LZ-SESSION-ATTACH-FAILED".to_owned(), reason)),
            }
        };
        let (blocking_code, blocking_reason) =
            blocked.map_or((None, None), |(code, reason)| (Some(code), Some(reason)));
        Ok(super::to_value(SessionTerminalStatusResult {
            boot_id: self.boot_id,
            session_id: session.id,
            session_revision: session.revision,
            opening,
            terminal,
            attachable: blocking_code.is_none(),
            blocking_code,
            blocking_reason,
        }))
    }

    pub(super) async fn terminal_control(
        &self,
        method: &str,
        params: Value,
    ) -> Result<Value, lazoo_protocol::RpcError> {
        let result = match method {
            "terminal.open" => self
                .terminal_open(
                    serde_json::from_value(params)
                        .map_err(|error| super::invalid_params(error.to_string()))?,
                )
                .await
                .and_then(|value| to_value(value).map_err(|error| error.to_string())),
            "terminal.list" => self
                .terminal_list()
                .await
                .and_then(|value| to_value(value).map_err(|error| error.to_string())),
            "terminal.show" => self
                .terminal_show(
                    serde_json::from_value(params)
                        .map_err(|error| super::invalid_params(error.to_string()))?,
                )
                .await
                .and_then(|value| to_value(value).map_err(|error| error.to_string())),
            "terminal.resize" => self
                .terminal_resize(
                    serde_json::from_value(params)
                        .map_err(|error| super::invalid_params(error.to_string()))?,
                )
                .await
                .and_then(|value| to_value(value).map_err(|error| error.to_string())),
            "terminal.stop" => self
                .terminal_stop(
                    serde_json::from_value(params)
                        .map_err(|error| super::invalid_params(error.to_string()))?,
                )
                .await
                .and_then(|value| to_value(value).map_err(|error| error.to_string())),
            _ => {
                return Err(lazoo_protocol::RpcError {
                    code: "method-not-found".into(),
                    message: format!("unknown terminal method: {method}"),
                    data: None,
                });
            }
        };
        result.map_err(super::invalid_params)
    }
    pub(crate) async fn shutdown_terminals(&self) {
        let mut pending = {
            let state = self.terminals.lock().await;
            state
                .reservations
                .values()
                .map(|reservation| reservation.outcome.subscribe())
                .collect::<Vec<_>>()
        };
        for reservation in &mut pending {
            let _ = timeout(Duration::from_secs(3), reservation.changed()).await;
        }
        let sessions = {
            let state = self.terminals.lock().await;
            state
                .sessions
                .values()
                .map(|terminal| terminal.session.clone())
                .collect::<Vec<_>>()
        };
        for session in sessions {
            let _ = tokio::task::spawn_blocking(move || session.stop()).await;
        }
    }
    pub(crate) async fn terminal_open(
        &self,
        params: TerminalOpenParams,
    ) -> Result<TerminalRecord, String> {
        validate_open(&params)?;
        let (mut receiver, owner) = {
            let mut state = self.terminals.lock().await;
            release_exited(&mut state, self.boot_id)?;
            if let Some(existing) = state.command_keys.get(&params.idempotency_key) {
                if existing.body != params {
                    return Err(
                        "terminal idempotency key was already used with a different request".into(),
                    );
                }
                return Ok(existing.last.clone());
            }
            if let Some(existing) = state.reservations.get(&params.idempotency_key) {
                if existing.body == params {
                    (existing.outcome.subscribe(), false)
                } else {
                    return Err(
                        "terminal idempotency key was already used with a different request".into(),
                    );
                }
            } else {
                let active = state
                    .sessions
                    .values()
                    .map(|terminal| {
                        terminal
                            .session
                            .exit_status()
                            .map_err(|error| terminal_error(&error))
                    })
                    .collect::<Result<Vec<_>, _>>()
                    .map(|statuses| statuses.into_iter().filter(|status| status.live).count())?;
                if active + state.reservations.len() >= MAX_TERMINALS {
                    return Err("terminal limit reached".into());
                }
                if state.command_keys.len() + state.reservations.len() >= MAX_COMMAND_KEYS {
                    evict_one_dead(&mut state)?;
                }
                let (outcome, receiver) = watch::channel(None);
                state.reservations.insert(
                    params.idempotency_key.clone(),
                    Reservation {
                        body: params.clone(),
                        outcome,
                    },
                );
                (receiver, true)
            }
        };
        if owner {
            let app = self.clone();
            let request = params.clone();
            let command =
                TerminalCommand::new(&params.executable, &params.cwd, params.columns, params.rows)
                    .args(params.args.clone());
            tokio::spawn(async move {
                let spawned = tokio::task::spawn_blocking(move || TerminalSession::spawn(command))
                    .await
                    .map_err(|error| error.to_string())
                    .and_then(|result| result.map_err(|error| error.to_string()));
                app.complete_terminal_open(request, spawned).await;
            });
        }
        loop {
            if let Some(outcome) = receiver.borrow().clone() {
                return outcome;
            }
            receiver
                .changed()
                .await
                .map_err(|_| "terminal open operation was cancelled".to_owned())?;
        }
    }

    async fn complete_terminal_open(
        &self,
        params: TerminalOpenParams,
        spawned: Result<TerminalSession, String>,
    ) {
        let mut state = self.terminals.lock().await;
        let reservation = state.reservations.remove(&params.idempotency_key);
        let shutdown = self.shutdown_receiver();
        let result = match spawned {
            Ok(_session) if *shutdown.borrow() => Err("daemon is shutting down".into()),
            Ok(session) => {
                let id = session.id();
                let terminal = LiveTerminal {
                    command: params.clone(),
                    session,
                    revision: 1,
                    stopping: false,
                };
                match record(self.boot_id, id, &terminal) {
                    Ok(record) => {
                        state.command_keys.insert(
                            params.idempotency_key.clone(),
                            CommandKey {
                                body: params,
                                id,
                                last: record.clone(),
                            },
                        );
                        state.sessions.insert(id, terminal);
                        Ok(record)
                    }
                    Err(error) => Err(error),
                }
            }
            Err(error) => Err(error),
        };
        if let Some(reservation) = reservation {
            reservation.outcome.send_replace(Some(result));
        }
    }
    pub(crate) async fn terminal_list(&self) -> Result<TerminalListResult, String> {
        let mut state = self.terminals.lock().await;
        release_exited(&mut state, self.boot_id)?;
        let current = state
            .sessions
            .iter()
            .map(|(id, terminal)| record(self.boot_id, *id, terminal).map(|record| (*id, record)))
            .collect::<Result<Vec<_>, _>>()?;
        for (id, record) in current {
            if let Some(key) = state.command_keys.values_mut().find(|key| key.id == id) {
                key.last = record;
            }
        }
        let terminals = state
            .command_keys
            .values()
            .map(|key| key.last.clone())
            .collect();
        Ok(TerminalListResult {
            boot_id: self.boot_id,
            terminals,
        })
    }
    pub(crate) async fn terminal_show(
        &self,
        params: TerminalShowParams,
    ) -> Result<TerminalRecord, String> {
        let mut state = self.terminals.lock().await;
        if let Some(terminal) = state.sessions.get(&params.id) {
            let current = record(self.boot_id, params.id, terminal)?;
            if let Some(key) = state
                .command_keys
                .values_mut()
                .find(|key| key.id == params.id)
            {
                key.last = current.clone();
            }
            return Ok(current);
        }
        state
            .command_keys
            .values()
            .find(|key| key.id == params.id)
            .map(|key| key.last.clone())
            .ok_or_else(|| "terminal was not found".into())
    }
    pub(crate) async fn terminal_resize(
        &self,
        params: TerminalResizeParams,
    ) -> Result<TerminalRecord, String> {
        require_boot_id(self.boot_id, params.expected_boot_id)?;
        let mut state = self.terminals.lock().await;
        let terminal = state
            .sessions
            .get_mut(&params.id)
            .ok_or("terminal was not found")?;
        if terminal.stopping {
            return Err("terminal is stopping".into());
        }
        if terminal.revision != params.expected_revision {
            return Err("terminal revision conflict".into());
        }
        terminal
            .session
            .resize(params.columns, params.rows)
            .map_err(|error| error.to_string())?;
        terminal.command.columns = params.columns;
        terminal.command.rows = params.rows;
        terminal.revision += 1;
        let current = record(self.boot_id, params.id, terminal)?;
        if let Some(key) = state
            .command_keys
            .values_mut()
            .find(|key| key.id == params.id)
        {
            key.last = current.clone();
        }
        Ok(current)
    }
    pub(crate) async fn terminal_stop(
        &self,
        params: TerminalStopParams,
    ) -> Result<TerminalRecord, String> {
        self.terminal_stop_with(params, |session| session.stop())
            .await
    }

    async fn terminal_stop_with<F>(
        &self,
        params: TerminalStopParams,
        stop: F,
    ) -> Result<TerminalRecord, String>
    where
        F: FnOnce(TerminalSession) -> Result<(), TerminalError> + Send + 'static,
    {
        require_boot_id(self.boot_id, params.expected_boot_id)?;
        let session = {
            let mut state = self.terminals.lock().await;
            let terminal = state
                .sessions
                .get_mut(&params.id)
                .ok_or("terminal was not found")?;
            if terminal.stopping {
                return Err("terminal is stopping".into());
            }
            if terminal.revision != params.expected_revision {
                return Err("terminal revision conflict".into());
            }
            terminal.revision += 1;
            terminal.stopping = true;
            terminal.session.clone()
        };
        let stopped = tokio::task::spawn_blocking(move || stop(session))
            .await
            .map_err(|error| error.to_string())
            .and_then(|result| result.map_err(|error| error.to_string()));
        if let Err(error) = stopped {
            let mut state = self.terminals.lock().await;
            clear_failed_stop(
                &mut state,
                self.boot_id,
                params.id,
                params.expected_revision + 1,
            );
            return Err(error);
        }
        let mut state = self.terminals.lock().await;
        let terminal = state
            .sessions
            .get(&params.id)
            .ok_or("terminal was not found")?;
        if terminal.revision != params.expected_revision + 1 {
            return Err("terminal stop was superseded".into());
        }
        let record = record(self.boot_id, params.id, terminal)?;
        state.sessions.remove(&params.id);
        if let Some(key) = state
            .command_keys
            .values_mut()
            .find(|key| key.id == params.id)
        {
            key.last = record.clone();
        }
        Ok(record)
    }
    pub(crate) async fn terminal_input(&self, input: TerminalInput) -> Result<(), String> {
        require_boot_id(self.boot_id, input.expected_boot_id)?;
        let state = self.terminals.lock().await;
        let terminal = state
            .sessions
            .get(&input.id)
            .ok_or("terminal was not found")?;
        if terminal.stopping {
            return Err("terminal is stopping".into());
        }
        terminal
            .session
            .write_input(input.bytes)
            .map_err(|error| terminal_error(&error))
    }
    pub(crate) async fn terminal_output(
        &self,
        request: TerminalOutputRequest,
    ) -> Result<TerminalOutput, String> {
        require_boot_id(self.boot_id, request.expected_boot_id)?;
        let state = self.terminals.lock().await;
        let terminal = state
            .sessions
            .get(&request.id)
            .ok_or("terminal was not found")?;
        if terminal.stopping {
            return Err("terminal is stopping".into());
        }
        let output = terminal
            .session
            .read_output(request.after, request.limit)
            .map_err(|error| terminal_error(&error))?;
        let status = terminal
            .session
            .exit_status()
            .map_err(|error| terminal_error(&error))?;
        Ok(TerminalOutput {
            start: output.start,
            end: output.end,
            bytes: output.bytes,
            gap: output.gap,
            live: status.live,
            exit_code: status.exit_code,
        })
    }
}

impl App {
    /// `session.terminal.attach`: reopen a suspended Claude Code or Codex session's native
    /// harness in an owner terminal. Only `suspended` qualifies: every provider keeps a live
    /// protocol host for the whole time a session is idle, so a second interactive process on
    /// the same native id would put two writers on one transcript. The TUI runs under the
    /// owner's own native configuration rather than the employee's authority mode, because the
    /// owner is at the keyboard; what is typed there never enters Lazoo's turn ledger.
    pub(super) async fn session_terminal_attach(&self, params: Value) -> Result<Value, RpcError> {
        let params: SessionTerminalAttachParams = serde_json::from_value(params)
            .map_err(|error| super::invalid_params(error.to_string()))?;
        super::require_session_idempotency_key(&params.idempotency_key)?;
        if params.expected_revision == 0 {
            return Err(super::session_invalid(
                "expectedRevision must be greater than zero",
            ));
        }
        let session = self
            .store
            .get_session(params.id)
            .map_err(super::session_store_error)?;
        if session.revision != params.expected_revision {
            return Err(super::session_store_error(
                lazoo_store::StoreError::SessionStale {
                    session_id: session.id,
                    current_revision: session.revision,
                },
            ));
        }
        if session.state != SessionLifecycleState::Suspended {
            return Err(super::session_store_error(
                lazoo_store::StoreError::SessionState {
                    session_id: session.id,
                    state: state_name(session.state),
                    expected: "suspended",
                },
            ));
        }
        if self
            .store
            .is_memory_summary_session(session.id)
            .map_err(super::session_store_error)?
        {
            return Err(super::session_failure(
                "LZ-MEMORY-SESSION-RESERVED",
                "memory summary sessions are reserved and cannot be taken over",
                session.id,
            ));
        }
        let open = self.attach_command(&session, &params)?;
        if let Some(replayed) = self.reserve_attachment(session.id, &open).await? {
            return Ok(super::to_value(SessionTerminalAttachResult {
                session_id: session.id,
                terminal: replayed,
            }));
        }
        // An older suspended session and the one resumed from it share a native transcript, so
        // the owner may not drive it while the employee has a session in flight. Checked after
        // the slot is taken: `begin_session` marks a waking session `starting` before it decides
        // whether to resume, so either this check sees that session or its decision sees the slot.
        match self.store.active_session_for_employee(session.employee_id) {
            Ok(None) => {}
            Ok(Some(active)) => {
                self.terminals.lock().await.attachments.remove(&session.id);
                return Err(super::session_store_error(
                    lazoo_store::StoreError::SessionConflict {
                        employee_id: session.employee_id,
                        session_id: active.id,
                    },
                ));
            }
            Err(error) => {
                self.terminals.lock().await.attachments.remove(&session.id);
                return Err(super::session_store_error(error));
            }
        }
        let opened = self.terminal_open(open).await;
        self.complete_attachment(session.id, opened).await
    }

    /// Resolves the provider executable, native resume arguments and working directory for an
    /// attach from Lazoo's own records, refusing when any of them is unknown.
    fn attach_command(
        &self,
        session: &EmployeeSessionRecord,
        params: &SessionTerminalAttachParams,
    ) -> Result<TerminalOpenParams, RpcError> {
        let provider = session
            .provider
            .parse::<ProviderKind>()
            .map_err(super::provider_error)?;
        let native_session_id = session.native_session_id.as_deref().unwrap_or_default();
        let Some(args) = attach_args(provider, native_session_id) else {
            return Err(super::session_failure(
                "LZ-SESSION-ATTACH-UNSUPPORTED",
                "no proven native resume command exists for this session's provider",
                session.id,
            ));
        };
        let executable = self
            .providers
            .discover(&self.provider_environment)
            .into_iter()
            .find(|discovery| discovery.provider == provider)
            .and_then(|discovery| discovery.candidate)
            .ok_or(ProviderError::NotDetected(provider))
            .map_err(super::provider_error)?
            .path;
        let cwd = match self
            .store
            .task_workspace_for_session(session.id)
            .map_err(super::session_store_error)?
        {
            Some(workspace) => PathBuf::from(workspace),
            None => self
                .session_directory(session.id)
                .map_err(super::session_store_error)?,
        };
        Ok(TerminalOpenParams {
            executable: executable.to_string_lossy().into_owned(),
            args,
            cwd: cwd.to_string_lossy().into_owned(),
            columns: params.columns,
            rows: params.rows,
            // Namespaced so an attach never replays or collides with an ad-hoc terminal.open key.
            idempotency_key: format!("session-attach:{}:{}", session.id, params.idempotency_key),
        })
    }

    /// Whether an owner terminal is, or is still opening, attached to `session_id`. A resume must
    /// not start a second native process on a conversation the owner is driving.
    pub(super) async fn session_has_live_attachment(&self, session_id: Uuid) -> bool {
        let mut state = self.terminals.lock().await;
        // An exit-status failure leaves the entry in place, which reads as still attached.
        let _ = release_exited(&mut state, self.boot_id);
        state
            .attachments
            .get(&session_id)
            .copied()
            .is_some_and(|bound| {
                bound.is_none_or(|terminal_id| state.sessions.contains_key(&terminal_id))
            })
    }

    /// Takes the session's single attach slot, or returns the terminal an identical earlier
    /// request already opened. A slot whose terminal exited or was stopped is free again.
    async fn reserve_attachment(
        &self,
        session_id: Uuid,
        open: &TerminalOpenParams,
    ) -> Result<Option<TerminalRecord>, RpcError> {
        let mut state = self.terminals.lock().await;
        release_exited(&mut state, self.boot_id)
            .map_err(|error| attach_failed(&error, session_id))?;
        if let Some(existing) = state.command_keys.get(&open.idempotency_key)
            && existing.body == *open
        {
            return Ok(Some(existing.last.clone()));
        }
        if let Some(bound) = state.attachments.get(&session_id).copied()
            && bound.is_none_or(|terminal_id| state.sessions.contains_key(&terminal_id))
        {
            return Err(RpcError {
                code: "LZ-SESSION-ATTACHED".to_owned(),
                message: "an owner terminal is already attached to this session".to_owned(),
                data: Some(json!({"sessionId": session_id, "terminalId": bound})),
            });
        }
        state.attachments.insert(session_id, None);
        Ok(None)
    }

    async fn complete_attachment(
        &self,
        session_id: Uuid,
        opened: Result<TerminalRecord, String>,
    ) -> Result<Value, RpcError> {
        let terminal = {
            let mut state = self.terminals.lock().await;
            match &opened {
                Ok(terminal) => state.attachments.insert(session_id, Some(terminal.id)),
                Err(_) => state.attachments.remove(&session_id),
            };
            opened
        }
        .map_err(|error| attach_failed(&error, session_id))?;
        self.store
            .record_session_terminal_attached(session_id, terminal.id)
            .map_err(super::session_store_error)?;
        self.publish_events()?;
        Ok(super::to_value(SessionTerminalAttachResult {
            session_id,
            terminal,
        }))
    }
}

fn attach_failed(error: &str, session_id: Uuid) -> RpcError {
    super::session_failure("LZ-SESSION-ATTACH-FAILED", error, session_id)
}

fn state_name(state: SessionLifecycleState) -> String {
    serde_json::to_value(state)
        .ok()
        .and_then(|value| value.as_str().map(str::to_owned))
        .unwrap_or_default()
}

fn validate_open(params: &TerminalOpenParams) -> Result<(), String> {
    if params.args.len() > 64
        || params.idempotency_key.is_empty()
        || params.idempotency_key.len() > 256
    {
        return Err("terminal request is invalid".into());
    }
    let executable = Path::new(&params.executable);
    let cwd = Path::new(&params.cwd);
    if !executable.is_absolute() || !executable.is_file() {
        return Err("terminal executable must be an existing absolute file".into());
    }
    if !cwd.is_absolute() || !cwd.is_dir() {
        return Err("terminal cwd must be a directory".into());
    }
    if std::fs::metadata(cwd)
        .map_err(|error| error.to_string())?
        .file_attributes()
        & 0x400
        != 0
    {
        return Err("terminal cwd must not be a reparse point".into());
    }
    Ok(())
}
fn record(boot_id: Uuid, id: Uuid, terminal: &LiveTerminal) -> Result<TerminalRecord, String> {
    let status = terminal
        .session
        .exit_status()
        .map_err(|error| terminal_error(&error))?;
    Ok(TerminalRecord {
        id,
        boot_id,
        pid: terminal.session.pid(),
        executable: terminal.command.executable.clone(),
        args: terminal.command.args.clone(),
        cwd: terminal.command.cwd.clone(),
        columns: terminal.command.columns,
        rows: terminal.command.rows,
        revision: terminal.revision,
        live: status.live,
        exit_code: status.exit_code,
    })
}
fn terminal_error(error: &TerminalError) -> String {
    error.to_string()
}
fn require_boot_id(actual: Uuid, expected: Uuid) -> Result<(), String> {
    if actual == expected {
        Ok(())
    } else {
        Err("terminal lifecycle truth was lost after daemon restart".into())
    }
}
fn evict_one_dead(state: &mut TerminalState) -> Result<(), String> {
    let key = state
        .command_keys
        .iter()
        .find_map(|(key, command)| (!state.sessions.contains_key(&command.id)).then(|| key.clone()))
        .ok_or("terminal idempotency map is full of live terminals")?;
    let id = state
        .command_keys
        .remove(&key)
        .ok_or("terminal idempotency state changed")?
        .id;
    debug_assert!(!state.sessions.contains_key(&id));
    Ok(())
}

fn release_exited(state: &mut TerminalState, boot_id: Uuid) -> Result<(), String> {
    let exited = state
        .sessions
        .iter()
        .map(|(id, terminal)| {
            if terminal.stopping {
                return Ok(None);
            }
            let status = terminal
                .session
                .exit_status()
                .map_err(|error| terminal_error(&error))?;
            if status.live {
                Ok(None)
            } else {
                record(boot_id, *id, terminal).map(Some)
            }
        })
        .collect::<Result<Vec<_>, String>>()?
        .into_iter()
        .flatten()
        .collect::<Vec<_>>();
    for record in exited {
        state.sessions.remove(&record.id);
        if let Some(key) = state
            .command_keys
            .values_mut()
            .find(|key| key.id == record.id)
        {
            key.last = record;
        }
    }
    Ok(())
}

fn clear_failed_stop(state: &mut TerminalState, boot_id: Uuid, id: Uuid, revision: u64) {
    let record = state.sessions.get_mut(&id).and_then(|terminal| {
        if terminal.revision != revision || !terminal.stopping {
            return None;
        }
        terminal.stopping = false;
        record(boot_id, id, terminal).ok()
    });
    if let Some(record) = record
        && let Some(key) = state.command_keys.values_mut().find(|key| key.id == id)
    {
        key.last = record;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use lazoo_providers::DiscoveryEnvironment;
    use lazoo_runtime::DataRoot;
    use lazoo_store::Store;
    use tempfile::tempdir;

    fn test_app() -> (tempfile::TempDir, App) {
        let temporary = tempdir().unwrap();
        let root = DataRoot::from_path(temporary.path().join("data"));
        root.ensure().unwrap();
        let store = Store::open_path(root.state_db()).unwrap();
        let app =
            App::new_with_provider_environment(store, root, DiscoveryEnvironment::empty()).unwrap();
        (temporary, app)
    }

    fn command(key: &str) -> TerminalOpenParams {
        TerminalOpenParams {
            executable: r"C:\Windows\System32\cmd.exe".into(),
            args: vec!["/Q".into(), "/K".into()],
            cwd: std::env::current_dir()
                .unwrap()
                .to_string_lossy()
                .into_owned(),
            columns: 80,
            rows: 25,
            idempotency_key: key.into(),
        }
    }

    #[tokio::test]
    async fn injected_stop_failure_releases_the_stop_lease_for_retry() {
        let (_temporary, app) = test_app();
        let opened = app
            .terminal_open(command("injected-stop-failure"))
            .await
            .unwrap();
        let failed = app
            .terminal_stop_with(
                TerminalStopParams {
                    id: opened.id,
                    expected_boot_id: opened.boot_id,
                    expected_revision: 1,
                },
                |_| Err(TerminalError::ShutdownTimeout),
            )
            .await;
        assert!(failed.is_err());
        let shown = app
            .terminal_show(TerminalShowParams { id: opened.id })
            .await
            .unwrap();
        assert_eq!(shown.revision, 2);
        assert!(
            app.terminal_stop(TerminalStopParams {
                id: opened.id,
                expected_boot_id: opened.boot_id,
                expected_revision: 1,
            })
            .await
            .is_err()
        );
        let stopped = app
            .terminal_stop(TerminalStopParams {
                id: opened.id,
                expected_boot_id: opened.boot_id,
                expected_revision: 2,
            })
            .await
            .unwrap();
        assert!(!stopped.live);
        assert_eq!(app.terminal_list().await.unwrap().terminals, vec![stopped]);
    }
}
