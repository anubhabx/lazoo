use std::{
    collections::BTreeMap,
    ffi::{OsStr, OsString, c_void},
    os::windows::io::{AsRawHandle, RawHandle},
    path::{Path, PathBuf},
    process::{ExitStatus, Stdio},
    sync::Arc,
};

use tokio::process::{Child, ChildStderr, ChildStdin, ChildStdout, Command};
use uuid::Uuid;
use windows::{
    Win32::{
        Foundation::{CloseHandle, HANDLE, WAIT_OBJECT_0},
        System::{
            JobObjects::{
                AssignProcessToJobObject, CreateJobObjectW, IsProcessInJob,
                JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE, JOBOBJECT_BASIC_ACCOUNTING_INFORMATION,
                JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JobObjectBasicAccountingInformation,
                JobObjectExtendedLimitInformation, QueryInformationJobObject,
                SetInformationJobObject, TerminateJobObject,
            },
            Threading::{
                CreateEventW, INFINITE, OpenEventW, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION,
                SYNCHRONIZATION_SYNCHRONIZE, SetEvent, WaitForSingleObject,
            },
        },
    },
    core::{BOOL, HSTRING},
};

use crate::RuntimeError;

#[derive(Debug)]
pub struct JobObject {
    handle: HANDLE,
}

impl JobObject {
    pub fn kill_on_close() -> Result<Self, RuntimeError> {
        let handle = unsafe { CreateJobObjectW(None, None) }?;
        let job = Self { handle };
        let mut limits = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
        limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
        unsafe {
            SetInformationJobObject(
                job.handle,
                JobObjectExtendedLimitInformation,
                (&raw const limits).cast::<c_void>(),
                u32::try_from(size_of_val(&limits)).expect("job limit structure exceeds u32"),
            )
        }?;
        Ok(job)
    }

    pub fn assign_process(&self, process: &impl AsRawHandle) -> Result<(), RuntimeError> {
        self.assign_raw_process(process.as_raw_handle())
    }

    pub fn assign_raw_process(&self, process: RawHandle) -> Result<(), RuntimeError> {
        let process = HANDLE(process);
        unsafe { AssignProcessToJobObject(self.handle, process) }?;
        Ok(())
    }

    pub fn contains_process(&self, process: &impl AsRawHandle) -> Result<bool, RuntimeError> {
        let mut contained = BOOL::default();
        let process = HANDLE(process.as_raw_handle());
        unsafe { IsProcessInJob(process, Some(self.handle), &raw mut contained) }?;
        Ok(contained.as_bool())
    }

    pub fn contains_process_id(&self, process_id: u32) -> Result<bool, RuntimeError> {
        let process = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, process_id) }?;
        let process = ProcessHandle(process);
        let mut contained = BOOL::default();
        unsafe { IsProcessInJob(process.0, Some(self.handle), &raw mut contained) }?;
        Ok(contained.as_bool())
    }

    pub fn active_processes(&self) -> Result<u32, RuntimeError> {
        let mut accounting = JOBOBJECT_BASIC_ACCOUNTING_INFORMATION::default();
        unsafe {
            QueryInformationJobObject(
                Some(self.handle),
                JobObjectBasicAccountingInformation,
                (&raw mut accounting).cast::<c_void>(),
                u32::try_from(size_of_val(&accounting))
                    .expect("job accounting structure exceeds u32"),
                None,
            )
        }?;
        Ok(accounting.ActiveProcesses)
    }

    pub fn terminate(&self, exit_code: u32) -> Result<(), RuntimeError> {
        unsafe { TerminateJobObject(self.handle, exit_code) }?;
        Ok(())
    }
}

unsafe impl Send for JobObject {}
unsafe impl Sync for JobObject {}

impl Drop for JobObject {
    fn drop(&mut self) {
        unsafe {
            let _ = CloseHandle(self.handle);
        }
    }
}

#[derive(Debug)]
pub struct LaunchGate {
    name: String,
    handle: HANDLE,
}

impl LaunchGate {
    pub fn create() -> Result<Self, RuntimeError> {
        let name = format!("Local\\Lazoo.SessionGate.{}", Uuid::new_v4());
        let wide_name = HSTRING::from(&name);
        let handle = unsafe { CreateEventW(None, false, false, &wide_name) }?;
        Ok(Self { name, handle })
    }

    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn signal(&self) -> Result<(), RuntimeError> {
        unsafe { SetEvent(self.handle) }?;
        Ok(())
    }
}

unsafe impl Send for LaunchGate {}
unsafe impl Sync for LaunchGate {}

impl Drop for LaunchGate {
    fn drop(&mut self) {
        unsafe {
            let _ = CloseHandle(self.handle);
        }
    }
}

pub fn wait_for_launch_gate(name: &str) -> Result<(), RuntimeError> {
    let wide_name = HSTRING::from(name);
    let handle = unsafe { OpenEventW(SYNCHRONIZATION_SYNCHRONIZE, false, &wide_name) }?;
    let guard = EventHandle(handle);
    let result = unsafe { WaitForSingleObject(guard.0, INFINITE) };
    if result != WAIT_OBJECT_0 {
        return Err(RuntimeError::Windows(windows::core::Error::from_thread()));
    }
    Ok(())
}

