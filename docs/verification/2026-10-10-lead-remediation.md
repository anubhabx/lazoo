# Lead remediation verification

Date completed: 2026-10-10 IST

Branch: `feat/lead-remediation`

Base: `feat/idle-suspension` at `4d2ff45` (pull request #2, open), because this slice needs schema 11; the pull request retargets to `main` once #2 merges.

Product version: `0.1.0`

Database schema: 12 (`0012_lead_remediation.sql` adds `incidents.escalation`, `remediation_deadline_at`, `escalated_at`, `escalation_reason`, and the `incident_remedies` table)

Scope: the escalation, reason, and remedy protocol types and incident fields, the domain bounds and rule, the schema-12 columns and table, promotion and recurrence escalation, the escalation sweep, atomic remedies through the shared cancellation and delegation transactions, the `incident_remedy` tool, the `incident list --escalation` CLI filter, acceptance harness, and continuity documentation. Desktop product code and contract are unchanged.

## Outcome

PASS.

An incident promoted to `priority` on a child task now opens a five-minute window for the lead of that task tree, named in the lead's `status` notice. Inside it the lead may apply at most two remedies through `incident_remedy`: `cancel-task` cancels the affected direct child as the lead, and `redelegate-task` sends a failed or cancelled child's objective out again through every delegation guard. Each remedy commits in the same transaction as its action and is kept under the incident's `remedies`; a refused remedy consumes no attempt. The daemon's 15-second sweep closes the window: an incident that has not recurred since the lead's last remedy becomes `remediated`, and anything else unresolved becomes `escalation: user` with a reason. A recurrence after the second remedy, or after a remediated window, reaches the user at once, as does any promotion with no lead to remediate it and any unresolved priority incident written before schema 12. `lazooctl incident list --escalation user` lists them. Only the user resolves an incident, and escalation starts no turn, cancels nothing, and retries nothing.

No live model turn was run.

## Design notes

Design and implementation landed together under the owner's standing instruction to continue the build; `docs/superpowers/specs/2026-10-10-lead-remediation-design.md` and `docs/superpowers/plans/2026-10-10-lead-remediation.md` record the contract for owner review in the pull request.

| Decision | Reason |
|---|---|
| Two remedies, cancel and redelegate, on a direct child of the lead's root | Both reuse existing guarded store transitions (`cancel_task_tx`, `delegate_task_tx`, split out of the commands without behavior change), so a remedy cannot do anything the user's `task cancel` or the lead's `task_delegate` could not. Turn retry and session restart were left out because both would start a provider process from employee-authored traffic. |
| The remedy row commits with its action | A refused action (window, attempts, scope, target state, or any task guard) rolls back with nothing recorded, so a stale roster revision does not burn one of the lead's two attempts. The schema repeats the bound with `CHECK (attempt BETWEEN 1 AND 2)` and `UNIQUE (incident_id, attempt)`. |
| Only `failed` or `cancelled` tasks are redelegated | The old and new task never run at once, and an `uncertain` task, which may have run, is never repeated. |
| Recurrence is counted by occurrences, not clocks | Each remedy stores `occurrences_at_remedy`; "recurred since the last remedy" is `occurrences > occurrences_at_remedy`, so a keeper-supplied `occurredAt` cannot confuse the rule. Only the window deadline uses the daemon's clock. |
| `remediation_verdict` is one pure function | Both places that judge an incident (`raise_incident` for a recurrence, `escalate_due_incidents` for a passed window) call it, and its ten-row table is tested directly. |
| Promotion uses the notice predicate | `incident_lead_root` decides both whether the lead gets a notice and whether it gets a window, so a lead is never offered a window it was not told about. |
| The sweep shares the idle-suspension ticker | `run_timed_sweeps` runs both every 15 seconds; a window closes up to 15 seconds late, the same slack the idle sweep already accepts. |
| New incident fields are serde-defaulted | Incident payloads in events written before schema 12 still parse into `IncidentRecord`. |

## Fresh verification matrix

All commands were run from `C:\workspace\Orchestrator` on branch `feat/lead-remediation` after the final change.

| Command | Exit | Evidence |
|---|---:|---|
| `cargo fmt --all -- --check` | 0 | No formatting differences. |
| `cargo clippy --workspace --all-targets --locked -- -D warnings` | 0 | All workspace targets linted with warnings denied; no lint fixes were needed. |
| `cargo test --workspace --locked` | 0 | One invocation, 344 passed, 0 failed, 11 ignored (explicit live-provider tests). New: `lazood` 3 (the notice-turn remedy scenario, the sweep scenario, the acceptance-script contract; 88 daemon tests in all), `lazoo-store` 4 (both window outcomes and the bounds, no-lead promotion, the schema-11 upgrade), `lazoo-domain` 1 plus 1 extended, `lazoo-protocol` 3 extended, `lazooctl` 1 extended. This also closes the one-invocation run the idle-suspension slice deferred for lack of disk space. |
| `cargo build --workspace --release --locked` | 0 | Locked release build. |
| `scripts\acceptance\lead-remediation.ps1` | 0 | PASS schema-12 manifest, `deterministic-fake-transport`, 12 commands, every evidence flag true. |
| `scripts\acceptance\idle-suspension.ps1` | 0 | PASS schema-12 manifest. |
| `scripts\acceptance\activation-policy.ps1` | 0 | PASS schema-12 manifest. |
| `scripts\acceptance\coalesced-notices.ps1` | 0 | PASS schema-12 manifest. |
| `scripts\acceptance\peer-messaging.ps1` | 0 | PASS schema-12 manifest. |
| `scripts\acceptance\result-delivery.ps1` | 0 | PASS schema-12 manifest; one notice per promoted incident unchanged. |
| `scripts\acceptance\minimum-incidents.ps1` | 0 | PASS schema-12 manifest; fold, threshold, acknowledge, and resolve unchanged. |
| `scripts\acceptance\collaboration-tools.ps1` | 0 | PASS schema-12 manifest. |
| `scripts\acceptance\approval-gate.ps1` | 0 | PASS schema-12 manifest. |
| `scripts\acceptance\automatic-task-dispatch.ps1` | 0 | PASS schema-12 fake-transport manifest; cancellation before and after native acceptance unchanged through the shared `cancel_dispatch_turn`. |
| `scripts\acceptance\first-slice.ps1 -SkipBuild` | 0 | PASS manifest; database schema 12. |
| `scripts\acceptance\employee-roster.ps1 -SkipBuild` | 0 | PASS manifest. |
| `scripts\acceptance\collaboration-bus.ps1 -SkipBuild` | 0 | PASS manifest. |
| `scripts\acceptance\conversation-delivery.ps1 -SkipBuild` | 0 | PASS deterministic manifest. |
| `scripts\acceptance\task-delegation-graph.ps1 -SkipBuild` | 0 | PASS manifest against the real daemon with the notice loop and timed sweeps active; `task delegate` and `task cancel` unchanged through the split transactions. |
| `scripts\acceptance\native-session-ownership.ps1 -SkipBuild -Provider codex` | 0 | PASS manifest; the real Codex CLI accepted the nine declared tools on `thread/start` (non-model session contract only). |
| `git diff --check` | 0 | No whitespace errors. |

All sixteen acceptance scripts ran in the foreground; no daemon process was left behind.

Not run, as in every prior backend slice: `provider-adapters.ps1 -Provider codex` requires a live model turn that the handoff does not authorize, and `desktop-shell.ps1` asserts against the unchanged, separately owned UI; their tracked manifests were left as committed. `npm --prefix apps\lazoo-desktop\ui run check` was not run because `node_modules` is absent on this machine; the frontend is unchanged and the `lazoo-desktop` crate's tests passed in the workspace run.

## Lead-remediation acceptance evidence

`artifacts/acceptance/lead-remediation-manifest.json` records literal `PASS`, schema 12, and `deterministic-fake-transport`. Its focused commands prove:

- the whole rule as a table: two remedies and a recurrence escalate at once, an open window waits, a closed window escalates with no remedy or a recurrence after one, and a remedy that held is `remediated`;
- the lead's notice names the window and the tool while it is open, and keeps the old advice when there is none;
- a promoted child incident opens a lead window five minutes long; refused remedies (redelegating a live child, a worker's turn acting on its lead's incident) record nothing; `cancel-task` cancels the child with the lead's endpoint as the evidence actor and replays without writing, and the same key with another kind is refused; `redelegate-task` creates a new accepted child under the lead's root with the same objective and references to the old task and the incident; a third remedy is refused; and the next recurrence goes to the user as `remedies-exhausted`, with the event sequence `raised, recurred, escalated, remedy-applied, remedy-applied, escalated-to-user`;
- the sweep does nothing while windows are open, then escalates an unremedied window as `no-remedy` and marks a window whose remedy held `remediated`, the closed windows refuse further remedies, and a remediated failure that comes back reaches the user as `recurred-after-remedy`;
- a promotion about the lead's own root task goes to the user as `no-lead` with no window, and a normal incident never escalates;
- an unresolved priority incident in a schema-11 database becomes `user` / `schema-upgrade` on upgrade while resolved and normal ones stay `none`;
- `lazooctl incident list --escalation user` maps the filter and an unknown value is refused;
- inside a real notice turn on the fake backend the lead's tool calls cancel the child, redelegate it, are refused a third time with `LZ-INCIDENT-REMEDY-CLOSED` and a malformed call with `LZ-TOOL-INVALID` without ending the turn, `incident_list` shows the window, `incident.show` returns both remedies, and the recurrence on the redelegated task reaches `incident.list --escalation user`;
- the sweep's escalation starts no provider turn or session and leaves every task as it was;
- every daemon-managed Codex session declares the nine tools; and
- schema is 12 and the desktop tree is unchanged.

## Boundary evidence

- `git diff --check` passed.
- Both staged and unstaged diffs under `apps/lazoo-desktop` are empty.
- No Tauri command, capability, `desktop.snapshot` field, or frontend source changed.
- No remedy or escalation acknowledges or resolves an incident, starts a provider process, retries anything, widens authority, approves anything, or deletes data; a remedy acts only on a direct child of the calling lead's own root task.
- This slice is submitted as a pull request rather than a local fast-forward.
- `UI BACKEND CONTRACT: GO` remains intentionally withheld.

## Retained follow-up boundaries

User notification channels beyond the incident ledger and event stream, remedies that start a provider process or reach grandchildren, the remaining section 14.2 thresholds and configurable thresholds, a message-arrival notification replacing the 250 ms polls, the live Codex gate, cross-tree and user-to-employee messaging, typed desktop Inbox binding, ConPTY, per-capability policy, managed memory, scheduling, and resource governance remain separate reviewed slices.
