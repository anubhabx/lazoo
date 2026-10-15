# Resource Accounting Design

**Date:** 2026-10-13

**Status:** Implemented on `feat/resource-accounting` (stacked on `feat/desktop-inbox`, pull request #11) with deterministic acceptance, for owner review in a pull request before merge.

**Scope:** Backend-only, no schema change. The daemon reads Windows accounting on request and reports it through two new methods: `session.resources` (one live session's native process tree, from its Job Object) and `system.resources` (the daemon's own process). `lazooctl session resources <id>` and `lazooctl system resources` map to them. No desktop app, Tauri, `desktop.snapshot`, or collaboration-tool change; no enforcement; no live model use.

## 1. Outcome

Section 20 of the brief budgets "Lazoo itself" (daemon, desktop, keeper, CLI) against a 20 percent ceiling and says native provider harnesses "are measured separately and are not counted against the 20 percent harness ceiling, though Lazoo still displays their use." Until now Lazoo owned every employee's process tree in a Job Object (section 13) but displayed nothing about it, and measured nothing about itself.

The user can now:

1. `lazooctl system resources`: the daemon's own PID, CPU time (user, kernel, and their sum), working set and its peak, handle count, uptime, boot id, and how many sessions are live in it.
2. `lazooctl session resources <id>`: for a session live in this daemon, the Job Object accounting of its native process tree: active and total processes, CPU time, and peak process and job memory, beside the session's employee, provider, and native host PID.

Both are point-in-time samples with a `sampledAt` timestamp. Nothing is stored, no event is emitted, and no collaboration tool can read either.

## 2. Rule

- `lazoo_runtime::JobObject::accounting` reads `JobObjectBasicAccountingInformation` (processes and CPU time) and `JobObjectExtendedLimitInformation` (peak memory) from the job Lazoo already owns per session. Windows keeps that accounting for the whole tree and keeps it after the tree ends, until the job handle closes.
- `lazoo_runtime::current_process_accounting` reads `GetProcessTimes`, `K32GetProcessMemoryInfo`, and `GetProcessHandleCount` for the calling process.
- Every provider session keeps its transport's job handle and answers `ProviderSession::resources`; the daemon's `ManagedSession` carries it through, with `None` for a session that owns no job.
- `session.resources` requires the session to be live in this daemon (started here and not stopped); any other session is `LZ-SESSION-NOT-LIVE` with its state, because there is no tree to measure. An unknown session is `LZ-SESSION-NOT-FOUND`.
- A provider's use is reported beside the daemon's, never folded into it: `system.resources` counts live sessions but carries none of their accounting.

## 3. Rules kept

- `desktop.snapshot`, `system.status`, `session.show`, and `session.list` are unchanged; the samples live only behind the two new methods.
- No limit, throttle, or CPU-rate control is set on any job; this slice measures and displays, and section 20.2's enforcement stays a separate slice.
- No rolling 30-second or 5-minute window is kept; the daemon reports what Windows reports now, and a caller that wants a rate samples twice.

## 4. Deterministic acceptance

`scripts/acceptance/resource-accounting.ps1` runs the runtime, CLI, and daemon fake-backend proofs and writes `artifacts/acceptance/resource-accounting-manifest.json`. The runtime proof assigns a real process to a real Job Object, reads its accounting live and after termination, and reads the test process's own accounting; the daemon proof uses a fake session that reports fixed accounting. No provider smoke or live model turn.

## 5. Explicitly deferred work

- the section 20.2 enforcement: an infrastructure job with CPU-rate control, background throttling at the soft threshold, and queue backpressure;
- rolling-window measurement and the 20 percent gate under idle, routing, failure storm, indexing, terminal, and voice scenarios;
- keeper and desktop self-accounting (separate processes the daemon does not own);
- a desktop surface for the samples (the separately owned UI).

## 6. Review gate

Merge authorizes only the two read-only methods, the two CLI commands, and the runtime accounting readers above; it does not authorize any limit on a job, any stored sample, or desktop changes.
