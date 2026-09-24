use lazoo_domain::{
    IncidentNotice, MAX_NOTICE_OUTPUT_BYTES, TaskResultNotice, render_incident_notice_text,
    render_task_result_text,
};
use lazoo_protocol::{IncidentPriority, TaskState};
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
    });
    assert!(text.starts_with("[Lazoo incident] Priority incident LZ-MESSAGE-TIMEOUT on task"));
    assert!(text.contains(" for Mira: Delivery to Mira timed out"));
    assert!(text.contains("Occurrences: 3"));
    assert!(text.contains("Recommended next action: Check the provider session"));
    assert!(text.contains("will not retry, restart, or change authority"));
    assert!(text.contains(&Uuid::from_u128(9).to_string()));
}
