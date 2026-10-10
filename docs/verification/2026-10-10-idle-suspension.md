# Idle suspension verification

Date completed: 2026-10-10 IST

Branch: `feat/idle-suspension`

Base: `main` at `6b7f006` (`Merge pull request #1 from anubhabx/feat/activation-policy`)

Product version: `0.1.0`

Database schema: 11 (`0011_idle_suspension.sql` adds `employees.idle_timeout_seconds`, default 0, checked to 0 or 60-86400)

Scope: the `idleTimeoutSeconds` protocol field, the domain bound and due rule, the schema-11 column through the store, the store's guarded idle stop, the daemon's idle sweep, delivery on the recipient's current session, the `--idle-timeout-seconds` CLI flag, acceptance harness, and continuity documentation. Desktop product code and contract and the tool catalog are unchanged.

## Outcome

PASS, with two gate commands run in a different shape because the host disk was full (see "Host constraints").

The local user can now give an employee an idle timeout with `lazooctl employee update <id> --expected-revision N --idle-timeout-seconds 900`. Every 15 seconds the daemon finds idle sessions owned by this boot whose employee's timeout has passed since the session started or a turn on it last changed, and stops each one through the ordinary stop path under the `session.stop` key `idle-suspend:<sessionId>`; the roster then reports the employee `suspended`. A session with a submitting or accepted turn on it is never due, and the store re-checks that in the same transaction that marks the session `stopping`; a turn still `recorded` resets the quiet clock but does not hold the session. The next user turn activates the employee with a fresh session; a `notice-turns` employee is woken by its next notice turn exactly once, as before.

Every employee written before schema 11 reads 0, and under 0 nothing changed: a warmed session stays up until the user stops it or the daemon shuts down. A timeout change is revisioned like every other employee setting and refused while the employee has an active session.

Delivery of a recorded turn now resolves the recipient's current idle session instead of the session named when the turn was recorded. Before, a turn recorded against a session that was stopped before delivery could only fail with `LZ-TURN-SESSION-HANDLE-MISSING`, for a user stop as well as for idle suspension; the new daemon test fails on the old code and passes on the new.

No live model turn was run.

## Design notes

Design and implementation landed together under the owner's standing instruction to continue the build; `docs/superpowers/specs/2026-10-10-idle-suspension-design.md` and `docs/superpowers/plans/2026-10-10-idle-suspension.md` record the contract for owner review in the pull request.

| Decision | Reason |
|---|---|
| The timeout is a defaulted, checked employee column; 0 means never | It is the user's per-employee decision, revisioned and refused during an active session like `activation_policy`; the `DEFAULT` keeps every existing row and every raw insert in the upgrade tests at 0, and a plain integer lets `employee update` clear it with 0 instead of a nullable-option wire shape. |
| Due-ness is re-checked inside `begin_idle_session_stop` | `begin_turn_submission` already refuses a session that is not `idle`, so checking "no unfinished turn" and setting `stopping` in one immediate transaction is enough to make the sweep and turn delivery race-free without any lock in the daemon. |
| A `recorded` turn resets the quiet clock but does not hold the session | A turn can stay `recorded` indefinitely (a follow-up refused with `LZ-TURN-TASK-SCOPE` does); counting it as in flight would keep that session's provider process up forever. With delivery resolving the current session, suspending under it costs at most a fresh activation. Found in self-review; the first draft counted it as in flight. |
| The sweep selects `state = 'idle' AND owner_boot_id = ?` in SQL | Every dispatch leaves a session row behind, so scanning the whole session history every 15 seconds would grow without bound. Found in self-review. |
| The stop reuses `session.stop`, keyed `idle-suspend:<sessionId>` | The session ledger names why each session stopped, replay is free, and the provider stop, the ten-second bound, and the `LZ-SESSION-STOP-FAILED` incident are the user stop's own code (`finish_session_stop_command`). |
| `delivery_target` resolves the current session from the roster | It is the only place a turn's recorded session name was trusted after the fact; the roster's current idle session equals that name whenever the named session is still idle, so nothing else changes. |
| A fixed 15-second sweep, aborted before `shutdown_live_sessions` | Each sweep is one read per idle session; sleeping until the earliest deadline is a later refinement. Aborting first keeps the sweep and shutdown from stopping the same session concurrently. |
| `quiet_since` counts from the last change to any turn on the session | A turn's `updated_at` moves with every provider event, so a session is quiet only after its last turn's terminal event. |

