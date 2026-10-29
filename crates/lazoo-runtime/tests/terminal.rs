#![cfg(windows)]

use lazoo_runtime::{TerminalCommand, TerminalError, TerminalSession};
use std::{
    thread,
    time::{Duration, Instant},
};
use windows::Win32::{
    Foundation::CloseHandle,
    System::Threading::{OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION},
};

fn command(args: &[&str]) -> TerminalCommand {
    TerminalCommand::new(
        r"C:\Windows\System32\cmd.exe",
        std::env::current_dir().unwrap(),
        80,
        25,
    )
    .args(args.iter().copied())
}
fn powershell(args: &[&str]) -> TerminalCommand {
    TerminalCommand::new(
        r"C:\Windows\System32\WindowsPowerShell\v1.0\powershell.exe",
        std::env::current_dir().unwrap(),
        80,
        25,
    )
    .args(args.iter().copied())
}
fn wait_for(session: &TerminalSession, needle: &[u8]) -> Vec<u8> {
    for _ in 0..80 {
        let output = session.read_output(0, 64 * 1024).unwrap();
        if output
            .bytes
            .windows(needle.len())
            .any(|part| part == needle)
        {
            return output.bytes;
        }
        thread::sleep(Duration::from_millis(25));
    }
    panic!("timed out waiting for {needle:?}")
}

#[test]
fn echo_and_interactive_input_are_buffered() {
    let session = TerminalSession::spawn(command(&["/Q", "/K"])).unwrap();
    session.write_input(b"echo deterministic\r\n").unwrap();
    assert!(
        wait_for(&session, b"deterministic")
            .windows(13)
            .any(|p| p == b"deterministic")
    );
    session.stop().unwrap();
}
#[test]
fn resize_and_bounds_are_validated() {
    let session = TerminalSession::spawn(command(&["/Q", "/C", "echo ok"])).unwrap();
    assert!(matches!(
        session.resize(0, 25),
        Err(TerminalError::InvalidDimensions)
    ));
    session.resize(120, 40).unwrap();
    assert!(matches!(
        session.read_output(0, 64 * 1024 + 1),
        Err(TerminalError::ReadTooLarge)
    ));
    session.stop().unwrap();
}

#[test]
fn stop_ends_the_contained_console_process() {
    let session = TerminalSession::spawn(command(&["/Q", "/K"])).unwrap();
    let before_stop = session.resources().unwrap();
    assert!(before_stop.active_processes >= 1);
    assert!(before_stop.total_processes >= 1);
    session.stop().unwrap();
    assert!(!session.exit_status().unwrap().live);
    assert_eq!(session.resources().unwrap().active_processes, 0);
}

#[test]
fn read_only_view_does_not_keep_a_canonical_terminal_alive() {
    let session = TerminalSession::spawn(command(&["/Q", "/K"])).unwrap();
    let view = session.view();
    let pid = view.pid();
    drop(session);
    assert!(matches!(view.exit_status(), Err(TerminalError::Stopped)));
    for _ in 0..40 {
        let process = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) };
        if process.is_err() {
            return;
        }
        unsafe {
            let _ = CloseHandle(process.unwrap());
        }
        thread::sleep(Duration::from_millis(25));
    }
    panic!("canonical terminal {pid} survived after its last owner dropped");
}

#[test]
fn blocked_input_worker_is_cancelled_before_drop_joins() {
    let session = TerminalSession::spawn(powershell(&[
        "-NoProfile",
        "-Command",
        "Start-Sleep -Seconds 30",
    ]))
    .unwrap();
    let payload = vec![b'x'; 16 * 1024];
    let mut backpressured = false;
    for _ in 0..256 {
        if matches!(
            session.write_input(&payload),
            Err(TerminalError::InputBackpressure)
        ) {
            backpressured = true;
            break;
        }
    }
    assert!(backpressured, "input queue did not apply backpressure");
    let started = Instant::now();
    drop(session);
    assert!(started.elapsed() < Duration::from_secs(3));
}

#[test]
fn stop_kills_a_descendant_process_tree() {
    let script = "$p = Start-Process cmd.exe -ArgumentList '/Q','/C','ping -n 30 127.0.0.1 > nul' -PassThru; Write-Output ('child-pid:' + $p.Id); Start-Sleep -Seconds 30";
    let session = TerminalSession::spawn(powershell(&["-NoProfile", "-Command", script])).unwrap();
    let output = wait_for(&session, b"child-pid:");
    let after_marker = std::str::from_utf8(&output)
        .unwrap()
        .split("child-pid:")
        .nth(1)
        .unwrap();
    let pid_text: String = after_marker
        .chars()
        .skip_while(|character| !character.is_ascii_digit())
        .take_while(char::is_ascii_digit)
        .collect();
    let child_pid = pid_text.parse::<u32>().unwrap();
    session.stop().unwrap();
    for _ in 0..40 {
        let process = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, child_pid) };
        if process.is_err() {
            return;
        }
        unsafe {
            let _ = CloseHandle(process.unwrap());
        }
        thread::sleep(Duration::from_millis(25));
    }
    panic!("descendant {child_pid} survived terminal stop");
}

#[test]
fn failed_spawn_returns_without_a_session() {
    let failed = TerminalSession::spawn(TerminalCommand::new(
        r"Z:\definitely-missing-lazoo-terminal.exe",
        std::env::current_dir().unwrap(),
        80,
        25,
    ));
    assert!(matches!(failed, Err(TerminalError::Windows(_))));
}

#[test]
fn command_environment_overlay_is_visible_only_to_its_child() {
    const KEY: &str = "LAZOO_TERMINAL_OVERLAY_TEST";
    const VALUE: &str = "private=child=value";
    assert!(
        std::env::var_os(KEY).is_none(),
        "test key already exists in parent"
    );

    let session = TerminalSession::spawn(
        command(&["/Q", "/C", "echo %LAZOO_TERMINAL_OVERLAY_TEST%"])
            .env("lazoo_terminal_overlay_test", VALUE),
    )
    .unwrap();
    let output = wait_for(&session, VALUE.as_bytes());
    assert!(
        output
            .windows(VALUE.len())
            .any(|bytes| bytes == VALUE.as_bytes())
    );
    session.stop().unwrap();

    assert!(
        std::env::var_os(KEY).is_none(),
        "child overlay changed parent"
    );
    let sibling = TerminalSession::spawn(command(&[
        "/Q",
        "/C",
        "if defined LAZOO_TERMINAL_OVERLAY_TEST (echo inherited) else (echo clean)",
    ]))
    .unwrap();
    assert!(
        wait_for(&sibling, b"clean")
            .windows(5)
            .any(|bytes| bytes == b"clean")
    );
    sibling.stop().unwrap();
}

#[test]
fn command_environment_overlay_rejects_invalid_names_and_values() {
    for command in [
        command(&["/Q", "/C", "echo no"]).env("BAD=NAME", "value"),
        command(&["/Q", "/C", "echo no"]).env("BAD\0NAME", "value"),
        command(&["/Q", "/C", "echo no"]).env("VALID_NAME", "bad\0value"),
    ] {
        assert!(matches!(
            TerminalSession::spawn(command),
            Err(TerminalError::InvalidEnvironment)
        ));
    }
}

#[test]
fn command_debug_redacts_environment_overlay_values() {
    let command = command(&["/Q", "/C", "echo no"]).env("BEARER_TOKEN", "very-secret-value");
    let debug = format!("{command:?}");
    assert!(!debug.contains("very-secret-value"));
    assert!(debug.contains("BEARER_TOKEN"));
}
