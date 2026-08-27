use std::path::PathBuf;

use clap::{Parser, Subcommand};
use lazoo_ipc::{IpcError, PipeClient};
use lazoo_protocol::{CheckoutMode, JsonRpcRequest, JsonRpcResponse, RpcError};
use lazoo_runtime::{DataRoot, Installation, RuntimeError};
use serde_json::{Value, json};
use thiserror::Error;
use uuid::Uuid;

#[derive(Debug, Parser)]
#[command(name = "lazooctl", version, about = "Lazoo local control client")]
pub struct Cli {
    #[arg(long, global = true)]
    pub json: bool,
    #[command(subcommand)]
    pub command: Command,
}

#[derive(Debug, Subcommand)]
pub enum Command {
    Status,
    #[command(subcommand)]
    Project(ProjectCommand),
    #[command(subcommand)]
    Events(EventsCommand),
    #[command(subcommand)]
    System(SystemCommand),
}

#[derive(Debug, Subcommand)]
pub enum ProjectCommand {
    Register {
        path: PathBuf,
        #[arg(long)]
        name: Option<String>,
        #[arg(long, default_value = "isolated", value_parser = parse_checkout_mode)]
        checkout_mode: CheckoutMode,
        #[arg(long)]
        idempotency_key: Option<String>,
    },
    List,
}

#[derive(Debug, Subcommand)]
pub enum EventsCommand {
    List {
        #[arg(long, default_value_t = 0)]
        after: u64,
        #[arg(long, default_value_t = 100)]
        limit: u32,
    },
    Follow {
        #[arg(long, default_value_t = 0)]
        after: u64,
    },
}

#[derive(Debug, Subcommand)]
pub enum SystemCommand {
    Shutdown,
}

#[derive(Debug, Error)]
pub enum CliError {
    #[error("RPC request failed: {0:?}")]
    Rpc(RpcError),
    #[error(transparent)]
    Ipc(#[from] IpcError),
    #[error(transparent)]
    Runtime(#[from] RuntimeError),
    #[error(transparent)]
    Json(#[from] serde_json::Error),
}

impl CliError {
    #[must_use]
    pub fn exit_code(&self) -> i32 {
        match self {
            Self::Rpc(error) => exit_code_for_rpc(error),
            Self::Ipc(IpcError::ProtocolIncompatible) => 4,
            Self::Ipc(IpcError::Authentication) => 5,
            Self::Ipc(_) | Self::Runtime(_) => 3,
            Self::Json(_) => 10,
        }
    }
}

pub async fn execute(cli: &Cli) -> Result<Vec<String>, CliError> {
    let mut output = Vec::new();
    execute_with(cli, |line| output.push(line)).await?;
    Ok(output)
}

pub async fn execute_with(cli: &Cli, mut emit: impl FnMut(String)) -> Result<(), CliError> {
    let root = DataRoot::resolve()?;
    let installation = Installation::load_or_create(&root)?;
    let (method, params, follows) = command_request(&cli.command);
    let mut connection =
        PipeClient::connect(&installation, "lazooctl", env!("CARGO_PKG_VERSION")).await?;
    let request = JsonRpcRequest {
        jsonrpc: "2.0".to_owned(),
        id: Uuid::new_v4().to_string(),
        method: method.to_owned(),
        params,
    };
    connection.send_json(&request).await?;
    let response: JsonRpcResponse = connection.receive_json().await?;
    let result = response_result(response)?;
    emit(render_result(&result, cli.json)?);
    if follows {
        loop {
            let notification: Value = connection.receive_json().await?;
            let event = notification.get("params").cloned().unwrap_or(notification);
            emit(render_result(&event, cli.json)?);
        }
    }
    Ok(())
}

#[must_use]
pub fn command_request(command: &Command) -> (&'static str, Value, bool) {
    match command {
        Command::Status => ("system.status", json!({}), false),
        Command::Project(ProjectCommand::Register {
            path,
            name,
            checkout_mode,
            idempotency_key,
        }) => (
            "project.register",
            json!({
                "path": path,
                "displayName": name,
                "checkoutMode": checkout_mode,
                "idempotencyKey": idempotency_key.clone().unwrap_or_else(|| Uuid::new_v4().to_string()),
            }),
            false,
        ),
        Command::Project(ProjectCommand::List) => ("project.list", json!({}), false),
        Command::Events(EventsCommand::List { after, limit }) => (
            "events.list",
            json!({"after": after, "limit": limit}),
            false,
        ),
        Command::Events(EventsCommand::Follow { after }) => (
            "events.subscribe",
            json!({"after": after, "limit": 1024}),
            true,
        ),
        Command::System(SystemCommand::Shutdown) => ("system.shutdown", json!({}), false),
    }
}

pub fn render_result(value: &Value, json_output: bool) -> Result<String, serde_json::Error> {
    if json_output {
        serde_json::to_string(value)
    } else {
        serde_json::to_string_pretty(value)
    }
}

#[must_use]
pub fn exit_code_for_rpc(error: &RpcError) -> i32 {
    match error.code.as_str() {
        "LZ-INVALID-REQUEST" | "LZ-INVALID-PARAMS" | "LZ-PROJECT-INVALID" => 2,
        "LZ-IPC" => 3,
        "LZ-PROTOCOL-INCOMPATIBLE" => 4,
        "LZ-IPC-AUTH" => 5,
        "LZ-PROJECT-CONFLICT" => 6,
        "LZ-STORE" => 7,
        _ => 10,
    }
}

fn response_result(response: JsonRpcResponse) -> Result<Value, CliError> {
    match (response.result, response.error) {
        (Some(result), None) => Ok(result),
        (None, Some(error)) => Err(CliError::Rpc(error)),
        _ => Err(CliError::Rpc(RpcError {
            code: "LZ-INVALID-RESPONSE".to_owned(),
            message: "daemon response contained an invalid result/error combination".to_owned(),
            data: None,
        })),
    }
}

fn parse_checkout_mode(value: &str) -> Result<CheckoutMode, String> {
    match value {
        "isolated" => Ok(CheckoutMode::Isolated),
        "lead-main" => Ok(CheckoutMode::LeadMain),
        _ => Err("expected isolated or lead-main".to_owned()),
    }
}
