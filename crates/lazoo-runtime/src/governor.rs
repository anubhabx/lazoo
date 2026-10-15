//! The section 20 infrastructure resource-governor primitive.
//!
//! This module deliberately governs only explicitly enrolled Lazoo infrastructure mains.  Its
//! `SILENT_BREAKAWAY_OK` limit keeps native provider descendants out of this job; provider trees
//! remain owned and accounted for by their separate [`crate::process_job::JobObject`].

use std::{collections::VecDeque, ffi::c_void, time::Duration};

use thiserror::Error;
use uuid::Uuid;
use windows::{
    Win32::{
        Foundation::{
            CloseHandle, ERROR_ALREADY_EXISTS, ERROR_NOT_SUPPORTED, GetLastError, HANDLE, HLOCAL,
            LocalFree, WAIT_ABANDONED, WAIT_OBJECT_0,
        },
        Security::{
            Authorization::{
                ConvertStringSecurityDescriptorToSecurityDescriptorW, SDDL_REVISION_1,
            },
            PSECURITY_DESCRIPTOR, SECURITY_ATTRIBUTES,
        },
        System::{
            JobObjects::{
                AssignProcessToJobObject, CreateJobObjectW, JOB_OBJECT_CPU_RATE_CONTROL_ENABLE,
                JOB_OBJECT_CPU_RATE_CONTROL_HARD_CAP, JOB_OBJECT_LIMIT_SILENT_BREAKAWAY_OK,
                JOBOBJECT_BASIC_ACCOUNTING_INFORMATION, JOBOBJECT_CPU_RATE_CONTROL_INFORMATION,
                JOBOBJECT_CPU_RATE_CONTROL_INFORMATION_0, JOBOBJECT_EXTENDED_LIMIT_INFORMATION,
                JobObjectBasicAccountingInformation, JobObjectCpuRateControlInformation,
                JobObjectExtendedLimitInformation, QueryInformationJobObject,
                SetInformationJobObject,
            },
            Threading::{
                CreateMutexW, GetCurrentProcess, INFINITE, OpenProcess,
                PROCESS_QUERY_LIMITED_INFORMATION, ReleaseMutex, WaitForSingleObject,
            },
        },
    },
    core::HSTRING,
};

use crate::{OwnerSecurity, RuntimeError};

/// The enforced CPU rate, expressed in Windows' cycles-per-10,000 unit.
pub const INFRASTRUCTURE_CPU_RATE: u32 = 2_000;
/// The soft threshold at which a caller should stop admitting background work.
pub const BACKGROUND_THROTTLE_PERCENT: u32 = 15;
const SHORT_WINDOW: Duration = Duration::from_secs(30);
const LONG_WINDOW: Duration = Duration::from_mins(5);
// The sampler coalesces to one point per elapsed second. Capacity includes the one point retained
// immediately before a window boundary for interpolation.
const SHORT_SAMPLE_CAPACITY: usize = 32;
const LONG_SAMPLE_CAPACITY: usize = 302;

/// A Windows Job Object shared by the daemon, keeper, and CLI for one installation.
///
/// Dropping a handle only closes that caller's handle. This job intentionally has no
/// `KILL_ON_JOB_CLOSE` limit, so one infrastructure process exiting cannot kill the others.
#[derive(Debug)]
pub struct InfrastructureGovernor {
    handle: HANDLE,
    name: String,
}

// A Job Object handle refers to a kernel object whose query operations are synchronized by
// Windows. This wrapper never mutates policy after construction and closes its one owned handle.
unsafe impl Send for InfrastructureGovernor {}
unsafe impl Sync for InfrastructureGovernor {}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CpuCapStatus {
    /// Windows confirmed `ENABLE | HARD_CAP` at 20 percent.
    HardCap20Percent,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct InfrastructureSample {
    pub active_processes: u32,
    pub total_processes: u32,
    pub user_cpu_millis: u64,
    pub kernel_cpu_millis: u64,
    pub peak_process_memory_bytes: u64,
    pub peak_job_memory_bytes: u64,
}

#[derive(Debug, Error)]
pub enum GovernorError {
    #[error("infrastructure resource governor requires Windows Job Object CPU-rate control")]
    WindowsUnsupported,
    #[error(
        "the existing infrastructure job is not the required 20% hard-cap silent-breakaway job"
    )]
    ConfigurationMismatch,
    #[error("Windows operation failed: {0}")]
    Windows(#[from] windows::core::Error),
    #[error("installation identifier must be a UUID")]
    InvalidInstallationId,
    #[error("could not determine the current Windows owner for the infrastructure job")]
    OwnerLookup,
}

