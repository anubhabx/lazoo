# Minimum incidents verification

Date completed: 2026-09-23 IST

Branch: `feat/minimum-incidents`

Base: `feat/collaboration-tools` at `d72de10` (`feat: expose Codex collaboration tools`), itself based on `main` at `81773b8`

Product version: `0.1.0`

Database schema: 9

Scope: domain taxonomy, protocol contracts, schema-9 store ledger, daemon raise sites and RPCs, two Codex collaboration tools, CLI, acceptance harness, and continuity documentation. Desktop product code and contract are unchanged.

## Outcome

PASS. The focused verification below passed with commit `762aec2`; the full workspace gate passed on 2026-09-24 after three follow-up fixes (see "Full gate re-run").

Every committed operational failure now becomes one human-readable incident. The daemon raises an incident immediately after it commits a session start failure, a provider rejection, stall, timeout, exit, or missing completion evidence, a dead letter, a dispatch failure before provider I/O, or restart owner loss, and folds keeper emergency spool records in at startup. Equivalent failures (same code, employee, project) share one row whose occurrence counters advance; three inside five minutes promote it to `priority` with an `incident.escalated` event. Each incident carries a plain-language title, explanation, and next action, the linked scope ids, and bounded detail holding the raw failure code and provider error.

Only the local user settles incidents: `incident.acknowledge` and `incident.resolve` are revision-checked and idempotent, a new occurrence reopens an acknowledged row, and a resolved row never reopens. Codex sessions gain `incident_report` and `incident_list`, scoped to the caller's project and identity. Incidents never retry work, restart anything, re-authenticate, change authority, or delete data; the failure record they point at stays authoritative.

No live model turn was run.

## Design notes

Design and implementation landed together under the owner's standing instruction to continue the workstreams; `docs/superpowers/specs/2026-09-23-minimum-incidents-design.md` and `docs/superpowers/plans/2026-09-23-minimum-incidents.md` record the contract for owner review before merge.

| Decision | Reason |
|---|---|
| Incidents are raised in their own transaction right after the failure commit, not inside it | Keeps every existing failure path untouched; the failure row is already record-first and authoritative, and restart reconciliation raises its own incidents for the crash path. Folding into the failure transaction is a named later upgrade. |
| Fingerprint is code + employee + project | Matches the brief's "three equivalent failures" and "same employee process" thresholds and turns a restart flood into one row per employee with a count. |
| Keeper spool records fold through the same idempotent raise keyed by spool record id | Repeated boots never duplicate; the existing `emergency_incident_count` in the desktop snapshot is unchanged. |
| Employee reports fold like any other code | Repeated reports from one employee in one project update the latest title and explanation instead of flooding the ledger; the deferred lead-notification slice can revisit this. |

## Fresh verification matrix

All commands were run from `C:\workspace\Orchestrator` on branch `feat/minimum-incidents` after the final fixes unless marked otherwise.

| Command | Exit | Evidence |
|---|---:|---|
| `cargo fmt --all -- --check` | 0 | No formatting differences. |
| `cargo clippy --workspace --all-targets --locked -- -D warnings` | 0 | All workspace targets linted with warnings denied. |
| `cargo check --workspace --all-targets --locked` | 0 | All workspace targets checked (ran in the interrupted gate before the final lint-only edits; those edits were re-linted by the clippy run above). |
| `cargo test -p lazoo-domain --test incident` | 0 | 4 passed. |
| `cargo test -p lazoo-protocol --test incident` | 0 | 3 passed. |
| `cargo test -p lazoo-store --test incident` | 0 | 3 passed. |
| `cargo test -p lazood --test daemon -- incident session_start_failure provider_turn_failures dead_letters_and_dispatch restart_owner_loss` | 0 | 6 passed (five scenarios and the acceptance-script contract). |
| `cargo test -p lazooctl --test cli incident` | 0 | 2 passed. |
| `scripts\acceptance\minimum-incidents.ps1` | 0 | PASS schema-9 manifest, `deterministic-fake-transport`, 14 commands, every evidence flag true. |
| `git diff --check` | 0 | No whitespace errors. |

## Full gate re-run (2026-09-24)

The combined background gate behind commit `762aec2` was stopped by the Claude Code harness for host memory pressure, so on 2026-09-24 IST it was re-run in the foreground one piece at a time on `feat/minimum-incidents`. It surfaced three gaps that the focused verification above could not see; all three are fixed in the follow-up commit on this branch.

| Gap | Symptom | Fix |
|---|---|---|
| `crates/lazoo-store/src/migration.rs` still declared `VERSION = 8` while its table listed migration 9 | `employee_and_roster_survive_store_reopen` failed with `UnknownMigration { found: 9, supported: 8 }`: a fresh database applied schema 9 and then refused to reopen. The daemon and CLI tests never reopen a store, so they could not see it. | `VERSION` is derived from the last `MIGRATIONS` entry, so the guard can no longer drift from the table. |
| `lead_turn_lists_employees_delegates_and_waits_through_collaboration_tools` still asserted the four-tool catalog | The daemon suite failed on "every daemon-managed Codex session declares the Lazoo tool catalog". | The assertion lists all six tools. |
| `first-slice`, `employee-roster`, `native-session-ownership`, `collaboration-bus`, and `conversation-delivery` still asserted schema 8 | Each script exited 1 with `expected '8' but found '9'`. The earlier claim that every acceptance script asserted schema 9 was wrong. | The five scripts assert 9, and `first-slice` writes `databaseSchemaVersion = 9`. |

