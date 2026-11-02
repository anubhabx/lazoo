//! Private `ConPTY` transport for the daemon's terminal protocol.  This module deliberately has
//! no persistence or tracing of terminal bytes: callers own any audit policy above this layer.

use std::{
    cmp::Ordering,
    collections::VecDeque,
    ffi::{OsStr, OsString, c_void},
    fmt,
    os::windows::ffi::OsStrExt,
    path::PathBuf,
    sync::{
        Arc, Mutex, Weak,
        atomic::{AtomicBool, Ordering as AtomicOrdering},
        mpsc::{self, SyncSender, TrySendError},
    },
    thread::{self, JoinHandle},
};

use thiserror::Error;
use uuid::Uuid;
use windows::{
    Win32::{
        Foundation::{
            CloseHandle, DUPLICATE_SAME_ACCESS, DuplicateHandle, HANDLE, STILL_ACTIVE,
            WAIT_OBJECT_0, WAIT_TIMEOUT,
        },
        Storage::FileSystem::{ReadFile, WriteFile},
        System::{
            Console::{COORD, ClosePseudoConsole, CreatePseudoConsole, HPCON, ResizePseudoConsole},
            IO::CancelSynchronousIo,
            JobObjects::{
                AssignProcessToJobObject, CreateJobObjectW, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
                JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JobObjectExtendedLimitInformation,
                SetInformationJobObject, TerminateJobObject,
            },
            Pipes::CreatePipe,
            Threading::{
                CREATE_SUSPENDED, CREATE_UNICODE_ENVIRONMENT, CreateProcessW,
                DeleteProcThreadAttributeList, EXTENDED_STARTUPINFO_PRESENT, GetCurrentProcess,
                GetCurrentThread, GetExitCodeProcess, InitializeProcThreadAttributeList,
                LPPROC_THREAD_ATTRIBUTE_LIST, PROC_THREAD_ATTRIBUTE_PSEUDOCONSOLE,
                PROCESS_INFORMATION, ResumeThread, STARTF_USESTDHANDLES, STARTUPINFOEXW,
                TerminateProcess, UpdateProcThreadAttribute, WaitForSingleObject,
            },
        },
    },
    core::{PCWSTR, PWSTR},
};

use crate::{JobAccounting, RuntimeError, process_job::job_accounting};

const MAX_DIMENSION: u16 = 1000;
const MAX_INPUT: usize = 16 * 1024;
const MAX_READ: usize = 64 * 1024;
const OUTPUT_CAPACITY: usize = 1024 * 1024;
const INPUT_QUEUE_CAPACITY: usize = 32;
const SHUTDOWN_WAIT_MS: u32 = 2_000;

type EnvironmentEntry = (Vec<u16>, Vec<u16>);

#[derive(Clone, Eq, PartialEq)]
pub struct TerminalCommand {
    executable: PathBuf,
    arguments: Vec<OsString>,
    cwd: PathBuf,
    cols: u16,
    rows: u16,
    environment: Vec<(OsString, OsString)>,
    environment_remove: Vec<OsString>,
}

impl fmt::Debug for TerminalCommand {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("TerminalCommand")
            .field("executable", &self.executable)
            .field("arguments", &self.arguments)
            .field("cwd", &self.cwd)
            .field("cols", &self.cols)
            .field("rows", &self.rows)
            .field("environment_remove", &self.environment_remove)
            .field(
                "environment",
                &self
                    .environment
                    .iter()
                    .map(|(name, _)| (name, "<redacted>"))
                    .collect::<Vec<_>>(),
            )
            .finish()
    }
}

