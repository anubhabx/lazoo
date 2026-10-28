use lazoo_protocol::{SessionLifecycleState, TurnState};
use tokio::time::{Duration, timeout};
use uuid::Uuid;

use super::App;

impl App {
    /// A summary's native context is single-use, including after an invalid model response.
    pub(super) async fn cleanup_memory_session(&self, turn_id: Uuid) -> Result<(), String> {
        self.live_turns.lock().await.remove(&turn_id);
        let mut turn = self
            .store
            .get_turn(turn_id)
            .map_err(|error| error.to_string())?;
        if !matches!(
            turn.state,
            TurnState::Completed
                | TurnState::TerminalFailed
                | TurnState::Cancelled
                | TurnState::Uncertain
        ) {
            turn = self
                .store
                .mark_turn_uncertain(&lazoo_store::MarkTurnUncertainInput {
                    request_id: format!("memory-deadline:{turn_id}"),
                    idempotency_key: format!("memory-deadline:{turn_id}"),
                    turn_id,
                    expected_revision: turn.revision,
                    failure_code: "LZ-MEMORY-TURN-UNCERTAIN".to_owned(),
                    failure_message: "internal summary stopped without final native evidence"
                        .to_owned(),
                })
                .map_err(|error| error.to_string())?
                .turn;
        }
        let Some(session_id) = self
            .store
            .memory_summary_session(turn_id)
            .map_err(|error| error.to_string())?
        else {
            return Ok(());
        };
        let session = self
            .store
            .get_session(session_id)
            .map_err(|error| error.to_string())?;
        if session.owner_boot_id != Some(self.boot_id)
            || !matches!(
                session.state,
                SessionLifecycleState::Starting
                    | SessionLifecycleState::Idle
                    | SessionLifecycleState::Stopping
            )
        {
            return Ok(());
        }
        if turn.state == TurnState::Uncertain {
            // Uncertain turns deliberately prohibit normal reuse/stop. Reap this private native
            // process and retain failed session + uncertain turn evidence for human review.
            if let Some(native) = self.live_sessions.lock().await.remove(&session_id) {
                let _ = timeout(Duration::from_secs(10), native.stop()).await;
            }
            self.record_session_failure(
                session_id,
                session.revision,
                "LZ-MEMORY-SESSION-UNCERTAIN",
                "internal summary session was stopped after an uncertain turn",
            )
            .map_err(|error| error.message)?;
        } else {
            self.stop_session_command(
                session_id,
                session.revision,
                format!("memory-stop:{turn_id}"),
                format!("memory-stop:{turn_id}"),
            )
            .await
            .map_err(|error| error.message)?;
        }
        self.publish_events().map_err(|error| error.message)
    }
}
