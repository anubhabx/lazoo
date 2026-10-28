use std::{path::PathBuf, time::Duration};

use clap::Subcommand;
use lazoo_ipc::PipeClient;
use lazoo_protocol::{TerminalIoRequest, TerminalIoResponse};
use lazoo_runtime::Installation;
use serde_json::{Value, json};
use tokio::time::timeout;
use uuid::Uuid;

/// Terminal attach byte I/O is intentionally handled by the authenticated terminal pipe,
/// not by these JSON-RPC control-plane commands.
#[derive(Debug, Subcommand)]
pub enum TerminalCommand {
    Open {
        executable: PathBuf,
        #[arg(long = "arg")]
        args: Vec<String>,
        #[arg(long)]
        cwd: PathBuf,
        #[arg(long, default_value_t = 80)]
        columns: u16,
        #[arg(long, default_value_t = 25)]
        rows: u16,
        #[arg(long)]
        idempotency_key: Option<String>,
    },
    List,
    Show {
        id: Uuid,
    },
    Resize {
        id: Uuid,
        #[arg(long)]
        columns: u16,
        #[arg(long)]
        rows: u16,
        #[arg(long)]
        expected_revision: u64,
        #[arg(long)]
        expected_boot_id: Uuid,
    },
    Stop {
        id: Uuid,
        #[arg(long)]
        expected_revision: u64,
        #[arg(long)]
        expected_boot_id: Uuid,
    },
    Read {
        id: Uuid,
        #[arg(long)]
        expected_boot_id: Uuid,
        #[arg(long, default_value_t = 0)]
        after: u64,
        #[arg(long, default_value_t = 65_536)]
        limit: usize,
    },
    Write {
        id: Uuid,
        #[arg(long)]
        expected_boot_id: Uuid,
        #[arg(long)]
        text: String,
    },
}

#[must_use]
pub fn command_request(command: &TerminalCommand) -> (&'static str, Value, bool) {
    match command {
        TerminalCommand::Open {
            executable,
            args,
            cwd,
            columns,
            rows,
            idempotency_key,
        } => (
            "terminal.open",
            json!({"executable": executable, "args": args, "cwd": cwd, "columns": columns, "rows": rows, "idempotencyKey": idempotency_key.clone().unwrap_or_else(|| Uuid::new_v4().to_string())}),
            false,
        ),
        TerminalCommand::List => ("terminal.list", json!({}), false),
        TerminalCommand::Show { id } => ("terminal.show", json!({"id": id}), false),
        TerminalCommand::Resize {
            id,
            columns,
            rows,
            expected_revision,
            expected_boot_id,
        } => (
            "terminal.resize",
            json!({"id": id, "columns": columns, "rows": rows, "expectedRevision": expected_revision, "expectedBootId": expected_boot_id}),
            false,
        ),
        TerminalCommand::Stop {
            id,
            expected_revision,
            expected_boot_id,
        } => (
            "terminal.stop",
            json!({"id": id, "expectedRevision": expected_revision, "expectedBootId": expected_boot_id}),
            false,
        ),
        TerminalCommand::Read { .. } | TerminalCommand::Write { .. } => {
            ("terminal.io", json!({}), false)
        }
    }
}

pub async fn execute_io(
    command: &TerminalCommand,
    installation: &Installation,
) -> Result<Value, String> {
    timeout(
        Duration::from_secs(8),
        execute_io_inner(command, installation),
    )
    .await
    .map_err(|_| "terminal I/O timed out".to_owned())?
}

async fn execute_io_inner(
    command: &TerminalCommand,
    installation: &Installation,
) -> Result<Value, String> {
    let (request, input) = match command {
        TerminalCommand::Read {
            id,
            expected_boot_id,
            after,
            limit,
        } => (
            TerminalIoRequest::Read {
                id: *id,
                expected_boot_id: *expected_boot_id,
                after: *after,
                limit: *limit,
            },
            None,
        ),
        TerminalCommand::Write {
            id,
            expected_boot_id,
            text,
        } => {
            if text.len() > 16 * 1024 {
                return Err("terminal input exceeds 16KiB".into());
            }
            (
                TerminalIoRequest::Write {
                    id: *id,
                    expected_boot_id: *expected_boot_id,
                },
                Some(text.as_bytes()),
            )
        }
        _ => return Err("not a terminal I/O command".into()),
    };
    let mut connection =
        PipeClient::connect_terminal(installation, "lazooctl", env!("CARGO_PKG_VERSION"))
            .await
            .map_err(|error| error.to_string())?;
    connection
        .send_json(&request)
        .await
        .map_err(|error| error.to_string())?;
    if let Some(input) = input {
        connection
            .send_bytes(input)
            .await
            .map_err(|error| error.to_string())?;
    }
    let response: TerminalIoResponse = connection
        .receive_json()
        .await
        .map_err(|error| error.to_string())?;
    if let Some(error) = response.error {
        return Err(error);
    }
    let bytes = if matches!(command, TerminalCommand::Read { .. }) {
        let output = response
            .output
            .as_ref()
            .ok_or_else(|| "terminal read response omitted output metadata".to_owned())?;
        if output.end < output.start || output.end - output.start != output.length as u64 {
            return Err("terminal read response has invalid output offsets".into());
        }
        let bytes = connection
            .receive_bytes(64 * 1024)
            .await
            .map_err(|error| error.to_string())?;
        if bytes.len() as u64 != output.length as u64 {
            return Err("terminal read response length does not match output metadata".into());
        }
        bytes
    } else {
        Vec::new()
    };
    Ok(json!({"output": response.output, "bytes": bytes}))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn io_commands_map_without_control_plane_parameters() {
        let id = Uuid::nil();
        let boot = Uuid::from_u128(1);
        for command in [
            TerminalCommand::Read {
                id,
                expected_boot_id: boot,
                after: 4,
                limit: 16,
            },
            TerminalCommand::Write {
                id,
                expected_boot_id: boot,
                text: "bytes".into(),
            },
        ] {
            let (method, parameters, stream) = command_request(&command);
            assert_eq!(method, "terminal.io");
            assert_eq!(parameters, json!({}));
            assert!(!stream);
        }
    }
}
