mod configuration;
mod memory;
mod memory_worker;
mod terminal;
pub use memory::MemoryCommand;
pub use memory_worker::MemoryWorkerCommand;
pub use terminal::TerminalCommand;

use std::path::PathBuf;

use clap::{Parser, Subcommand};
use lazoo_ipc::{IpcError, PipeClient};
use lazoo_protocol::{
    ActivationPolicy, ApprovalDecision, ApprovalState, AuthorityMode, CheckoutMode,
    EmployeeCheckoutPolicy, InboxFilter, IncidentEscalation, IncidentPriority, IncidentState,
    JsonRpcRequest, JsonRpcResponse, MessageEndpoint, MessageKind, MessagePriority, MessageSender,
    RpcError, TaskReportKind, TaskResolution, TaskState, UserMessageEndpoint, UserMessageKind,
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
    #[command(subcommand)]
    MemoryWorker(MemoryWorkerCommand),
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
    Message(MessageCommand),
    #[command(subcommand)]
    Conversation(ConversationCommand),
    #[command(subcommand)]
    Turn(TurnCommand),
    #[command(subcommand)]
    Task(TaskCommand),
    #[command(subcommand)]
    Approval(ApprovalCommand),
    #[command(subcommand)]
    Incident(IncidentCommand),
    #[command(subcommand)]
    Inbox(InboxCommand),
    #[command(subcommand)]
    Workspace(WorkspaceCommand),
    #[command(subcommand)]
    System(SystemCommand),
    #[command(subcommand)]
    Terminal(TerminalCommand),
    #[command(subcommand)]
    Memory(MemoryCommand),
}

#[derive(Debug, Subcommand)]
pub enum IncidentCommand {
    List {
        #[arg(long, default_value_t = 0)]
        after_sequence: u64,
        #[arg(long, default_value_t = 100)]
        limit: u32,
        #[arg(long, value_parser = parse_incident_state)]
        state: Option<IncidentState>,
        #[arg(long, value_parser = parse_incident_priority)]
        priority: Option<IncidentPriority>,
        #[arg(long)]
        code: Option<String>,
        #[arg(long)]
        employee_id: Option<Uuid>,
        #[arg(long)]
        project_id: Option<Uuid>,
        /// `user` lists the priority notifications: incidents no lead remedy settled.
        #[arg(long, value_parser = parse_incident_escalation)]
        escalation: Option<IncidentEscalation>,
    },
    Show {
        id: Uuid,
    },
    Acknowledge {
        id: Uuid,
        #[arg(long)]
        expected_revision: u64,
        #[arg(long)]
        idempotency_key: Option<String>,
    },
    Resolve {
        id: Uuid,
        #[arg(long)]
        expected_revision: u64,
        #[arg(long)]
        idempotency_key: Option<String>,
    },
    /// The persistent-failure thresholds that promote an incident, and your overrides of them.
    #[command(subcommand)]
    Threshold(IncidentThresholdCommand),
}

#[derive(Debug, Subcommand)]
pub enum IncidentThresholdCommand {
    List,
    /// Promote CODE's incident once OCCURRENCES equivalent failures land inside the window.
    Set {
        code: String,
        #[arg(long)]
        occurrences: u64,
        #[arg(long, default_value_t = 300)]
        window_seconds: u64,
    },
    /// Return CODE to its default threshold.
    Reset {
        code: String,
    },
}

/// What needs you, derived from the approval, task, and incident ledgers for the desktop
/// Inbox; nothing here changes anything.
#[derive(Debug, Subcommand)]
pub enum InboxCommand {
    List {
        /// all, approvals, permissions, incidents, or resolved (the archive).
        #[arg(long, default_value = "all", value_parser = parse_inbox_filter)]
        filter: InboxFilter,
        #[arg(long, default_value_t = 200)]
        limit: u32,
        /// The `nextBefore` of the previous resolved page.
        #[arg(long)]
        before: Option<String>,
    },
    /// The rail badge: how many items need you right now.
    Counts,
}

/// The desktop workspace read models: a space's conversation history with its markers, and
/// who can lead a new conversation right now. Nothing here changes anything.
#[derive(Debug, Subcommand)]
pub enum WorkspaceCommand {
    Conversations {
        /// A registered project; omit for Home (projectless conversations).
        #[arg(long)]
        project_id: Option<Uuid>,
        #[arg(long, default_value_t = 200)]
        limit: u32,
        /// The `nextBefore` of the previous page.
        #[arg(long)]
        before: Option<String>,
    },
    Launcher {
        #[arg(long)]
        project_id: Option<Uuid>,
    },
    /// The active conversation: its lead header, worker dock, attention items, and newest turns.
    Show {
        id: Uuid,
        /// How many of the newest turns to include (1 to 500).
        #[arg(long, default_value_t = 50)]
        turn_limit: u32,
    },
}

