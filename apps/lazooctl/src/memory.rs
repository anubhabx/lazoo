use clap::Subcommand;
use lazoo_protocol::MemoryRecordType;
use serde_json::{Value, json};
use uuid::Uuid;

#[derive(Debug, Subcommand)]
pub enum MemoryCommand {
    /// Show a directly configured policy. `--global` and `--project-id` cannot be combined.
    PolicyShow {
        #[arg(long)]
        global: bool,
        #[arg(long)]
        project_id: Option<Uuid>,
    },
    /// Replace a policy with revision checking. This does not archive or delete anything.
    PolicySet {
        #[arg(long)]
        global: bool,
        #[arg(long)]
        project_id: Option<Uuid>,
        #[arg(long)]
        expected_revision: u64,
        #[arg(long)]
        retention_enabled: bool,
        #[arg(long, default_value_t = 30)]
        retention_days: u32,
        #[arg(long, default_value_t = 268_435_456)]
        quota_bytes: u64,
        #[arg(long, default_value_t = 4_194_304)]
        max_source_bytes: u64,
        #[arg(long = "allow-type", value_parser = parse_record_type)]
        allowed_types: Vec<MemoryRecordType>,
        #[arg(long, default_value_t = true, action = clap::ArgAction::Set)]
        inject_enabled: bool,
    },
    Archive {
        source_turn_id: Uuid,
        #[arg(long)]
        project_id: Option<Uuid>,
    },
    Search {
        query: String,
        #[arg(long)]
        project_id: Option<Uuid>,
        #[arg(long)]
        employee_id: Option<Uuid>,
        #[arg(long)]
        conversation_id: Option<Uuid>,
        #[arg(long)]
        task_id: Option<Uuid>,
        #[arg(long)]
        root_task_id: Option<Uuid>,
        #[arg(long)]
        branch: Option<String>,
        #[arg(long = "type", value_parser = parse_record_type)]
        record_types: Vec<MemoryRecordType>,
        #[arg(long, default_value_t = 20)]
        limit: u32,
    },
    Context {
        query: String,
        #[arg(long)]
        project_id: Option<Uuid>,
        #[arg(long)]
        employee_id: Option<Uuid>,
        #[arg(long)]
        conversation_id: Option<Uuid>,
        #[arg(long)]
        task_id: Option<Uuid>,
        #[arg(long)]
        root_task_id: Option<Uuid>,
        #[arg(long)]
        branch: Option<String>,
        #[arg(long = "type", value_parser = parse_record_type)]
        record_types: Vec<MemoryRecordType>,
        #[arg(long, default_value_t = 8)]
        limit: u32,
    },
    CleanupPreview {
        #[arg(long)]
        project_id: Option<Uuid>,
        #[arg(long)]
        before: String,
    },
    /// Apply exactly the preview JSON supplied by a preceding cleanup-preview response.
    CleanupApply {
        #[arg(long)]
        preview_json: String,
    },
    QueueStatus {
        #[arg(long)]
        project_id: Option<Uuid>,
        #[arg(long, default_value_t = 20)]
        limit: u32,
    },
}

pub fn command_request(command: &MemoryCommand) -> (&'static str, Value, bool) {
    match command {
        MemoryCommand::PolicyShow { global, project_id } => (
            "memory.policy.show",
            json!({"global":global,"projectId":project_id}),
            false,
        ),
        MemoryCommand::PolicySet {
            global,
            project_id,
            expected_revision,
            retention_enabled,
            retention_days,
            quota_bytes,
            max_source_bytes,
            allowed_types,
            inject_enabled,
        } => (
            "memory.policy.set",
            json!({"global":global,"projectId":project_id,"expectedRevision":expected_revision,"retentionEnabled":retention_enabled,"retentionDays":retention_days,"quotaBytes":quota_bytes,"maxSourceBytes":max_source_bytes,"allowedTypes":allowed_types,"injectEnabled":inject_enabled}),
            false,
        ),
        MemoryCommand::Archive {
            source_turn_id,
            project_id,
        } => (
            "memory.archive",
            json!({"sourceTurnId":source_turn_id,"projectId":project_id}),
            false,
        ),
        MemoryCommand::Search {
            query,
            project_id,
            employee_id,
            conversation_id,
            task_id,
            root_task_id,
            branch,
            record_types,
            limit,
        } => (
            "memory.search",
            json!({"query":query,"projectId":project_id,"employeeId":employee_id,"conversationId":conversation_id,"taskId":task_id,"rootTaskId":root_task_id,"branch":branch,"recordTypes":record_types,"limit":limit}),
            false,
        ),
        MemoryCommand::Context {
            query,
            project_id,
            employee_id,
            conversation_id,
            task_id,
            root_task_id,
            branch,
            record_types,
            limit,
        } => (
            "memory.context",
            json!({"query":query,"projectId":project_id,"employeeId":employee_id,"conversationId":conversation_id,"taskId":task_id,"rootTaskId":root_task_id,"branch":branch,"recordTypes":record_types,"limit":limit}),
            false,
        ),
        MemoryCommand::CleanupPreview { project_id, before } => (
            "memory.cleanup.preview",
            json!({"projectId":project_id,"before":before}),
            false,
        ),
        MemoryCommand::CleanupApply { preview_json } => {
            let preview = serde_json::from_str::<Value>(preview_json)
                .unwrap_or_else(|_| Value::String(preview_json.clone()));
            ("memory.cleanup.apply", json!({"preview":preview}), false)
        }
        MemoryCommand::QueueStatus { project_id, limit } => (
            "memory.queue.status",
            json!({"projectId":project_id,"limit":limit}),
            false,
        ),
    }
}

fn parse_record_type(value: &str) -> Result<MemoryRecordType, String> {
    serde_json::from_value(Value::String(value.to_owned())).map_err(|_| {
        "type must be decision, action, achievement, question, failure, constraint, fact, or summary".to_owned()
    })
}
