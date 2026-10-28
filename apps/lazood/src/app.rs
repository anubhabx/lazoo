mod approval;
mod configuration;
mod delivery;
mod idle;
mod inbox;
mod incident;
mod memory;
mod memory_context;
mod memory_worker;
mod memory_worker_cleanup;
mod memory_worker_execution;
#[cfg(test)]
mod memory_worker_tests;
mod resources;
mod resume;
mod task;
mod terminal;
mod tools;
mod turn;
mod workspace;

pub use incident::UserNotifier;

use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    future::Future,
    pin::Pin,
    str::FromStr,
    sync::{
        Arc,
        atomic::{AtomicU8, Ordering},
    },
    time::Instant,
};

use chrono::{SecondsFormat, Utc};
use lazoo_domain::{
    inspect_git_repository, validate_employee_name, validate_employee_text_fields,
    validate_idle_timeout, validate_native_settings,
};
use lazoo_protocol::{
    AcknowledgeMessageParams, AuthorityMode, CancelMessageParams, ClaimMessageParams,
    CreateEmployeeParams, DesktopSnapshot, DisableEmployeeParams, EmployeeShowParams, EventSummary,
    EventsListParams, FailMessageParams, JsonRpcRequest, JsonRpcResponse, MessageListParams,
    MessageShowParams, PROTOCOL_V1, ProviderListEntry, ProviderObservation, ProviderProbeParams,
    ProviderSmokeParams, ProviderSmokeResult, RegisterProjectParams, RetryMessageParams,
    RosterRefreshParams, RpcError, SendMessageParams, SessionShowParams, StopSessionParams,
    SystemStatus, UpdateEmployeeParams, WarmSessionParams,
};
use lazoo_providers::{
    AuthenticationState, DiscoveryEnvironment, DiscoverySource, ProviderCapabilities,
    ProviderError, ProviderEvent, ProviderKind, ProviderProbe, ProviderRegistry, ProviderSession,
    ProviderSessionHandle, ProviderVerification, SessionRequest, TurnEventObserver, TurnRequest,
};
use lazoo_runtime::{DataRoot, JobAccounting};
use lazoo_store::{
    AcknowledgeMessageInput, BeginSessionInput, BeginSessionStopInput, CancelMessageInput,
    ClaimMessageInput, CreateEmployeeInput, DisableEmployeeInput, FailMessageInput,
    FailSessionInput, FinishSessionStopInput, MarkSessionReadyInput,
    RefreshProviderObservationsInput, RegisterProjectInput, RetryMessageInput, SendMessageInput,
    Store, StoreStatus, UpdateEmployeeInput,
};
use serde::Serialize;
use serde_json::{Value, json};
use tokio::{
    sync::{Mutex, Notify, broadcast, watch},
    time::{Duration, timeout},
};
use uuid::Uuid;

pub type SessionBackendFuture<'a> =
    Pin<Box<dyn Future<Output = Result<Box<dyn ManagedSession>, ProviderError>> + Send + 'a>>;

pub trait ManagedSession: Send + Sync {
    fn id(&self) -> String;
    fn native_host_pid(&self) -> Option<u32>;
    /// Job Object accounting for the native process tree; `None` when there is no job.
    fn resources(&self) -> Option<JobAccounting> {
        None
    }
    fn ready_for_turn(
        &self,
    ) -> Pin<Box<dyn Future<Output = Result<(), ProviderError>> + Send + '_>> {
        Box::pin(async { Ok(()) })
    }
    fn turn_observed<'a>(
        &'a self,
        request: &'a TurnRequest,
        observer: &'a dyn TurnEventObserver,
    ) -> Pin<Box<dyn Future<Output = Result<Vec<ProviderEvent>, ProviderError>> + Send + 'a>>;
    fn cancel(&self) -> Pin<Box<dyn Future<Output = Result<(), ProviderError>> + Send + '_>>;
    fn stop(&self) -> Pin<Box<dyn Future<Output = Result<(), ProviderError>> + Send + '_>>;
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum ProviderIoState {
    Ready,
    Polling,
    Started,
    Cancelled,
}

pub(super) struct LiveTurn {
    pub session: Arc<dyn ManagedSession>,
    provider_io_state: AtomicU8,
}

impl LiveTurn {
    const READY: u8 = 0;
    const POLLING: u8 = 1;
    const STARTED: u8 = 2;
    const CANCELLED: u8 = 3;

    pub fn new(session: Arc<dyn ManagedSession>) -> Self {
        Self {
            session,
            provider_io_state: AtomicU8::new(Self::READY),
        }
    }

    pub fn provider_io_state(&self) -> ProviderIoState {
        match self.provider_io_state.load(Ordering::SeqCst) {
            Self::READY => ProviderIoState::Ready,
            Self::POLLING => ProviderIoState::Polling,
            Self::STARTED => ProviderIoState::Started,
            _ => ProviderIoState::Cancelled,
        }
    }

    pub fn cancel_before_provider_io(&self) -> bool {
        self.provider_io_state
            .compare_exchange(
                Self::READY,
                Self::CANCELLED,
                Ordering::SeqCst,
                Ordering::SeqCst,
            )
            .is_ok()
    }

    pub fn begin_provider_poll(&self) -> bool {
        self.provider_io_state
            .compare_exchange(
                Self::READY,
                Self::POLLING,
                Ordering::SeqCst,
                Ordering::SeqCst,
            )
            .is_ok()
    }

    pub fn finish_provider_poll(&self) {
        self.provider_io_state
            .store(Self::STARTED, Ordering::SeqCst);
    }
}

pub trait SessionBackend: Send + Sync {
    fn start<'a>(
        &'a self,
        provider: ProviderKind,
        request: &'a SessionRequest,
    ) -> SessionBackendFuture<'a>;
}

#[derive(Clone, Debug)]
struct ProviderSessionBackend {
    providers: ProviderRegistry,
}

impl ManagedSession for ProviderSessionHandle {
    fn id(&self) -> String {
        ProviderSession::id(self)
    }

    fn native_host_pid(&self) -> Option<u32> {
        ProviderSession::native_host_pid(self)
    }

    fn resources(&self) -> Option<JobAccounting> {
        ProviderSession::resources(self)
    }

    fn turn_observed<'a>(
        &'a self,
        request: &'a TurnRequest,
        observer: &'a dyn TurnEventObserver,
    ) -> Pin<Box<dyn Future<Output = Result<Vec<ProviderEvent>, ProviderError>> + Send + 'a>> {
        ProviderSession::turn_observed(self, request, observer)
    }

    fn cancel(&self) -> Pin<Box<dyn Future<Output = Result<(), ProviderError>> + Send + '_>> {
        ProviderSession::cancel(self)
    }

    fn stop(&self) -> Pin<Box<dyn Future<Output = Result<(), ProviderError>> + Send + '_>> {
        ProviderSession::stop(self)
    }
}

impl SessionBackend for ProviderSessionBackend {
    fn start<'a>(
        &'a self,
        provider: ProviderKind,
        request: &'a SessionRequest,
    ) -> SessionBackendFuture<'a> {
        Box::pin(async move {
            self.providers
                .start(provider, request)
                .await
                .map(|session| Box::new(session) as Box<dyn ManagedSession>)
        })
    }
}

#[derive(Clone)]
pub struct App {
    store: Store,
    root: DataRoot,
    boot_id: Uuid,
    started: Instant,
    events: broadcast::Sender<EventSummary>,
    shutdown: watch::Sender<bool>,
    providers: ProviderRegistry,
    provider_environment: DiscoveryEnvironment,
    session_backend: Arc<dyn SessionBackend>,
    live_sessions: Arc<Mutex<BTreeMap<Uuid, Arc<dyn ManagedSession>>>>,
    live_turns: Arc<Mutex<BTreeMap<Uuid, Arc<LiveTurn>>>>,
    task_dispatch_notify: Arc<Notify>,
    approval_notify: Arc<Notify>,
    /// Woken by `publish_events`, which follows every store write, so a tool wait re-reads the
    /// store when something changed instead of polling it.
    store_changed: Arc<Notify>,
    approval_timeout: Duration,
    user_notifier: Arc<dyn UserNotifier>,
    resource_budget: Option<Arc<resources::ResourceBudget>>,
    resource_budget_error: Option<String>,
    terminals: Arc<Mutex<terminal::TerminalState>>,
}

/// How long a provider waits for the local user before a pending approval expires and is
/// declined. Waiting never counts against the provider turn budget.
// ponytail: one global expiry; make it per-project policy when the approval Inbox contract lands.
const DEFAULT_APPROVAL_TIMEOUT: Duration = Duration::from_mins(15);

pub struct EventSubscription {
    /// The durable replay is drained in pages through `subscription_page` before live events.
    pub replay_high_water: u64,
    pub live: broadcast::Receiver<EventSummary>,
}

impl App {
    pub fn new(store: Store, root: DataRoot) -> Result<Self, lazoo_store::StoreError> {
        Self::new_with_provider_environment(store, root, DiscoveryEnvironment::current())
    }

