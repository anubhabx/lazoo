# Claude Code live lifecycle verification

Date completed: 2026-10-17 IST

Branch: `feat/core-v1-completion` (main checkout; nothing committed)

Base: `1c2980b` on `feat/core-v1-completion`

Product version: `0.1.0`

Database schema: 19 (this slice adds no migration and no Rust change)

Scope: one new LIVE acceptance script, `scripts/acceptance/claude-live-lifecycle.ps1`, the live counterpart of the deterministic cancellation and resume-on-wake proofs. In one daemon lifetime on a disposable `LAZOO_DATA_ROOT` it creates a Claude Code employee under `full-autonomy`, a project-less conversation, and sends at most three real prompts to the owner's native Claude subscription: messaging (`LAZOO_CLAUDE_OK` plus the code word PELICAN), cancellation (a long count, cancelled once the turn is `accepted`), and resume (stop the session, then ask for the code word back). It then checks the planned-shutdown native tree. It writes `artifacts/acceptance/claude-live-lifecycle-manifest.json`. It is excluded from the deterministic gate.

## Outcome

Updated by the integrator after the fix: PASS. The adapter defect below was fixed on `feat/core-v1-completion` (the Claude Code adapter now observes the `Cancelled` event before returning it, and the fixture test asserts the observer sees it), the script's unwrapped `.Count` on the first session's process list was wrapped in `@()`, and the fifth run passed all five gates (authentication, messaging, cancellation, resume, plannedShutdown) in 20.5 s with 3 live turns: turn 1 `LAZOO_CLAUDE_OK`, turn 2 `cancelled`, `session stop` to `suspended`, turn 3 `PELICAN` on the same native session id with one `session.resumed` event and no `session.resume.skipped`, and no native tree process alive after the planned shutdown. The manifest at `artifacts/acceptance/claude-live-lifecycle-manifest.json` is that PASS run. The text below records the runs that found the defect, exactly as written at the time; see `2026-10-17-live-lifecycle-gates.md` for the day's full live picture.

Original outcome: FAIL, and the failure is a product finding, not a script defect. Messaging PASSED live. Cancellation FAILED live: `lazooctl turn cancel` on an accepted Claude Code turn returned `LZ-TURN-UNCERTAIN` (exit 12) in 34 to 55 ms, and the turn ended `uncertain`, not `cancelled`. The resume and planned-shutdown gates were therefore NOT RUN. The script stops at the first failed gate and does not retry; I ran the whole script no more than the three runs allowed.

| Gate | Result | Evidence |
|---|---|---|
| authentication | PASS | `provider probe claude-code`: native `ready`, external `unknown`, verification `configured`, version `2.1.302 (Claude Code)`. No prompt is sent if the probe reports a missing native login. |
| messaging | PASS | Turn 1 completed in 7.4 s (run 2) and 9.1 s (run 3); trimmed output `LAZOO_CLAUDE_OK`; session and native session id recorded. |
| cancellation | FAIL | Cancel on the `accepted` turn: `LZ-TURN-UNCERTAIN`, "provider cancellation outcome is uncertain". Stored turn: state `uncertain`, `failureCode` `LZ-TURN-NO-ACCEPTANCE-EVIDENCE`, `failureMessage` "provider returned without durable completion evidence". Session after the cancel: `idle`. Reproduced on runs 2 and 3. |
| resume | NOT_RUN | Never reached. |
| plannedShutdown | NOT_RUN | Never reached. |

### Likely cause (read from the code, not changed or proven)

