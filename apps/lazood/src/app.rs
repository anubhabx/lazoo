use std::{fs, str::FromStr, time::Instant};

use chrono::{SecondsFormat, Utc};
use lazoo_domain::inspect_git_repository;
use lazoo_protocol::{
    DesktopSnapshot, EventSummary, EventsListParams, JsonRpcRequest, JsonRpcResponse, PROTOCOL_V1,
    ProviderListEntry, ProviderProbeParams, ProviderSmokeParams, ProviderSmokeResult,
    RegisterProjectParams, RpcError, SystemStatus,
};
use lazoo_providers::{
    DiscoveryEnvironment, DiscoverySource, ProviderError, ProviderKind, ProviderRegistry,
    ProviderSession, SessionRequest, TurnRequest,
};
use lazoo_runtime::DataRoot;
use lazoo_store::{RegisterProjectInput, Store, StoreStatus};
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
}

pub struct EventSubscription {
    pub catch_up: Vec<EventSummary>,
    pub live: broadcast::Receiver<EventSummary>,
}

impl App {
    #[must_use]
    pub fn new(store: Store, root: DataRoot) -> Self {
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
            .discover(&DiscoveryEnvironment::current())
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
            .probe(provider, &DiscoveryEnvironment::current())
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
        _ => "LZ-STORE",
    };
    RpcError {
        code: code.to_owned(),
        message: error.to_string(),
        data: None,
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