    #[doc(hidden)]
    pub fn new_with_provider_environment(
        store: Store,
        root: DataRoot,
        provider_environment: DiscoveryEnvironment,
    ) -> Result<Self, lazoo_store::StoreError> {
        let providers = ProviderRegistry::default();
        let session_backend = Arc::new(ProviderSessionBackend {
            providers: providers.clone(),
        });
        Self::new_with_dependencies(
            store,
            root,
            provider_environment,
            providers,
            session_backend,
        )
    }

    #[doc(hidden)]
    pub fn new_with_session_backend(
        store: Store,
        root: DataRoot,
        provider_environment: DiscoveryEnvironment,
        session_backend: Arc<dyn SessionBackend>,
    ) -> Result<Self, lazoo_store::StoreError> {
        Self::new_with_dependencies(
            store,
            root,
            provider_environment,
            ProviderRegistry::default(),
            session_backend,
        )
    }

    fn new_with_dependencies(
        store: Store,
        root: DataRoot,
        provider_environment: DiscoveryEnvironment,
        providers: ProviderRegistry,
        session_backend: Arc<dyn SessionBackend>,
    ) -> Result<Self, lazoo_store::StoreError> {
        let (events, _) = broadcast::channel(1024);
        let (shutdown, _) = watch::channel(false);
        let boot_id = Uuid::new_v4();
        let sessions = store.reconcile_sessions(boot_id)?;
        let messages = store.reconcile_messages()?;
        let turns = store.reconcile_turns(boot_id)?;
        store.reconcile_approvals(boot_id)?;
        let mut tasks = store.reconcile_tasks()?.tasks;
        tasks.extend(store.reconcile_task_dispatches(boot_id)?.tasks);
        for event in store.publish_pending_outbox()? {
            let _ = events.send(event);
        }
        let app = Self {
            store,
            root,
            boot_id,
            started: Instant::now(),
            events,
            shutdown,
            providers,
            provider_environment,
            session_backend,
            live_sessions: Arc::new(Mutex::new(BTreeMap::new())),
            live_turns: Arc::new(Mutex::new(BTreeMap::new())),
            task_dispatch_notify: Arc::new(Notify::new()),
            approval_notify: Arc::new(Notify::new()),
            store_changed: Arc::new(Notify::new()),
            approval_timeout: DEFAULT_APPROVAL_TIMEOUT,
            user_notifier: Arc::new(lazoo_runtime::NativeNotifier),
            resource_budget: None,
            resource_budget_error: None,
            terminals: Arc::new(Mutex::new(terminal::TerminalState::default())),
        };
        app.configuration_recover_startup()?;
        app.memory_recover()?;
        app.note_startup_incidents(&sessions.sessions, &messages.messages, &turns.turns, &tasks);
        Ok(app)
    }

    #[doc(hidden)]
    #[must_use]
    pub fn with_approval_timeout(mut self, timeout: Duration) -> Self {
        self.approval_timeout = timeout;
        self
    }

    #[doc(hidden)]
    #[must_use]
    pub fn with_user_notifier(mut self, notifier: Arc<dyn UserNotifier>) -> Self {
        self.user_notifier = notifier;
        self
    }

    #[allow(clippy::too_many_lines)] // One declarative RPC dispatch table, kept together for auditing.
    pub async fn handle(&self, request: JsonRpcRequest) -> JsonRpcResponse {
        if request.jsonrpc != "2.0" {
            return failure(
                &request.id,
                "LZ-INVALID-REQUEST",
                "jsonrpc must be 2.0",
                None,
            );
        }
        let result = match request.method.as_str() {
            "system.status" => self.status(),
            "system.budget" => self.system_budget(&request.params),
            "memory.policy.show" => self.memory_policy_show(&request.params),
            "memory.policy.set" => self.memory_policy_set(&request.params),
            "memory.archive" => self.memory_archive(&request.params),
            "memory.search" => self.memory_search(&request.params),
            "memory.context" => self.memory_context(&request.params),
            "memory.cleanup.preview" => self.memory_cleanup_preview(&request.params),
            "memory.cleanup.apply" => self.memory_cleanup_apply(&request.params),
            "memory.queue.status" => self.memory_queue_status(&request.params),
            "memory.worker.show" => self.memory_worker_show(request.params),
            "memory.worker.configure" => self.memory_worker_configure(request.params),
            "provider.config.select.preview" => {
                self.configuration_select_preview(request.params).await
            }
            "provider.config.apply" => self.configuration_apply(request.params).await,
            "provider.config.rollback" => self.configuration_rollback(request.params),
            "provider.config.operations.show" => self.configuration_show(request.params),
            "terminal.open" | "terminal.list" | "terminal.show" | "terminal.resize"
            | "terminal.stop" => self.terminal_control(&request.method, request.params).await,
            "project.register" => self.register(&request),
            "project.list" => self
                .store
                .list_projects()
                .map(to_value)
                .map_err(store_error),
            "events.list" => self.list_events(request.params),
            "desktop.snapshot" => self.desktop_snapshot(&request.params),
            "provider.list" => self.provider_list(&request.params),
            "provider.config" => self.provider_config(&request.params),
            "provider.config.preview" => self.provider_config_preview(&request.params),
            "provider.probe" => self.provider_probe(request.params).await,
            "provider.smoke" => self.provider_smoke(request.params).await,
            "employee.create" => self.create_employee(&request).await,
            "employee.list" => self.employee_list(&request.params),
            "employee.show" => self.employee_show(request.params),
            "employee.update" => self.update_employee(&request).await,
            "employee.disable" => self.disable_employee(&request),
            "roster.get" => self.roster_get(&request.params),
            "roster.refresh" => self.refresh_roster(&request).await,
            "session.warm" => self.warm_session(&request).await,
            "session.list" => self.session_list(&request.params),
            "session.show" => self.session_show(request.params),
            "session.stop" => self.stop_session(&request).await,
            "session.resources" => self.session_resources(&request.params).await,
            "session.terminal.attach" => self.session_terminal_attach(request.params).await,
            "session.terminal.status" => self.session_terminal_status(request.params).await,
            "conversation.create" => self.create_conversation(&request),
            "conversation.list" => self.conversation_list(request.params),
            "conversation.show" => self.conversation_show(request.params),
            "conversation.rename" => self.rename_conversation(&request),
            "turn.submit" => self.submit_turn(&request).await,
            "turn.retry" => self.retry_turn(&request).await,
            "turn.cancel" => self.cancel_turn(&request).await,
            "turn.list" => self.turn_list(request.params),
            "turn.show" => self.turn_show(request.params),
            "task.delegate" => self.delegate_task(&request),
            "task.accept" => self.accept_task(&request),
            "task.start" => self.start_task(&request),
            "task.report" => self.report_task(&request),
            "task.cancel" => self.cancel_task(&request).await,
            "task.resolve" => self.resolve_task(&request),
            "task.list" => self.task_list(&request.params),
            "task.show" => self.task_show(&request.params),
            "approval.decide" => self.decide_approval(&request),
            "approval.list" => self.approval_list(&request.params),
            "approval.show" => self.approval_show(&request.params),
            "inbox.list" => self.inbox_list(&request.params),
            "inbox.counts" => self.inbox_counts(&request.params),
            "workspace.conversations" => self.workspace_conversations(&request.params),
            "workspace.launcher" => self.workspace_launcher(&request.params),
            "workspace.conversation" => self.workspace_conversation(&request.params),
            "incident.list" => self.incident_list(&request.params),
            "incident.show" => self.incident_show(&request.params),
            "incident.acknowledge" => self.acknowledge_incident(&request),
            "incident.resolve" => self.resolve_incident(&request),
            "incident.threshold.list" => self.incident_threshold_list(&request.params),
            "incident.threshold.set" => self.set_incident_threshold(&request.params),
            "incident.threshold.reset" => self.reset_incident_threshold(&request.params),
            "message.send" => self.send_message(&request),
            "message.claim" => self.claim_message(&request),
            "message.acknowledge" => self.acknowledge_message(&request),
            "message.fail" => self.fail_message(&request),
            "message.retry" => self.retry_message(&request),
            "message.cancel" => self.cancel_message(&request),
            "message.list" => self.message_list(request.params),
            "message.show" => self.message_show(request.params),
            "system.resources" => self.system_resources(&request.params).await,
            "system.shutdown" => {
                let _ = self.shutdown.send(true);
                Ok(json!({"planned": true}))
            }
            method => Err(RpcError {
                code: "LZ-METHOD-NOT-FOUND".to_owned(),
                message: "method not found".to_owned(),
                data: Some(json!({"method": method})),
            }),
        };
        match result {
            Ok(value) => JsonRpcResponse::success(request.id, value),
            Err(error) => JsonRpcResponse::failure(request.id, error),
        }
    }

