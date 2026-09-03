mod conversation;
mod employee;
mod message;
mod project;
mod roster;

pub use conversation::{
    ConversationValidationError, MAX_CONVERSATION_COMMAND_KEY_BYTES, MAX_CONVERSATION_TITLE_BYTES,
    MAX_TURN_EVENT_PAYLOAD_BYTES, MAX_TURN_TEXT_BYTES, MAX_TURN_TRANSCRIPT_BYTES,
    validate_activation_request, validate_conversation_command_key,
    validate_conversation_list_limit, validate_conversation_title, validate_envelope_sender,
    validate_turn_event_payload, validate_turn_text, validate_turn_transcript_size,
    validate_turn_transition,
};
pub use employee::{
    EmployeeValidationError, validate_employee_name, validate_employee_text_fields,
    validate_native_settings,
};
pub use message::{
    MessageValidationError, retry_delay_seconds, validate_acknowledgement, validate_max_attempts,
    validate_message_command_key, validate_message_content, validate_message_failure,
};
pub use project::{
    GitRepositoryInspection, ProjectError, inspect_git_repository, normalize_windows_identity,
};
pub use roster::{apply_session_availability, derive_availability};
