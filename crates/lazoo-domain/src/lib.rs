mod approval;
mod conversation;
mod delivery;
mod employee;
mod inbox;
mod incident;
mod message;
mod project;
mod roster;
mod task;
mod workspace;

pub use approval::{
    ApprovalPolicyOutcome, ApprovalValidationError, MAX_APPROVAL_ACTION_BYTES,
    MAX_APPROVAL_DETAIL_BYTES, approval_fingerprint, approval_policy, validate_approval_action,
    validate_approval_detail, validate_approval_list_limit,
};
pub use conversation::{
    ConversationValidationError, MAX_CONVERSATION_COMMAND_KEY_BYTES, MAX_CONVERSATION_TITLE_BYTES,
    MAX_TURN_EVENT_PAYLOAD_BYTES, MAX_TURN_TEXT_BYTES, MAX_TURN_TRANSCRIPT_BYTES,
    validate_activation_request, validate_conversation_command_key,
    validate_conversation_list_limit, validate_conversation_title, validate_envelope_sender,
    validate_turn_event_payload, validate_turn_text, validate_turn_transcript_size,
    validate_turn_transition,
};
pub use delivery::{
    IncidentNotice, MAX_NOTICE_OUTPUT_BYTES, MAX_PEER_MESSAGE_BYTES, PeerMessageNotice,
    TaskResultNotice, notice_turn_may_activate, render_coalesced_notice_text,
    render_incident_notice_text, render_peer_message_text, render_task_result_text,
    validate_peer_message_content,
};
pub use employee::{
    EmployeeValidationError, MAX_IDLE_TIMEOUT_SECONDS, MIN_IDLE_TIMEOUT_SECONDS, idle_session_due,
    validate_employee_name, validate_employee_text_fields, validate_idle_timeout,
    validate_native_settings,
};
pub use inbox::{
    InboxCursor, InboxEmployees, InboxValidationError, MAX_INBOX_LIMIT, MAX_INBOX_TITLE_BYTES,
    approval_inbox_item, inbox_counts, inbox_filter_matches, incident_inbox_item,
    sort_inbox_archive, sort_unresolved_inbox, task_inbox_item, validate_inbox_limit,
};
pub use incident::{
    DESCRIBED_INCIDENT_CODES, INCIDENT_REMEDIATION_WINDOW_SECONDS, INCIDENT_THRESHOLD_OCCURRENCES,
    INCIDENT_THRESHOLD_WINDOW_SECONDS, IncidentFacts, IncidentText, IncidentThreshold,
    IncidentValidationError, MAX_INCIDENT_CODE_BYTES, MAX_INCIDENT_DETAIL_BYTES,
    MAX_INCIDENT_REMEDIES, MAX_INCIDENT_TEXT_BYTES, MAX_INCIDENT_TITLE_BYTES, RemediationVerdict,
    default_incident_threshold, describe_incident, incident_fingerprint, incident_threshold,
    incident_window, remediation_verdict, validate_incident_code, validate_incident_detail,
    validate_incident_list_limit, validate_incident_text, validate_incident_threshold,
    validate_incident_title,
};
pub use message::{
    MessageValidationError, retry_delay_seconds, validate_acknowledgement, validate_max_attempts,
    validate_message_command_key, validate_message_content, validate_message_failure,
};
pub use project::{
    GitRepositoryInspection, ProjectError, inspect_git_repository, normalize_windows_identity,
};
pub use roster::{apply_session_availability, derive_availability};
pub use task::{
    MAX_TASK_DEPTH, MAX_TASK_EVIDENCE_BYTES, TaskValidationError, effective_authority,
    task_transition_allowed, validate_task_depth, validate_task_evidence, validate_task_failure,
    validate_task_objective, validate_task_references,
};
pub use workspace::{
    ConversationFacts, MAX_DERIVED_TITLE_BYTES, MAX_WORKSPACE_LIMIT, UNTITLED_CONVERSATION,
    WorkspaceValidationError, conversation_marker, conversation_title, lead_startability,
    validate_workspace_limit,
};