    /// Retain a live receiver before fixing the durable replay interval. Callers drain with
    /// `subscription_page`; this deliberately avoids materializing an unbounded backlog.
    pub fn subscribe(&self, after: u64) -> Result<EventSubscription, lazoo_store::StoreError> {
        self.subscription(after)
    }

    pub fn subscription(&self, after: u64) -> Result<EventSubscription, lazoo_store::StoreError> {
        let _ = i64::try_from(after).map_err(|_| lazoo_store::StoreError::NegativeSequence)?;
        // Subscribe first so every event after this durable watermark remains in `live` even if
        // replay writes take time. The server deduplicates the overlap by sequence.
        let live = self.events.subscribe();
        let replay_high_water = self.store.status()?.event_high_water;
        Ok(EventSubscription {
            replay_high_water,
            live,
        })
    }

    /// One bounded durable page from a subscription's fixed replay interval.
    pub fn subscription_page(
        &self,
        after: u64,
        replay_high_water: u64,
    ) -> Result<Vec<EventSummary>, lazoo_store::StoreError> {
        Ok(self
            .store
            .list_events(after, 1024)?
            .into_iter()
            .take_while(|event| event.sequence <= replay_high_water)
            .collect())
    }

    #[must_use]
    pub fn shutdown_receiver(&self) -> watch::Receiver<bool> {
        self.shutdown.subscribe()
    }

    #[must_use]
    pub fn data_root(&self) -> &DataRoot {
        &self.root
    }

    fn status(&self) -> Result<Value, RpcError> {
        let status = self.store.status().map_err(store_error)?;
        Ok(to_value(self.system_status(status)))
    }

    fn system_status(&self, status: StoreStatus) -> SystemStatus {
        let uptime_millis = u64::try_from(self.started.elapsed().as_millis()).unwrap_or(u64::MAX);
        let keeper = read_optional_json(self.root.keeper_state_file());
        SystemStatus {
            daemon_version: env!("CARGO_PKG_VERSION").to_owned(),
            boot_id: self.boot_id,
            uptime_millis,
            protocol_version: PROTOCOL_V1,
            data_root: self.root.root().to_path_buf(),
            schema_version: status.schema_version,
            journal_mode: status.journal_mode,
            project_count: status.project_count,
            event_high_water: status.event_high_water,
            pending_outbox: status.outbox_pending,
            emergency_incident_count: status.emergency_incident_count,
            keeper,
            degraded: Vec::new(),
        }
    }

    fn register(&self, request: &JsonRpcRequest) -> Result<Value, RpcError> {
        let params: RegisterProjectParams = serde_json::from_value(request.params.clone())
            .map_err(|error| invalid_params(error.to_string()))?;
        if params.idempotency_key.trim().is_empty() {
            return Err(invalid_params("idempotencyKey must not be empty"));
        }
        let inspection = inspect_git_repository(&params.path).map_err(|error| RpcError {
            code: "LZ-PROJECT-INVALID".to_owned(),
            message: error.to_string(),
            data: None,
        })?;
        let display_name = params.display_name.unwrap_or_else(|| {
            inspection.canonical_worktree.file_name().map_or_else(
                || inspection.canonical_worktree.to_string_lossy().into_owned(),
                |name| name.to_string_lossy().into_owned(),
            )
        });
        let result = self
            .store
            .register_project(&RegisterProjectInput {
                request_id: request.id.clone(),
                idempotency_key: params.idempotency_key,
                repository_identity: inspection.identity,
                canonical_path: inspection.canonical_worktree,
                display_name,
                checkout_mode: params.checkout_mode,
            })
            .map_err(store_error)?;
        self.publish_events()?;
        Ok(to_value(result))
    }

    fn list_events(&self, value: Value) -> Result<Value, RpcError> {
        let params: EventsListParams =
            serde_json::from_value(value).map_err(|error| invalid_params(error.to_string()))?;
        if params.limit == 0 || params.limit > 1024 {
            return Err(invalid_params("limit must be between 1 and 1024"));
        }
        self.store
            .list_events(params.after, params.limit)
            .map(to_value)
            .map_err(store_error)
    }

    fn desktop_snapshot(&self, params: &Value) -> Result<Value, RpcError> {
        require_empty_params(params, "desktop.snapshot")?;
        let store_status = self.store.status().map_err(store_error)?;
        let recent_after = store_status.event_high_water.saturating_sub(100);
        let projects = self.store.list_projects().map_err(store_error)?;
        let events = self
            .store
            .list_events(recent_after, 100)
            .map_err(store_error)?;
        Ok(to_value(DesktopSnapshot {
            captured_at: Utc::now().to_rfc3339_opts(SecondsFormat::Millis, true),
            status: self.system_status(store_status),
            projects,
            events,
        }))
    }

    fn provider_list(&self, params: &Value) -> Result<Value, RpcError> {
        require_empty_params(params, "provider.list")?;
        let providers = self
            .providers
            .discover(&self.provider_environment)
            .into_iter()
            .map(|discovery| ProviderListEntry {
                provider: discovery.provider.to_string(),
                detected: discovery.candidate.is_some(),
                executable: discovery
                    .candidate
                    .as_ref()
                    .map(|candidate| candidate.path.clone()),
                discovery_source: discovery
                    .candidate
                    .map(|candidate| discovery_source_name(candidate.source).to_owned()),
            })
            .collect::<Vec<_>>();
        Ok(to_value(providers))
    }

    /// Read-only configuration discovery (brief section 16.1): known files parsed, secrets
    /// redacted, integrations hashed, duplicates and conflicts named. Nothing is written.
    fn provider_config(&self, params: &Value) -> Result<Value, RpcError> {
        require_empty_params(params, "provider.config")?;
        Ok(to_value(lazoo_providers::discover_provider_configuration(
            &self.provider_environment,
        )))
    }

    /// Brief section 16.1 step 6: the migration preview over the discovery report, derived on
    /// every call. Nothing is selected, written, or stored.
    fn provider_config_preview(&self, params: &Value) -> Result<Value, RpcError> {
        require_empty_params(params, "provider.config.preview")?;
        Ok(to_value(lazoo_providers::migration_preview(
            &lazoo_providers::discover_provider_configuration(&self.provider_environment),
        )))
    }

    async fn provider_probe(&self, value: Value) -> Result<Value, RpcError> {
        let params: ProviderProbeParams =
            serde_json::from_value(value).map_err(|error| invalid_params(error.to_string()))?;
        let provider = ProviderKind::from_str(&params.provider).map_err(provider_error)?;
        self.providers
            .probe(provider, &self.provider_environment)
            .await
            .map(to_value)
            .map_err(provider_error)
    }

    async fn provider_smoke(&self, value: Value) -> Result<Value, RpcError> {
        const MAX_PROMPT_BYTES: usize = 16 * 1024;
        let params: ProviderSmokeParams =
            serde_json::from_value(value).map_err(|error| invalid_params(error.to_string()))?;
        if params.prompt.trim().is_empty() || params.prompt.len() > MAX_PROMPT_BYTES {
            return Err(invalid_params(
                "prompt must contain between 1 and 16384 UTF-8 bytes",
            ));
        }
        let provider = ProviderKind::from_str(&params.provider).map_err(provider_error)?;
        let cwd = std::env::current_dir()
            .map_err(ProviderError::from)
            .map_err(provider_error)?;
        let request = SessionRequest::new(cwd).with_permission_mode("untrusted");
        let session = self
            .session_backend
            .start(provider, &request)
            .await
            .map_err(provider_error)?;
        let session_id = session.id();
        let turn = session
            .turn_observed(&TurnRequest::new(params.prompt), &SmokeObserver)
            .await;
        let stop = session.stop().await;
        let events = turn.map_err(provider_error)?;
        stop.map_err(provider_error)?;
        if smoke_turn_answered(&events) {
            self.record_verified_observation(provider).await?;
        }
        let (events, transcript_truncated) = concise_events(events);
        Ok(to_value(ProviderSmokeResult {
            provider: provider.to_string(),
            session_id,
            events,
            transcript_truncated,
            checked_at: Utc::now().to_rfc3339_opts(SecondsFormat::Millis, true),
        }))
    }

