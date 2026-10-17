# OpenCode governed delivery verification

Date completed: 2026-10-17 IST

Branch: `feat/opencode-governed-delivery`

Base: `feat/core-v1-completion` at `1072025`

Product version: `0.1.0`

Database schema: 19 (no migration)

Scope: the OpenCode adapter launch (a per-mode inline `OPENCODE_PERMISSION` block), the ACP permission bridge (two more admitted kinds and a recorded refusal reason), the daemon rule that lets OpenCode receive turns and tasks under every authority, a guard that keeps a prompt-free session from serving a narrower turn, the deterministic stories and acceptance harness, and the live script the integrator runs. No protocol, store, CLI, or desktop change. One short live observation of `opencode acp` (two prompts on the free model, disposable directories) establishes the permission kinds; the daemon-level live ask proof is the integrator's, after merging. No support is claimed.

## Outcome

PASS (deterministic). Live daemon proof NOT RUN.

OpenCode employees now receive user turns and dispatched tasks under `ask`, `self-decision`, and `full-autonomy`. The adapter launches `opencode acp` with a block chosen by the session's permission mode. Under `ask` and `self-decision` every governed tool asks, so each command or file change arrives as `session/request_permission`; the bridge turns `execute`, `edit`, `delete`, and `move` into a pending Inbox approval (or a policy answer under `self-decision`) and answers every other kind with a cancel, so a network, connector, or unknown request never runs and never becomes an approval. Under `full-autonomy` nothing changes (`{"*":"allow"}`).

## Vendor facts

### Context7 (checked 2026-10-17)

Library `/anomalyco/opencode`, `packages/web/src/content/docs/agents.mdx`:

> `"ask"` — Prompt for approval before running the tool
> `"allow"` — Allow all operations without approval
> `"deny"` — Disable the tool

The permission keys table in the same source: `read` gates `read`; `edit` gates `write`, `edit`, `apply_patch`; `glob`, `grep`, `list`, `bash`, `task`, `external_directory`, `webfetch`, `websearch`, `lsp`, `skill`, `question` gate the tool of that name; `doom_loop` is a stuck-agent recovery prompt. It adds:

> Permission keys are matched as wildcard patterns against the underlying tool name, so the same syntax works for built-ins, custom tools, and MCP tools — for example `"mymcp_*": "deny"` denies every tool from an MCP server, and `"mymcp_search": "ask"` targets a single one.

`packages/web/src/content/docs/permissions.mdx`:

> Rules are evaluated sequentially with the last matching rule taking precedence, typically placing catch-all rules first.

and, from the earlier full-autonomy record (`/websites/opencode_ai`, CLI environment variables): `OPENCODE_PERMISSION | string | Inlined json permissions config`. The value is the `permission` object itself, without the `"permission"` wrapper that appears in `opencode.json`. Context7 does not list `todowrite` as a permission key.

### Live observation (OpenCode 1.19.0, two prompts)

Run through `opencode acp` with `OPENCODE_PERMISSION={"*":"ask"}`, a disposable `XDG_CONFIG_HOME`, `XDG_DATA_HOME`, `XDG_CACHE_HOME`, and `XDG_STATE_HOME`, working directory `C:\Users\anubhab\AppData\Local\Temp\claude\C--workspace-Orchestrator\da5552e4-4a05-4b3b-969d-9e72610ac50e\scratchpad\opencode-ask-probe\work`, default model `opencode/big-pickle`. Prompt 1 asked for a read of a small file then `echo LAZOO_PROBE` (both allowed once; both completed). Prompt 2 asked for list, glob, grep, write, webfetch, todowrite, and `rm note.txt` (every request rejected, so nothing ran). Both prompts ended with `stopReason: end_turn`. Every `session/request_permission` received:

| Tool (title of its `tool_call` update) | ACP `toolCall.kind` | Request `title` | `rawInput` in the request |
|---|---|---|---|
| `read` (file) | `read` | `read` | `{}` |
| `read` (directory; the model used it for "list") | `read` | `read` | `{}` |
| `bash` (`echo LAZOO_PROBE`) | `execute` | `echo LAZOO_PROBE` | `{"command":"echo LAZOO_PROBE"}` |
| `bash` (`rm note.txt`) | `execute` | `rm note.txt` | `{"command":"rm note.txt"}` |
| `glob` | `search` | the pattern (`*.txt`) | `{"pattern":"*.txt"}` |
| `grep` | `search` | the pattern (`pelican`) | `{"pattern":"pelican"}` |
| `write` | `edit` | the file path | `{"filepath":..., "diff":...}` |
| `webfetch` | `fetch` | the URL | `{"url":..., "format":"markdown"}` |
| `todowrite` | `other` | `todowrite` | `{}` |

