# OpenCode full-autonomy delivery verification

Date completed: 2026-10-17 IST

Branch: `feat/opencode-full-autonomy`

Base: `feat/core-v1-completion` at `cb09a4b`

Product version: `0.1.0`

Database schema: 19 (no migration)

Scope: one rule (`provider_enforces_authority`) behind the dispatch and turn delivery gates, so OpenCode employees receive turns and tasks under `full-autonomy` only; the OpenCode adapter launch, which sets the inline `OPENCODE_PERMISSION` block `{"*":"allow"}` for that one permission mode and no other; the deterministic story, launch-spec test and acceptance harness; and this record. No protocol, store, CLI, or desktop change. No live OpenCode run was made here and no support is claimed: the integrator runs the live delivery after merging.

## Outcome

PASS.

Before this slice OpenCode was refused at both gates under every authority. Now a Codex lead under full autonomy delegates to an OpenCode worker whose authority is full autonomy; the task is accepted, dispatched on an OpenCode session of its own (the fake backend records the session as started with the `auto` permission mode), and completed. The same lead's delegation to an OpenCode worker whose authority is `ask` is proposed, accepted by the user, and refused at dispatch with the task failed (`LZ-TASK-AUTHORITY-UNSUPPORTED`); a delegation to a `self-decision` OpenCode worker is accepted outright and refused at dispatch the same way; Grok Build and Antigravity workers under full autonomy are refused the same way. No session starts for any refused task. An OpenCode lead refuses user turns under `ask` and `self-decision` before any provider I/O and completes one under full autonomy on a session it activates for it. An OpenCode employee is still rejected as a memory worker, which needs the governed `ask` ceiling. The existing before-I/O refusal test is unchanged and still passes.

## Decisions

| Decision | Reason |
|---|---|
| Only `full-autonomy` | It is the one authority OpenCode's native configuration can enforce prompt-free; under `ask` and `self-decision` a native allow rule could bypass Lazoo's approval observer, the same reasoning that limited Claude Code to full autonomy. |
| The permission mode, not the provider, selects the environment | `command()` sets `OPENCODE_PERMISSION` only when `SessionRequest.permission_mode` is `auto`, the string `authority_permission_mode(FullAutonomy)` produces. Any other mode, no mode (the probe), or a near miss (`AUTO`, `full-autonomy`, empty) leaves the environment empty, so the adapter never widens authority on its own. |
| Inline block, never a config file | The variable is passed to the launched process only; no OpenCode config file is written, edited, or read by Lazoo. |
| `OPENCODE_PERMISSION` with `{"*":"allow"}` | See the vendor fact below. A top-level `"*"` rule covers built-in and MCP tools alike, and the inline block outranks global, custom, project, and `.opencode` config (established against OpenCode 1.19.0 before this slice; not re-run live here). |
| Approval observer unchanged | An ACP `session/request_permission` that still arrives under full autonomy follows today's path: execute and edit kinds are admitted, other kinds refused. |
| Memory worker still excludes OpenCode | `memory_worker.rs` and `memory_worker_execution.rs` require `provider_enforces_authority(provider, AuthorityMode::Ask)`, which stays false for OpenCode; the story asserts the configure RPC refuses an OpenCode summary employee. |
| No support claim | Brief section 21.3: a deterministic path is not a live gate. |

### Vendor fact and Context7 quote

Checked with Context7 on 2026-10-17, library `/websites/opencode_ai` (source `https://opencode.ai/docs/cli`, "Environment variables"):

> `OPENCODE_PERMISSION` | string | Inlined json permissions config

and library `/anomalyco/opencode` (`packages/web/src/content/docs/permissions.mdx`), which shows the `permission` object shape that the inline value takes, with `"*"` as the global fallback:

> `"permission": { "*": "ask", "bash": "allow", "edit": "deny" }`

The same documentation states that without configuration OpenCode "allows all operations" and that "the last matching rule wins". Context7 documents the variable name, its type (inline JSON permissions config), and the `"*"` rule shape; the precedence over config files and the coverage of MCP tools come from the 1.19.0 inspection and are not in the quote. The value `{"*":"allow"}` is the `permission` object itself, without the `"permission"` wrapper that appears in `opencode.json`.

## What stays refused

- `ask` and `self-decision` for OpenCode, at both gates, before any provider I/O.
- Grok Build and Antigravity under every authority (Antigravity has no prompt-free mode; Grok Build is unchanged).
- OpenCode as a memory worker.
- A turn with no task fails closed.

## Commands

All commands ran in `C:\workspace\Orchestrator\.claude\worktrees\s3-opencode` with `CARGO_TARGET_DIR` set to that worktree's `target` directory. The workspace test, `scripts\gate.ps1`, and release build were not run (integrator owns the full gate).

| Command | Exit | Evidence |
|---|---:|---|
| `cargo test -p lazood --test daemon opencode_runs_turns_and_tasks_only_under_full_autonomy --locked -- --exact` | 0 | 1 passed. |
| `cargo test -p lazood --test daemon opencode_full_autonomy_acceptance_contract_is_deterministic_and_backend_only --locked -- --exact` | 0 | 1 passed. |
| `cargo test -p lazood --test daemon task_turn_rejects_a_provider_without_authority_override_support_before_io --locked -- --exact` | 0 | 1 passed. |
| `cargo test -p lazood --test daemon claude_code_runs_turns_and_tasks_only_under_full_autonomy --locked -- --exact` | 0 | 1 passed (Claude Code unchanged). |
| `cargo test -p lazood --test daemon --locked -- --skip task_graph_acceptance_executes_backend_only_contract` | 0 | 140 passed, 0 failed, 1 filtered out (the skipped release-build test). |
| `cargo test -p lazoo-providers --test opencode --locked` | 0 | 8 passed, 2 ignored (explicit live-provider tests); includes `opencode_launch_is_prompt_free_only_under_full_autonomy`. The binary needs `lazoo-session-host.exe` beside it, built once with `cargo build -p lazoo-session-host --locked`. |
| `cargo fmt --all -- --check` | 0 | No formatting differences. |
| `cargo clippy -p lazood -p lazoo-providers --all-targets --locked -- -D warnings` | 0 | Clean. |
| `pwsh -NoProfile -File scripts/acceptance/opencode-full-autonomy.ps1` | 0 | PASS schema-19 manifest, `deterministic-fake-transport`, 5 commands, every evidence flag true. |

## Boundary evidence

- `git diff -- apps/lazoo-desktop` and the staged diff are empty; the acceptance script enforces this.
- No schema, protocol, store, or CLI change; schema 19 is unchanged.
- No real `opencode` binary was launched and no prompt was sent. The launch is proven on the `CommandSpec` only, and the delivery on the fake session backend.
- The daemon's own environment is inherited by the launched process and cannot be scrubbed through `CommandSpec`; an `OPENCODE_PERMISSION` already present in that environment would reach OpenCode under every mode. Lazoo does not set one outside full autonomy.
- Collaboration tools stay Codex-only; an OpenCode session receives none.
- `UI BACKEND CONTRACT: GO` remains intentionally withheld.

## Retained boundaries

- Governed delivery under `ask` and `self-decision` needs a forced `"*": "ask"` block (so every action raises `session/request_permission`), a widening of the admitted ACP tool kinds, and a live `session/request_permission` proof. These are owner decisions and are not in this slice.
- Grok Build stays excluded.
- Antigravity has no prompt-free mode.
- The live delivery gate (session, user turn, dispatched task, cancellation, resume through a configured endpoint) is owed before any claim of support; it is the integrator's to run after merging.