    /// A smoke turn that a model answered is the only evidence of model readiness the product
    /// has, so it is recorded as the provider's `verified` observation through the same store
    /// path a roster refresh uses (the roster revision advances and `roster.refreshed` is
    /// emitted only when the effective observation changed).
    ///
    /// The version, protocol, capabilities, and check time come from a fresh probe taken now.
    /// A probe that finds the binary gone or degraded cannot describe the provider that just
    /// answered, so the stored observation is used instead; with neither there is nothing
    /// truthful to record and the smoke result is returned unchanged. Authentication is
    /// `native: ready`, since a model answered through the native channel, and `external` stays
    /// as the base observation reported it, else `unknown`.
    async fn record_verified_observation(&self, provider: ProviderKind) -> Result<(), RpcError> {
        let fresh = self.observe_provider(provider).await;
        let usable = !matches!(fresh.verification.as_str(), "not-detected" | "degraded");
        let base = if usable {
            Some(fresh)
        } else {
            self.store
                .roster_snapshot()
                .map_err(store_error)?
                .entries
                .into_iter()
                .find_map(|entry| {
                    entry
                        .provider_observation
                        .filter(|observation| observation.provider == provider.to_string())
                })
        };
        let Some(mut observation) = base else {
            return Ok(());
        };
        let external = observation
            .authentication
            .get("external")
            .and_then(Value::as_str)
            .unwrap_or("unknown")
            .to_owned();
        "verified".clone_into(&mut observation.verification);
        observation.degradation_reason = None;
        observation.authentication = json!({"native": "ready", "external": external});
        let key = format!("provider-smoke:{}", Uuid::new_v4());
        self.store
            .refresh_provider_observations(&RefreshProviderObservationsInput {
                request_id: key.clone(),
                idempotency_key: key,
                observations: vec![observation],
            })
            .map_err(store_error)?;
        self.publish_events()
    }

    async fn create_employee(&self, request: &JsonRpcRequest) -> Result<Value, RpcError> {
        let params: CreateEmployeeParams = serde_json::from_value(request.params.clone())
            .map_err(|error| invalid_params(error.to_string()))?;
        require_idempotency_key(&params.idempotency_key)?;
        let name = validate_employee_name(&params.name)
            .map_err(|error| employee_validation_error(&error))?;
        validate_employee_text_fields(
            &params.role,
            &params.instructions,
            params.model.as_deref(),
            params.reasoning_effort.as_deref(),
        )
        .map_err(|error| employee_validation_error(&error))?;
        validate_native_settings(&params.native_settings)
            .map_err(|error| employee_validation_error(&error))?;
        validate_idle_timeout(params.idle_timeout_seconds)
            .map_err(|error| employee_validation_error(&error))?;
        let provider = employee_provider(&params.provider)?;
        let observation = self.observe_provider(provider).await;
        let result = self
            .store
            .create_employee(&CreateEmployeeInput {
                request_id: request.id.clone(),
                idempotency_key: params.idempotency_key,
                name,
                provider: provider.to_string(),
                role: params.role.trim().to_owned(),
                instructions: params.instructions,
                model: normalized_optional(params.model),
                reasoning_effort: normalized_optional(params.reasoning_effort),
                native_settings: params.native_settings,
                authority_mode: params.authority_mode,
                checkout_policy: params.checkout_policy,
                activation_policy: params.activation_policy,
                idle_timeout_seconds: params.idle_timeout_seconds,
                enabled: params.enabled,
                provider_observation: Some(observation),
            })
            .map_err(store_error)?;
        self.publish_events()?;
        Ok(to_value(result))
    }

    fn employee_list(&self, params: &Value) -> Result<Value, RpcError> {
        require_empty_params(params, "employee.list")?;
        self.store
            .list_employees()
            .map(to_value)
            .map_err(store_error)
    }

    fn employee_show(&self, value: Value) -> Result<Value, RpcError> {
        let params: EmployeeShowParams =
            serde_json::from_value(value).map_err(|error| invalid_params(error.to_string()))?;
        self.store
            .get_employee(params.id)
            .map(to_value)
            .map_err(store_error)
    }

    async fn update_employee(&self, request: &JsonRpcRequest) -> Result<Value, RpcError> {
        let params: UpdateEmployeeParams = serde_json::from_value(request.params.clone())
            .map_err(|error| invalid_params(error.to_string()))?;
        require_idempotency_key(&params.idempotency_key)?;
        if params.expected_revision == 0 {
            return Err(employee_invalid(
                "expectedRevision must be greater than zero",
            ));
        }
        let current = self.store.get_employee(params.id).map_err(store_error)?;
        let name = params
            .name
            .as_deref()
            .map(validate_employee_name)
            .transpose()
            .map_err(|error| employee_validation_error(&error))?;
        let provider = params
            .provider
            .as_deref()
            .map(employee_provider)
            .transpose()?;
        let desired_role = params.role.as_deref().unwrap_or(&current.role);
        let desired_instructions = params
            .instructions
            .as_deref()
            .unwrap_or(&current.instructions);
        let desired_model = params.model.as_deref().or(current.model.as_deref());
        let desired_effort = params
            .reasoning_effort
            .as_deref()
            .or(current.reasoning_effort.as_deref());
        validate_employee_text_fields(
            desired_role,
            desired_instructions,
            desired_model,
            desired_effort,
        )
        .map_err(|error| employee_validation_error(&error))?;
        if let Some(native_settings) = &params.native_settings {
            validate_native_settings(native_settings)
                .map_err(|error| employee_validation_error(&error))?;
        }
        if let Some(seconds) = params.idle_timeout_seconds {
            validate_idle_timeout(seconds).map_err(|error| employee_validation_error(&error))?;
        }
        let should_probe = provider.is_some_and(|value| value.to_string() != current.provider)
            || params.enabled == Some(true) && !current.enabled;
        let provider_observation = if should_probe {
            let desired_provider = provider.unwrap_or_else(|| {
                ProviderKind::from_str(&current.provider)
                    .expect("stored employee providers are validated")
            });
            Some(self.observe_provider(desired_provider).await)
        } else {
            None
        };
        let result = self
            .store
            .update_employee(&UpdateEmployeeInput {
                request_id: request.id.clone(),
                idempotency_key: params.idempotency_key,
                employee_id: params.id,
                expected_revision: params.expected_revision,
                name,
                provider: provider.map(|value| value.to_string()),
                role: params.role.map(|value| value.trim().to_owned()),
                instructions: params.instructions,
                model: normalized_optional(params.model),
                reasoning_effort: normalized_optional(params.reasoning_effort),
                native_settings: params.native_settings,
                authority_mode: params.authority_mode,
                checkout_policy: params.checkout_policy,
                activation_policy: params.activation_policy,
                idle_timeout_seconds: params.idle_timeout_seconds,
                enabled: params.enabled,
                provider_observation,
            })
            .map_err(store_error)?;
        self.publish_events()?;
        Ok(to_value(result))
    }

    fn disable_employee(&self, request: &JsonRpcRequest) -> Result<Value, RpcError> {
        let params: DisableEmployeeParams = serde_json::from_value(request.params.clone())
            .map_err(|error| invalid_params(error.to_string()))?;
        require_idempotency_key(&params.idempotency_key)?;
        if params.expected_revision == 0 {
            return Err(employee_invalid(
                "expectedRevision must be greater than zero",
            ));
        }
        let result = self
            .store
            .disable_employee(&DisableEmployeeInput {
                request_id: request.id.clone(),
                idempotency_key: params.idempotency_key,
                employee_id: params.id,
                expected_revision: params.expected_revision,
            })
            .map_err(store_error)?;
        self.publish_events()?;
        Ok(to_value(result))
    }

    fn roster_get(&self, params: &Value) -> Result<Value, RpcError> {
        require_empty_params(params, "roster.get")?;
        self.store
            .roster_snapshot()
            .map(to_value)
            .map_err(store_error)
    }

    async fn refresh_roster(&self, request: &JsonRpcRequest) -> Result<Value, RpcError> {
        let params: RosterRefreshParams = serde_json::from_value(request.params.clone())
            .map_err(|error| invalid_params(error.to_string()))?;
        require_idempotency_key(&params.idempotency_key)?;
        let providers = self
            .store
            .list_employees()
            .map_err(store_error)?
            .into_iter()
            .filter(|employee| employee.enabled)
            .map(|employee| employee.provider)
            .collect::<BTreeSet<_>>();
        let mut observations = Vec::with_capacity(providers.len());
        for provider in providers {
            let provider = employee_provider(&provider)?;
            observations.push(self.observe_provider(provider).await);
        }
        let result = self
            .store
            .refresh_provider_observations(&RefreshProviderObservationsInput {
                request_id: request.id.clone(),
                idempotency_key: params.idempotency_key,
                observations,
            })
            .map_err(store_error)?;
        self.publish_events()?;
        Ok(to_value(result))
    }

    async fn warm_session(&self, request: &JsonRpcRequest) -> Result<Value, RpcError> {
        let params: WarmSessionParams = serde_json::from_value(request.params.clone())
            .map_err(|error| invalid_params(error.to_string()))?;
        require_session_idempotency_key(&params.idempotency_key)?;
        if params.expected_employee_revision == 0 {
            return Err(session_invalid(
                "expectedEmployeeRevision must be greater than zero",
            ));
        }
        let employee = self
            .store
            .get_employee(params.employee_id)
            .map_err(session_store_error)?;
        let provider = ProviderKind::from_str(&employee.provider).map_err(provider_error)?;
        let starting = self
            .store
            .begin_session(&BeginSessionInput {
                request_id: request.id.clone(),
                idempotency_key: params.idempotency_key,
                employee_id: params.employee_id,
                expected_employee_revision: params.expected_employee_revision,
                owner_boot_id: self.boot_id,
            })
            .map_err(session_store_error)?;
        self.publish_events()?;
        if starting.event_sequence.is_none() {
            return Ok(to_value(starting));
        }
        self.start_new_session(employee, provider, starting, None)
            .await
            .map(to_value)
    }