#[derive(Debug, Subcommand)]
pub enum ApprovalCommand {
    List {
        #[arg(long, default_value_t = 0)]
        after_sequence: u64,
        #[arg(long, default_value_t = 100)]
        limit: u32,
        #[arg(long, value_parser = parse_approval_state)]
        state: Option<ApprovalState>,
        #[arg(long)]
        turn_id: Option<Uuid>,
        #[arg(long)]
        task_id: Option<Uuid>,
        #[arg(long)]
        conversation_id: Option<Uuid>,
        #[arg(long)]
        employee_id: Option<Uuid>,
    },
    Show {
        id: Uuid,
    },
    Approve {
        id: Uuid,
        #[arg(long)]
        expected_revision: u64,
        #[arg(long)]
        idempotency_key: Option<String>,
    },
    Decline {
        id: Uuid,
        #[arg(long)]
        expected_revision: u64,
        #[arg(long)]
        idempotency_key: Option<String>,
    },
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
    /// Infrastructure CPU cap and rolling aggregate measurements.
    Budget,
    Shutdown,
    /// The daemon's own CPU time, working set, and handle count.
    Resources,
}

#[derive(Debug, Subcommand)]
pub enum ProviderCommand {
    /// Preview one explicitly selected MCP entry without changing provider files.
    ConfigPreview {
        #[arg(long, value_parser = parse_json_value)]
        selection: Value,
    },
    ConfigApply {
        operation_id: Uuid,
        #[arg(long)]
        expected_revision: u64,
    },
    ConfigRollback {
        operation_id: Uuid,
        #[arg(long)]
        expected_revision: u64,
    },
    ConfigShow {
        operation_id: Uuid,
    },
    List,
    /// Read every known provider configuration file, with secrets redacted, and name
    /// duplicate and conflicting integrations across providers. Writes nothing.
    Config,
    /// The migration preview over that discovery: per integration, whether Lazoo would manage
    /// it as is, merge an exact duplicate across providers, or need your choice for a
    /// same-name conflict. Selects and writes nothing.
    Preview,
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
        #[arg(long, default_value = "manual", value_parser = parse_employee_activation_policy)]
        activation_policy: ActivationPolicy,
        /// Suspend an idle session after this many quiet seconds (60-86400); 0 never does.
        #[arg(long, default_value_t = 0)]
        idle_timeout_seconds: u32,
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
        #[arg(long, value_parser = parse_employee_activation_policy)]
        activation_policy: Option<ActivationPolicy>,
        /// Suspend an idle session after this many quiet seconds (60-86400); 0 never does.
        #[arg(long)]
        idle_timeout_seconds: Option<u32>,
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
    /// Process count, CPU time, and peak memory of a live session's native process tree.
    Resources {
        id: Uuid,
    },
    Stop {
        id: Uuid,
        #[arg(long)]
        expected_revision: u64,
        #[arg(long)]
        idempotency_key: Option<String>,
    },
    /// Reopen a suspended Claude Code or Codex session's native harness in an owner terminal.
    /// Use `terminal read`, `terminal write`, and `terminal stop` on the returned terminal.
    Attach {
        id: Uuid,
        #[arg(long)]
        expected_revision: u64,
        #[arg(long, default_value_t = 80)]
        columns: u16,
        #[arg(long, default_value_t = 25)]
        rows: u16,
        #[arg(long)]
        idempotency_key: Option<String>,
    },
}

#[derive(Debug, Subcommand)]
pub enum MessageCommand {
    Send {
        sender_employee_id: Uuid,
        recipient_employee_id: Uuid,
        #[arg(long)]
        sender_session_id: Option<Uuid>,
        #[arg(long)]
        recipient_session_id: Option<Uuid>,
        #[arg(long)]
        project_id: Option<Uuid>,
        #[arg(long)]
        conversation_id: Option<Uuid>,
        #[arg(long)]
        task_id: Option<Uuid>,
        #[arg(long)]
        correlation_id: Option<Uuid>,
        #[arg(long)]
        causation_id: Option<Uuid>,
        #[arg(long, value_parser = parse_message_kind)]
        kind: MessageKind,
        #[arg(long, value_parser = parse_json_value)]
        body: Value,
        #[arg(long, default_value = "[]", value_parser = parse_json_array)]
        references: Value,
        #[arg(long, default_value_t = 3)]
        max_attempts: u32,
        /// Mark the message `priority`: one dead letter of it is a priority incident.
        #[arg(long)]
        priority: bool,
        #[arg(long)]
        idempotency_key: Option<String>,
    },
    Claim {
        id: Uuid,
        #[arg(long)]
        expected_revision: u64,
        #[arg(long)]
        idempotency_key: Option<String>,
    },
    Acknowledge {
        id: Uuid,
        #[arg(long)]
        expected_revision: u64,
        #[arg(long)]
        attempt_id: Uuid,
        #[arg(long)]
        acknowledgement_key: String,
        #[arg(long, default_value = "{}", value_parser = parse_json_value)]
        evidence: Value,
        #[arg(long)]
        idempotency_key: Option<String>,
    },
    Fail {
        id: Uuid,
        #[arg(long)]
        expected_revision: u64,
        #[arg(long)]
        attempt_id: Uuid,
        #[arg(long, action = clap::ArgAction::Set)]
        retryable: bool,
        #[arg(long)]
        failure_code: String,
        #[arg(long)]
        failure_message: String,
        #[arg(long)]
        idempotency_key: Option<String>,
    },
    Retry {
        id: Uuid,
        #[arg(long)]
        expected_revision: u64,
        #[arg(long)]
        idempotency_key: Option<String>,
    },
    Cancel {
        id: Uuid,
        #[arg(long)]
        expected_revision: u64,
        #[arg(long)]
        idempotency_key: Option<String>,
    },
    List {
        #[arg(long, default_value_t = 0)]
        after_sequence: u64,
        #[arg(long, default_value_t = 100)]
        limit: u32,
    },
    Show {
        id: Uuid,
    },
}

