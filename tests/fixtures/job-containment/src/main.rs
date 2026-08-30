use std::{collections::BTreeMap, path::Path, time::Duration};

use lazoo_runtime::{JobChild, JobCommand};
use serde_json::json;
use tokio::{
    io::AsyncReadExt as _,
    time::{Instant, sleep},
};
use uuid::Uuid;
use windows::Win32::{
    Foundation::{CloseHandle, HANDLE, WAIT_OBJECT_0, WAIT_TIMEOUT},
    System::Threading::{OpenProcess, PROCESS_SYNCHRONIZE, WaitForSingleObject},
};

#[tokio::main]
async fn main() {
    if let Err(error) = run().await {
        eprintln!("{error}");
        std::process::exit(1);
    }
}

async fn run() -> Result<(), Box<dyn std::error::Error>> {
    let directory = std::env::current_exe()?
        .parent()
        .ok_or("probe executable has no parent")?
        .to_path_buf();
    let host = directory.join("lazoo-session-host.exe");
    let tree = directory.join("lazoo-session-tree-fixture.exe");
    let run_root = std::env::temp_dir().join(format!("lazoo-job-probe-{}", Uuid::new_v4()));
    std::fs::create_dir(&run_root)?;
    let pid_file = run_root.join("pids.txt");
    let command = JobCommand::new(&tree, &run_root).args([
        std::ffi::OsString::from("--root"),
        pid_file.as_os_str().to_owned(),
    ]);
    let mut child = JobChild::spawn_with_host(&host, &command)?;
    let mut stderr = child
        .take_stderr()
        .ok_or("session host stderr is unavailable")?;
    let stderr_task = tokio::spawn(async move {
        let mut output = String::new();
        let _ = stderr.read_to_string(&mut output).await;
        output
    });
    let host_pid = child.id().ok_or("session host PID is unavailable")?;
    let pids = match wait_for_fixture_pids(&pid_file).await {
        Ok(pids) => pids,
        Err(error) => {
            let status = child.try_wait()?;
            let stderr = if status.is_some() {
                stderr_task.await?
            } else {
                child.terminate_tree(94)?;
                let _ = child.wait().await;
                stderr_task.await?
            };
            return Err(format!("{error}; host status {status:?}; stderr: {stderr}").into());
        }
    };
    let root_pid = *pids.get("root").ok_or("root fixture PID is missing")?;
    let child_pid = *pids.get("child").ok_or("child fixture PID is missing")?;
    let host_contained = child
        .job()
        .contains_process_id(host_pid)
        .map_err(|error| format!("host containment check failed: {error}"))?;
    let root_contained = child
        .job()
        .contains_process_id(root_pid)
        .map_err(|error| format!("root containment check failed: {error}"))?;
    let child_contained = child
        .job()
        .contains_process_id(child_pid)
        .map_err(|error| format!("child containment check failed: {error}"))?;
    if !host_contained || !root_contained || !child_contained {
        return Err("one or more fixture processes escaped the Job Object".into());
    }
    if child
        .job()
        .active_processes()
        .map_err(|error| format!("job accounting query failed: {error}"))?
        < 3
    {
        return Err("Job Object did not report the host, root, and child processes".into());
    }
    let root_wait = ProcessWaitHandle::open(root_pid)?;
    let child_wait = ProcessWaitHandle::open(child_pid)?;
    child
        .terminate_tree(93)
        .map_err(|error| format!("job termination failed: {error}"))?;
    let _ = child
        .wait()
        .await
        .map_err(|error| format!("session host wait failed: {error}"))?;
    root_wait
        .wait(root_pid)
        .map_err(|error| format!("root exit check failed: {error}"))?;
    child_wait
        .wait(child_pid)
        .map_err(|error| format!("child exit check failed: {error}"))?;
    println!(
        "{}",
        serde_json::to_string(&json!({
            "status": "PASS",
            "hostPid": host_pid,
            "rootPid": root_pid,
            "childPid": child_pid,
            "creationTimeContainment": true,
            "completeTreeTerminated": true
        }))?
    );
    std::fs::remove_dir_all(&run_root)?;
    Ok(())
}

async fn wait_for_fixture_pids(
    path: &Path,
) -> Result<BTreeMap<String, u32>, Box<dyn std::error::Error>> {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        if let Ok(contents) = std::fs::read_to_string(path) {
            let pids = contents
                .lines()
                .filter_map(|line| line.split_once('='))
                .map(|(label, pid)| Ok((label.to_owned(), pid.parse::<u32>()?)))
                .collect::<Result<BTreeMap<_, _>, std::num::ParseIntError>>()?;
            if pids.len() == 2 {
                return Ok(pids);
            }
        }
        if Instant::now() >= deadline {
            return Err("fixture processes did not report PIDs".into());
        }
        sleep(Duration::from_millis(50)).await;
    }
}

struct ProcessWaitHandle(HANDLE);

impl ProcessWaitHandle {
    fn open(process_id: u32) -> Result<Self, windows::core::Error> {
        unsafe { OpenProcess(PROCESS_SYNCHRONIZE, false, process_id) }.map(Self)
    }

    fn wait(&self, process_id: u32) -> Result<(), Box<dyn std::error::Error>> {
        let result = unsafe { WaitForSingleObject(self.0, 10_000) };
        if result == WAIT_OBJECT_0 {
            Ok(())
        } else if result == WAIT_TIMEOUT {
            Err(format!("process {process_id} remained alive after Job Object termination").into())
        } else {
            Err(format!("waiting for process {process_id} failed").into())
        }
    }
}

impl Drop for ProcessWaitHandle {
    fn drop(&mut self) {
        unsafe {
            let _ = CloseHandle(self.0);
        }
    }
}