    async fn start_new_session(
        &self,
        employee: lazoo_protocol::EmployeeRecord,
        provider: ProviderKind,
        starting: lazoo_protocol::SessionMutationResult,
        working_directory: Option<std::path::PathBuf>,
    ) -> Result<lazoo_protocol::SessionMutationResult, RpcError> {
        let session_id = starting.session.id;
        // Task dispatch passes its own workspace and never resumes; every other start may
        // continue the employee's suspended predecessor.
        let resume = if working_directory.is_none() {
            self.decide_session_resume(&employee, provider, session_id)
                .await
        } else {
            resume::ResumeDecision::Fresh
        };
        let working_directory = working_directory
            .or_else(|| resume.working_directory())
            .map_or_else(|| self.root.ensure_session_working_dir(session_id), Ok);
        let Ok(working_directory) = working_directory else {
            self.record_session_failure(
                session_id,
                starting.session.revision,
                "LZ-SESSION-START-FAILED",
                "session working directory could not be prepared",
            )?;
            return Err(session_failure(
                "LZ-SESSION-START-FAILED",
                "session working directory could not be prepared",
                session_id,
            ));
        };
        let mut session_request = SessionRequest::new(working_directory)
            .with_permission_mode(authority_permission_mode(employee.authority_mode));
        if let Some(model) = employee.model {
            session_request = session_request.with_model(model);
        }
        if let Some(effort) = employee.reasoning_effort {
            session_request = session_request.with_effort(effort);
        }
        if provider == ProviderKind::Codex {
            session_request = session_request.with_tools(tools::collaboration_tools());
        }
        if let Some(native_session_id) = resume.native_session_id() {
            session_request = session_request.resume(native_session_id);
        }

        let Ok(session) = self.session_backend.start(provider, &session_request).await else {
            self.record_session_failure(
                session_id,
                starting.session.revision,
                "LZ-SESSION-START-FAILED",
                "provider session did not become ready",
            )?;
            return Err(session_failure(
                "LZ-SESSION-START-FAILED",
                "provider session did not become ready",
                session_id,
            ));
        };
        let session = Arc::<dyn ManagedSession>::from(session);
        let native_session_id = session.id();
        let Some(native_host_pid) = session.native_host_pid() else {
            let _ = timeout(Duration::from_secs(10), session.stop()).await;
            self.record_session_failure(
                session_id,
                starting.session.revision,
                "LZ-SESSION-START-FAILED",
                "provider session did not report an owned host process",
            )?;
            return Err(session_failure(
                "LZ-SESSION-START-FAILED",
                "provider session did not report an owned host process",
                session_id,
            ));
        };
        self.live_sessions
            .lock()
            .await
            .insert(session_id, Arc::clone(&session));
        let ready = match self.store.mark_session_ready(&MarkSessionReadyInput {
            session_id,
            expected_revision: starting.session.revision,
            owner_boot_id: self.boot_id,
            native_session_id,
            native_host_pid,
        }) {
            Ok(ready) => ready,
            Err(error) => {
                self.live_sessions.lock().await.remove(&session_id);
                let _ = timeout(Duration::from_secs(10), session.stop()).await;
                let _ = self.record_session_failure(
                    session_id,
                    starting.session.revision,
                    "LZ-SESSION-START-FAILED",
                    "session readiness could not be recorded",
                );
                return Err(session_store_error(error));
            }
        };
        self.record_resume_decision(session_id, &resume, &ready.session);
        self.publish_events()?;
        Ok(ready)
    }

    fn session_list(&self, params: &Value) -> Result<Value, RpcError> {
        require_empty_params(params, "session.list")?;
        self.store
            .list_sessions()
            .map(to_value)
            .map_err(session_store_error)
    }

    fn session_show(&self, value: Value) -> Result<Value, RpcError> {
        let params: SessionShowParams =
            serde_json::from_value(value).map_err(|error| invalid_params(error.to_string()))?;
        self.store
            .get_session(params.id)
            .map(to_value)
            .map_err(session_store_error)
    }

    async fn stop_session(&self, request: &JsonRpcRequest) -> Result<Value, RpcError> {
        let params: StopSessionParams = serde_json::from_value(request.params.clone())
            .map_err(|error| invalid_params(error.to_string()))?;
        require_session_idempotency_key(&params.idempotency_key)?;
        if params.expected_revision == 0 {
            return Err(session_invalid(
                "expectedRevision must be greater than zero",
            ));
        }
        self.stop_session_command(
            params.id,
            params.expected_revision,
            request.id.clone(),
            params.idempotency_key,
        )
        .await
        .map(to_value)
    }

    async fn stop_session_command(
        &self,
        session_id: Uuid,
        expected_revision: u64,
        request_id: String,
        idempotency_key: String,
    ) -> Result<lazoo_protocol::SessionMutationResult, RpcError> {
        let stopping = self
            .store
            .begin_session_stop(&BeginSessionStopInput {
                request_id,
                idempotency_key,
                session_id,
                expected_revision,
            })
            .map_err(session_store_error)?;
        self.finish_session_stop_command(stopping).await
    }

    /// Stops the native session behind a session the store has just marked `stopping`, or
    /// returns a replayed stop unchanged.
    async fn finish_session_stop_command(
        &self,
        stopping: lazoo_protocol::SessionMutationResult,
    ) -> Result<lazoo_protocol::SessionMutationResult, RpcError> {
        self.publish_events()?;
        if stopping.event_sequence.is_none() {
            self.task_dispatch_notify.notify_one();
            return Ok(stopping);
        }

        let session_id = stopping.session.id;
        let session = self.live_sessions.lock().await.remove(&session_id);
        let Some(session) = session else {
            self.record_session_failure(
                session_id,
                stopping.session.revision,
                "LZ-SESSION-HANDLE-MISSING",
                "the daemon no longer owns a live handle for this session",
            )?;
            self.task_dispatch_notify.notify_one();
            return Err(session_failure(
                "LZ-SESSION-HANDLE-MISSING",
                "the daemon no longer owns a live handle for this session",
                session_id,
            ));
        };
        if let Ok(Ok(())) = timeout(Duration::from_secs(10), session.stop()).await {
            let stopped = self
                .store
                .finish_session_stop(&FinishSessionStopInput {
                    session_id,
                    expected_revision: stopping.session.revision,
                })
                .map_err(session_store_error)?;
            self.publish_events()?;
            self.task_dispatch_notify.notify_one();
            Ok(stopped)
        } else {
            drop(session);
            self.record_session_failure(
                session_id,
                stopping.session.revision,
                "LZ-SESSION-STOP-FAILED",
                "provider session did not stop cleanly",
            )?;
            self.task_dispatch_notify.notify_one();
            Err(session_failure(
                "LZ-SESSION-STOP-FAILED",
                "provider session did not stop cleanly",
                session_id,
            ))
        }
    }

    fn record_session_failure(
        &self,
        session_id: Uuid,
        expected_revision: u64,
        code: &str,
        message: &str,
    ) -> Result<(), RpcError> {
        let failed = self
            .store
            .fail_session(&FailSessionInput {
                session_id,
                expected_revision,
                failure_code: code.to_owned(),
                failure_message: message.to_owned(),
            })
            .map_err(session_store_error)?;
        self.publish_events()?;
        self.note_session_incident(&failed.session, code);
        Ok(())
    }

    fn send_message(&self, request: &JsonRpcRequest) -> Result<Value, RpcError> {
        let params: SendMessageParams = serde_json::from_value(request.params.clone())
            .map_err(|error| message_invalid(error.to_string()))?;
        let result = self
            .store
            .send_message(&SendMessageInput {
                request_id: request.id.clone(),
                idempotency_key: params.idempotency_key,
                sender: params.sender,
                recipient: params.recipient,
                project_id: params.project_id,
                conversation_id: params.conversation_id,
                task_id: params.task_id,
                correlation_id: params.correlation_id,
                causation_id: params.causation_id,
                kind: params.kind,
                priority: params.priority,
                body: params.body,
                references: params.references,
                max_attempts: params.max_attempts,
            })
            .map_err(message_store_error)?;
        self.publish_events()?;
        Ok(to_value(result))
    }

    fn claim_message(&self, request: &JsonRpcRequest) -> Result<Value, RpcError> {
        let params: ClaimMessageParams = serde_json::from_value(request.params.clone())
            .map_err(|error| message_invalid(error.to_string()))?;
        require_message_revision_value(params.expected_revision)?;
        let result = self
            .store
            .claim_message(&ClaimMessageInput {
                request_id: request.id.clone(),
                idempotency_key: params.idempotency_key,
                message_id: params.id,
                expected_revision: params.expected_revision,
            })
            .map_err(message_store_error)?;
        self.publish_events()?;
        Ok(to_value(result))
    }

