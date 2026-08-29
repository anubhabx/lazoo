use serde_json::Value;

const REDACTED: &str = "[REDACTED]";

#[must_use]
pub fn redact_json(mut value: Value) -> Value {
    redact_value(&mut value);
    value
}

fn redact_value(value: &mut Value) {
    match value {
        Value::Object(object) => {
            for (key, child) in object {
                if is_secret_key(key) {
                    *child = Value::String(REDACTED.to_owned());
                } else {
                    redact_value(child);
                }
            }
        }
        Value::Array(array) => {
            for child in array {
                redact_value(child);
            }
        }
        Value::Null | Value::Bool(_) | Value::Number(_) | Value::String(_) => {}
    }
}

fn is_secret_key(key: &str) -> bool {
    let normalized = key
        .chars()
        .filter(char::is_ascii_alphanumeric)
        .flat_map(char::to_lowercase)
        .collect::<String>();
    [
        "apikey",
        "authorization",
        "cookie",
        "credential",
        "password",
        "refreshtoken",
        "secret",
        "token",
    ]
    .iter()
    .any(|secret| normalized == *secret || normalized.ends_with(secret))
}
