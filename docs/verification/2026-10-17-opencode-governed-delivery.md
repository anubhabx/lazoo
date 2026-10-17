# OpenCode governed delivery verification

Date completed: 2026-10-17 IST

Branch: `feat/opencode-governed-delivery`

Base: `feat/core-v1-completion` at `1072025`

Product version: `0.1.0`

Database schema: 19 (no migration)

Review follow-up (same day): the block now applies to every non-`auto` session, `read` keeps OpenCode's documented env-file defaults, a second live probe showed the inline block does NOT outrank every project configuration so governed delivery now fails closed beside one, refusals no longer count as provider progress, and the live script is hardened. The sections below describe the final state; the probe is under "Precedence probe".

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

Not established by the first probe: that `"read":"allow"` (and `glob`, `grep`, `list`) suppress the request when `"*":"ask"` precedes them (the probe ran only `"*":"ask"`; Context7 documents the precedence); that `list` is a tool in 1.19.0; and how OpenCode treats the `cancelled` outcome Lazoo sends (the probe answered rejections with `reject_once`).

### Context7 (review follow-up)

Default read rule, library `/anomalyco/opencode`, `packages/web/src/content/docs/es/permissions.mdx`:

```json
{ "permission": { "read": { "*": "allow", "*.env": "deny", "*.env.*": "deny", "*.env.example": "allow" } } }
```

described there as "allow general file access while denying sensitive .env files by default, except for .env.example". The documented default is `deny`, not `ask`, so the governed block uses this exact object (a stricter or equal result; a `read` request that still arrives is declined by the bridge). From `agents.mdx`: `read`, `edit`, `glob`, `grep`, `list`, `bash`, `task`, `external_directory`, `lsp`, and `skill` "accept either a shorthand action ... or an object of glob/pattern → action". From `permissions.mdx`: rules are matched by wildcard (`*` any characters, `?` one) and "the last matching rule winning"; the `evaluate` source returns the last matching rule and defaults to `ask` when none matches.

Configuration precedence, `config.mdx` (later overrides earlier): remote, global `~/.config/opencode/opencode.json`, `OPENCODE_CONFIG`, project `opencode.json`, `.opencode` directories, inline `OPENCODE_CONFIG_CONTENT`, then managed files. `permissions.mdx` (agent scoped example): "Agent permissions are merged with global configurations, with agent rules taking precedence". The documentation does not say where `OPENCODE_PERMISSION` sits; the probe below shows it does not beat an agent-scoped project rule.

## Precedence probe (review follow-up)

Two live prompts on `opencode/big-pickle`, disposable XDG directories, working directory `...\opencode-ask-probe\work2` containing `opencode.json` = `{"permission":{"bash":"allow"},"agent":{"build":{"permission":{"bash":"allow"}}}}` and `.opencode/agents/build.md` with frontmatter `permission: bash: allow`. Prompt: run the shell command `hostname` with the bash tool.

| Launch | `session/request_permission` (kind `execute`) | Result |
|---|---|---|
| `OPENCODE_PERMISSION` = the governed block | none (0 requests) | the tool call went `pending` then `completed`; `hostname` ran with no request |
| the same plus `OPENCODE_CONFIG_CONTENT={"permission":<the governed block>}` | none (0 requests) | `hostname` ran again with no request |

So the inline block does not outrank the project configuration for `bash`, and `OPENCODE_CONFIG_CONTENT` (documented as higher than project config) did not repair it. This corrects the earlier full-autonomy record, which said the inline block "outranks global, custom, project, and `.opencode` config". Which of the three project sources won (the top-level `permission`, the `agent.build.permission` entry, or the agent markdown) was not isolated within the prompt budget; the docs' agent-over-global rule makes the agent-scoped sources the likely cause (an inference, not proven). One malformed `OPENCODE_CONFIG_CONTENT` attempt (a shell quoting slip) hung the handshake and sent no prompt. Per the review plan Lazoo therefore does not pass `OPENCODE_CONFIG_CONTENT` and fails closed instead (see Decisions).

