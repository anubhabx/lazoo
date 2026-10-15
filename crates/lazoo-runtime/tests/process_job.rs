use std::{process::Command, thread, time::Duration};

use lazoo_runtime::{JobObject, LaunchGate, current_process_accounting, wait_for_launch_gate};

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

#[test]
fn job_accounting_reports_the_tree_and_survives_its_termination() {
    let job = JobObject::kill_on_close().unwrap();
    let mut child = Command::new("powershell.exe")
        .args(["-NoProfile", "-Command", "Start-Sleep -Seconds 30"])
        .spawn()
        .unwrap();
    job.assign_process(&child).unwrap();

    let live = job.accounting().unwrap();
    assert!(live.active_processes >= 1, "{live:?}");
    assert!(live.total_processes >= live.active_processes, "{live:?}");
    assert!(live.peak_process_memory_bytes > 0, "{live:?}");
    assert!(
        live.peak_job_memory_bytes >= live.peak_process_memory_bytes,
        "{live:?}"
    );

    job.terminate(93).unwrap();
    child.wait().unwrap();
    let after = job.accounting().unwrap();
    assert_eq!(after.active_processes, 0, "{after:?}");
    assert!(after.total_processes >= live.total_processes, "{after:?}");
    assert!(
        after.peak_job_memory_bytes >= live.peak_job_memory_bytes,
        "{after:?}"
    );
    assert!(
        after.user_cpu_millis + after.kernel_cpu_millis
            >= live.user_cpu_millis + live.kernel_cpu_millis,
        "{after:?}"
    );
}

#[test]
fn current_process_accounting_reports_this_process() {
    let sample = current_process_accounting().unwrap();
    assert_eq!(sample.pid, std::process::id());
    assert!(sample.working_set_bytes > 0, "{sample:?}");
    assert!(
        sample.peak_working_set_bytes >= sample.working_set_bytes,
        "{sample:?}"
    );
    assert!(sample.handle_count > 0, "{sample:?}");
    let later = current_process_accounting().unwrap();
    assert!(
        later.user_cpu_millis + later.kernel_cpu_millis
            >= sample.user_cpu_millis + sample.kernel_cpu_millis,
        "{later:?}"
    );
}