impl InfrastructureGovernor {
    /// Creates or opens this installation's protected named job, verifies its policy, and
    /// explicitly enrolls the calling infrastructure main.
    pub fn attach_current(
        installation_id: impl AsRef<str>,
    ) -> Result<(Self, CpuCapStatus, InfrastructureSample), GovernorError> {
        let installation_id = Uuid::parse_str(installation_id.as_ref())
            .map_err(|_| GovernorError::InvalidInstallationId)?;
        let name = format!("Local\\Lazoo.Infrastructure.{installation_id}");
        let initialization_name = format!("Local\\Lazoo.Infrastructure.Init.{installation_id}");
        let wide_name = HSTRING::from(&name);
        let owner = OwnerSecurity::current().map_err(runtime_error_to_governor)?;
        let sddl = HSTRING::from(owner.protected_sddl());
        let mut descriptor = PSECURITY_DESCRIPTOR::default();
        unsafe {
            ConvertStringSecurityDescriptorToSecurityDescriptorW(
                &sddl,
                SDDL_REVISION_1,
                &raw mut descriptor,
                None,
            )
        }
        .map_err(GovernorError::Windows)?;
        let attributes = SECURITY_ATTRIBUTES {
            nLength: u32::try_from(size_of::<SECURITY_ATTRIBUTES>())
                .expect("security attributes exceed u32"),
            lpSecurityDescriptor: descriptor.0.cast::<c_void>(),
            bInheritHandle: false.into(),
        };
        let initialization_name = HSTRING::from(&initialization_name);
        let initialization =
            unsafe { CreateMutexW(Some(&raw const attributes), false, &initialization_name) }
                .map_err(GovernorError::Windows);
        let initialization = initialization?;
        let initialization = MutexGuard::acquire(initialization)?;
        let handle = unsafe { CreateJobObjectW(Some(&raw const attributes), &wide_name) }
            .map_err(GovernorError::Windows);
        let created = unsafe { GetLastError() } != ERROR_ALREADY_EXISTS;
        unsafe {
            let _ = LocalFree(Some(HLOCAL(descriptor.0.cast::<c_void>())));
        }
        let handle = handle?;
        let governor = Self { handle, name };

        if created || initialization.was_abandoned() {
            governor.configure().map_err(cpu_cap_error)?;
        }
        governor.verify_configuration()?;
        drop(initialization);
        unsafe { AssignProcessToJobObject(governor.handle, GetCurrentProcess()) }
            .map_err(GovernorError::Windows)?;
        let sample = governor.point_sample()?;
        Ok((governor, CpuCapStatus::HardCap20Percent, sample))
    }

    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }

    /// A point sample for the explicitly enrolled infrastructure processes only.
    pub fn point_sample(&self) -> Result<InfrastructureSample, GovernorError> {
        let mut accounting = JOBOBJECT_BASIC_ACCOUNTING_INFORMATION::default();
        unsafe {
            QueryInformationJobObject(
                Some(self.handle),
                JobObjectBasicAccountingInformation,
                (&raw mut accounting).cast::<c_void>(),
                u32::try_from(size_of_val(&accounting)).expect("job accounting exceeds u32"),
                None,
            )
        }
        .map_err(GovernorError::Windows)?;
        let mut limits = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
        unsafe {
            QueryInformationJobObject(
                Some(self.handle),
                JobObjectExtendedLimitInformation,
                (&raw mut limits).cast::<c_void>(),
                u32::try_from(size_of_val(&limits)).expect("job limits exceed u32"),
                None,
            )
        }
        .map_err(GovernorError::Windows)?;
        Ok(InfrastructureSample {
            active_processes: accounting.ActiveProcesses,
            total_processes: accounting.TotalProcesses,
            user_cpu_millis: hundred_nanos_to_millis(accounting.TotalUserTime),
            kernel_cpu_millis: hundred_nanos_to_millis(accounting.TotalKernelTime),
            peak_process_memory_bytes: u64::try_from(limits.PeakProcessMemoryUsed)
                .unwrap_or(u64::MAX),
            peak_job_memory_bytes: u64::try_from(limits.PeakJobMemoryUsed).unwrap_or(u64::MAX),
        })
    }

    /// Returns whether a process is part of this infrastructure job. This is mainly useful for
    /// verifying that a locally spawned native provider descendant silently broke away.
    pub fn contains_process_id(&self, process_id: u32) -> Result<bool, GovernorError> {
        let process = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, process_id) }
            .map_err(GovernorError::Windows)?;
        let process = HandleGuard(process);
        let mut contained = windows::core::BOOL::default();
        unsafe {
            windows::Win32::System::JobObjects::IsProcessInJob(
                process.0,
                Some(self.handle),
                &raw mut contained,
            )
        }
        .map_err(GovernorError::Windows)?;
        Ok(contained.as_bool())
    }

    fn configure(&self) -> windows::core::Result<()> {
        let limits = JOBOBJECT_EXTENDED_LIMIT_INFORMATION {
            BasicLimitInformation:
                windows::Win32::System::JobObjects::JOBOBJECT_BASIC_LIMIT_INFORMATION {
                    LimitFlags: JOB_OBJECT_LIMIT_SILENT_BREAKAWAY_OK,
                    ..Default::default()
                },
            ..Default::default()
        };
        unsafe {
            SetInformationJobObject(
                self.handle,
                JobObjectExtendedLimitInformation,
                (&raw const limits).cast::<c_void>(),
                u32::try_from(size_of_val(&limits)).expect("job limits exceed u32"),
            )
        }?;
        let cpu = JOBOBJECT_CPU_RATE_CONTROL_INFORMATION {
            ControlFlags: JOB_OBJECT_CPU_RATE_CONTROL_ENABLE | JOB_OBJECT_CPU_RATE_CONTROL_HARD_CAP,
            Anonymous: JOBOBJECT_CPU_RATE_CONTROL_INFORMATION_0 {
                CpuRate: INFRASTRUCTURE_CPU_RATE,
            },
        };
        unsafe {
            SetInformationJobObject(
                self.handle,
                JobObjectCpuRateControlInformation,
                (&raw const cpu).cast::<c_void>(),
                u32::try_from(size_of_val(&cpu)).expect("CPU control structure exceeds u32"),
            )
        }
    }

    fn verify_configuration(&self) -> Result<(), GovernorError> {
        let mut limits = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
        unsafe {
            QueryInformationJobObject(
                Some(self.handle),
                JobObjectExtendedLimitInformation,
                (&raw mut limits).cast::<c_void>(),
                u32::try_from(size_of_val(&limits)).expect("job limits exceed u32"),
                None,
            )
        }
        .map_err(GovernorError::Windows)?;
        if !limits
            .BasicLimitInformation
            .LimitFlags
            .contains(JOB_OBJECT_LIMIT_SILENT_BREAKAWAY_OK)
        {
            return Err(GovernorError::ConfigurationMismatch);
        }
        let mut cpu = JOBOBJECT_CPU_RATE_CONTROL_INFORMATION::default();
        unsafe {
            QueryInformationJobObject(
                Some(self.handle),
                JobObjectCpuRateControlInformation,
                (&raw mut cpu).cast::<c_void>(),
                u32::try_from(size_of_val(&cpu)).expect("CPU control structure exceeds u32"),
                None,
            )
        }
        .map_err(cpu_cap_error)?;
        let expected = JOB_OBJECT_CPU_RATE_CONTROL_ENABLE | JOB_OBJECT_CPU_RATE_CONTROL_HARD_CAP;
        let rate = unsafe { cpu.Anonymous.CpuRate };
        if cpu.ControlFlags != expected || rate != INFRASTRUCTURE_CPU_RATE {
            return Err(GovernorError::ConfigurationMismatch);
        }
        Ok(())
    }
}

