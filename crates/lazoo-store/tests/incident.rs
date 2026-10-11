use std::fs;

use lazoo_protocol::{
    AuthorityMode, EmployeeCheckoutPolicy, IncidentPriority, IncidentSource, IncidentState,
};
use lazoo_runtime::DataRoot;
use lazoo_store::{
    CreateEmployeeInput, ListIncidentsInput, RaiseIncidentInput, SettleIncidentInput, Store,
    StoreError,
};
use serde_json::json;
use tempfile::tempdir;
use uuid::Uuid;

fn store_with_employee() -> (tempfile::TempDir, Store, Uuid) {
    let temporary = tempdir().unwrap();
    let store = Store::open_path(temporary.path().join("lazoo.db")).unwrap();
    let employee = store
        .create_employee(&CreateEmployeeInput {
            request_id: "create-rhea-request".to_owned(),
            idempotency_key: "create-rhea".to_owned(),
            name: "Rhea".to_owned(),
            provider: "codex".to_owned(),
            role: "Lead".to_owned(),
            instructions: String::new(),
            model: None,
            reasoning_effort: None,
            native_settings: json!({}),
            authority_mode: AuthorityMode::Ask,
            checkout_policy: EmployeeCheckoutPolicy::Isolated,
            activation_policy: lazoo_protocol::ActivationPolicy::Manual,
            idle_timeout_seconds: 0,
            enabled: true,
            provider_observation: None,
        })
        .unwrap()
        .employee;
    (temporary, store, employee.id)
}

fn raise(key: &str, code: &str, employee_id: Option<Uuid>) -> RaiseIncidentInput {
    RaiseIncidentInput {
        request_id: format!("request-{key}"),
        idempotency_key: key.to_owned(),
        code: code.to_owned(),
        source: IncidentSource::Daemon,
        title: None,
        explanation: None,
        project_id: None,
        employee_id,
        provider: Some("codex".to_owned()),
        task_id: None,
        session_id: None,
        turn_id: None,
        message_id: None,
        detail: json!({"failureCode": code, "key": key}),
        occurred_at: None,
    }
}

fn settle(key: &str, incident_id: Uuid, expected_revision: u64) -> SettleIncidentInput {
    SettleIncidentInput {
        request_id: format!("request-{key}"),
        idempotency_key: key.to_owned(),
        incident_id,
        expected_revision,
    }
}

fn incident_event_types(store: &Store) -> Vec<String> {
    store
        .list_events(0, 1000)
        .unwrap()
        .into_iter()
        .map(|event| event.event_type)
        .filter(|kind| kind.starts_with("incident"))
        .collect()
}

