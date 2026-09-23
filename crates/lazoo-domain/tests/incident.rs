use lazoo_domain::{
    INCIDENT_THRESHOLD_OCCURRENCES, INCIDENT_THRESHOLD_WINDOW_SECONDS, IncidentValidationError,
    MAX_INCIDENT_TITLE_BYTES, describe_incident, incident_fingerprint, incident_threshold_crossed,
    incident_window, validate_incident_code, validate_incident_detail,
    validate_incident_list_limit, validate_incident_text, validate_incident_title,
};
use serde_json::json;
use uuid::Uuid;

#[test]
fn known_codes_describe_the_situation_and_the_next_safe_action() {
    let stalled = describe_incident("LZ-SESSION-STALLED");
    assert_eq!(
        stalled.title,
        "Session accepted work but produced no progress"
    );
    assert!(stalled.explanation.contains("no progress"));
    assert!(stalled.next_action.contains("never retries"));
    for code in [
        "LZ-PROVIDER-AUTH",
        "LZ-PROVIDER-INCOMPATIBLE",
        "LZ-PROCESS-EXIT",
        "LZ-MESSAGE-TIMEOUT",
        "LZ-MESSAGE-DEADLETTER",
        "LZ-WORKTREE-CONFLICT",
        "LZ-CONFIG-INCOMPATIBLE",
        "LZ-DAEMON-UNAVAILABLE",
        "LZ-SESSION-START-FAILED",
        "LZ-SESSION-OWNER-LOST",
        "LZ-TASK-DISPATCH-OWNER-LOST",
        "LZ-EMPLOYEE-REPORTED",
    ] {
        let text = describe_incident(code);
        assert!(!text.title.starts_with("Operational failure"), "{code}");
        assert!(!text.explanation.is_empty() && !text.next_action.is_empty());
    }
    let unknown = describe_incident("LZ-SOMETHING-NEW");
    assert_eq!(unknown.title, "Operational failure LZ-SOMETHING-NEW");
    assert!(unknown.next_action.contains("Inspect"));
}

#[test]
fn threshold_window_promotes_three_equivalent_failures_inside_five_minutes() {
    assert_eq!(INCIDENT_THRESHOLD_OCCURRENCES, 3);
    assert_eq!(INCIDENT_THRESHOLD_WINDOW_SECONDS, 300);
    assert_eq!(incident_window(10, 1), (false, 2));
    assert_eq!(incident_window(299, 2), (false, 3));
    assert!(incident_threshold_crossed(3));
    assert!(!incident_threshold_crossed(2));
    assert_eq!(incident_window(301, 2), (true, 1));
    assert_eq!(
        incident_window(-5, 2),
        (true, 1),
        "clock skew restarts the window"
    );
}

#[test]
fn fingerprints_fold_equivalent_failures_and_separate_scopes() {
    let employee = Uuid::new_v4();
    let project = Uuid::new_v4();
    let same = incident_fingerprint("LZ-PROCESS-EXIT", Some(employee), Some(project));
    assert_eq!(
        same,
        incident_fingerprint("LZ-PROCESS-EXIT", Some(employee), Some(project))
    );
    assert_eq!(same.len(), 64);
    assert_ne!(
        same,
        incident_fingerprint("LZ-PROCESS-EXIT", Some(employee), None)
    );
    assert_ne!(
        same,
        incident_fingerprint("LZ-SESSION-STALLED", Some(employee), Some(project))
    );
}

#[test]
fn validation_bounds_codes_titles_text_detail_and_limits() {
    assert!(validate_incident_code("LZ-PROCESS-EXIT").is_ok());
    assert_eq!(
        validate_incident_code("process-exit"),
        Err(IncidentValidationError::CodeInvalid)
    );
    assert_eq!(
        validate_incident_code("LZ-"),
        Err(IncidentValidationError::CodeInvalid)
    );
    assert_eq!(
        validate_incident_code(&format!("LZ-{}", "A".repeat(80))),
        Err(IncidentValidationError::CodeInvalid)
    );
    assert_eq!(
        validate_incident_title("  Disk full  ").unwrap(),
        "Disk full"
    );
    assert_eq!(
        validate_incident_title("   "),
        Err(IncidentValidationError::TitleEmpty)
    );
    assert_eq!(
        validate_incident_title(&"x".repeat(MAX_INCIDENT_TITLE_BYTES + 1)),
        Err(IncidentValidationError::TitleTooLong)
    );
    assert_eq!(validate_incident_text("").unwrap(), "");
    assert_eq!(
        validate_incident_text(&"x".repeat(3000)),
        Err(IncidentValidationError::TextTooLong)
    );
    assert!(validate_incident_detail(&json!({"failureCode": "LZ-X"})).is_ok());
    assert_eq!(
        validate_incident_detail(&json!([1])),
        Err(IncidentValidationError::DetailNotObject)
    );
    assert_eq!(
        validate_incident_detail(&json!({"blob": "x".repeat(17 * 1024)})),
        Err(IncidentValidationError::DetailTooLarge)
    );
    assert!(validate_incident_list_limit(1).is_ok());
    assert_eq!(
        validate_incident_list_limit(0),
        Err(IncidentValidationError::ListLimit)
    );
    assert_eq!(
        validate_incident_list_limit(1025),
        Err(IncidentValidationError::ListLimit)
    );
}