### Global agent-scoped probe (follow-up)

The same live setup with a disposable `XDG_CONFIG_HOME` (free model, one `hostname` prompt per row, working directory with no OpenCode configuration, `OPENCODE_PERMISSION` = the governed block):

| Global `$XDG_CONFIG_HOME/opencode/opencode.json` | `session/request_permission` (kind `execute`) | Result |
|---|---|---|
| `{"agent":{"build":{"permission":{"bash":"allow"}}}}` | none (0 requests) | `hostname` ran with no request: the inline block LOST |
| `{"permission":{"bash":"allow"}}` | arrived | the block WON: the `execute` request reached the client |

So agent-scoped permissions beat the inline block wherever they live (project or global); a top-level `permission` does not, in the global file. This is consistent with the earlier inference that the agent-scoped project sources, not the top-level `permission`, defeated the block, but the project sources were still not isolated one by one. Context7 (`/anomalyco/opencode`, `packages/util/src/global-roots.ts`) gives the global location: `config: path.join(process.env.XDG_CONFIG_HOME || path.join(os.homedir(), ".config"), app)`, quoted from the docs as "Global user settings applied across all projects on the machine. `~/.config/opencode/opencode.json(c)`"; the code has no Windows-specific branch, so on Windows the default is `%USERPROFILE%\.config\opencode` (the same path `crates/lazoo-providers/src/config.rs` already reads for discovery). The docs also list a managed system directory (`%ProgramData%\opencode` on Windows) that loads `agents/` for every user; it is not scanned (see Retained boundaries).

## The block per mode

`SessionRequest.permission_mode` selects it, exactly as before; the daemon derives the mode from the authority.

| Permission mode | Authority | `OPENCODE_PERMISSION` |
|---|---|---|
| `auto` | `full-autonomy` | `{"*":"allow"}` (unchanged) |
| `ask` | `ask` | `{"*":"ask","read":{"*":"allow","*.env":"deny","*.env.*":"deny","*.env.example":"allow"},"glob":"allow","grep":"allow","list":"allow"}` |
| `accept-edits` | `self-decision` | the same governed block |
| every other mode, including none and `untrusted` (the daemon's `provider.smoke`), and near misses (`AUTO`, `ASK`, `Accept-Edits`, ` ask`, empty) | | the same governed block |
| the version probe (an ACP handshake, no prompt) | | no variable |

Why this list. `"*"` comes first because the last matching rule wins, so the four read-only, non-governed tools (PROJECT-BRIEF section 12.2: reading and searching are not governed) override the catch-all; `read` is the object form so `.env`, `.env.*` files stay denied and `.env.example` readable, as OpenCode's own default has it and everything else asks: `bash`, `write`, `edit`, `apply_patch`, `webfetch`, `websearch`, `task`, `skill`, MCP tools, `external_directory`, and `todowrite` (kind `other` in the probe, so it is declined; its only cost is that the model loses its todo scratchpad). A tool whose name is not in the list, new or unknown, stays under `"*":"ask"` and never auto-runs. The block is inline, never a file; Lazoo writes no OpenCode configuration.

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
| Every non-`auto` session gets the governed block (review item 2) | `provider.smoke` starts OpenCode with `untrusted` and a user prompt; with no block the owner's own configuration decided. Only `auto` is allow-all and only the no-prompt version probe launches without a block. |
| `read` keeps the documented env-file defaults, as `deny` (review item 1) | A shorthand `"read":"allow"` replaces OpenCode's default object and would open `.env` files without asking. The review recalled `ask`; the documented default is `deny` (quoted above), so the block uses that, with `.env.example` allowed. |
| Governed delivery fails closed beside project configuration (review item 3) | Per the probe, a governed `OpenCodeAdapter::start` returns `ProviderError::Unsupported` (the session fails to start; a task fails at dispatch) when the working directory or an ancestor up to the repository root has an `opencode.json` or `opencode.jsonc` naming a `permission`, `agent`, `agents`, `mode`, or `tools` key (or unreadable), or any `.opencode` directory, or when `OPENCODE_CONFIG`, `OPENCODE_CONFIG_CONTENT`, or `OPENCODE_CONFIG_DIR` is set in the daemon's environment. The project scan stops before the user's home directory, whose `.opencode` is OpenCode's own installation and the owner's global configuration (it would otherwise refuse every session under the home directory). A second, global scan covers the owner's configuration roots: `$XDG_CONFIG_HOME/opencode` when `XDG_CONFIG_HOME` is set and non-empty, else `%USERPROFILE%\.config\opencode` (the same variables the adapter's home lookup uses: `USERPROFILE`, then `HOME`), plus `%USERPROFILE%\.opencode`. In those roots a governed start is refused when `opencode.json` or `opencode.jsonc` names `agent`, `agents`, or `mode`, when an `agent` or `agents` subdirectory (searched to a depth of eight) contains any `*.md`, or when a file or directory there cannot be read (not UTF-8, over 1 MiB, or any I/O error other than not-found). A global top-level `permission` (or legacy `tools`) alone does NOT refuse, because the probe showed the block wins there. The refusal's `detail` (a new optional field on `ProviderError::Unsupported`, shown in its message) names the triggering file, directory, or variable, never its contents. `auto` is exempt: it is allow-all and has no governed rules to bypass. `provider.smoke` runs in the daemon's current directory, so it is subject to the same refusal. |
| Refusals are not progress (review item 4) | A refusal `Progress` event is observed and recorded but does not reset the progress deadline, so a model that keeps asking for refused tools still stalls. Reused JSON-RPC ids, reused `toolCallId`s, an id that is neither string nor number, and malformed requests are now recorded with the same `permissionRefused` payload. |