    fn acknowledge_message(&self, request: &JsonRpcRequest) -> Result<Value, RpcError> {
        let params: AcknowledgeMessageParams = serde_json::from_value(request.params.clone())
            .map_err(|error| message_invalid(error.to_string()))?;
        require_message_revision_value(params.expected_revision)?;
        let result = self
            .store
            .acknowledge_message(&AcknowledgeMessageInput {
                request_id: request.id.clone(),
                idempotency_key: params.idempotency_key,
                message_id: params.id,
                expected_revision: params.expected_revision,
                attempt_id: params.attempt_id,
                acknowledgement_key: params.acknowledgement_key,
                evidence: params.evidence,
            })
            .map_err(message_store_error)?;
        self.publish_events()?;
        Ok(to_value(result))
    }

    fn fail_message(&self, request: &JsonRpcRequest) -> Result<Value, RpcError> {
        let params: FailMessageParams = serde_json::from_value(request.params.clone())
            .map_err(|error| message_invalid(error.to_string()))?;
        require_message_revision_value(params.expected_revision)?;
        let result = self
            .store
            .fail_message(&FailMessageInput {
                request_id: request.id.clone(),
                idempotency_key: params.idempotency_key,
                message_id: params.id,
                expected_revision: params.expected_revision,
                attempt_id: params.attempt_id,
                retryable: params.retryable,
                failure_code: params.failure_code,
                failure_message: params.failure_message,
            })
            .map_err(message_store_error)?;
        self.publish_events()?;
        if result.event_sequence.is_some() {
            self.note_dead_letter(&result.message);
        }
        Ok(to_value(result))
    }

    fn retry_message(&self, request: &JsonRpcRequest) -> Result<Value, RpcError> {
        let params: RetryMessageParams = serde_json::from_value(request.params.clone())
            .map_err(|error| message_invalid(error.to_string()))?;
        require_message_revision_value(params.expected_revision)?;
        let result = self
            .store
            .retry_message(&RetryMessageInput {
                request_id: request.id.clone(),
                idempotency_key: params.idempotency_key,
                message_id: params.id,
                expected_revision: params.expected_revision,
            })
            .map_err(message_store_error)?;
        self.publish_events()?;
        Ok(to_value(result))
    }

    fn cancel_message(&self, request: &JsonRpcRequest) -> Result<Value, RpcError> {
        let params: CancelMessageParams = serde_json::from_value(request.params.clone())
            .map_err(|error| message_invalid(error.to_string()))?;
        require_message_revision_value(params.expected_revision)?;
        let result = self
            .store
            .cancel_message(&CancelMessageInput {
                request_id: request.id.clone(),
                idempotency_key: params.idempotency_key,
                message_id: params.id,
                expected_revision: params.expected_revision,
            })
            .map_err(message_store_error)?;
        self.publish_events()?;
        Ok(to_value(result))
    }

    fn message_list(&self, value: Value) -> Result<Value, RpcError> {
        let params: MessageListParams =
            serde_json::from_value(value).map_err(|error| message_invalid(error.to_string()))?;
        if params.limit == 0 || params.limit > 1024 {
            return Err(message_invalid("limit must be between 1 and 1024"));
        }
        self.store
            .list_messages(params.after_sequence, params.limit)
            .map(to_value)
            .map_err(message_store_error)
    }

    fn message_show(&self, value: Value) -> Result<Value, RpcError> {
        let params: MessageShowParams =
            serde_json::from_value(value).map_err(|error| message_invalid(error.to_string()))?;
        self.store
            .get_message(params.id)
            .map(to_value)
            .map_err(message_store_error)
    }

    pub async fn shutdown_live_sessions(&self) {
        self.live_turns.lock().await.clear();
        let session_ids = self
            .live_sessions
            .lock()
            .await
            .keys()
            .copied()
            .collect::<Vec<_>>();
        for session_id in session_ids {
            let Ok(session) = self.store.get_session(session_id) else {
                self.live_sessions.lock().await.remove(&session_id);
                continue;
            };
            if session.state == lazoo_protocol::SessionLifecycleState::Idle {
                let _ = self
                    .stop_session_command(
                        session_id,
                        session.revision,
                        format!("shutdown-{}-{session_id}", self.boot_id),
                        format!("shutdown-{}-{session_id}", self.boot_id),
                    )
                    .await;
                continue;
            }
            if let Some(handle) = self.live_sessions.lock().await.remove(&session_id) {
                let _ = timeout(Duration::from_secs(10), handle.stop()).await;
            }
            if matches!(
                session.state,
                lazoo_protocol::SessionLifecycleState::Starting
                    | lazoo_protocol::SessionLifecycleState::Stopping
            ) {
                let _ = self.record_session_failure(
                    session_id,
                    session.revision,
                    "LZ-SESSION-SHUTDOWN",
                    "daemon shutdown interrupted the session transition",
                );
            }
        }
    }

    async fn observe_provider(&self, provider: ProviderKind) -> ProviderObservation {
        match self
            .providers
            .probe(provider, &self.provider_environment)
            .await
        {
            Ok(probe) => observation_from_probe(probe),
            Err(error) => observation_from_error(provider, &error),
        }
    }

    fn publish_events(&self) -> Result<(), RpcError> {
        for event in self.store.publish_pending_outbox().map_err(store_error)? {
            let _ = self.events.send(event);
        }
        self.store_changed.notify_waiters();
        Ok(())
    }
}

fn require_empty_params(params: &Value, method: &str) -> Result<(), RpcError> {
    if params.as_object().is_some_and(serde_json::Map::is_empty) {
        Ok(())
    } else {
        Err(invalid_params(format!(
            "{method} accepts only an empty object"
        )))
    }
}

fn require_idempotency_key(value: &str) -> Result<(), RpcError> {
    if value.trim().is_empty() {
        Err(employee_invalid("idempotencyKey must not be empty"))
    } else if value.len() > 256 {
        Err(employee_invalid(
            "idempotencyKey must contain at most 256 UTF-8 bytes",
        ))
    } else {
        Ok(())
    }
}

fn require_session_idempotency_key(value: &str) -> Result<(), RpcError> {
    if value.trim().is_empty() {
        Err(session_invalid("idempotencyKey must not be empty"))
    } else if value.len() > 256 {
        Err(session_invalid(
            "idempotencyKey must contain at most 256 UTF-8 bytes",
        ))
    } else {
        Ok(())
    }
}

/// Whether a provider enforces a task authority ceiling, so Lazoo may deliver turns and
/// dispatch tasks to it. Codex: its approval requests are bridged into the schema-8 ledger under
/// every authority. `OpenCode`: under every authority too, because the adapter launches
/// `opencode acp` with an inline `OPENCODE_PERMISSION` block chosen by the permission mode
/// (allow-all for `auto`; for `ask` and `accept-edits` every governed tool asks and only the
/// read-only tools are allowed by name) and the ACP bridge admits only command and file-change
/// requests into the ledger, declining every other kind. Claude Code: only `full-autonomy`,
/// which maps to its `auto` permission mode, the one mode in which no prompt can occur; under
/// `ask` and `self-decision` its prompts stay where Lazoo cannot answer them. Antigravity has no
/// prompt-free mode, and Grok Build (ACP) is not launched with a governed permission block, so
/// both stay refused.
pub(crate) const fn provider_enforces_authority(
    provider: ProviderKind,
    authority: AuthorityMode,
) -> bool {
    match provider {
        ProviderKind::Codex | ProviderKind::OpenCode => true,
        ProviderKind::ClaudeCode => matches!(authority, AuthorityMode::FullAutonomy),
        ProviderKind::GrokBuild | ProviderKind::Antigravity => false,
    }
}

/// Whether the session the employee's authority launches can serve a turn or task at
/// `authority`. `OpenCode` takes its permission block at launch and ACP cannot narrow it per
/// turn, so a session launched prompt-free (a `full-autonomy` employee) never serves a narrower
/// `ask` or `self-decision` turn: that would run it without the approvals the turn requires. A
/// governed session (`ask`, `self-decision`) serves any authority up to its own, and the
/// approval observer applies the turn's authority to each request.
pub(crate) const fn session_serves_authority(
    provider: ProviderKind,
    authority: AuthorityMode,
    employee_authority: AuthorityMode,
) -> bool {
    provider_enforces_authority(provider, authority)
        && !(matches!(provider, ProviderKind::OpenCode)
            && matches!(employee_authority, AuthorityMode::FullAutonomy)
            && !matches!(authority, AuthorityMode::FullAutonomy))
}

/// Whether a provider may run the memory worker's summary turns. Besides the governed `ask`
/// ceiling the summary turn must be read-only. The daemon declines every permission request on a
/// summary turn, but `OpenCode`'s ACP adapter rejects a `read_only` turn request
/// ("read-only turns" unsupported), so a summary could neither be proven read-only nor run there;
/// it stays excluded.
pub(crate) const fn provider_runs_memory_summaries(provider: ProviderKind) -> bool {
    provider_enforces_authority(provider, AuthorityMode::Ask)
        && !matches!(provider, ProviderKind::OpenCode)
}