#[test]
#[allow(clippy::too_many_lines)] // One ledger walk keeps fold, escalation, and replay assertions in order.
fn equivalent_failures_fold_escalate_at_the_threshold_and_replay_without_writes() {
    let (_temporary, store, employee) = store_with_employee();

    let first = store
        .raise_incident(&raise("stall-1", "LZ-MESSAGE-TIMEOUT", Some(employee)))
        .unwrap();
    assert_eq!(first.incident.version, 1);
    assert_eq!(first.incident.state, IncidentState::Open);
    assert_eq!(first.incident.priority, IncidentPriority::Normal);
    assert_eq!(first.incident.occurrences, 1);
    assert_eq!(first.incident.revision, 1);
    assert_eq!(first.incident.title, "Delivery deadline expired");
    assert!(first.incident.next_action.contains("explicitly"));
    assert_eq!(first.incident.employee_id, Some(employee));
    assert_eq!(first.incident.detail["key"], "stall-1");
    assert!(first.event_sequence.is_some());

    let replay = store
        .raise_incident(&raise("stall-1", "LZ-MESSAGE-TIMEOUT", Some(employee)))
        .unwrap();
    assert_eq!(replay.incident, first.incident);
    assert!(replay.event_sequence.is_none(), "replays are write-free");
    assert!(matches!(
        store.raise_incident(&raise("stall-1", "LZ-PROCESS-EXIT", Some(employee))),
        Err(StoreError::IncidentIdempotencyConflict)
    ));

    let second = store
        .raise_incident(&raise("stall-2", "LZ-MESSAGE-TIMEOUT", Some(employee)))
        .unwrap();
    assert_eq!(second.incident.id, first.incident.id);
    assert_eq!(second.incident.occurrences, 2);
    assert_eq!(second.incident.window_occurrences, 2);
    assert_eq!(second.incident.priority, IncidentPriority::Normal);
    assert_eq!(second.incident.revision, 2);
    assert_eq!(second.incident.detail["key"], "stall-2");

    let third = store
        .raise_incident(&raise("stall-3", "LZ-MESSAGE-TIMEOUT", Some(employee)))
        .unwrap();
    assert_eq!(third.incident.id, first.incident.id);
    assert_eq!(third.incident.occurrences, 3);
    assert_eq!(
        third.incident.priority,
        IncidentPriority::Priority,
        "three equivalent failures inside five minutes cross the threshold"
    );
    // No task tree means no lead to remediate it: the promotion is the user's at once.
    assert_eq!(
        third.incident.escalation,
        lazoo_protocol::IncidentEscalation::User
    );
    assert_eq!(
        third.incident.escalation_reason,
        Some(lazoo_protocol::IncidentEscalationReason::NoLead)
    );
    assert!(third.incident.remediation_deadline_at.is_none());
    assert_eq!(
        third.incident.first_occurred_at,
        first.incident.first_occurred_at
    );
    assert!(third.incident.last_occurred_at >= second.incident.last_occurred_at);

    let other_scope = store
        .raise_incident(&raise("stall-other", "LZ-MESSAGE-TIMEOUT", None))
        .unwrap();
    assert_ne!(other_scope.incident.id, first.incident.id);
    assert_eq!(other_scope.incident.occurrences, 1);
    let other_code = store
        .raise_incident(&raise("exit-1", "LZ-PROCESS-EXIT", Some(employee)))
        .unwrap();
    assert_ne!(other_code.incident.id, first.incident.id);

    assert_eq!(
        incident_event_types(&store),
        vec![
            "incident.raised",
            "incident.recurred",
            "incident.escalated",
            "incident.raised",
            "incident.raised",
        ]
    );
    let priority_only = store
        .list_incidents(&ListIncidentsInput {
            limit: 10,
            priority: Some(IncidentPriority::Priority),
            ..ListIncidentsInput::default()
        })
        .unwrap();
    assert_eq!(priority_only.len(), 1);
    assert_eq!(priority_only[0].id, first.incident.id);
    let for_employee = store
        .list_incidents(&ListIncidentsInput {
            limit: 10,
            employee_id: Some(employee),
            ..ListIncidentsInput::default()
        })
        .unwrap();
    assert_eq!(for_employee.len(), 2);
    let by_code = store
        .list_incidents(&ListIncidentsInput {
            limit: 10,
            code: Some("LZ-PROCESS-EXIT".to_owned()),
            ..ListIncidentsInput::default()
        })
        .unwrap();
    assert_eq!(by_code.len(), 1);
    assert!(matches!(
        store.list_incidents(&ListIncidentsInput {
            limit: 0,
            ..ListIncidentsInput::default()
        }),
        Err(StoreError::IncidentValidation(_))
    ));
    assert!(matches!(
        store.raise_incident(&raise("bad-code", "process-exit", Some(employee))),
        Err(StoreError::IncidentValidation(_))
    ));
}

