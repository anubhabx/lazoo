# Activation Policy Design

**Date:** 2026-09-28

**Status:** Implemented on `feat/activation-policy` (based on `main` at `2c28a0e`) with deterministic acceptance, for owner review in a pull request before merge.

**Scope:** Backend-only. Schema 10 adds one column, `employees.activation_policy`, that defaults to `manual` for every existing and new employee. The daemon's notice pass may start one bounded provider session for a dormant recipient whose own policy is `notice-turns`. `lazooctl employee create` and `employee update` gain `--activation-policy`. No desktop or Tauri change, no tool-catalog change, no change to user-authored activation, no live model use.

## 1. Outcome

Before this slice a lead that had gone dormant while its children finished, incidents were promoted, or peers wrote to it kept its coalesced notice turn `recorded` and its notices `queued` until the local user ran `lazooctl session warm` or submitted a new user turn. That rule protected the user from a provider process nobody asked for, but it also meant a lead could never finish a delegation on its own overnight.

Lazoo now lets the user opt one employee at a time into bounded automatic activation, and still answers three questions without inference:

1. Who allowed this activation? The recipient's own `activationPolicy`, a revisioned employee setting the user changed through `lazooctl employee update --activation-policy notice-turns`. The default, `manual`, behaves exactly as before, for employees created before schema 10 as well.
2. What did the activation run? Exactly one recorded notice turn on the recipient's own task. The session's `session.warm` command is keyed `notice-activation:<turnId>` and its `session.stop` command `notice-activation:<turnId>:stop`, so the session ledger names the turn that started and ended it.
3. What happens when it goes wrong? A refused start fails the session and raises an incident exactly like any other start failure; the turn stays `recorded`, its notices stay `queued`, the same turn never activates again, and the user resumes the recipient exactly as under `manual`.

## 2. Policy

| `activationPolicy` | A dormant recipient with a recorded notice turn |
|---|---|
| `manual` (default) | waits for `session.warm` or a new user turn, as before |
| `notice-turns` | the daemon starts one session, delivers that turn, and stops the session |

`notice_turn_may_activate` in `lazoo-domain` is the whole rule. The policy lives on the employee row beside `authority_mode` and `checkout_policy`, is exposed on `EmployeeRecord`, `employee.create` (default `manual`), and `employee.update`, and behaves like every other employee setting: a change bumps the employee's `configurationRevision` and the roster revision, is refused while the employee has an active session, and stales any session started under the old revision.

## 3. Bounds

- One activation per recorded turn. `begin_session` is keyed `notice-activation:<turnId>`; a replay returns the earlier session without starting anything, so a turn that is still `recorded` after its activation (the start failed, or the daemon restarted mid-turn and owner loss failed the session) waits for the user. A notice read inside another turn cancels its batch turn, and the fresh turn recorded later is a new turn with its own single activation.
- The recipient's own task only. `record_notice_turn` already fixes the turn's task to one assigned to the recipient and validates its ancestry; the activated session carries no other turn because the pass is sequential and the session is stopped when this turn ends.
- Stop after the turn. `stop_session_command` runs whether the turn completed or failed, so the recipient is `suspended`, dormant again, before the pass moves on. A session the user warmed is never stopped by a notice.
- No retry. A failed start, a failed turn, or a daemon restart leaves the evidence on the session, turn, message, and incident ledgers; nothing starts a second session for that turn.
- No widening. The activated session starts exactly as `session.warm` starts one: the session working directory, the employee's authority as the permission mode, the eight Codex tools declared on `thread/start`, never `thread/resume`. The turn runs under the task's effective authority, and governed actions inside it still flow through the schema-8 approval gate.
- Wait when unsure. A recipient session that is starting or stopping (`SessionConflict`), a provider that is no longer route ready, a session owned by another boot, or a missing session handle leaves the turn waiting, unchanged from before.

## 4. Durable records

Migration `0010_activation_policy.sql`: `ALTER TABLE employees ADD COLUMN activation_policy TEXT NOT NULL DEFAULT '"manual"'`, stored as a JSON string like its sibling policy columns. `EmployeeRecord` gains `activationPolicy`; `CreateEmployeeParams` takes it with a `manual` default and `UpdateEmployeeParams` optionally.

Each activation is one `employee_sessions` row owned by this boot, one `session_commands` row under `session.warm` keyed `notice-activation:<turnId>`, one under `session.stop` keyed `notice-activation:<turnId>:stop`, and the events `session.starting`, `session.ready`, `session.stopping`, and `session.suspended`, or `session.failed` plus an `LZ-SESSION-START-FAILED` incident. The notice turn itself, its companions, claims, and acknowledgements are recorded exactly as the coalesced-notices slice left them.

## 5. Rules kept from the earlier slices

- Employee-authored `turn.submit` with `activateIfNeeded` is still refused with `LZ-TURN-INVALID`; policy activation belongs to the daemon's notice pass, keyed to a recorded notice turn, and no tool argument can request it.
- Notice recording, coalescing, companion evidence, `task_wait` and `message_send` consumption, and task projection are unchanged; a peer-only turn still never completes the recipient's task.
- A dormant recipient under `manual` is never activated by a result, an incident notice, or a peer message.
- Task dispatch keeps its own start-turn-stop session for workers; this slice does not touch it.

## 6. Fixed boundaries

- No tool can read or set the activation policy; only the local user changes it through `lazooctl employee`.
- An activation never resumes a Codex thread; every session is a fresh `thread/start` with the declared tools.
- `desktop.snapshot`, Tauri commands and capabilities, `apps/lazoo-desktop`, and the tool catalog are byte-for-byte unchanged.

## 7. Deterministic acceptance

`scripts/acceptance/activation-policy.ps1` runs protocol, domain, store, CLI, and daemon fake-backend proofs and confirms the desktop tree is unchanged. It must show: the policy serialises as kebab-case and defaults to `manual`; the domain rule activates only under `notice-turns`; a policy change bumps the employee's configuration revision, an identical change bumps nothing, and a row written without the column reads `manual`; `lazooctl employee create` defaults the flag and `employee update --activation-policy notice-turns` maps it; a dormant `notice-turns` lead is started once for a queued child result, receives it as one turn on its own task, completes the task, and is suspended again with exactly one more provider start and stop; a refused start fails the session, raises one incident, leaves the turn recorded and the notice queued, is never retried on the next pass, and the user's `session.warm` still delivers the same turn without stopping the user's session; a `manual` lead still waits for the user; a `manual` recipient of a peer message is still never activated; schema is 10.

## 8. Explicitly deferred work

- an idle timeout that suspends a user-warmed session, and any activation trigger other than a recorded notice turn (scheduled actions, dispatch of a task whose worker is dormant by policy, user notification channels);
- a per-tree or per-hour activation budget above the one-per-turn bound;
- a message-arrival notification replacing the 250 ms polls inside `task_wait` and `message_send`;
- the live Codex tool-call, peer-message, coalesced-notice, and policy-activation gate;
- cross-tree and user-to-employee messaging through the tools, bounded lead remediation, typed desktop Inbox binding, ConPTY, per-capability policy, managed memory, scheduling, and resource governance.

## 9. Review gate

The owner should review this design together with the pull request. Merge authorizes only the schema-10 column, its CLI and RPC exposure, and one bounded daemon activation per recorded notice turn for employees the user explicitly set to `notice-turns`; it does not authorize any other activation trigger, desktop work, live provider consumption, or any deferred capability above.