#[derive(Debug, Subcommand)]
pub enum ConversationCommand {
    Create {
        lead_employee_id: Uuid,
        #[arg(long)]
        project_id: Option<Uuid>,
        #[arg(long)]
        title: Option<String>,
        #[arg(long)]
        idempotency_key: Option<String>,
    },
    List {
        #[arg(long, default_value_t = 0)]
        after_sequence: u64,
        #[arg(long, default_value_t = 100)]
        limit: u32,
    },
    Show {
        id: Uuid,
    },
    /// Set or clear (omit --title) the conversation's title at the revision you saw.
    Rename {
        id: Uuid,
        #[arg(long)]
        expected_revision: u64,
        #[arg(long)]
        title: Option<String>,
    },
}

#[derive(Debug, Subcommand)]
pub enum TurnCommand {
    Submit {
        conversation_id: Uuid,
        #[arg(long, requires = "recipient_employee")]
        author_employee: Option<Uuid>,
        #[arg(long)]
        recipient_employee: Option<Uuid>,
        #[arg(long)]
        task_id: Option<Uuid>,
        #[arg(long, default_value = "ask", value_parser = parse_authority_mode)]
        authority_mode: AuthorityMode,
        #[arg(long)]
        text: String,
        #[arg(long)]
        activate_if_needed: bool,
        #[arg(long, default_value_t = 3)]
        max_attempts: u32,
        #[arg(long)]
        idempotency_key: Option<String>,
    },
    Retry {
        id: Uuid,
        #[arg(long)]
        expected_revision: u64,
        #[arg(long)]
        idempotency_key: Option<String>,
    },
    Cancel {
        id: Uuid,
        #[arg(long)]
        expected_revision: u64,
        #[arg(long)]
        idempotency_key: Option<String>,
    },
    List {
        conversation_id: Uuid,
        #[arg(long, default_value_t = 0)]
        after_sequence: u64,
        #[arg(long, default_value_t = 100)]
        limit: u32,
    },
    Show {
        id: Uuid,
    },
}