#[test]
fn acknowledge_and_resolve_are_revisioned_idempotent_and_recurrence_reopens_until_resolved() {
    let (_temporary, store, employee) = store_with_employee();
    let raised = store
        .raise_incident(&raise("exit-1", "LZ-MESSAGE-TIMEOUT", Some(employee)))
        .unwrap()
        .incident;

    assert!(matches!(
        store.acknowledge_incident(&settle("ack-stale", raised.id, 7)),
        Err(StoreError::IncidentStale {
            current_revision: 1,
            ..
        })
    ));
    let acknowledged = store
        .acknowledge_incident(&settle("ack-1", raised.id, raised.revision))
        .unwrap();
    assert_eq!(acknowledged.incident.state, IncidentState::Acknowledged);
    assert_eq!(acknowledged.incident.revision, 2);
    assert!(acknowledged.incident.acknowledged_at.is_some());
    let replay = store
        .acknowledge_incident(&settle("ack-1", raised.id, 99))
        .unwrap();
    assert_eq!(replay.incident, acknowledged.incident);
    assert!(replay.event_sequence.is_none());
    assert!(
        matches!(
            store.acknowledge_incident(&settle("ack-2", raised.id, 2)),
            Err(StoreError::IncidentState { .. })
        ),
        "an acknowledged incident cannot be acknowledged again"
    );

    let recurred = store
        .raise_incident(&raise("exit-2", "LZ-MESSAGE-TIMEOUT", Some(employee)))
        .unwrap()
        .incident;
    assert_eq!(recurred.id, raised.id);
    assert_eq!(
        recurred.state,
        IncidentState::Open,
        "a new occurrence reopens an acknowledged incident"
    );
    assert!(recurred.acknowledged_at.is_none());
    assert_eq!(recurred.revision, 3);

    let resolved = store
        .resolve_incident(&settle("resolve-1", raised.id, recurred.revision))
        .unwrap()
        .incident;
    assert_eq!(resolved.state, IncidentState::Resolved);
    assert!(resolved.resolved_at.is_some());
    assert!(matches!(
        store.resolve_incident(&settle("resolve-2", raised.id, resolved.revision)),
        Err(StoreError::IncidentState { .. })
    ));
    assert!(matches!(
        store.acknowledge_incident(&settle("ack-3", Uuid::new_v4(), 1)),
        Err(StoreError::IncidentNotFound { .. })
    ));

    let fresh = store
        .raise_incident(&raise("exit-3", "LZ-MESSAGE-TIMEOUT", Some(employee)))
        .unwrap()
        .incident;
    assert_ne!(fresh.id, raised.id, "a resolved incident never reopens");
    assert_eq!(fresh.occurrences, 1);
    assert_eq!(
        incident_event_types(&store),
        vec![
            "incident.raised",
            "incident.acknowledged",
            "incident.recurred",
            "incident.resolved",
            "incident.raised",
        ]
    );
    let open = store
        .list_incidents(&ListIncidentsInput {
            limit: 10,
            state: Some(IncidentState::Open),
            ..ListIncidentsInput::default()
        })
        .unwrap();
    assert_eq!(open.len(), 1);
    assert_eq!(open[0].id, fresh.id);
}

#[test]
fn emergency_spool_records_fold_into_keeper_incidents_exactly_once() {
    let temporary = tempdir().unwrap();
    let root = DataRoot::from_path(temporary.path());
    root.ensure().unwrap();
    fs::write(
        root.spool_pending().join("incident-1.ndjson"),
        concat!(
            r#"{"incident_id":"incident-1","code":"LZ-DAEMON-UNAVAILABLE","#,
            r#""message":"restart budget exhausted","occurred_at":"2026-08-28T00:00:00Z"}"#,
            "\n",
            r#"{"incident_id":"incident-2","code":"not a code","#,
            r#""message":"daemon exited 3","occurred_at":"2026-08-28T00:01:00Z"}"#,
            "\n"
        ),
    )
    .unwrap();
    let store = Store::open_path(root.state_db()).unwrap();
    assert_eq!(store.status().unwrap().schema_version, 14);
    assert_eq!(store.import_pending_incidents(&root).unwrap(), 2);

    let raised = store.reconcile_emergency_incidents().unwrap();
    assert_eq!(raised.len(), 2, "both spool records fold on this boot");
    assert_eq!(
        raised[0].id, raised[1].id,
        "both spool records share one keeper scope"
    );
    assert_eq!(raised[0].code, "LZ-DAEMON-UNAVAILABLE");
    assert_eq!(raised[0].source, IncidentSource::Keeper);
    assert_eq!(raised[0].occurrences, 1);
    assert_eq!(raised[1].occurrences, 2);
    let folded = store.get_incident(raised[0].id).unwrap();
    assert_eq!(folded.occurrences, 2);
    assert_eq!(folded.window_occurrences, 2);
    assert_eq!(folded.first_occurred_at, "2026-08-28T00:00:00Z");
    assert_eq!(folded.last_occurred_at, "2026-08-28T00:01:00Z");
    assert_eq!(folded.explanation, "daemon exited 3");
    assert_eq!(folded.detail["emergencyIncidentId"], "incident-2");

    assert!(
        store.reconcile_emergency_incidents().unwrap().is_empty(),
        "a second startup folds nothing new"
    );
    assert_eq!(store.get_incident(raised[0].id).unwrap().occurrences, 2);
    assert_eq!(
        store
            .list_incidents(&ListIncidentsInput {
                limit: 10,
                ..ListIncidentsInput::default()
            })
            .unwrap()
            .len(),
        1
    );
}