const fn authority_permission_mode(authority: AuthorityMode) -> &'static str {
    match authority {
        AuthorityMode::Ask => "ask",
        AuthorityMode::SelfDecision => "accept-edits",
        AuthorityMode::FullAutonomy => "auto",
    }
}

fn normalized_optional(value: Option<String>) -> Option<String> {
    value.and_then(|value| {
        let value = value.trim();
        (!value.is_empty()).then(|| value.to_owned())
    })
}

fn employee_provider(value: &str) -> Result<ProviderKind, RpcError> {
    ProviderKind::from_str(value)
        .map_err(|_| employee_invalid(format!("unknown employee provider: {value}")))
}

fn employee_validation_error(error: &lazoo_domain::EmployeeValidationError) -> RpcError {
    employee_invalid(error.to_string())
}

fn employee_invalid(message: impl Into<String>) -> RpcError {
    RpcError {
        code: "LZ-EMPLOYEE-INVALID".to_owned(),
        message: message.into(),
        data: None,
    }
}

fn observation_from_probe(probe: ProviderProbe) -> ProviderObservation {
    let verification = serialized_name(probe.verification);
    let degradation_reason = match probe.verification {
        ProviderVerification::NotDetected => {
            Some("provider executable was not detected".to_owned())
        }
        ProviderVerification::Degraded => {
            let authentication_blocked = matches!(
                probe.authentication.native,
                AuthenticationState::NeedsLogin | AuthenticationState::Blocked
            ) || matches!(
                probe.authentication.external,
                AuthenticationState::NeedsLogin | AuthenticationState::Blocked
            );
            Some(if authentication_blocked {
                "provider authentication is not ready".to_owned()
            } else {
                "provider readiness probe failed".to_owned()
            })
        }
        _ => None,
    };
    ProviderObservation {
        provider: probe.provider.to_string(),
        verification,
        version: probe.version,
        protocol: probe.protocol,
        capabilities: to_value(probe.capabilities),
        authentication: json!({
            "native": serialized_name(probe.authentication.native),
            "external": serialized_name(probe.authentication.external),
        }),
        degradation_reason,
        checked_at: probe.checked_at,
    }
}

fn observation_from_error(provider: ProviderKind, error: &ProviderError) -> ProviderObservation {
    let (verification, reason) = match error {
        ProviderError::NotDetected(_) => (
            ProviderVerification::NotDetected,
            "provider executable was not detected",
        ),
        ProviderError::Authentication(_) => (
            ProviderVerification::Degraded,
            "provider authentication is not ready",
        ),
        ProviderError::Unsupported { .. } => (
            ProviderVerification::Degraded,
            "provider protocol is incompatible",
        ),
        ProviderError::Timeout { .. } | ProviderError::Stalled { .. } => (
            ProviderVerification::Degraded,
            "provider readiness probe timed out",
        ),
        ProviderError::ProcessExited { .. } => (
            ProviderVerification::Degraded,
            "provider readiness process exited",
        ),
        ProviderError::UnknownProvider(_)
        | ProviderError::Protocol(_)
        | ProviderError::InvalidEvent(_)
        | ProviderError::Observer(_)
        | ProviderError::Io(_)
        | ProviderError::Json(_)
        | ProviderError::Runtime(_) => (
            ProviderVerification::Degraded,
            "provider readiness probe failed",
        ),
    };
    ProviderObservation {
        provider: provider.to_string(),
        verification: serialized_name(verification),
        version: None,
        protocol: None,
        capabilities: to_value(ProviderCapabilities::default()),
        authentication: json!({"native": "unknown", "external": "unknown"}),
        degradation_reason: Some(reason.to_owned()),
        checked_at: Utc::now().to_rfc3339_opts(SecondsFormat::Millis, true),
    }
}

fn serialized_name(value: impl Serialize) -> String {
    serde_json::to_value(value)
        .expect("serializable enum")
        .as_str()
        .expect("enum serializes as a string")
        .to_owned()
}

fn discovery_source_name(source: DiscoverySource) -> &'static str {
    match source {
        DiscoverySource::InheritedPath => "inherited-path",
        DiscoverySource::PersistedUserPath => "persisted-user-path",
        DiscoverySource::PersistedMachinePath => "persisted-machine-path",
        DiscoverySource::KnownLocation => "known-location",
    }
}

/// The smoke turn declines every permission request, as `ProviderSession::turn` does, and
/// keeps no events of its own.
struct SmokeObserver;

impl TurnEventObserver for SmokeObserver {
    fn observe(&self, _event: &ProviderEvent) -> Result<(), ProviderError> {
        Ok(())
    }
}

/// Whether a model answered: the turn completed, no provider error was reported, and the
/// completion is not a failed, interrupted, or cancelled one. ACP reports its stop reason
/// (for example `end_turn`) as the completion status, so only the unsuccessful ones are named.
fn smoke_turn_answered(events: &[ProviderEvent]) -> bool {
    events
        .iter()
        .all(|event| !matches!(event, ProviderEvent::Error { .. }))
        && events.iter().any(|event| {
            matches!(
                event,
                ProviderEvent::TurnCompleted { status, .. }
                    if !matches!(status.as_str(), "failed" | "interrupted" | "cancelled")
            )
        })
}

fn concise_events(events: Vec<lazoo_providers::ProviderEvent>) -> (Vec<Value>, bool) {
    const MAX_EVENT_STRING_BYTES: usize = 64 * 1024;
    const MAX_TRANSCRIPT_BYTES: usize = 512 * 1024;
    let mut output = Vec::new();
    let mut used = 0usize;
    let mut truncated = false;
    for event in events {
        let mut value = to_value(event);
        let Some(object) = value.as_object_mut() else {
            continue;
        };
        object.remove("raw");
        for field in ["text", "output", "message"] {
            if let Some(Value::String(text)) = object.get_mut(field)
                && text.len() > MAX_EVENT_STRING_BYTES
            {
                truncate_utf8(text, MAX_EVENT_STRING_BYTES);
                truncated = true;
            }
        }
        let bytes = serde_json::to_vec(&value).map_or(MAX_TRANSCRIPT_BYTES, |bytes| bytes.len());
        if used.saturating_add(bytes) > MAX_TRANSCRIPT_BYTES {
            truncated = true;
            break;
        }
        used += bytes;
        output.push(value);
    }
    (output, truncated)
}

fn truncate_utf8(value: &mut String, maximum: usize) {
    let mut end = maximum.min(value.len());
    while end > 0 && !value.is_char_boundary(end) {
        end -= 1;
    }
    value.truncate(end);
}

fn read_optional_json(path: std::path::PathBuf) -> Option<Value> {
    fs::read(path)
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
}

fn to_value(value: impl Serialize) -> Value {
    serde_json::to_value(value).expect("serializable protocol value")
}

fn invalid_params(message: impl Into<String>) -> RpcError {
    RpcError {
        code: "LZ-INVALID-PARAMS".to_owned(),
        message: message.into(),
        data: None,
    }
}

#[allow(clippy::needless_pass_by_value)]
fn store_error(error: lazoo_store::StoreError) -> RpcError {
    let code = match &error {
        lazoo_store::StoreError::ProjectConflict { .. } => "LZ-PROJECT-CONFLICT",
        lazoo_store::StoreError::EmployeeConflict { .. } => "LZ-EMPLOYEE-CONFLICT",
        lazoo_store::StoreError::EmployeeNotFound { .. } => "LZ-EMPLOYEE-NOT-FOUND",
        lazoo_store::StoreError::EmployeeStale { .. } => "LZ-EMPLOYEE-STALE",
        lazoo_store::StoreError::EmployeeSessionActive { .. } => "LZ-EMPLOYEE-SESSION-ACTIVE",
        _ => "LZ-STORE",
    };
    let data = match &error {
        lazoo_store::StoreError::EmployeeStale {
            current_revision, ..
        } => Some(json!({"currentRevision": current_revision})),
        lazoo_store::StoreError::EmployeeSessionActive { session_id, .. } => {
            Some(json!({"sessionId": session_id}))
        }
        _ => None,
    };
    RpcError {
        code: code.to_owned(),
        message: error.to_string(),
        data,
    }
}