Fresh results after those fixes, all from `C:\workspace\Orchestrator` on `feat/minimum-incidents`:

| Command | Exit | Evidence |
|---|---:|---|
| `cargo fmt --all -- --check` | 0 | No formatting differences. |
| `cargo clippy --workspace --all-targets --locked -- -D warnings` | 0 | All workspace targets linted with warnings denied. |
| `cargo test --workspace --locked` | 0 | Entire Rust workspace passed: 66 daemon integration tests, 20 CLI tests, 16 Codex adapter tests, every store suite including the reopen test, doc-tests. Explicit live-provider tests remained ignored. |
| `cargo build --workspace --release --locked` | 0 | Locked release build; the binaries postdate the store fix. |
| `npm --prefix apps\lazoo-desktop\ui run check` | 0 | Prettier, TypeScript, 47 Vitest tests in 5 files, and the Vite production build passed on the unchanged frontend. |
| `scripts\acceptance\minimum-incidents.ps1` | 0 | PASS schema-9 manifest, re-run after the store fix. |
| `scripts\acceptance\collaboration-tools.ps1` | 0 | PASS schema-9 manifest. |
| `scripts\acceptance\approval-gate.ps1` | 0 | PASS schema-9 manifest. |
| `scripts\acceptance\first-slice.ps1 -SkipBuild` | 0 | PASS manifest; database schema 9. |
| `scripts\acceptance\employee-roster.ps1 -SkipBuild` | 0 | PASS manifest. |
| `scripts\acceptance\native-session-ownership.ps1 -SkipBuild -Provider codex` | 0 | PASS manifest; real Codex CLI accepting the six declared tools, non-model session contract only. |
| `scripts\acceptance\collaboration-bus.ps1 -SkipBuild` | 0 | PASS manifest. |
| `scripts\acceptance\conversation-delivery.ps1 -SkipBuild` | 0 | PASS deterministic manifest. |
| `scripts\acceptance\task-delegation-graph.ps1 -SkipBuild` | 0 | PASS manifest. |
| `scripts\acceptance\automatic-task-dispatch.ps1` | 0 | PASS schema-9 fake-transport manifest. |
| `git diff --check` | 0 | No whitespace errors. |

The embedded `task_graph_acceptance_executes_backend_only_contract` daemon test and the `status_reports_protocol_store_and_boot_identity` test assert schema 9 and are part of the workspace test run above.

## Minimum-incidents acceptance evidence

`artifacts/acceptance/minimum-incidents-manifest.json` records literal `PASS`, schema 9, and `deterministic-fake-transport`. Its focused commands prove:

- every brief code plus the owner-loss family describes the situation and the next safe action, and unknown codes get a truthful generic text;
- the three-in-five-minutes window advances, restarts after the window or on clock skew, and crosses at three;
- equivalent failures fold into one row, escalate on the third occurrence, replay without writes, conflict on a reused key with a different code, and list by state, priority, code, and employee;
- acknowledge and resolve are revision-checked and idempotent, a recurrence reopens an acknowledged row, and a resolved row never reopens;
- keeper spool records fold once per record and never twice across boots;
- two failed warms fold into one `LZ-SESSION-START-FAILED` incident with the employee, provider, and session, while the desktop snapshot keys stay unchanged;
- three provider timeouts fold into one `LZ-MESSAGE-TIMEOUT` incident that escalates to `priority` and then settles through `incident.acknowledge` and `incident.resolve` with stale, state, and not-found errors;
- a non-retryable `message.fail` raises `LZ-MESSAGE-DEADLETTER` with the message and recipient, and a drifted worker's dispatch raises `LZ-CONFIG-INCOMPATIBLE` with the task and message;
- two foreign-boot sessions and one spool record become three startup incidents, and a second boot raises nothing new;
- `incident_report` and `incident_list` act as the caller with bounded text and scope, and a report without a title is refused; and
- CLI mapping, exit code 15, and schema 9.

## Boundary evidence

- `git diff --check` passed.
- Both staged and unstaged diffs under `apps/lazoo-desktop` are empty.
- No Tauri command, capability, `desktop.snapshot` field, or frontend source changed; the snapshot keeps only its existing emergency-record count.
- No incident path retries, restarts, re-authenticates, changes authority, or deletes anything.
- No push or pull request was created.
- `UI BACKEND CONTRACT: GO` remains intentionally withheld.

## Retained follow-up boundaries

Lead notification of promoted incidents and bounded remediation (needs activation controls), user notification channels, per-code thresholds beyond the generic rule, raising inside the failure transaction, typed desktop Inbox binding, peer-message tools, other-provider incident sources, ConPTY, per-capability policy, managed memory, scheduling, and resource governance remain separate reviewed slices.