impl TerminalCommand {
    pub fn new(
        executable: impl Into<PathBuf>,
        cwd: impl Into<PathBuf>,
        cols: u16,
        rows: u16,
    ) -> Self {
        Self {
            executable: executable.into(),
            arguments: Vec::new(),
            cwd: cwd.into(),
            cols,
            rows,
            environment: Vec::new(),
            environment_remove: Vec::new(),
        }
    }
    #[must_use]
    pub fn arg(mut self, argument: impl Into<OsString>) -> Self {
        self.arguments.push(argument.into());
        self
    }
    #[must_use]
    pub fn args(mut self, arguments: impl IntoIterator<Item = impl Into<OsString>>) -> Self {
        self.arguments.extend(arguments.into_iter().map(Into::into));
        self
    }
    /// Adds an internal-only environment override for this spawned process.
    /// Values are never exposed through terminal metadata or `Debug` output.
    #[must_use]
    pub fn env(mut self, name: impl Into<OsString>, value: impl Into<OsString>) -> Self {
        self.environment.push((name.into(), value.into()));
        self
    }

    #[must_use]
    pub fn env_remove(mut self, name: impl Into<OsString>) -> Self {
        self.environment_remove.push(name.into());
        self
    }
    #[must_use]
    pub fn executable(&self) -> &std::path::Path {
        &self.executable
    }
    #[must_use]
    pub fn cwd(&self) -> &std::path::Path {
        &self.cwd
    }
}

