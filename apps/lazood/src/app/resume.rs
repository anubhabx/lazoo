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

use lazoo_protocol::{EmployeeRecord, EmployeeSessionRecord, SessionLifecycleState};
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
        let _ = match decision {
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
        if !self.store.session_has_completed_turn(prior_session_id)? {
            // A native session that never completed a turn has no durable transcript, and a
            // resume that the provider rejected leaves exactly such a session behind: skipping
            // it here is what keeps one bad resume from repeating on every wake.
            return skipped("no-completed-turn");
        }
        if self.session_has_live_attachment(prior_session_id).await {
            return skipped("owner-terminal-attached");
        }
        // A session that was itself resumed ran in its predecessor's directory, not its own.
        let working_directory = self
            .store
            .resumed_working_directory(prior_session_id)?
            .map_or_else(
                || self.root.session_working_dir(prior_session_id),
                PathBuf::from,
            );
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