Every request carried the options `once` (`allow_once`), `always` (`allow_always`), `reject` (`reject_once`), no `_meta`, and a `locations` array. The tool name is not in the request: it is the `title` of the preceding `tool_call` update (`read`, `bash`, `glob`, `grep`, `write`, `webfetch`, `todowrite`) and, for `read`, the request title itself. No `list`, `edit`, `apply_patch`, `websearch`, `task`, or MCP tool was exercised; the model answered a directory listing with `read`.

Not established by the probe: that `"read":"allow"` (and `glob`, `grep`, `list`) suppress the request when `"*":"ask"` precedes them (the probe ran only `"*":"ask"`; Context7 documents the precedence); that `list` is a tool in 1.19.0; and how OpenCode treats the `cancelled` outcome Lazoo sends (the probe answered rejections with `reject_once`).

## The block per mode

`SessionRequest.permission_mode` selects it, exactly as before; the daemon derives the mode from the authority.

| Permission mode | Authority | `OPENCODE_PERMISSION` |
|---|---|---|
| `auto` | `full-autonomy` | `{"*":"allow"}` (unchanged) |
| `ask` | `ask` | `{"*":"ask","read":"allow","glob":"allow","grep":"allow","list":"allow"}` |
| `accept-edits` | `self-decision` | `{"*":"ask","read":"allow","glob":"allow","grep":"allow","list":"allow"}` |
| any other, no mode (the probe), near misses (`AUTO`, `ASK`, `Accept-Edits`, ` ask`, empty) | | no variable |

Why this list. `"*"` comes first because the last matching rule wins, so the four read-only, non-governed tools (PROJECT-BRIEF section 12.2: reading and searching are not governed) override the catch-all and everything else asks: `bash`, `write`, `edit`, `apply_patch`, `webfetch`, `websearch`, `task`, `skill`, MCP tools, `external_directory`, and `todowrite` (kind `other` in the probe, so it is declined; its only cost is that the model loses its todo scratchpad). A tool whose name is not in the list, new or unknown, stays under `"*":"ask"` and never auto-runs. The block is inline, never a file; Lazoo writes no OpenCode configuration.

## Bridge mapping

| ACP `toolCall.kind` | Result |
|---|---|
| `execute` | `command-execution` approval, action `execute an ACP command` (as before) |
| `edit` | `file-change` approval, action `edit ACP files` (as before) |
| `delete` | `file-change` approval, action `delete ACP files` (new) |
| `move` | `file-change` approval, action `move ACP files` (new) |
| `read`, `search`, `think`, `fetch`, `other`, unknown | refused: answered with a cancel outcome (never a selected option, so never a one-shot or session-wide grant), no approval, no `PermissionRequested` event |

Every admitted request keeps `scope_expansion: true`, and only the one-shot allow option is ever selected after the observer approves. A refusal is now recorded: the bridge emits a `Progress` event whose `message` is `ACP permission request refused: <reason>` and whose raw payload is `{"permissionRefused": {"reason", "toolKind", "toolCallId", "answer": "cancelled"}}` (the title and raw input are left out; titles can carry secrets). In the daemon the message is in the turn transcript and the raw payload goes to raw-frame capture. Under `"*":"ask"` the read-only tools should not request permission at all, so a `read` request that does arrive means an unknown tool and declining it is the fail-closed outcome.

## Decisions

