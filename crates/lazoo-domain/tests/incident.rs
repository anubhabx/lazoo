use lazoo_domain::{
    DESCRIBED_INCIDENT_CODES, INCIDENT_THRESHOLD_OCCURRENCES, INCIDENT_THRESHOLD_WINDOW_SECONDS,
    IncidentFacts, IncidentThreshold, IncidentValidationError, MAX_INCIDENT_TITLE_BYTES,
    RemediationVerdict, default_incident_threshold, describe_incident, incident_fingerprint,
    incident_threshold, incident_window, remediation_verdict, validate_incident_code,
    validate_incident_detail, validate_incident_list_limit, validate_incident_text,
    validate_incident_threshold, validate_incident_title,
};
use lazoo_protocol::IncidentEscalationReason;
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
    assert_eq!(incident_window(10, 1, 300), (false, 2));
    assert_eq!(incident_window(299, 2, 300), (false, 3));
    let task = IncidentFacts {
        names_task: true,
        ..IncidentFacts::default()
    };
    let timeout = incident_threshold("LZ-MESSAGE-TIMEOUT", task, None);
    assert!(timeout.crossed(3));
    assert!(!timeout.crossed(2));
    assert_eq!(incident_window(301, 2, 300), (true, 1));
    assert_eq!(
        incident_window(-5, 2, 300),
        (true, 1),
        "clock skew restarts the window"
    );
    assert_eq!(
        incident_window(301, 2, 600),
        (false, 3),
        "a longer window keeps counting"
    );
}

#[test]
fn section_fourteen_two_thresholds_promote_stalls_crashes_and_deaths_at_once() {
    // (code, tied to assigned work, dead-lettered a priority message) -> occurrences that promote
    for (code, names_task, priority_message, threshold) in [
        ("LZ-SESSION-STALLED", true, false, 1),
        ("LZ-SESSION-STALLED", false, false, 1),
        ("LZ-DAEMON-UNAVAILABLE", false, false, 1),
        ("LZ-PROCESS-EXIT", true, false, 1),
        ("LZ-PROCESS-EXIT", false, false, 2),
        ("LZ-MESSAGE-DEADLETTER", true, true, 1),
        ("LZ-MESSAGE-DEADLETTER", false, true, 1),
        (
            "LZ-MESSAGE-DEADLETTER",
            true,
            false,
            INCIDENT_THRESHOLD_OCCURRENCES,
        ),
        (
            "LZ-MESSAGE-TIMEOUT",
            true,
            false,
            INCIDENT_THRESHOLD_OCCURRENCES,
        ),
        // Only a dead letter reads the message's priority.
        (
            "LZ-MESSAGE-TIMEOUT",
            false,
            true,
            INCIDENT_THRESHOLD_OCCURRENCES,
        ),
        (
            "LZ-WORKTREE-CONFLICT",
            false,
            false,
            INCIDENT_THRESHOLD_OCCURRENCES,
        ),
        (
            "LZ-SOMETHING-NEW",
            false,
            false,
            INCIDENT_THRESHOLD_OCCURRENCES,
        ),
    ] {
        let facts = IncidentFacts {
            names_task,
            priority_message,
        };
        let effective = incident_threshold(code, facts, None);
        assert_eq!(effective.occurrences, threshold, "{code} {facts:?}");
        assert_eq!(effective.window_seconds, INCIDENT_THRESHOLD_WINDOW_SECONDS);
        assert!(effective.crossed(threshold));
        assert!(!effective.crossed(threshold - 1));
    }
}

#[test]
fn a_configured_threshold_replaces_the_default_but_never_delays_a_loss() {
    let five_in_ten_minutes = IncidentThreshold {
        occurrences: 5,
        window_seconds: 600,
    };
    let none = IncidentFacts::default();
    // An override replaces counts, including the first-occurrence defaults.
    for code in [
        "LZ-SESSION-STALLED",
        "LZ-DAEMON-UNAVAILABLE",
        "LZ-PROCESS-EXIT",
        "LZ-MESSAGE-TIMEOUT",
        "LZ-MESSAGE-DEADLETTER",
    ] {
        assert_eq!(
            incident_threshold(code, none, Some(five_in_ten_minutes)),
            five_in_ten_minutes,
            "{code}"
        );
    }
    let one = IncidentThreshold {
        occurrences: 1,
        window_seconds: 600,
    };
    assert_eq!(
        incident_threshold("LZ-MESSAGE-TIMEOUT", none, Some(one)),
        one,
        "a threshold may also be lowered"
    );
    // A death under assigned work and a dead-lettered priority message promote at once whatever
    // is configured; the configured window still applies.
    for (code, facts) in [
        (
            "LZ-PROCESS-EXIT",
            IncidentFacts {
                names_task: true,
                priority_message: false,
            },
        ),
        (
            "LZ-MESSAGE-DEADLETTER",
            IncidentFacts {
                names_task: false,
                priority_message: true,
            },
        ),
    ] {
        assert_eq!(
            incident_threshold(code, facts, Some(five_in_ten_minutes)),
            one,
            "{code}"
        );
    }
    assert_eq!(
        default_incident_threshold("LZ-PROCESS-EXIT"),
        IncidentThreshold {
            occurrences: 2,
            window_seconds: 300,
        }
    );

    assert_eq!(validate_incident_threshold(1, 60), Ok(one_minute(1)));
    assert_eq!(
        validate_incident_threshold(100, 86_400)
            .unwrap()
            .occurrences,
        100
    );
    for (occurrences, window_seconds) in [(0, 300), (101, 300), (3, 59), (3, 86_401)] {
        assert_eq!(
            validate_incident_threshold(occurrences, window_seconds),
            Err(IncidentValidationError::ThresholdRange),
            "{occurrences} in {window_seconds}"
        );
    }
    // Every code `incident threshold list` shows has words of its own.
    for code in DESCRIBED_INCIDENT_CODES {
        validate_incident_code(code).unwrap();
        assert!(
            !describe_incident(code)
                .title
                .starts_with("Operational failure"),
            "{code}"
        );
    }
}

const fn one_minute(occurrences: u64) -> IncidentThreshold {
    IncidentThreshold {
        occurrences,
        window_seconds: 60,
    }
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

#[test]
fn remediation_escalates_unless_a_remedy_holds_through_the_window() {
    use IncidentEscalationReason::{NoRemedy, RecurredAfterRemedy, RemediesExhausted};
    use RemediationVerdict::{EscalateToUser, Open, Remediated};
    // (window closed, remedies applied, recurred since the last remedy) -> verdict
    for (closed, remedies, recurred, verdict) in [
        (false, 0, false, Open),
        (false, 1, false, Open),
        (false, 1, true, Open),
        (false, 2, false, Open),
        (false, 2, true, EscalateToUser(RemediesExhausted)),
        (true, 0, false, EscalateToUser(NoRemedy)),
        (true, 1, false, Remediated),
        (true, 2, false, Remediated),
        (true, 1, true, EscalateToUser(RecurredAfterRemedy)),
        (true, 2, true, EscalateToUser(RemediesExhausted)),
    ] {
        assert_eq!(
            remediation_verdict(closed, remedies, recurred),
            verdict,
            "closed={closed} remedies={remedies} recurred={recurred}"
        );
    }
}