## What stays refused

- ACP tool kinds other than `execute`, `edit`, `delete`, `move`, under every mode, with the reason recorded.
- A prompt-free (`full-autonomy`) OpenCode session serving a narrower `ask` or `self-decision` turn or task.
- OpenCode as a memory worker.
- Grok Build and Antigravity under every authority; Claude Code under `ask` and `self-decision`.
- A turn with no task fails closed to `ask`.

## Tests

- Provider launch: `opencode_launch_makes_every_governed_tool_ask_under_ask_and_self_decision` (exact block per mode, catch-all first, `read` the exact object form, bare allows exactly `glob`, `grep`, `list`, no governed tool named) and `opencode_launch_is_prompt_free_only_under_full_autonomy` (asserts allow-all only for `auto` and the governed block for every other mode, including none, `untrusted`, and near misses); `opencode_launches_acp_for_the_requested_workspace` asserts a request with no mode gets the governed block.
- Precedence guard: `governed_sessions_refuse_a_project_configuration_that_can_override_the_block` (the real adapter refuses `ask`, `accept-edits`, `untrusted`, and no mode beside an `opencode.json` with `permission` and `agent` keys and beside a `.opencode` directory; `auto` still starts) and the unit tests `opencode::tests::governed_sessions_refuse_configuration_that_can_override_the_block`, `the_scan_stops_before_the_home_directory`, and `the_scan_stops_at_the_repository_root`.
- Global guard: `governed_sessions_refuse_the_owners_global_agent_configuration` (re-executes the test binary once per case with `XDG_CONFIG_HOME`, `USERPROFILE`, and `HOME` pointing at temporary directories, so the real home is never read and the other tests are not raced: clean roots start; a global top-level `permission` starts; an agent-scoped `opencode.json` under the XDG root, a `mode` in `~/.config/opencode/opencode.jsonc` without XDG, a `~/.opencode/agents/build.md`, and a non-UTF-8 configuration each refuse with the file named and no contents; the default root is ignored while XDG names another; `auto` always starts) and the unit tests `the_global_scan_refuses_agent_scoped_carriers_and_names_the_file`, `a_global_top_level_permission_alone_does_not_refuse`, and `the_global_scan_fails_closed_on_an_unreadable_file_or_directory`.
- Test caveat: the other governed tests in `tests/opencode.rs` and the daemon's OpenCode tests start sessions under the real process environment, so on a machine whose real global OpenCode configuration carries agent-scoped permissions they now refuse by design. This machine's `~/.config/opencode/opencode.jsonc` holds only `$schema` and `~/.opencode` holds `auth.json`, `package.json`, `package-lock.json`, and `node_modules`, with no config file or agent directory, so neither refuses a governed start here.
- Refusals: `observed_acp_records_a_reused_jsonrpc_id_and_refusals_are_not_progress` (a reused JSON-RPC id is recorded; a fixture that loops on a refused `fetch` request with 200 ms gaps ends as `Stalled` at the 1.5 s progress deadline instead of running to completion), and the duplicate case of `observed_acp_refuses_malformed_and_duplicate_requests_without_reauthorizing` now asserts the reused `toolCallId` is recorded.
- Task path: `a_prompt_free_opencode_session_refuses_a_narrower_delegated_task_before_any_session_starts` (a Codex lead delegates to a `full-autonomy` OpenCode employee with `requestedAuthority: "ask"`; the proposal is accepted; dispatch fails with `LZ-TASK-AUTHORITY-UNSUPPORTED`, the task is failed, and the start count does not move). The `promptFreeSessionNeverServesNarrowerAuthority` evidence flag is set only after this test, in both acceptance scripts.
- The daemon ACP-fixture loop deletes `fake-acp-response-1.json` before each submit, so the `cancelled` and `selected` assertions cannot read a stale file.
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
| `cargo test -p lazood --test daemon --locked -- --skip task_graph_acceptance_executes_backend_only_contract` | 0 | 145 passed, 0 failed, 1 filtered out (the skipped release-build test). |
| `cargo test -p lazoo-providers --locked` | 0 | 90 passed, 10 ignored (explicit live tests): lib 7, antigravity 6, claude 19, codex 17, config 2, config_apply 10, contract 10, grok 5, opencode 13, preview 1, registry 1. Needs `lazoo-session-host.exe` beside the binaries, built once with `cargo build -p lazoo-session-host --locked`. |
| `cargo fmt --all -- --check` | 0 | No formatting differences. |
| `cargo clippy -p lazood -p lazoo-providers --all-targets --locked -- -D warnings` | 0 | Clean. |
| `pwsh -NoProfile -File scripts/acceptance/opencode-governed-delivery.ps1` | 0 | PASS schema-19 manifest, `deterministic-fake-transport`, 14 commands (thirteen tests and the desktop diff guard), every evidence flag true. |
| `pwsh -NoProfile -File scripts/acceptance/opencode-full-autonomy.ps1` | 0 | PASS (renamed test and flags). |
| `[scriptblock]::Create` over the three OpenCode acceptance scripts | 0 | All parse; `opencode-governed-live.ps1` was parsed only, not run. |

