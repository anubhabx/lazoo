use std::{
    fs,
    sync::{Arc, Mutex},
    time::Duration,
};

use lazoo_keeper::{
    EmergencyIncident, EmergencySpool, KeeperState, NotificationPort, RestartAction, RestartBudget,
    SpoolError, record_unavailable,
};
use lazoo_runtime::DataRoot;
use tempfile::tempdir;

#[test]
fn restart_budget_uses_fixed_backoff_then_exhausts() {
    let mut budget = RestartBudget::default();
    assert_eq!(
        budget.record_exit(0),
        RestartAction::RestartAfter(Duration::from_secs(1))
    );
    assert_eq!(
        budget.record_exit(1_000),
        RestartAction::RestartAfter(Duration::from_secs(2))
    );
    assert_eq!(
        budget.record_exit(2_000),
        RestartAction::RestartAfter(Duration::from_secs(5))
    );
    assert_eq!(budget.record_exit(3_000), RestartAction::Exhausted);
}

#[test]
fn ten_healthy_minutes_reset_the_restart_window() {
    let mut budget = RestartBudget::default();
    budget.record_exit(0);
    budget.record_exit(1_000);
    assert_eq!(
        budget.record_exit(601_001),
        RestartAction::RestartAfter(Duration::from_secs(1))
    );
}

#[test]
fn keeper_state_replaces_atomically_and_round_trips() {
    let temporary = tempdir().unwrap();
    let root = DataRoot::from_path(temporary.path());
    root.ensure().unwrap();
    let state = KeeperState {
        exit_times_millis: vec![1, 2],
        consecutive_failures: 2,
        last_exit_code: Some(9),
        last_incident_id: None,
        ..KeeperState::default()
    };
    state.save(&root).unwrap();
    assert_eq!(KeeperState::load(&root).unwrap(), state);
}

#[test]
fn stale_active_segment_seals_complete_lines_and_preserves_partial_tail() {
    let temporary = tempdir().unwrap();
    let root = DataRoot::from_path(temporary.path());
    root.ensure().unwrap();
    fs::write(root.spool_active(), b"{\"complete\":true}\n{\"partial\":").unwrap();
    let spool = EmergencySpool::new(root.clone());
    let sealed = spool.recover_stale().unwrap().unwrap();
    assert_eq!(fs::read_to_string(sealed).unwrap(), "{\"complete\":true}\n");
    assert_eq!(
        fs::read_to_string(root.spool_active()).unwrap(),
        "{\"partial\":"
    );
}

#[test]
fn spool_enforces_record_and_pending_segment_bounds_without_overwrite() {
    let temporary = tempdir().unwrap();
    let root = DataRoot::from_path(temporary.path());
    root.ensure().unwrap();
    let spool = EmergencySpool::new(root.clone());
    let mut oversized = incident("large");
    oversized.message = "x".repeat(16 * 1024);
    assert!(matches!(
        spool.append_and_seal(&oversized),
        Err(SpoolError::RecordTooLarge { .. })
    ));
    for index in 0..8 {
        fs::write(
            root.spool_pending()
                .join(format!("existing-{index}.ndjson")),
            "{}\n",
        )
        .unwrap();
    }
    assert!(matches!(
        spool.append_and_seal(&incident("ninth")),
        Err(SpoolError::Capacity)
    ));
    assert_eq!(fs::read_dir(root.spool_pending()).unwrap().count(), 8);
}

#[test]
fn notifier_failure_cannot_undo_durable_incident() {
    let temporary = tempdir().unwrap();
    let root = DataRoot::from_path(temporary.path());
    root.ensure().unwrap();
    let notifier = RecordingNotifier::failing();
    let mut state = KeeperState::default();
    let result = record_unavailable(&root, &notifier, &mut state, 12);
    assert!(result.is_ok());
    assert_eq!(*notifier.calls.lock().unwrap(), 1);
    assert!(state.notification_attempted);
    assert_eq!(state.notification_error.as_deref(), Some("blocked"));
    let segment = fs::read_dir(root.spool_pending())
        .unwrap()
        .next()
        .unwrap()
        .unwrap()
        .path();
    let body = fs::read_to_string(segment).unwrap();
    assert!(body.contains("LZ-DAEMON-UNAVAILABLE"));
}

#[test]
fn repeated_exhaustion_with_same_state_is_deduplicated() {
    let temporary = tempdir().unwrap();
    let root = DataRoot::from_path(temporary.path());
    root.ensure().unwrap();
    let notifier = RecordingNotifier::working();
    let mut state = KeeperState::default();
    record_unavailable(&root, &notifier, &mut state, 1).unwrap();
    record_unavailable(&root, &notifier, &mut state, 1).unwrap();
    let records: usize = fs::read_dir(root.spool_pending())
        .unwrap()
        .map(|entry| {
            fs::read_to_string(entry.unwrap().path())
                .unwrap()
                .lines()
                .count()
        })
        .sum();
    assert_eq!(records, 1);
}

fn incident(id: &str) -> EmergencyIncident {
    EmergencyIncident {
        incident_id: id.to_owned(),
        code: "LZ-DAEMON-UNAVAILABLE".to_owned(),
        message: "daemon failed".to_owned(),
        occurred_at: "2026-08-28T00:00:00Z".to_owned(),
    }
}

struct RecordingNotifier {
    calls: Arc<Mutex<usize>>,
    fails: bool,
}

impl RecordingNotifier {
    fn failing() -> Self {
        Self {
            calls: Arc::new(Mutex::new(0)),
            fails: true,
        }
    }
    fn working() -> Self {
        Self {
            calls: Arc::new(Mutex::new(0)),
            fails: false,
        }
    }
}

impl NotificationPort for RecordingNotifier {
    fn notify(&self, _title: &str, _message: &str) -> Result<(), String> {
        *self.calls.lock().unwrap() += 1;
        if self.fails {
            Err("blocked".to_owned())
        } else {
            Ok(())
        }
    }
}