The deterministic proof of cancellation (`2026-10-17-adapter-cancellation.md`) asserts that `cancel()` returns the turn with exactly one `ProviderEvent::Cancelled`. It does not assert that the daemon's observer records it. In `crates/lazoo-providers/src/claude.rs`, `collect_turn` handles "host exited after a cancel request" by pushing `ProviderEvent::Cancelled` onto the returned `events` vector and returning `Ok(events)` (about line 410). Every `observer.observe(...)` call in that function is on a different path (about lines 448, 468, 492, 496). So the daemon's `AppTurnObserver` (`apps/lazood/src/app/turn.rs`), which is the only code that turns a `Cancelled` event into `finish_turn_cancel`, never sees it. `run_registered_submitting_turn` then gets `Ok(_)` from the provider and `finish_provider_delivery` finds the turn still `accepted` and marks it `uncertain` with `LZ-TURN-NO-ACCEPTANCE-EVIDENCE`, which is exactly the stored failure code. The cancel RPC's own polling loop then sees `uncertain` and returns `LZ-TURN-UNCERTAIN`. The fake used by the deterministic tests passes because it is checked at the adapter, below the observer. A fix (observe the `Cancelled` event before returning, or have `finish_provider_delivery` treat a cancel-requested turn as cancelled) is Rust and outside this slice; the integrator or owner decides. Until then this script FAILS at the cancellation gate by design: it is reporting a real defect.

## Design notes

| Decision | Reason |
|---|---|
| Turn 1 to 3 run through background `lazooctl turn submit`, with `--authority-mode full-autonomy --activate-if-needed` | The turn authority defaults to `ask`, which Claude Code refuses by design; submit blocks until the turn ends, so the cancel needs a second process. Each argument is quoted explicitly because `Start-Process -ArgumentList` joins with spaces (run 1 failed on that before any model call). |
| Cancel is requested as soon as `turn show` reports `accepted`, with the expected revision read just before and up to three revision-conflict retries | Mirrors `conversation-delivery.ps1`. Only a revision conflict is retried; any other refusal is the recorded outcome. |
| A refused or uncertain cancel records the stored turn failure code and message and the session state, then fails | This is what exposed the likely cause above. |
| Cancellation also asserts no `turn.completed` event for the cancelled turn, at least one `turn.cancelled`, and that the session afterwards is `idle`, `stopping` or `suspended`; which of them is recorded as `sessionAfterCancel` | The brief asks for "still usable or stopped by the daemon" to be recorded. Not reached in any run. |
| The stop uses `lazooctl session stop` only when the session is `idle`; otherwise the daemon's own stop is recorded | Not reached. |
| Resume gate fails if any `session.resume.skipped` event exists (payload kept in the manifest), if the native session id differs, if there is not exactly one `session.resumed` naming the prior session, or if the output is not `PELICAN` | Specified; not reached. |
| The daemon environment is scrubbed of `ANTHROPIC_*`, `CLAUDE_CODE_*` (except `CLAUDE_CODE_GIT_BASH_PATH`), `CLAUDECODE`, `CLAUDE_PID`, `CLAUDE_EFFORT`, `CLAUDE_PLUGIN_*`, `AI_AGENT` and the Codex thread variables, names only recorded | The script was run from inside a Claude Code session; leaked session variables could make the child refuse to start or select a non-subscription channel (the Claude adapter treats `ANTHROPIC_API_KEY` and similar as an external credential). Only the child process environment is changed. |
| `-Build` is opt-in; the default uses the existing release binaries | The task forbids running cargo; a stale build is the caller's risk, the manifest records the Claude version. |
| Timeouts are recorded outcomes (`TIMEOUT`, `BLOCKED_AUTHENTICATION`, `TURN_FAILED`, `CANCEL_NOT_CONFIRMED`, `FAILED_ASSERTION`) with `status` `FAIL`, exit 1 | A hung or unauthenticated turn must not hang or crash the script; there is no retry loop. |
| Hard cap of three live turns per run | `Submit-LiveTurn` refuses a fourth. |

## Commands

All from `C:\workspace\Orchestrator`, release binaries from `target\release`, Claude Code 2.1.302.