#[test]
fn stalls_and_crashes_promote_at_once_and_a_second_unexpected_exit_promotes() {
    use lazoo_protocol::{IncidentEscalation, IncidentEscalationReason};

    let (_temporary, store, employee) = store_with_employee();
    for (key, code) in [
        ("stall", "LZ-SESSION-STALLED"),
        ("crash", "LZ-DAEMON-UNAVAILABLE"),
    ] {
        let born = store
            .raise_incident(&raise(key, code, Some(employee)))
            .unwrap()
            .incident;
        assert_eq!(born.occurrences, 1);
        assert_eq!(born.priority, IncidentPriority::Priority, "{code}");
        // No task tree, so no lead: the user is told at once.
        assert_eq!(born.escalation, IncidentEscalation::User);
        assert_eq!(
            born.escalation_reason,
            Some(IncidentEscalationReason::NoLead)
        );
    }

    let first_exit = store
        .raise_incident(&raise("exit-1", "LZ-PROCESS-EXIT", Some(employee)))
        .unwrap()
        .incident;
    assert_eq!(first_exit.priority, IncidentPriority::Normal);
    assert_eq!(first_exit.escalation, IncidentEscalation::None);
    let second_exit = store
        .raise_incident(&raise("exit-2", "LZ-PROCESS-EXIT", Some(employee)))
        .unwrap()
        .incident;
    assert_eq!(second_exit.id, first_exit.id);
    assert_eq!(second_exit.priority, IncidentPriority::Priority);
    assert_eq!(second_exit.escalation, IncidentEscalation::User);

    // A third stall folds into the promoted incident without a second promotion.
    let again = store
        .raise_incident(&raise("stall-2", "LZ-SESSION-STALLED", Some(employee)))
        .unwrap()
        .incident;
    assert_eq!(again.occurrences, 2);
    assert_eq!(
        incident_event_types(&store),
        vec![
            "incident.escalated",
            "incident.escalated",
            "incident.raised",
            "incident.escalated",
            "incident.recurred",
        ]
    );
}

