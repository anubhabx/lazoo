use std::{
    env,
    path::PathBuf,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use chrono::Utc;
use lazoo_runtime::{DataRoot, Installation, NativeNotifier, RuntimeError, SingleInstance};
use thiserror::Error;
use tokio::{process::Command, time::sleep};
use uuid::Uuid;

use crate::{
    EmergencyIncident, EmergencySpool, KeeperState, RestartAction, RestartBudget, SpoolError,
};

pub trait NotificationPort {
    fn notify(&self, title: &str, message: &str) -> Result<(), String>;
}

impl NotificationPort for NativeNotifier {
    fn notify(&self, title: &str, message: &str) -> Result<(), String> {
        NativeNotifier::notify(self, title, message).map_err(|error| error.to_string())
    }
}

#[derive(Debug, Error)]
pub enum KeeperError {
    #[error(transparent)]
    Runtime(#[from] RuntimeError),
    #[error(transparent)]
    Spool(#[from] SpoolError),
    #[error("daemon process failed: {0}")]
    Process(#[from] std::io::Error),
    #[error("system clock precedes Unix epoch")]
    Clock,
}

pub async fn run() -> Result<i32, KeeperError> {
    let root = DataRoot::resolve()?;
    root.ensure()?;
    let installation = Installation::load_or_create(&root)?;
    let _instance = SingleInstance::acquire(installation.id(), "keeper")?;
    EmergencySpool::new(root.clone()).recover_stale()?;
    supervise(&root, &NativeNotifier).await
}

async fn supervise(root: &DataRoot, notifier: &impl NotificationPort) -> Result<i32, KeeperError> {
    let mut state = KeeperState::load(root)?;
    let mut budget = RestartBudget::from_exit_times(state.exit_times_millis.clone());
    let daemon_path = daemon_path()?;
    loop {
        let started = SystemTime::now();
        let status = Command::new(&daemon_path)
            .env("LAZOO_DATA_ROOT", root.root())
            .status()
            .await?;
        if status.success() {
            state.consecutive_failures = 0;
            state.last_exit_code = status.code();
            state.exit_times_millis.clear();
            state.save(root)?;
            return Ok(0);
        }
        let runtime = started.elapsed().unwrap_or(Duration::ZERO);
        let now = unix_millis()?;
        if runtime >= Duration::from_secs(10 * 60) {
            budget = RestartBudget::default();
        }
        state.last_exit_code = status.code();
        state.consecutive_failures = state.consecutive_failures.saturating_add(1);
        match budget.record_exit(now) {
            RestartAction::RestartAfter(delay) => {
                state.exit_times_millis = budget.exit_times().to_vec();
                state.save(root)?;
                sleep(delay).await;
            }
            RestartAction::Exhausted => {
                state.exit_times_millis = budget.exit_times().to_vec();
                record_unavailable(root, notifier, &mut state, status.code().unwrap_or(-1))?;
                return Ok(20);
            }
        }
    }
}

pub fn record_unavailable(
    root: &DataRoot,
    notifier: &impl NotificationPort,
    state: &mut KeeperState,
    exit_code: i32,
) -> Result<(), KeeperError> {
    let incident_id = state
        .last_incident_id
        .clone()
        .unwrap_or_else(|| Uuid::new_v4().to_string());
    let incident = EmergencyIncident {
        incident_id: incident_id.clone(),
        code: "LZ-DAEMON-UNAVAILABLE".to_owned(),
        message: format!("lazood exhausted its restart budget after exit code {exit_code}"),
        occurred_at: Utc::now().to_rfc3339(),
    };
    EmergencySpool::new(root.clone()).append_and_seal(&incident)?;
    state.last_incident_id = Some(incident_id);
    state.save(root)?;
    state.notification_attempted = true;
    state.notification_error = notifier
        .notify("Lazoo daemon unavailable", &incident.message)
        .err();
    state.save(root)?;
    if let Some(error) = &state.notification_error {
        eprintln!("native notification failed after durable spool: {error}");
    }
    Ok(())
}

fn daemon_path() -> Result<PathBuf, std::io::Error> {
    if let Some(path) = env::var_os("LAZOO_DAEMON_PATH") {
        return Ok(PathBuf::from(path));
    }
    let mut path = env::current_exe()?;
    path.set_file_name("lazood.exe");
    Ok(path)
}

fn unix_millis() -> Result<u64, KeeperError> {
    let millis = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|_| KeeperError::Clock)?
        .as_millis();
    u64::try_from(millis).map_err(|_| KeeperError::Clock)
}
