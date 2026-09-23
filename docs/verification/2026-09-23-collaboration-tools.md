# Codex collaboration tools verification

Date completed: 2026-09-23 IST

Branch: `feat/collaboration-tools`

Base: `main` at `81773b8` (`docs: record approval gate branch under feat naming`)

Product version: `0.1.0`

Database schema: 8 (unchanged; no migration in this slice)

Scope: provider contract, Codex adapter, daemon tool surface, fake-backend and fixture tests, acceptance harness, and continuity documentation. Desktop product code and contract, the CLI, and the store schema are unchanged.

## Outcome

PASS. Every daemon-managed Codex session now declares four Lazoo tools (`employees_list`, `task_delegate`, `task_wait`, `task_report`) through the app-server dynamic-tool surface, and the model can call them from inside its native session. Each call acts only as the turn's recipient employee and session and goes through the same store paths as `lazooctl task delegate|report`, so roster-revision, self-delegation, authority-narrowing, depth, fan-out, and active-descendant guards fail closed identically. Refused calls answer the model a stable `LZ-TOOL-*` or `LZ-TASK-*` code without ending the turn; an observer without the control plane answers `LZ-TOOL-UNATTACHED`.

`task_delegate` under `ask` produces a `proposed` task that waits for the local user; otherwise the child is accepted and wakes the existing dispatcher. `task_wait` blocks for at most ten minutes, polls the caller's task tree, aborts when the caller's turn ends, and returns each child's state, failure, and native completion output. A worker's explicit `task_report failed` remains authoritative over its later native completion. Tool wait time is excluded from the Codex turn budget in the same way approval waits are.

No live model turn was run. The only real-Codex interaction in this record is the existing non-model `native-session-ownership.ps1 -Provider codex` warm, which now exercises the tool declaration against Codex CLI 0.153.4.

## Design notes

Design and implementation landed together under the owner's standing instruction to continue the workstreams; `docs/superpowers/specs/2026-09-23-collaboration-tools-design.md` and `docs/superpowers/plans/2026-09-23-collaboration-tools.md` record the contract for owner review before merge. Two facts were established against the real provider during implementation:

| Fact | RED evidence | GREEN evidence |
|---|---|---|
| Codex rejects dotted dynamic tool names | The first full run of `native-session-ownership.ps1 -Provider codex` failed `session warm` with `LZ-SESSION-START-FAILED`; a direct app-server probe returned JSON-RPC `-32600 dynamic tool name must match ^[a-zA-Z0-9_-]+$ to match Responses API: employees.list`. | The catalog now declares `employees_list`, `task_delegate`, `task_wait`, `task_report`; the adapter refuses any other alphabet before `thread/start` (`codex_refuses_dynamic_tool_names_outside_the_responses_api_alphabet`), and the same acceptance passes. |
| Warming a session bumps the roster revision | The first daemon scenario tests scripted `task_delegate` with a revision read before `session.warm` and failed with `LZ-TASK-ROSTER-STALE`. | The tests read the revision after warming, matching what `employees_list` would return to a live lead. |

`codex app-server generate-json-schema --experimental` confirmed that `dynamicTools` exists only on `thread/start` and only behind `capabilities.experimentalApi`; the adapter enables that capability only when tools are declared and fails closed on resume.

Four pre-existing clippy 1.98 findings surfaced under `-D warnings` while building this slice and were fixed without behavior change: a `format!`-collect in `approval_fingerprint`, `Duration::from_secs(15 * 60)` for the approval timeout, and two over-length test functions (an existing daemon approval test and an existing store approval test) that now carry an explicit allow.

## Fresh verification matrix

All commands were run from `C:\workspace\Orchestrator` on branch `feat/collaboration-tools` after the final fixes.