#[derive(Debug, Subcommand)]
pub enum TaskCommand {
    Delegate {
        parent_task_id: Uuid,
        #[arg(long)]
        actor_employee: Uuid,
        #[arg(long)]
        actor_session: Uuid,
        #[arg(long)]
        assigned_employee: Uuid,
        #[arg(long)]
        objective: String,
        #[arg(long, default_value = "[]", value_parser = parse_json_array)]
        references: Value,
        #[arg(long, value_parser = parse_authority_mode)]
        requested_authority: Option<AuthorityMode>,
        #[arg(long)]
        roster_revision: u64,
        #[arg(long)]
        idempotency_key: Option<String>,
    },
    Accept {
        id: Uuid,
        #[arg(long)]
        expected_revision: u64,
        #[arg(long)]
        idempotency_key: Option<String>,
    },
    Start {
        id: Uuid,
        #[arg(long)]
        expected_revision: u64,
        #[arg(long)]
        actor_employee: Uuid,
        #[arg(long)]
        actor_session: Uuid,
        #[arg(long)]
        turn_id: Uuid,
        #[arg(long, default_value = "{}", value_parser = parse_json_value)]
        evidence: Value,
        #[arg(long)]
        idempotency_key: Option<String>,
    },
    Report {
        id: Uuid,
        #[arg(long)]
        expected_revision: u64,
        #[arg(long)]
        actor_employee: Uuid,
        #[arg(long)]
        actor_session: Uuid,
        #[arg(long, value_parser = parse_task_report_kind)]
        kind: TaskReportKind,
        #[arg(long, default_value = "{}", value_parser = parse_json_value)]
        evidence: Value,
        #[arg(long)]
        failure_code: Option<String>,
        #[arg(long)]
        failure_message: Option<String>,
        #[arg(long)]
        idempotency_key: Option<String>,
    },
    Cancel {
        id: Uuid,
        #[arg(long)]
        expected_revision: u64,
        #[arg(long, requires = "actor_session")]
        actor_employee: Option<Uuid>,
        #[arg(long, requires = "actor_employee")]
        actor_session: Option<Uuid>,
        #[arg(long, default_value = "{}", value_parser = parse_json_value)]
        evidence: Value,
        #[arg(long)]
        idempotency_key: Option<String>,
    },
    Resolve {
        id: Uuid,
        #[arg(long)]
        expected_revision: u64,
        #[arg(long, value_parser = parse_task_resolution)]
        resolution: TaskResolution,
        #[arg(long, default_value = "{}", value_parser = parse_json_value)]
        evidence: Value,
        #[arg(long)]
        failure_code: Option<String>,
        #[arg(long)]
        failure_message: Option<String>,
        #[arg(long)]
        idempotency_key: Option<String>,
    },
    List {
        #[arg(long, default_value_t = 0)]
        after_sequence: u64,
        #[arg(long, default_value_t = 100)]
        limit: u32,
        #[arg(long)]
        root_task_id: Option<Uuid>,
        #[arg(long)]
        parent_task_id: Option<Uuid>,
        #[arg(long)]
        conversation_id: Option<Uuid>,
        #[arg(long)]
        project_id: Option<Uuid>,
        #[arg(long)]
        assigned_employee_id: Option<Uuid>,
        #[arg(long, value_parser = parse_task_state)]
        state: Option<TaskState>,
    },
    Show {
        id: Uuid,
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
    if let Command::Terminal(
        command @ (TerminalCommand::Read { .. } | TerminalCommand::Write { .. }),
    ) = &cli.command
    {
        let output = terminal::execute_io(command, &installation)
            .await
            .map_err(|message| {
                CliError::Rpc(RpcError {
                    code: "LZ-TERMINAL-IO".to_owned(),
                    message,
                    data: None,
                })
            })?;
        emit(render_result(&output, cli.json)?);
        return Ok(());
    }
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
        Command::MemoryWorker(command) => memory_worker::command_request(command),
        Command::Terminal(command) => terminal::command_request(command),
        Command::Memory(command) => memory::command_request(command),
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
        Command::Provider(ProviderCommand::ConfigPreview { selection }) => {
            let (method, params) = configuration::preview_request(selection);
            (method, params, false)
        }
        Command::Provider(ProviderCommand::ConfigApply {
            operation_id,
            expected_revision,
        }) => {
            let (method, params) = configuration::apply_request(*operation_id, *expected_revision);
            (method, params, false)
        }
        Command::Provider(ProviderCommand::ConfigRollback {
            operation_id,
            expected_revision,
        }) => {
            let (method, params) =
                configuration::rollback_request(*operation_id, *expected_revision);
            (method, params, false)
        }
        Command::Provider(ProviderCommand::ConfigShow { operation_id }) => {
            let (method, params) = configuration::show_request(*operation_id);
            (method, params, false)
        }
        Command::Provider(ProviderCommand::Config) => ("provider.config", json!({}), false),
        Command::Provider(ProviderCommand::Preview) => {
            ("provider.config.preview", json!({}), false)
        }
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
        Command::Message(command) => message_request(command),
        Command::Conversation(command) => conversation_request(command),
        Command::Turn(command) => turn_request(command),
        Command::Task(command) => task_request(command),
        Command::Approval(command) => approval_request(command),
        Command::Incident(command) => incident_request(command),
        Command::Inbox(command) => inbox_request(command),
        Command::Workspace(command) => workspace_request(command),
        Command::System(SystemCommand::Shutdown) => ("system.shutdown", json!({}), false),
        Command::System(SystemCommand::Resources) => ("system.resources", json!({}), false),
        Command::System(SystemCommand::Budget) => ("system.budget", json!({}), false),
    }
}

fn incident_request(command: &IncidentCommand) -> (&'static str, Value, bool) {
    match command {
        IncidentCommand::List {
            after_sequence,
            limit,
            state,
            priority,
            code,
            employee_id,
            project_id,
            escalation,
        } => (
            "incident.list",
            json!({
                "afterSequence": after_sequence,
                "limit": limit,
                "state": state,
                "priority": priority,
                "code": code,
                "employeeId": employee_id,
                "projectId": project_id,
                "escalation": escalation,
            }),
            false,
        ),
        IncidentCommand::Show { id } => ("incident.show", json!({"id": id}), false),
        IncidentCommand::Acknowledge {
            id,
            expected_revision,
            idempotency_key,
        } => (
            "incident.acknowledge",
            json!({
                "id": id,
                "expectedRevision": expected_revision,
                "idempotencyKey": generated_key(idempotency_key.as_deref()),
            }),
            false,
        ),
        IncidentCommand::Resolve {
            id,
            expected_revision,
            idempotency_key,
        } => (
            "incident.resolve",
            json!({
                "id": id,
                "expectedRevision": expected_revision,
                "idempotencyKey": generated_key(idempotency_key.as_deref()),
            }),
            false,
        ),
        IncidentCommand::Threshold(IncidentThresholdCommand::List) => {
            ("incident.threshold.list", json!({}), false)
        }
        IncidentCommand::Threshold(IncidentThresholdCommand::Set {
            code,
            occurrences,
            window_seconds,
        }) => (
            "incident.threshold.set",
            json!({
                "code": code,
                "occurrences": occurrences,
                "windowSeconds": window_seconds,
            }),
            false,
        ),
        IncidentCommand::Threshold(IncidentThresholdCommand::Reset { code }) => {
            ("incident.threshold.reset", json!({"code": code}), false)
        }
    }
}

