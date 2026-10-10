use serde_json::Value;
use thiserror::Error;

const MAX_NAME_CHARS: usize = 80;
const MAX_ROLE_CHARS: usize = 120;
const MAX_INSTRUCTIONS_BYTES: usize = 32 * 1024;
const MAX_NATIVE_SETTINGS_BYTES: usize = 32 * 1024;
const MAX_PROVIDER_SETTING_BYTES: usize = 256;
/// Shortest and longest idle timeouts an employee may set; 0 means never suspend.
pub const MIN_IDLE_TIMEOUT_SECONDS: u32 = 60;
pub const MAX_IDLE_TIMEOUT_SECONDS: u32 = 86_400;

#[derive(Clone, Debug, Eq, Error, PartialEq)]
pub enum EmployeeValidationError {
    #[error("employee name must not be empty")]
    NameEmpty,
    #[error("employee name must contain at most 80 characters")]
    NameTooLong,
    #[error("employee role must contain at most 120 characters")]
    RoleTooLong,
    #[error("employee instructions must contain at most 32768 UTF-8 bytes")]
    InstructionsTooLong,
    #[error("employee model must contain at most 256 UTF-8 bytes")]
    ModelTooLong,
    #[error("employee reasoning effort must contain at most 256 UTF-8 bytes")]
    ReasoningEffortTooLong,
    #[error("provider-native settings must be a JSON object")]
    NativeSettingsNotObject,
    #[error("provider-native settings must contain at most 32768 JSON bytes")]
    NativeSettingsTooLarge,
    #[error("provider-native settings cannot contain credential-like keys")]
    NativeSettingsSecretKey,
    #[error("idle timeout must be 0 (never) or between 60 and 86400 seconds")]
    IdleTimeoutOutOfRange,
}

impl EmployeeValidationError {
    #[must_use]
    pub const fn code(&self) -> &'static str {
        match self {
            Self::NameEmpty => "employee-name-empty",
            Self::NameTooLong => "employee-name-too-long",
            Self::RoleTooLong => "employee-role-too-long",
            Self::InstructionsTooLong => "employee-instructions-too-long",
            Self::ModelTooLong => "employee-model-too-long",
            Self::ReasoningEffortTooLong => "employee-reasoning-effort-too-long",
            Self::NativeSettingsNotObject => "native-settings-not-object",
            Self::NativeSettingsTooLarge => "native-settings-too-large",
            Self::NativeSettingsSecretKey => "native-settings-secret-key",
            Self::IdleTimeoutOutOfRange => "idle-timeout-out-of-range",
        }
    }
}

pub fn validate_employee_name(value: &str) -> Result<String, EmployeeValidationError> {
    let value = value.trim();
    if value.is_empty() {
        return Err(EmployeeValidationError::NameEmpty);
    }
    if value.chars().count() > MAX_NAME_CHARS {
        return Err(EmployeeValidationError::NameTooLong);
    }
    Ok(value.to_owned())
}

pub fn validate_employee_text_fields(
    role: &str,
    instructions: &str,
    model: Option<&str>,
    reasoning_effort: Option<&str>,
) -> Result<(), EmployeeValidationError> {
    if role.chars().count() > MAX_ROLE_CHARS {
        return Err(EmployeeValidationError::RoleTooLong);
    }
    if instructions.len() > MAX_INSTRUCTIONS_BYTES {
        return Err(EmployeeValidationError::InstructionsTooLong);
    }
    if model.is_some_and(|value| value.len() > MAX_PROVIDER_SETTING_BYTES) {
        return Err(EmployeeValidationError::ModelTooLong);
    }
    if reasoning_effort.is_some_and(|value| value.len() > MAX_PROVIDER_SETTING_BYTES) {
        return Err(EmployeeValidationError::ReasoningEffortTooLong);
    }
    Ok(())
}

pub const fn validate_idle_timeout(seconds: u32) -> Result<(), EmployeeValidationError> {
    if seconds == 0 || (seconds >= MIN_IDLE_TIMEOUT_SECONDS && seconds <= MAX_IDLE_TIMEOUT_SECONDS)
    {
        Ok(())
    } else {
        Err(EmployeeValidationError::IdleTimeoutOutOfRange)
    }
}

/// An idle session is due for suspension once it has been quiet for its employee's whole idle
/// timeout. A timeout of 0 never suspends, so a session the user warmed stays up until stopped.
#[must_use]
pub fn idle_session_due(idle_timeout_seconds: u32, quiet_seconds: i64) -> bool {
    idle_timeout_seconds != 0 && quiet_seconds >= i64::from(idle_timeout_seconds)
}

pub fn validate_native_settings(value: &Value) -> Result<(), EmployeeValidationError> {
    if !value.is_object() {
        return Err(EmployeeValidationError::NativeSettingsNotObject);
    }
    if serde_json::to_vec(value).map_or(usize::MAX, |serialized| serialized.len())
        > MAX_NATIVE_SETTINGS_BYTES
    {
        return Err(EmployeeValidationError::NativeSettingsTooLarge);
    }
    reject_secret_keys(value)
}

fn reject_secret_keys(value: &Value) -> Result<(), EmployeeValidationError> {
    match value {
        Value::Object(object) => {
            for (key, child) in object {
                let normalized = key
                    .chars()
                    .filter(char::is_ascii_alphanumeric)
                    .flat_map(char::to_lowercase)
                    .collect::<String>();
                if [
                    "secret",
                    "token",
                    "password",
                    "credential",
                    "authorization",
                    "apikey",
                ]
                .iter()
                .any(|term| normalized.contains(term))
                {
                    return Err(EmployeeValidationError::NativeSettingsSecretKey);
                }
                reject_secret_keys(child)?;
            }
            Ok(())
        }
        Value::Array(values) => {
            for child in values {
                reject_secret_keys(child)?;
            }
            Ok(())
        }
        _ => Ok(()),
    }
}
