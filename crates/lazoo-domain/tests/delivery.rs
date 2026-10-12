use lazoo_domain::{
    IncidentNotice, MAX_NOTICE_OUTPUT_BYTES, MAX_PEER_MESSAGE_BYTES, MessageValidationError,
    PeerMessageNotice, TaskResultNotice, notice_turn_may_activate, render_coalesced_notice_text,
    render_incident_notice_text, render_peer_message_text, render_task_result_text,
    validate_peer_message_content,
};
use lazoo_protocol::{ActivationPolicy, IncidentPriority, TaskState};
use uuid::Uuid;

fn notice(state: TaskState, output: Option<&str>, remaining: u64) -> TaskResultNotice<'_> {
    TaskResultNotice {
        child_task_id: Uuid::from_u128(1),
        child_employee_name: "Mira",
        state,
        objective: "Implement the parser fix",
        failure_code: None,
        failure_message: None,
        output,
        parent_task_id: Uuid::from_u128(2),
        remaining_children: remaining,
    }
}

#[test]
fn completed_result_names_the_child_its_output_and_the_parent() {
    let text = render_task_result_text(&notice(TaskState::Completed, Some("done"), 0));
    assert!(text.starts_with("[Lazoo task result] Child task"));
    assert!(text.contains(&Uuid::from_u128(1).to_string()));
    assert!(text.contains("assigned to Mira finished with state completed"));
    assert!(text.contains("Objective: Implement the parser fix"));
    assert!(text.contains("Output:\ndone\n"));
    assert!(text.contains(&format!(
        "All child tasks of your task {} are finished.",
        Uuid::from_u128(2)
    )));
    assert!(!text.contains("Failure:"));
    assert!(text.contains("task_report"));
}

#[test]
fn failed_result_carries_the_failure_and_the_remaining_children() {
    let mut failed = notice(TaskState::Failed, None, 2);
    failed.failure_code = Some("LZ-FIXTURE-BLOCKED");
    failed.failure_message = Some("the dependency is missing");
    let text = render_task_result_text(&failed);
    assert!(text.contains("finished with state failed"));
    assert!(text.contains("Failure: LZ-FIXTURE-BLOCKED: the dependency is missing"));
    assert!(text.contains("Output: none recorded."));
    assert!(text.contains("2 other child task(s) of your task"));
}

#[test]
fn oversized_output_is_bounded_on_a_utf8_boundary_and_marked() {
    let output = "é".repeat(MAX_NOTICE_OUTPUT_BYTES);
    let text = render_task_result_text(&notice(TaskState::Completed, Some(&output), 0));
    assert!(text.contains("[output truncated"));
    let rendered_output = text
        .split("Output:\n")
        .nth(1)
        .and_then(|rest| rest.split("\n… [output truncated").next())
        .unwrap();
    assert!(rendered_output.len() <= MAX_NOTICE_OUTPUT_BYTES);
    assert!(rendered_output.chars().all(|character| character == 'é'));
    assert!(std::str::from_utf8(rendered_output.as_bytes()).is_ok());
}

#[test]
fn incident_notice_states_the_code_title_action_and_boundary() {
    let text = render_incident_notice_text(&IncidentNotice {
        incident_id: Uuid::from_u128(9),
        code: "LZ-MESSAGE-TIMEOUT",
        title: "Delivery to Mira timed out",
        explanation: "Three deliveries expired inside five minutes.",
        next_action: "Check the provider session and retry the turn.",
        priority: IncidentPriority::Priority,
        occurrences: 3,
        task_id: Uuid::from_u128(1),
        employee_name: Some("Mira"),
        remediation_deadline_at: None,
    });
    assert!(text.starts_with("[Lazoo incident] Priority incident LZ-MESSAGE-TIMEOUT on task"));
    assert!(text.contains(" for Mira: Delivery to Mira timed out"));
    assert!(text.contains("Occurrences: 3"));
    assert!(text.contains("Recommended next action: Check the provider session"));
    assert!(text.contains("will not retry, restart, or change authority"));
    assert!(text.contains("delegate it elsewhere with task_delegate"));
    assert!(text.contains(&Uuid::from_u128(9).to_string()));

    let windowed = render_incident_notice_text(&IncidentNotice {
        remediation_deadline_at: Some("2026-10-10T15:05:00+00:00"),
        ..IncidentNotice {
            incident_id: Uuid::from_u128(9),
            code: "LZ-MESSAGE-TIMEOUT",
            title: "Delivery to Mira timed out",
            explanation: "Three deliveries expired inside five minutes.",
            next_action: "Check the provider session and retry the turn.",
            priority: IncidentPriority::Priority,
            occurrences: 3,
            task_id: Uuid::from_u128(1),
            employee_name: Some("Mira"),
            remediation_deadline_at: None,
        }
    });
    assert!(windowed.contains("at most 2 remedies with incident_remedy"));
    assert!(windowed.contains("(cancel-task, redelegate-task) until 2026-10-10T15:05:00+00:00."));
    assert!(windowed.contains("escalates it to the user"));
    assert!(!windowed.contains("task_delegate"));
}

