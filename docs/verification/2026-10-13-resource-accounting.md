# Resource accounting verification

Date completed: 2026-10-13 IST

Branch: `feat/resource-accounting`

Base: `feat/desktop-inbox` at `0d31153` (pull request #11, open against `main`)

Product version: `0.1.0`

Database schema: 15 (no migration)

Scope: the runtime's Job Object and process accounting readers, `ProviderSession::resources` through every provider session, the protocol types, the daemon's `session.resources` and `system.resources`, `lazooctl session resources` and `lazooctl system resources`, acceptance harness, and continuity documentation. The desktop app, Tauri surface, `desktop.snapshot`, `system.status`, `session.show`, and collaboration tools are unchanged.

## Outcome

PASS.

Section 20 of the brief says Lazoo displays each native provider harness's resource use separately from its own and never counts it against the 20 percent harness ceiling. The daemon now reads that use from Windows on request: `system.resources` reports the daemon's own process (PID, boot id, uptime, user and kernel CPU time and their sum, working set and its peak, handle count, live session count), and `session.resources { id }` reports a live session's native process tree from the Job Object Lazoo already owns for it (active and total processes, CPU time, peak process and job memory) beside the session's employee, provider, and native host PID. A session that is not live in this daemon is refused with `LZ-SESSION-NOT-LIVE` and its state; an unknown one is `LZ-SESSION-NOT-FOUND`. Both are point samples with a `sampledAt` timestamp: nothing is stored, no event is emitted, no limit or rate control is set on any job, and no collaboration tool can read either.

No live model turn was run.

## Design notes

| Decision | Reason |
|---|---|
| Point samples, no stored history or rolling window | Section 20.1's rolling 30-second and 5-minute windows are the enforcement slice's concern; a caller that wants a rate samples twice. Storing samples would be a new ledger with retention rules the brief does not ask for here. |
| Two new methods instead of fields on `system.status` or `session.show` | `system.status` is embedded in `desktop.snapshot`, which the owner boundary keeps unchanged; `session.show` is the durable record, and a sample is a reading, not a record. |
| Only a live session can be measured | The accounting lives in the Job Object handle this daemon holds; a suspended, failed, or prior-boot session has no handle, and inventing zeros would misreport. |
| Provider sessions keep their transport's job handle (`Option<Arc<JobObject>>`) | The transport is behind a mutex for I/O; the job handle is separate and cheap to clone, so a sample never waits on a turn. |
| `JobObject::active_processes` now reads through `accounting` | One query path for the whole tree's figures. |
| `K32GetProcessMemoryInfo` through the `Win32_System_ProcessStatus` feature | The psapi reader the `windows` crate exposes; no new dependency. |

## Fresh verification matrix

All commands were run from `C:\workspace\Orchestrator` on branch `feat/resource-accounting` after the final change.

| Command | Exit | Evidence |
|---|---:|---|
| `cargo fmt --all -- --check` | 0 | No formatting differences. |
| `cargo clippy --workspace --all-targets --locked -- -D warnings` | 0 | Clean with the one story-test allowance. |
| `cargo test --workspace --locked` | 0 | One invocation, 381 passed, 0 failed, 11 ignored (explicit live-provider tests), 67 test binaries. New: `lazoo-runtime` 2, `lazooctl` 1, `lazood` 2 (the resource story and the acceptance-script contract; 103 daemon tests). |
| `cargo build --workspace --release --locked` | 0 | Locked release build. |
| `scripts\acceptance\resource-accounting.ps1` | 0 | PASS schema-15 manifest, `deterministic-fake-transport`, 6 commands, every evidence flag true. |
| `desktop-inbox`, `configurable-thresholds`, `message-priority`, `message-arrival`, `user-notifications`, `incident-thresholds`, `lead-remediation`, `idle-suspension`, `activation-policy`, `coalesced-notices`, `peer-messaging`, `result-delivery`, `minimum-incidents`, `collaboration-tools`, `approval-gate`, `automatic-task-dispatch` | 0 | PASS schema-15 manifests. |
| `first-slice`, `employee-roster`, `collaboration-bus`, `conversation-delivery`, `task-delegation-graph`, each with `-SkipBuild` | 0 | PASS manifests against the real release binaries, with the schema-15 migration in the real daemon. |
| `scripts\acceptance\native-session-ownership.ps1 -SkipBuild -Provider codex` | 0 | PASS manifest on the first run, straight after `task-delegation-graph`; real Codex CLI, non-model session contract only. |
| `git diff --check` | 0 | No whitespace errors. |

Not run, as in every prior backend slice: `provider-adapters.ps1 -Provider codex` (live model turn) and `desktop-shell.ps1` (unchanged, separately owned UI). `npm --prefix apps\lazoo-desktop\ui run check` was not run because the frontend is unchanged.

## Resource-accounting acceptance evidence

`artifacts/acceptance/resource-accounting-manifest.json` records literal `PASS`, schema 15, and `deterministic-fake-transport`. Its focused commands prove:

- a real process assigned to a real Job Object reports at least one active process, a total at least as large, and a positive peak process and job memory while it runs, and after `terminate` reports zero active processes while its totals, peaks, and CPU time never go down;
- the test process reads its own PID, a positive working set no larger than its peak, a positive handle count, and CPU time that never goes down between two samples;
- `lazooctl session resources <id>` and `lazooctl system resources` map to `session.resources` and `system.resources`, and the session id is required;
- through the daemon, `system.resources` names the daemon's own PID and the boot id `system.status` reports, carries no session accounting, and counts live sessions as they are warmed and stopped; `session.resources` reports a warm session's tree (two active and three total processes, 1500 user and 250 kernel CPU milliseconds summed to 1750, the fixed peak memory) beside its employee, provider, and native host PID 4242; an unknown session is `LZ-SESSION-NOT-FOUND`, an extra parameter is `LZ-INVALID-PARAMS`, and the stopped session is `LZ-SESSION-NOT-LIVE` with state `suspended`; no event names a resource sample, and `session.show`, `system.status`, and the desktop snapshot carry no resource field; and
- schema is 15 and the desktop tree is unchanged.

## Boundary evidence

- `git diff --check` passed; both diffs under `apps/lazoo-desktop` are empty.
- No Tauri command, capability, `desktop.snapshot` field, frontend source, migration, event type, or collaboration tool changed; the daemon gains `session.resources` and `system.resources`, and `lazooctl` gains `session resources` and `system resources`.
- No job gained a limit, throttle, or CPU-rate control; the readers only query accounting Windows already keeps.
- `UI BACKEND CONTRACT: GO` remains intentionally withheld.

## Retained follow-up boundaries

Section 20.2's enforcement (an infrastructure job with CPU-rate control, soft-threshold throttling, queue backpressure, rolling-window measurement and the 20 percent gate), keeper and desktop self-accounting, a desktop surface for the samples, and everything else NEXT-SESSION lists remain separate reviewed slices.