| Command | Exit | Evidence |
|---|---:|---|
| `pwsh -NoProfile -Command` parse of `scripts/acceptance/claude-live-lifecycle.ps1` | 0 | 0 parse errors after each edit. |
| `pwsh -NoProfile -File scripts/acceptance/claude-live-lifecycle.ps1` (run 1) | 1 | Script defect, fixed: `Start-Process` did not quote the prompt, so `lazooctl` rejected the arguments (`unexpected argument 'the'`, 44 ms). Daemon and employee were created; no prompt reached the model. 0 model turns. |
| same (run 2) | 1 | Messaging PASS (7.4 s, `LAZOO_CLAUDE_OK`). Cancel returned `LZ-TURN-UNCERTAIN` in 44 ms. 2 model turns. |
| same (run 3, with the diagnostic capture added) | 1 | Messaging PASS (9.1 s, `LAZOO_CLAUDE_OK`). Cancel returned `LZ-TURN-UNCERTAIN` in 34 ms; stored failure `LZ-TURN-NO-ACCEPTANCE-EVIDENCE`; session `idle`. 2 model turns. Total 11.5 s. |
| same (run 4, after the adapter fix and a release rebuild) | 1 | Messaging PASS; cancellation PASS (the turn ended `cancelled`); resume gate FAILED_ASSERTION on a script error (`.Count` on an unwrapped process list under strict mode) before turn 3 was sent. 2 model turns. |
| same (run 5, after the one-line script fix) | 0 | PASS on all five gates in 20.5 s; 3 model turns; `PELICAN` returned on the resumed native session. |

Outputs recorded (no secrets):

| Turn | Prompt | Trimmed output | Duration |
|---|---|---|---|
| 1 messaging | `Remember the code word PELICAN for later in this conversation. Reply with exactly LAZOO_CLAUDE_OK and nothing else.` | `LAZOO_CLAUDE_OK` | 7.4 s / 9.1 s |
| 2 cancellation | `Count slowly from 1 to 400, one number per line, with no other text.` | none; the turn ended `uncertain`, not `cancelled` | cancel call 34 to 55 ms |
| 3 resume | `Reply with exactly the code word I asked you to remember, and nothing else.` | not run | not run |

## Boundary evidence

- Live model turns actually sent: 4 across the three runs (run 1: 0, run 2: 2, run 3: 2), under the cap of 9 total and 3 per run. The manifest's `liveTurnsSent` counts submissions attempted and reads 2 for the last run.
- Authentication channel: native subscription. The probe reported native `ready`; the daemon's environment carried no API credential or cloud selector.
- No user configuration changed. `%USERPROFILE%\.claude` and every other provider configuration were neither read nor written by the script; only the daemon child process environment was altered. Data lived in a temporary `lazoo-claude-live-<id>` root that the script removes.
- Files touched by this slice: `scripts/acceptance/claude-live-lifecycle.ps1`, this record, and `artifacts/acceptance/claude-live-lifecycle-manifest.json` (it holds run 3). No Rust, no other script or document, nothing committed. The uncommitted edits to `apps/lazood/tests/daemon.rs`, `scripts/gate.ps1` and `artifacts/acceptance/provider-adapters-manifest.json` seen in `git status` are not mine.
- Gate exclusion the integrator must make in `scripts/gate.ps1`: change `$excluded = @('provider-adapters', 'desktop-shell')` to `$excluded = @('provider-adapters', 'claude-live-lifecycle', 'desktop-shell')`, and update the comment above it to name `claude-live-lifecycle.ps1` as a live script. The daemon contract test `gate_runner_runs_the_whole_gate_and_excludes_only_the_live_and_ui_scripts` in `apps/lazood/tests/daemon.rs` pins that exact line (`"$excluded = @('provider-adapters', 'claude-live-lifecycle', 'desktop-shell')"`). When I last read the working tree, both files already carried that edit, uncommitted and made by someone else, so the integrator only needs to keep them together.

## Retained boundaries

- Permissions are `NOT_APPLICABLE_FULL_AUTONOMY_ONLY`: Claude Code receives turns only under `full-autonomy`; `ask` and `self-decision` are refused by design, so no permission prompt is exercised live.
- Forking a live session is a PROJECT-BRIEF section 27 decision and is not attempted.
- Resume on wake with a real `claude -p --resume` and planned-shutdown cleanup of a real Claude native tree remain UNVERIFIED live: the run stopped at the cancellation gate. After the cancel defect is fixed, run the script once; to prove resume alone sooner, the script's order cannot be changed without altering the lifecycle it claims.
- The idle/`stopped` state of a session whose host `cancel()` killed is unobserved; the Claude adapter's `cancel()` sets `stopped` and kills the host, and the session record stayed `idle` in the daemon.
- The likely-cause analysis above is from reading the code and from the stored failure code; it is not confirmed by a patched run.
