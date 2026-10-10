//! Idle suspension: a session that has been quiet for its employee's `idleTimeoutSeconds` is
//! stopped, so a warmed employee does not keep a provider process alive that nobody is using.
//!
//! The rule is the employee's own setting, and 0 (the default) never suspends. Only an idle
//! session owned by this daemon boot with no unfinished turn on it is due; the store re-checks
//! that in the same transaction that marks it `stopping`, under the `session.stop` key
//! `idle-suspend:<sessionId>`. Suspension ends the native thread: the next user turn or a
//! `notice-turns` activation starts a fresh session exactly as it would for any dormant employee.

use chrono::{DateTime, Utc};
use lazoo_protocol::RpcError;
use lazoo_store::BeginIdleSessionStopInput;
use tokio::time::{Duration, sleep};

use super::{App, session_store_error};

// ponytail: fixed sweep cadence, so a session suspends up to this long after its deadline;
// sleep until the earliest deadline instead if that slack ever matters.
const IDLE_SWEEP_INTERVAL: Duration = Duration::from_secs(15);

impl App {
    /// Runs the timed sweeps until shutdown: idle suspension, closing lead remediation
    /// windows (`escalate_due_incidents_once`), and announcing user escalations
    /// (`notify_user_escalations_once`), which share this cadence and its slack.
    pub async fn run_timed_sweeps(&self) {
        let mut shutdown = self.shutdown_receiver();
        loop {
            if *shutdown.borrow() {
                return;
            }
            let now = Utc::now();
            if let Err(error) = self.suspend_idle_sessions_once(now).await {
                tracing::warn!(code = %error.code, message = %error.message, "idle suspension failed");
            }
            if let Err(error) = self.escalate_due_incidents_once(now) {
                tracing::warn!(code = %error.code, message = %error.message, "incident escalation failed");
            }
            if let Err(error) = self.notify_user_escalations_once(now) {
                tracing::warn!(code = %error.code, message = %error.message, "incident notification failed");
            }
            tokio::select! {
                () = sleep(IDLE_SWEEP_INTERVAL) => {}
                changed = shutdown.changed() => {
                    if changed.is_err() || *shutdown.borrow() {
                        return;
                    }
                }
            }
        }
    }

    /// One sweep at `now`: stops every session that is due. A session that became busy
    /// between the listing and its stop is skipped; a failed provider stop fails the session
    /// and raises an incident through the ordinary stop path, and is never retried.
    #[doc(hidden)]
    pub async fn suspend_idle_sessions_once(&self, now: DateTime<Utc>) -> Result<(), RpcError> {
        let mut first_error = None;
        for session in self
            .store
            .idle_sessions_due(self.boot_id, now)
            .map_err(session_store_error)?
        {
            let stopping = match self
                .store
                .begin_idle_session_stop(&BeginIdleSessionStopInput {
                    session_id: session.id,
                    owner_boot_id: self.boot_id,
                    now,
                }) {
                Ok(Some(stopping)) => stopping,
                Ok(None) => continue,
                Err(error) => {
                    first_error.get_or_insert(session_store_error(error));
                    continue;
                }
            };
            tracing::info!(
                session_id = %session.id,
                employee_id = %session.employee_id,
                "idle session suspended by its employee's idle timeout"
            );
            if let Err(error) = self.finish_session_stop_command(stopping).await {
                first_error.get_or_insert(error);
            }
        }
        first_error.map_or(Ok(()), Err)
    }
}
