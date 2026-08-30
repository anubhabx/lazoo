# Lazoo Durable Employee Roster Design

**Status:** Approved by the owner-reviewed project brief and the 2026-08-30 backend-first direction

**Scope:** Backend-only employee configuration, provider observations, and revisioned live roster. No desktop or frontend changes.

## Outcome

Lazoo gains stable named employee records and a daemon-owned roster that can answer which employees are disabled, offline, unverified, or dormant without starting a model turn. Configuration changes are durable, idempotent, revision-checked, and atomically paired with audit and outbox records.

This slice does not own native employee processes, Job Objects, ConPTY, conversations, tasks, worktree creation, approvals, peer messaging, or resource enforcement. A dormant employee is configured and route-eligible in principle; it is not running and has no native session.

## Domain contract

An employee record contains:

- stable UUID and case-insensitively unique display name;
- provider binding using one of the five registered provider identifiers;
- role and operating instructions;
- optional model and reasoning effort;
- a bounded provider-native settings object that cannot contain secret-like keys;
- enabled state;
- authority mode: `ask`, `self-decision`, or `full-autonomy`;
- checkout policy: `isolated` or `read-only`;
- configuration revision and creation/update/disable timestamps.

Defaults are `ask`, `isolated`, and enabled. Disabling preserves identity and history. Physical deletion is outside this slice.

Names are trimmed, 1-80 Unicode scalar values, and unique under SQLite `NOCASE`. Role is at most 120 characters; instructions are at most 32 KiB; model and effort are each at most 256 bytes. Native settings must be a JSON object no larger than 32 KiB after serialization. Keys containing `secret`, `token`, `password`, `credential`, `authorization`, or `api-key` after punctuation-insensitive lowercase normalization are rejected recursively.

## Persistence and revisions

Migration 2 adds:

- `employees`;
- `provider_observations`;
- singleton `roster_state`.

Every effective employee create, update, or disable operation increments both the employee configuration revision where applicable and the global roster revision. It writes the domain row, immutable audit event, transactional outbox row, and processed-command response in one immediate SQLite transaction.

Update and disable commands require `expectedRevision`. A mismatch fails with `LZ-EMPLOYEE-STALE` and includes only the current revision. Reusing an idempotency key returns the original response. An update that produces no effective configuration change records the processed response but emits no event and does not increment either revision.

The global roster revision changes only when effective roster data changes. Refresh timestamps may advance without changing the revision.

## Provider observations and availability

Provider probing is explicit and non-model. Employee create and provider-changing update perform one bounded probe before persistence. `roster.refresh` probes each distinct enabled provider once and stores only safe normalized evidence:

- verification;
- version and protocol;
- capability bitmap;
- authentication state;
- degradation reason;
- checked time.

Executable paths, raw payloads, credentials, environment values, and provider evidence strings are not persisted in roster state.

Availability is derived deterministically:

| State | Rule |
|---|---|
| `disabled` | Employee is disabled |
| `offline` | No observation; provider missing/degraded; authentication blocked or needs login; or session capability is absent |
| `unverified` | Provider is only detected/unverified, or authentication readiness remains unknown |
| `dormant` | Provider is configured/verified, session capability is present, and at least one supported authentication channel is ready |

Probe failures do not erase an employee. They become a safe offline observation with a stable public reason. `employee.list` and `roster.get` never start a process. `roster.refresh` may launch only the adapter's bounded non-model probe and never calls provider smoke or starts a model session.

## RPC and CLI

Version-1 JSON-RPC adds:

- `employee.create`
- `employee.list`
- `employee.show`
- `employee.update`
- `employee.disable`
- `roster.get`
- `roster.refresh`

The CLI mirrors them under `lazooctl employee ...` and `lazooctl roster ...`. Mutations accept or generate idempotency keys. Update and disable require an expected configuration revision.

Stable errors include:

- `LZ-EMPLOYEE-INVALID`
- `LZ-EMPLOYEE-NOT-FOUND`
- `LZ-EMPLOYEE-CONFLICT`
- `LZ-EMPLOYEE-STALE`

These map to CLI exit code 9. Existing provider errors remain exit code 8.

## Security and truthfulness

- Provider identifiers are validated through the provider registry contract.
- Provider-native settings are configuration metadata, never a credential channel.
- Audit payloads and roster responses contain no executable paths or raw probe evidence.
- Full autonomy is stored configuration only; this slice does not grant execution authority.
- `desktop.snapshot` remains byte-for-byte compatible at the protocol field level and receives no employee or provider fields.
- No UI capability or Tauri permission changes are allowed.

## Testing and acceptance

Test-first gates cover:

1. strict JSON contracts, defaults, unknown-field rejection, and secret-key validation;
2. migration 1-to-2 upgrade and checksum protection;
3. atomic create/idempotency/name conflict;
4. update, disable, stale revision, no-op revision stability, and restart durability;
5. deterministic availability for every provider/authentication state;
6. one probe per distinct enabled provider and no probe for disabled employees;
7. daemon/CLI RPC mapping and stable errors;
8. full workspace tests, formatting, Clippy, check, locked release build, legacy first-slice acceptance, and a new employee-roster acceptance run.

The acceptance run uses an isolated data root and a fake/missing provider environment. It proves an offline employee remains durable across daemon restart, stale updates are rejected, disable preserves history, roster revisions are monotonic, and no model smoke is invoked.