#[test]
fn user_escalations_are_claimed_once_and_their_outcome_is_never_rewritten() {
    use chrono::Utc;
    use lazoo_protocol::{IncidentEscalation, IncidentUserNotification};

    let (_temporary, store, employee) = store_with_employee();
    // Two user escalations (a stall and a crash the keeper already announced), one normal
    // incident, and one user escalation the user has already acknowledged.
    let stall = store
        .raise_incident(&raise("stall", "LZ-SESSION-STALLED", Some(employee)))
        .unwrap()
        .incident;
    let crash = store
        .raise_incident(&RaiseIncidentInput {
            source: IncidentSource::Keeper,
            ..raise("crash", "LZ-DAEMON-UNAVAILABLE", None)
        })
        .unwrap()
        .incident;
    let normal = store
        .raise_incident(&raise("timeout", "LZ-MESSAGE-TIMEOUT", Some(employee)))
        .unwrap()
        .incident;
    let seen = store
        .raise_incident(&raise("seen", "LZ-SESSION-STALLED", None))
        .unwrap()
        .incident;
    store
        .acknowledge_incident(&settle("ack-seen", seen.id, seen.revision))
        .unwrap();
    for incident in [&stall, &crash, &seen] {
        assert_eq!(incident.escalation, IncidentEscalation::User);
        assert!(incident.user_notified_at.is_none());
    }

    let now = Utc::now();
    let claimed = store.claim_user_notifications(now, 64).unwrap();
    assert_eq!(
        claimed
            .iter()
            .map(|incident| incident.id)
            .collect::<Vec<_>>(),
        vec![stall.id],
        "the keeper's record, the normal incident, and the acknowledged one are not announced"
    );
    assert_eq!(
        claimed[0].user_notification,
        Some(IncidentUserNotification::Claimed)
    );
    assert_eq!(claimed[0].user_notified_at, Some(now.to_rfc3339()));
    assert_eq!(claimed[0].revision, stall.revision + 1);
    let crash = store.get_incident(crash.id).unwrap();
    assert_eq!(
        crash.user_notification,
        Some(IncidentUserNotification::Keeper)
    );
    assert!(
        store
            .get_incident(normal.id)
            .unwrap()
            .user_notification
            .is_none()
    );
    assert!(
        store
            .get_incident(seen.id)
            .unwrap()
            .user_notification
            .is_none()
    );
    assert!(
        store.claim_user_notifications(now, 64).unwrap().is_empty(),
        "a claim is never repeated"
    );

    let settled = store.settle_user_notifications(&[stall.id], false).unwrap();
    assert_eq!(
        settled[0].user_notification,
        Some(IncidentUserNotification::Failed)
    );
    assert!(
        store
            .settle_user_notifications(&[stall.id, crash.id], true)
            .unwrap()
            .is_empty(),
        "a settled outcome, and a keeper record, are never rewritten"
    );
    assert_eq!(
        store.get_incident(stall.id).unwrap().user_notification,
        Some(IncidentUserNotification::Failed)
    );
    let notified = incident_event_types(&store)
        .into_iter()
        .filter(|kind| kind == "incident.user-notified")
        .count();
    assert_eq!(notified, 2, "the keeper settlement and the failed outcome");
}

/// Sends one message from and to `employee_id`, claims it, and fails it for good, so it is
/// dead-lettered; returns its record.
fn dead_letter(
    store: &Store,
    key: &str,
    employee_id: Uuid,
    priority: lazoo_protocol::MessagePriority,
) -> lazoo_protocol::MessageRecord {
    use lazoo_protocol::{MessageEndpoint, MessageKind};
    use lazoo_store::{ClaimMessageInput, FailMessageInput, SendMessageInput};

    let endpoint = MessageEndpoint {
        employee_id,
        session_id: None,
    };
    let sent = store
        .send_message(&SendMessageInput {
            request_id: format!("send-{key}"),
            idempotency_key: format!("send-{key}"),
            sender: endpoint,
            recipient: endpoint,
            project_id: None,
            conversation_id: None,
            task_id: None,
            correlation_id: None,
            causation_id: None,
            kind: MessageKind::Status,
            priority,
            body: json!({"text": key}),
            references: Vec::new(),
            max_attempts: 3,
        })
        .unwrap()
        .message;
    let claimed = store
        .claim_message(&ClaimMessageInput {
            request_id: format!("claim-{key}"),
            idempotency_key: format!("claim-{key}"),
            message_id: sent.envelope.id,
            expected_revision: sent.delivery.revision,
        })
        .unwrap()
        .message;
    store
        .fail_message(&FailMessageInput {
            request_id: format!("fail-{key}"),
            idempotency_key: format!("fail-{key}"),
            message_id: sent.envelope.id,
            expected_revision: claimed.delivery.revision,
            attempt_id: claimed.attempts[0].id,
            retryable: false,
            failure_code: "LZ-TURN-PROVIDER".to_owned(),
            failure_message: "the provider refused the delivery".to_owned(),
        })
        .unwrap()
        .message
}

fn dead_letter_raise(key: &str, message: &lazoo_protocol::MessageRecord) -> RaiseIncidentInput {
    let attempt_id = message
        .dead_letter
        .as_ref()
        .map_or_else(Uuid::new_v4, |dead| dead.attempt_id);
    RaiseIncidentInput {
        idempotency_key: lazoo_store::dead_letter_incident_key(message.envelope.id, attempt_id),
        employee_id: Some(message.envelope.recipient.employee_id),
        message_id: Some(message.envelope.id),
        ..raise(key, "LZ-MESSAGE-DEADLETTER", None)
    }
}

