# Configurable Thresholds Design

**Date:** 2026-10-11

**Status:** Implemented on `feat/configurable-thresholds` (stacked on `feat/message-priority`, pull request #7) with deterministic acceptance, for owner review in a pull request before merge.

**Scope:** Backend-only. Schema 15 adds `incident_thresholds`, the local user's per-code overrides of section 14.2's persistent-failure thresholds, set and reset through three daemon RPC methods and `lazooctl incident threshold`. No desktop app, Tauri, or collaboration-tool change; no live model use.

## 1. Outcome

Section 14.2 says the persistent-failure thresholds "are configurable but begin with" its defaults. They were compiled in: a user who found stall promotions noisy on a slow model, or wanted timeouts promoted sooner, had to rebuild Lazoo.

The user can now:

1. `lazooctl incident threshold list`: every documented code with its effective threshold (`occurrences` inside `windowSeconds`), whether it is the `default` or `configured`, and when it was set.
2. `lazooctl incident threshold set <CODE> --occurrences N [--window-seconds S]`: override one code (1 to 100 occurrences, 60 to 86400 seconds, five minutes by default).
3. `lazooctl incident threshold reset <CODE>`: return it to the default.

## 2. Rule

`lazoo_domain::incident_threshold(code, facts, configured)` returns the effective `IncidentThreshold { occurrences, window_seconds }`:

- Two rules are about what was lost, not how often, and promote at once whatever is configured: an `LZ-PROCESS-EXIT` that names assigned work (an employee death under its task) and an `LZ-MESSAGE-DEADLETTER` of a `priority` message. A configured window still applies to them.
- Otherwise a configured override replaces the code's own default, including the first-occurrence defaults of `LZ-SESSION-STALLED` and `LZ-DAEMON-UNAVAILABLE` and the second-exit default of an unqualified `LZ-PROCESS-EXIT`.
- With no override, section 14.2's defaults apply unchanged: one stall or daemon crash, two unexpected exits, and three equivalent failures inside five minutes for everything else.

`raise_incident` reads the override inside its own transaction, so a change applies from the next occurrence. The window that counts occurrences uses the effective `window_seconds`. An incident already promoted stays promoted; lowering or raising a threshold never demotes, resolves, or re-escalates anything.

## 3. Durable records

Migration `0015_incident_thresholds.sql`: `incident_thresholds(code PRIMARY KEY, occurrences CHECK 1..100, window_seconds CHECK 60..86400, updated_at)`. `set` upserts only when a value changes, emitting `incident.threshold-set`; `reset` deletes the row and emits `incident.threshold-reset`; writing the value a code already has, or resetting a default, changes nothing and emits no event. Both are naturally idempotent, so they take no idempotency key or expected revision. `IncidentThresholdRecord` carries `code`, `occurrences`, `windowSeconds`, `source` (`default` or `configured`), and `updatedAt`; `list` returns every code Lazoo describes in its own words (`DESCRIBED_INCIDENT_CODES`) plus every configured one, sorted by code, with the threshold an ordinary occurrence is judged against.

## 4. Rules kept

- Only the local user sets thresholds, through the daemon and CLI. No collaboration tool can read or set them.
- A threshold changes only when an incident is promoted; escalation, remediation windows, notification, and acknowledgement are unchanged.
- Codes are validated like incident codes (`LZ-[A-Z0-9-]+`, at most 64 bytes); an override for a code not yet raised is kept and listed.

## 5. Deterministic acceptance

`scripts/acceptance/configurable-thresholds.ps1` runs protocol, domain, store, CLI, and daemon proofs and writes `artifacts/acceptance/configurable-thresholds-manifest.json`. No provider smoke or live model turn.

## 6. Explicitly deferred work

- per-project or per-employee thresholds;
- a desktop settings surface (it needs the separately owned UI);
- the live Codex gate, typed desktop Inbox binding, and everything else NEXT-SESSION lists.

## 7. Review gate

Merge authorizes only the schema-15 table, the three RPC methods, `lazooctl incident threshold`, and the override rule above; it does not authorize tool access to thresholds or desktop changes.
