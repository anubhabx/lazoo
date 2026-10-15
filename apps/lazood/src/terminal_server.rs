//! Authenticated, bounded terminal byte I/O isolated from the control RPC listener.
use crate::{App, DaemonError};
use lazoo_ipc::{AuthenticatedConnection, IpcError, PipeServer};
use lazoo_protocol::{
    TERMINAL_INPUT_MAX_BYTES, TERMINAL_OUTPUT_MAX_BYTES, TerminalInput, TerminalIoRequest,
    TerminalIoResponse, TerminalOutputMetadata, TerminalOutputRequest,
};
use tokio::{
    task::JoinSet,
    time::{Duration, timeout},
};

const DEADLINE: Duration = Duration::from_secs(5);
const MAX_CLIENTS: usize = 16;

pub(crate) async fn run(app: App, mut server: PipeServer) -> Result<(), DaemonError> {
    let mut shutdown = app.shutdown_receiver();
    let mut clients = JoinSet::new();
    loop {
        tokio::select! {
            result = server.accept_pending() => {
                let pending = result?;
                if clients.len() >= MAX_CLIENTS { drop(pending); continue; }
                let app = app.clone();
                clients.spawn(async move {
                    if let Ok(connection) = pending.authenticate().await {
                        // Cancellation closes a partially consumed frame rather than reusing it.
                        let _ = timeout(DEADLINE, serve(app, connection)).await;
                    }
                });
            }
            _ = clients.join_next(), if !clients.is_empty() => {}
            changed = shutdown.changed() => { if changed.is_err() || *shutdown.borrow() { break; } }
        }
    }
    clients.abort_all();
    while clients.join_next().await.is_some() {}
    Ok(())
}

async fn serve(app: App, mut connection: AuthenticatedConnection) -> Result<(), IpcError> {
    let header = connection.receive_bytes(4096).await?;
    let request: TerminalIoRequest = serde_json::from_slice(&header)?;
    let (result, bytes) = match request {
        TerminalIoRequest::Read {
            id,
            expected_boot_id,
            after,
            limit,
        } => {
            if limit == 0 || limit > TERMINAL_OUTPUT_MAX_BYTES {
                (
                    Err("terminal output limit must be 1 through 65536".to_owned()),
                    None,
                )
            } else {
                match app
                    .terminal_output(TerminalOutputRequest {
                        id,
                        expected_boot_id,
                        after,
                        limit,
                    })
                    .await
                {
                    Ok(output) => (
                        Ok(Some(TerminalOutputMetadata {
                            start: output.start,
                            end: output.end,
                            length: output.bytes.len(),
                            gap: output.gap,
                            live: output.live,
                            exit_code: output.exit_code,
                        })),
                        Some(output.bytes),
                    ),
                    Err(error) => (Err(error), None),
                }
            }
        }
        TerminalIoRequest::Write {
            id,
            expected_boot_id,
        } => {
            let bytes = connection.receive_bytes(TERMINAL_INPUT_MAX_BYTES).await?;
            (
                app.terminal_input(TerminalInput {
                    id,
                    expected_boot_id,
                    bytes,
                })
                .await
                .map(|()| None),
                None,
            )
        }
    };
    let response = match result {
        Ok(output) => TerminalIoResponse {
            error: None,
            output,
        },
        Err(error) => TerminalIoResponse {
            error: Some(error),
            output: None,
        },
    };
    connection.send_json(&response).await?;
    if let Some(bytes) = bytes {
        connection.send_bytes(&bytes).await?;
    }
    Ok(())
}
