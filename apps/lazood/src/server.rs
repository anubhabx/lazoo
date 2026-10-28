use lazoo_ipc::{AuthenticatedConnection, PipeServer};
use lazoo_protocol::{EventsListParams, JsonRpcRequest, JsonRpcResponse, RpcError};
use lazoo_runtime::{DataRoot, Installation, OwnerSecurity, SingleInstance};
use lazoo_store::Store;
use serde_json::json;
use tokio::{
    task::JoinSet,
    time::{Duration, timeout},
};

use crate::{App, DaemonError};

const MAX_CLIENTS: usize = 64;
const REQUEST_TIMEOUT: Duration = Duration::from_secs(5);

pub async fn run() -> Result<(), DaemonError> {
    let root = DataRoot::resolve()?;
    root.ensure()?;
    let installation = Installation::load_or_create(&root)?;
    let _instance = SingleInstance::acquire(installation.id(), "daemon")?;
    let store = Store::open_path(root.state_db())?;
    store.import_pending_incidents(&root)?;
    let governor =
        lazoo_runtime::InfrastructureGovernor::attach_current(installation.id().to_string())
            .map(|(governor, _, _)| governor)
            .map_err(|error| error.to_string());
    if let Err(error) = &governor {
        tracing::warn!(%error, "infrastructure CPU cap unavailable");
    }
    let app = App::new(store, root)?.with_resource_governor(governor);
    // Establish every fallible listener before starting tasks that retain App/process handles.
    let owner = OwnerSecurity::current()?;
    let mut server = PipeServer::bind(&installation, &owner)?;
    let terminal_server = PipeServer::bind_terminal(&installation, &owner)?;
    let resource_app = app.clone();
    let resource_sampler = tokio::spawn(async move { resource_app.run_resource_sampler().await });
    let archive_app = app.clone();
    let memory_archiver = tokio::spawn(async move { archive_app.run_memory_archiver().await });
    let summary_app = app.clone();
    let memory_worker = tokio::spawn(async move { summary_app.run_memory_worker().await });
    let dispatcher_app = app.clone();
    let dispatcher = tokio::spawn(async move { dispatcher_app.run_task_dispatcher().await });
    let notices_app = app.clone();
    let notices = tokio::spawn(async move { notices_app.run_notice_delivery().await });
    let idle_app = app.clone();
    let idle = tokio::spawn(async move { idle_app.run_timed_sweeps().await });
    let terminal_app = app.clone();
    let mut terminal_service =
        tokio::spawn(
            async move { crate::terminal_server::run(terminal_app, terminal_server).await },
        );
    let mut shutdown = app.shutdown_receiver();
    let mut clients = JoinSet::new();
    let mut terminal_finished = false;
    let result = loop {
        tokio::select! {
            accepted = server.accept_pending() => {
                let pending = match accepted {
                    Ok(pending) => pending,
                    Err(error) => break Err(DaemonError::Ipc(error)),
                };
                // Do not leave an already-connected overflow peer waiting for a hello.
                if clients.len() >= MAX_CLIENTS {
                    drop(pending);
                    continue;
                }
                let client_app = app.clone();
                clients.spawn(async move {
                    if let Ok(connection) = pending.authenticate().await {
                        let _ = serve_connection(client_app, connection).await;
                    }
                });
            }
            _ = clients.join_next(), if !clients.is_empty() => {}
            result = &mut terminal_service => {
                terminal_finished = true;
                break match result {
                    Ok(Ok(())) if *shutdown.borrow() => Ok(()),
                    Ok(Err(error)) => Err(error),
                    other => Err(DaemonError::Ipc(lazoo_ipc::IpcError::Io(std::io::Error::other(format!("terminal listener exited unexpectedly: {other:?}"))))),
                };
            }
            changed = shutdown.changed() => {
                if changed.is_err() || *shutdown.borrow() { break Ok(()); }
            }
        }
    };
    let _ = timeout(Duration::from_secs(2), async {
        while clients.join_next().await.is_some() {}
    })
    .await;
    clients.abort_all();
    while clients.join_next().await.is_some() {}
    if !terminal_finished {
        terminal_service.abort();
        let _ = terminal_service.await;
    }
    app.shutdown_terminals().await;
    resource_sampler.abort();
    let _ = resource_sampler.await;
    memory_archiver.abort();
    let _ = memory_archiver.await;
    memory_worker.abort();
    let _ = memory_worker.await;
    // The sweep must not race shutdown for the same session; an interrupted stop is recorded
    // by shutdown_live_sessions like any other interrupted transition.
    idle.abort();
    let _ = idle.await;
    app.shutdown_live_sessions().await;
    dispatcher.abort();
    let _ = dispatcher.await;
    notices.abort();
    let _ = notices.await;
    result
}

