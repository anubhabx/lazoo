# Desktop UI pilot: verification record

**Date:** 2026-10-31

**Branch:** `feat/desktop-ui-pilot`, cut from `origin/main` at `87b63e8` (pull request #39, backend UI readiness). Design: `docs/superpowers/specs/2026-10-31-desktop-ui-pilot-design.md`.

## What landed

The previous read-only desktop shell was discarded. The desktop now binds the backend contract the readiness handoff describes:

| Layer | What | Where |
|---|---|---|
| Tauri commands | `daemon_call` (Rust allowlist: 42 reads, 25 mutations; `system.shutdown`, `terminal.open`, `provider.config.apply`, `memory.cleanup.*`, `message.*`, employee-only task verbs refused), `events_subscribe` / `events_unsubscribe` (durable replay then live, as a channel), `terminal_stream_start` / `terminal_stream_stop` / `terminal_stream_focus` (authenticated byte-pipe poller: immediate re-read while output flows, 16 ms to 200 ms backoff when focused, 1 s when hidden), `terminal_write` (≤ 16 KiB, never retried) | `apps/lazoo-desktop/src/{lib,client,events,terminal}.rs`, `build.rs`, `capabilities/main.json` |
| Store | one store with the read models, their `eventHighWater` marks, one subscription from the lowest mark, per-model 250 ms debounced re-reads keyed by event family, reconnect with backoff and a full re-read, actions sent exactly as described with one idempotency key per logical operation kept in `localStorage` until the outcome is known | `apps/lazoo-desktop/ui/src/lib/store.ts` |
| Surfaces | titlebar with space switcher and command palette trigger; rail with New work, grouped history and markers, Inbox badge, Settings; launcher; conversation with agent tabs (lead first, workers as they appear), header actions as data, Native and Transcript pane modes, attention strip (Inbox items and native trust requests), composer; Inbox; Settings (General, Employees, Harnesses, Projects, Memory, Resources, Daemon); command palette; details panel | `apps/lazoo-desktop/ui/src/{App.tsx,components,pages}` |
| Terminal | xterm.js 6 with the WebGL renderer, one instance per opened agent kept across tab swaps, `session.terminal.view` opened automatically when the daemon reports `viewable`, attach offered when `attachable`, resize debounced through `terminal.resize`, keys forwarded until the daemon refuses them, exit and stream errors shown in the frame note, leaving a conversation closes its views | `apps/lazoo-desktop/ui/src/lib/terminals.ts`, `components/TerminalPane.tsx` |
| Transcript | blocks from `workspace.conversation.turns` with `turn.show` per turn (text, folded thoughts, tool, terminal, state and error rows), older pages through `beforeTurnSequence`, lead and worker filters by task | `lib/transcript.ts`, `components/TranscriptPane.tsx` |
| Motion | one easing (`cubic-bezier(0.22, 1, 0.36, 1)`), 150 / 220 / 300 ms, opacity and transform only, `AnimatePresence` with layout on every list, `MotionConfig reducedMotion="user"` | `styles/app.css`, components |

Deliberate deviation from the design system: body type is 13 px / 18 px (metadata 12, micro 11) rather than 14 / 20, following the owner's stated preference for smaller base type; the token file is unchanged.

## Automated verification

| Check | Result |
|---|---|
| `npm run check` in `apps/lazoo-desktop/ui` (prettier, `tsc -b`, vitest, production build) | PASS: 26 tests in 3 files (store, transcript, app with axe on launcher, conversation, inbox); bundle 911 kB JS / 37 kB CSS, fonts bundled, no remote assets |
| `cargo test -p lazoo-desktop` | PASS: 23 unit tests (allowlist refusals, deadlines, frame shapes, base64, response parsing) and 6 configuration tests |
| `cargo clippy -p lazoo-desktop --all-targets -- -D warnings`, `cargo fmt --check` | PASS (reported by the Rust implementer on the final sources) |
| `scripts/acceptance/desktop-shell.ps1 -Configuration debug -SkipBuild` | See the "Acceptance run" section below |

The full `scripts/gate.ps1` (locked release build, every acceptance script) was not run in this session: the host had 2.5 GB of free memory and a release build of the workspace needs about 6 GB. The debug binaries were built with `CARGO_BUILD_JOBS=2`.

## Live pilot on the owner's machine

Disposable data root `%TEMP%\lazoo-ui-pilot-e7acb5d6\data`, debug `lazoo-keeper` → `lazood` (schema 20), a temporary Git repository registered as project "UI pilot", one employee `mira` on Codex (`codex-cli 0.165.0-alpha.1.2`, app-server/v2, verification `configured`), the desktop driven through UI Automation (`scratchpad/desktop-drive.ps1`), screenshots kept beside this session's scratchpad.

| Step | Observed |
|---|---|
| Open the desktop with no employee | Launcher shows "No employees yet"; creating `mira` with `lazooctl employee create` made her appear as a lead candidate without a reload (event `employee.created` → launcher re-read) |
| Pick the lead, type "Reply with exactly LAZOO_UI_OK and nothing else.", Enter | `conversation.create` then `turn.submit { activateIfNeeded }`; the rail row appeared with the derived title; session `70f0fc47` started, the turn completed in about 9 s (`turn.completed`, `memory.archived`); the row's marker became completed and was marked seen |
| Native pane | `session.terminal.status` reported `viewable`; the pane opened `session.terminal.view` with its measured 138×28 and the genuine Codex TUI rendered through the byte pipe and xterm (Codex's own "Hooks need review" prompt was the first screen) |
| Transcript pane | The user block, the assistant text `LAZOO_UI_OK`, and the turn's events (`turnStarted`, `sessionStarted`, `progress`, `usage`, `turnCompleted`) with times |
| Escape sent into the view | Codex skipped the hook review, then printed `thread/resume failed during TUI bootstrap: thread/resume transport error: remote app-server worker channel is closed` and exited 1; the pane showed "View closed · exit 1" with Reopen |
| Reopen | The daemon replayed the stored idempotency key and returned the exited terminal; fixed in this branch: a closed or exited view forgets its key, and an open that returns a dead terminal retries once with a fresh key |
| Second request from the launcher | Refused before acceptance: `LZ-TURN-PROVIDER: provider rejected the turn before acceptance`; the composer showed the code, the header offered Cancel the turn, Retry the turn, Stop the session, the attention strip showed the escalated incident "Employee process exited unexpectedly" with Acknowledge and Resolve, the rail marker turned needs-attention, the Inbox badge read 1, the status bar read "1 needs you"; all within one debounce after the events |
| Stop the session (two-step confirm) | `session.stop` → `session.suspended`; the lifecycle word became suspended, Retry the turn left the header (the lead is no longer idle), the pane offered "Attach and take over" with the daemon's reason |
| Follow-up from the composer | Woke a fresh session `1b59e1d9` (`session.resume.skipped`, as documented for Codex), the turn completed, the native view opened again and rendered the hooks prompt; Escape within eight seconds reproduced the same bootstrap failure (exit 1), the pane reported the stream error and, after this branch's fix, resolves the exit code from `terminal.show` |

Not exercised live in this first run: worker tabs, the attention strip, native trust, attach. The second and third runs below cover the first three.

## Second live run: Claude Code lead (trust strip, start failure)

Fresh data root, two Claude Code employees (`mira`, `rune`, both `full-autonomy`), debug binaries.

| Step | Observed |
|---|---|
| Start from the launcher | `session.starting`, then `native-trust.requested`; the daemon's trust event family is spelled `native-trust.*`, which the desktop's event map did not match, so the first build showed no strip until relaunch (fixed: both spellings re-read the trust list and the conversation) |
| Trust strip | "Claude Code asks to trust `\?\C:\…\sessions\<id>` · Lazoo-managed workspace" with the auto-accept choice, Approve, Decline; the long path widened the conversation grid and pushed the pane toolbar off-screen (fixed: the grid column is `minmax(0, 1fr)` and the path truncates) |
| Approve (about three minutes after the request) | `native-trust.decided`, then `session.failed` `LZ-SESSION-START-FAILED` "provider session did not become ready"; lifecycle `failed`, Cancel the turn offered, `incident.raised` |
| Follow-up from the composer, approve within seconds with auto-accept on | Same failure on the fresh session (`893978ad`), composer shows `LZ-TURN-ACTIVATION-FAILED: the recipient session could not be activated`, `incident.recurred` |

Claude Code 2.1.314 did not reach readiness on this host under the debug daemon in either attempt. The backend verified 2.1.312 live on 2026-10-17. The desktop reported every step with the daemon's words; nothing was simulated.

## Third live run: Codex lead delegates to a Codex worker (the orchestration loop)

Fresh data root, two Codex employees (`mira` lead, `rune` worker, both `ask`), debug binaries, Home space. Request: use `task_delegate` to delegate "reply with exactly WORKER_OK and nothing else" to rune, then reply `DELEGATED`.

| Step | Observed |
|---|---|
| Lead turn | Session `13ea2360` started; the native view opened on its own and rendered the genuine Codex TUI (left on its hooks prompt, untouched); the lead called `task_delegate` (`task.delegated`), its turn completed |
| Worker appears | The `rune` tab appeared beside `mira · main` without any reload, with the needs-approval marker; the attention strip showed "rune · Task proposed: reply with exactly WORKER_OK and nothing else. · Accept or cancel the task" with Accept and Cancel; Inbox badge 1; status bar "1 worker · 1 needs you" |
| Accept from the strip | `task.accept` as data with its revision and a fresh key; the daemon dispatched rune (session `4d4366b9`), the worker's turn completed (`WORKER_OK`), the task completed, the worker session was suspended, and the lead received the result notice as a further turn (`turn.completed` at 98) |
| Worker tab | Marker complete (green); Native mode states "No live session · rune finished this task; its turns are in the transcript" (copy added after this run; the first build said the worker had not started); Transcript shows the dispatch turn authored by mira, the reply `WORKER_OK`, and the turn's events |
| Lead tab | Native view still live and streaming (terminal `ad59737c`, 138×30); Transcript shows both lead turns |

This is the owner's required shape, proven live: the main thread is there from the start, a worker appears as it starts working, each has its own pane, and finished workers stay with their transcript.

## Resource measurement

Sampled with `scratchpad/measure-desktop.ps1` (the desktop process tree: `lazoo-desktop.exe` plus its WebView2 processes, 8 processes, 12 logical cores, debug desktop binary, 1240×800 window) over 30 s windows during the third run:

| State | CPU of machine | Private working set | Summed working set |
|---|---|---|---|
| Conversation open, lead native view live and focused (poller at 16 to 200 ms), worker tab complete | 0.047 % | 313 MiB | 463 MiB |
| Launcher idle, views closed | 0.116 % (includes the page transition at the start of the window) | 216 MiB | 461 MiB |

Against the brief's budget as the backend measured it on 2026-10-17 (private working set basis: 350 MiB idle, 750 MiB routing): both PASS. The private working set is higher than the old read-only shell's 127 MiB because the WebGL terminal renderer and the larger bundle live in the WebView2 GPU and renderer processes. `scripts/acceptance/desktop-budget.ps1 -SkipBuild` on the release binaries: **PASS** on every gate (desktop connected, WebView2 attributed, no provider process, idle CPU under 1 %, idle memory under 350 MiB, routing CPU under 5 %, active memory under 750 MiB, planned shutdown, cleanup). Idle window: mean CPU share 0.08 %, private working set 91.3 MiB; routing window: mean CPU share 0.19 %, private working set 89.6 MiB (60 s windows; the script's launcher-text wait was updated from the old shell's heading to the new launcher's). The script's own window is the launcher without a conversation; the terminal-streaming numbers are the hand measurement above.

## Findings for the backend owner

1. **Native view bootstrap on this machine.** The view launches `codex resume <thread> --remote ws://127.0.0.1:<port> --remote-auth-token-env LAZOO_NATIVE_VIEW_TOKEN --no-alt-screen`. With the owner's Codex configuration the TUI first shows "Hooks need review" (one hook new or changed). After the prompt is dismissed (skip, or presumably any choice) the TUI's `thread/resume` bootstrap fails with "remote app-server worker channel is closed" and exits 1. It reproduced on two sessions, after 90 s and after 8 s. After the first failure `session.terminal.status` reported `LZ-SESSION-VIEW-UNSUPPORTED` ("the provider does not support a native read-only view") for the same still-idle session, and the next turn on it was rejected before acceptance with an `LZ-PROCESS-EXIT` incident, so the failure appears to take the controller's app-server worker with it. The second session also lost its view bridge after its failed view. Suggested checks: whether the bridge's bootstrap window or its `thread/resume` handling tolerates the hooks prompt (run the view TUI with hooks already trusted, or with a view-mode flag that skips the review), and whether a viewer failure can close the controller's channel. The prompt is raised by hooks in the owner's own Codex configuration, not by Lazoo; a view that is left on the prompt keeps streaming indefinitely (third run).
2. **No audit event for terminal open or close.** The desktop re-reads terminal status on conversation refreshes and, while a visible pane has no stream, every 5 s. A `terminal.*` event family would let it react immediately.
3. **Claude Code 2.1.314 does not reach readiness** on this host under the debug daemon (`LZ-SESSION-START-FAILED`, "provider session did not become ready", twice, trust approved within seconds the second time). The 2026-10-17 live gates used 2.1.312.
4. **Trust events are spelled `native-trust.*`** while the RPC family is `native.trust.*`; the contract index lists neither for the desktop's re-read table.
5. **Schema is 20 on `main`.** The contract index still says 15 to 19 in places; the desktop reads whatever the daemon reports.

## Desktop changes made from the live run

- Exited or closed views forget their idempotency key; an open that reconciles to a dead terminal retries once with a fresh key.
- Stream errors are shown in the frame note and the exit code is recovered from `terminal.show`.
- A visible pane without a stream polls terminal status every 5 s.
- The open conversation is marked seen whenever its row refreshes, so a result that lands while it is open does not raise the unread marker.
- With exactly one lead and nothing remembered, the launcher preselects it.
- Status bar facts render as single text nodes so UI Automation sees "schema 20" whole.
- `native-trust.*` events re-read the trust list and the open conversation.
- A long trust path no longer widens the conversation grid; the pane toolbar stays in view.
- A finished worker's Native pane says so and offers the transcript instead of claiming it never started.

## Acceptance run

`scripts/acceptance/desktop-shell.ps1 -Configuration debug -SkipBuild` (rewritten for this UI; it drives the real window through UI Automation against the debug binaries): **PASS** in 13.3 s, manifest `artifacts/acceptance/desktop-shell-manifest.json`. Gates: runtimeAssets, releaseBuild (binary presence), cleanupIsolation, daemonReady, desktopWindow, customTitlebar, launcher, connection, spaceSwitch, commandPaletteKeyboard, inboxPage, settingsProjects, defaultScreenshot (1180×760), narrowScreenshot (860×620), processIsolation all PASS; highDpiEvidence BLOCKED because Windows text scaling is 100 % (unchanged rule). The first run failed on the space switcher, whose `aria-haspopup` button exposes ExpandCollapsePattern rather than InvokePattern; the helper now tries Invoke, SelectionItem, ExpandCollapse, and Toggle. The release-profile run (`-SkipBuild`, the gate's default binaries from `target
elease`, built here with `CARGO_BUILD_JOBS=2`) also **PASSED** in 14.2 s with the same gates; that manifest is the committed one.

## What the owner should look at first

1. Run it: `npm --prefix apps\lazoo-desktop\ui ci && npm --prefix apps\lazoo-desktop\ui run build`, `cargo build -p lazoo-desktop -p lazood -p lazoo-keeper -p lazooctl -p lazoo-session-host`, start `lazoo-keeper.exe`, then `lazoo-desktop.exe` (same `LAZOO_DATA_ROOT`). Create an employee in Settings → Employees or with `lazooctl employee create <name> --provider codex`.
2. The design direction in the spec: whether the agent tab strip, the single focus pane, and the Transcript mode match what you wanted; the 13 px body type.
3. Finding 1 above, which decides whether Native mode is usable on your machine today; Transcript mode is unaffected.
