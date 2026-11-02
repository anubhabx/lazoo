# Verification: desktop native first, dithered identity, accounts

**Date:** 2026-11-03 · **Branch:** `feat/desktop-ui-native-first` · **Design:** `docs/superpowers/specs/2026-11-03-desktop-native-first-design.md`

## What changed

- **Native first.** The conversation is the harness's own TUI, edge to edge, under a single agent bar (avatar tabs, mono machine facts, one inline action, a menu, the Activity toggle). The Native/Transcript toggle, the "default pane" setting and `TranscriptPane` are removed. Lazoo's own record of each turn (request, reply, finished tool and team-call counts, reasoning noted, errors) is **Activity** in the side panel (Ctrl+I), next to **Details**.
- **Transcript decision.** No harness exposes a complete structured stream (code map in the design note, section 4): Claude Code's interactive mode gives text and tool status only, OpenCode gives text, no harness persists tool arguments or results. Re-rendering is not attempted.
- **Design system v0.3.** Radii 2 to 6 px, flat hairline surfaces (no raised shadows, no glows, no backdrop blur), motion 60 to 200 ms, no animation on keyboard-driven surfaces (palette, page changes, list updates), the stage uses the terminal's own background so the TUI sits flush. Copilot finally has a harness hue.
- **Dithered identity.** `lib/dither.ts` + `components/AgentAvatar.tsx`: each agent is a mirror-symmetric 10x10 glyph seeded from its name (shape) in its harness hue (colour), shaded with a 4x4 Bayer ramp. Working sweeps a diagonal band, thinking shimmers, needs-you lights a dithered accent frame, failed turns rose, dormant dims. One shared 12 fps ticker; off-screen avatars are skipped; reduced motion holds them still. Dithered fields replace gradients on the launcher's top edge, empty terminal states, loading bars, usage meters (the reserve is a dithered block) and sparklines.
- **Hierarchy.** Launcher: title, prompt, one truth line, the team as rows, recent work. No greeting, glow, sparkle icons or suggestion chips. Sidebar: lead avatars on conversation rows (animated while active), shortcuts on hover only. Settings: creation forms move behind a primary button and a dialog (Accounts), General becomes a keyboard reference.
- **Accounts (Settings).** Harness tabs; one routing control (own login, use in order, balance, pin, API pool) with a sentence explaining it; accounts in routing order with move up/down, state (ready, cooling down, exhausted until, key needed, disabled), observed usage windows as meters with the reserve marked, a reserve selector (disabled with the daemon's reason when the harness reports no window), Sign in or Rotate key inline and the rest in a menu; the unmanaged personal login's windows; a usage table (sessions, turns, failed, requests, tokens, a 7-day or 24-hour dithered sparkline) where unreported tokens say "not reported". Verify routing is a dialog that says it spends quota. The agent bar shows which account a session uses.

## Bugs found by driving the real desktop, and fixed

| Symptom | Root cause | Fix |
|---|---|---|
| Claude Code TUI warned "Transcript saving is off — inherited CLAUDE_CODE_CHILD_SESSION marker"; resume after suspension failed ("No conversation found"); a new conversation's first message failed with `LZ-TURN-ACTIVATION-FAILED` | A daemon started from inside Claude Code (a terminal in it, or an agent driving Lazoo) passed the parent session's context to every Claude session | `interactive_session.rs` strips the parent-session variables (`CLAUDECODE`, `CLAUDE_CODE_CHILD_SESSION`, `CLAUDE_CODE_SESSION_ID`, the messaging socket and token, bridge id, entry point, exec path, PID, effort, plugin dirs); user configuration stays. Stripping only the marker was worse (the process never became ready). Unit test `claude_sessions_drop_an_inherited_parent_session_marker`; live: `session warm` reaches idle in 6 s from a clean environment and from an environment with nine inherited variables |
| Accept/Decline of a proposed task and the composer pushed off screen | The conversation grid's implicit column grew to the attention row's nowrap min-content | `grid-template-columns: minmax(0, 1fr)` on the conversation and the attention strip |
| "only daemon-owned idle sessions can be viewed" shown while Claude was still starting | The pane quoted the view-blocking reason for a starting session | A starting session says "Starting atlas… The native view opens as soon as the session is ready." |
| Trust prompt showed `\\?\C:\…` | Verbatim path prefix displayed as is | Stripped for display |
| "Input refused by the provider" stayed forever after one keystroke | Sticky flag | Transient note (4 s) that points to the composer |
| "Stop the session" was the inline action of an idle lead | First action took the slot | Inline slot takes only cancel-turn or non-destructive actions |
| Sidebar scrolled sideways | An invisible tooltip pseudo-element extended the scroll area | `overflow-x: hidden`, tooltips that stay inside the rail |
| Composer promised "your message queues" for a busy lead | The daemon delivers immediately and Claude refuses mid-turn (`LZ-TURN-PROVIDER`) | Honest copy |

## Found and not fixed here (handed to the backend)

1. **Codex worker exits mid-turn after its native view opens.** Run 3: forge (Codex, gpt-6-luna; the TUI reported v0.166.0-alpha.5.1) fixed `calc.py` and ran the tests (rollout `rollout-2026-11-03T01-32-56-…` ends 20:03:15.55 UTC after the last tool output); the process ended at 20:03:18.8 with exit code None, `LZ-PROCESS-EXIT`, task uncertain. The view was opened about 7 s into the turn. Run 1, without opening the view, completed. Suspect: the broker's documented "driver overflow terminates the child" path while the viewer bootstraps history.
2. **A message to a busy lead is rejected, not queued**, and the rejection is recorded as `LZ-PROVIDER-INCOMPATIBLE`, which is misleading.
3. **`lazooctl events list` fails with "IPC I/O failed: early eof"** for pages containing large events (any `--limit` above the default, or the page after sequence 100 in run 3).
4. **A resume that fails during startup fails the activation** instead of falling back to a fresh session (it self-heals on the next message because a failed predecessor is never resumed). Less likely now that transcripts are saved, but Claude's own transcript cleanup can still cause it.
5. An approval left pending past its deadline (about 5 minutes) ends with `LZ-MESSAGE-TIMEOUT` and Lazoo stops the lead's session; by design, but the deadline is short for a person away from the desk.

## Evidence

- `npm run check` (prettier, `tsc -b`, vitest 53 tests with axe on the shell and dialogs, production build): pass.
- New tests: `lib/dither.test.ts` (deterministic, distinct, symmetric, non-empty glyphs; Bayer coverage; state to motion), activity folding (counts, collaboration calls, errors), accounts (priority order and move with the policy revision, reserve preference, unenforceable reserve disabled with reason), destructive worker action through the menu and a confirm dialog, the read-only fact appearing once the view is live (it surfaced a missing terminal subscription).
- `cargo test -p lazoo-providers --lib claude_sessions_drop`, `cargo clippy -p lazoo-providers --lib -- -D warnings`: pass.
- Real window (debug build, disposable data roots, CDP on WebView2): three live runs with a Claude Code lead (claude-sonnet-5-5, notice-turns), a Codex worker (gpt-6-luna) and an OpenCode worker (unverified, so not offered as route-ready). Delegation proposal, accept, worker tab, worker result notice waking the lead, and the lead's reply all worked through the new UI; the failures above were found on the way.
- Not run: the full `scripts/gate.ps1` (release build and acceptance scripts) because the disk sat between 0.5 and 13 GB free while Codex built the account slice in parallel; the owner should run it before merge. `desktop-shell.ps1` relies on names that were kept (New work, Inbox, Settings, Projects, Recents, Open command palette, Search commands, What should the team work on?, Start, Not now, Set up your team, General, Daemon).