impl Drop for InfrastructureGovernor {
    fn drop(&mut self) {
        unsafe {
            let _ = CloseHandle(self.handle);
        }
    }
}

fn runtime_error_to_governor(error: RuntimeError) -> GovernorError {
    match error {
        RuntimeError::Windows(error) => GovernorError::Windows(error),
        _ => GovernorError::OwnerLookup,
    }
}

fn cpu_cap_error(error: windows::core::Error) -> GovernorError {
    if error.code().0 == (0x8007_0000_u32 | ERROR_NOT_SUPPORTED.0).cast_signed() {
        GovernorError::WindowsUnsupported
    } else {
        GovernorError::Windows(error)
    }
}

struct HandleGuard(HANDLE);

impl Drop for HandleGuard {
    fn drop(&mut self) {
        unsafe {
            let _ = CloseHandle(self.0);
        }
    }
}

struct MutexGuard {
    handle: HANDLE,
    abandoned: bool,
}

impl MutexGuard {
    fn acquire(handle: HANDLE) -> Result<Self, GovernorError> {
        let result = unsafe { WaitForSingleObject(handle, INFINITE) };
        if result != WAIT_OBJECT_0 && result != WAIT_ABANDONED {
            unsafe {
                let _ = CloseHandle(handle);
            }
            return Err(GovernorError::Windows(windows::core::Error::from_thread()));
        }
        Ok(Self {
            handle,
            abandoned: result == WAIT_ABANDONED,
        })
    }

