use std::{
    process::Command,
    time::{Duration, Instant},
};
use uuid::Uuid;
use windows::{
    Win32::{
        Foundation::CloseHandle,
        System::Threading::{CreateMutexW, ReleaseMutex},
    },
    core::HSTRING,
};

use lazoo_runtime::{
    BackgroundQueue, BackgroundRefusal, BudgetSample, GovernorError, RollingBudget,
    RollingBudgetError, WindowState,
};

fn sample(seconds: u64, cpu_seconds: u64) -> BudgetSample {
    BudgetSample {
        elapsed: Duration::from_secs(seconds),
        cumulative_cpu: Duration::from_secs(cpu_seconds),
    }
}

#[test]
fn native_provider_descendant_silently_breaks_away_in_child_helper() {
    if std::env::var_os("LAZOO_GOVERNOR_CHILD").is_some() {
        let (governor, cap, sample) =
            lazoo_runtime::InfrastructureGovernor::attach_current(Uuid::new_v4().to_string())
                .expect("child helper can configure and attach its own infrastructure job");
        assert_eq!(cap, lazoo_runtime::CpuCapStatus::HardCap20Percent);
        assert!(sample.active_processes >= 1, "{sample:?}");
        let mut provider = Command::new("cmd.exe")
            .args(["/c", "ping -n 3 127.0.0.1 > nul"])
            .spawn()
            .expect("local provider helper starts");
        assert!(
            !governor.contains_process_id(provider.id()).unwrap(),
            "SILENT_BREAKAWAY_OK must exclude native provider descendants"
        );
        provider.wait().unwrap();
        return;
    }
    let status = Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "native_provider_descendant_silently_breaks_away_in_child_helper",
            "--nocapture",
        ])
        .env("LAZOO_GOVERNOR_CHILD", "1")
        .status()
        .expect("governor child helper starts");
    assert!(status.success(), "child helper returned {status}");
}

/// A main that cannot take the initialization lock gives up after about five seconds with a
/// named error instead of waiting forever. The attach runs in a child process so that an
/// unexpected success never enrolls the test process in the 20% capped job.
#[test]
fn initialization_lock_held_elsewhere_times_out_in_child_helper() {
    if std::env::var_os("LAZOO_GOVERNOR_CHILD").is_some() {
        let installation_id = Uuid::new_v4();
        let name = HSTRING::from(format!(
            "Local\\Lazoo.Infrastructure.Init.{installation_id}"
        ));
        // This thread owns the lock for the whole test. A Windows mutex is re-entrant for its
        // owner, so the attach must run on another thread to be made to wait.
        let held = unsafe { CreateMutexW(None, true, &name) }.expect("init lock is created");
        let started = Instant::now();
        let outcome = std::thread::spawn(move || {
            lazoo_runtime::InfrastructureGovernor::attach_current(installation_id.to_string())
        })
        .join()
        .expect("attach thread finishes");
        let elapsed = started.elapsed();
        let error = outcome.expect_err("attach must not enroll while another main holds the lock");
        assert!(
            matches!(error, GovernorError::InitializationTimedOut),
            "{error:?}"
        );
        assert_eq!(
            error.to_string(),
            "infrastructure job initialization lock timed out"
        );
        assert!(
            elapsed >= Duration::from_millis(4_500) && elapsed < Duration::from_secs(30),
            "{elapsed:?}"
        );
        unsafe {
            let _ = ReleaseMutex(held);
            let _ = CloseHandle(held);
        }
        return;
    }
    let status = Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "initialization_lock_held_elsewhere_times_out_in_child_helper",
            "--nocapture",
        ])
        .env("LAZOO_GOVERNOR_CHILD", "1")
        .status()
        .expect("governor child helper starts");
    assert!(status.success(), "child helper returned {status}");
}

