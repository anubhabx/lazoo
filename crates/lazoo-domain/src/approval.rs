use lazoo_protocol::{ApprovalKind, AuthorityMode};
use serde_json::Value;
use sha2::{Digest, Sha256};
use thiserror::Error;

pub const MAX_APPROVAL_ACTION_BYTES: usize = 1024;
pub const MAX_APPROVAL_DETAIL_BYTES: usize = 16 * 1024;

#[derive(Clone, Debug, Eq, Error, PartialEq)]
pub enum ApprovalValidationError {
    #[error("approval action must not be empty")]
    ActionEmpty,
    #[error("approval action must contain at most 1024 UTF-8 bytes")]
    ActionTooLong,
    #[error("approval detail must be a JSON object")]
    DetailNotObject,
    #[error("approval detail must contain at most 16384 JSON bytes")]
    DetailTooLarge,
    #[error("approval list limit must be between 1 and 1024")]
    ListLimit,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ApprovalPolicyOutcome {
    AutoApprove,
    RequireUser,
}

/// Authority is policy, not prompt text. The most restrictive applicable rule wins:
/// `ask` always waits for the local user, `self-decision` may act inside its assigned scope
/// but must wait when the provider asks to widen that scope, and `full-autonomy` proceeds.
// ponytail: scope expansion is the only capability axis today; add per-capability rules
// (network, connectors, integration) when a provider reports them distinctly.
#[must_use]
pub const fn approval_policy(
    authority: AuthorityMode,
    scope_expansion: bool,
) -> ApprovalPolicyOutcome {
    match authority {
        AuthorityMode::Ask => ApprovalPolicyOutcome::RequireUser,
        AuthorityMode::SelfDecision if scope_expansion => ApprovalPolicyOutcome::RequireUser,
        AuthorityMode::SelfDecision | AuthorityMode::FullAutonomy => {
            ApprovalPolicyOutcome::AutoApprove
        }
    }
}

pub fn validate_approval_action(value: &str) -> Result<String, ApprovalValidationError> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        Err(ApprovalValidationError::ActionEmpty)
    } else if trimmed.len() > MAX_APPROVAL_ACTION_BYTES {
        Err(ApprovalValidationError::ActionTooLong)
    } else {
        Ok(trimmed.to_owned())
    }
}

pub fn validate_approval_detail(value: &Value) -> Result<(), ApprovalValidationError> {
    if !value.is_object() {
        Err(ApprovalValidationError::DetailNotObject)
    } else if serde_json::to_vec(value).map_or(usize::MAX, |bytes| bytes.len())
        > MAX_APPROVAL_DETAIL_BYTES
    {
        Err(ApprovalValidationError::DetailTooLarge)
    } else {
        Ok(())
    }
}

pub const fn validate_approval_list_limit(limit: u32) -> Result<(), ApprovalValidationError> {
    if limit == 0 || limit > 1024 {
        Err(ApprovalValidationError::ListLimit)
    } else {
        Ok(())
    }
}

/// A stable digest of the governed action. A decision is bound to this fingerprint and is
/// never reused for a different action.
#[must_use]
pub fn approval_fingerprint(kind: ApprovalKind, action: &str, scope_expansion: bool) -> String {
    let kind = match kind {
        ApprovalKind::CommandExecution => "command-execution",
        ApprovalKind::FileChange => "file-change",
    };
    let digest = Sha256::digest(format!("{kind}\n{action}\n{scope_expansion}").as_bytes());
    digest.iter().map(|byte| format!("{byte:02x}")).collect()
}