#[derive(Debug, Error)]
pub enum TerminalError {
    #[error("terminal dimensions must be in 1..={MAX_DIMENSION}")]
    InvalidDimensions,
    #[error("terminal environment names cannot contain NUL or '=' and values cannot contain NUL")]
    InvalidEnvironment,
    #[error("terminal input exceeds {MAX_INPUT} bytes")]
    InputTooLarge,
    #[error("terminal output request exceeds {MAX_READ} bytes")]
    ReadTooLarge,
    #[error("terminal input queue is full")]
    InputBackpressure,
    #[error("terminal is stopped")]
    Stopped,
    #[error("terminal process did not exit within {SHUTDOWN_WAIT_MS}ms")]
    ShutdownTimeout,
    #[error("Windows operation failed: {0}")]
    Windows(#[from] windows::core::Error),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TerminalOutput {
    pub start: u64,
    pub end: u64,
    pub bytes: Vec<u8>,
    pub gap: bool,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TerminalExitStatus {
    pub live: bool,
    pub exit_code: Option<u32>,
}

#[derive(Debug)]
pub struct TerminalSession {
    inner: Arc<Inner>,
}
/// Capability-restricted view of a terminal. It intentionally cannot write input or stop the
/// process tree, so callers may safely expose it for read-only native views.
#[derive(Debug)]
pub struct TerminalView {
    inner: Weak<Inner>,
    id: Uuid,
    pid: u32,
}
#[derive(Debug)]
struct Inner {
    id: Uuid,
    pid: u32,
    process: HANDLE,
    job: HANDLE,
    pty: Mutex<Option<PseudoConsole>>,
    input: Mutex<Option<SyncSender<Vec<u8>>>>,
    output: Arc<Mutex<OutputRing>>,
    writer_thread: OwnedHandle,
    workers: Mutex<Vec<JoinHandle<()>>>,
    exit_recorded: AtomicBool,
}
// Windows handles can be used from multiple threads. Mutable pseudo-console state is locked.
unsafe impl Send for Inner {}
unsafe impl Sync for Inner {}

impl Inner {
    fn shutdown(&self) -> Result<(), TerminalError> {
        tracing::warn!(terminal_id = %self.id, pid = self.pid,
            reason = "terminal-job-stop-or-drop", requested_exit_code = 1,
            "terminating terminal process job");
        self.input
            .lock()
            .expect("terminal input mutex poisoned")
            .take();
        unsafe {
            let _ = TerminateJobObject(self.job, 1);
            let _ = CancelSynchronousIo(self.writer_thread.0);
        }
        let wait = unsafe { WaitForSingleObject(self.process, SHUTDOWN_WAIT_MS) };
        if wait == WAIT_TIMEOUT {
            return Err(TerminalError::ShutdownTimeout);
        }
        if wait != WAIT_OBJECT_0 {
            return Err(TerminalError::Windows(windows::core::Error::from_thread()));
        }
        let mut code = 0;
        match unsafe { GetExitCodeProcess(self.process, &raw mut code) } {
            Ok(()) => self.record_exit(code),
            Err(error) => tracing::warn!(terminal_id = %self.id, pid = self.pid,
                reason = "terminal-exit-status-query-failed", os_error = error.code().0,
                "terminal exit status unavailable"),
        }
        // Closing ConPTY after termination closes its pipe ends; the reader keeps draining until EOF.
        if let Some(pty) = self.pty.lock().expect("terminal pty mutex poisoned").take() {
            drop(pty);
        }
        for worker in self
            .workers
            .lock()
            .expect("terminal workers mutex poisoned")
            .drain(..)
        {
            let _ = worker.join();
        }
        Ok(())
    }

    fn record_exit(&self, code: u32) {
        if !self.exit_recorded.swap(true, AtomicOrdering::AcqRel) {
            tracing::warn!(terminal_id = %self.id, pid = self.pid,
                reason = "terminal-process-exit", exit_code = code,
                "terminal child exit observed");
        }
    }
}

impl Drop for Inner {
    fn drop(&mut self) {
        let _ = self.shutdown();
        unsafe {
            let _ = CloseHandle(self.process);
            let _ = CloseHandle(self.job);
        }
    }
}

impl TerminalSession {
    pub fn spawn(command: TerminalCommand) -> Result<Self, TerminalError> {
        let TerminalCommand {
            executable,
            arguments,
            cwd,
            cols,
            rows,
            environment,
            environment_remove,
        } = command;
        validate_dimensions(cols, rows)?;
        let environment = environment_block(&environment, &environment_remove)?;
        // These are the application-facing ends.  ConPTY owns the opposite ends after creation.
        let (input_read, input_write) = pipe()?;
        let (output_read, output_write) = pipe()?;
        let pty = PseudoConsole(unsafe {
            CreatePseudoConsole(
                COORD {
                    X: i16::try_from(cols).expect("validated terminal columns fit i16"),
                    Y: i16::try_from(rows).expect("validated terminal rows fit i16"),
                },
                input_read.0,
                output_write.0,
                0,
            )
        }?);
        let attribute_list = AttributeList::new(pty.0)?;
        let mut startup = STARTUPINFOEXW::default();
        startup.StartupInfo.cb =
            u32::try_from(size_of::<STARTUPINFOEXW>()).expect("STARTUPINFOEXW size fits u32");
        startup.lpAttributeList = attribute_list.0;
        // Do not let an inherited parent console bypass ConPTY (notably under test runners).
        startup.StartupInfo.hStdInput = HANDLE::default();
        startup.StartupInfo.hStdOutput = HANDLE::default();
        startup.StartupInfo.hStdError = HANDLE::default();
        startup.StartupInfo.dwFlags |= STARTF_USESTDHANDLES;
        let mut process_info = PROCESS_INFORMATION::default();
        let application = wide(executable.as_os_str());
        let cwd = wide(cwd.as_os_str());
        let mut line = command_line(&executable, &arguments);
        unsafe {
            CreateProcessW(
                PCWSTR(application.as_ptr()),
                Some(PWSTR(line.as_mut_ptr())),
                None,
                None,
                false,
                CREATE_SUSPENDED | CREATE_UNICODE_ENVIRONMENT | EXTENDED_STARTUPINFO_PRESENT,
                Some(environment.as_ptr().cast()),
                PCWSTR(cwd.as_ptr()),
                (&raw const startup).cast(),
                &raw mut process_info,
            )?;
        }
        // ConPTY requires these ends to stay open through CreateProcessW.
        drop(input_read);
        drop(output_write);
        let process = OwnedHandle(process_info.hProcess);
        let thread = OwnedHandle(process_info.hThread);
        let job = match create_kill_on_close_job() {
            Ok(job) => job,
            Err(error) => {
                terminate_and_wait(process.0);
                return Err(error);
            }
        };
        if let Err(error) = unsafe { AssignProcessToJobObject(job.0, process.0) } {
            terminate_and_wait(process.0);
            return Err(error.into());
        }
        // The process cannot run until containment has succeeded.
        if unsafe { ResumeThread(thread.0) } == u32::MAX {
            let error = windows::core::Error::from_thread();
            terminate_and_wait(process.0);
            return Err(error.into());
        }

        let output = Arc::new(Mutex::new(OutputRing::new(OUTPUT_CAPACITY)));
        let (tx, rx) = mpsc::sync_channel(INPUT_QUEUE_CAPACITY);
        let output_for_reader = Arc::clone(&output);
        let reader = thread::spawn(move || read_worker(output_read, output_for_reader));
        let (writer_ready, writer_thread) = mpsc::sync_channel(1);
        let writer = thread::spawn(move || write_worker(input_write, rx, writer_ready));
        let writer_thread = writer_thread.recv().map_err(|_| TerminalError::Stopped)?;
        Ok(Self {
            inner: Arc::new(Inner {
                exit_recorded: AtomicBool::new(false),
                id: Uuid::new_v4(),
                pid: process_info.dwProcessId,
                process: process.into_raw(),
                job: job.into_raw(),
                pty: Mutex::new(Some(pty)),
                input: Mutex::new(Some(tx)),
                output,
                writer_thread,
                workers: Mutex::new(vec![reader, writer]),
            }),
        })
    }
    #[must_use]
    pub fn id(&self) -> Uuid {
        self.inner.id
    }
    #[must_use]
    pub fn pid(&self) -> u32 {
        self.inner.pid
    }
    #[must_use]
    pub fn view(&self) -> TerminalView {
        TerminalView {
            inner: Arc::downgrade(&self.inner),
            id: self.inner.id,
            pid: self.inner.pid,
        }
    }
    pub fn resize(&self, cols: u16, rows: u16) -> Result<(), TerminalError> {
        validate_dimensions(cols, rows)?;
        let guard = self.inner.pty.lock().expect("terminal pty mutex poisoned");
        let pty = guard.as_ref().ok_or(TerminalError::Stopped)?;
        unsafe {
            ResizePseudoConsole(
                pty.0,
                COORD {
                    X: i16::try_from(cols).expect("validated terminal columns fit i16"),
                    Y: i16::try_from(rows).expect("validated terminal rows fit i16"),
                },
            )?;
        }
        Ok(())
    }
    pub fn write_input(&self, bytes: impl AsRef<[u8]>) -> Result<(), TerminalError> {
        let bytes = bytes.as_ref();
        if bytes.len() > MAX_INPUT {
            return Err(TerminalError::InputTooLarge);
        }
        let guard = self
            .inner
            .input
            .lock()
            .expect("terminal input mutex poisoned");
        let sender = guard.as_ref().ok_or(TerminalError::Stopped)?;
        sender
            .try_send(bytes.to_vec())
            .map_err(|error| match error {
                TrySendError::Full(_) => TerminalError::InputBackpressure,
                TrySendError::Disconnected(_) => TerminalError::Stopped,
            })
    }
    pub fn read_output(
        &self,
        after: u64,
        max_bytes: usize,
    ) -> Result<TerminalOutput, TerminalError> {
        if max_bytes > MAX_READ {
            return Err(TerminalError::ReadTooLarge);
        }
        Ok(self
            .inner
            .output
            .lock()
            .expect("terminal output mutex poisoned")
            .read(after, max_bytes))
    }
    pub fn exit_status(&self) -> Result<TerminalExitStatus, TerminalError> {
        let mut code = 0;
        unsafe {
            GetExitCodeProcess(self.inner.process, &raw mut code)?;
        }
        let active = STILL_ACTIVE.0 as u32;
        if code != active {
            self.inner.record_exit(code);
        }
        Ok(TerminalExitStatus {
            live: code == active,
            exit_code: (code != active).then_some(code),
        })
    }
    /// Accounting for this terminal's contained native process tree. This borrows the Job Object
    /// handle and never alters terminal/process ownership or lifecycle.
    pub fn resources(&self) -> Result<JobAccounting, RuntimeError> {
        job_accounting(self.inner.job)
    }
    pub fn stop(&self) -> Result<(), TerminalError> {
        self.shutdown()
    }
    fn shutdown(&self) -> Result<(), TerminalError> {
        self.inner.shutdown()
    }
}
impl Clone for TerminalSession {
    fn clone(&self) -> Self {
        Self {
            inner: Arc::clone(&self.inner),
        }
    }
}
impl TerminalView {
    #[must_use]
    pub fn id(&self) -> Uuid {
        self.id
    }
    #[must_use]
    pub fn pid(&self) -> u32 {
        self.pid
    }
    pub fn resize(&self, cols: u16, rows: u16) -> Result<(), TerminalError> {
        TerminalSession {
            inner: self.inner.upgrade().ok_or(TerminalError::Stopped)?,
        }
        .resize(cols, rows)
    }
    pub fn read_output(
        &self,
        after: u64,
        max_bytes: usize,
    ) -> Result<TerminalOutput, TerminalError> {
        TerminalSession {
            inner: self.inner.upgrade().ok_or(TerminalError::Stopped)?,
        }
        .read_output(after, max_bytes)
    }
    pub fn exit_status(&self) -> Result<TerminalExitStatus, TerminalError> {
        TerminalSession {
            inner: self.inner.upgrade().ok_or(TerminalError::Stopped)?,
        }
        .exit_status()
    }
}
impl Clone for TerminalView {
    fn clone(&self) -> Self {
        Self {
            inner: Weak::clone(&self.inner),
            id: self.id,
            pid: self.pid,
        }
    }
}
fn validate_dimensions(cols: u16, rows: u16) -> Result<(), TerminalError> {
    if cols == 0 || rows == 0 || cols > MAX_DIMENSION || rows > MAX_DIMENSION {
        Err(TerminalError::InvalidDimensions)
    } else {
        Ok(())
    }
}

fn environment_block(
    overrides: &[(OsString, OsString)],
    removals: &[OsString],
) -> Result<Vec<u16>, TerminalError> {
    let mut environment = inherited_environment()?;
    for name in removals {
        let name = name.encode_wide().collect::<Vec<_>>();
        if name.is_empty()
            || name
                .iter()
                .any(|unit| *unit == 0 || *unit == u16::from(b'='))
        {
            return Err(TerminalError::InvalidEnvironment);
        }
        environment
            .retain(|(existing, _)| environment_name_cmp(existing, &name) != Ordering::Equal);
    }
    for (name, value) in overrides {
        let name = name.encode_wide().collect::<Vec<_>>();
        let value = value.encode_wide().collect::<Vec<_>>();
        if name.is_empty()
            || name
                .iter()
                .any(|unit| *unit == 0 || *unit == u16::from(b'='))
            || value.contains(&0)
        {
            return Err(TerminalError::InvalidEnvironment);
        }
        if let Some(entry) = environment
            .iter_mut()
            .find(|(existing, _)| environment_name_cmp(existing, &name) == Ordering::Equal)
        {
            *entry = (name, value);
        } else {
            environment.push((name, value));
        }
    }
    environment.sort_by(|(left, _), (right, _)| environment_name_cmp(left, right));
    Ok(serialize_environment(environment))
}

fn serialize_environment(entries: impl IntoIterator<Item = EnvironmentEntry>) -> Vec<u16> {
    let mut block = Vec::new();
    for (name, value) in entries {
        block.extend(name);
        block.push(u16::from(b'='));
        block.extend(value);
        block.push(0);
    }
    // A Windows Unicode environment block ends with an empty entry. Empty blocks need two NULs.
    if block.is_empty() {
        block.push(0);
    }
    block.push(0);
    block
}

fn inherited_environment() -> Result<Vec<EnvironmentEntry>, TerminalError> {
    let raw = unsafe { GetEnvironmentStringsW() };
    if raw.is_null() {
        return Err(TerminalError::Windows(windows::core::Error::from_thread()));
    }
    let mut environment = Vec::new();
    let mut start = raw;
    unsafe {
        loop {
            let mut end = start;
            while *end != 0 {
                end = end.add(1);
            }
            if end == start {
                break;
            }
            let entry = std::slice::from_raw_parts(start, end.offset_from(start).cast_unsigned());
            if let Some((name, value)) = split_environment_entry(entry) {
                environment.push((name.to_vec(), value.to_vec()));
            }
            start = end.add(1);
        }
        let _ = FreeEnvironmentStringsW(raw);
    }
    Ok(environment)
}

fn split_environment_entry(entry: &[u16]) -> Option<(&[u16], &[u16])> {
    let equals = if entry.first() == Some(&u16::from(b'=')) {
        entry[1..]
            .iter()
            .position(|unit| *unit == u16::from(b'='))
            .map(|offset| offset + 1)
    } else {
        entry.iter().position(|unit| *unit == u16::from(b'='))
    }?;
    Some((&entry[..equals], &entry[equals + 1..]))
}

fn environment_name_cmp(left: &[u16], right: &[u16]) -> Ordering {
    let result = unsafe {
        CompareStringOrdinal(
            left.as_ptr(),
            i32::try_from(left.len()).expect("Windows environment name fits i32"),
            right.as_ptr(),
            i32::try_from(right.len()).expect("Windows environment name fits i32"),
            1,
        )
    };
    match result {
        1 => Ordering::Less,
        2 => Ordering::Equal,
        3 => Ordering::Greater,
        _ => left.cmp(right),
    }
}

#[link(name = "kernel32")]
unsafe extern "system" {
    fn CompareStringOrdinal(
        left: *const u16,
        left_len: i32,
        right: *const u16,
        right_len: i32,
        ignore_case: i32,
    ) -> i32;
    fn GetEnvironmentStringsW() -> *mut u16;
    fn FreeEnvironmentStringsW(environment: *mut u16) -> i32;
}

fn wide(value: &OsStr) -> Vec<u16> {
    value.encode_wide().chain(Some(0)).collect()
}
fn command_line(executable: &std::path::Path, arguments: &[OsString]) -> Vec<u16> {
    let mut line = Vec::new();
    append_windows_argument(&mut line, executable.as_os_str());
    for argument in arguments {
        line.push(u16::from(b' '));
        append_windows_argument(&mut line, argument);
    }
    line.push(0);
    line
}
fn append_windows_argument(output: &mut Vec<u16>, argument: &OsStr) {
    let argument: Vec<u16> = argument.encode_wide().collect();
    if !argument.is_empty() && !argument.iter().any(|unit| matches!(*unit, 9 | 32 | 34)) {
        output.extend(argument);
        return;
    }
    output.push(u16::from(b'"'));
    let mut backslashes = 0;
    for unit in argument {
        if unit == u16::from(b'\\') {
            backslashes += 1;
            continue;
        }
        if unit == u16::from(b'"') {
            output.extend(std::iter::repeat_n(u16::from(b'\\'), backslashes * 2 + 1));
            output.push(unit);
        } else {
            output.extend(std::iter::repeat_n(u16::from(b'\\'), backslashes));
            output.push(unit);
        }
        backslashes = 0;
    }
    output.extend(std::iter::repeat_n(u16::from(b'\\'), backslashes * 2));
    output.push(u16::from(b'"'));
}
fn terminate_and_wait(process: HANDLE) {
    unsafe {
        let _ = TerminateProcess(process, 1);
        let _ = WaitForSingleObject(process, SHUTDOWN_WAIT_MS);
    }
}
fn pipe() -> Result<(OwnedHandle, OwnedHandle), TerminalError> {
    let mut read = HANDLE::default();
    let mut write = HANDLE::default();
    unsafe {
        CreatePipe(&raw mut read, &raw mut write, None, 0)?;
    }
    Ok((OwnedHandle(read), OwnedHandle(write)))
}
fn create_kill_on_close_job() -> Result<OwnedHandle, TerminalError> {
    let job = unsafe { CreateJobObjectW(None, None) }?;
    let job = OwnedHandle(job);
    let mut limits = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
    limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
    unsafe {
        SetInformationJobObject(
            job.0,
            JobObjectExtendedLimitInformation,
            (&raw const limits).cast::<c_void>(),
            u32::try_from(size_of_val(&limits)).expect("job limits size fits u32"),
        )?;
    }
    Ok(job)
}
fn read_worker(handle: OwnedHandle, ring: Arc<Mutex<OutputRing>>) {
    let mut buffer = [0_u8; 8192];
    loop {
        let mut read = 0;
        if unsafe { ReadFile(handle.0, Some(&mut buffer), Some(&raw mut read), None) }.is_err()
            || read == 0
        {
            break;
        }
        ring.lock()
            .expect("terminal output mutex poisoned")
            .push(&buffer[..read as usize]);
    }
    drop(ring);
    drop(handle);
}
fn write_worker(
    handle: OwnedHandle,
    receiver: mpsc::Receiver<Vec<u8>>,
    ready: mpsc::SyncSender<OwnedHandle>,
) {
    let Ok(thread) = duplicate_current_thread() else {
        return;
    };
    if ready.send(thread).is_err() {
        return;
    }
    drop(ready);
    for bytes in receiver {
        let mut written = 0;
        if unsafe { WriteFile(handle.0, Some(&bytes), Some(&raw mut written), None) }.is_err()
            || written != u32::try_from(bytes.len()).expect("terminal input is bounded to 16 KiB")
        {
            break;
        }
    }
    drop(handle);
}

fn duplicate_current_thread() -> Result<OwnedHandle, TerminalError> {
    let mut duplicate = HANDLE::default();
    unsafe {
        DuplicateHandle(
            GetCurrentProcess(),
            GetCurrentThread(),
            GetCurrentProcess(),
            &raw mut duplicate,
            0,
            false,
            DUPLICATE_SAME_ACCESS,
        )?;
    }
    Ok(OwnedHandle(duplicate))
}

#[derive(Debug)]
struct OwnedHandle(HANDLE);
// A HANDLE has no Rust aliasing semantics. Ownership transfers exactly once into each worker.
unsafe impl Send for OwnedHandle {}
impl OwnedHandle {
    fn into_raw(self) -> HANDLE {
        let handle = self.0;
        std::mem::forget(self);
        handle
    }
}
impl Drop for OwnedHandle {
    fn drop(&mut self) {
        unsafe {
            let _ = CloseHandle(self.0);
        }
    }
}
#[derive(Debug)]
struct PseudoConsole(HPCON);
impl Drop for PseudoConsole {
    fn drop(&mut self) {
        unsafe {
            ClosePseudoConsole(self.0);
        }
    }
}
#[derive(Debug)]
struct AttributeList(LPPROC_THREAD_ATTRIBUTE_LIST, #[allow(dead_code)] Vec<usize>);
impl AttributeList {
    fn new(pty: HPCON) -> Result<Self, TerminalError> {
        let mut bytes = 0;
        unsafe {
            let _ = InitializeProcThreadAttributeList(None, 1, None, &raw mut bytes);
        }
        let words = bytes.div_ceil(size_of::<usize>());
        let mut storage = vec![0usize; words];
        let list = LPPROC_THREAD_ATTRIBUTE_LIST(storage.as_mut_ptr().cast());
        unsafe {
            InitializeProcThreadAttributeList(Some(list), 1, None, &raw mut bytes)?;
            UpdateProcThreadAttribute(
                list,
                0,
                PROC_THREAD_ATTRIBUTE_PSEUDOCONSOLE as usize,
                // The attribute value is the opaque HPCON value itself, not an address of it.
                Some(pty.0 as *const c_void),
                size_of::<HPCON>(),
                None,
                None,
            )?;
        }
        Ok(Self(list, storage))
    }
}
impl Drop for AttributeList {
    fn drop(&mut self) {
        unsafe {
            DeleteProcThreadAttributeList(self.0);
        }
    }
}

#[derive(Debug)]
struct OutputRing {
    bytes: VecDeque<u8>,
    start: u64,
    end: u64,
    capacity: usize,
}
impl OutputRing {
    fn new(capacity: usize) -> Self {
        Self {
            bytes: VecDeque::new(),
            start: 0,
            end: 0,
            capacity,
        }
    }
    fn push(&mut self, incoming: &[u8]) {
        self.bytes.extend(incoming);
        self.end += incoming.len() as u64;
        while self.bytes.len() > self.capacity {
            self.bytes.pop_front();
            self.start += 1;
        }
    }
    fn read(&self, after: u64, max: usize) -> TerminalOutput {
        let gap = after < self.start;
        let start = after.max(self.start).min(self.end);
        let offset = usize::try_from(start - self.start).expect("output ring offset fits usize");
        let count = max.min(self.bytes.len().saturating_sub(offset));
        let bytes = self
            .bytes
            .iter()
            .skip(offset)
            .take(count)
            .copied()
            .collect::<Vec<_>>();
        TerminalOutput {
            start,
            end: start + bytes.len() as u64,
            bytes,
            gap,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{os::windows::ffi::OsStringExt, slice};
    use windows::Win32::{
        Foundation::{HLOCAL, LocalFree},
        UI::Shell::CommandLineToArgvW,
    };
    #[test]
    fn output_ring_reports_dropped_prefix() {
        let mut ring = OutputRing::new(3);
        ring.push(b"abcd");
        let output = ring.read(0, 3);
        assert!(output.gap);
        assert_eq!(output.start, 1);
        assert_eq!(output.bytes, b"bcd");
    }

    #[test]
    fn command_line_round_trips_quotes_backslashes_spaces_and_empty_values() {
        let executable = PathBuf::from(r"C:\Program Files\Lazoo\runner.exe");
        let arguments = [
            OsString::from("plain"),
            OsString::from("space value"),
            OsString::from("quote\"value"),
            OsString::from("trailing\\"),
            OsString::new(),
        ];
        let command_line = command_line(&executable, &arguments);
        let mut count = 0;
        let argv = unsafe { CommandLineToArgvW(PCWSTR(command_line.as_ptr()), &raw mut count) };
        assert!(!argv.is_null());
        let actual = unsafe { slice::from_raw_parts(argv, usize::try_from(count).unwrap()) }
            .iter()
            .map(|argument| {
                let mut length = 0;
                while unsafe { *argument.0.add(length) } != 0 {
                    length += 1;
                }
                OsString::from_wide(unsafe { slice::from_raw_parts(argument.0, length) })
            })
            .collect::<Vec<_>>();
        unsafe {
            let _ = LocalFree(Some(HLOCAL(argv.cast())));
        }
        assert_eq!(actual[0], executable.into_os_string());
        assert_eq!(&actual[1..], &arguments);
    }

    #[test]
    fn environment_serialization_double_terminates_empty_and_nonempty_blocks() {
        assert_eq!(serialize_environment(Vec::new()), vec![0, 0]);
        assert_eq!(
            serialize_environment(vec![(
                "NAME".encode_utf16().collect(),
                "value".encode_utf16().collect(),
            )]),
            "NAME=value\0\0".encode_utf16().collect::<Vec<_>>(),
        );
    }

    #[test]
    fn account_environment_removes_inherited_names_case_insensitively() {
        let block = environment_block(&[], &[OsString::from("pAtH")]).unwrap();
        let text = String::from_utf16_lossy(&block).to_lowercase();
        assert!(!text.split('\0').any(|entry| entry.starts_with("path=")));
        let block = environment_block(
            &[("PATH".into(), "selected-profile-path".into())],
            &["path".into()],
        )
        .unwrap();
        let text = String::from_utf16_lossy(&block);
        assert!(
            text.split('\0')
                .any(|entry| entry == "PATH=selected-profile-path")
        );
    }
}
