use std::{process::Command, thread, time::Duration};

use lazoo_runtime::{JobObject, LaunchGate, wait_for_launch_gate};

#[test]
fn job_object_owns_and_terminates_an_assigned_process() {
    let job = JobObject::kill_on_close().unwrap();
    let mut child = Command::new("powershell.exe")
        .args(["-NoProfile", "-Command", "Start-Sleep -Seconds 30"])
        .spawn()
        .unwrap();

    job.assign_process(&child).unwrap();
    assert!(job.contains_process(&child).unwrap());
    assert!(job.active_processes().unwrap() >= 1);

    job.terminate(91).unwrap();
    let status = child.wait().unwrap();
    assert!(!status.success());
}

#[test]
fn launch_gate_blocks_until_explicitly_signaled() {
    let gate = LaunchGate::create().unwrap();
    let name = gate.name().to_owned();
    let waiter = thread::spawn(move || wait_for_launch_gate(&name).unwrap());

    thread::sleep(Duration::from_millis(100));
    assert!(!waiter.is_finished());
    gate.signal().unwrap();
    waiter.join().unwrap();
}
