# Incident thresholds verification

Date completed: 2026-10-10 IST

Branch: `feat/incident-thresholds`

Base: `feat/lead-remediation` at `619244e` (pull request #3, open, itself stacked on #2)

Product version: `0.1.0`

Database schema: 12 (unchanged)

Scope: the per-code persistent-failure threshold in `lazoo-domain`, born-priority promotion and per-code folding in the store, acceptance harness, and continuity documentation. No migration, protocol, tool, CLI, RPC, or desktop change.

## Outcome

PASS.

The brief's section 14.2 default thresholds now apply per code. `LZ-SESSION-STALLED` (missed progress evidence past the adapter's deadline), `LZ-DAEMON-UNAVAILABLE` (a daemon crash the keeper recorded), and an `LZ-PROCESS-EXIT` whose incident names a task (an employee process death under assigned work) are promoted to `priority` by their first occurrence; any other `LZ-PROCESS-EXIT` by its second; every other code by three inside five minutes as before. An incident promoted by its first occurrence is created as `priority` with the same promotion escalation a fold applies (a lead window on a child of an active root, otherwise the user with `no-lead`), and its single event is `incident.escalated`.

The dead-lettered priority message threshold is not implemented: message envelopes have no priority field to key it on. Configurable thresholds are not implemented.

No live model turn was run.

## Design notes

| Decision | Reason |
|---|---|
| A threshold table keyed by code, plus whether the incident names a task | Every section 14.2 case maps onto an existing code; "while assigned work is active" is the incident naming a task, because the daemon raises the incident right after the failed turn or dispatch that held the work. No schema change is needed. |
| A threshold of one creates the incident as `priority` | Promotion must not wait for a second failure that may never come; the creating occurrence runs the same `promotion_escalation` as a fold. |
| A born-priority incident's event is `incident.escalated` | Every promotion is announced by the same event type, whichever occurrence caused it. |
| The generic store tests moved to `LZ-MESSAGE-TIMEOUT` | They exercise the three-in-five-minutes fold and the acknowledge, resolve, and reopen rules; stalls and exits now have dedicated tests. |

## Fresh verification matrix

All commands were run from `C:\workspace\Orchestrator` on branch `feat/incident-thresholds` after the final change.

| Command | Exit | Evidence |
|---|---:|---|
| `cargo fmt --all -- --check` | 0 | No formatting differences. |
| `cargo clippy --workspace --all-targets --locked -- -D warnings` | 0 | No lint fixes were needed. |
| `cargo test --workspace --locked` | 0 | One invocation, 348 passed, 0 failed, 11 ignored (explicit live-provider tests). New: `lazoo-domain` 1 (the threshold table), `lazoo-store` 2 (task-less stalls, crashes, and second exits; a stall or death on a child opening the lead window), `lazood` 1 (the acceptance-script contract; 89 daemon tests) and 1 extended (the keeper crash folded at startup is priority and the user's). |
| `cargo build --workspace --release --locked` | 0 | Locked release build. |
| `scripts\acceptance\incident-thresholds.ps1` | 0 | PASS schema-12 manifest, `deterministic-fake-transport`, 10 commands, every evidence flag true. |
| `scripts\acceptance\lead-remediation.ps1` | 0 | PASS schema-12 manifest; remediation unchanged. |
| `scripts\acceptance\minimum-incidents.ps1` | 0 | PASS schema-12 manifest; fold, acknowledge, resolve, and keeper spool import unchanged apart from the crash's priority. |
| `idle-suspension`, `activation-policy`, `coalesced-notices`, `peer-messaging`, `result-delivery`, `collaboration-tools`, `approval-gate`, `automatic-task-dispatch` | 0 | PASS schema-12 manifests. |
| `first-slice`, `employee-roster`, `collaboration-bus`, `conversation-delivery`, `task-delegation-graph`, each with `-SkipBuild` | 0 | PASS manifests against the real release binaries. |
| `scripts\acceptance\native-session-ownership.ps1 -SkipBuild -Provider codex` | 1, then 0, 0 | The first run, immediately after `task-delegation-graph`, exited 1; its output was not captured. Two immediate reruns passed. This slice changes no session, provider, or tool code; the owner's own VS Code Codex app-server was running on the machine throughout. Treat it as a flake to watch, not as proven stable. |
| `git diff --check` | 0 | No whitespace errors. |

Not run, as in every prior backend slice: `provider-adapters.ps1 -Provider codex` (live model turn) and `desktop-shell.ps1` (unchanged, separately owned UI). `npm --prefix apps\lazoo-desktop\ui run check` was not run because `node_modules` is absent; the frontend is unchanged.

## Incident-thresholds acceptance evidence

`artifacts/acceptance/incident-thresholds-manifest.json` records literal `PASS`, schema 12, and `deterministic-fake-transport`. Its focused commands prove:

- the threshold table: stalls and crashes at one with or without a task, a process exit at one with a task and two without, and every other code at three;
- the three-in-five-minutes window is unchanged for other codes, including fold, replay, and the generic escalation;
- a task-less stall or daemon crash is born `priority` and goes to the user as `no-lead`, a first unexpected exit stays normal and the second promotes it, and a later stall folds without a second promotion;
- a stall or a death under assigned work on a child is born `priority` with a lead window, both are queued to the lead, and a timeout on the same child still needs three;
- the keeper's crash record folded at daemon startup is a priority incident for the user;
- the lead's remediation window and the escalation sweep are unchanged; and
- schema is 12 and the desktop tree is unchanged.

## Boundary evidence

- `git diff --check` passed; both diffs under `apps/lazoo-desktop` are empty.
- No migration, protocol, tool, CLI, RPC, Tauri, or frontend change.
- Promotion never retries, restarts, re-authenticates, or changes authority.
- `UI BACKEND CONTRACT: GO` remains intentionally withheld.

## Retained follow-up boundaries

The dead-lettered priority message threshold (needs a message priority field), configurable thresholds and windows, a user notification channel, the message-arrival notification, the live Codex gate, and everything else NEXT-SESSION lists remain separate reviewed slices.
