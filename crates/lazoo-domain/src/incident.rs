use std::fmt::Write as _;

use lazoo_protocol::IncidentEscalationReason;
use serde_json::Value;
use sha2::{Digest, Sha256};
use thiserror::Error;
use uuid::Uuid;

/// Equivalent failures inside one window promote an incident to `priority`.
pub const INCIDENT_THRESHOLD_OCCURRENCES: u64 = 3;
pub const INCIDENT_THRESHOLD_WINDOW_SECONDS: i64 = 5 * 60;
pub const MAX_INCIDENT_CODE_BYTES: usize = 64;
pub const MAX_INCIDENT_TITLE_BYTES: usize = 256;
pub const MAX_INCIDENT_TEXT_BYTES: usize = 2048;
pub const MAX_INCIDENT_DETAIL_BYTES: usize = 16 * 1024;

#[derive(Clone, Debug, Eq, Error, PartialEq)]
pub enum IncidentValidationError {
    #[error("incident code must match LZ-[A-Z0-9-]+ and contain at most 64 bytes")]
    CodeInvalid,
    #[error("incident title must not be empty")]
    TitleEmpty,
    #[error("incident title must contain at most 256 UTF-8 bytes")]
    TitleTooLong,
    #[error("incident text must contain at most 2048 UTF-8 bytes")]
    TextTooLong,
    #[error("incident detail must be a JSON object")]
    DetailNotObject,
    #[error("incident detail must contain at most 16384 JSON bytes")]
    DetailTooLarge,
    #[error("incident list limit must be between 1 and 1024")]
    ListLimit,
}

/// Human-readable text for one incident code. Employees and operators read this before any raw
/// diagnostic, so it names the situation, what Lazoo already did, and the next safe action.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct IncidentText {
    pub title: String,
    pub explanation: String,
    pub next_action: String,
}

#[must_use]
pub fn describe_incident(code: &str) -> IncidentText {
    let (title, explanation, next_action) = match code {
        "LZ-PROVIDER-AUTH" => (
            "Provider authentication is unavailable",
            "The provider rejected the session because its credentials or entitlement are \
             missing or expired.",
            "Sign in to the provider natively, then refresh the roster.",
        ),
        "LZ-PROVIDER-INCOMPATIBLE" => (
            "Provider version or protocol is unsupported",
            "The installed provider answered with a protocol Lazoo could not interpret.",
            "Check the installed provider version against the supported range.",
        ),
        "LZ-PROCESS-EXIT" => (
            "Employee process exited unexpectedly",
            "The native provider process ended while Lazoo still owned work in it.",
            "Inspect the provider's own logs; Lazoo did not restart it or retry the work.",
        ),
        "LZ-SESSION-STALLED" => (
            "Session accepted work but produced no progress",
            "The provider accepted the turn and then emitted no progress or completion \
             evidence before the deadline.",
            "Retry the turn explicitly or stop the session; Lazoo never retries accepted work \
             automatically.",
        ),
        "LZ-MESSAGE-TIMEOUT" => (
            "Delivery deadline expired",
            "A delivery or acknowledgement deadline passed before the provider answered.",
            "Retry the message or turn explicitly if the work is still wanted.",
        ),
        "LZ-MESSAGE-DEADLETTER" => (
            "Message exhausted safe delivery attempts",
            "The bus stopped retrying after the configured attempts and parked the message as \
             a dead letter.",
            "Review the dead letter, then resend it explicitly or cancel it.",
        ),
        "LZ-WORKTREE-CONFLICT" => (
            "Task workspace could not be prepared",
            "The isolated Git worktree or projectless workspace for a task could not be created.",
            "Check the repository state and disk space, then re-delegate the task.",
        ),
        "LZ-CONFIG-INCOMPATIBLE" => (
            "Configuration changed before dispatch",
            "The employee, project, or provider configuration no longer matches what the task \
             was accepted against.",
            "Review the current configuration and re-delegate the task.",
        ),
        "LZ-DAEMON-UNAVAILABLE" => (
            "Daemon was unavailable",
            "The keeper recorded a daemon exit through the emergency spool while work may \
             have been in flight.",
            "Check the keeper restart budget and the records recovered after restart.",
        ),
        "LZ-SESSION-START-FAILED" => (
            "Employee session failed to start",
            "The provider session did not become ready in time or did not report an owned \
             process.",
            "Probe the provider and check its installation before warming the employee again.",
        ),
        "LZ-SESSION-OWNER-LOST"
        | "LZ-MESSAGE-OWNER-LOST"
        | "LZ-TURN-OWNER-LOST"
        | "LZ-TASK-DISPATCH-OWNER-LOST" => (
            "In-flight work lost its daemon",
            "The daemon that owned this work stopped before it finished; the record was closed \
             or marked uncertain on restart and was not retried.",
            "Review the uncertain records and retry only the work that is still wanted.",
        ),
        "LZ-TURN-PROVIDER" => (
            "Provider rejected a turn",
            "The provider refused the turn before accepting it.",
            "Read the turn failure detail, then retry or resubmit the turn explicitly.",
        ),
        "LZ-EMPLOYEE-REPORTED" => (
            "Employee reported an operational problem",
            "An employee recorded a problem it could not resolve inside its own task.",
            "Review the report and decide whether to intervene or re-delegate.",
        ),
        _ => (
            "Operational failure",
            "Lazoo recorded a failure it has no specific description for.",
            "Inspect the linked records before retrying anything.",
        ),
    };
    IncidentText {
        title: if title == "Operational failure" {
            format!("Operational failure {code}")
        } else {
            title.to_owned()
        },
        explanation: explanation.to_owned(),
        next_action: next_action.to_owned(),
    }
}

