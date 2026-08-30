use std::{collections::BTreeSet, fs, str::FromStr, time::Instant};

use chrono::{SecondsFormat, Utc};
use lazoo_domain::{
    inspect_git_repository, validate_employee_name, validate_employee_text_fields,
    validate_native_settings,
};
use lazoo_protocol::{
    CreateEmployeeParams, DesktopSnapshot, DisableEmployeeParams, EmployeeShowParams, EventSummary,
    EventsListParams, JsonRpcRequest, JsonRpcResponse, PROTOCOL_V1, ProviderListEntry,
    ProviderObservation, ProviderProbeParams, ProviderSmokeParams, ProviderSmokeResult,
    RegisterProjectParams, RosterRefreshParams, RpcError, SystemStatus, UpdateEmployeeParams,
};
use lazoo_providers::{
    AuthenticationState, DiscoveryEnvironment, DiscoverySource, ProviderCapabilities,
    ProviderError, ProviderKind, ProviderProbe, ProviderRegistry, ProviderSession,
    ProviderVerification, SessionRequest, TurnRequest,
};
use lazoo_runtime::DataRoot;
use lazoo_store::{
    CreateEmployeeInput, DisableEmployeeInput, RefreshProviderObservationsInput,
    RegisterProjectInput, Store, StoreStatus, UpdateEmployeeInput,
};
use serde::Serialize;
use serde_json::{Value, json};
use tokio::sync::{broadcast, watch};
use uuid::Uuid;

#[derive(Clone, Debug)]
pub struct App {
    store: Store,
    root: DataRoot,
    boot_id: Uuid,
    started: Instant,
    events: broadcast::Sender<EventSummary>,
    shutdown: watch::Sender<bool>,
    providers: ProviderRegistry,
    provider_environment: DiscoveryEnvironment,
}

pub struct EventSubscription {
    pub catch_up: Vec<EventSummary>,
    pub live: broadcast::Receiver<EventSummary>,
}

impl App {
    #[must_use]
    pub fn new(store: Store, root: DataRoot) -> Self {
        Self::new_with_provider_environment(store, root, DiscoveryEnvironment::current())
    }

    #[doc(hidden)]
    #[must_use]
    pub fn new_with_provider_environment(
        store: Store,
        root: DataRoot,
        provider_environment: DiscoveryEnvironment,
    ) -> Self {
        let (events, _) = broadcast::channel(1024);
        let (shutdown, _) = watch::channel(false);
        Self {
            store,
            root,
            boot_id: Uuid::new_v4(),
            started: Instant::now(),
            events,
            shutdown,
            providers: ProviderRegistry::default(),
            provider_environment,
        }
    }

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
            "project.register" => self.register(&request),
            "project.list" => self
                .store
                .list_projects()
                .map(to_value)
                .map_err(store_error),
            "events.list" => self.list_events(request.params),
            "desktop.snapshot" => self.desktop_snapshot(&request.params),
            "provider.list" => self.provider_list(&request.params),
            "provider.probe" => self.provider_probe(request.params).await,
            "provider.smoke" => self.provider_smoke(request.params).await,
            "employee.create" => self.create_employee(&request).await,
            "employee.list" => self.employee_list(&request.params),
            "employee.show" => self.employee_show(request.params),
            "employee.update" => self.update_employee(&request).await,
            "employee.disable" => self.disable_employee(&request),
            "roster.get" => self.roster_get(&request.params),
            "roster.refresh" => self.refresh_roster(&request).await,
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

    pub fn subscribe(
        &self,
        after: u64,
    ) -> Result<(Vec<EventSummary>, broadcast::Receiver<EventSummary>), lazoo_store::StoreError>
    {
        let live = self.events.subscribe();
        let catch_up = self.store.list_events(after, 1024)?;
        Ok((catch_up, live))
    }

    pub fn subscription(&self, after: u64) -> Result<EventSubscription, lazoo_store::StoreError> {
        let (catch_up, live) = self.subscribe(after)?;
        Ok(EventSubscription { catch_up, live })
    }

    #[must_use]
    pub fn shutdown_receiver(&self) -> watch::Receiver<bool> {
        self.shutdown.subscribe()
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
        for event in self.store.publish_pending_outbox().map_err(store_error)? {
            let _ = self.events.send(event);
        }
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
            .providers
            .start(provider, &request)
            .await
            .map_err(provider_error)?;
        let session_id = session.id();
        let turn = session.turn(&TurnRequest::new(params.prompt)).await;
        let stop = session.stop().await;
        let events = turn.map_err(provider_error)?;
        stop.map_err(provider_error)?;
        let (events, transcript_truncated) = concise_events(events);
        Ok(to_value(ProviderSmokeResult {
            provider: provider.to_string(),
            session_id,
            events,
            transcript_truncated,
            checked_at: Utc::now().to_rfc3339_opts(SecondsFormat::Millis, true),
        }))
    }

    async fn create_employee(&self, request: &JsonRpcRequest) -> Result<Value, RpcError> {
        let params: CreateEmployeeParams = serde_json::from_value(request.params.clone())
            .map_err(|error| invalid_params(error.to_string()))?;
        require_idempotency_key(&params.idempotency_key)?;
        let name = validate_employee_name(&params.name).map_err(employee_validation_error)?;
        validate_employee_text_fields(
            &params.role,
            &params.instructions,
            params.model.as_deref(),
            params.reasoning_effort.as_deref(),
        )
        .map_err(employee_validation_error)?;
        validate_native_settings(&params.native_settings).map_err(employee_validation_error)?;
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
            .map_err(employee_validation_error)?;
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
        .map_err(employee_validation_error)?;
        if let Some(native_settings) = &params.native_settings {
            validate_native_settings(native_settings).map_err(employee_validation_error)?;
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

fn employee_validation_error(error: lazoo_domain::EmployeeValidationError) -> RpcError {
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
        | ProviderError::Io(_)
        | ProviderError::Json(_) => (
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
        _ => "LZ-STORE",
    };
    let data = match &error {
        lazoo_store::StoreError::EmployeeStale {
            current_revision, ..
        } => Some(json!({"currentRevision": current_revision})),
        _ => None,
    };
    RpcError {
        code: code.to_owned(),
        message: error.to_string(),
        data,
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
        | ProviderError::Io(_)
        | ProviderError::Json(_) => "LZ-PROVIDER",
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
