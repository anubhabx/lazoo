//! Resource accounting (brief section 20): the daemon's own process and each live session's
//! native process tree, read from Windows on request and never stored. A provider's use is
//! displayed beside Lazoo's own and never counted against Lazoo's ceiling.

use chrono::{SecondsFormat, Utc};
use lazoo_protocol::{DaemonResources, RpcError, SessionResources, SessionResourcesParams};
use lazoo_runtime::current_process_accounting;
use serde_json::{Value, json};

use super::{App, invalid_params, session_store_error, to_value};

pub(super) struct ResourceBudget {
    governor: lazoo_runtime::InfrastructureGovernor,
    rolling: std::sync::Mutex<lazoo_runtime::RollingBudget>,
    assessment: std::sync::Mutex<Option<lazoo_runtime::BudgetAssessment>>,
    started: std::time::Instant,
}

impl ResourceBudget {
    fn sample(&self) -> Result<(), String> {
        let point = self
            .governor
            .point_sample()
            .map_err(|error| error.to_string())?;
        let next = self
            .rolling
            .lock()
            .map_err(|_| "resource sampler lock poisoned")?
            .observe(lazoo_runtime::BudgetSample {
                elapsed: self.started.elapsed(),
                cumulative_cpu: std::time::Duration::from_millis(
                    point
                        .user_cpu_millis
                        .saturating_add(point.kernel_cpu_millis),
                ),
            })
            .map_err(|error| error.to_string())?;
        *self
            .assessment
            .lock()
            .map_err(|_| "resource sampler lock poisoned")? = Some(next);
        Ok(())
    }
}

impl App {
    pub(super) fn background_throttled(&self) -> bool {
        self.resource_budget.as_ref().is_none_or(|budget| {
            budget.assessment.lock().map_or(true, |value| {
                value.is_none_or(|assessment| assessment.throttle_background)
            })
        })
    }

    #[must_use]
    pub fn with_resource_governor(
        mut self,
        result: Result<lazoo_runtime::InfrastructureGovernor, String>,
    ) -> Self {
        match result {
            Ok(governor) => {
                let processors = u32::try_from(
                    std::thread::available_parallelism().map_or(1, std::num::NonZeroUsize::get),
                )
                .unwrap_or(u32::MAX);
                self.resource_budget = Some(std::sync::Arc::new(ResourceBudget {
                    governor,
                    rolling: std::sync::Mutex::new(
                        lazoo_runtime::RollingBudget::new(processors)
                            .expect("logical processor count is positive"),
                    ),
                    assessment: std::sync::Mutex::new(None),
                    started: std::time::Instant::now(),
                }));
            }
            Err(error) => self.resource_budget_error = Some(error),
        }
        self
    }

    pub(crate) async fn run_resource_sampler(&self) {
        let Some(budget) = &self.resource_budget else {
            return;
        };
        let mut ticks = tokio::time::interval(std::time::Duration::from_secs(1));
        ticks.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            ticks.tick().await;
            if let Err(error) = budget.sample() {
                if let Ok(mut assessment) = budget.assessment.lock() {
                    *assessment = None;
                }
                tracing::warn!(%error, "resource sample unavailable");
            }
        }
    }

    pub(super) fn system_budget(&self, value: &Value) -> Result<Value, RpcError> {
        super::require_empty_params(value, "system.budget")?;
        let Some(budget) = &self.resource_budget else {
            return Ok(
                json!({"enforced": false, "reason": self.resource_budget_error.as_deref()
                .unwrap_or("this daemon has not enrolled in an infrastructure job"),
                "desktopIncluded": false, "providerTreesIncluded": false}),
            );
        };
        let point = budget.governor.point_sample().map_err(|error| RpcError {
            code: "LZ-RESOURCES".to_owned(),
            message: error.to_string(),
            data: None,
        })?;
        let assessment = budget.assessment.lock().map_err(|_| RpcError {
            code: "LZ-RESOURCES".to_owned(),
            message: "resource sampler lock poisoned".to_owned(),
            data: None,
        })?;
        Ok(json!({
            "enforced": true, "hardCapPercent": 20, "softThresholdPercent": 15,
            "activeProcesses": point.active_processes, "totalProcesses": point.total_processes,
            "cpuMillis": point.user_cpu_millis.saturating_add(point.kernel_cpu_millis),
            "peakJobMemoryBytes": point.peak_job_memory_bytes,
            "thirtySeconds": window_json(assessment.map(|value| value.thirty_seconds)),
            "fiveMinutes": window_json(assessment.map(|value| value.five_minutes)),
            "throttleBackground": assessment.is_none_or(|value| value.throttle_background),
            "desktopIncluded": false, "providerTreesIncluded": false, "sampledAt": now(),
        }))
    }

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

fn window_json(window: Option<lazoo_runtime::WindowState>) -> Value {
    match window {
        Some(lazoo_runtime::WindowState::Ready {
            aggregate_machine_cpu_percent,
        }) => json!({"state": "ready", "cpuPercent": aggregate_machine_cpu_percent}),
        _ => json!({"state": "warming"}),
    }
}

fn now() -> String {
    Utc::now().to_rfc3339_opts(SecondsFormat::Millis, true)
}
