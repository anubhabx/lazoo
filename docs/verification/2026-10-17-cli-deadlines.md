# CLI deadlines verification

Date completed: 2026-10-17 IST

Branch: `worktree-wf_0f2eb737-354-2` (a dedicated worktree branch; the integrator merges it into `main`)

Base: `f98ab93` on `feat/core-v1-completion` (schema 18; `codex/core-v1-backend` stops at `610bdbb`, two commits earlier, and does not contain this base)

Product version: `0.1.0`

Database schema: 19 after integration (this slice adds no migration; the sibling raw-frame slice added 0019, and the integrator bumped this manifest literal to 19 on `feat/core-v1-completion`)

Scope: `lazooctl`'s request write and response read get deadlines through one `round_trip` helper with a per-method `response_timeout`; `InfrastructureGovernor::attach_current` stops waiting on the initialization mutex after 5 s, returns the new `GovernorError::InitializationTimedOut`, and frees its security descriptor on every exit path; `lazooctl` gains `tempfile` as a dev-dependency for an in-process stalled-daemon test; `scripts/test-cli-stress.ps1` names an IPC client deadline in its failure message when one occurs and keeps the stderr of exit-0 calls in its manifest. No protocol, RPC, error code, store, migration, or desktop change. The heap-corruption audit outcome is recorded below without a code change.

## Outcome

PASS for what this slice claims: a daemon that never reads or never answers now ends a `lazooctl` call at a known deadline with the existing `LZ-IPC` code (exit 3) and a message naming the phase (`IPC I/O failed: daemon request write timed out` or `IPC I/O failed: daemon response timed out`), and a main that cannot take the governor's initialization lock runs un-enrolled after 5 s and says so (`infrastructure CPU cap unavailable: infrastructure job initialization lock timed out`) instead of blocking forever.

This bounds and labels the hang. It is not a causal fix for the stress run that timed out on `--json status` after 850 successful calls (`artifacts/acceptance/cli-stress-9ba572a0bfd4405fac774bcf2891dcd2.json`), and it is not a fix for the 0xC0000374 heap-corruption exit recorded on 2026-10-13. Neither has an established cause. What changes is that a stall in the request write, the response read, or the handshake becomes a recorded exit-3 failure with a phase on stderr, below the stress harness's 30 s kill, rather than an opaque kill with empty output. A stall on the governor's initialization lock is different: after 5 s the call runs un-enrolled and exits 0 with the lock message on stderr, which the harness used to discard; its manifest now keeps that stderr under `stderrOnSuccess`, so the phase is recorded inside a PASS rather than lost.

No live model turn was run. No release binary was built in this worktree.

## Design notes

| Decision | Reason |
|---|---|
| Reuse `IpcError::Io(TimedOut)`, code `LZ-IPC`, exit 3, instead of a new code | It is what the handshake timeout (`connection.rs`, "server handshake timed out") and the daemon's own request/response deadlines (`server.rs`) already produce; scripts learn no new exit code, and the message names the phase. |
| Request write deadline 5 s | Mirrors the daemon's `REQUEST_TIMEOUT`: a request it has not read in 5 s is dropped there anyway, so waiting longer to write cannot succeed. |
| 20 s response deadline for `system.status`, `system.resources`, `system.budget`, `project.list`, `events.list`, `roster.get`, `provider.list` | Read-only ledger queries that answer from the store (5 s SQLite busy timeout). A stall in this one phase is recorded below the stress harness's 30 s `WaitForExit`. The bounds before it add in series (installation lock retry 2 s in `main` and again in `execute_with`, governor lock 5 s, pipe open 3 s, handshake 5 s, request write 5 s, then this 20 s: 42 s plus process start), so a call that stalls in several phases can still be killed by the harness. |
| 600 s response deadline for every other non-follow RPC | Above the 300 s default provider turn deadline (`lazoo-providers` contract) plus native start/stop, probe and worktree time. The two tiers are checked against those fixed points in a unit test; raise the long tier only together with the turn deadline. |
| `events.subscribe` follow loop stays unbounded | The user ends a follow; a deadline there would be a bug. Said in a comment at the loop. |
| Governor initialization lock waits 5 s, then `InitializationTimedOut` | Configuring the job takes milliseconds. Every caller (`lazooctl` main, `lazood` server, keeper supervisor) already treats a governor error as non-fatal and prints or traces it, so no caller changed. The policy moves from wait-forever to run-un-enrolled-after-5 s. |
| The timeout check happens only on `WAIT_TIMEOUT`; `WAIT_FAILED` keeps `Error::from_thread()` | The two failures mean different things and the existing path stays as it was. |
| Governor test runs the attach in a child process | An attach that unexpectedly succeeded would enroll the `cargo test` process in the 20% hard-capped job. The existing `LAZOO_GOVERNOR_CHILD` pattern keeps the parent out. The lock is held on one thread and the attach runs on another, because a Windows mutex is re-entrant for its owner. |
| In-process stalled daemon for the CLI test | A `PipeServer` that reads the request and never replies, with a 200 ms deadline passed to `round_trip`, proves the mechanism in 0.23 s; a real-binary proof would cost 20 s per run for the same evidence. |
| No new abstraction for the deadline | One private async helper and one private `fn response_timeout(method)`; the two `timeout(...)` calls use the same `tokio::time::timeout` plus `IpcError::Io(TimedOut)` pattern as the daemon. |

