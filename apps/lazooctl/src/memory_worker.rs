//! CLI request construction for durable memory-summary worker settings.

use clap::Subcommand;
use serde_json::{Value, json};
use uuid::Uuid;

#[derive(Debug, Subcommand)]
pub enum MemoryWorkerCommand {
    Show {
        #[arg(long)]
        project_id: Option<Uuid>,
    },
    Configure {
        #[arg(long)]
        project_id: Option<Uuid>,
        #[arg(long)]
        employee_id: Uuid,
        #[arg(long)]
        model: String,
        #[arg(long)]
        requests_per_minute: u32,
        #[arg(long)]
        enabled: bool,
        #[arg(long)]
        expected_revision: Option<u64>,
    },
}

pub fn command_request(command: &MemoryWorkerCommand) -> (&'static str, Value, bool) {
    match command {
        MemoryWorkerCommand::Show { project_id } => (
            "memory.worker.show",
            json!({"projectId": project_id}),
            false,
        ),
        MemoryWorkerCommand::Configure {
            project_id,
            employee_id,
            model,
            requests_per_minute,
            enabled,
            expected_revision,
        } => (
            "memory.worker.configure",
            json!({
                "projectId": project_id,
                "employeeId": employee_id,
                "model": model,
                "requestsPerMinute": requests_per_minute,
                "enabled": enabled,
                "expectedRevision": expected_revision,
            }),
            false,
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn configure_uses_the_strict_worker_rpc_shape() {
        let employee_id = Uuid::nil();
        let (method, parameters, follows) = command_request(&MemoryWorkerCommand::Configure {
            project_id: None,
            employee_id,
            model: "gpt-6.1".into(),
            requests_per_minute: 12,
            enabled: true,
            expected_revision: None,
        });
        assert_eq!(method, "memory.worker.configure");
        assert!(!follows);
        assert_eq!(parameters["employeeId"], employee_id.to_string());
        assert!(parameters["expectedRevision"].is_null());
    }
}