#[test]
fn rolling_windows_warm_then_report_total_machine_share() {
    let mut budget = RollingBudget::new(4).unwrap();
    let first = budget.observe(sample(0, 0)).unwrap();
    assert_eq!(first.thirty_seconds, WindowState::Warming);
    let short = budget.observe(sample(30, 6)).unwrap();
    assert_eq!(
        short.thirty_seconds,
        WindowState::Ready {
            aggregate_machine_cpu_percent: 5
        }
    );
    assert_eq!(short.five_minutes, WindowState::Warming);
    let long = budget.observe(sample(300, 60)).unwrap();
    assert_eq!(
        long.five_minutes,
        WindowState::Ready {
            aggregate_machine_cpu_percent: 5
        }
    );
    assert!(!long.throttle_background);
}

#[test]
fn rolling_budget_refuses_regressing_counters_and_bounds_storage() {
    let mut budget = RollingBudget::new(1).unwrap();
    budget.observe(sample(2, 1)).unwrap();
    assert_eq!(
        budget.observe(sample(1, 1)),
        Err(RollingBudgetError::ElapsedRegressed)
    );
    assert_eq!(
        budget.observe(sample(3, 0)),
        Err(RollingBudgetError::CpuRegressed)
    );
    for second in 3..1_000 {
        budget.observe(sample(second, second)).unwrap();
    }
    let (short, long) = budget.sample_counts();
    assert!(short <= 301, "{short}");
    assert!(long <= 601, "{long}");
}

#[test]
fn recent_burst_is_high_for_thirty_seconds_but_low_for_five_minutes() {
    let mut budget = RollingBudget::new(1).unwrap();
    budget.observe(sample(0, 0)).unwrap();
    let result = budget.observe(sample(300, 30)).unwrap();
    assert_eq!(
        result.five_minutes,
        WindowState::Ready {
            aggregate_machine_cpu_percent: 10
        }
    );

    let result = budget.observe(sample(330, 60)).unwrap();
    assert_eq!(
        result.thirty_seconds,
        WindowState::Ready {
            aggregate_machine_cpu_percent: 100
        }
    );
    assert_eq!(
        result.five_minutes,
        WindowState::Ready {
            aggregate_machine_cpu_percent: 19
        }
    );
    assert!(result.throttle_background);
}

#[test]
fn oversampling_is_coalesced_without_losing_five_minute_burst_history() {
    let mut budget = RollingBudget::new(1).unwrap();
    for millisecond in (0..=600_000).step_by(100) {
        let cpu_milliseconds = match millisecond {
            100_000..104_000 => millisecond - 100_000,
            104_000.. => 4_000,
            _ => 0,
        };
        let result = budget
            .observe(BudgetSample {
                elapsed: Duration::from_millis(millisecond),
                cumulative_cpu: Duration::from_millis(cpu_milliseconds),
            })
            .unwrap();
        if millisecond == 400_000 {
            assert_eq!(
                result.five_minutes,
                WindowState::Ready {
                    aggregate_machine_cpu_percent: 1
                }
            );
        }
    }
    let result = budget.observe(sample(600, 4)).unwrap();
    assert_eq!(
        result.five_minutes,
        WindowState::Ready {
            aggregate_machine_cpu_percent: 0
        }
    );
    let (short, long) = budget.sample_counts();
    assert!(short <= 32 && long <= 302, "{short}, {long}");
}

#[test]
fn background_queue_explicitly_refuses_soft_threshold_and_full_queue() {
    let mut budget = RollingBudget::new(1).unwrap();
    budget.observe(sample(0, 0)).unwrap();
    let hot = budget.observe(sample(30, 5)).unwrap();
    assert!(hot.throttle_background);
    let mut queue = BackgroundQueue::new(1);
    assert_eq!(
        queue.try_push("index", hot),
        Err(BackgroundRefusal::Throttled)
    );

    let cool = budget.observe(sample(60, 5)).unwrap();
    queue.try_push("index", cool).unwrap();
    assert_eq!(queue.try_push("retry", cool), Err(BackgroundRefusal::Full));
    assert_eq!(queue.pop(), Some("index"));
}
