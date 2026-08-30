use std::path::PathBuf;

use clap::{Parser, Subcommand};
use lazoo_ipc::{IpcError, PipeClient};
use lazoo_protocol::{
    AuthorityMode, CheckoutMode, EmployeeCheckoutPolicy, JsonRpcRequest, JsonRpcResponse, RpcError,
};
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
    Provider(ProviderCommand),
    #[command(subcommand)]
    Employee(EmployeeCommand),
    #[command(subcommand)]
    Roster(RosterCommand),
    #[command(subcommand)]
    Session(SessionCommand),
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

#[derive(Debug, Subcommand)]
pub enum ProviderCommand {
    List,
    Probe {
        provider: String,
    },
    Smoke {
        provider: String,
        #[arg(long)]
        prompt: String,
    },
}

#[derive(Debug, Subcommand)]
pub enum EmployeeCommand {
    Create {
        name: String,
        #[arg(long)]
        provider: String,
        #[arg(long, default_value = "")]
        role: String,
        #[arg(long, default_value = "")]
        instructions: String,
        #[arg(long)]
        model: Option<String>,
        #[arg(long)]
        reasoning_effort: Option<String>,
        #[arg(long, default_value = "{}", value_parser = parse_json_value)]
        native_settings: Value,
        #[arg(long, default_value = "ask", value_parser = parse_authority_mode)]
        authority_mode: AuthorityMode,
        #[arg(long, default_value = "isolated", value_parser = parse_employee_checkout_policy)]
        checkout_policy: EmployeeCheckoutPolicy,
        #[arg(long)]
        disabled: bool,
        #[arg(long)]
        idempotency_key: Option<String>,
    },
    List,
    Show {
        id: Uuid,
    },
    Update {
        id: Uuid,
        #[arg(long)]
        expected_revision: u64,
        #[arg(long)]
        name: Option<String>,
        #[arg(long)]
        provider: Option<String>,
        #[arg(long)]
        role: Option<String>,
        #[arg(long)]
        instructions: Option<String>,
        #[arg(long)]
        model: Option<String>,
        #[arg(long)]
        reasoning_effort: Option<String>,
        #[arg(long, value_parser = parse_json_value)]
        native_settings: Option<Value>,
        #[arg(long, value_parser = parse_authority_mode)]
        authority_mode: Option<AuthorityMode>,
        #[arg(long, value_parser = parse_employee_checkout_policy)]
        checkout_policy: Option<EmployeeCheckoutPolicy>,
        #[arg(long, action = clap::ArgAction::Set)]
        enabled: Option<bool>,
        #[arg(long)]
        idempotency_key: Option<String>,
    },
    Disable {
        id: Uuid,
        #[arg(long)]
        expected_revision: u64,
        #[arg(long)]
        idempotency_key: Option<String>,
    },
}

#[derive(Debug, Subcommand)]
pub enum RosterCommand {
    Show,
    Refresh {
        #[arg(long)]
        idempotency_key: Option<String>,
    },
}

#[derive(Debug, Subcommand)]
pub enum SessionCommand {
    Warm {
        employee_id: Uuid,
        #[arg(long)]
        expected_employee_revision: u64,
        #[arg(long)]
        idempotency_key: Option<String>,
    },
    List,
    Show {
        id: Uuid,
    },
    Stop {
        id: Uuid,
        #[arg(long)]
        expected_revision: u64,
        #[arg(long)]
        idempotency_key: Option<String>,
    },
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
        Command::Provider(ProviderCommand::List) => ("provider.list", json!({}), false),
        Command::Provider(ProviderCommand::Probe { provider }) => {
            ("provider.probe", json!({"provider": provider}), false)
        }
        Command::Provider(ProviderCommand::Smoke { provider, prompt }) => (
            "provider.smoke",
            json!({"provider": provider, "prompt": prompt}),
            false,
        ),
        Command::Employee(command) => employee_request(command),
        Command::Roster(RosterCommand::Show) => ("roster.get", json!({}), false),
        Command::Roster(RosterCommand::Refresh { idempotency_key }) => (
            "roster.refresh",
            json!({
                "idempotencyKey": idempotency_key.clone().unwrap_or_else(|| Uuid::new_v4().to_string()),
            }),
            false,
        ),
        Command::Session(command) => session_request(command),
        Command::System(SystemCommand::Shutdown) => ("system.shutdown", json!({}), false),
    }
}

