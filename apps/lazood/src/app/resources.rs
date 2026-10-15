//! Resource accounting (brief section 20): the daemon's own process and each live session's
//! native process tree, read from Windows on request and never stored. A provider's use is
//! displayed beside Lazoo's own and never counted against Lazoo's ceiling.

use chrono::{SecondsFormat, Utc};
use lazoo_protocol::{DaemonResources, RpcError, SessionResources, SessionResourcesParams};
use lazoo_runtime::current_process_accounting;
use serde_json::{Value, json};

use super::{App, invalid_params, session_store_error, to_value};

impl App {
    /// Job Object accounting for one session that is live in this daemon. A session that is
    /// not live (never started here, suspended, or stopped) has no tree to measure.
    pub(super) async fn session_resources(&self, value: &Value) -> Result<Value, RpcError> {
        let params: SessionResourcesParams = serde_json::from_value(value.clone())
            .map_err(|error| invalid_params(error.to_string()))?;
        let session = self
            .store
            .get_session(params.id)
            .map_err(session_store_error)?;
        let live = self.live_sessions.lock().await.get(&params.id).cloned();
        let accounting = live
            .and_then(|handle| handle.resources())
            .ok_or_else(|| RpcError {
                code: "LZ-SESSION-NOT-LIVE".to_owned(),
                message: "session is not live in this daemon; resources are measured only while its native process tree runs".to_owned(),
                data: Some(json!({"sessionId": params.id, "state": session.state})),
            })?;
        Ok(to_value(SessionResources {
            session_id: session.id,
            employee_id: session.employee_id,
            provider: session.provider,
            native_host_pid: session.native_host_pid,
            active_processes: accounting.active_processes,
            total_processes: accounting.total_processes,
            cpu_millis: accounting.user_cpu_millis + accounting.kernel_cpu_millis,
            user_cpu_millis: accounting.user_cpu_millis,
            kernel_cpu_millis: accounting.kernel_cpu_millis,
            peak_process_memory_bytes: accounting.peak_process_memory_bytes,
            peak_job_memory_bytes: accounting.peak_job_memory_bytes,
            sampled_at: now(),
        }))
    }

    /// The daemon's own process: the part of "Lazoo itself" this process can measure.
    pub(super) async fn system_resources(&self, value: &Value) -> Result<Value, RpcError> {
        super::require_empty_params(value, "system.resources")?;
        let accounting = current_process_accounting().map_err(|error| RpcError {
            code: "LZ-RESOURCES".to_owned(),
            message: error.to_string(),
            data: None,
        })?;
        let live_sessions =
            u32::try_from(self.live_sessions.lock().await.len()).unwrap_or(u32::MAX);
        Ok(to_value(DaemonResources {
            pid: accounting.pid,
            boot_id: self.boot_id,
            uptime_millis: u64::try_from(self.started.elapsed().as_millis()).unwrap_or(u64::MAX),
            cpu_millis: accounting.user_cpu_millis + accounting.kernel_cpu_millis,
            user_cpu_millis: accounting.user_cpu_millis,
            kernel_cpu_millis: accounting.kernel_cpu_millis,
            working_set_bytes: accounting.working_set_bytes,
            peak_working_set_bytes: accounting.peak_working_set_bytes,
            handle_count: accounting.handle_count,
            live_sessions,
            sampled_at: now(),
        }))
    }
}

fn now() -> String {
    Utc::now().to_rfc3339_opts(SecondsFormat::Millis, true)
}