fn inbox_request(command: &InboxCommand) -> (&'static str, Value, bool) {
    match command {
        InboxCommand::List {
            filter,
            limit,
            before,
        } => (
            "inbox.list",
            json!({"filter": filter, "limit": limit, "before": before}),
            false,
        ),
        InboxCommand::Counts => ("inbox.counts", json!({}), false),
    }
}

fn workspace_request(command: &WorkspaceCommand) -> (&'static str, Value, bool) {
    match command {
        WorkspaceCommand::Conversations {
            project_id,
            limit,
            before,
        } => (
            "workspace.conversations",
            json!({"projectId": project_id, "limit": limit, "before": before}),
            false,
        ),
        WorkspaceCommand::Launcher { project_id } => (
            "workspace.launcher",
            json!({"projectId": project_id}),
            false,
        ),
        WorkspaceCommand::Show { id, turn_limit } => (
            "workspace.conversation",
            json!({"id": id, "turnLimit": turn_limit}),
            false,
        ),
    }
}

fn approval_request(command: &ApprovalCommand) -> (&'static str, Value, bool) {
    match command {
        ApprovalCommand::List {
            after_sequence,
            limit,
            state,
            turn_id,
            task_id,
            conversation_id,
            employee_id,
        } => (
            "approval.list",
            json!({
                "afterSequence": after_sequence,
                "limit": limit,
                "state": state,
                "turnId": turn_id,
                "taskId": task_id,
                "conversationId": conversation_id,
                "employeeId": employee_id,
            }),
            false,
        ),
        ApprovalCommand::Show { id } => ("approval.show", json!({"id": id}), false),
        ApprovalCommand::Approve {
            id,
            expected_revision,
            idempotency_key,
        } => approval_decision_request(
            *id,
            *expected_revision,
            ApprovalDecision::Approve,
            idempotency_key.as_deref(),
        ),
        ApprovalCommand::Decline {
            id,
            expected_revision,
            idempotency_key,
        } => approval_decision_request(
            *id,
            *expected_revision,
            ApprovalDecision::Decline,
            idempotency_key.as_deref(),
        ),
    }
}

fn approval_decision_request(
    id: Uuid,
    expected_revision: u64,
    decision: ApprovalDecision,
    idempotency_key: Option<&str>,
) -> (&'static str, Value, bool) {
    (
        "approval.decide",
        json!({
            "id": id,
            "expectedRevision": expected_revision,
            "decision": decision,
            "idempotencyKey": generated_key(idempotency_key),
        }),
        false,
    )
}

fn conversation_request(command: &ConversationCommand) -> (&'static str, Value, bool) {
    match command {
        ConversationCommand::Create {
            lead_employee_id,
            project_id,
            title,
            idempotency_key,
        } => (
            "conversation.create",
            json!({
                "projectId": project_id,
                "leadEmployeeId": lead_employee_id,
                "title": title,
                "idempotencyKey": generated_key(idempotency_key.as_deref()),
            }),
            false,
        ),
        ConversationCommand::List {
            after_sequence,
            limit,
        } => (
            "conversation.list",
            json!({"afterSequence": after_sequence, "limit": limit}),
            false,
        ),
        ConversationCommand::Show { id } => ("conversation.show", json!({"id": id}), false),
        ConversationCommand::Rename {
            id,
            expected_revision,
            title,
        } => (
            "conversation.rename",
            json!({"id": id, "expectedRevision": expected_revision, "title": title}),
            false,
        ),
    }
}

fn turn_request(command: &TurnCommand) -> (&'static str, Value, bool) {
    match command {
        TurnCommand::Submit {
            conversation_id,
            author_employee,
            recipient_employee,
            task_id,
            authority_mode,
            text,
            activate_if_needed,
            max_attempts,
            idempotency_key,
        } => {
            let author = author_employee.map_or_else(
                || {
                    MessageSender::User(UserMessageEndpoint {
                        kind: UserMessageKind::User,
                    })
                },
                |employee_id| {
                    MessageSender::Employee(MessageEndpoint {
                        employee_id,
                        session_id: None,
                    })
                },
            );
            let recipient = recipient_employee.map(|employee_id| MessageEndpoint {
                employee_id,
                session_id: None,
            });
            (
                "turn.submit",
                json!({
                    "conversationId": conversation_id,
                    "author": author,
                    "recipient": recipient,
                    "taskId": task_id,
                    "authorityMode": authority_mode,
                    "text": text,
                    "activateIfNeeded": activate_if_needed,
                    "maxAttempts": max_attempts,
                    "idempotencyKey": generated_key(idempotency_key.as_deref()),
                }),
                false,
            )
        }
        TurnCommand::Retry {
            id,
            expected_revision,
            idempotency_key,
        } => turn_revision_request(
            "turn.retry",
            *id,
            *expected_revision,
            idempotency_key.as_deref(),
        ),
        TurnCommand::Cancel {
            id,
            expected_revision,
            idempotency_key,
        } => turn_revision_request(
            "turn.cancel",
            *id,
            *expected_revision,
            idempotency_key.as_deref(),
        ),
        TurnCommand::List {
            conversation_id,
            after_sequence,
            limit,
        } => (
            "turn.list",
            json!({
                "conversationId": conversation_id,
                "afterSequence": after_sequence,
                "limit": limit
            }),
            false,
        ),
        TurnCommand::Show { id } => ("turn.show", json!({"id": id}), false),
    }
}

