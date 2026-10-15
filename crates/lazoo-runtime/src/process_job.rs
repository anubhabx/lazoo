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
        Foundation::{CloseHandle, FILETIME, HANDLE, WAIT_OBJECT_0},
        System::{
            JobObjects::{
                AssignProcessToJobObject, CreateJobObjectW, IsProcessInJob,
                JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE, JOBOBJECT_BASIC_ACCOUNTING_INFORMATION,
                JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JobObjectBasicAccountingInformation,
                JobObjectExtendedLimitInformation, QueryInformationJobObject,
                SetInformationJobObject, TerminateJobObject,
            },
            ProcessStatus::{K32GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS},
            Threading::{
                CreateEventW, GetCurrentProcess, GetProcessHandleCount, GetProcessTimes, INFINITE,
                OpenEventW, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION,
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
        Ok(self.accounting()?.active_processes)
    }

    /// The job's accounting so far: processes, CPU time, and peak memory, kept by Windows for
    /// the whole tree and surviving its termination until the job handle closes.
    pub fn accounting(&self) -> Result<JobAccounting, RuntimeError> {
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
        let mut limits = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
        unsafe {
            QueryInformationJobObject(
                Some(self.handle),
                JobObjectExtendedLimitInformation,
                (&raw mut limits).cast::<c_void>(),
                u32::try_from(size_of_val(&limits)).expect("job limit structure exceeds u32"),
                None,
            )
        }?;
        Ok(JobAccounting {
            active_processes: accounting.ActiveProcesses,
            total_processes: accounting.TotalProcesses,
            user_cpu_millis: hundred_nanos_to_millis(accounting.TotalUserTime),
            kernel_cpu_millis: hundred_nanos_to_millis(accounting.TotalKernelTime),
            peak_process_memory_bytes: bytes(limits.PeakProcessMemoryUsed),
            peak_job_memory_bytes: bytes(limits.PeakJobMemoryUsed),
        })
    }

    pub fn terminate(&self, exit_code: u32) -> Result<(), RuntimeError> {
        unsafe { TerminateJobObject(self.handle, exit_code) }?;
        Ok(())
    }
}

unsafe impl Send for JobObject {}
unsafe impl Sync for JobObject {}

/// Point-in-time accounting for one job: an employee's native process tree. Lazoo displays it
/// and never counts it against its own ceiling (brief section 20).
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct JobAccounting {
    pub active_processes: u32,
    pub total_processes: u32,
    pub user_cpu_millis: u64,
    pub kernel_cpu_millis: u64,
    pub peak_process_memory_bytes: u64,
    pub peak_job_memory_bytes: u64,
}

/// Point-in-time accounting for the calling process: the daemon's own share of the machine.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct ProcessAccounting {
    pub pid: u32,
    pub user_cpu_millis: u64,
    pub kernel_cpu_millis: u64,
    pub working_set_bytes: u64,
    pub peak_working_set_bytes: u64,
    pub handle_count: u32,
}

/// CPU time, working set, and handle count of the calling process.
pub fn current_process_accounting() -> Result<ProcessAccounting, RuntimeError> {
    let process = unsafe { GetCurrentProcess() };
    let mut creation = FILETIME::default();
    let mut exit = FILETIME::default();
    let mut kernel = FILETIME::default();
    let mut user = FILETIME::default();
    unsafe {
        GetProcessTimes(
            process,
            &raw mut creation,
            &raw mut exit,
            &raw mut kernel,
            &raw mut user,
        )
    }?;
    let mut counters = PROCESS_MEMORY_COUNTERS::default();
    let size = u32::try_from(size_of_val(&counters)).expect("memory counters exceed u32");
    counters.cb = size;
    unsafe { K32GetProcessMemoryInfo(process, &raw mut counters, size) }.ok()?;
    let mut handle_count = 0u32;
    unsafe { GetProcessHandleCount(process, &raw mut handle_count) }?;
    Ok(ProcessAccounting {
        pid: std::process::id(),
        user_cpu_millis: filetime_to_millis(user),
        kernel_cpu_millis: filetime_to_millis(kernel),
        working_set_bytes: bytes(counters.WorkingSetSize),
        peak_working_set_bytes: bytes(counters.PeakWorkingSetSize),
        handle_count,
    })
}

fn filetime_to_millis(value: FILETIME) -> u64 {
    ((u64::from(value.dwHighDateTime) << 32) | u64::from(value.dwLowDateTime)) / 10_000
}

fn hundred_nanos_to_millis(value: i64) -> u64 {
    u64::try_from(value).unwrap_or(0) / 10_000
}

fn bytes(value: usize) -> u64 {
    u64::try_from(value).unwrap_or(u64::MAX)
}

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
