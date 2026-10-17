//! Strict final-text parser for an internal memory-summary turn.
//!
//! Dispatch is deliberately kept in the app integration layer: it must claim a durable job,
//! create a dedicated audited conversation/turn, and never retry an uncertain turn.

use lazoo_protocol::{EmployeeRecord, MemoryRecordType};
use lazoo_providers::ProviderKind;
use serde::Deserialize;
use serde_json::{Value, json};
use tokio::time::Duration;
use uuid::Uuid;

use super::App;

pub(super) const MAX_RECORDS: usize = 32;
pub(super) const MAX_TEXT_BYTES: usize = 8_192;

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct SummaryOutput {
    pub records: Vec<SummaryRecord>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct SummaryRecord {
    #[serde(rename = "type")]
    pub record_type: MemoryRecordType,
    pub text: String,
    pub confidence: f64,
    pub importance: u8,
}

/// Accept only one JSON value, with no prose wrapper or model-supplied source identifiers.
pub(super) fn parse_summary_output(text: &str) -> Result<SummaryOutput, String> {
    if text.len() > 64 * 1024 {
        return Err("summary output exceeds 64 KiB".to_owned());
    }
    let output: SummaryOutput =
        serde_json::from_str(text).map_err(|_| "summary output must be strict JSON".to_owned())?;
    if output.records.len() > MAX_RECORDS {
        return Err("summary output has too many records".to_owned());
    }
    for record in &output.records {
        if record.text.is_empty()
            || record.text.len() > MAX_TEXT_BYTES
            || !(0.0..=1.0).contains(&record.confidence)
            || record.importance > 100
        {
            return Err("summary record is out of bounds".to_owned());
        }
    }
    Ok(output)
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct WorkerScope {
    project_id: Option<Uuid>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct WorkerConfigure {
    project_id: Option<Uuid>,
    expected_revision: Option<u64>,
    employee_id: Uuid,
    model: String,
    requests_per_minute: u32,
    enabled: bool,
}

impl App {
    pub(super) fn memory_worker_show(
        &self,
        value: Value,
    ) -> Result<Value, lazoo_protocol::RpcError> {
        let scope: WorkerScope =
            serde_json::from_value(value).map_err(|e| super::invalid_params(e.to_string()))?;
        let settings = self
            .store
            .memory_worker_settings(scope.project_id)
            .map_err(super::store_error)?;
        let employee = settings
            .as_ref()
            .map(|settings| self.store.get_employee(settings.employee_id))
            .transpose()
            .map_err(super::store_error)?;
        Ok(worker_response(settings.as_ref(), employee.as_ref()))
    }
    pub(super) fn memory_worker_configure(
        &self,
        value: Value,
    ) -> Result<Value, lazoo_protocol::RpcError> {
        let input: WorkerConfigure =
            serde_json::from_value(value).map_err(|e| super::invalid_params(e.to_string()))?;
        if !(1..=60).contains(&input.requests_per_minute) {
            return Err(super::invalid_params(
                "requestsPerMinute must be between 1 and 60",
            ));
        }
        let employee = self
            .store
            .get_employee(input.employee_id)
            .map_err(super::store_error)?;
        if input.enabled && !employee.enabled {
            return Err(super::invalid_params(
                "memory worker employee must be enabled",
            ));
        }
        if input.enabled && employee.model.as_deref() != Some(input.model.as_str()) {
            return Err(super::invalid_params(
                "memory worker model must exactly match the employee model",
            ));
        }
        let provider = employee
            .provider
            .parse::<ProviderKind>()
            .map_err(|_| super::invalid_params("memory worker employee provider is unsupported"))?;
        if input.enabled && !super::provider_runs_memory_summaries(provider) {
            return Err(super::invalid_params(
                "memory worker provider cannot enforce the governed ask authority ceiling",
            ));
        }
        let settings = self
            .store
            .set_memory_worker_settings(&lazoo_store::SetMemoryWorkerSettingsInput {
                project_id: input.project_id,
                expected_revision: input.expected_revision,
                employee_id: input.employee_id,
                model: input.model,
                requests_per_minute: input.requests_per_minute,
                enabled: input.enabled,
            })
            .map_err(super::store_error)?;
        Ok(worker_response(Some(&settings), Some(&employee)))
    }
    /// Single-concurrency scheduler. One admission per minute is a conservative global bound and
    /// therefore never exceeds any configured per-project request budget (which is at least one).
    pub(crate) async fn run_memory_worker(&self) {
        let mut ticks = tokio::time::interval(Duration::from_mins(1));
        ticks.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            ticks.tick().await;
            if let Err(error) = self.run_memory_worker_once().await {
                tracing::warn!(%error, "memory summary worker admission failed");
            }
        }
    }

    pub(crate) async fn run_memory_worker_once(&self) -> Result<bool, String> {
        if self.background_throttled() {
            return Ok(false);
        }
        self.summarize_next_memory_job_once().await
    }

    /// One unthrottled admission: claim the next due job and run it to settlement. The daemon
    /// loop only ever reaches this through the throttled `run_memory_worker_once`.
    #[doc(hidden)]
    pub async fn summarize_next_memory_job_once(&self) -> Result<bool, String> {
        let Some(job) = self
            .store
            .claim_memory_summary_for_worker(self.boot_id)
            .map_err(|error| error.to_string())?
        else {
            return Ok(false);
        };
        self.process_memory_job(&job).await?;
        Ok(true)
    }
}
fn worker_response(
    settings: Option<&lazoo_store::MemoryWorkerSettings>,
    employee: Option<&EmployeeRecord>,
) -> Value {
    let provider = employee.as_ref().map(|employee| employee.provider.as_str());
    let model = employee
        .as_ref()
        .and_then(|employee| employee.model.as_deref());
    json!({
        "settings": settings,
        "provider": provider,
        "model": model,
        "projectPermission": employee.map(|_| json!({"state": "nativeEmployeeOnly"})),
    })
}

#[cfg(test)]
mod tests {
    use super::parse_summary_output;

    #[test]
    fn summary_output_is_strict_and_bounded() {
        assert!(
            parse_summary_output(
                r#"{"records":[{"type":"fact","text":"ok","confidence":0.5,"importance":1}]}"#
            )
            .is_ok()
        );
        assert!(parse_summary_output("prefix {} ").is_err());
    }
}