#[allow(clippy::too_many_lines)] // Keep the eight literal RPC mappings in one auditable function.
fn task_request(command: &TaskCommand) -> (&'static str, Value, bool) {
    match command {
        TaskCommand::Delegate {
            parent_task_id,
            actor_employee,
            actor_session,
            assigned_employee,
            objective,
            references,
            requested_authority,
            roster_revision,
            idempotency_key,
        } => (
            "task.delegate",
            json!({
                "parentTaskId": parent_task_id,
                "actor": {"employeeId": actor_employee, "sessionId": actor_session},
                "assignedEmployeeId": assigned_employee,
                "objective": objective,
                "references": references,
                "requestedAuthority": requested_authority,
                "rosterRevision": roster_revision,
                "idempotencyKey": generated_key(idempotency_key.as_deref()),
            }),
            false,
        ),
        TaskCommand::Accept {
            id,
            expected_revision,
            idempotency_key,
        } => task_revision_request(
            "task.accept",
            *id,
            *expected_revision,
            idempotency_key.as_deref(),
        ),
        TaskCommand::Start {
            id,
            expected_revision,
            actor_employee,
            actor_session,
            turn_id,
            evidence,
            idempotency_key,
        } => (
            "task.start",
            json!({
                "id": id,
                "expectedRevision": expected_revision,
                "actor": {"employeeId": actor_employee, "sessionId": actor_session},
                "turnId": turn_id,
                "evidence": evidence,
                "idempotencyKey": generated_key(idempotency_key.as_deref()),
            }),
            false,
        ),
        TaskCommand::Report {
            id,
            expected_revision,
            actor_employee,
            actor_session,
            kind,
            evidence,
            failure_code,
            failure_message,
            idempotency_key,
        } => (
            "task.report",
            json!({
                "id": id,
                "expectedRevision": expected_revision,
                "actor": {"employeeId": actor_employee, "sessionId": actor_session},
                "kind": kind,
                "evidence": evidence,
                "failureCode": failure_code,
                "failureMessage": failure_message,
                "idempotencyKey": generated_key(idempotency_key.as_deref()),
            }),
            false,
        ),
        TaskCommand::Cancel {
            id,
            expected_revision,
            actor_employee,
            actor_session,
            evidence,
            idempotency_key,
        } => {
            let actor = actor_employee.map_or_else(
                || {
                    MessageSender::User(UserMessageEndpoint {
                        kind: UserMessageKind::User,
                    })
                },
                |employee_id| {
                    MessageSender::Employee(MessageEndpoint {
                        employee_id,
                        session_id: *actor_session,
                    })
                },
            );
            (
                "task.cancel",
                json!({
                    "id": id,
                    "expectedRevision": expected_revision,
                    "actor": actor,
                    "evidence": evidence,
                    "idempotencyKey": generated_key(idempotency_key.as_deref()),
                }),
                false,
            )
        }
        TaskCommand::Resolve {
            id,
            expected_revision,
            resolution,
            evidence,
            failure_code,
            failure_message,
            idempotency_key,
        } => (
            "task.resolve",
            json!({
                "id": id,
                "expectedRevision": expected_revision,
                "resolution": resolution,
                "evidence": evidence,
                "failureCode": failure_code,
                "failureMessage": failure_message,
                "idempotencyKey": generated_key(idempotency_key.as_deref()),
            }),
            false,
        ),
        TaskCommand::List {
            after_sequence,
            limit,
            root_task_id,
            parent_task_id,
            conversation_id,
            project_id,
            assigned_employee_id,
            state,
        } => (
            "task.list",
            json!({
                "afterSequence": after_sequence,
                "limit": limit,
                "rootTaskId": root_task_id,
                "parentTaskId": parent_task_id,
                "conversationId": conversation_id,
                "projectId": project_id,
                "assignedEmployeeId": assigned_employee_id,
                "state": state,
            }),
            false,
        ),
        TaskCommand::Show { id } => ("task.show", json!({"id": id}), false),
    }
}

fn task_revision_request(
    method: &'static str,
    id: Uuid,
    expected_revision: u64,
    idempotency_key: Option<&str>,
) -> (&'static str, Value, bool) {
    (
        method,
        json!({
            "id": id,
            "expectedRevision": expected_revision,
            "idempotencyKey": generated_key(idempotency_key),
        }),
        false,
    )
}

fn turn_revision_request(
    method: &'static str,
    id: Uuid,
    expected_revision: u64,
    idempotency_key: Option<&str>,
) -> (&'static str, Value, bool) {
    (
        method,
        json!({
            "id": id,
            "expectedRevision": expected_revision,
            "idempotencyKey": generated_key(idempotency_key),
        }),
        false,
    )
}