A client-side deadline does not cancel daemon work. A mutating RPC that times out on the client may still commit server-side; the long tier exceeds the server's own deadlines so this needs a stalled daemon, and the existing `--idempotency-key` flags are the retry path.

## Fresh verification matrix

All commands were run from the worktree `C:\workspace\Orchestrator\.claude\worktrees\wf_0f2eb737-354-2` with `CARGO_TARGET_DIR=C:\workspace\Orchestrator\target` after the final change.

| Command | Exit | Evidence |
|---|---:|---|
| `cargo fmt --all -- --check` | 0 | No formatting differences. |
| `cargo clippy -p lazooctl -p lazoo-runtime -p lazood --all-targets --locked -- -D warnings` | 0 | Clean. |
| `cargo test -p lazooctl --lib tests::stalled_daemon_response_times_out_cleanly --locked -- --exact` | 0 | 1 passed in 0.23 s with a 200 ms deadline: `CliError::Ipc(IpcError::Io)` of kind `TimedOut`, message `daemon response timed out`, `exit_code() == 3`, display `IPC I/O failed: daemon response timed out`. |
| `cargo test -p lazooctl --lib tests::response_timeout_tiers_read_only_queries_short_and_everything_else_long --locked -- --exact` | 0 | 1 passed: the seven read-only methods map to 20 s, `provider.smoke`/`task.create`/`session.warm`/`roster.refresh` to 600 s; 5 s equals the daemon's request timeout, 20 s is under 30 s, 600 s is over 300 s. |
| `cargo test -p lazooctl --locked` | 0 | Whole crate: 4 lib unit tests, 28 `cli`, 1 `configuration`, 1 `memory`, 0 doc tests; 0 failed. |
| `cargo test -p lazoo-runtime --test governor --locked` | 0 | 7 passed, 0 failed, including `initialization_lock_held_elsewhere_times_out_in_child_helper` (child reported `InitializationTimedOut` after about 5.0 s; the binary finished in 5.02 s). Each child-helper test also runs once nested in its child process and is not counted twice here. |
| `cargo test -p lazoo-ipc --locked` | 0 | 1 lib unit test and 10 `ipc` tests passed, 0 failed; the IPC crate is unchanged and still passes. |
| `cargo test -p lazood --test cli_deadlines_contract --locked` | 0 | 1 passed: the acceptance script names the three tests exactly with `--locked -- --exact`, carries every evidence flag, the desktop diff guard, `deterministic-fake-transport`, `schemaVersion = 18`, and the `UNBOUNDED_BY_DESIGN`, `NOT_ESTABLISHED`, `NOT_FIXED_NO_FIRST_PARTY_DEFECT_FOUND` boundaries; it contains no provider smoke, `LivePrompt`, `test-cli-stress`, or `--release`. |
| `scripts\acceptance\cli-deadlines.ps1` | 0 | PASS manifest at `artifacts/acceptance/cli-deadlines-manifest.json`: schema 18, `deterministic-fake-transport`, 4 commands, every evidence flag true. |
| `[System.Management.Automation.Language.Parser]::ParseFile` on `scripts/test-cli-stress.ps1` and `scripts/acceptance/cli-deadlines.ps1` | 0 | 0 parse errors each. This is the only check of the stress-script change; the script itself was not executed (it needs a release build). |
| `git diff --check` | 0 | No whitespace errors. |

Not run, deliberately: `cargo test --workspace`, `scripts/gate.ps1`, `scripts/test-cli-stress.ps1`, `scripts/acceptance/task-delegation-graph.ps1` and every other real-binary script (they need a release build; the integrator runs the full gate after merging), `provider-adapters.ps1` (live model turn), `desktop-shell.ps1` and `npm --prefix apps\lazoo-desktop\ui run check` (unchanged, separately owned UI). The request-write deadline is exercised by no test: the in-process pipe accepts a small request frame at once, so only the response-read deadline and the tier table are proven; the write deadline is the same `timeout` call with a different message.

## CLI deadlines acceptance evidence

`artifacts/acceptance/cli-deadlines-manifest.json` records literal `PASS`, schema 18, and `deterministic-fake-transport`. Its commands prove:

- a daemon that reads the request and never replies ends the CLI call at the deadline with the labelled `LZ-IPC` timeout and exit 3 (`stalledDaemonResponseIsLabelledIpcTimeoutExitThree`);
- the read-only tier is under the stress harness's kill and the other tier is above the provider turn deadline (`readOnlyQueriesShortTierUnderHarnessKill`, `otherRpcsLongTierAboveProviderTurn`);
- a held initialization lock produces `InitializationTimedOut` after about 5 s in a child process (`governorInitLockTimesOutInsteadOfWaitingForever`);
- the desktop tree is unchanged (`uiChangesAbsent`), and no provider or external model was used (`providerSmokeAbsent`, `externalModelUseAbsent`).

