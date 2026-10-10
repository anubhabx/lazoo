# User notifications verification

Date completed: 2026-10-10 IST

Branch: `feat/user-notifications`

Base: `feat/incident-thresholds` at `67024ef` (pull request #4, open, stacked on #3 and #2)

Product version: `0.1.0`

Database schema: 13 (`0013_user_notifications.sql` adds `incidents.user_notified_at` and `incidents.user_notification`)

Scope: the `IncidentUserNotification` protocol field, the two schema-13 columns, the store's claim and settle, the daemon's `UserNotifier` and announcement pass in the timed sweep, acceptance harness, and continuity documentation. The desktop app, Tauri surface, tools, CLI, and RPC methods are unchanged.

## Outcome

PASS.

Every open incident that reaches `escalation: user` is now announced on the Windows desktop at most once, with the toast the keeper already uses. The timed sweep claims every unannounced open user escalation (oldest first, up to 64 per pass) in one transaction, shows one notification for them (the incident's own title, explanation, and next action when it is alone, bounded to 240 bytes, or "Lazoo: N incidents need you" with their titles), and records `shown` or `failed`. A keeper-sourced record is settled as `keeper` without a second notification. A refused notification is logged and never retried. A lead window is announced only once it escalates. Incidents escalated before schema 13 start unannounced and are named together in the first pass.

No test shows a real notification: the daemon tests replace the notifier with a recorder through `App::with_user_notifier`.

No live model turn was run.

## Design notes

| Decision | Reason |
|---|---|
| Claim, then show, then settle | The claim commits before anything is shown, so a crash can never announce twice; it can leave `claimed`, which says honestly that the outcome is unknown. |
| One notification per pass | A burst of escalations, or the first pass after an upgrade, becomes one notification with a count instead of a flood (section 14.4's "one recovery summary rather than a flood"). |
| Keeper records settle as `keeper` | The keeper already showed "Lazoo daemon unavailable" before the daemon came back; announcing its spool record again would be a duplicate. |
| Only `open` incidents | An incident the user acknowledged or resolved has been seen; a later recurrence reopens it and it is announced then. |
| A trait with the native toast as the default | It reuses `lazoo_runtime::NativeNotifier` unchanged and keeps every test free of real notifications. |
| The pass shares the timed sweep | Escalations from a recurrence, a promotion, or a closing window are all announced within 15 seconds by one place. |

## Fresh verification matrix

All commands were run from `C:\workspace\Orchestrator` on branch `feat/user-notifications` after the final change.

| Command | Exit | Evidence |
|---|---:|---|
| `cargo fmt --all -- --check` | 0 | No formatting differences. |
| `cargo clippy --workspace --all-targets --locked -- -D warnings` | 0 | No lint fixes were needed. |
| `cargo test --workspace --locked` | 0 | One invocation, 351 passed, 0 failed, 11 ignored (explicit live-provider tests). New: `lazoo-store` 1 (claim once, keeper settlement, never rewritten) and 1 extended (upgraded incidents start unannounced), `lazood` 2 (the recording-notifier scenario, the acceptance-script contract; 91 daemon tests), `lazoo-protocol` 1 extended. |
| `cargo build --workspace --release --locked` | 0 | Locked release build. |
| `scripts\acceptance\user-notifications.ps1` | 0 | PASS schema-13 manifest, `deterministic-fake-transport`, 6 commands, every evidence flag true. |
| `incident-thresholds`, `lead-remediation`, `idle-suspension`, `activation-policy`, `coalesced-notices`, `peer-messaging`, `result-delivery`, `minimum-incidents`, `collaboration-tools`, `approval-gate`, `automatic-task-dispatch` | 0 | PASS schema-13 manifests. |
| `first-slice`, `employee-roster`, `collaboration-bus`, `conversation-delivery`, `task-delegation-graph`, each with `-SkipBuild` | 0 | PASS manifests against the real release binaries, with the announcement pass running in the real daemon. |
| `scripts\acceptance\native-session-ownership.ps1 -SkipBuild -Provider codex` | 0 | PASS manifest, rewritten by this run; real Codex CLI, non-model session contract only. (A first attempt in the batch lost its log to a PowerShell variable-name clash and was not counted.) |
| `git diff --check` | 0 | No whitespace errors. |

Not run, as in every prior backend slice: `provider-adapters.ps1 -Provider codex` (live model turn) and `desktop-shell.ps1` (unchanged, separately owned UI). `npm --prefix apps\lazoo-desktop\ui run check` was not run because `node_modules` is absent; the frontend is unchanged.

## User-notifications acceptance evidence

`artifacts/acceptance/user-notifications-manifest.json` records literal `PASS`, schema 13, and `deterministic-fake-transport`. Its focused commands prove:

- `userNotifiedAt` and `userNotification` round-trip, and records written before schema 13 parse with neither;
- only open user escalations are claimed, each exactly once and oldest first, with the claim time recorded and the revision bumped; a keeper record is settled as `keeper`; normal and acknowledged incidents are not announced; and a settled outcome (or a keeper record) is never rewritten;
- a lone stall is announced with its own title and next action within 240 bytes and shows `shown` through `incident.show`; a lead window is not announced until the sweep escalates it, and then shares one "2 incidents need you" notification with a second unexpected exit; a further pass announces nothing; a refused notification is recorded `failed` and a later pass does not retry it; and no task changed;
- an incident moved to the user by the schema-12 upgrade starts unannounced; and
- schema is 13 and the desktop tree is unchanged.

## Boundary evidence

- `git diff --check` passed; both diffs under `apps/lazoo-desktop` are empty.
- No Tauri command, capability, `desktop.snapshot` field, frontend source, tool, CLI command, or RPC method changed; `IncidentRecord` gains two serde-defaulted fields.
- A notification never acknowledges, resolves, retries, restarts, or changes work.
- `UI BACKEND CONTRACT: GO` remains intentionally withheld.

## Retained follow-up boundaries

Notification actions and quiet hours, per-project notification policy, typed desktop Inbox binding, configurable thresholds, a message priority field, the message-arrival notification, the live Codex gate, and everything else NEXT-SESSION lists remain separate reviewed slices.