struct EventHandle(HANDLE);

impl Drop for EventHandle {
    fn drop(&mut self) {
        unsafe {
            let _ = CloseHandle(self.0);
        }
    }
}

struct ProcessHandle(HANDLE);

impl Drop for ProcessHandle {
    fn drop(&mut self) {
        unsafe {
            let _ = CloseHandle(self.0);
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct JobCommand {
    executable: PathBuf,
    args: Vec<OsString>,
    cwd: PathBuf,
    env: BTreeMap<OsString, OsString>,
}

impl JobCommand {
    #[must_use]
    pub fn new(executable: impl Into<PathBuf>, cwd: impl Into<PathBuf>) -> Self {
        Self {
            executable: executable.into(),
            args: Vec::new(),
            cwd: cwd.into(),
            env: BTreeMap::new(),
        }
    }

    #[must_use]
    pub fn arg(mut self, value: impl Into<OsString>) -> Self {
        self.args.push(value.into());
        self
    }

    #[must_use]
    pub fn args(mut self, values: impl IntoIterator<Item = impl Into<OsString>>) -> Self {
        self.args.extend(values.into_iter().map(Into::into));
        self
    }

    #[must_use]
    pub fn env(mut self, key: impl Into<OsString>, value: impl Into<OsString>) -> Self {
        self.env.insert(key.into(), value.into());
        self
    }

    #[must_use]
    pub fn envs(
        mut self,
        values: impl IntoIterator<Item = (impl Into<OsString>, impl Into<OsString>)>,
    ) -> Self {
        self.env.extend(
            values
                .into_iter()
                .map(|(key, value)| (key.into(), value.into())),
        );
        self
    }
}

#[derive(Debug)]
pub struct JobChild {
    job: Arc<JobObject>,
    _gate: LaunchGate,
    child: Child,
}

impl JobChild {
    pub fn spawn(command: &JobCommand) -> Result<Self, RuntimeError> {
        let host = session_host_path()?;
        Self::spawn_with_host(&host, command)
    }

    pub fn spawn_with_host(host: &Path, command: &JobCommand) -> Result<Self, RuntimeError> {
        let gate = LaunchGate::create()?;
        let job = Arc::new(JobObject::kill_on_close()?);
        let mut process = Command::new(host);
        process
            .arg("--gate")
            .arg(gate.name())
            .arg("--")
            .arg(&command.executable)
            .args(&command.args)
            .current_dir(&command.cwd)
            .envs(&command.env)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        let mut child = process.spawn().map_err(|source| RuntimeError::Io {
            path: host.to_path_buf(),
            source,
        })?;
        let raw_handle = child.raw_handle().ok_or_else(|| RuntimeError::Io {
            path: host.to_path_buf(),
            source: std::io::Error::other("session host process handle is unavailable"),
        })?;
        if let Err(error) = job.assign_raw_process(raw_handle) {
            let _ = child.start_kill();
            return Err(error);
        }
        if let Err(error) = gate.signal() {
            let _ = job.terminate(92);
            let _ = child.start_kill();
            return Err(error);
        }
        Ok(Self {
            job,
            _gate: gate,
            child,
        })
    }

    #[must_use]
    pub fn id(&self) -> Option<u32> {
        self.child.id()
    }

    #[must_use]
    pub fn job(&self) -> Arc<JobObject> {
        Arc::clone(&self.job)
    }

    pub fn take_stdin(&mut self) -> Option<ChildStdin> {
        self.child.stdin.take()
    }

    pub fn take_stdout(&mut self) -> Option<ChildStdout> {
        self.child.stdout.take()
    }

    pub fn take_stderr(&mut self) -> Option<ChildStderr> {
        self.child.stderr.take()
    }

    pub fn try_wait(&mut self) -> Result<Option<ExitStatus>, std::io::Error> {
        self.child.try_wait()
    }

    pub async fn wait(&mut self) -> Result<ExitStatus, std::io::Error> {
        self.child.wait().await
    }

    pub fn terminate_tree(&self, exit_code: u32) -> Result<(), RuntimeError> {
        self.job.terminate(exit_code)
    }
}

pub fn session_host_path() -> Result<PathBuf, RuntimeError> {
    let current = std::env::current_exe().map_err(|source| RuntimeError::Io {
        path: PathBuf::from("current executable"),
        source,
    })?;
    let directory = current.parent().ok_or_else(|| RuntimeError::Io {
        path: current.clone(),
        source: std::io::Error::new(
            std::io::ErrorKind::NotFound,
            "current executable has no parent directory",
        ),
    })?;
    let filename = OsStr::new("lazoo-session-host.exe");
    let sibling = directory.join(filename);
    if sibling.is_file() {
        return Ok(sibling);
    }
    if directory.file_name() == Some(OsStr::new("deps"))
        && let Some(parent) = directory.parent()
    {
        let test_sibling = parent.join(filename);
        if test_sibling.is_file() {
            return Ok(test_sibling);
        }
    }
    Err(RuntimeError::Io {
        path: sibling,
        source: std::io::Error::new(
            std::io::ErrorKind::NotFound,
            "lazoo-session-host.exe is not beside the current executable",
        ),
    })
}