    fn was_abandoned(&self) -> bool {
        self.abandoned
    }
}

impl Drop for MutexGuard {
    fn drop(&mut self) {
        unsafe {
            let _ = ReleaseMutex(self.handle);
            let _ = CloseHandle(self.handle);
        }
    }
}

fn hundred_nanos_to_millis(value: i64) -> u64 {
    u64::try_from(value).unwrap_or(0) / 10_000
}

/// A monotonic cumulative CPU sample supplied by the daemon's sampler.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BudgetSample {
    pub elapsed: Duration,
    pub cumulative_cpu: Duration,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum WindowState {
    Warming,
    Ready { aggregate_machine_cpu_percent: u32 },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BudgetAssessment {
    pub thirty_seconds: WindowState,
    pub five_minutes: WindowState,
    pub throttle_background: bool,
}

#[derive(Debug, Error, Eq, PartialEq)]
pub enum RollingBudgetError {
    #[error("logical processor count must be non-zero")]
    ZeroLogicalProcessors,
    #[error("elapsed time regressed")]
    ElapsedRegressed,
    #[error("cumulative CPU time regressed")]
    CpuRegressed,
}

/// Bounded rolling CPU accounting. It never sleeps or delays caller work.
#[derive(Debug)]
pub struct RollingBudget {
    logical_processors: u32,
    last: Option<BudgetSample>,
    short: VecDeque<BudgetSample>,
    long: VecDeque<BudgetSample>,
}

impl RollingBudget {
    pub fn new(logical_processors: u32) -> Result<Self, RollingBudgetError> {
        if logical_processors == 0 {
            return Err(RollingBudgetError::ZeroLogicalProcessors);
        }
        Ok(Self {
            logical_processors,
            last: None,
            short: VecDeque::with_capacity(SHORT_SAMPLE_CAPACITY),
            long: VecDeque::with_capacity(LONG_SAMPLE_CAPACITY),
        })
    }

    pub fn observe(
        &mut self,
        sample: BudgetSample,
    ) -> Result<BudgetAssessment, RollingBudgetError> {
        if let Some(last) = self.last {
            if sample.elapsed < last.elapsed {
                return Err(RollingBudgetError::ElapsedRegressed);
            }
            if sample.cumulative_cpu < last.cumulative_cpu {
                return Err(RollingBudgetError::CpuRegressed);
            }
        }
        self.last = Some(sample);
        push_coalesced(&mut self.short, sample);
        push_coalesced(&mut self.long, sample);
        trim_window(&mut self.short, sample, SHORT_WINDOW, SHORT_SAMPLE_CAPACITY);
        trim_window(&mut self.long, sample, LONG_WINDOW, LONG_SAMPLE_CAPACITY);
        let thirty_seconds =
            window_state(&self.short, sample, SHORT_WINDOW, self.logical_processors);
        let five_minutes = window_state(&self.long, sample, LONG_WINDOW, self.logical_processors);
        let throttle_background = matches!(thirty_seconds, WindowState::Ready { aggregate_machine_cpu_percent } if aggregate_machine_cpu_percent >= BACKGROUND_THROTTLE_PERCENT);
        Ok(BudgetAssessment {
            thirty_seconds,
            five_minutes,
            throttle_background,
        })
    }

    #[must_use]
    pub fn sample_counts(&self) -> (usize, usize) {
        (self.short.len(), self.long.len())
    }
}

/// Keep at most one (the latest) cumulative sample per elapsed second. The public sampler may be
/// called more often, but rolling precision is deliberately one second rather than pretending to
/// retain sub-second history in a bounded five-minute ring.
fn push_coalesced(samples: &mut VecDeque<BudgetSample>, sample: BudgetSample) {
    if samples
        .back()
        .is_some_and(|last| last.elapsed.as_secs() == sample.elapsed.as_secs())
    {
        samples.pop_back();
    }
    samples.push_back(sample);
}

/// Preserve the sample immediately before the window boundary as an interpolation anchor. When
/// oversampled, discard interior points rather than that anchor, so bounded storage cannot make a
/// continuously sampled window warm forever.
fn trim_window(
    samples: &mut VecDeque<BudgetSample>,
    latest: BudgetSample,
    window: Duration,
    capacity: usize,
) {
    let boundary = latest.elapsed.saturating_sub(window);
    while samples.len() > 1 && samples[1].elapsed <= boundary {
        samples.pop_front();
    }
    while samples.len() > capacity {
        samples.remove(1);
    }
}

fn window_state(
    samples: &VecDeque<BudgetSample>,
    latest: BudgetSample,
    window: Duration,
    processors: u32,
) -> WindowState {
    let Some(first) = samples.front().copied() else {
        return WindowState::Warming;
    };
    if latest.elapsed < window {
        return WindowState::Warming;
    }
    let boundary = latest.elapsed.saturating_sub(window);
    if first.elapsed > boundary {
        return WindowState::Warming;
    }
    let boundary_cpu = interpolated_cpu(samples, boundary);
    let cpu = latest.cumulative_cpu.saturating_sub(boundary_cpu);
    let denominator = window.as_nanos().saturating_mul(u128::from(processors));
    let percent = cpu
        .as_nanos()
        .saturating_mul(100)
        .checked_div(denominator)
        .map_or(0, |value| u32::try_from(value).unwrap_or(u32::MAX));
    WindowState::Ready {
        aggregate_machine_cpu_percent: percent,
    }
}

fn interpolated_cpu(samples: &VecDeque<BudgetSample>, at: Duration) -> Duration {
    let before = samples
        .iter()
        .rev()
        .find(|sample| sample.elapsed <= at)
        .copied()
        .expect("window state checked the boundary anchor");
    if before.elapsed == at {
        return before.cumulative_cpu;
    }
    let after = samples
        .iter()
        .find(|sample| sample.elapsed >= at)
        .copied()
        .expect("latest sample is after the window boundary");
    let interval = after.elapsed.saturating_sub(before.elapsed).as_nanos();
    if interval == 0 {
        return after.cumulative_cpu;
    }
    let progressed = at.saturating_sub(before.elapsed).as_nanos();
    let delta = after
        .cumulative_cpu
        .saturating_sub(before.cumulative_cpu)
        .as_nanos();
    Duration::from_nanos(
        u64::try_from(
            before
                .cumulative_cpu
                .as_nanos()
                .saturating_add(delta.saturating_mul(progressed) / interval),
        )
        .unwrap_or(u64::MAX),
    )
}

/// A bounded queue for work the daemon has already classified as background. A full queue or a
/// soft-threshold assessment explicitly refuses admission; user, cancellation, and acknowledgement
/// operations must bypass this type entirely.
#[derive(Debug)]
pub struct BackgroundQueue<T> {
    capacity: usize,
    items: VecDeque<T>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BackgroundRefusal {
    Full,
    Throttled,
}

impl<T> BackgroundQueue<T> {
    #[must_use]
    pub fn new(capacity: usize) -> Self {
        Self {
            capacity,
            items: VecDeque::with_capacity(capacity),
        }
    }

    pub fn try_push(
        &mut self,
        item: T,
        assessment: BudgetAssessment,
    ) -> Result<(), BackgroundRefusal> {
        if assessment.throttle_background {
            return Err(BackgroundRefusal::Throttled);
        }
        if self.items.len() == self.capacity {
            return Err(BackgroundRefusal::Full);
        }
        self.items.push_back(item);
        Ok(())
    }

    pub fn pop(&mut self) -> Option<T> {
        self.items.pop_front()
    }
}
