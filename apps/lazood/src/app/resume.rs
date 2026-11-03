//! Session resume on wake: when an employee is activated and its latest session at the same
//! configuration revision is `suspended`, the new session continues that session's native
//! conversation instead of starting an empty one (brief 9.1: "resumable identifiers retained
//! where supported"). Every other case starts fresh, and a suspended predecessor that was not
//! continued is audited as `session.resume.skipped` with the reason.
//!
//! The decision is made once, in `App::start_new_session`, so every activation path (user
//! turn, notice turn, `session.warm`) shares it. Task dispatch passes its own workspace and
//! never resumes. A configuration change never resumes: the predecessor lookup is scoped to the
//! employee's current configuration revision.

use std::path::PathBuf;

use lazoo_protocol::{EmployeeRecord, EmployeeSessionRecord, SessionLifecycleState, TurnState};
use lazoo_providers::{ProviderKind, attach_args};
use lazoo_store::SessionResumeOutcome;
use uuid::Uuid;

use super::App;

pub(super) enum ResumeDecision {
    /// No suspended predecessor, so there was nothing to continue.
    Fresh,
    /// A suspended predecessor existed and is not continued; `reason` is audited.
    Skipped {
        prior_session_id: Uuid,
        reason: &'static str,
    },
    Resume {
        prior_session_id: Uuid,
        native_session_id: String,
        working_directory: PathBuf,
    },
}

impl ResumeDecision {
    pub(super) fn working_directory(&self) -> Option<PathBuf> {
        match self {
            Self::Resume {
                working_directory, ..
            } => Some(working_directory.clone()),
            Self::Fresh | Self::Skipped { .. } => None,
        }
    }

    pub(super) fn native_session_id(&self) -> Option<&str> {
        match self {
            Self::Resume {
                native_session_id, ..
            } => Some(native_session_id),
            Self::Fresh | Self::Skipped { .. } => None,
        }
    }
}

impl App {
    /// A process that exited while resuming may have lost its transcript. One fresh attempt
    /// gets a new durable session row; all other failures and fresh-start failures return as-is.
    pub(super) async fn retry_failed_resume(
        &self,
        employee: EmployeeRecord,
        provider: ProviderKind,
        failed_session_id: Uuid,
        prior_session_id: Uuid,
    ) -> Result<lazoo_protocol::SessionMutationResult, lazoo_protocol::RpcError> {
        let key = format!("resume-fallback:{failed_session_id}");
        let fresh = self
            .store
            .begin_session(&lazoo_store::BeginSessionInput {
                request_id: key.clone(),
                idempotency_key: key,
                employee_id: employee.id,
                expected_employee_revision: employee.configuration_revision,
                owner_boot_id: self.boot_id,
            })
            .map_err(super::session_store_error)?;
        self.store
            .record_session_resume_outcome(
                failed_session_id,
                prior_session_id,
                SessionResumeOutcome::Failed {
                    fresh_session_id: fresh.session.id,
                },
            )
            .map_err(super::session_store_error)?;
        self.publish_events()?;
        let directory = self.root.session_working_dir(fresh.session.id);
        // Passing an explicit workspace prevents another resume decision, even if startup fails.
        if std::fs::create_dir_all(&directory).is_err() {
            self.record_session_failure(
                fresh.session.id,
                fresh.session.revision,
                "LZ-SESSION-START-FAILED",
                "fresh session directory could not be prepared",
            )?;
            return Err(super::session_failure(
                "LZ-SESSION-START-FAILED",
                "fresh session directory could not be prepared",
                fresh.session.id,
            ));
        }
        Box::pin(self.start_new_session(employee, provider, fresh, Some(directory))).await
    }

    /// Decides whether the starting session `session_id` continues its employee's suspended
    /// predecessor. A store failure while deciding falls back to a fresh session: resume is an
    /// optimisation and must never stop an employee from waking.
    pub(super) async fn decide_session_resume(
        &self,
        employee: &EmployeeRecord,
        provider: ProviderKind,
        session_id: Uuid,
    ) -> ResumeDecision {
        self.try_decide_session_resume(employee, provider, session_id)
            .await
            .unwrap_or(ResumeDecision::Fresh)
    }

    /// Audits the decision once the new session is ready, so a start that fails never leaves a
    /// `session.resumed` behind. The session is already live: losing the audit event must not
    /// fail it.
    pub(super) fn record_resume_decision(
        &self,
        session_id: Uuid,
        decision: &ResumeDecision,
        ready: &EmployeeSessionRecord,
    ) {
        let recorded = match decision {
            ResumeDecision::Fresh => return,
            ResumeDecision::Skipped {
                prior_session_id,
                reason,
            } => self.store.record_session_resume_outcome(
                session_id,
                *prior_session_id,
                SessionResumeOutcome::Skipped { reason },
            ),
            ResumeDecision::Resume {
                prior_session_id,
                working_directory,
                ..
            } => self.store.record_session_resume_outcome(
                session_id,
                *prior_session_id,
                SessionResumeOutcome::Resumed {
                    native_session_id: ready.native_session_id.as_deref().unwrap_or_default(),
                    working_directory: &working_directory.to_string_lossy(),
                },
            ),
        };
        if let Err(error) = recorded {
            tracing::warn!(%session_id, %error, "session resume decision could not be audited");
        }
    }

