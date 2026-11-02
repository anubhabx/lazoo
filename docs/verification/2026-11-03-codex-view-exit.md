# Codex native-view exit investigation

Date: 2026-11-03 (Asia/Calcutta). Worktree: `C:\workspace\Orchestrator\.worktrees\copilot-account-routing`.
Branch: `fix/codex-view-exit`, based on `fix/native-session-resilience` (PR #47), starting HEAD `485c882cee65a1e9d1da8d95d2b51a52923b464b`.

## Finding and evidence limit

The controller queue treated an immediately full 32-slot channel as a fatal transport failure. `try_send` failed even for a finite, valid burst while the durable turn observer was briefly occupied. `read_stdout` then called `TerminateJobObject(..., 1)`. Its reason was discarded and the turn saw `ProcessExited { code: None }`.

The fake app-server regression attaches an authenticated viewer after `turn/started`, serves its metadata bootstrap and history page, streams a command item and its output, serves a final history refresh, then emits 64 final-message deltas, token usage and rate limits. The final `item/completed` and `turn/completed` follow the last tool output. A 600 ms observer delay models finite persistence latency, with stdout drained on another Tokio worker. This is a controlled stress reproduction, not a captured transcript of replay f.

With instrumentation but before the queue fix, the regression failed. `artifacts/codex-view-exit-red.log` contains:

```text
21:49:52.377598Z host_pid=7812 reason="controller-queue-overflow-or-invalid-response"
21:49:52.377688Z job_id=e3188a20-955f-4582-97d9-51ecfda5a4e3 requested_exit_code=1
21:49:52.379266Z host_pid=7812 exit_code=Some(1) success=false
finite end-of-turn burst must not kill the canonical child: ProcessExited { code: None }
```

These timestamps are UTC on 2026-11-02, already 2026-11-03 locally. This establishes a Lazoo-initiated kill on this code path. It does **not** establish that replay f took that path: the old live daemon retained neither its close reason nor exit status. This queue bug can also affect canonical bursts without a viewer; the fixture demonstrates survival with a viewer, not that a viewer is necessary to trigger it. Claude's instrumented desktop replay remains the acceptance check for the original intermittent failure.

## Fix and coverage

Controller delivery now waits for a channel slot for at most two seconds. Capacity remains 32; the conservative 8 MiB byte reservation includes the one pending frame. Meaningful events remain ordered and are not dropped. A permanently stalled consumer, closed receiver, oversized controller frame, and byte-budget violation still fail closed, with distinct diagnostics. Viewer notifications retain bounded, lossy fan-out: a lagging viewer can disconnect and resync without terminating canonical work.

The regression now checks all 64 deltas, assembled final text, usage, completion, and a subsequent canonical turn. A second variant closes the lease, connects a fresh authenticated viewer, initializes and closes it before releasing final completion. Both use real fixture processes and the real WebSocket bridge. Unit coverage checks delayed draining without loss, byte-reservation release, stalled/closed receivers, and the existing hard byte limit.

Controller drop now terminates through its owned job handle directly; a concurrently held child mutex can no longer silently skip cleanup. This is controller-owned cleanup, never a viewer capability.

## Process and view ownership

This starting HEAD has **no separate history-reader app-server**. The preceding verification document explicitly records that approach as rejected because the installed binary refuses unloaded-thread reads. `codex_view.rs` sends bounded read requests through the canonical broker and receives its notifications.

`CodexBroker` owns the canonical `JobChild` job. The native viewer is launched by `terminal_open_with_command` into a separately created ConPTY job. The lease only closes its listener/connections and cannot call broker stop, answer canonical server requests, or terminate the canonical job. Existing daemon native-view close/reopen tests and runtime terminal tests remain in the validation run.

The new exit observer duplicates a **process handle**, with inheritance disabled. It owns no job handle, stdin, stdout, or controller capability. An OS-wait thread can report exit after controller drop or Tokio-task cancellation without keeping the job alive. The observed PID is `lazoo-session-host`; that executable waits for its native child and propagates its exit code. Job termination logs its requested code separately; the observer records the actual Windows status rather than assuming termination succeeded.

## Replay diagnostics

`lazood` uses valid `RUST_LOG`, with `info` as fallback. Suggested scoped setting:

```powershell
$env:RUST_LOG = 'info,lazood=debug,lazoo_providers=debug,lazoo_runtime=debug'
```

All closure/termination records below are warn level. An initiation record can say `exit_code=None, status_observed=false`; the later `Codex child exit observed` supplies the real status for the same host PID/job ID. No exit code is fabricated at request time.

| Path | Evidence |
| --- | --- |
| Controller frame/queue overflow, closed receiver, invalid response | Fixed reason, PID/job ID; failed delivery also records safe method label and queue counts/bytes |
| Controller write error/deadline, reader EOF/I/O/JSON error | Fixed close reason and first reason; separate actual exit record |
| Explicit stop, stop deadline, controller drop | Lifecycle reason and eventual actual code |
| Job termination/final handle close | Job ID, requested code or active-process count, correlated actual exit |
| Idle stop, session stop/replace, daemon shutdown | Daemon reason and session/host identity, then broker lifecycle and actual exit |
| View lease close/disconnect | Canonical PID, fixed reason, `canonical_termination=false`; terminal termination/exit has its own PID |
| Native process exits itself | Actual status; no termination request is needed to trigger the observer |

Debug records contain allowlisted method labels, frame sizes, message classes, viewer read phases, and canonical-host/viewer-PID association. They omit RPC bodies, arguments, environment values, response IDs, URLs, and capability tokens. Unknown method strings become `other`. Tests capture self-exit code 37 and controller-drop code 1 and check that neither the view token nor its environment-variable name appears. Canonical turn errors also recover an available real code after transport closure.

## Validation

- PASS: the new end-of-turn fixture failed before the queue fix and passed after it (`codex-view-exit-red.log`, `codex-view-exit-green.log`).
- PASS: `cargo fmt --all -- --check`.
- PASS: workspace clippy with `-D warnings` (`artifacts/codex-view-exit-clippy-final.log`).
- PASS: full provider, daemon and runtime suites, exit 0 (`artifacts/codex-view-exit-suites.log`): 424 passing result entries including two nested child-fixture invocations, 0 failed, 23 ignored. The ignored tests remain existing opt-in live/native checks.
- PASS: runtime suite rerun after ensuring exit-status diagnostics cannot interrupt terminal cleanup (`artifacts/codex-view-exit-runtime-final.log`); fmt and workspace clippy were also rerun successfully.
- PASS: `git diff --check`; no UI changes or staged paths; branch remains `fix/codex-view-exit`.

Every Cargo invocation uses this worktree's target and two build jobs:

```powershell
$env:CARGO_BUILD_JOBS = '2'
$env:CARGO_TARGET_DIR = Join-Path (Get-Location) 'target'
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test -p lazoo-providers -p lazood -p lazoo-runtime
git diff --check
```

The runtime suite is included because job/terminal observability changed. No release build, authenticated model turn, desktop UI edit, staging, commit, or push is part of this slice. Existing scratch artifacts are untouched. `artifacts/codex-view-exit-*` logs are local evidence, excluded from staging.

## Exact staging handoff

Claude commits, pushes, opens the PR against `fix/native-session-resilience`, and replays with the debug filter above. Stage only:

```powershell
git add -- Cargo.lock PR-BODY.md apps/lazood/src/app.rs apps/lazood/src/app/idle.rs apps/lazood/src/app/terminal/native_view.rs apps/lazood/src/main.rs crates/lazoo-providers/Cargo.toml crates/lazoo-providers/src/codex_broker.rs crates/lazoo-providers/src/codex_view.rs crates/lazoo-providers/tests/codex.rs crates/lazoo-providers/tests/fixtures/codex/fake-codex-app-server.ps1 crates/lazoo-runtime/Cargo.toml crates/lazoo-runtime/src/process_job.rs crates/lazoo-runtime/src/terminal.rs docs/verification/2026-11-03-codex-view-exit.md
```

Suggested title: `Preserve Codex completion bursts and trace native child exits`.
PR body: root `PR-BODY.md`.


## Real-desktop replay (Claude, 2026-11-03)

Debug build of this branch, fresh data root, daemon launched directly with `RUST_LOG=info,lazood=debug,lazoo_providers=debug,lazoo_runtime=debug`, the real desktop over CDP; Claude Code lead (claude-sonnet-5-5, notice-turns), Codex worker forge (gpt-6-luna, Codex 0.166.0-alpha.5.1). In both conversations forge's native view was opened about 2 s into its turn, the step that ended forge in replay f.

| Conversation | Forge | Lead | Incidents |
|---|---|---|---|
| Fix `add()`, run the tests | completed, suspended 22:06:07 | completed after the result notice | 0 |
| Add `test_mul_float`, run the tests | completed (test added, run reported 2 passed, 1 failed because the isolated worktree still has the unfixed `add`), suspended 22:07:04 | completed, reported the exact result and why | 0 |

The new lifecycle records show the normal end-of-task path: `controller-stop`, then "closing Codex viewer lease only" with `canonical_termination: false`, then the stop-deadline job termination and the observed exit. Nothing on the viewer path reached the canonical child.
