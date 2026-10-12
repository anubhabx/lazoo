use lazoo_domain::{
    ConversationFacts, UNTITLED_CONVERSATION, WorkspaceValidationError, conversation_marker,
    conversation_title, lead_startability, validate_workspace_limit,
};
use lazoo_protocol::{
    ActivationPolicy, AuthorityMode, ConversationMarker, EmployeeAvailability,
    EmployeeCheckoutPolicy, EmployeeRecord, RosterEntry, TaskState, TurnState, WorkerCounts,
};
use serde_json::json;
use uuid::Uuid;

fn entry(availability: EmployeeAvailability, reason: Option<&str>) -> RosterEntry {
    RosterEntry {
        employee: EmployeeRecord {
            id: Uuid::from_u128(1),
            name: "Rhea".to_owned(),
            provider: "codex".to_owned(),
            role: "Lead".to_owned(),
            instructions: String::new(),
            model: None,
            reasoning_effort: None,
            native_settings: json!({}),
            authority_mode: AuthorityMode::Ask,
            checkout_policy: EmployeeCheckoutPolicy::Isolated,
            activation_policy: ActivationPolicy::Manual,
            idle_timeout_seconds: 0,
            enabled: availability != EmployeeAvailability::Disabled,
            configuration_revision: 1,
            created_at: "2026-10-13T00:00:00Z".to_owned(),
            updated_at: "2026-10-13T00:00:00Z".to_owned(),
            disabled_at: None,
        },
        availability,
        availability_reason: reason.map(str::to_owned),
        provider_observation: None,
        session: None,
    }
}

#[test]
fn the_marker_prefers_attention_then_activity_then_failure_then_completion() {
    let quiet = ConversationFacts::default();
    assert_eq!(conversation_marker(&quiet), (None, None));

    let mut facts = ConversationFacts {
        unresolved: 2,
        active_turns: 1,
        workers: WorkerCounts {
            total: 3,
            running: 1,
            waiting: 1,
            failed: 1,
            completed: 0,
        },
        root_task_state: Some(TaskState::Failed),
        last_turn_state: Some(TurnState::TerminalFailed),
    };
    assert_eq!(
        conversation_marker(&facts),
        (
            Some(ConversationMarker::NeedsAttention),
            Some("2 items need you".to_owned())
        )
    );
    facts.unresolved = 1;
    assert_eq!(
        conversation_marker(&facts).1.as_deref(),
        Some("1 item needs you")
    );
    facts.unresolved = 0;
    assert_eq!(
        conversation_marker(&facts),
        (
            Some(ConversationMarker::Active),
            Some("1 worker running".to_owned())
        )
    );
    facts.workers.running = 0;
    assert_eq!(
        conversation_marker(&facts),
        (
            Some(ConversationMarker::Active),
            Some("a turn is in flight".to_owned())
        )
    );
    facts.active_turns = 0;
    assert_eq!(
        conversation_marker(&facts),
        (
            Some(ConversationMarker::Active),
            Some("1 waiting to run".to_owned())
        )
    );
    facts.workers.waiting = 0;
    assert_eq!(
        conversation_marker(&facts),
        (
            Some(ConversationMarker::Failed),
            Some("the plan failed".to_owned())
        )
    );
    facts.root_task_state = Some(TaskState::Completed);
    assert_eq!(
        conversation_marker(&facts),
        (
            Some(ConversationMarker::Failed),
            Some("the last turn failed".to_owned())
        )
    );
    facts.last_turn_state = Some(TurnState::Completed);
    assert_eq!(
        conversation_marker(&facts),
        (
            Some(ConversationMarker::Completed),
            Some("the plan completed".to_owned())
        )
    );
    facts.root_task_state = Some(TaskState::Cancelled);
    assert_eq!(conversation_marker(&facts), (None, None));
}

#[test]
fn titles_come_from_the_store_then_the_first_request_then_a_default() {
    assert_eq!(
        conversation_title(Some("  Parser fix  "), Some("ignored")),
        ("Parser fix".to_owned(), false)
    );
    assert_eq!(
        conversation_title(Some("   "), Some("\n  Fix the parser\nand more")),
        ("Fix the parser".to_owned(), true)
    );
    let long = "x".repeat(200);
    let (title, derived) = conversation_title(None, Some(&long));
    assert!(derived);
    assert_eq!(title.len(), 80);
    assert_eq!(
        conversation_title(None, None),
        (UNTITLED_CONVERSATION.to_owned(), true)
    );
    assert_eq!(
        conversation_title(None, Some("   \n  ")),
        (UNTITLED_CONVERSATION.to_owned(), true)
    );
}

#[test]
fn startability_follows_roster_availability_with_lazoo_words_for_each_block() {
    for availability in [
        EmployeeAvailability::Dormant,
        EmployeeAvailability::Idle,
        EmployeeAvailability::Suspended,
    ] {
        assert_eq!(lead_startability(&entry(availability, None)), (true, None));
    }
    assert_eq!(
        lead_startability(&entry(
            EmployeeAvailability::Disabled,
            Some("employee is disabled")
        )),
        (false, Some("Rhea is disabled".to_owned()))
    );
    assert_eq!(
        lead_startability(&entry(EmployeeAvailability::Starting, None)),
        (false, Some("Rhea is starting".to_owned()))
    );
    assert_eq!(
        lead_startability(&entry(
            EmployeeAvailability::Offline,
            Some("provider executable was not detected")
        )),
        (
            false,
            Some("Rhea's harness is offline: provider executable was not detected".to_owned())
        )
    );
    assert_eq!(
        lead_startability(&entry(EmployeeAvailability::Unverified, None)),
        (
            false,
            Some("Rhea's harness is unverified: not authenticated or compatible".to_owned())
        )
    );
    assert_eq!(
        lead_startability(&entry(
            EmployeeAvailability::Failed,
            Some("LZ-SESSION-OWNER-LOST")
        )),
        (
            false,
            Some("Rhea's last session failed: LZ-SESSION-OWNER-LOST".to_owned())
        )
    );
    assert_eq!(
        validate_workspace_limit(0),
        Err(WorkspaceValidationError::Limit)
    );
    assert_eq!(
        validate_workspace_limit(501),
        Err(WorkspaceValidationError::Limit)
    );
    assert_eq!(validate_workspace_limit(200), Ok(()));
}