pub fn validate_incident_code(code: &str) -> Result<(), IncidentValidationError> {
    let body = code.strip_prefix("LZ-").unwrap_or_default();
    if body.is_empty()
        || code.len() > MAX_INCIDENT_CODE_BYTES
        || !body
            .bytes()
            .all(|byte| byte.is_ascii_uppercase() || byte.is_ascii_digit() || byte == b'-')
    {
        return Err(IncidentValidationError::CodeInvalid);
    }
    Ok(())
}

pub fn validate_incident_title(title: &str) -> Result<String, IncidentValidationError> {
    let trimmed = title.trim();
    if trimmed.is_empty() {
        Err(IncidentValidationError::TitleEmpty)
    } else if trimmed.len() > MAX_INCIDENT_TITLE_BYTES {
        Err(IncidentValidationError::TitleTooLong)
    } else {
        Ok(trimmed.to_owned())
    }
}

pub fn validate_incident_text(text: &str) -> Result<String, IncidentValidationError> {
    let trimmed = text.trim();
    if trimmed.len() > MAX_INCIDENT_TEXT_BYTES {
        Err(IncidentValidationError::TextTooLong)
    } else {
        Ok(trimmed.to_owned())
    }
}

pub fn validate_incident_detail(value: &Value) -> Result<(), IncidentValidationError> {
    if !value.is_object() {
        Err(IncidentValidationError::DetailNotObject)
    } else if serde_json::to_vec(value).map_or(usize::MAX, |bytes| bytes.len())
        > MAX_INCIDENT_DETAIL_BYTES
    {
        Err(IncidentValidationError::DetailTooLarge)
    } else {
        Ok(())
    }
}

pub const fn validate_incident_list_limit(limit: u32) -> Result<(), IncidentValidationError> {
    if limit == 0 || limit > 1024 {
        Err(IncidentValidationError::ListLimit)
    } else {
        Ok(())
    }
}

/// Equivalent failures share one incident: same code, same employee, same project.
#[must_use]
pub fn incident_fingerprint(
    code: &str,
    employee_id: Option<Uuid>,
    project_id: Option<Uuid>,
) -> String {
    let employee = employee_id.map(|id| id.to_string()).unwrap_or_default();
    let project = project_id.map(|id| id.to_string()).unwrap_or_default();
    let digest = Sha256::digest(format!("{code}\n{employee}\n{project}").as_bytes());
    let mut hex = String::with_capacity(digest.len() * 2);
    for byte in digest {
        let _ = write!(hex, "{byte:02x}");
    }
    hex
}

/// Advances the threshold window for one more occurrence. Returns whether the window restarted
/// and the occurrence count inside the current window.
#[must_use]
pub const fn incident_window(elapsed_seconds: i64, window_occurrences: u64) -> (bool, u64) {
    if elapsed_seconds < 0 || elapsed_seconds > INCIDENT_THRESHOLD_WINDOW_SECONDS {
        (true, 1)
    } else {
        (false, window_occurrences + 1)
    }
}

#[must_use]
pub const fn incident_threshold_crossed(window_occurrences: u64) -> bool {
    window_occurrences >= INCIDENT_THRESHOLD_OCCURRENCES
}

/// A lead may apply at most this many remedies to one promoted incident...
pub const MAX_INCIDENT_REMEDIES: u32 = 2;
/// ...and only within this long after the promotion.
pub const INCIDENT_REMEDIATION_WINDOW_SECONDS: i64 = 5 * 60;

/// What becomes of a promoted incident in its lead's remediation window.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RemediationVerdict {
    /// The lead may still act; nothing changes yet.
    Open,
    /// The window closed and the failure has not recurred since the lead's last remedy.
    Remediated,
    /// Unresolved: the incident becomes a priority notification for the user.
    EscalateToUser(IncidentEscalationReason),
}

/// The whole bounded-remediation rule. `remedies` is how many the lead applied, and `recurred`
/// whether the failure occurred again after the last of them (false while there are none).
/// A remediated incident that recurs is judged with a closed window.
#[must_use]
pub const fn remediation_verdict(
    window_closed: bool,
    remedies: u32,
    recurred: bool,
) -> RemediationVerdict {
    if recurred && remedies >= MAX_INCIDENT_REMEDIES {
        RemediationVerdict::EscalateToUser(IncidentEscalationReason::RemediesExhausted)
    } else if !window_closed {
        RemediationVerdict::Open
    } else if remedies == 0 {
        RemediationVerdict::EscalateToUser(IncidentEscalationReason::NoRemedy)
    } else if recurred {
        RemediationVerdict::EscalateToUser(IncidentEscalationReason::RecurredAfterRemedy)
    } else {
        RemediationVerdict::Remediated
    }
}