## Fresh verification matrix

All commands were run from `C:\workspace\Orchestrator` on branch `feat/idle-suspension` after the final fixes.

| Command | Exit | Evidence |
|---|---:|---|
| `cargo fmt --all -- --check` | 0 | No formatting differences. |
| `cargo clippy --workspace --all-targets --locked -- -D warnings` | 0 | All workspace targets linted with warnings denied; no new lint fixes were needed. |
| `cargo test -p <member> --locked`, for every workspace member | 0 | Every member passed: `lazood` 85 daemon integration tests (5 new: the suspension scenario, the unfinished-turn and zero-timeout scenario, the stranded-turn scenario, the `notice-turns` wake-after-suspension scenario, and the acceptance-script contract), `lazoo-store` 75 (2 new: idle-timeout employee column, idle session due and stop), `lazoo-protocol` 26 (1 new), `lazoo-domain` 44 (1 new), `lazooctl` 20 (1 extended), `lazoo-providers` 44, `lazoo-keeper` 7, `lazoo-runtime` 12, `lazoo-ipc` 9, `lazoo-desktop` 12, `lazoo-session-host` 1. Explicit live-provider tests remained ignored. Run with `CARGO_INCREMENTAL=0` and `CARGO_PROFILE_DEV_DEBUG=0`. |
| `cargo build --workspace --release --locked` | 0 | Locked release build. |
| `scripts\acceptance\idle-suspension.ps1` | 0 | PASS schema-11 manifest, `deterministic-fake-transport`, 11 commands, every evidence flag true. |
| `scripts\acceptance\activation-policy.ps1` | 0 | PASS schema-11 manifest; policy activation unchanged. |
| `scripts\acceptance\coalesced-notices.ps1` | 0 | PASS schema-11 manifest. |
| `scripts\acceptance\peer-messaging.ps1` | 0 | PASS schema-11 manifest. |
| `scripts\acceptance\result-delivery.ps1` | 0 | PASS schema-11 manifest. |
| `scripts\acceptance\minimum-incidents.ps1` | 0 | PASS schema-11 manifest. |
| `scripts\acceptance\collaboration-tools.ps1` | 0 | PASS schema-11 manifest; the tool catalog is unchanged. |
| `scripts\acceptance\approval-gate.ps1` | 0 | PASS schema-11 manifest. |
| `scripts\acceptance\automatic-task-dispatch.ps1` | 0 | PASS schema-11 fake-transport manifest. |
| `scripts\acceptance\first-slice.ps1 -SkipBuild` | 0 | PASS manifest; database schema 11. |
| `scripts\acceptance\employee-roster.ps1 -SkipBuild` | 0 | PASS manifest; employee create, update, and roster round-trip through the real release daemon and CLI on the schema-11 employee record. |
| `scripts\acceptance\collaboration-bus.ps1 -SkipBuild` | 0 | PASS manifest. |
| `scripts\acceptance\conversation-delivery.ps1 -SkipBuild` | 0 | PASS deterministic manifest; user-authored activation unchanged. |
| `scripts\acceptance\task-delegation-graph.ps1 -SkipBuild` | 0 | PASS manifest, run against the real daemon with the notice loop and idle sweep active. |
| `scripts\acceptance\native-session-ownership.ps1 -SkipBuild -Provider codex` | 0 | PASS manifest; real Codex CLI accepting the eight declared tools, non-model session contract only. |
| `git diff --check` | 0 | No whitespace errors. |

All fifteen acceptance scripts ran in the foreground in small groups; no daemon process was left behind.

Self-review then changed the sweep (in-flight means `submitting` or `accepted` only, and idle sessions are selected in SQL; see Design notes) and reworked the stranded-turn test to stop its session through idle suspension. After that change `cargo fmt --all -- --check`, clippy with `-D warnings`, the full `lazoo-store` suite (75) and `lazood` suite (86, including the task-graph acceptance against the real release daemon), `cargo build --workspace --release --locked`, and `idle-suspension.ps1` were all re-run and passed, and the reworked test was again confirmed to fail on the old delivery code. The other fourteen scripts' manifests come from the run just before that change; it touches only the idle sweep, which never stops anything for their employees (timeout 0).