## Boundary evidence

- `git diff -- apps/lazoo-desktop` is empty; the acceptance script enforces it. No schema, protocol, store, or CLI change.
- Two live probes: `opencode acp` with two prompts each on the free model in disposable directories (the permission-kind probe rejected every destructive request; the precedence probe ran only `hostname`). The daemon-level live run is the integrator's. No owner OpenCode configuration was read or written; the guard reads only the session's own working directory chain.
- The daemon's own environment is inherited by the launched process and cannot be scrubbed through `CommandSpec`; an `OPENCODE_PERMISSION` already in it would reach OpenCode under every mode. Lazoo sets one for every session except the version probe, and the live script scrubs it, `OPENCODE_CONFIG`, `OPENCODE_CONFIG_CONTENT`, and `OPENCODE_CONFIG_DIR` from the daemon environment (XDG variables stay so the owner's authentication works); the adapter itself refuses a governed start when any `OPENCODE_CONFIG*` variable is present.
- `UI BACKEND CONTRACT: GO` remains intentionally withheld.

## Integrator actions

- The gate exclusions for `opencode-governed-live` were added by the integrator and are unchanged here.
- Rerun `scripts\acceptance\opencode-governed-live.ps1` (rebuild `target\release` first). It now also: requires turn 3 to have an approved approval record decided by the user or policy and fails if none exists; requires turn 2 to record a `toolUpdate` whose final status is `failed`; asserts each approval is `command-execution` with action `execute an ACP command`; scrubs `OPENCODE_CONFIG`, `OPENCODE_CONFIG_CONTENT`, and `OPENCODE_CONFIG_DIR` as well as `OPENCODE_PERMISSION`. Parse-checked only here.
- The daemon's `provider.smoke` and every governed session now refuse to start beside the configuration listed under Decisions. Run the live script from a directory chain without an `opencode.json` that names `permission`, `agent`, `agents`, `mode`, or `tools` and without a `.opencode` directory below the home directory.
- Not changed: the integrator's later edits to the live script (the `hostname` prompt and `-like` comparison) were kept.