Its boundaries record what is not claimed: `eventsFollow = UNBOUNDED_BY_DESIGN`, `cliStressTimeoutCause = NOT_ESTABLISHED`, `heapCorruptionExit = NOT_FIXED_NO_FIRST_PARTY_DEFECT_FOUND`.

## Heap-corruption audit verdict

Recorded here because the owner asked for the outcome; no code changed for it.

- The exit was observed once in roughly 2,200 calls on 2026-10-13: `lazooctl project list` printed its result and then exited with status -1073740940 (0xC0000374) at process exit (`docs/verification/2026-10-13-resource-budget-failure-storm.md`, commit `b15945e`). It did not reproduce in the later 4,096-call runs.
- The governor is ruled out by history, re-checked in this worktree: `git show b15945e:apps/lazooctl/src/main.rs` has no governor attach, and `crates/lazoo-runtime/src/governor.rs` first appears in `2521809` (2026-10-16), three days after the crash.
- `apps/lazooctl/src` contains no `unsafe` code (checked with `Select-String`). The first-party `unsafe` on the CLI path (`OwnerSecurity::current`, `apply_to_path`, DPAPI `crypt_data` in `security.rs`; `SingleInstance`; the governor's `LocalFree` and `MutexGuard`) was audited in the slice map: token buffers are sized from the returned `required` length and no overrun was found. The audit's "each allocation is freed exactly once" was wrong for one path, found in review: `attach_current` returned through `?` on a `CreateMutexW` failure, and on the new lock timeout, before its `LocalFree`, leaking the security descriptor once per failed attach. It is now freed by a drop guard on every path (`DescriptorGuard`, the `HandleGuard` pattern). The leak postdates the crash (no governor at `b15945e`) and is a leak, not an overrun, so it does not change the verdict. No other first-party defect was found.
- The crash came after output was printed, so it sits in connection drop, tokio runtime drop, or `ExitProcess`. The one remaining candidate is third-party and low confidence: mio's `NamedPipe` drop cancelling a pending overlapped read while the runtime and its IOCP are torn down. mio is designed to be memory-safe there (it leaks the `Arc<Inner>` and keeps the buffer alive), so this is a candidate, not a finding.
- Not proven: the cause. This read-only audit cannot see earlier in-process corruption that the heap check only reports at exit. A recurrence under `scripts/test-cli-stress.ps1` would appear as exit code -1073740940 in `exitCodes` with its stdout/stderr in `failures`. WER LocalDumps was not enabled; the owner's machine state was not changed.

## Boundary evidence

- `git diff --check` passed; both diffs under `apps/lazoo-desktop` are empty.
- No Tauri command, capability, `desktop.snapshot` field, frontend source, migration, protocol type, RPC method, or error code changed. `GovernorError` gains one variant and `lazoo-runtime` one private drop guard; `lazooctl` gains three private constants, two private functions, and a dev-dependency.
- `Cargo.lock` changes by one line: `tempfile` under `lazooctl`'s dependencies.
- The stress script changes in two places. When any failed invocation's stderr carries an `IPC I/O failed: ... timed out` message, the thrown message names that deadline and the count instead of the bare `N CLI invocations failed`; the governor's non-fatal `infrastructure job initialization lock timed out` line does not match that pattern, so a call that printed it and then failed for another reason is not misclassified. The manifest gains `stderrOnSuccess`: the first 50 exit-0 calls with non-empty stderr (arguments, duration, stderr), which is where a governor lock timeout appears. Its assertions, counts, other manifest fields, and cleanup are unchanged. Both literals are pinned by `cli_stress_harness_keeps_exit_zero_stderr_and_classifies_ipc_deadlines_only`.
- `UI BACKEND CONTRACT: GO` remains intentionally withheld.

## Retained follow-up boundaries

- The cause of the 850-call `--json status` timeout stays open. A recurrence under the stress harness appears in one of three shapes: an exit-3 failure whose stderr carries `IPC I/O failed: daemon request write timed out`, `daemon response timed out`, or `server handshake timed out`; an exit-0 entry in the manifest's `stderrOnSuccess` carrying `infrastructure job initialization lock timed out` with a duration near 5 s, so a PASS with a non-empty `stderrOnSuccess` is a reproduction of the governor phase, not a clean run; or, when a call stalls in more than one phase, still a kill at 30 s with a `timeoutDiagnostic` and empty output. Classify it then.
- `App::handle(request)` has no server-side deadline (`apps/lazood/src/server.rs`); only the request read and the response write are bounded there. The client bound is the only bound on handler time.
- `system.resources` awaits the daemon's `live_sessions` lock; with live sessions it could legitimately exceed 20 s. This is an inference from the code, not an observation; the stress run has no sessions.
- Enabling WER LocalDumps for `lazooctl.exe` during stress runs is an owner decision about machine state and was not done.
- The acceptance manifest literal was bumped to `schemaVersion = 19` by the integrator on `feat/core-v1-completion`, together with the sibling slices.