Not run, as in every prior backend slice: `provider-adapters.ps1 -Provider codex` requires a live model turn (`LAZOO_CODEX_OK`) that the handoff does not authorize, and `desktop-shell.ps1` asserts against the unchanged, separately owned UI; their tracked manifests were left as committed.

## Host constraints

Drive C: was full when the gate started (`LNK1180: insufficient disk space`, then `os error 112`); 23 GB of it sits in `%TEMP%\Adobe`, which this session did not touch. Only the repository's own regenerable `target` directory was cleared, and the gate was sequenced to fit:

- `cargo test --workspace --locked` in one invocation could not finish: unifying features across all members rebuilds the `windows` and Tauri crates in additional variants, and that link ran the disk out. The same tests were run member by member instead (row above), each green; the one-invocation run should be repeated once the disk has room.
- Debug builds ran with `CARGO_INCREMENTAL=0` and `CARGO_PROFILE_DEV_DEBUG=0`, which change only build caching and debug symbols, not test behavior.
- `npm --prefix apps\lazoo-desktop\ui run check` was not run: `node_modules` is absent on this machine and reinstalling it would consume the space the gate needed. The frontend is unchanged (both diffs under `apps/lazoo-desktop` are empty) and the `lazoo-desktop` crate's 12 tests passed.
- The release binaries were built before the `-SkipBuild` scripts and removed after them, and the test-based scripts ran afterwards on fresh debug builds.

## Idle-suspension acceptance evidence

`artifacts/acceptance/idle-suspension-manifest.json` records literal `PASS`, schema 11, and `deterministic-fake-transport`. Its focused commands prove:

- `idleTimeoutSeconds` defaults to 0 on `employee.create`, an `employee.update` without it keeps the stored value, and 0 or any explicit value is carried;
- only 0 or 60 to 86400 seconds validate, the rule never fires for 0, fires only once the quiet time reaches the timeout, and treats a clock that went backwards as not quiet;
- a timeout change bumps the employee's configuration revision, an identical change bumps nothing and emits no event, the schema refuses 30 seconds, and a row inserted without the column reads 0;
- `lazooctl employee create` sends `idleTimeoutSeconds: 0`, `employee update --idle-timeout-seconds 900` maps it, and an update without the flag sends nothing;
- a session is due only after its employee's whole timeout, never under a 0 timeout, never for another boot, is not stopped early, is stopped once under `idle-suspend:<sessionId>`, and a user stop with that key replays it;
- a quiet session is stopped with exactly one provider stop, becomes `suspended` without a failure, is reported `suspended` on the roster with `session.stopping` and `session.suspended` events, nothing further is stopped or started, and the next user turn starts a fresh session;
- a session whose turn is still in flight is not stopped whatever the clock says, quiet time counts from the end of that turn, and a default 0 timeout keeps a warmed session up for thirty days of quiet;
- a user turn left `recorded` against a session does not hold it: the session is suspended for idleness, and the turn is then delivered, once, on the session the user warmed afterwards (on the old delivery code this test fails with the turn stranded);
- a `notice-turns` lead whose session its own idle timeout stopped after a delegation is woken exactly once by its child's result, completes its task, and is stopped again; and
- schema is 11 and the desktop tree is unchanged.

## Boundary evidence

- `git diff --check` passed.
- Both staged and unstaged diffs under `apps/lazoo-desktop` are empty.
- No Tauri command, capability, `desktop.snapshot` field, frontend source, or tool declaration changed; the only migration is the one defaulted column.
- No tool can read or set the idle timeout; the sweep never starts a session, resumes a thread, retries a stop, widens authority, approves anything, changes task ownership, or deletes data.
- This slice is submitted as a pull request against `main` rather than a local fast-forward.
- `UI BACKEND CONTRACT: GO` remains intentionally withheld.

## Retained follow-up boundaries

A deadline-driven sweep, project or global idle defaults, a message-arrival notification replacing the 250 ms polls, a live Codex tool-call, peer-message, coalesced-notice, policy-activation, and idle-suspension gate, activation triggers other than a recorded notice turn, activation budgets, cross-tree and user-to-employee messaging through the tools, bounded lead remediation, user notification channels, typed desktop Inbox binding, ConPTY, per-capability policy, managed memory, scheduling, and resource governance remain separate reviewed slices.