fn generated_key(value: Option<&str>) -> String {
    value.map_or_else(|| Uuid::new_v4().to_string(), str::to_owned)
}

fn message_request(command: &MessageCommand) -> (&'static str, Value, bool) {
    match command {
        MessageCommand::Send { .. } => message_send_request(command),
        MessageCommand::Claim {
            id,
            expected_revision,
            idempotency_key,
        } => message_revision_request(
            "message.claim",
            *id,
            *expected_revision,
            idempotency_key.as_deref(),
        ),
        MessageCommand::Acknowledge {
            id,
            expected_revision,
            attempt_id,
            acknowledgement_key,
            evidence,
            idempotency_key,
        } => (
            "message.acknowledge",
            json!({
                "id": id,
                "expectedRevision": expected_revision,
                "attemptId": attempt_id,
                "acknowledgementKey": acknowledgement_key,
                "evidence": evidence,
                "idempotencyKey": idempotency_key.clone().unwrap_or_else(|| Uuid::new_v4().to_string()),
            }),
            false,
        ),
        MessageCommand::Fail {
            id,
            expected_revision,
            attempt_id,
            retryable,
            failure_code,
            failure_message,
            idempotency_key,
        } => (
            "message.fail",
            json!({
                "id": id,
                "expectedRevision": expected_revision,
                "attemptId": attempt_id,
                "retryable": retryable,
                "failureCode": failure_code,
                "failureMessage": failure_message,
                "idempotencyKey": idempotency_key.clone().unwrap_or_else(|| Uuid::new_v4().to_string()),
            }),
            false,
        ),
        MessageCommand::Retry {
            id,
            expected_revision,
            idempotency_key,
        } => message_revision_request(
            "message.retry",
            *id,
            *expected_revision,
            idempotency_key.as_deref(),
        ),
        MessageCommand::Cancel {
            id,
            expected_revision,
            idempotency_key,
        } => message_revision_request(
            "message.cancel",
            *id,
            *expected_revision,
            idempotency_key.as_deref(),
        ),
        MessageCommand::List {
            after_sequence,
            limit,
        } => (
            "message.list",
            json!({"afterSequence": after_sequence, "limit": limit}),
            false,
        ),
        MessageCommand::Show { id } => ("message.show", json!({"id": id}), false),
    }
}

fn message_send_request(command: &MessageCommand) -> (&'static str, Value, bool) {
    let MessageCommand::Send {
        sender_employee_id,
        recipient_employee_id,
        sender_session_id,
        recipient_session_id,
        project_id,
        conversation_id,
        task_id,
        correlation_id,
        causation_id,
        kind,
        body,
        references,
        max_attempts,
        priority,
        idempotency_key,
    } = command
    else {
        unreachable!("message_send_request only accepts message send commands")
    };

    (
        "message.send",
        json!({
            "sender": {
                "employeeId": sender_employee_id,
                "sessionId": sender_session_id,
            },
            "recipient": {
                "employeeId": recipient_employee_id,
                "sessionId": recipient_session_id,
            },
            "projectId": project_id,
            "conversationId": conversation_id,
            "taskId": task_id,
            "correlationId": correlation_id,
            "causationId": causation_id,
            "kind": kind,
            "body": body,
            "references": references,
            "maxAttempts": max_attempts,
            "priority": if *priority { MessagePriority::Priority } else { MessagePriority::Normal },
            "idempotencyKey": idempotency_key.clone().unwrap_or_else(|| Uuid::new_v4().to_string()),
        }),
        false,
    )
}

