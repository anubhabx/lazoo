# Provider live lifecycle verification

Date completed: 2026-10-17 IST

Branch: `feat/core-v1-completion` (main checkout; nothing committed by this slice)

Base: `cb09a4b` on `feat/core-v1-completion` (the Claude Code history below was written against `1c2980b`)

Product version: `0.1.0`

Database schema: 19 (this slice adds no migration and no production Rust change)

Scope: the live lifecycle acceptance script is now provider-aware. `scripts/acceptance/provider-live-lifecycle.ps1 -Provider <claude-code|codex|opencode>` (renamed with `git mv` from `scripts/acceptance/claude-live-lifecycle.ps1`) runs, in one daemon lifetime on a disposable `LAZOO_DATA_ROOT`, one employee of the chosen provider, a project-less conversation, and at most three real prompts on the owner's native account: messaging, cancellation (a long count, cancelled once the turn is `accepted`), and a turn after `session stop` (the wake), then checks the planned-shutdown native tree. Each provider has its own expectations (next section). Its manifest is `artifacts/acceptance/provider-live-lifecycle-<provider>-manifest.json`. It is excluded from the deterministic gate.

This slice also adds two ignored adapter-level live cancellation tests, `live_opencode_cancel_mid_turn_reports_cancelled` and `live_agy_cancel_mid_turn_reports_cancelled`, the real-CLI counterparts of the deterministic ACP and Antigravity cancel tests.

## Outcome

| Provider | Script status | Evidence |
|---|---|---|
| `claude-code` | PASS (2026-10-17, earlier run, not rerun here) | `artifacts/acceptance/provider-live-lifecycle-claude-code-manifest.json` (renamed with `git mv` from `claude-live-lifecycle-manifest.json`; contents unchanged, so it still carries the old `claudeVersion` key). Five gates in 20.5 s, 3 live turns. |
| `codex` | PASS (one run, 39.4 s, 3 live turns) | `artifacts/acceptance/provider-live-lifecycle-codex-manifest.json`. |
| `opencode` | WRITTEN, NOT RUN | The branch that makes the daemon deliver to OpenCode under `full-autonomy` had not merged; the integrator runs it after that merge. No OpenCode lifecycle manifest exists. |