#[allow(clippy::needless_pass_by_value)]
fn session_store_error(error: lazoo_store::StoreError) -> RpcError {
    let (code, data) = match &error {
        lazoo_store::StoreError::EmployeeNotFound { employee_id } => (
            "LZ-SESSION-EMPLOYEE-NOT-FOUND",
            Some(json!({"employeeId": employee_id})),
        ),
        lazoo_store::StoreError::EmployeeStale {
            current_revision, ..
        } => (
            "LZ-SESSION-EMPLOYEE-STALE",
            Some(json!({"currentRevision": current_revision})),
        ),
        lazoo_store::StoreError::EmployeeDisabled { employee_id } => (
            "LZ-SESSION-EMPLOYEE-DISABLED",
            Some(json!({"employeeId": employee_id})),
        ),
        lazoo_store::StoreError::SessionConflict { session_id, .. } => (
            "LZ-SESSION-CONFLICT",
            Some(json!({"sessionId": session_id})),
        ),
        lazoo_store::StoreError::SessionNotFound { session_id } => (
            "LZ-SESSION-NOT-FOUND",
            Some(json!({"sessionId": session_id})),
        ),
        lazoo_store::StoreError::SessionStale {
            current_revision, ..
        } => (
            "LZ-SESSION-STALE",
            Some(json!({"currentRevision": current_revision})),
        ),
        lazoo_store::StoreError::SessionState { session_id, .. } => {
            ("LZ-SESSION-STATE", Some(json!({"sessionId": session_id})))
        }
        lazoo_store::StoreError::SessionOwnership { session_id } => (
            "LZ-SESSION-OWNERSHIP",
            Some(json!({"sessionId": session_id})),
        ),
        lazoo_store::StoreError::SessionProviderNotReady { employee_id, .. } => (
            "LZ-SESSION-PROVIDER-NOT-READY",
            Some(json!({"employeeId": employee_id})),
        ),
        _ => ("LZ-SESSION-STORE", None),
    };
    RpcError {
        code: code.to_owned(),
        message: error.to_string(),
        data,
    }
}

fn session_invalid(message: impl Into<String>) -> RpcError {
    RpcError {
        code: "LZ-SESSION-INVALID".to_owned(),
        message: message.into(),
        data: None,
    }
}

fn require_message_revision_value(value: u64) -> Result<(), RpcError> {
    if value == 0 {
        Err(message_invalid(
            "expectedRevision must be greater than zero",
        ))
    } else {
        Ok(())
    }
}

fn message_invalid(message: impl Into<String>) -> RpcError {
    RpcError {
        code: "LZ-MESSAGE-INVALID".to_owned(),
        message: message.into(),
        data: None,
    }
}

#[allow(clippy::needless_pass_by_value)]
fn message_store_error(error: lazoo_store::StoreError) -> RpcError {
    let (code, data, safe_message) = message_store_error_parts(&error);
    RpcError {
        code: code.to_owned(),
        message: safe_message,
        data,
    }
}

type MessageErrorParts = (&'static str, Option<Value>, String);

fn message_store_error_parts(error: &lazoo_store::StoreError) -> MessageErrorParts {
    match error {
        lazoo_store::StoreError::MessageNotFound { message_id } => (
            "LZ-MESSAGE-NOT-FOUND",
            Some(json!({"messageId": message_id})),
            error.to_string(),
        ),
        lazoo_store::StoreError::MessageTaskNotFound { task_id } => (
            "LZ-MESSAGE-TASK-NOT-FOUND",
            Some(json!({"taskId": task_id})),
            error.to_string(),
        ),
        lazoo_store::StoreError::MessageTaskScope { task_id } => (
            "LZ-MESSAGE-TASK-SCOPE",
            Some(json!({"taskId": task_id})),
            error.to_string(),
        ),
        lazoo_store::StoreError::MessageEmployeeNotFound { role, employee_id } => (
            "LZ-MESSAGE-EMPLOYEE-NOT-FOUND",
            Some(json!({"role": role, "employeeId": employee_id})),
            error.to_string(),
        ),
        lazoo_store::StoreError::MessageEmployeeDisabled { role, employee_id } => (
            "LZ-MESSAGE-EMPLOYEE-DISABLED",
            Some(json!({"role": role, "employeeId": employee_id})),
            error.to_string(),
        ),
        lazoo_store::StoreError::MessageSessionNotFound { role, session_id } => (
            "LZ-MESSAGE-SESSION-NOT-FOUND",
            Some(json!({"role": role, "sessionId": session_id})),
            error.to_string(),
        ),
        lazoo_store::StoreError::MessageSessionEmployeeMismatch {
            role,
            session_id,
            employee_id,
        } => (
            "LZ-MESSAGE-SESSION-EMPLOYEE-MISMATCH",
            Some(json!({
                "role": role,
                "sessionId": session_id,
                "employeeId": employee_id
            })),
            error.to_string(),
        ),
        lazoo_store::StoreError::MessageProjectNotFound { project_id } => (
            "LZ-MESSAGE-PROJECT-NOT-FOUND",
            Some(json!({"projectId": project_id})),
            error.to_string(),
        ),
        lazoo_store::StoreError::MessageCausationNotFound { message_id } => (
            "LZ-MESSAGE-CAUSATION-NOT-FOUND",
            Some(json!({"messageId": message_id})),
            error.to_string(),
        ),
        lazoo_store::StoreError::MessageCorrelation { provided, expected } => (
            "LZ-MESSAGE-CORRELATION",
            Some(json!({"provided": provided, "expected": expected})),
            error.to_string(),
        ),
        _ => message_delivery_store_error_parts(error),
    }
}

fn message_delivery_store_error_parts(error: &lazoo_store::StoreError) -> MessageErrorParts {
    match error {
        lazoo_store::StoreError::MessageStale {
            message_id,
            current_revision,
        } => (
            "LZ-MESSAGE-STALE",
            Some(json!({
                "messageId": message_id,
                "currentRevision": current_revision
            })),
            error.to_string(),
        ),
        lazoo_store::StoreError::MessageState {
            message_id,
            state,
            expected,
        } => (
            "LZ-MESSAGE-STATE",
            Some(json!({
                "messageId": message_id,
                "state": state,
                "expected": expected
            })),
            error.to_string(),
        ),
        lazoo_store::StoreError::MessageAttempt {
            message_id,
            attempt_id,
        } => (
            "LZ-MESSAGE-ATTEMPT",
            Some(json!({"messageId": message_id, "attemptId": attempt_id})),
            error.to_string(),
        ),
        lazoo_store::StoreError::MessageRetryNotDue {
            message_id,
            next_attempt_at,
        } => (
            "LZ-MESSAGE-RETRY-NOT-DUE",
            Some(json!({
                "messageId": message_id,
                "nextAttemptAt": next_attempt_at
            })),
            error.to_string(),
        ),
        lazoo_store::StoreError::MessageValidation(_) => {
            ("LZ-MESSAGE-INVALID", None, error.to_string())
        }
        _ => (
            "LZ-MESSAGE-STORE",
            None,
            "message store operation failed".to_owned(),
        ),
    }
}

fn session_failure(code: &str, message: &str, session_id: Uuid) -> RpcError {
    RpcError {
        code: code.to_owned(),
        message: message.to_owned(),
        data: Some(json!({"sessionId": session_id})),
    }
}

#[allow(clippy::needless_pass_by_value)] // map_err owns the provider error at this RPC boundary.
fn provider_error(error: ProviderError) -> RpcError {
    let message = error.to_string();
    let code = match error {
        ProviderError::UnknownProvider(_) => "LZ-PROVIDER-UNKNOWN",
        ProviderError::NotDetected(_) => "LZ-PROVIDER-NOT-DETECTED",
        ProviderError::Unsupported { .. } => "LZ-PROVIDER-UNSUPPORTED",
        ProviderError::Authentication(_) => "LZ-PROVIDER-AUTH",
        ProviderError::Timeout { .. } | ProviderError::Stalled { .. } => "LZ-PROVIDER-TIMEOUT",
        ProviderError::ProcessExited { .. } => "LZ-PROVIDER-PROCESS",
        ProviderError::Protocol(_)
        | ProviderError::InvalidEvent(_)
        | ProviderError::Observer(_)
        | ProviderError::Io(_)
        | ProviderError::Json(_)
        | ProviderError::Runtime(_) => "LZ-PROVIDER",
    };
    RpcError {
        code: code.to_owned(),
        message,
        data: None,
    }
}

fn failure(id: &str, code: &str, message: &str, data: Option<Value>) -> JsonRpcResponse {
    JsonRpcResponse::failure(
        id,
        RpcError {
            code: code.to_owned(),
            message: message.to_owned(),
            data,
        },
    )
}

#[cfg(test)]
mod tests {
    use lazoo_providers::{ProviderEvent, ProviderKind};
    use serde_json::json;

    use super::concise_events;

    #[test]
    fn concise_provider_transcripts_remove_raw_payloads_and_bound_strings() {
        let (events, truncated) = concise_events(vec![ProviderEvent::TextDelta {
            provider: ProviderKind::Codex,
            session_id: "session-1".to_owned(),
            turn_id: None,
            text: "x".repeat(70 * 1024),
            raw: Some(json!({"authorization": "secret"})),
        }]);

        assert!(truncated);
        assert_eq!(events.len(), 1);
        assert!(events[0].get("raw").is_none());
        assert_eq!(events[0]["text"].as_str().unwrap().len(), 64 * 1024);
        assert!(!events[0].to_string().contains("secret"));
    }
}