| Decision | Reason |
|---|---|
| `provider_enforces_authority(OpenCode, _)` is `true` for every authority | The launch block plus the bridge enforce each mode. Claude Code stays full-autonomy only; Grok Build and Antigravity stay refused. |
| New `session_serves_authority(provider, authority, employee_authority)` at the turn and task-dispatch gates | OpenCode takes its permission block at launch and ACP cannot narrow it per turn, but a turn's effective authority can be narrower than its employee's (a user turn may ask for `ask` from a `full-autonomy` lead). Opening `ask` naively would have run such a turn on an allow-all session with no approvals, a widening. A `full-autonomy` OpenCode employee therefore serves only `full-autonomy` turns and tasks (`LZ-TASK-AUTHORITY-UNSUPPORTED` before any provider I/O); a governed session serves any authority up to its own, and the approval observer applies the turn's authority to each request. |
| `ask` and `self-decision` share one block | The daemon, not OpenCode, separates them: under `self-decision` policy answers an in-scope request and a scope expansion waits for the user. |
| The memory worker keeps excluding OpenCode, now explicitly (`provider_runs_memory_summaries`) | A summary turn is `read_only`, and the ACP adapter rejects a `read_only` turn request (`read-only turns` unsupported), so an OpenCode summary could neither be proven read-only nor run. The daemon also declines every permission request on a summary turn, but that is not the guarantee the worker needs. The configure error is unchanged. |
| Real ACP requests are always scope expansions | The bridge cannot prove confinement, so under `self-decision` the policy never answers a real OpenCode request: it waits for the user. This is conservative and unchanged; the deterministic story proves the policy path with scripted in-scope requests, and the live script records which resolution a real run produces. |
| Network and connector kinds are declined, not governed | A third approval kind would change the desktop Inbox contract, an owner decision (below). |
| `opencode_runs_turns_and_tasks_only_under_full_autonomy` renamed `opencode_delivers_turns_and_tasks_under_every_authority_its_launch_enforces` and rewritten | Its premise (ask and self-decision refused) is gone; `opencode-full-autonomy.ps1` and its contract test follow the new name. The earlier record stays as history. |
| No support claim | Brief section 21.3: a deterministic path is not a live gate. |

## What stays refused

- ACP tool kinds other than `execute`, `edit`, `delete`, `move`, under every mode, with the reason recorded.
- A prompt-free (`full-autonomy`) OpenCode session serving a narrower `ask` or `self-decision` turn or task.
- OpenCode as a memory worker.
- Grok Build and Antigravity under every authority; Claude Code under `ask` and `self-decision`.
- A turn with no task fails closed to `ask`.

## Tests

- Provider launch: `opencode_launch_makes_every_governed_tool_ask_under_ask_and_self_decision` (exact block per mode, catch-all first, allow list exactly `read`, `glob`, `grep`, `list`, no governed tool named) and `opencode_launch_is_prompt_free_only_under_full_autonomy` (now also asserts nothing under near-miss modes).
- ACP bridge: `observed_acp_admits_command_edit_delete_and_move_as_governed_requests` and `observed_acp_refuses_every_other_kind_with_a_cancel_and_a_recorded_reason` (real adapter against the fake ACP fixture, extended with `KIND_*` prompt markers), and the unit test `acp::tests::acp_delete_and_move_map_to_file_changes_and_every_other_kind_stays_refused`.
- Daemon story `opencode_ask_and_self_decision_turns_are_governed_through_the_approval_ledger`: on the fake backend, an `ask` command request is a pending Inbox approval, the turn waits, `approve` completes it with `Approve` forwarded, `decline` ends it with the declined approval recorded and `Decline` forwarded, and a `self-decision` in-scope request is answered by policy with no pending approval while a scope expansion waits; sessions start with modes `ask` and `accept-edits`. Then the real OpenCode adapter on the fake ACP fixture, through the daemon: an `execute` request is held, approved, and answered with the offered one-shot option; a declined `delete` is a `file-change` approval with action `delete ACP files` and the provider is answered with a cancel; `fetch` and `other` requests create no approval, are answered with a cancel, end the turn, and leave the refusal reason in the turn events; under `self-decision` a real-bridge request still waits for the user and `fetch` is still declined.
- Daemon story `opencode_delivers_turns_and_tasks_under_every_authority_its_launch_enforces`: workers of each authority dispatch on sessions launched `auto`, `accept-edits`, and `ask`; Grok Build and Antigravity are refused; a `full-autonomy` OpenCode lead completes a full-autonomy turn and refuses `ask` and `self-decision` turns before I/O; the memory worker refuses OpenCode.
- Acceptance contract tests for `opencode-governed-delivery.ps1` and for the shape of `opencode-governed-live.ps1`.

## Commands

All ran in `C:\workspace\Orchestrator\.claude\worktrees\s6-opencode-ask` with `CARGO_TARGET_DIR` set to that worktree's `target` directory. The workspace test, `scripts\gate.ps1`, and any release build were not run.

