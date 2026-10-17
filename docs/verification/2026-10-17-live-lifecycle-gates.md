# Live provider lifecycle gates, 2026-10-17

Date completed: 2026-10-17 IST

Branch: `feat/core-v1-completion` (pull request #37, stacked on `codex/core-v1-backend`)

Product version: `0.1.0`

Database schema: 19

Scope: the live, owner-account runs behind PROJECT-BRIEF section 21.3 for the four required providers, executed through the owner's installed and already-authenticated CLIs on this machine: the ignored `live_*` adapter tests in `crates/lazoo-providers/tests`, the daemon smoke `scripts/acceptance/provider-adapters.ps1`, and the new `scripts/acceptance/provider-live-lifecycle.ps1 -Provider claude-code` (messaging, cancellation, resume on wake, planned shutdown through the real daemon and CLI). Grok Build is excluded by the owner. Every run used a disposable `LAZOO_DATA_ROOT`; no user provider configuration was changed. Transcript: `artifacts/2026-10-17-live-gates.log`.

## Outcome

PASS for every provider's live lifecycle as defined below, after two retries and three product fixes (a Claude Code cancellation that ended `uncertain`, an OpenCode lead that could never become dormant, and ACP stop reasons the daemon did not recognise as completion), each found by these runs and re-proven live.

| Provider | Readiness (no model) | Adapter turn and resume | Daemon smoke (`provider-adapters.ps1`) | Lifecycle beyond a reply |
|---|---|---|---|---|
| Claude Code 2.1.302, native subscription | `live_claude_non_model_probe` ok, 0.66 s | `live_claude_model_backed_turn_and_resume`: first attempt FAILED (`InvalidEvent`, "frame before correlated startup init", 3.4 s); PASS on the retry, 20.0 s | PASS, 12.6 s, exact `LAZOO_CLAUDE_OK` | `provider-live-lifecycle.ps1 -Provider claude-code` PASS: messaging, cancellation, resume on wake, planned shutdown (below) |
| Codex 0.153.4 (`codex.cmd`), ChatGPT account | `live_codex_app_server_handshake` ok, 4.1 s; `live_codex_session_start_reports_contained_host` ok, 3.5 s | `live_codex_single_turn_and_resume` PASS, 20.1 s | PASS, exact `LAZOO_CODEX_OK` | `provider-live-lifecycle.ps1 -Provider codex` PASS (39.4 s, 3 turns): `LAZOO_CODEX_OK`, a live cancel through the native interrupt ended `cancelled` 95 ms after the request with no `turn.completed`, `session stop` to `suspended`, a fresh wake with one `session.resume.skipped` (`codex-dynamic-tools`) and a new native thread, planned shutdown left none of the 16 native tree processes |
| OpenCode 1.19.0 ACP, Big Pickle | `live_opencode_acp_handshake` ok, 4.5 s | `live_opencode_model_backed_turn` (turn, `session/load`, second turn): first attempt FAILED because the resumed reply was `</think>LAZOO_OPENCODE_RESUME_OK`; PASS on the retry, 18.6 s | first attempt FAIL (turn completed, 28,966 tokens, reply did not equal the token); PASS on the retry | `provider-live-lifecycle.ps1 -Provider opencode` PASS (59.1 s, 4 turns: verification smoke, messaging `LAZOO_OPENCODE_OK`, a live cancel through the daemon ended `cancelled`, `session stop` to `suspended`, a fresh wake with one `session.resume.skipped` (`unsupported-provider`), planned shutdown left no native process); adapter-level `live_opencode_cancel_mid_turn_reports_cancelled` PASS (18.1 s, mid-stream); stall handling stays the deterministic `opencode_acceptance_without_progress_becomes_stalled` |
| Antigravity 1.3.6 (`agy`) | `live_agy_non_model_probe` ok, 16.9 s | `live_agy_model_backed_turn_and_resume` PASS, 43.7 s (`--conversation <id>` resume) | PASS, exact `LAZOO_AGY_OK` | `live_agy_cancel_mid_turn_reports_cancelled` PASS (6.0 s), weak: the adapter returns events only after the turn, so the cancel fired after a fixed 6 s before any output, proving a prompt kill and `Cancelled`, not a cut-off stream |

The two first-attempt failures did not recur. The OpenCode one is model variance: the free model leaked a closing `</think>` tag into its answer text, so the exact-token check failed although the ACP lifecycle (session, prompt, streamed text, usage, completion, `session/load`, second turn) completed; it is not a Lazoo defect and the retry passed with the exact token. The Claude one is unexplained: the adapter refused a frame that arrived before a correlated `init`, and the refusal did not say which frame. The refusal now names the frame's type, subtype, and session id, so a recurrence is classifiable; a manual reproduction with the adapter's exact flags from the test's working directory showed eight `SessionStart` hook frames (all matching the session), then `init`, `commands_changed`, `thinking_tokens`, `assistant`, `rate_limit_event`, `result`, which the adapter accepts. No adapter logic changed between the failed attempt and the passing retry.

## Claude Code live lifecycle

`scripts/acceptance/provider-live-lifecycle.ps1 -Provider claude-code` starts the real `lazood.exe` on a disposable data root with this outer Claude session's `ANTHROPIC_*`, `CLAUDE_CODE_*`, and session variables removed from the daemon's environment, so the native login is the only channel; creates a Claude Code lead under `full-autonomy`; and runs one daemon lifetime:

1. Turn 1 asks the model to remember the code word `PELICAN` and reply exactly `LAZOO_CLAUDE_OK`: PASS.
2. Turn 2 asks for a long count and is cancelled with `lazooctl turn cancel` once accepted: the turn ends `cancelled`.
3. `lazooctl session stop` ends the session `suspended`.
4. Turn 3 asks for the remembered word: the reply is exactly `PELICAN`, the new session's native session id equals the first session's, the ledger holds exactly one `session.resumed` event naming the prior session and that native id and no `session.resume.skipped`.
5. Planned daemon shutdown leaves no native tree process alive.

Runs: the first complete run reached turn 2 and FAILED cancellation: `lazooctl turn cancel` returned `LZ-TURN-UNCERTAIN` and the turn was stored `uncertain` with `LZ-TURN-NO-ACCEPTANCE-EVIDENCE`. Cause, fixed on this branch: when the Claude Code host exited after a cancel request, `collect_turn` appended `ProviderEvent::Cancelled` to the returned events but never passed it to the turn observer, and the daemon records a cancellation only from its observer, so the turn stayed `accepted` and ended `uncertain`. The adapter now observes the `Cancelled` event before returning it; `claude_cancel_during_active_turn_kills_host_and_reports_cancelled` now runs the observed turn path and asserts the observer saw the cancellation (the earlier version of that test checked the returned events only, which is why the fixture gate missed it). The Antigravity adapter has no observed path of its own and uses the trait's default, which observes returned events after the turn, so it did not need the change. The second run passed cancellation and failed the resume gate on a script error (`.Count` on an unwrapped process list under strict mode) before turn 3 was sent; the third run, after that one-line script fix, passed all five gates in 20.5 s with 3 live turns. The script's own record is `docs/verification/2026-10-17-provider-live-lifecycle.md`.

## OpenCode daemon lifecycle

After OpenCode full-autonomy delivery merged, `provider-live-lifecycle.ps1 -Provider opencode` (free Big Pickle model, disposable data root) exposed two product gaps in turn, each fixed on this branch and re-proven live:

1. `conversation create` was refused with `LZ-CONVERSATION-LEAD-NOT-READY` ("provider model readiness has not been verified"), twice, even after a completed `provider smoke` answered `LAZOO_OPENCODE_OK`: the roster makes an employee dormant only for a `configured` or `verified` provider observation with a ready login, OpenCode's probe can report neither, and nothing recorded `verified`. A completed daemon smoke now records a `verified` observation with a ready native channel, sticky across later probes that cannot see a login (`2026-10-17-smoke-verifies-provider.md`). The script's OpenCode profile runs that smoke as its authentication gate.
2. The first delivered turn then ended `uncertain` (`LZ-TURN-UNCERTAIN`): the ACP adapter reported the raw `stopReason` (`end_turn`) as the completion status and the daemon marks a turn completed only for `completed`. The adapter now maps ACP stop reasons onto the contract vocabulary (unit test `acp_stop_reasons_normalize_to_contract_statuses`).

With both fixes the run passed messaging and live cancellation through the daemon and was refused only by the script's own three-turn cap, which the verification smoke consumes; with the cap at four for OpenCode the final run passed all five gates in 59.1 s with 4 live turns (`artifacts/acceptance/provider-live-lifecycle-opencode-manifest.json`).

## Commands

| Command | Exit | Result |
|---|---|---|
| `pwsh -File scripts/acceptance/provider-live-lifecycle.ps1 -Provider codex` (the script generalized from the Claude one; Claude evidence kept as `provider-live-lifecycle-claude-code-manifest.json`) | 0 | PASS, 39.4 s, 3 live turns; `artifacts/acceptance/provider-live-lifecycle-codex-manifest.json`. |
| `cargo test -p lazoo-providers --test opencode live_opencode_cancel_mid_turn_reports_cancelled --locked -- --ignored --exact` and `--test antigravity live_agy_cancel_mid_turn_reports_cancelled` | 0, 0 | ok 18.1 s and ok 6.0 s, one live turn each. |
| `pwsh -File scripts/acceptance/provider-live-lifecycle.ps1 -Provider opencode` (after OpenCode full-autonomy delivery merged) | 1, 1 | `LZ-CONVERSATION-LEAD-NOT-READY`, "provider model readiness has not been verified", both before any lifecycle turn: a fresh data root never makes an OpenCode employee dormant because its probe cannot report a ready login or a verified model, and a completed `provider.smoke` (`LAZOO_OPENCODE_OK`) records nothing. Product gap, see below. |
| The same after the readiness fix, then after the ACP stop-reason fix, then with the cap at four | 1, 1, 0 | `LZ-TURN-UNCERTAIN` on turn 1 (stop reason), then messaging and cancellation PASS but the wake refused by the three-turn cap, then PASS on all five gates in 59.1 s with 4 live turns. |
| `pwsh -File scripts/acceptance/opencode-governed-live.ps1` (after OpenCode governed delivery merged; prompt changed from `echo LAZOO_ASK_OK` to `hostname` because the free model answered the echo from the prompt text without running anything) | 1, then 0 | FAIL (no approval raised for the echo prompt), then PASS in 89.7 s with 4 live turns: under `ask` the shell command raised an `action-required` Inbox approval, `lazooctl approval approve` let it complete with the real machine name, `approval decline` left the tool `failed` with no output, under `self-decision` the request (a scope expansion to the bridge) waited for the user and completed after approval, and planned shutdown left nothing alive; `artifacts/acceptance/opencode-governed-live-manifest.json`. |
| The same after the review-driven hardening (block for every non-`auto` mode, `.env` reads denied, fail-closed beside a repository OpenCode config, stricter assertions: approval kind `command-execution`, a `failed` tool update on the decline, an approval record required under `self-decision`) | 0 | PASS in 66.3 s with 4 live turns; the inline block does not beat a repository's own agent-scoped OpenCode permissions (probed live), so governed starts now fail closed beside such a config and the owner's global config remains the one unprobed source. |
| `cargo test -p lazoo-providers --test <claude|codex|opencode|antigravity> live_*_probe / *handshake* / *contained_host --locked -- --ignored --exact` | 0 | Five readiness checks ok (durations above). |
| `cargo test -p lazoo-providers --test claude live_claude_model_backed_turn_and_resume --locked -- --ignored --exact` | 101, then 0 | FAILED once (see Outcome), then ok in 20.0 s. |
| `cargo test -p lazoo-providers --test codex live_codex_single_turn_and_resume --locked -- --ignored --exact` | 0 | ok, 20.1 s. |
| `cargo test -p lazoo-providers --test opencode live_opencode_model_backed_turn --locked -- --ignored --exact` | 101, then 0 | FAILED once on `</think>LAZOO_OPENCODE_RESUME_OK`, then ok in 18.6 s. |
| `cargo test -p lazoo-providers --test antigravity live_agy_model_backed_turn_and_resume --locked -- --ignored --exact` | 0 | ok, 43.7 s. |
| `pwsh -File scripts/acceptance/provider-adapters.ps1 -Provider <p> -SkipBuild` for claude-code, codex, opencode, antigravity | 0, 0, 1 then 0, 0 | PASS, PASS, FAIL then PASS on retry, PASS; `artifacts/acceptance/provider-adapters-manifest.json`. |
| `pwsh -File scripts/acceptance/provider-live-lifecycle.ps1 -Provider claude-code` | 1, 1, 0 | FAIL at cancellation (product defect, fixed), FAIL at resume (script defect, fixed), PASS in 20.5 s with 3 live turns; `artifacts/acceptance/provider-live-lifecycle-claude-code-manifest.json`. |
| `cargo test -p lazoo-providers --test claude --locked` and `--test antigravity --locked` after the fix | 0 | 19 passed and 6 passed (2 ignored each). |

Live model turns sent on 2026-10-17, all short: about 9 through the lifecycle script across its development and final runs, 11 through the adapter tests (including the retries), 5 through the daemon smokes (OpenCode twice), and 2 manual frame-order probes of the Claude CLI. No turn used tools.

## Boundary evidence

- No user provider configuration was read for writing or changed; every daemon ran on a disposable data root, and the vendor probes earlier in the day used disposable homes.
- The authentication channel actually verified for Claude Code is the native subscription (`authenticationChannel: native-subscription` in the lifecycle manifest; the external API variables were removed from the daemon environment).
- `provider-live-lifecycle` and `provider-adapters` are excluded from the deterministic gate (`scripts/gate.ps1` `$excluded`, pinned by `gate_runner_runs_the_whole_gate_and_excludes_only_the_live_and_ui_scripts`).
- `UI BACKEND CONTRACT: GO` remains intentionally withheld.

## Retained boundaries

- Permissions under `ask` and `self-decision` are refused for Claude Code by design (only the managed-settings tier can make the harness the sole permission source), so the live permission gate for Claude Code is the fixture-proven `can_use_tool` bridge plus that refusal; OpenCode's live `session/request_permission` proof waits on the owner's governed-delivery decision.
- Live cancellation is proven for Claude Code and Codex through the daemon (`provider-live-lifecycle.ps1`) and for OpenCode and Antigravity at the adapter level (ignored `live_*_cancel_mid_turn_reports_cancelled` tests); the Antigravity proof is a prompt kill before any output, not a cut-off stream.
- The Claude Code first-attempt refusal is unexplained; the diagnostic refusal message is the instrument for the next occurrence.
- Exact-token conformance on the free OpenCode model is subject to model variance (`</think>` leak); the lifecycle itself completed both times.
- Terminal attach was proven on the real binaries without a model (`session-terminal-attach.ps1`); nobody has yet looked at the resumed native TUI's rendered history by eye.