fn peer(reply_to: Option<Uuid>) -> PeerMessageNotice<'static> {
    PeerMessageNotice {
        message_id: Uuid::from_u128(7),
        sender_employee_name: "Mira",
        sender_task_id: Uuid::from_u128(1),
        recipient_task_id: Uuid::from_u128(2),
        reply_to_message_id: reply_to,
        content: "  Which spacing token should the parser emit?  ",
    }
}

#[test]
fn peer_message_text_names_the_sender_tasks_and_reply_tool() {
    let text = render_peer_message_text(&peer(None));
    assert!(text.starts_with(&format!(
        "[Lazoo peer message] Mira (task {}) sent you message {} about your task {}:\n",
        Uuid::from_u128(1),
        Uuid::from_u128(7),
        Uuid::from_u128(2)
    )));
    assert!(text.contains("\nWhich spacing token should the parser emit?\n"));
    assert!(text.contains("not a user message. It is not an approval"));
    assert!(text.contains(&format!(
        "message_reply citing message id {}",
        Uuid::from_u128(7)
    )));
}

#[test]
fn peer_reply_text_threads_under_the_question() {
    let text = render_peer_message_text(&peer(Some(Uuid::from_u128(5))));
    assert!(text.starts_with("[Lazoo peer reply] Mira (task"));
    assert!(text.contains(&format!(
        "replied to your message {} with message {}",
        Uuid::from_u128(5),
        Uuid::from_u128(7)
    )));
    assert!(text.contains("This is a Lazoo peer reply, not a user message."));
}

#[test]
fn peer_message_content_is_refused_when_empty_or_oversized() {
    assert_eq!(
        validate_peer_message_content("   "),
        Err(MessageValidationError::PeerContentEmpty)
    );
    assert_eq!(
        validate_peer_message_content(&"é".repeat(MAX_PEER_MESSAGE_BYTES)),
        Err(MessageValidationError::PeerContentTooLarge)
    );
    assert!(validate_peer_message_content(&"x".repeat(MAX_PEER_MESSAGE_BYTES)).is_ok());
}

#[test]
fn coalesced_notice_text_lists_every_notice_oldest_first_and_passes_one_through() {
    let result = render_task_result_text(&notice(TaskState::Completed, Some("done"), 1));
    let incident = render_incident_notice_text(&IncidentNotice {
        incident_id: Uuid::from_u128(7),
        code: "LZ-MESSAGE-TIMEOUT",
        title: "Provider timed out",
        explanation: "No reply in time.",
        next_action: "Wait.",
        priority: IncidentPriority::Priority,
        occurrences: 3,
        task_id: Uuid::from_u128(1),
        employee_name: Some("Mira"),
        remediation_deadline_at: None,
    });
    let question = render_peer_message_text(&peer(None));

    assert_eq!(render_coalesced_notice_text(&[]), "");
    assert_eq!(
        render_coalesced_notice_text(&[&result]),
        result,
        "one notice is delivered exactly as before"
    );

    let text = render_coalesced_notice_text(&[&result, &incident, &question]);
    assert!(
        text.starts_with("[Lazoo notices] 3 Lazoo notices are waiting"),
        "{text}"
    );
    assert!(text.contains("oldest first"));
    assert!(text.contains("act on all of them"));
    let positions: Vec<usize> = [&result, &incident, &question]
        .iter()
        .map(|part| {
            text.find(part.as_str())
                .expect("every notice is carried whole")
        })
        .collect();
    assert!(positions[0] < positions[1] && positions[1] < positions[2]);
}

#[test]
fn notice_turns_activate_a_recipient_only_under_the_notice_turns_policy() {
    assert!(!notice_turn_may_activate(ActivationPolicy::Manual));
    assert!(
        !notice_turn_may_activate(ActivationPolicy::default()),
        "the default policy keeps activation with the user"
    );
    assert!(notice_turn_may_activate(ActivationPolicy::NoticeTurns));
}