## Retained boundaries

- Network (`fetch`) and connector (`other`, MCP) requests are declined, not governed. Governing them needs a third approval kind, which changes the desktop Inbox contract (kinds today are `command-execution` and `file-change`): an owner decision.
- `external_directory`, `task`, and `skill` ask and, being kind `other` or unobserved, are declined; a lead's use of OpenCode subagents is therefore unavailable under governed modes.
- The owner's global OpenCode configuration is now probed and covered (see the global probe): `$XDG_CONFIG_HOME/opencode` (else `~/.config/opencode`) and `~/.opencode` are scanned for agent-scoped carriers. Still unprobed: whether a global `mode` entry (the legacy agent spelling) or a global `agents` key bypasses the block (the scan refuses them anyway, so it may refuse more than strictly needed); whether a global top-level legacy `tools` entry bypasses (not scanned, on the assumption it behaves like `permission`); and whether a global `agent`/`agents` markdown definition bypasses (the project-level `.opencode/agents/build.md` was in the earlier probe together with other sources, so it was not isolated).
- Not scanned: the managed system directory the OpenCode docs list (`%ProgramData%\opencode` on Windows, which loads `agents/` for every user), config-directory plugins and `OPENCODE_CONFIG_DIR` contents (the variable itself refuses), and `config.json`, a legacy global filename. A machine administrator or a plugin could still carry an agent-scoped allow.
- Which project source defeated the block (top-level `permission`, `agent.<name>.permission`, or `.opencode/agents/*.md`) is not isolated; the guard refuses all of them (and the legacy `mode` and `tools` keys), so it may refuse more than strictly needed.
- Approval cards carry no command or path target: the adapter drops permission titles and raw input as potentially secret, so the Inbox shows only `execute an ACP command`, `edit ACP files`, `delete ACP files`, or `move ACP files`. Surfacing a redacted target is an owner decision.
- Refusals are answered with the `cancelled` outcome, not `reject_once`. How OpenCode treats `cancelled` for a tool call is only inferred from the integrator's live run; the permission probe used `reject_once`.
- OpenCode `self-decision` behaves like `ask`: every ACP request is marked a scope expansion (no confinement proof), so the policy never auto-answers one and the user is always asked.
- Unproven live: that `read`, `glob`, `grep`, and `list` are allowed natively beside `"*":"ask"` and that `read` of `.env` is denied (the object form was used in the precedence probe's launch but no read was requested); that `list` exists in 1.19.0; how OpenCode reacts to the `cancelled` outcome (the probe used `reject_once`); that a real `delete` or `move` kind is ever emitted (the probe saw `write` as `edit`).
- Real-bridge `self-decision` always waits for the user until ACP requests can carry a confinement proof.
- Grok Build stays excluded; Antigravity has no prompt-free mode.
- The live delivery gate is owed before any claim of support.