| Command | Exit | Evidence |
|---|---:|---|
| `cargo fmt --all -- --check` | 0 | No formatting differences. |
| `cargo clippy --workspace --all-targets --locked -- -D warnings` | 0 | All workspace targets linted with warnings denied. |
| `cargo check --workspace --all-targets --locked` | 0 | All workspace targets checked. |
| `cargo test --workspace --locked` | 0 | Entire Rust workspace passed; 60 daemon integration tests (4 new: three tool scenarios and the acceptance-script contract), 16 Codex adapter tests (4 new), 9 adapter contract tests (1 new), 18 CLI tests, store and domain suites unchanged. Explicit live-provider tests remained ignored. |
| `cargo build --workspace --release --locked` | 0 | Locked release build passed. |
| `npm --prefix apps/lazoo-desktop/ui run check` | 0 | Prettier, TypeScript, Vitest, and the Vite production build passed on the unchanged frontend. |
| `scripts\acceptance\collaboration-tools.ps1` | 0 | PASS schema-8 manifest, `deterministic-fake-transport`, 9 commands. |
| `scripts\acceptance\approval-gate.ps1` | 0 | PASS schema-8 manifest. |
| `scripts\acceptance\first-slice.ps1 -SkipBuild` | 0 | PASS manifest; database schema 8. |
| `scripts\acceptance\employee-roster.ps1 -SkipBuild` | 0 | PASS manifest. |
| `scripts\acceptance\native-session-ownership.ps1 -SkipBuild -Provider codex` | 0 | PASS manifest; non-model session contract only, real Codex CLI 0.153.4 accepting the declared tools. |
| `scripts\acceptance\collaboration-bus.ps1 -SkipBuild` | 0 | PASS manifest. |
| `scripts\acceptance\conversation-delivery.ps1 -SkipBuild` | 0 | PASS deterministic manifest. |
| `scripts\acceptance\task-delegation-graph.ps1 -SkipBuild` | 0 | PASS manifest. |
| `scripts\acceptance\automatic-task-dispatch.ps1` | 0 | PASS schema-8 fake-transport manifest. |

## Collaboration-tools acceptance evidence

`artifacts/acceptance/collaboration-tools-manifest.json` records literal `PASS`, schema 8, and `deterministic-fake-transport`. Its focused commands prove:

- the default observer answers `LZ-TOOL-UNATTACHED` and `ToolResult` helpers are truthful;
- `normalize_codex_tool_call` keeps thread, turn, call id, tool, and arguments, with the active-turn fallback;
- the Codex adapter opts into `experimentalApi` only with declared tools, declares them as `dynamicTools` on `thread/start`, answers `item/tool/call` with `{contentItems: [inputText], success}`, redacts retained arguments in the `lazoo:<tool>` turn event, and fails closed for an unattached observer;
- sessions without tools keep `experimentalApi: false` and no `dynamicTools`, and declaring tools on resume fails with `ProviderError::Unsupported`;
- every daemon-managed Codex session declares the four tools, and a `self-decision` lead lists the roster with itself marked, delegates with the cited roster revision, and waits until the automatically dispatched child completes with the worker's native output;
- an `ask` lead's delegation stays `proposed` with no dispatch, while an unknown tool, malformed arguments, an authority-widening request, and a missing call id are refused with stable codes and the turn still completes;
- a dispatched worker's `task_report progress` and `task_report failed` carry its dispatch session, and the failure code outranks the later native completion;
- schema remains 8; and
- no provider smoke, external model use, or desktop UI change.

## Boundary evidence

- `git diff --check` passed.
- Both staged and unstaged diffs under `apps/lazoo-desktop` are empty.
- No Tauri command, capability, `desktop.snapshot` field, frontend source, CLI command, or migration changed.
- Tools are declared only for `ProviderKind::Codex` and only on `thread/start`; ACP, Claude Code, and Antigravity sessions are unchanged.
- No tool can name another actor, approve a governed action, or widen authority; a tool result is never an approval.
- No push or pull request was created.
- `UI BACKEND CONTRACT: GO` remains intentionally withheld.

## Retained follow-up boundaries

Result delivery to a lead whose turn ended (activation controls), minimum incidents with an `incident_list` tool, `message_send`/`message_reply` tools, a dedicated tool-call ledger, a live Codex tool-call gate, tools for ACP providers, typed desktop Inbox binding, ConPTY, per-capability policy, managed memory, scheduling, and resource governance remain separate reviewed slices.