async fn serve_connection(
    app: App,
    mut connection: AuthenticatedConnection,
) -> Result<(), DaemonError> {
    let request: JsonRpcRequest = timeout(REQUEST_TIMEOUT, connection.receive_json())
        .await
        .map_err(|_| {
            lazoo_ipc::IpcError::Io(std::io::Error::new(
                std::io::ErrorKind::TimedOut,
                "client request timed out",
            ))
        })??;
    if request.method != "events.subscribe" {
        send_response(&mut connection, &app.handle(request).await).await?;
        return Ok(());
    }
    let params: EventsListParams = match serde_json::from_value(request.params) {
        Ok(params) => params,
        Err(error) => {
            send_response(
                &mut connection,
                &JsonRpcResponse::failure(
                    request.id,
                    RpcError {
                        code: "LZ-INVALID-PARAMS".to_owned(),
                        message: error.to_string(),
                        data: None,
                    },
                ),
            )
            .await?;
            return Ok(());
        }
    };
    let mut subscription = app.subscription(params.after)?;
    send_response(
        &mut connection,
        &JsonRpcResponse::success(request.id, json!({"subscribed": true})),
    )
    .await?;
    let mut watermark = params.after;
    for event in subscription.catch_up {
        watermark = watermark.max(event.sequence);
        send_response(
            &mut connection,
            &json!({"jsonrpc":"2.0", "method":"events.event", "params":event}),
        )
        .await?;
    }
    loop {
        let event =
            subscription.live.recv().await.map_err(|error| {
                lazoo_ipc::IpcError::Io(std::io::Error::other(error.to_string()))
            })?;
        if event.sequence > watermark {
            watermark = event.sequence;
            send_response(
                &mut connection,
                &json!({"jsonrpc":"2.0", "method":"events.event", "params":event}),
            )
            .await?;
        }
    }
}

async fn send_response<T: serde::Serialize>(
    connection: &mut AuthenticatedConnection,
    value: &T,
) -> Result<(), DaemonError> {
    timeout(REQUEST_TIMEOUT, connection.send_json(value))
        .await
        .map_err(|_| {
            lazoo_ipc::IpcError::Io(std::io::Error::new(
                std::io::ErrorKind::TimedOut,
                "client response timed out",
            ))
        })??;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use lazoo_ipc::PipeClient;

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn response_to_a_nonreading_client_has_a_deadline() {
        let temporary = tempfile::tempdir().unwrap();
        let installation =
            Installation::load_or_create(&DataRoot::from_path(temporary.path())).unwrap();
        let mut server =
            PipeServer::bind(&installation, &OwnerSecurity::current().unwrap()).unwrap();
        let (server_side, client_side) = tokio::join!(
            server.accept(),
            PipeClient::connect(&installation, "non-reader", "0.1.0")
        );
        let mut connection = server_side.unwrap();
        let _client = client_side.unwrap();
        let payload = json!({"data": "x".repeat(512 * 1024)});
        let error = timeout(Duration::from_secs(10), async {
            // Windows may accept one whole frame into its pipe buffers even when the
            // receiver is idle. Fill that bounded OS buffer before asserting backpressure.
            for _ in 0..32 {
                send_response(&mut connection, &payload).await?;
            }
            Ok::<(), DaemonError>(())
        })
        .await
        .expect("stalled writes must be bounded")
        .unwrap_err();
        assert!(
            error.to_string().contains("client response timed out"),
            "{error}"
        );
    }
}