fn message_revision_request(
    method: &'static str,
    id: Uuid,
    expected_revision: u64,
    idempotency_key: Option<&str>,
) -> (&'static str, Value, bool) {
    (
        method,
        json!({
            "id": id,
            "expectedRevision": expected_revision,
            "idempotencyKey": idempotency_key
                .map_or_else(|| Uuid::new_v4().to_string(), str::to_owned),
        }),
        false,
    )
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
        SessionCommand::Resources { id } => ("session.resources", json!({"id": id}), false),
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
        SessionCommand::Attach {
            id,
            expected_revision,
            columns,
            rows,
            idempotency_key,
        } => (
            "session.terminal.attach",
            json!({
                "id": id,
                "expectedRevision": expected_revision,
                "columns": columns,
                "rows": rows,
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
            activation_policy,
            idle_timeout_seconds,
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
                "activationPolicy": activation_policy,
                "idleTimeoutSeconds": idle_timeout_seconds,
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
            activation_policy,
            idle_timeout_seconds,
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
                "activationPolicy": activation_policy,
                "idleTimeoutSeconds": idle_timeout_seconds,
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
        code if code.starts_with("LZ-MESSAGE-") => 11,
        code if code.starts_with("LZ-CONVERSATION-") || code.starts_with("LZ-TURN-") => 12,
        code if code.starts_with("LZ-TASK-") => 13,
        code if code.starts_with("LZ-APPROVAL-") => 14,
        code if code.starts_with("LZ-INCIDENT-") => 15,
        _ => 10,
    }
}

fn parse_inbox_filter(value: &str) -> Result<InboxFilter, String> {
    match value {
        "all" => Ok(InboxFilter::All),
        "approvals" => Ok(InboxFilter::Approvals),
        "permissions" => Ok(InboxFilter::Permissions),
        "incidents" => Ok(InboxFilter::Incidents),
        "resolved" => Ok(InboxFilter::Resolved),
        _ => Err("expected all, approvals, permissions, incidents, or resolved".to_owned()),
    }
}

fn parse_incident_state(value: &str) -> Result<IncidentState, String> {
    match value {
        "open" => Ok(IncidentState::Open),
        "acknowledged" => Ok(IncidentState::Acknowledged),
        "resolved" => Ok(IncidentState::Resolved),
        _ => Err("expected open, acknowledged, or resolved".to_owned()),
    }
}

fn parse_incident_priority(value: &str) -> Result<IncidentPriority, String> {
    match value {
        "normal" => Ok(IncidentPriority::Normal),
        "priority" => Ok(IncidentPriority::Priority),
        _ => Err("expected normal or priority".to_owned()),
    }
}

fn parse_incident_escalation(value: &str) -> Result<IncidentEscalation, String> {
    match value {
        "none" => Ok(IncidentEscalation::None),
        "lead" => Ok(IncidentEscalation::Lead),
        "remediated" => Ok(IncidentEscalation::Remediated),
        "user" => Ok(IncidentEscalation::User),
        _ => Err("expected none, lead, remediated, or user".to_owned()),
    }
}

fn parse_approval_state(value: &str) -> Result<ApprovalState, String> {
    match value {
        "pending" => Ok(ApprovalState::Pending),
        "approved" => Ok(ApprovalState::Approved),
        "declined" => Ok(ApprovalState::Declined),
        "expired" => Ok(ApprovalState::Expired),
        "cancelled" => Ok(ApprovalState::Cancelled),
        _ => Err("expected pending, approved, declined, expired, or cancelled".to_owned()),
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

fn parse_json_array(value: &str) -> Result<Value, String> {
    let value = parse_json_value(value)?;
    if value.is_array() {
        Ok(value)
    } else {
        Err("expected a JSON array".to_owned())
    }
}

fn parse_message_kind(value: &str) -> Result<MessageKind, String> {
    match value {
        "delegation" => Ok(MessageKind::Delegation),
        "peer" => Ok(MessageKind::Peer),
        "reply" => Ok(MessageKind::Reply),
        "result" => Ok(MessageKind::Result),
        "status" => Ok(MessageKind::Status),
        "system" => Ok(MessageKind::System),
        _ => Err("expected delegation, peer, reply, result, status, or system".to_owned()),
    }
}

fn parse_authority_mode(value: &str) -> Result<AuthorityMode, String> {
    match value {
        "ask" => Ok(AuthorityMode::Ask),
        "self-decision" => Ok(AuthorityMode::SelfDecision),
        "full-autonomy" => Ok(AuthorityMode::FullAutonomy),
        _ => Err("expected ask, self-decision, or full-autonomy".to_owned()),
    }
}

fn parse_task_state(value: &str) -> Result<TaskState, String> {
    match value {
        "proposed" => Ok(TaskState::Proposed),
        "accepted" => Ok(TaskState::Accepted),
        "running" => Ok(TaskState::Running),
        "cancel-requested" => Ok(TaskState::CancelRequested),
        "completed" => Ok(TaskState::Completed),
        "failed" => Ok(TaskState::Failed),
        "cancelled" => Ok(TaskState::Cancelled),
        "uncertain" => Ok(TaskState::Uncertain),
        _ => Err("expected proposed, accepted, running, cancel-requested, completed, failed, cancelled, or uncertain".to_owned()),
    }
}

fn parse_task_report_kind(value: &str) -> Result<TaskReportKind, String> {
    match value {
        "progress" => Ok(TaskReportKind::Progress),
        "completed" => Ok(TaskReportKind::Completed),
        "failed" => Ok(TaskReportKind::Failed),
        _ => Err("expected progress, completed, or failed".to_owned()),
    }
}

fn parse_task_resolution(value: &str) -> Result<TaskResolution, String> {
    match value {
        "completed" => Ok(TaskResolution::Completed),
        "failed" => Ok(TaskResolution::Failed),
        "cancelled" => Ok(TaskResolution::Cancelled),
        _ => Err("expected completed, failed, or cancelled".to_owned()),
    }
}

fn parse_employee_checkout_policy(value: &str) -> Result<EmployeeCheckoutPolicy, String> {
    match value {
        "isolated" => Ok(EmployeeCheckoutPolicy::Isolated),
        "read-only" => Ok(EmployeeCheckoutPolicy::ReadOnly),
        _ => Err("expected isolated or read-only".to_owned()),
    }
}

fn parse_employee_activation_policy(value: &str) -> Result<ActivationPolicy, String> {
    match value {
        "manual" => Ok(ActivationPolicy::Manual),
        "notice-turns" => Ok(ActivationPolicy::NoticeTurns),
        _ => Err("expected manual or notice-turns".to_owned()),
    }
}
