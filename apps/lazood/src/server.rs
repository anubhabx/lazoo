use lazoo_ipc::{AuthenticatedConnection, PipeServer};
use lazoo_protocol::{EventsListParams, JsonRpcRequest, JsonRpcResponse, RpcError};
use lazoo_runtime::{DataRoot, Installation, OwnerSecurity, SingleInstance};
use lazoo_store::Store;
use serde_json::json;

use crate::{App, DaemonError};

pub async fn run() -> Result<(), DaemonError> {
    let root = DataRoot::resolve()?;
    root.ensure()?;
    let installation = Installation::load_or_create(&root)?;
    let _instance = SingleInstance::acquire(installation.id(), "daemon")?;
    let store = Store::open_path(root.state_db())?;
    store.import_pending_incidents(&root)?;
    let app = App::new(store, root);
    let owner = OwnerSecurity::current()?;
    let mut server = PipeServer::bind(&installation, &owner)?;
    let mut shutdown = app.shutdown_receiver();
    loop {
        tokio::select! {
            accepted = server.accept() => {
                let connection = accepted?;
                let client_app = app.clone();
                tokio::spawn(async move { let _ = serve_connection(client_app, connection).await; });
            }
            changed = shutdown.changed() => {
                if changed.is_err() || *shutdown.borrow() { break; }
            }
        }
    }
    Ok(())
}

async fn serve_connection(
    app: App,
    mut connection: AuthenticatedConnection,
) -> Result<(), DaemonError> {
    let request: JsonRpcRequest = connection.receive_json().await?;
    if request.method != "events.subscribe" {
        connection.send_json(&app.handle(request).await).await?;
        return Ok(());
    }
    let params: EventsListParams = match serde_json::from_value(request.params) {
        Ok(params) => params,
        Err(error) => {
            connection
                .send_json(&JsonRpcResponse::failure(
                    request.id,
                    RpcError {
                        code: "LZ-INVALID-PARAMS".to_owned(),
                        message: error.to_string(),
                        data: None,
                    },
                ))
                .await?;
            return Ok(());
        }
    };
    let mut subscription = app.subscription(params.after)?;
    connection
        .send_json(&JsonRpcResponse::success(
            request.id,
            json!({"subscribed": true}),
        ))
        .await?;
    let mut watermark = params.after;
    for event in subscription.catch_up {
        watermark = watermark.max(event.sequence);
        connection
            .send_json(&json!({"jsonrpc":"2.0", "method":"events.event", "params":event}))
            .await?;
    }
    loop {
        let event =
            subscription.live.recv().await.map_err(|error| {
                lazoo_ipc::IpcError::Io(std::io::Error::other(error.to_string()))
            })?;
        if event.sequence > watermark {
            watermark = event.sequence;
            connection
                .send_json(&json!({"jsonrpc":"2.0", "method":"events.event", "params":event}))
                .await?;
        }
    }
}
