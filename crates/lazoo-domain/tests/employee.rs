use lazoo_domain::{
    validate_employee_name, validate_employee_text_fields, validate_native_settings,
};
use serde_json::json;

#[test]
fn employee_name_is_trimmed_bounded_and_nonempty() {
    assert_eq!(validate_employee_name("  Rhea  ").unwrap(), "Rhea");
    assert_eq!(
        validate_employee_name("   ").unwrap_err().code(),
        "employee-name-empty"
    );
    assert_eq!(
        validate_employee_name(&"x".repeat(81)).unwrap_err().code(),
        "employee-name-too-long"
    );
}

#[test]
fn employee_text_fields_are_bounded() {
    validate_employee_text_fields("Lead", "Coordinate work", Some("model"), Some("high")).unwrap();
    assert_eq!(
        validate_employee_text_fields(&"r".repeat(121), "", None, None)
            .unwrap_err()
            .code(),
        "employee-role-too-long"
    );
    assert_eq!(
        validate_employee_text_fields("", &"i".repeat(32 * 1024 + 1), None, None)
            .unwrap_err()
            .code(),
        "employee-instructions-too-long"
    );
}

#[test]
fn native_settings_require_a_bounded_object_without_secret_keys() {
    assert_eq!(
        validate_native_settings(&json!(["not", "an", "object"]))
            .unwrap_err()
            .code(),
        "native-settings-not-object"
    );
    assert_eq!(
        validate_native_settings(&json!({"nested": {"api_key": "secret"}}))
            .unwrap_err()
            .code(),
        "native-settings-secret-key"
    );
    assert_eq!(
        validate_native_settings(&json!({"Authorization": "Bearer secret"}))
            .unwrap_err()
            .code(),
        "native-settings-secret-key"
    );
    validate_native_settings(&json!({"sandboxMode": "workspace-write"})).unwrap();
}
