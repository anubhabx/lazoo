use std::{fs, sync::Arc, thread};

use base64::Engine;
use lazoo_runtime::{
    DataRoot, Installation, OwnerSecurity, RuntimeError, SingleInstance, atomic_write,
};
use tempfile::tempdir;

#[test]
fn data_root_creates_the_documented_layout() {
    let temporary = tempdir().unwrap();
    let root = DataRoot::from_path(temporary.path());

    root.ensure().unwrap();

    assert!(root.state_dir().is_dir());
    assert!(root.runtime_dir().is_dir());
    assert!(root.spool_active().parent().unwrap().is_dir());
    assert!(root.spool_pending().is_dir());
    assert!(root.spool_reconciled().is_dir());
    assert!(root.daemon_logs().is_dir());
    assert!(root.keeper_logs().is_dir());
}

#[test]
fn atomic_write_replaces_the_complete_file() {
    let temporary = tempdir().unwrap();
    let path = temporary.path().join("state.json");

    atomic_write(&path, br#"{"revision":1}"#).unwrap();
    atomic_write(&path, br#"{"revision":2}"#).unwrap();

    assert_eq!(fs::read(path).unwrap(), br#"{"revision":2}"#);
}

#[test]
fn installation_round_trips_a_dpapi_protected_nonce() {
    let temporary = tempdir().unwrap();
    let root = DataRoot::from_path(temporary.path());
    root.ensure().unwrap();

    let created = Installation::load_or_create(&root).unwrap();
    let reopened = Installation::load_or_create(&root).unwrap();

    assert_eq!(created.id(), reopened.id());
    assert_eq!(created.pipe_name(), reopened.pipe_name());
    assert_eq!(
        created.unprotect_nonce().unwrap(),
        reopened.unprotect_nonce().unwrap()
    );
    let metadata = fs::read_to_string(root.installation_file()).unwrap();
    let plaintext =
        base64::engine::general_purpose::STANDARD.encode(created.unprotect_nonce().unwrap());
    assert!(!metadata.contains(&plaintext));
}

#[test]
fn current_owner_has_a_string_sid_and_protected_sddl() {
    let owner = OwnerSecurity::current().unwrap();

    assert!(owner.sid().starts_with("S-1-5-"));
    assert_eq!(
        owner.protected_sddl(),
        format!("D:P(A;;GA;;;SY)(A;;GA;;;{})", owner.sid())
    );
}

#[test]
fn owner_security_applies_a_protected_runtime_dacl() {
    let temporary = tempdir().unwrap();
    let owner = OwnerSecurity::current().unwrap();

    owner.apply_to_path(temporary.path()).unwrap();
    let sddl = OwnerSecurity::read_path_sddl(temporary.path()).unwrap();

    assert!(sddl.contains(owner.sid()));
    assert!(sddl.contains("SY"));
    assert!(!sddl.contains(";;;BU)"));
    assert!(!sddl.contains(";;;AU)"));
}

#[test]
fn duplicate_single_instance_is_rejected_until_guard_drops() {
    let installation = uuid::Uuid::new_v4();
    let first = SingleInstance::acquire(installation, "runtime-test").unwrap();

    let second = SingleInstance::acquire(installation, "runtime-test").unwrap_err();
    assert!(matches!(second, RuntimeError::AlreadyRunning { .. }));

    drop(first);
    SingleInstance::acquire(installation, "runtime-test").unwrap();
}

#[test]
fn concurrent_installation_creation_converges_on_one_identity() {
    let temporary = Arc::new(tempdir().unwrap());
    let root = DataRoot::from_path(temporary.path());
    root.ensure().unwrap();

    let handles: Vec<_> = (0..4)
        .map(|_| {
            let root = root.clone();
            thread::spawn(move || Installation::load_or_create(&root).unwrap().id())
        })
        .collect();
    let ids: Vec<_> = handles
        .into_iter()
        .map(|handle| handle.join().unwrap())
        .collect();

    assert!(ids.windows(2).all(|pair| pair[0] == pair[1]));
}
