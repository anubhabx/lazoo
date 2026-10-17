# Adapter cancellation verification

Date completed: 2026-10-17 IST

Branch: `feat/adapter-cancellation` (a dedicated worktree branch; the integrator merges it into `feat/core-v1-completion`)

Base: `6b58c4e` on `feat/core-v1-completion`

Product version: `0.1.0`

Database schema: 19 (this slice adds no migration)

Scope: deterministic tests that `cancel()` ends an active turn for the two adapters that had none, Claude Code and Antigravity. Each adapter gains one test for cancel during an active turn and one for cancel with no active turn. A stall mode in the Claude fake (`stall-turn`) and a new always-stalling Antigravity fake (`fake-agy-stall`) let the cancel land. No production code, protocol, RPC, error code, store, migration, or desktop change.

## Outcome

PASS for what this slice claims: for both adapters, `cancel()` called while a turn is blocked on the host's stdout ends that turn in about 2 s (the fake sleeps 30 s and the test allows 10 s), the turn returns `Ok` with exactly one `ProviderEvent::Cancelled` carrying the provider kind and the session id, no `TurnCompleted` and no `Error` event, `stop()` afterwards returns `Ok`, and the fake host process is gone. Cancel with no active turn returns `Ok(())` twice, leaves the host running, and `stop()` still ends it.

The "host is gone" check is on the fake's own PID, not a handle the adapter reports: each stalling fake writes `$PID` (the PowerShell interpreter that runs the fake) to `host-pid` in its working directory, and the test polls `tasklist /FI "PID eq <pid>"` until it no longer lists it. It therefore proves the whole job tree was terminated, not only the `.cmd` wrapper.

No adapter production code changed: the cancellation paths read as designed (`ClaudeSession::cancel`, `AntigravitySession::cancel`) and both tests pass against them. No live provider was run.

## Design notes

| Decision | Reason |
|---|---|
| Stall selected by the existing launch-argument pattern (`--model stall-turn`), not an environment variable | `fake-claude.ps1` already switches modes on `$CliArgs -contains '<marker>'` (`queued-startup`, `startup-hooks`, ...). An environment variable is process-wide and the tests in one binary run in parallel; a launch argument is per session. |
| The stall branch sits inside the existing read loop, after the user message is read and `init` is written | It proves the cancel lands after `begin_turn` and the prompt write, in the state the production code guards (`lifecycle` true, `active` true). Other modes are untouched; the only addition outside the branch is a `host-pid` write that happens when `stall-turn` is given. |
| The fake writes a `turn-started` marker after its first text delta; the test cancels when it appears | Deterministic ordering with no sleep guess: the marker exists only after the adapter has set the turn active and written the prompt. The Claude fake writes `before cancel` as a text delta so the test also proves output before the cancel is kept. |
| New `fake-agy-stall.cmd` / `.ps1` rather than extending `turn.jsonl` | The Antigravity tests had no fake CLI at all (only normalization of the recorded `turn.jsonl`). The new fake does one thing, stall, and its name says so; it replays `init` with `conversation-1` so the `Cancelled` event's session id (`conversation-1`) is the post-init id, not the `pending:` placeholder. |
| `join!` of the turn and the canceller inside a 10 s `tokio::time::timeout` | The fake stalls 30 s and the adapter's progress timeout is 30 s. A cancel that failed to end the turn would surface as a stall or a timeout, both far above 10 s, so "promptly" is a hard failure rather than a soft elapsed-time assertion. |
| Idle-cancel test asserts `Ok`, host still running, then `stop()` | Both `cancel()` implementations only act when a turn is active (`lifecycle` for Claude, `active` for Antigravity) and return `Ok(())` otherwise; the test pins that and that it does not set `stopped`, since `stop()` must still do its work. |
| Test helpers (`wait_for_file`, `host_is_running`, `assert_host_gone`) are duplicated in the two test files | Each integration test file is its own crate, and the repo has no shared test-support module for the providers tests; a new module for 30 lines would be the larger change. |

## Fresh verification matrix

