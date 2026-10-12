# Configurable thresholds verification

Date completed: 2026-10-11 IST

Branch: `feat/configurable-thresholds`

Base: `feat/message-priority` at `befad6f` (pull request #7, open, stacked on #6, #5, #4, #3, and #2)

Product version: `0.1.0`

Database schema: 15 (`0015_incident_thresholds.sql` adds `incident_thresholds`)

Scope: the threshold protocol types, the domain threshold rule and validation, the schema-15 table, the store's override inside `raise_incident` with list, set, and reset, the daemon's three RPC methods, `lazooctl incident threshold`, acceptance harness, and continuity documentation. The desktop app, Tauri surface, and collaboration tools are unchanged.

## Outcome

PASS.

Section 14.2's persistent-failure thresholds are now the local user's to tune. `lazooctl incident threshold set <CODE> --occurrences N [--window-seconds S]` (`incident.threshold.set`) overrides one code with 1 to 100 occurrences inside 60 to 86400 seconds, five minutes by default; `reset` returns it to the default; `list` shows every code Lazoo describes in its own words plus every configured code, with the threshold an ordinary occurrence is judged against, its `source` (`default` or `configured`), and when it was set.

`raise_incident` reads the override inside its own transaction, so a change applies from the next occurrence, and the window that counts occurrences uses the effective `window_seconds`. An override replaces the code's own default, including the first-occurrence defaults for stalls and daemon crashes. Two failures are about what was lost, not how often, and still promote at once whatever is configured: an employee process that dies while it holds assigned work, and a dead-lettered priority message. A change never demotes, resolves, or re-escalates an incident. Writing the value a code already has, or resetting a default, changes nothing and emits no event. No collaboration tool can read or set a threshold.

No live model turn was run.

## Design notes

| Decision | Reason |
|---|---|
| An override replaces counts, but losses stay at one | A user tuning noisy stalls or timeouts should not be able to silently delay the report of lost work or a lost priority notice; the configured window still applies to them. |
| One row per code, no project or employee scope | Section 14.2 describes per-failure-kind thresholds; narrower scopes need a settings model the desktop does not have yet. |
| No idempotency key or revision | Setting a value and resetting are naturally idempotent for a single local user; an unchanged write records nothing. |
| `list` shows described codes plus configured ones | The user sees the defaults they might change without Lazoo inventing a registry of every code a provider can produce; an override for a code not yet raised is kept and listed. |
| `IncidentThreshold::crossed` replaces `incident_threshold_crossed` | The rule now returns a window as well as a count, so the store asks one value both questions. |
| `#[allow(clippy::too_many_lines)]` on the two story tests | Each is one user story read top to bottom, as the repository's other story tests are. |

## Fresh verification matrix

All commands were run from `C:\workspace\Orchestrator` on branch `feat/configurable-thresholds` after the final change.

| Command | Exit | Evidence |
|---|---:|---|
| `cargo fmt --all -- --check` | 0 | No formatting differences. |
| `cargo clippy --workspace --all-targets --locked -- -D warnings` | 0 | Clean with the two story-test allowances. |
| `cargo test --workspace --locked` | 0 | One invocation, 365 passed, 0 failed, 11 ignored (explicit live-provider tests). New: `lazoo-protocol` 1, `lazoo-domain` 1, `lazoo-store` 1, `lazooctl` 1, `lazood` 2 (the user's threshold session through the daemon and the acceptance-script contract; 98 daemon tests); the domain threshold-window and section 14.2 tests moved to the new rule. |
| `cargo build --workspace --release --locked` | 0 | Locked release build. |
| `scripts\acceptance\configurable-thresholds.ps1` | 0 | PASS schema-15 manifest, `deterministic-fake-transport`, 7 commands, every evidence flag true. |
| `message-priority`, `message-arrival`, `user-notifications`, `incident-thresholds`, `lead-remediation`, `idle-suspension`, `activation-policy`, `coalesced-notices`, `peer-messaging`, `result-delivery`, `minimum-incidents`, `collaboration-tools`, `approval-gate`, `automatic-task-dispatch` | 0 | PASS schema-15 manifests. |
| `first-slice`, `employee-roster`, `collaboration-bus`, `conversation-delivery`, `task-delegation-graph`, each with `-SkipBuild` | 0 | PASS manifests against the real release binaries, with the schema-15 migration in the real daemon. |
| `scripts\acceptance\native-session-ownership.ps1 -SkipBuild -Provider codex` | 0 | PASS manifest on a rerun; real Codex CLI, non-model session contract only. The first run in the batch, right after `task-delegation-graph` with under 4 GB of memory free, exited 1 with "daemon-loss native tree process 30576 remained alive after 10 seconds" (process teardown outlived the script's ten-second wait); run on its own it passed. No code in this slice touches session or process ownership. The same script failed once after `task-delegation-graph` in the message-priority gate (a different message, the provider probe reporting `offline`) and in the incident-thresholds gate. |
| `git diff --check` | 0 | No whitespace errors. |

Not run, as in every prior backend slice: `provider-adapters.ps1 -Provider codex` (live model turn) and `desktop-shell.ps1` (unchanged, separately owned UI). `npm --prefix apps\lazoo-desktop\ui run check` was not run because the frontend is unchanged.

## Configurable-thresholds acceptance evidence

`artifacts/acceptance/configurable-thresholds-manifest.json` records literal `PASS`, schema 15, and `deterministic-fake-transport`. Its focused commands prove:

- the set and reset parameters are strict, the window defaults to 300 seconds, and a record serializes with camel-case keys and a kebab-case source;
- an override replaces a code's default (including stalls, daemon crashes, and exits) and may lower it, a death under assigned work and a dead-lettered priority message still promote at once with the configured window, a longer window keeps counting, the ranges are enforced, and every listed code has a description of its own;
- in the store, an override applies from the next occurrence, a ten-minute window promotes two conflicts 400 seconds apart, a lowered timeout threshold promotes at once, an undescribed code's override is kept and listed, a reset restores the default without demoting a promoted incident, unchanged writes and default resets emit no event, and out-of-range values and malformed codes are refused;
- `lazooctl incident threshold list|set|reset` maps to the three methods and requires `--occurrences`;
- through the daemon, the user lists, sets, applies, and resets a stall threshold, a death under assigned work is promoted at once while exits are configured to five, `list` refuses parameters, an invalid set is `LZ-INCIDENT-INVALID`, the events are recorded in order, and no tool in a session's catalog names a threshold; and
- schema is 15 and the desktop tree is unchanged.

## Boundary evidence

- `git diff --check` passed; both diffs under `apps/lazoo-desktop` are empty.
- No Tauri command, capability, `desktop.snapshot` field, frontend source, or collaboration tool changed; the daemon gains `incident.threshold.list`, `incident.threshold.set`, and `incident.threshold.reset`, and `lazooctl incident` gains `threshold`.
- A threshold change never demotes, resolves, retries, or re-escalates anything.
- `UI BACKEND CONTRACT: GO` remains intentionally withheld.

## Retained follow-up boundaries

Per-project or per-employee thresholds, a desktop settings surface, priority-ordered delivery, the live Codex gate, typed desktop Inbox binding, and everything else NEXT-SESSION lists remain separate reviewed slices.
