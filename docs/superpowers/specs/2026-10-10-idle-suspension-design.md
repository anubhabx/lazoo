# Idle Suspension Design

**Date:** 2026-10-10

**Status:** Implemented on `feat/idle-suspension` (based on `main` at `6b7f006`, the merge of the activation-policy pull request) with deterministic acceptance, for owner review in a pull request before merge.

**Scope:** Backend-only. Schema 11 adds one column, `employees.idle_timeout_seconds`, that defaults to 0 (never) for every existing and new employee. A daemon sweep stops an idle session once its employee's timeout has passed with no turn on it. `lazooctl employee create` and `employee update` gain `--idle-timeout-seconds`. Delivery of a recorded turn no longer depends on the session that was active when the turn was recorded. No desktop or Tauri change, no tool-catalog change, no live model use.

## 1. Outcome

Before this slice a session the user warmed stayed up until the user ran `lazooctl session stop` or the daemon shut down. A lead left to finish a delegation overnight kept a provider process alive for hours between its own turns, which is exactly what the brief's lazy-activation principle (section 3.3) and the core-v1 scenario's last step ("employees return to idle and suspend after policy timeout") rule out.

Lazoo now lets the user give each employee an idle timeout, and still answers three questions without inference:

1. Who allowed the suspension? The employee's own `idleTimeoutSeconds`, a revisioned employee setting the user changed through `lazooctl employee update --idle-timeout-seconds N`. The default, 0, never suspends, for employees created before schema 11 as well.
2. Why did this session stop? Its `session.stop` command is keyed `idle-suspend:<sessionId>`, so the session ledger names the reason, beside `notice-activation:<turnId>:stop` and the user's own keys.
3. What happens to work in flight? Nothing: a session with any unfinished turn on it is never due, and the check and the stop happen in one store transaction. Work that arrives after the stop finds the employee dormant and follows the existing rules: a user turn activates it, a notice turn activates it once under `notice-turns`, and anything else waits for the user.

## 2. Rule

| `idleTimeoutSeconds` | An idle session owned by this daemon boot, quiet for that long |
|---|---|
| `0` (default) | stays up until the user stops it, as before |
| `60`-`86400` | is stopped by the daemon's idle sweep |

`idle_session_due(timeout, quietSeconds)` in `lazoo-domain` is the whole rule: never for 0, otherwise once the quiet time reaches the timeout. Quiet time runs from the later of the session's `started_at` and the last change to any turn delivered on it. `validate_idle_timeout` bounds the setting at the RPC boundary, and a `CHECK` constraint repeats the bound in the schema.

The timeout lives on the employee row beside `activation_policy`, is exposed on `EmployeeRecord`, `employee.create` (default 0), and `employee.update` (optional; absent keeps it), and behaves like every other employee setting: a change bumps the employee's `configurationRevision` and the roster revision, is refused while the employee has an active session, and stales any session started under the old revision.

## 3. Bounds

- Only idle, only this boot. A session is due only in state `idle` and owned by the current daemon boot. Starting, stopping, failed, suspended, and prior-boot sessions are never touched; startup reconciliation already fails prior-boot sessions.
- Never under a turn. A session with a `submitting` or `accepted` turn on it is not due. `begin_idle_session_stop` re-checks due-ness inside the same immediate transaction that marks the session `stopping`, so a turn that reaches the session first keeps it, and a turn submitted afterwards fails the existing idle-session guard in `begin_turn_submission` and leaves the turn recorded for the next delivery.
- A recorded turn resets the clock but does not hold the session. Recording a turn moves its `updated_at`, so a just-recorded turn makes the session quiet again for a whole timeout. A turn left `recorded` (for example a follow-up refused with `LZ-TURN-TASK-SCOPE`) does not keep a provider process up forever: delivery finds the recipient's current session (section 4), so suspending under it costs at most a fresh activation.
- One stop per session. The stop is a `session.stop` command keyed `idle-suspend:<sessionId>`; a replay returns the stopping session without a new event, and a session never becomes idle again once stopped.
- Ordinary stop path. After the store marks the session `stopping`, the daemon finishes the stop exactly as `session.stop` does: the live handle is removed and stopped with the same ten-second bound, the session becomes `suspended`, and a failed provider stop fails the session with `LZ-SESSION-STOP-FAILED` and an incident. Nothing is retried.
- No widening. The sweep never starts a session, never resumes a thread, and never touches a task, message, approval, or incident. Suspension ends the native thread: the next activation is a fresh `thread/start` with the declared tools, because the Codex adapter cannot declare tools on `thread/resume`. Opting in trades conversation continuity for no idle provider process.
- Fixed cadence. The sweep runs every 15 seconds, so a session suspends up to 15 seconds after its deadline. Shutdown aborts the sweep before stopping live sessions, so the two never race for one session.

