# Lazoo Native Session Ownership Design

**Status:** Approved by the owner-reviewed project brief and the 2026-08-30 backend-first direction

**Scope:** Backend-only durable employee-session ownership and creation-time Windows Job Object containment. No desktop, ConPTY, conversation, task, worktree, or model-turn work.

## Outcome

Lazoo can warm one provider-native session for an enabled employee, retain its durable identity and observed state, stop it cleanly, and reconcile it truthfully after daemon loss. Every provider session process tree is placed in a dedicated kill-on-close Windows Job Object before the native provider receives its first instruction.

This slice deliberately stops at an idle native session. It does not send prompts, consume model quota, expose terminal I/O, grant editing access, create worktrees, or imply that a session can survive daemon process death. Provider-native resumable identifiers are retained for a later conversation slice.

## Creation-time containment

The existing adapters own their protocol streams and spawn either JSON-RPC or NDJSON child processes. Replacing those transports would duplicate mature provider-specific code. Assigning a normally started provider to a Job Object afterward would also leave a race in which early descendants could escape the per-session job.

The narrow solution is a new `lazoo-session-host.exe`:

1. Lazoo creates an unnamed kill-on-close Job Object and a random local launch-gate event.
2. Lazoo starts the host with provider executable and argument metadata while preserving the adapter's current directory, environment overlay, and piped standard streams.
3. The host opens the gate and blocks without starting the provider.
4. Lazoo assigns the host process to the Job Object.
5. Only after assignment succeeds does Lazoo signal the gate.
6. The host starts the provider with inherited stdin/stdout/stderr and waits for its exit.

Children inherit the host's job membership. The Job Object does not permit breakaway and uses `JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE`. Forced stop terminates the complete job, not only the wrapper PID. If assignment or gate signaling fails, startup fails closed and the host is terminated.

The host is a protocol-transparent process shim: it does not read provider stdin/stdout, inspect payloads, persist credentials, or make policy decisions. Production discovery resolves it beside the currently running Lazoo executable. A missing host is a stable startup failure rather than an uncontained fallback.

## Durable session contract

A session record contains:

- stable Lazoo session UUID;
- employee UUID and employee configuration revision used at start;
- provider identifier;
- lifecycle state: `starting`, `idle`, `stopping`, `suspended`, or `failed`;
- session revision;
- owning daemon boot UUID while active;
- provider-native resumable session identifier when available;
- transient host PID while active;
- safe public failure code and message when failed;
- created, updated, started, and stopped timestamps.

Only `starting`, `idle`, and `stopping` are active states. An employee may have at most one active session. Historical suspended and failed rows remain durable. Physical deletion is outside this slice.

The daemon exposes:

- `session.warm`
- `session.list`
- `session.show`
- `session.stop`

The CLI mirrors them under `lazooctl session ...`. Warm and stop require idempotency keys. Warm also requires the employee's expected configuration revision; stop requires the session's expected revision. Stable session errors use `LZ-SESSION-*` and CLI exit code 10.

## State transitions and transactions

Warm is a two-phase operation because SQLite and native process creation cannot be one transaction:

1. `begin_session` validates the employee, expected configuration revision, enabled state, provider observation, and absence of another active session. It records `starting`, the command-to-session mapping, an audit event, an outbox row, and a roster revision in one immediate transaction.
2. The daemon creates the isolated session directory and starts the provider through the contained host.
3. Success records native session ID, host PID, `idle`, a new session revision, audit/outbox, and roster revision atomically.
4. Failure records a redacted stable reason and `failed` atomically. It never erases the session record.

The command mapping points to the durable session ID rather than a frozen response body, so replay returns the current record and cannot launch a duplicate process.

Stop first transitions an idle session to `stopping`, then asks the adapter to stop within a bounded timeout. Success records `suspended` and clears transient ownership. Timeout or stop failure terminates the Job Object and records `failed`. Replayed stop commands return the current durable record.

## Daemon loss and reconciliation

The live handle map is daemon-owned and never reconstructed from a PID. Job handles close when the daemon exits, so Windows terminates the owned tree. On the next boot, the store changes any prior-boot `starting`, `idle`, or `stopping` record to `failed` with `LZ-SESSION-OWNER-LOST`, clears the host PID, emits audit/outbox evidence, and advances the roster revision.

Lazoo never adopts an unrelated or unverifiable process after restart. A later command may use the retained provider-native identifier to implement explicit resume, but resume is not part of this slice.

Planned daemon shutdown asks all live sessions to stop before the pipe server exits. The Job Object remains the final cleanup guarantee if a provider ignores graceful shutdown.

## Roster truth

Employee availability gains `starting`, `idle`, `suspended`, and `failed` states. Disabled/offline/unverified provider truth still takes precedence. Otherwise the latest session state maps directly; an employee with no session remains `dormant`.

Effective session transitions advance the global roster revision. Read-only session and roster commands never start a provider. `session.warm` is the only new command that starts a provider, and it stops before any model turn.

## Security and scope boundaries

- The launch gate name is random, short-lived, local to the current Windows session, and carries no secret or provider data.
- Provider credentials remain in provider-native stores and inherited environment; Lazoo does not log environment values.
- Session working directories live under the owner-protected Lazoo data root. They are not project worktrees and are not advertised as editing environments.
- Host PIDs are diagnostic ownership metadata, not durable process identity after stop or restart.
- `desktop.snapshot`, Tauri commands, and UI permissions remain unchanged.
- ConPTY, terminal attachment, turns, conversations, task delivery, approvals, worktrees, and resource limits remain separate later slices.

## Acceptance

Acceptance must prove:

1. the host blocks before its gate is signaled;
2. root and descendant fixture processes are in the same dedicated Job Object;
3. terminating or closing the job removes the complete tree;
4. warm is idempotent and creates only one native process tree;
5. stop is revision-checked, bounded, and preserves the durable session record;
6. forced daemon loss removes the native tree and the next boot records owner loss instead of adopting a PID;
7. roster revisions and availability reflect starting/idle/suspended/failed truth;
8. no provider prompt/smoke, desktop field, Tauri capability, worktree, or ConPTY path is introduced.
