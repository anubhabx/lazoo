use std::{fs, time::Instant};

use chrono::{SecondsFormat, Utc};
use lazoo_domain::inspect_git_repository;
use lazoo_protocol::{
    DesktopSnapshot, EventSummary, EventsListParams, JsonRpcRequest, JsonRpcResponse, PROTOCOL_V1,
    RegisterProjectParams, RpcError, SystemStatus,
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
        }
    }

    // Keep the request boundary async so transports can await it without an API change.
    #[allow(clippy::unused_async, clippy::unused_async_trait_impl)]
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
        require_empty_params(params)?;
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
}

fn require_empty_params(params: &Value) -> Result<(), RpcError> {
    if params.as_object().is_some_and(serde_json::Map::is_empty) {
        Ok(())
    } else {
        Err(invalid_params(
            "desktop.snapshot accepts only an empty object",
        ))
    }
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