    /// The directory the native conversation of `session_id` lives in: where a resumed session
    /// ran (its predecessor directory), otherwise the session own directory.
    pub(super) fn session_directory(
        &self,
        session_id: Uuid,
    ) -> Result<PathBuf, lazoo_store::StoreError> {
        Ok(self
            .store
            .resumed_working_directory(session_id)?
            .map_or_else(|| self.root.session_working_dir(session_id), PathBuf::from))
    }

    /// A resumed session whose first conversation turn ended failed or uncertain is stopped at
    /// once, so it ends `suspended` with no completed turn and the next wake starts fresh. A
    /// rejected resume (the provider no longer has the transcript) would otherwise leave an idle
    /// session that fails every turn, and with an idle timeout of 0 nothing would ever end it.
    pub(super) async fn stop_resumed_session_after_failed_first_turn(
        &self,
        turn_id: Uuid,
        session_id: Uuid,
    ) {
        let failed = self.store.get_turn(turn_id).is_ok_and(|current| {
            matches!(
                current.state,
                TurnState::RetryableFailed | TurnState::TerminalFailed | TurnState::Uncertain
            )
        });
        if !failed {
            return;
        }
        let proven = self
            .store
            .session_turn_evidence(session_id)
            .map_or(true, |evidence| evidence.completed);
        let resumed = self
            .store
            .resumed_working_directory(session_id)
            .is_ok_and(|directory| directory.is_some());
        if proven || !resumed {
            return;
        }
        let Ok(session) = self.store.get_session(session_id) else {
            return;
        };
        if session.state != SessionLifecycleState::Idle {
            return;
        }
        let key = format!("resume-failed-first-turn:{session_id}");
        if let Err(error) =
            Box::pin(self.stop_session_command(session_id, session.revision, key.clone(), key))
                .await
        {
            tracing::warn!(%session_id, code = %error.code, "failed resumed session could not be stopped");
        }
    }

    async fn try_decide_session_resume(
        &self,
        employee: &EmployeeRecord,
        provider: ProviderKind,
        session_id: Uuid,
    ) -> Result<ResumeDecision, lazoo_store::StoreError> {
        let Some(prior) = self.store.previous_session_for_employee(
            employee.id,
            employee.configuration_revision,
            session_id,
        )?
        else {
            return Ok(ResumeDecision::Fresh);
        };
        if prior.state != SessionLifecycleState::Suspended {
            // A failed predecessor is never resumed.
            return Ok(ResumeDecision::Fresh);
        }
        let prior_session_id = prior.id;
        let skipped = |reason| {
            Ok(ResumeDecision::Skipped {
                prior_session_id,
                reason,
            })
        };
        if self
            .store
            .task_workspace_for_session(prior_session_id)?
            .is_some()
        {
            return skipped("task-session");
        }
        if self.store.is_memory_summary_session(session_id)?
            || self.store.is_memory_summary_session(prior_session_id)?
        {
            return skipped("memory-reserved");
        }
        let Some(native_session_id) = prior
            .native_session_id
            .filter(|id| attach_args(provider, id).is_some())
        else {
            return skipped("unsupported-provider");
        };
        if provider == ProviderKind::Codex {
            // Lazoo declares its collaboration tools to every Codex session, and Codex
            // `thread/resume` has no field to carry them: the adapter fails closed.
            return skipped("codex-dynamic-tools");
        }
        // A native session that never completed a turn has no durable transcript, and a resume
        // the provider rejected leaves exactly such a session behind: skipping it keeps one bad
        // resume from repeating on every wake. A session that was itself resumed and has not
        // failed a turn (a `session.warm`, or a notice whose delivery failed) still carries the
        // native id of the conversation it continued, so it qualifies too; otherwise it would
        // orphan the transcript of the session before it.
        let evidence = self.store.session_turn_evidence(prior_session_id)?;
        let carries_resumed_transcript = !evidence.failed
            && self
                .store
                .resumed_working_directory(prior_session_id)?
                .is_some();
        if !evidence.completed && !carries_resumed_transcript {
            return skipped("no-completed-turn");
        }
        if self.session_has_live_attachment(prior_session_id).await {
            return skipped("owner-terminal-attached");
        }
        // A session that was itself resumed ran in its predecessor's directory, not its own.
        let working_directory = self.session_directory(prior_session_id)?;
        if !working_directory.is_dir() {
            return skipped("working-directory-missing");
        }
        Ok(ResumeDecision::Resume {
            prior_session_id,
            native_session_id,
            working_directory,
        })
    }
}