| Ignored live adapter test | Result |
|---|---|
| `live_opencode_cancel_mid_turn_reports_cancelled` | PASS (18.1 s test time, 1 live turn on the free model) |
| `live_agy_cancel_mid_turn_reports_cancelled` | PASS (6.0 s, 1 live turn on the owner's Antigravity account), with a weak-evidence caveat below |

## Per-provider expectations

| | `claude-code` | `codex` | `opencode` |
|---|---|---|---|
| Employee | `--authority-mode full-autonomy` | default authority, created the way `conversation-delivery.ps1` does | `--authority-mode full-autonomy` |
| Turn authority | `full-autonomy` | default (`ask`); the prompts say "Do not use tools", so no permission prompt is exercised | `full-autonomy` |
| Turn 1 | trimmed output `LAZOO_CLAUDE_OK` | `LAZOO_CODEX_OK` | `LAZOO_OPENCODE_OK` (a leaked `</think>` tag is stripped for the comparison; the raw output is recorded) |
| Turn 2 | cancel once `accepted`; turn ends `cancelled`, no `turn.completed`, at least one `turn.cancelled`, session idle, stopping or suspended | same; the cancel goes through Codex's native interrupt | same, through the ACP cancel |
| Wake after `session stop` | RESUME: same native session id, exactly one `session.resumed` naming the prior session and native id, no `session.resume.skipped`, answer exactly `PELICAN` | FRESH by design: a new native session id, no `session.resumed`, exactly one `session.resume.skipped` with `reason` `codex-dynamic-tools`; the turn must complete; its trimmed output is recorded and NOT required to recall the code word | FRESH: a new native session id, no `session.resumed`, exactly one `session.resume.skipped` with `reason` `unsupported-provider`; the answer is recorded only |
| Daemon refuses delivery | not expected | not expected | `LZ-TURN-PROVIDER-UNSUPPORTED` fails the run at once with outcome `DELIVERY_REFUSED`, the refusal text in `evidence.deliveryRefusal`, and the refused submission is not counted as a live turn |
| Scrubbed from the daemon environment (names only recorded) | `ANTHROPIC_*`, `CLAUDECODE`, `CLAUDE_CODE_*` (except `CLAUDE_CODE_GIT_BASH_PATH`), `CLAUDE_PID`, `CLAUDE_EFFORT`, `CLAUDE_PLUGIN_*`, `AI_AGENT`, the Codex thread variables | `OPENAI_*`, `CODEX_API_KEY`, `CODEX_THREAD_ID`, `CODEX_SESSION_ID` | outer Claude and Codex session variables only |

All three share: the 3-turn cap per run, hard-refused by `Submit-LiveTurn`; timeouts and refusals are recorded outcomes (`TIMEOUT`, `BLOCKED_AUTHENTICATION`, `TURN_FAILED`, `CANCEL_NOT_CONFIRMED`, `DELIVERY_REFUSED`, `FAILED_ASSERTION`) with manifest `status` `FAIL` and exit 1; no retry loop; `-Build` stays opt-in and the default uses the existing release binaries.

## Codex run (2026-10-17, one run, 3 live turns)

Command: `pwsh -NoProfile -File scripts/acceptance/provider-live-lifecycle.ps1 -Provider codex` from `C:\workspace\Orchestrator` with the existing release binaries (`target\release\lazood.exe` built 15:06, `lazooctl.exe` built 14:05). Exit 0, output `PASS manifest: ...provider-live-lifecycle-codex-manifest.json`. All five gates PASS in 39.4 s.

| Gate | Result | Exact evidence from the manifest |
|---|---|---|
| authentication | PASS | `provider probe codex`: native `ready`, external `unknown`, verification `configured`, probe version `codex-cli 0.162.0-alpha.17.2`. The script's own `codex --version` on PATH (`codex.cmd`) printed `codex-cli 0.153.4`, so the probe and the PATH command resolve different Codex binaries; the manifest records both. |
| messaging | PASS | Turn 1 `completed` in 8,925 ms; trimmed output `LAZOO_CODEX_OK`; native session id `01a1494b-11fc-71a1-8d1c-f00638fc67c1` recorded. |
| cancellation | PASS | `turn cancel` returned `cancelled` 95 ms after the request; the background submit returned exit 0 after 446 ms; the stored turn is `cancelled`; 0 `turn.completed` and 1 `turn.cancelled` event for it; the session afterwards was `idle` (`STILL_USABLE_IDLE`), not stopped by the daemon, which is consistent with the interrupt path (Claude Code's host is killed instead). |
| resume (wake) | PASS | `session stop` ended the first session `suspended` (2,073 ms); its 16-process native tree had 0 survivors. Turn 3 `completed` in 13,961 ms on a NEW session `5ac1dd9c-55d1-406b-a8fb-a86ec6572b97` with a NEW native session id `01a1494b-444e-7482-ad50-9f22195ea3cb`. 0 `session.resumed` events; exactly one `session.resume.skipped`: `{"sessionId":"5ac1dd9c-...","priorSessionId":"17622a50-...","reason":"codex-dynamic-tools"}`. |
| plannedShutdown | PASS | Resumed-wake session native tree had 16 processes before `system shutdown`; the daemon exited 0 and every one of them was gone. |

Turn 3 evidence (not asserted as recall): prompt `Reply with exactly the code word I asked you to remember, and nothing else. Do not use tools.`; trimmed output `I don’t have the code word in this conversation.`; `freshStartAnswerRecalledCodeWord` false. That is the expected consequence of a fresh native Codex thread.

Live turns sent: 3 (cap 3). One run; the second permitted run was not needed. Only `-Provider codex` was run.

## Ignored live adapter cancellation tests (2026-10-17, one run each)

Both: `cargo test -p lazoo-providers --test <opencode|antigravity> <name> --locked -- --ignored --exact --nocapture` with `CARGO_TARGET_DIR=C:\workspace\Orchestrator\target`. Each starts the real session as the existing `live_*_model_backed_turn*` tests do, submits "Count slowly from 1 to 400, one number per line, with no other text. Do not use tools.", cancels, and asserts the turn returns within 60 s with a `Cancelled` event and no `TurnCompleted`. No production code changed.

`live_opencode_cancel_mid_turn_reports_cancelled`: PASS, `test result: ok. 1 passed`, 18.11 s including process start and stop. Printed `saw_delta=true cancel_after=13.1964064s returned_after=13.3948996s events=10`; terminal event `Cancelled { provider: OpenCode, session_id: "ses_eb6b8c17fffeJPFRe5Gq2ce1Rm", ... raw: {"stopReason":"cancelled", usage 0/0/0} }`. The test cancels after the first `TextDelta` the observer sees (or after 20 s without one); the delta arrived, so the cancel was genuinely mid-stream (about 13 s after the call began, which includes OpenCode's own startup and the model's first token). It also asserts the observer saw the `Cancelled` event, the property that failed for Claude Code before the earlier fix. It passed: the ACP adapter observes it.

`live_agy_cancel_mid_turn_reports_cancelled`: PASS, `test result: ok. 1 passed`, 6.03 s. Printed `cancel_after=6.0110807s returned_after=6.0135547s`; `events=1`, terminal event `Cancelled { provider: Antigravity, session_id: "pending:454b5350-22d6-4d29-aa25-8f9a7e35591f", turn_id: Some(...), raw: None }`. Caveat: the Antigravity adapter returns its events only after the turn (it uses the trait's default `turn_observed`), so there is no first-delta signal and the test cancels after a fixed 6 s. At that moment the CLI had emitted no `init` or text (one event, and the session id was still the `pending:` placeholder), so the test proves that `cancel()` kills the host and the adapter reports `Cancelled` promptly (2 ms after the kill) for a live `agy` process that is mid-startup or mid-first-response, not that a streaming answer was cut off. A longer wait (for example 15 s) would be the stronger variant; it was not run because the account is the owner's and the brief allows one run.

## Gate exclusion rename

`scripts/gate.ps1` now reads `$excluded = @('provider-adapters', 'provider-live-lifecycle', 'desktop-shell')`, and the header comment (line 19) names `provider-live-lifecycle.ps1` instead of `claude-live-lifecycle.ps1`. The identical literal pinned by `gate_runner_runs_the_whole_gate_and_excludes_only_the_live_and_ui_scripts` in `apps/lazood/tests/daemon.rs` (line 13780) was changed to match; `cargo test -p lazood --test daemon gate_runner_runs_the_whole_gate_and_excludes_only_the_live_and_ui_scripts --locked -- --exact` passed (1 passed). The gate was not run.

## Fresh verification matrix

| Command | Exit | Evidence |
|---|---:|---|
| `cargo fmt --all` | 0 | Only the two adapter test files and nothing else changed. |
| `cargo clippy -p lazoo-providers --all-targets --locked -- -D warnings` | 0 | Clean (the first two attempts failed on `doc_markdown` and `similar_names` in my new tests and were fixed). |
| `cargo test -p lazoo-providers --test opencode live_opencode_cancel_mid_turn_reports_cancelled --locked -- --ignored --exact --nocapture` | 0 | 1 passed (above). |
| `cargo test -p lazoo-providers --test antigravity live_agy_cancel_mid_turn_reports_cancelled --locked -- --ignored --exact --nocapture` | 0 | 1 passed (above). |
| `cargo test -p lazood --test daemon gate_runner_runs_the_whole_gate_and_excludes_only_the_live_and_ui_scripts --locked -- --exact` | 0 | 1 passed. |
| PowerShell parser on `provider-live-lifecycle.ps1` | 0 | 0 parse errors after each edit. |
| `pwsh -NoProfile -File scripts/acceptance/provider-live-lifecycle.ps1 -Provider codex` | 0 | PASS (above). |

Not run: `scripts/gate.ps1`, `cargo test --workspace`, a release build, `-Provider claude-code` (its PASS manifest from earlier today is kept) and `-Provider opencode`.

## Unproven

- The OpenCode branch of the script is written but NOT RUN. Its expectations (fresh start with `session.resume.skipped` reason `unsupported-provider`, a new native session id, the `DELIVERY_REFUSED` path, the `</think>` tolerance) are reasoned from the resume-on-wake record and the earlier OpenCode live results, not observed. Treat a first failure there as possibly a script expectation, not necessarily a product defect.
- Codex native interrupt is supported by the observed outcome (cancelled in 95 ms, session still `idle` and usable, then a clean `session stop` and a fresh-by-design wake), but the manifest's `evidence.cancelledEventPayloads` entry is truncated at 400 characters and does not itself name the interrupt method. The proof that the interrupt request is what ends the turn rests on the fixture test `codex_interrupt_correlates_the_active_turn` plus this behavior.
- Codex turn 3 never recalled the code word, by design; this run does not and cannot show a Codex resume. Codex resume waits on a vendor way to carry dynamic tools across `thread/resume`.
- The Antigravity live cancel is the weak variant described above.
- The `claude-code` manifest was produced by the script before the rename and the per-provider generalization (its `claudeVersion`, `authenticationChannel: native-subscription` and `userClaudeConfiguration` keys differ from the new per-provider manifests' `providerVersion`, `native-account` and `userProviderConfiguration`). The Claude Code branch of the generalized script keeps its prompts, assertions and flags (only the argument order of `employee create` differs) but has not been rerun after the edit, per the live-turn budget.
- `mode` in the generalized manifests is still the string `live-native-subscription` for every provider.

---

# Claude Code history (preserved from the earlier record)

The remainder is the record as it stood for `scripts/acceptance/claude-live-lifecycle.ps1`, the script's name before the rename. References to that file and to `claude-live-lifecycle-manifest.json` below are the old names of `provider-live-lifecycle.ps1 -Provider claude-code` and `provider-live-lifecycle-claude-code-manifest.json`.

## Claude Code outcome

Updated by the integrator after the fix: PASS. The adapter defect below was fixed on `feat/core-v1-completion` (the Claude Code adapter now observes the `Cancelled` event before returning it, and the fixture test asserts the observer sees it), the script's unwrapped `.Count` on the first session's process list was wrapped in `@()`, and the fifth run passed all five gates (authentication, messaging, cancellation, resume, plannedShutdown) in 20.5 s with 3 live turns: turn 1 `LAZOO_CLAUDE_OK`, turn 2 `cancelled`, `session stop` to `suspended`, turn 3 `PELICAN` on the same native session id with one `session.resumed` event and no `session.resume.skipped`, and no native tree process alive after the planned shutdown. The manifest is that PASS run. The text below records the runs that found the defect, exactly as written at the time; see `2026-10-17-live-lifecycle-gates.md` for the day's full live picture.

Original outcome: FAIL, and the failure is a product finding, not a script defect. Messaging PASSED live. Cancellation FAILED live: `lazooctl turn cancel` on an accepted Claude Code turn returned `LZ-TURN-UNCERTAIN` (exit 12) in 34 to 55 ms, and the turn ended `uncertain`, not `cancelled`. The resume and planned-shutdown gates were therefore NOT RUN. The script stops at the first failed gate and does not retry; I ran the whole script no more than the three runs allowed.

| Gate | Result | Evidence |
|---|---|---|
| authentication | PASS | `provider probe claude-code`: native `ready`, external `unknown`, verification `configured`, version `2.1.302 (Claude Code)`. No prompt is sent if the probe reports a missing native login. |
| messaging | PASS | Turn 1 completed in 7.4 s (run 2) and 9.1 s (run 3); trimmed output `LAZOO_CLAUDE_OK`; session and native session id recorded. |
| cancellation | FAIL | Cancel on the `accepted` turn: `LZ-TURN-UNCERTAIN`, "provider cancellation outcome is uncertain". Stored turn: state `uncertain`, `failureCode` `LZ-TURN-NO-ACCEPTANCE-EVIDENCE`, `failureMessage` "provider returned without durable completion evidence". Session after the cancel: `idle`. Reproduced on runs 2 and 3. |
| resume | NOT_RUN | Never reached. |
| plannedShutdown | NOT_RUN | Never reached. |

### Likely cause (read from the code, not changed or proven)

The deterministic proof of cancellation (`2026-10-17-adapter-cancellation.md`) asserts that `cancel()` returns the turn with exactly one `ProviderEvent::Cancelled`. It does not assert that the daemon's observer records it. In `crates/lazoo-providers/src/claude.rs`, `collect_turn` handles "host exited after a cancel request" by pushing `ProviderEvent::Cancelled` onto the returned `events` vector and returning `Ok(events)` (about line 410). Every `observer.observe(...)` call in that function is on a different path (about lines 448, 468, 492, 496). So the daemon's `AppTurnObserver` (`apps/lazood/src/app/turn.rs`), which is the only code that turns a `Cancelled` event into `finish_turn_cancel`, never sees it. `run_registered_submitting_turn` then gets `Ok(_)` from the provider and `finish_provider_delivery` finds the turn still `accepted` and marks it `uncertain` with `LZ-TURN-NO-ACCEPTANCE-EVIDENCE`, which is exactly the stored failure code. The cancel RPC's own polling loop then sees `uncertain` and returns `LZ-TURN-UNCERTAIN`. The fake used by the deterministic tests passes because it is checked at the adapter, below the observer. A fix (observe the `Cancelled` event before returning, or have `finish_provider_delivery` treat a cancel-requested turn as cancelled) is Rust and outside this slice; the integrator or owner decides. Until then this script FAILS at the cancellation gate by design: it is reporting a real defect. (The fix was later made on this branch; see the outcome above.)

### Design notes

| Decision | Reason |
|---|---|
| Turn 1 to 3 run through background `lazooctl turn submit`, with `--authority-mode full-autonomy --activate-if-needed` | The turn authority defaults to `ask`, which Claude Code refuses by design; submit blocks until the turn ends, so the cancel needs a second process. Each argument is quoted explicitly because `Start-Process -ArgumentList` joins with spaces (run 1 failed on that before any model call). |
| Cancel is requested as soon as `turn show` reports `accepted`, with the expected revision read just before and up to three revision-conflict retries | Mirrors `conversation-delivery.ps1`. Only a revision conflict is retried; any other refusal is the recorded outcome. |
| A refused or uncertain cancel records the stored turn failure code and message and the session state, then fails | This is what exposed the likely cause above. |
| Cancellation also asserts no `turn.completed` event for the cancelled turn, at least one `turn.cancelled`, and that the session afterwards is `idle`, `stopping` or `suspended`; which of them is recorded as `sessionAfterCancel` | The brief asks for "still usable or stopped by the daemon" to be recorded. |
| The stop uses `lazooctl session stop` only when the session is `idle`; otherwise the daemon's own stop is recorded | |
| Resume gate fails if any `session.resume.skipped` event exists (payload kept in the manifest), if the native session id differs, if there is not exactly one `session.resumed` naming the prior session, or if the output is not `PELICAN` | Specified for the Claude Code profile; the generalized script applies it only there. |
| The daemon environment is scrubbed of `ANTHROPIC_*`, `CLAUDE_CODE_*` (except `CLAUDE_CODE_GIT_BASH_PATH`), `CLAUDECODE`, `CLAUDE_PID`, `CLAUDE_EFFORT`, `CLAUDE_PLUGIN_*`, `AI_AGENT` and the Codex thread variables, names only recorded | The script was run from inside a Claude Code session; leaked session variables could make the child refuse to start or select a non-subscription channel (the Claude adapter treats `ANTHROPIC_API_KEY` and similar as an external credential). Only the child process environment is changed. |
| `-Build` is opt-in; the default uses the existing release binaries | The task forbids running cargo; a stale build is the caller's risk, the manifest records the Claude version. |
| Timeouts are recorded outcomes with `status` `FAIL`, exit 1 | A hung or unauthenticated turn must not hang or crash the script; there is no retry loop. |
| Hard cap of three live turns per run | `Submit-LiveTurn` refuses a fourth. |

### Commands

All from `C:\workspace\Orchestrator`, release binaries from `target\release`, Claude Code 2.1.302.

| Command | Exit | Evidence |
|---|---:|---|
| `pwsh -NoProfile -Command` parse of the script | 0 | 0 parse errors after each edit. |
| `pwsh -NoProfile -File scripts/acceptance/claude-live-lifecycle.ps1` (run 1) | 1 | Script defect, fixed: `Start-Process` did not quote the prompt, so `lazooctl` rejected the arguments (`unexpected argument 'the'`, 44 ms). Daemon and employee were created; no prompt reached the model. 0 model turns. |
| same (run 2) | 1 | Messaging PASS (7.4 s, `LAZOO_CLAUDE_OK`). Cancel returned `LZ-TURN-UNCERTAIN` in 44 ms. 2 model turns. |
| same (run 3, with the diagnostic capture added) | 1 | Messaging PASS (9.1 s, `LAZOO_CLAUDE_OK`). Cancel returned `LZ-TURN-UNCERTAIN` in 34 ms; stored failure `LZ-TURN-NO-ACCEPTANCE-EVIDENCE`; session `idle`. 2 model turns. Total 11.5 s. |
| same (run 4, after the adapter fix and a release rebuild) | 1 | Messaging PASS; cancellation PASS (the turn ended `cancelled`); resume gate FAILED_ASSERTION on a script error (`.Count` on an unwrapped process list under strict mode) before turn 3 was sent. 2 model turns. |
| same (run 5, after the one-line script fix) | 0 | PASS on all five gates in 20.5 s; 3 model turns; `PELICAN` returned on the resumed native session. |

Outputs recorded (no secrets):

| Turn | Prompt | Trimmed output | Duration |
|---|---|---|---|
| 1 messaging | `Remember the code word PELICAN for later in this conversation. Reply with exactly LAZOO_CLAUDE_OK and nothing else.` | `LAZOO_CLAUDE_OK` | 7.4 s / 9.1 s |
| 2 cancellation | `Count slowly from 1 to 400, one number per line, with no other text.` | none in runs 2 and 3 (the turn ended `uncertain`); `cancelled` in the final run | cancel call 34 to 55 ms before the fix |
| 3 resume | `Reply with exactly the code word I asked you to remember, and nothing else.` | `PELICAN` in the final run | not recorded here |

### Boundary evidence

- Live model turns sent for Claude Code before the final PASS were 4 across the first three runs (run 1: 0, run 2: 2, run 3: 2); the manifest's `liveTurnsSent` counts submissions attempted. The final passing run sent 3.
- Authentication channel: native subscription. The probe reported native `ready`; the daemon's environment carried no API credential or cloud selector.
- No user configuration changed. `%USERPROFILE%\.claude` and every other provider configuration were neither read nor written by the script; only the daemon child process environment was altered. Data lived in a temporary data root that the script removes.

### Retained boundaries (Claude Code)

- Permissions are `NOT_APPLICABLE_FULL_AUTONOMY_ONLY`: Claude Code receives turns only under `full-autonomy`; `ask` and `self-decision` are refused by design, so no permission prompt is exercised live.
- Forking a live session is a PROJECT-BRIEF section 27 decision and is not attempted.
- The idle/`stopped` state of a session whose host `cancel()` killed is unobserved; the Claude adapter's `cancel()` sets `stopped` and kills the host, and the session record stayed `idle` in the daemon before the fix.