| Command | Exit | Evidence |
|---|---:|---|
| `cargo test -p lazood --test daemon opencode_ask_and_self_decision_turns_are_governed_through_the_approval_ledger --locked -- --exact` | 0 | 1 passed. |
| `cargo test -p lazood --test daemon opencode_delivers_turns_and_tasks_under_every_authority_its_launch_enforces --locked -- --exact` | 0 | 1 passed. |
| `cargo test -p lazood --test daemon --locked -- --skip task_graph_acceptance_executes_backend_only_contract` | 0 | 144 passed, 0 failed, 1 filtered out (the skipped release-build test). |
| `cargo test -p lazoo-providers --locked` | 0 | 86 passed, 10 ignored (explicit live tests): lib 4, antigravity 6, claude 19, codex 17, config 2, config_apply 10, contract 10, grok 5, opencode 11, preview 1, registry 1. Needs `lazoo-session-host.exe` beside the binaries, built once with `cargo build -p lazoo-session-host --locked`. |
| `cargo fmt --all -- --check` | 0 | No formatting differences. |
| `cargo clippy -p lazood -p lazoo-providers --all-targets --locked -- -D warnings` | 0 | Clean. |
| `pwsh -NoProfile -File scripts/acceptance/opencode-governed-delivery.ps1` | 0 | PASS schema-19 manifest, `deterministic-fake-transport`, 10 commands (nine tests and the desktop diff guard), every evidence flag true. |
| `pwsh -NoProfile -File scripts/acceptance/opencode-full-autonomy.ps1` | 0 | PASS (renamed test and flags). |
| `[scriptblock]::Create` over the three OpenCode acceptance scripts | 0 | All parse; `opencode-governed-live.ps1` was parsed only, not run. |

## Boundary evidence

- `git diff -- apps/lazoo-desktop` is empty; the acceptance script enforces it. No schema, protocol, store, or CLI change.
- One live probe only: `opencode acp` with two prompts on the free model in disposable directories, answered by a throwaway script that never ran a destructive command (the delete, write, fetch, and todo requests were rejected). No daemon-level live run. No owner OpenCode configuration was read or written.
- The daemon's own environment is inherited by the launched process and cannot be scrubbed through `CommandSpec`; an `OPENCODE_PERMISSION` already in it would reach OpenCode under every mode. Lazoo sets one only for the three modes above, and the live script scrubs it from the daemon environment.
- `UI BACKEND CONTRACT: GO` remains intentionally withheld.

## Integrator actions

- Add `opencode-governed-live` to `$excluded` in `scripts/gate.ps1` and to the pinned literal in `gate_runner_runs_the_whole_gate_and_excludes_only_the_live_and_ui_scripts` (this slice edits neither).
- Rebuild `target\release` (`cargo build --workspace --release --locked`), then run `scripts\acceptance\opencode-governed-live.ps1` from the main checkout (or with `-ReleaseDir`). At most four live turns: the verification smoke, `ask` approved, `ask` declined, and a second employee under `self-decision`. Expect turn 3 to pend an approval flagged as a scope expansion (the script records `user-approved-scope-expansion` or `answered-without-a-pending-approval`); this differs from the brief's "no pending approval", for the reason in Decisions.
- The live script's tool-status check reads turn events of kind `toolUpdate`; if that name differs, the `no completed tool call` assertion for the declined turn is vacuous and the approval record (`declined`, decider `user`) is the evidence.

## Retained boundaries

- Network (`fetch`) and connector (`other`, MCP) requests are declined, not governed. Governing them needs a third approval kind, which changes the desktop Inbox contract (kinds today are `command-execution` and `file-change`): an owner decision.
- `external_directory`, `task`, and `skill` ask and, being kind `other` or unobserved, are declined; a lead's use of OpenCode subagents is therefore unavailable under governed modes.
- Unproven live: that `read`, `glob`, `grep`, and `list` are allowed natively beside `"*":"ask"`; that `list` exists in 1.19.0; how OpenCode reacts to the `cancelled` outcome (the probe used `reject_once`); that a real `delete` or `move` kind is ever emitted (the probe saw `write` as `edit`).
- Real-bridge `self-decision` always waits for the user until ACP requests can carry a confinement proof.
- Grok Build stays excluded; Antigravity has no prompt-free mode.
- The live delivery gate is owed before any claim of support.
