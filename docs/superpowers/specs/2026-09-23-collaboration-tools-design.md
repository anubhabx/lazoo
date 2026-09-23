# Codex Collaboration Tools Design

**Date:** 2026-09-23

**Status:** Implemented on `feat/collaboration-tools` with deterministic acceptance. The owner's standing instruction for this session was to continue the backend workstreams, so design and implementation landed together for owner review before merge.

**Scope:** Backend-only. The provider contract gains a declared-tool list and a tool-call observer hook, the Codex adapter declares Lazoo tools through the app-server dynamic-tool surface and answers `item/tool/call`, and the daemon serves four collaboration tools on behalf of the turn's recipient employee. No schema change, no desktop or Tauri change, no CLI change, no peer-message or incident or memory tools, no other-provider tool surface, and no live model use.

## 1. Outcome

Before this slice a Codex lead could be delegated *to* only through `lazooctl task delegate`; the model running inside its native session had no way to read the live roster, create a child task, wait on it, or report a structured outcome. The task graph, dispatcher, and approval gate were complete but unreachable from the employee that is supposed to drive them.

Every daemon-managed Codex session now receives four versioned tools, and a call from the model answers four questions without inference:

1. Who is on the roster right now, at which roster revision?
2. Which child task did this employee create, from which turn and session, under which effective authority?
3. What did the children it waited on actually produce, from provider evidence?
4. Which structured progress, completion, or failure did the assigned employee report for its own task?

Nothing a tool does bypasses the store: every mutation goes through the same `delegate_task` and `report_task` paths the CLI uses, with the actor fixed to the turn's recipient endpoint.

## 2. Fixed boundaries

- Only daemon-managed Codex sessions declare tools. Provider smoke, probe, and other providers keep their existing surfaces; `ProviderCapabilities` is unchanged.
- The daemon fixes the actor. A tool call can only act as the employee and session that own the turn; there is no way to name another actor, approve anything, widen authority, or bypass roster-revision, self-delegation, depth, fan-out, or active-descendant guards.
- A refused or failed call answers the model `success = false` with a stable `LZ-*` code and never ends the provider turn. Only an adapter-level observer failure (ledger unavailable) propagates and interrupts the turn, exactly as approvals do.
- The Codex `experimentalApi` capability is enabled only when tools are declared; sessions without tools keep the stable handshake. Declaring tools on `thread/resume` fails closed with `ProviderError::Unsupported`, because Codex 0.153 accepts `dynamicTools` only on `thread/start` and a resumed thread would silently lose them.
- Wire names use the Responses API alphabet `^[a-zA-Z0-9_-]+$`, so the brief's `employees.list`, `task.delegate`, `task.wait`, and `task.report` are declared as `employees_list`, `task_delegate`, `task_wait`, and `task_report`. Codex 0.153 rejects a dotted name at `thread/start`; the adapter now refuses such a declaration before spawning the thread so the failure names the tool instead of a generic session-start error.
- Tool arguments over 64 KiB are refused before they reach the control plane; results are truncated to 64 KiB before they reach the provider.
- `desktop.snapshot`, Tauri commands and capabilities, `apps/lazoo-desktop`, and schema 8 are byte-for-byte unchanged.

## 3. Wire path

```text
daemon start_new_session ── SessionRequest.tools ──▶ CodexAdapter::start
                                                     │ initialize {experimentalApi: true}
                                                     │ thread/start {dynamicTools: [...]}
                                                     ▼
Codex app-server ── item/tool/call ──▶ CodexSession::handle_tool_call
                                        │ normalize_codex_tool_call (bounded)
                                        ▼
                               TurnEventObserver::call_tool
                                        │ (daemon: App::call_collaboration_tool)
                                        ▼
                    Store::delegate_task | list/get_task | report_task | roster_snapshot
                                        ▼
Codex app-server ◀── {contentItems: [inputText], success} ◀── adapter answers, then continues
                                        │
                                        └──▶ ProviderEvent::ToolUpdate {tool_name: "lazoo:<tool>", status}
```

Waiting inside a tool call is excluded from the Codex progress and turn budgets in the same way approval waits are, so a bounded `task_wait` never trips the stall detector.

## 4. Tool catalog