#[test]
fn one_dead_lettered_priority_message_is_a_priority_incident_raised_once() {
    use lazoo_protocol::{IncidentEscalation, IncidentEscalationReason, MessagePriority};

    let (_temporary, store, employee_id) = store_with_employee();
    let normal = dead_letter(&store, "normal", employee_id, MessagePriority::Normal);
    let urgent = dead_letter(&store, "urgent", employee_id, MessagePriority::Priority);
    assert_eq!(urgent.envelope.priority, MessagePriority::Priority);
    assert_eq!(normal.envelope.priority, MessagePriority::Normal);
    assert!(urgent.dead_letter.is_some() && normal.dead_letter.is_some());
    let summaries = store.list_messages(0, 10).unwrap();
    assert_eq!(summaries[0].priority, MessagePriority::Normal);
    assert_eq!(summaries[1].priority, MessagePriority::Priority);

    // Only the priority dead letter waits for its incident.
    let unnoted = store.unnoted_priority_dead_letters(10).unwrap();
    assert_eq!(unnoted.len(), 1);
    assert_eq!(unnoted[0].envelope.id, urgent.envelope.id);

    // A normal dead letter still needs three inside the window.
    let folded = store
        .raise_incident(&dead_letter_raise("normal", &normal))
        .unwrap()
        .incident;
    assert_eq!(folded.priority, IncidentPriority::Normal);

    // One priority dead letter promotes the incident it folds into, and with no lead to
    // remediate it, it is the user's.
    let promoted = store
        .raise_incident(&dead_letter_raise("urgent", &urgent))
        .unwrap()
        .incident;
    assert_eq!(promoted.id, folded.id);
    assert_eq!(promoted.occurrences, 2);
    assert_eq!(promoted.priority, IncidentPriority::Priority);
    assert_eq!(promoted.escalation, IncidentEscalation::User);
    assert_eq!(
        promoted.escalation_reason,
        Some(IncidentEscalationReason::NoLead)
    );
    assert_eq!(promoted.message_id, Some(urgent.envelope.id));
    assert!(store.unnoted_priority_dead_letters(10).unwrap().is_empty());
    let replay = store
        .raise_incident(&dead_letter_raise("urgent", &urgent))
        .unwrap();
    assert!(
        replay.event_sequence.is_none(),
        "one incident per dead letter"
    );

    // Priority is read from the dead letter itself: naming a priority message that was never
    // dead-lettered, or using another code, promotes nothing.
    let (_other_temporary, other, other_employee) = store_with_employee();
    let live = dead_letter(&other, "live", other_employee, MessagePriority::Priority);
    let endpoint = live.envelope.recipient;
    let pending = other
        .send_message(&lazoo_store::SendMessageInput {
            request_id: "send-pending".to_owned(),
            idempotency_key: "send-pending".to_owned(),
            sender: endpoint,
            recipient: endpoint,
            project_id: None,
            conversation_id: None,
            task_id: None,
            correlation_id: None,
            causation_id: None,
            kind: lazoo_protocol::MessageKind::Status,
            priority: MessagePriority::Priority,
            body: json!({"text": "still queued"}),
            references: Vec::new(),
            max_attempts: 3,
        })
        .unwrap()
        .message;
    let asserted = other
        .raise_incident(&RaiseIncidentInput {
            message_id: Some(pending.envelope.id),
            employee_id: Some(other_employee),
            ..raise("asserted", "LZ-MESSAGE-DEADLETTER", None)
        })
        .unwrap()
        .incident;
    assert_eq!(asserted.priority, IncidentPriority::Normal);
    let timeout = other
        .raise_incident(&RaiseIncidentInput {
            message_id: Some(live.envelope.id),
            ..raise("timeout", "LZ-MESSAGE-TIMEOUT", Some(other_employee))
        })
        .unwrap()
        .incident;
    assert_eq!(timeout.priority, IncidentPriority::Normal);
}