All commands were run from the worktree `C:\workspace\Orchestrator\.claude\worktrees\s2-cancel` with `CARGO_TARGET_DIR=C:\workspace\Orchestrator\.claude\worktrees\s2-cancel\target`, because the main checkout's target directory was in use by the full gate. The provider tests spawn through `lazoo-session-host.exe`, which `cargo test -p lazoo-providers` does not build; `cargo build -p lazoo-session-host --locked` (debug) was run once so the binary sits beside the test executables.

| Command | Exit | Evidence |
|---|---:|---|
| `cargo fmt --all -- --check` | 0 | No formatting differences. |
| `cargo clippy -p lazoo-providers --all-targets --locked -- -D warnings` | 0 | Clean. |
| `cargo test -p lazoo-providers --test antigravity --locked` | 0 | 6 passed, 0 failed, 2 ignored (live probes): the 4 existing tests plus `agy_cancel_during_active_turn_kills_host_and_reports_cancelled` and `agy_cancel_without_an_active_turn_is_a_no_op_that_keeps_the_host`. |
| `cargo test -p lazoo-providers --test claude --locked -- --test-threads=1` | 0 | 19 passed, 0 failed, 2 ignored (live probes): the 17 existing tests plus `claude_cancel_during_active_turn_kills_host_and_reports_cancelled` and `claude_cancel_without_an_active_turn_is_a_no_op_that_keeps_the_host`. |
| `cargo test -p lazoo-providers --test claude --test antigravity --locked cancel`, run 5 times | 0 each | 2 passed in each binary on every run, about 2.5 s per binary, so the four new tests were not flaky. |
| `cargo test -p lazoo-providers --test claude --locked` (default parallelism) | 101 on 3 of 3 runs | 16 to 18 of 19 passed. The failing test varied between runs and was always one of the startup-frame tests (`claude_refuses_queued_startup_frames_before_writing_a_prompt`, `claude_refuses_mismatched_startup_hook_before_writing_a_prompt`, `claude_refuses_non_startup_hook_before_writing_a_prompt`, `claude_bounds_startup_hook_drain_before_writing_a_prompt`). The same flake reproduced without any change from this slice (stashed to base `6b58c4e`: 2 of 4 runs failed, with the same test names). It is pre-existing, load-dependent, and not touched here; the four new tests passed in all of these runs. |
| `git diff --quiet -- apps/lazoo-desktop` | 0 | Empty. |
| `git diff --stat -- crates/lazoo-providers/src` | 0 | Empty: no production code changed. |

Not run, deliberately: `cargo test --workspace`, the daemon test binary, `scripts/gate.ps1`, every acceptance script, and any release build (the integrator runs the full gate after merging).

## Boundary evidence

- `git diff --quiet -- apps/lazoo-desktop` exits 0; the diff is empty.
- `crates/lazoo-providers/src` is unchanged. The diff is two test files, one existing fixture script (`fake-claude.ps1`, +10 lines, all inside `stall-turn` branches), and two new fixture files.
- No Tauri command, capability, `desktop.snapshot` field, frontend source, migration, protocol type, RPC method, or error code changed. No acceptance script was added.

## Retained follow-up boundaries

- No live cancellation was run. Nothing here proves that a real `claude` or `agy` binary exits when its process tree is terminated, or that either emits no further output after the kill; the proof is against fakes that mirror each adapter's wire format and uses the same Job Object termination as production.
- Codex and the ACP adapters already have deterministic cancellation coverage (`tests/codex.rs: codex_interrupt_correlates_the_active_turn`, `observed_codex_rejection_interrupts_the_accepted_native_turn`; `tests/opencode.rs: acp_native_cancellation_missing_observer_and_deadline_all_reply_cancelled`). This slice adds none for them. Grok Build delegates to the same ACP session type, so it is presumed to share that coverage; that was not checked here.
- Cancel is kill-based for both adapters: the host is terminated and the session is marked stopped. A cancelled Claude Code or Antigravity session cannot start another turn; the caller must start a new session (resume by the native session id is the existing path). The tests assert the terminal state (`stop()` returns `Ok`), not any recovery.
- The cancel-with-a-cancelled-turn-already-ended race (cancel arriving after the turn completed but before `finish_turn`) is not exercised.
- The pre-existing startup-frame flake in `tests/claude.rs` under default parallelism is left for its owner. It looks tied to the 2 s `startup-hooks-ready` poll and PowerShell start time under load, which is an inference, not an investigation.
