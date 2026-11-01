# Claude Code 2.1.316 native sessions verification

Date: 2026-11-01 (Asia/Calcutta). Branch: `fix/claude-code-2.1.316-native-sessions`, stacked on
`codex/copilot-account-routing` (pull request #42). Scope: `crates/lazoo-providers` only
(`native_startup.rs`, `interactive_session.rs`); no protocol, schema, store or daemon change.

## Why

The owner asked for a live team run on the #42 build: Claude (`claude-sonnet-5-5`), Codex
(`gpt-6-luna`), OpenCode (`opencode/glm-5.3-flash-free`), Antigravity
(`gemini-3.8-flash-medium`) and Copilot (`gpt-5.6-luna`) as employees, one lead delegating a
short question to the rest. Codex, OpenCode, Antigravity and Copilot workers answered, but every
Claude Code 2.1.316 session failed, and a Claude lead never received its result notices.

## What changed, and the failure each change removes

| Failure seen live | Cause in Claude Code 2.1.316 | Change |
| --- | --- | --- |
| `LZ-SESSION-START-FAILED`, native exit code 1 | The trust dialog defaults to "No, exit", ignores keys sent before it reads input, and can redraw itself with No selected again; Lazoo's immediate Down+Enter chose No | Trust keys are sent only on a screen quiet for 500 ms; Down repeats every 750 ms until Yes is shown; Enter is sent only while Yes is selected and settled |
| Startup timeout after the fix above | A moved selection is redrawn as bare glyphs at absolute rows (`CSI 14;2H` space, `CSI 15;2H` pointer), so "pointer before label" text never matched | `claude_trust_pointer` tracks cursor rows and compares the row of the last pointer glyph with the label rows |
| "native trust screen showed a foreign workspace" for a worker | Task worktree paths (about 160 characters) wrap at the 120-column terminal; the row break normalized to a space | The displayed and canonical paths are compared with whitespace removed |
| Notices to a Claude lead rejected: "native prompt hook did not match the canonical prompt" | `UserPromptSubmit` reports every bracketed paste (all multi-line prompts) as `\n\n<pasted_content id="N">\n…\n</pasted_content id="N">\n`, N a short hex id (`2864`, `78ea` seen) | `claude_pasted_prompt` accepts exactly that wrapper around the canonical prompt, matching ids, nothing looser |
| "stale Claude prompt identity" on a notice or a turn sent right after the previous one | Input typed just after `Stop` joins the prompt Claude is still closing: no `UserPromptSubmit`, and the next hooks carry the old `prompt_id` | Before typing a turn prompt, wait for 1.5 s of screen quiet (bounded at 5 s) |

## Live evidence (real Claude Code 2.1.316)

Debug build of this branch, disposable data root, real harness logins, driven through `lazooctl`
and the desktop app:

- Session start through the trust dialog: 3 of 3 fresh sessions idle in 4–5 s, no warnings.
- Multi-line user prompt: completed with `MULTILINE_OK`.
- Lead notice: Claude lead delegates one task to Copilot and ends its turn; 3 of 3 trials passed
  (notice accepted, `PILOT_OK` relayed, `task_report` completed, root task completed). Trials 2 and
  3 reused the same Claude session immediately after the previous task finished, the case that
  failed with a 750 ms settle (1 of 3 failed then).
- Back to back: five sequential turns on one session, each sent the moment the previous returned,
  replied `SEQ_1` … `SEQ_5`.
- Claude worker in a wrapped task-worktree path: a Codex lead delegated to the Claude employee,
  which replied `ATLAS_OK`; root task completed in 28 s.
- Provider live tests (`tests/interactive_native.rs`, ignored by default):
  `claude_native_mcp_and_permission_acceptance` passed (one MCP call, one permission, one approved
  command); `antigravity_native_tui_two_turn_acceptance` passed (guards the restructured trust
  branch); `claude_native_tui_two_turn_acceptance` passed its first turn and fails at resume, a
  pre-existing problem listed below (before this change the session could not start at all).

```powershell
$env:LAZOO_LIVE_CLAUDE_INTERACTIVE = '1'; $env:LAZOO_LIVE_CLAUDE_APPROVE_COMMAND = '1'
$env:LAZOO_LIVE_ANTIGRAVITY_INTERACTIVE = '1'
$env:LAZOO_NATIVE_HOOK_HELPER = (Resolve-Path target\debug\lazooctl.exe)
$env:LAZOO_LIVE_NATIVE_WORKSPACE_PARENT = $env:TEMP
cargo test -p lazoo-providers --locked --test interactive_native -- --ignored --test-threads=1
```

## Automated checks run

- `cargo fmt --all -- --check`
- `cargo clippy -p lazoo-providers --all-targets --locked -- -D warnings`
- `cargo test -p lazoo-providers --locked`: 66 unit tests and 98 integration tests pass; new unit
  tests cover the pointer rows (label redraws and bare glyphs, both directions), the wrapped path,
  and the paste wrapper (decimal and hex ids, mismatched or non-hex ids, trailing text).

## Not run here

The full gate (`scripts/gate.ps1`: workspace clippy and tests, the locked release build, every
acceptance script) did not fit this machine: about 1 GB of disk was free (a workspace clippy run
filled it), and the owner's running Lazoo instance executes from the main checkout's
`target\release`, which the release step would overwrite. Run before merge:

```powershell
pwsh -NoProfile -File scripts/gate.ps1 -OutputPath artifacts/acceptance/claude-2.1.316-gate.json
```

## Open, found in the same run, not changed here

- Resume: Lazoo-launched interactive Claude sessions leave no transcript under
  `~/.claude/projects`, so `--resume <id>` exits with "No conversation found with session ID".
  Every suspension (idle timeout, daemon restart) therefore loses the employee's Claude session.
  Lazoo-session transcripts exist only from runs before `6cb611b` (2026-10-29) added
  `--restricted`; that flag is the first suspect.
- Out-of-order `MessageDisplay` hooks end long Claude lead turns as uncertain ("Claude display
  sequence exceeds bounds": a new message arrived at index 1 before index 0).
- A Codex lead's process exited (code none) while it wrote its final answer; its rollout ends
  cleanly two seconds earlier.
- Every worker session start changes the roster revision, so leads retry `task_delegate`
  repeatedly (a Claude lead used about a dozen retries for four delegations).
- An idle Antigravity session left from a finished task kept the next task for that employee
  undispatched until the session was stopped.
- The prompt settle is a timing heuristic, not a readiness signal from Claude Code.