| Tool | Purpose | Guards | Result |
|---|---|---|---|
| `employees_list` | Live roster with availability, authority, and `rosterRevision` | none beyond turn ownership | `{rosterRevision, employees[{id, name, role, provider, availability, availabilityReason, authorityMode, self}]}` |
| `task_delegate` | Create a child of the caller's current task for `employeeId` | parent accepted/running, actor session, roster revision, no self-delegation, authority never widens, depth 4, fan-out 8 | `{taskId, state, effectiveAuthority, revision, note}`; `proposed` under `ask`, otherwise `accepted` and dispatched when the worker is free |
| `task_wait` | Block up to `timeoutSeconds` (default 60, max 600) until listed tasks (default: unfinished children) are terminal | tasks must share the caller's root task; caller's turn must stay live | `{timedOut, tasks[{taskId, state, revision, assignedEmployeeId, objective, failureCode, failureMessage, output}]}` with `output` taken from the child's native completion evidence, bounded to 16 KiB |
| `task_report` | Report `progress`, `completed`, or `failed` for the caller's task | actor and active session match, completion requires no unfinished children | `{taskId, state, revision}`; a `failed` report without details records `LZ-TASK-REPORTED-FAILURE` |

Unknown names answer `LZ-TOOL-UNKNOWN`; malformed or oversized arguments answer `LZ-TOOL-INVALID` or `LZ-TOOL-ARGUMENTS-TOO-LARGE`; a call without an attached control plane answers `LZ-TOOL-UNATTACHED`.

## 5. Policy

Authority is unchanged policy. `task_delegate` requests at most the caller's effective authority and the assignee's configured mode; the store narrows and rejects expansion with `LZ-TASK-AUTHORITY-EXPANSION`. Under `ask` the child is `proposed` and waits for `task.accept` from the local user; no message, session, or dispatch exists until then. Governed actions inside the child's own turn still flow through the schema-8 approval gate; a tool result is never an approval.

## 6. Durable records

No new tables. A delegation writes the task row, its `accepted`/`proposed` evidence with the caller's session, the canonical delegation message, and one `task.delegated` event, exactly as the CLI path does. A report writes task evidence and the matching `task.*` event. The adapter surfaces each call as a `ToolUpdate` turn event named `lazoo:<tool>` with `completed` or `failed` status, so the turn ledger shows that a tool was called and whether it succeeded. Idempotency keys are `tool:<turnId>:<callId>`, so a re-delivered call replays without duplicate rows.

## 7. Timing

- `task_wait` clamps to `0..=600` seconds, polls every 250 ms, and returns early with `LZ-TOOL-TURN-CANCELLED` if the caller's turn is cancelled or leaves the in-flight states.
- Tool handling time is subtracted from the Codex turn budget when the adapter loop resumes.

## 8. Deterministic acceptance

`scripts/acceptance/collaboration-tools.ps1` runs focused contract, adapter-fixture, daemon fake-backend, and store schema tests and proves the desktop tree is unchanged. It must show:

- the default observer fails closed with `LZ-TOOL-UNATTACHED`;
- the Codex adapter opts into `experimentalApi` only with tools, declares `dynamicTools` on `thread/start`, answers `item/tool/call` with the Lazoo result, redacts the retained arguments, and fails closed on resume;
- every daemon-managed Codex session declares the four tools;
- a `self-decision` lead lists the roster, delegates, and waits until the automatically dispatched child completes with the worker's output;
- an `ask` lead's delegation stays `proposed`, unknown, malformed, and widening calls are refused, and the turn still completes;
- a worker's `task_report failed` is authoritative over its later native completion;
- schema remains 8 and no desktop, provider smoke, or live model use occurred.

## 9. Explicitly deferred work

- `message.send`, `message.reply`, `incident.report`, `incident.list`, `memory.search`, `memory.record`;
- a dedicated tool-call ledger with bounded arguments and results (today the turn ledger records name and status; the mutation ledgers record effects);
- delivering results to a dormant lead after its turn ended (activation controls);
- tools for ACP, Claude Code, and Antigravity sessions;
- a task-change notification replacing the `task_wait` poll;
- typed desktop binding, ConPTY, per-capability policy, managed memory, scheduling, and resource governance.

## 10. Review gate

The owner should review this design together with the branch. Merge authorizes only the backend Codex tool surface and its deterministic acceptance; it does not authorize desktop work, live provider consumption, or any deferred capability above.