## 4. Turns no longer stranded on a stopped session

`record_turn` names the recipient's active session on the turn when one exists. Before this slice delivery used that name even after the session stopped, so a turn recorded against a session that a user stop (or now an idle suspension) ended could only fail with `LZ-TURN-SESSION-HANDLE-MISSING`: a user turn could never be delivered, and a notice turn waited for the user even under `notice-turns`. Delivery now resolves the recipient's current idle session from the roster at delivery time, which is the named session whenever that session is still idle, and otherwise the session the user warmed since, or none (dormant). `begin_turn_submission` still binds the session that actually received the turn. Task dispatch, which already binds its own session, is unchanged.

## 5. Durable records

Migration `0011_idle_suspension.sql`: `ALTER TABLE employees ADD COLUMN idle_timeout_seconds INTEGER NOT NULL DEFAULT 0 CHECK (idle_timeout_seconds = 0 OR idle_timeout_seconds BETWEEN 60 AND 86400)`. `EmployeeRecord` gains `idleTimeoutSeconds`; `CreateEmployeeParams` takes it with a 0 default and `UpdateEmployeeParams` optionally.

Each suspension is one `session_commands` row under `session.stop` keyed `idle-suspend:<sessionId>` and the events `session.stopping` and `session.suspended` (or `session.failed` plus an `LZ-SESSION-STOP-FAILED` incident). The roster reports the employee `suspended`.

## 6. Rules kept from the earlier slices

- Employee-authored traffic still never starts a provider process on its own; only the user and a `notice-turns` notice turn activate an employee, and idle suspension only ever stops one.
- A session the user warmed is still never stopped by a notice; it may now be stopped by the employee's own idle timeout, which the user set.
- Notice recording, coalescing, `task_wait` and `message_send` consumption, the one-activation-per-turn bound, and task dispatch are unchanged.

## 7. Fixed boundaries

- No tool can read or set the idle timeout; only the local user changes it through `lazooctl employee`.
- `desktop.snapshot`, Tauri commands and capabilities, `apps/lazoo-desktop`, and the tool catalog are byte-for-byte unchanged.

## 8. Deterministic acceptance

`scripts/acceptance/idle-suspension.ps1` runs protocol, domain, store, CLI, and daemon fake-backend proofs and confirms the desktop tree is unchanged. It must show: the timeout defaults to 0 and an update without it keeps it; only 0 or 60 to 86400 seconds validate, and the due rule fires only after the whole timeout; a change bumps the configuration revision, an identical change bumps nothing, the schema refuses an out-of-range value, and a legacy row reads 0; the CLI maps the flag; a session is due only after its employee's timeout, never for another boot, and is stopped once under `idle-suspend:<sessionId>`; a quiet session is suspended with the employee reported `suspended`, and the next user turn starts a fresh session; an unfinished turn keeps its session, quiet time counts from the end of the last turn, and a timeout of 0 never suspends; a turn recorded against a since-stopped session is delivered on the current session; a `notice-turns` lead suspended for idleness is woken exactly once by its child's result and stopped again; schema is 11.

## 9. Explicitly deferred work

- a sweep that sleeps until the earliest deadline instead of a fixed 15-second cadence, and an index on `conversation_turns.recipient_session_id` if the sweep ever becomes measurable;
- idle timeouts on projects or globally, and per-provider defaults;
- a message-arrival notification replacing the 250 ms polls inside `task_wait` and `message_send`;
- the live Codex tool-call, peer-message, coalesced-notice, policy-activation, and idle-suspension gate;
- activation triggers other than a recorded notice turn, activation budgets, cross-tree and user-to-employee messaging through the tools, bounded lead remediation, typed desktop Inbox binding, ConPTY, per-capability policy, managed memory, scheduling, and resource governance.

## 10. Review gate

The owner should review this design together with the pull request. Merge authorizes only the schema-11 column, its CLI and RPC exposure, the daemon's idle sweep for employees the user gave a non-zero timeout, and delivery on the recipient's current session; it does not authorize any other activation or suspension trigger, desktop work, live provider consumption, or any deferred capability above.