fn session_request(command: &SessionCommand) -> (&'static str, Value, bool) {
    match command {
        SessionCommand::Warm {
            employee_id,
            expected_employee_revision,
            idempotency_key,
        } => (
            "session.warm",
            json!({
                "employeeId": employee_id,
                "expectedEmployeeRevision": expected_employee_revision,
                "idempotencyKey": idempotency_key.clone().unwrap_or_else(|| Uuid::new_v4().to_string()),
            }),
            false,
        ),
        SessionCommand::List => ("session.list", json!({}), false),
        SessionCommand::Show { id } => ("session.show", json!({"id": id}), false),
        SessionCommand::Stop {
            id,
            expected_revision,
            idempotency_key,
        } => (
            "session.stop",
            json!({
                "id": id,
                "expectedRevision": expected_revision,
                "idempotencyKey": idempotency_key.clone().unwrap_or_else(|| Uuid::new_v4().to_string()),
            }),
            false,
        ),
    }
}

fn employee_request(command: &EmployeeCommand) -> (&'static str, Value, bool) {
    match command {
        EmployeeCommand::Create {
            name,
            provider,
            role,
            instructions,
            model,
            reasoning_effort,
            native_settings,
            authority_mode,
            checkout_policy,
            disabled,
            idempotency_key,
        } => (
            "employee.create",
            json!({
                "name": name,
                "provider": provider,
                "role": role,
                "instructions": instructions,
                "model": model,
                "reasoningEffort": reasoning_effort,
                "nativeSettings": native_settings,
                "authorityMode": authority_mode,
                "checkoutPolicy": checkout_policy,
                "enabled": !disabled,
                "idempotencyKey": idempotency_key.clone().unwrap_or_else(|| Uuid::new_v4().to_string()),
            }),
            false,
        ),
        EmployeeCommand::List => ("employee.list", json!({}), false),
        EmployeeCommand::Show { id } => ("employee.show", json!({"id": id}), false),
        EmployeeCommand::Update {
            id,
            expected_revision,
            name,
            provider,
            role,
            instructions,
            model,
            reasoning_effort,
            native_settings,
            authority_mode,
            checkout_policy,
            enabled,
            idempotency_key,
        } => (
            "employee.update",
            json!({
                "id": id,
                "expectedRevision": expected_revision,
                "name": name,
                "provider": provider,
                "role": role,
                "instructions": instructions,
                "model": model,
                "reasoningEffort": reasoning_effort,
                "nativeSettings": native_settings,
                "authorityMode": authority_mode,
                "checkoutPolicy": checkout_policy,
                "enabled": enabled,
                "idempotencyKey": idempotency_key.clone().unwrap_or_else(|| Uuid::new_v4().to_string()),
            }),
            false,
        ),
        EmployeeCommand::Disable {
            id,
            expected_revision,
            idempotency_key,
        } => (
            "employee.disable",
            json!({
                "id": id,
                "expectedRevision": expected_revision,
                "idempotencyKey": idempotency_key.clone().unwrap_or_else(|| Uuid::new_v4().to_string()),
            }),
            false,
        ),
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
        code if code.starts_with("LZ-PROVIDER-") || code == "LZ-PROVIDER" => 8,
        code if code.starts_with("LZ-EMPLOYEE-") => 9,
        code if code.starts_with("LZ-SESSION-") => 10,
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

fn parse_json_value(value: &str) -> Result<Value, String> {
    serde_json::from_str(value).map_err(|error| format!("invalid JSON: {error}"))
}

fn parse_authority_mode(value: &str) -> Result<AuthorityMode, String> {
    match value {
        "ask" => Ok(AuthorityMode::Ask),
        "self-decision" => Ok(AuthorityMode::SelfDecision),
        "full-autonomy" => Ok(AuthorityMode::FullAutonomy),
        _ => Err("expected ask, self-decision, or full-autonomy".to_owned()),
    }
}

fn parse_employee_checkout_policy(value: &str) -> Result<EmployeeCheckoutPolicy, String> {
    match value {
        "isolated" => Ok(EmployeeCheckoutPolicy::Isolated),
        "read-only" => Ok(EmployeeCheckoutPolicy::ReadOnly),
        _ => Err("expected isolated or read-only".to_owned()),
    }
}
