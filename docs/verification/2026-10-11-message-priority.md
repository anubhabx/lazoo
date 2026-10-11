# Message priority verification

Date completed: 2026-10-11 IST

Branch: `feat/message-priority`

Base: `feat/message-arrival-notify` at `b24dde4` (pull request #6, open, stacked on #5, #4, #3, and #2)

Product version: `0.1.0`

Database schema: 14 (`0014_message_priority.sql` adds `messages.priority` and marks earlier incident notices `priority`)

Scope: the `MessagePriority` protocol field, the schema-14 column, priority on every store insert and read, the `IncidentFacts` threshold rule, the store's priority dead-letter listing and key, the daemon's dead-letter helper and pass, `lazooctl message send --priority`, acceptance harness, and continuity documentation. The desktop app, Tauri surface, and collaboration tools are unchanged.

## Outcome

PASS.

Every message envelope, message summary, and `message.send` request now carries `priority` (`normal` or `priority`, defaulting to `normal` for older records and requests). The notice that tells a lead about a promoted incident is always `priority`; the local user may send one through `message.send` or `lazooctl message send --priority`; every other message the daemon writes is `normal`, and no collaboration tool can set it.

One dead-lettered priority message is now a priority incident, as section 14.2 lists. `raise_incident` reads the message's priority and its dead letter inside its own transaction, so no caller can assert it, and `LZ-MESSAGE-DEADLETTER` for such a message is promoted by its first occurrence (born `priority`, or promoting the incident it folds into). `message.fail` raises it at once as before; `note_priority_dead_letters_once` raises it for every other priority dead letter (a failed notice or user turn, a failed dispatch, restart reconciliation), at startup and at the head of every timed sweep, so the same sweep escalates and announces it. Each dead letter is reported once, keyed `dead-letter:<message>:<attempt>`. Priority does not change delivery order.

No live model turn was run.

## Design notes

| Decision | Reason |
|---|---|
| Incident notices are the only automatic priority source | They are the one message whose loss silently breaks a safety path: a lead that never reads its notice cannot remediate, and before this slice the lost notice reached the ledger only as the notice turn's provider failure. |
| Priority read from the dead letter, not the caller | An employee-reported or mistaken raise naming a priority message that was never dead-lettered promotes nothing. |
| A sweep pass from the durable dead letter, not a hook per path | One query covers every path that dead-letters a message, including restart reconciliation, and a daemon that stops between the dead letter and the incident catches up after restart. |
| Normal dead letters from turn and task failures are unchanged | They are already reported by their own failure's incident; a second dead-letter incident would duplicate them. |
| Tools cannot set priority | Employee-authored traffic must not be able to make its own loss page the user. |
| Shared `database_at` test helper | The new upgrade test and the schema-11 upgrade test built the same old database; the helper also kept the new test under clippy's line limit. |

## Fresh verification matrix

All commands were run from `C:\workspace\Orchestrator` on branch `feat/message-priority` after the final change.

| Command | Exit | Evidence |
|---|---:|---|
| `cargo fmt --all -- --check` | 0 | No formatting differences. |
| `cargo clippy --workspace --all-targets --locked -- -D warnings` | 0 | Clean after the shared test helper. |
| `cargo test --workspace --locked` | 0 | One invocation, 359 passed, 0 failed, 11 ignored (explicit live-provider tests). New: `lazoo-protocol` 1, `lazoo-store` 2 (the dead-letter threshold and listing, the schema-13 upgrade), `lazood` 3 (the lost-notice sweep, the `message.fail` path, the acceptance-script contract; 96 daemon tests); extended: `lazoo-domain` threshold table, `lazooctl` send mapping. |
| `cargo build --workspace --release --locked` | 0 | Locked release build. |
| `scripts\acceptance\message-priority.ps1` | 0 | PASS schema-14 manifest, `deterministic-fake-transport`, 10 commands, every evidence flag true. |
| `message-arrival`, `user-notifications`, `incident-thresholds`, `lead-remediation`, `idle-suspension`, `activation-policy`, `coalesced-notices`, `peer-messaging`, `result-delivery`, `minimum-incidents`, `collaboration-tools`, `approval-gate`, `automatic-task-dispatch` | 0 | PASS schema-14 manifests. |
| `first-slice`, `employee-roster`, `collaboration-bus`, `conversation-delivery`, `task-delegation-graph`, each with `-SkipBuild` | 0 | PASS manifests against the real release binaries, with the schema-14 migration and the dead-letter pass running in the real daemon. |
| `scripts\acceptance\native-session-ownership.ps1 -SkipBuild -Provider codex` | 0 | PASS manifest on a rerun; real Codex CLI, non-model session contract only. The first run in the batch, right after `task-delegation-graph` while the host was low on memory, exited 1 with "provider route readiness expected 'dormant' but found 'offline'": the real `codex` probe reported the provider offline during roster refresh, before any schema-14 code path. The same intermittent failure after `task-delegation-graph` was recorded for the incident-thresholds slice. |
| `git diff --check` | 0 | No whitespace errors. |

Not run, as in every prior backend slice: `provider-adapters.ps1 -Provider codex` (live model turn) and `desktop-shell.ps1` (unchanged, separately owned UI). `npm --prefix apps\lazoo-desktop\ui run check` was not run because the frontend is unchanged.

## Message-priority acceptance evidence

`artifacts/acceptance/message-priority-manifest.json` records literal `PASS`, schema 14, and `deterministic-fake-transport`. Its focused commands prove:

- `priority` is kebab-case, defaults to `normal` for `message.send` requests and envelopes written before schema 14, and round-trips;
- `LZ-MESSAGE-DEADLETTER` has threshold one for a priority message and three inside five minutes otherwise, and no other code reads the message's priority;
- a priority dead letter is listed until its incident is raised, promotes the incident it folds into (no lead, so `escalation: user`), and is raised once; a normal dead letter keeps the default threshold; a raise naming a priority message that was never dead-lettered, or using another code, promotes nothing;
- incident notices written before schema 14 become priority and other messages stay normal;
- `lazooctl message send --priority` maps to `priority`, and the user's priority `message.send` dead-lettered through `message.fail` promotes at once while the sweep raises nothing twice;
- a lead's incident notice is `priority`; when its notice turn fails for good, nothing reports the dead letter until the sweep raises it as a priority incident about the lead, escalated to the user (`no-lead`) and announced by the same sweep, once, with no task changed;
- no collaboration tool source names `MessagePriority`; and
- schema is 14 and the desktop tree is unchanged.

## Boundary evidence

- `git diff --check` passed; both diffs under `apps/lazoo-desktop` are empty.
- No Tauri command, capability, `desktop.snapshot` field, frontend source, or collaboration tool changed; `MessageEnvelope`, `MessageSummary`, and `SendMessageParams` gain one serde-defaulted field each, and `lazooctl message send` gains `--priority`.
- A dead-letter incident never retries, resends, or cancels the message.
- `UI BACKEND CONTRACT: GO` remains intentionally withheld.

## Retained follow-up boundaries

Priority-ordered delivery and backpressure, message expiry, configurable thresholds, the live Codex gate, typed desktop Inbox binding, and everything else NEXT-SESSION lists remain separate reviewed slices.
