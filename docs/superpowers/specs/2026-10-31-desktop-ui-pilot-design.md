# Desktop UI pilot: design direction and architecture

**Date:** 2026-10-31

**Status:** Implemented on `feat/desktop-ui-pilot` against the backend readiness checkpoint (`origin/main` at `87b63e8`, pull request #39). The previous read-only shell is discarded; this document replaces its UI decisions. The information architecture (`2026-08-30-lazoo-desktop-information-architecture-design.md`) and design system (`2026-08-29-lazoo-desktop-design-system.md`) remain the vocabulary and token source; where this document is more specific, it wins for the pilot.

## 1. Moodboard: what is borrowed, and from where

A designer steals. These are the exact parts taken and the reason each was taken.

| Source | Taken | Why |
|---|---|---|
| Linear | 13px Inter body, three greys of text, hairline borders at low alpha, 32px rows, a 208px navigation rail with grouped history, one accent used for selection and primary action only, keyboard-first with `Ctrl+K`, a right-hand properties panel that is closed by default | The owner named Linear; its restraint is the brief's "productivity over flashiness" |
| Zed | Density, a 32px tab strip for the agents, a 28px status bar of monospace facts, panels divided by one-pixel rules and background roles rather than shadows | Developer-native chrome that stays out of the terminal's way |
| Raycast | Near-black canvas, the command surface as the one floating overlay, right-aligned shortcut hints | The command surface teaches the keyboard model without clutter |
| Ghostty, iTerm2 | The terminal pane is only the terminal: a one-pixel frame, 10px padding, no title gradients, the native TUI untouched | The brief forbids redrawing a harness |
| Warp | Blocks: each turn is one block with a left rule, tool rows nested below the text | This is the Transcript mode's shape (section 5) |
| GitHub timeline | Small icon per event on a vertical rule, time at the right edge | Event rows inside a transcript block |
| Vercel | Eight-pixel status dots always paired with a word, tabular numerals in meta lines | Status is never colour alone |
| Superhuman | One accent means "needs you"; everything else is grey | Attention is pulled, not pushed |

Not borrowed: metric-card dashboards, gradients, glass, spring or bounce motion, pill rows, pointer cursors on buttons, remote fonts.

## 2. Layout

```
titlebar 40px   [■ lazoo] [space ▾]            [search or run a command  ctrl k]        [daemon ●] [– ▢ ×]
──────────────────────────────────────────────────────────────────────────────────────────────────────
rail 208px     │ stage                                                                  │ details 320px
 New work   +  │ agent tabs 32px: [main · mira ●] [rune ●] [ash ▲] [kit ✓]      [native|transcript] [i]
 ─ Today       │ header 36px: mira · lead · codex · gpt-5.4 · ask · busy         [cancel turn] [rename]
 ● fix flaky…  │ ┌──────────────────────────────────────────────────────────────┐ (closed by default,
 ○ retry bud…  │ │                                                              │  ctrl+i opens)
 ─ 7 days      │ │          focus pane: xterm (native) or transcript            │
   doc pass    │ │                                                              │
 ─ Earlier     │ └──────────────────────────────────────────────────────────────┘
               │ attention strip: ▲ ash wants to run cargo test … [approve once] [decline]
 Inbox      2  │ composer: › what should mira do next?              shift+enter newline · enter send
 Settings      │
──────────────────────────────────────────────────────────────────────────────────────────────────────
status 28px     home · 3 workers · 1 needs you                     connected · schema 19 · 127 MiB
```

- **Space switcher** (titlebar): Home plus every registered project (`project.list`). The rail shows that space's conversations (`workspace.conversations`), grouped Today, Previous 7 days, Earlier, markers per IA section 6.3 (needs-attention in ember, active with a slow 2.4 s opacity pulse, failed, completed-unread kept locally against `lastResultAt`).
- **Stage** shows one of: the launcher (`workspace.launcher`), a conversation (`workspace.conversation`), the Inbox, or Settings. The rail and status bar persist.
- **Agent tabs**: the lead is the first tab and exists from the moment the conversation does; one tab per worker card appears when the card first exists (`workspace.conversation.workers`, oldest first) and stays after completion, muted. Selecting a tab swaps the focus pane; the swap is a 150 ms crossfade, never a layout change. A tab shows name, provider, a state dot and the state word in its tooltip and accessible name.
- **Focus pane** has two modes, a per-conversation toggle with a global default in Settings → General:
  - **Native**: the genuine harness TUI through `session.terminal.view` (active, viewable sessions) rendered by xterm.js with the WebGL renderer. Labelled read-only, because every provider presents that boundary differently (handoff: "Native terminal boundary"). When the session is suspended and attachable, the pane offers "Attach (takes over the session)" through `session.terminal.attach` and then accepts keyboard input. When neither is possible, the pane states the daemon's `viewBlockingReason` and offers Transcript.
  - **Transcript**: Lazoo's own rendering of the durable turn history (`workspace.conversation.turns`, newest 50, older pages by `beforeTurnSequence`, each turn's events through `turn.show` on demand): user requests, assistant text, thoughts folded, tool rows, state and failure codes. Works for every provider, for finished and suspended sessions, and costs no process. This is the "second UI mode" the owner invited.
- **Attention strip**: the conversation's own `attention` Inbox items, each with its actions as data. It sits outside the terminal frame, in UI type, so it never reads as provider output (IA section 8.4).
- **Composer**: visible on the lead tab only; Enter sends `turn.submit { activateIfNeeded: true }` to the lead with the root task's authority, Shift+Enter inserts a newline, a send in flight disables the button (no double submit). Worker tabs show the worker's actions in the header instead.
- **Details panel** (`Ctrl+I`): session id, native session id, working directory, workspace path and branch, authority, timings, failure code and message, `session.resources`. Everything that is true but not urgent lives here.
- **Inbox**: filters All, Approvals, Permissions (empty until passthrough exists), Incidents, Resolved; rows with title, required action, scope, deadline, actions; selecting a row's scope opens the conversation and tab it names.
- **Settings**: General (default pane mode, reduced motion follows the OS), Employees (`employee.list`, create and disable), Harnesses (`provider.list`, `roster.get`, native trust policies), Projects (`project.list`, register by path), Memory (`memory.policy.show`, `memory.queue.status`, `memory.worker.show`), Resources (`system.resources`, `system.budget`), Daemon (`system.status`). Settings is a page, never a modal.
- **Command surface** (`Ctrl+K`, 720×560 max): Go to (Home, each project, Inbox, Settings), Conversations (the space's history), Terminals (focus an agent tab, toggle Native/Transcript), Actions (New work, Cancel turn, Rename). Destructive actions require confirmation and are never one keystroke.
- **Native trust**: a pending `native.trust.list` request for the current conversation's lead or worker renders in the attention strip with Approve, Decline, and an "Automatically accept next time for this provider's managed workspaces" choice; it resolves through `native.trust.resolve` with revision and idempotency key.

## 3. Motion

One easing for everything: `cubic-bezier(0.22, 1, 0.36, 1)` (ease-out-quint). Durations from the design system: 150 ms for hover, press, selection, tab swap; 220 ms for panels, the command surface, rows appearing and leaving; 300 ms only for the mode switch between Native and Transcript. Only opacity and transform animate. Lists (rail rows, agent tabs, attention items, Inbox rows) use `AnimatePresence` with `popLayout` and `layout` so a row that appears or leaves eases the rows around it; nothing jumps. A `MotionConfig reducedMotion="user"` honours the OS setting. The running marker is the only perpetual motion: a 2.4 s opacity pulse between 0.35 and 1, removed under reduced motion.

## 4. Reads, events, actions

- Every read goes through one Tauri command, `daemon_call(method, params)`, which refuses any method outside an allowlist in Rust (section 6). No executable name, nonce, or pipe credential ever reaches the webview.
- The frontend keeps a single store: projects, the selected space, that space's conversations, the open conversation, the launcher, Inbox counts and items, pending native trust, terminal status per focused session, daemon status. Each model remembers its `eventHighWater`.
- One `events_subscribe` channel from the lowest mark the store holds. Event kinds map to the models to re-read (contract index section 2), debounced to one read per model per 250 ms. On `closed`, the store marks the connection uncertain, keeps the last models, and resubscribes from its last consumed sequence with backoff (500 ms doubling to 8 s).
- Actions are sent exactly as the daemon described them (`InboxAction` and `WorkspaceStartAction`): the frontend adds a fresh `idempotencyKey` (and `title` for rename), keeps the key and payload in `localStorage` until the outcome is known, and reuses the same key on a lost reply. A stale revision re-reads the model and drops the action.
- Start = `conversation.create`, then `turn.submit { activateIfNeeded: true }`, two keys, the second reused if the first reply is lost. A refusal with `data.turnId` re-reads the conversation and shows the daemon's code and message beside the composer.

## 5. Terminal rendering budget

- At most one xterm instance per open agent tab, created on first focus, kept mounted but hidden when another tab is focused, disposed when the tab leaves or the conversation closes. Views are opened with the pane's measured columns and rows and resized through `terminal.resize` on a 150 ms trailing debounce.
- Bytes arrive through `terminal_stream_start`: the Rust poller reads the authenticated byte pipe, polls again immediately while output flows, and backs off 16 ms → 200 ms when the pane is focused and → 1 s when hidden (`terminal_stream_focus`). A `gap` is shown as a one-line notice inside the pane, never hidden.
- Input goes through `terminal_write` only when the terminal is an attachment (a view's keystrokes are refused by the daemon; the pane does not pretend otherwise). Resize is always available.
- WebGL context loss falls back to the DOM renderer for that instance. Scrollback is 5,000 lines.

## 6. Tauri boundary

Commands: `daemon_call`, `events_subscribe`, `events_unsubscribe`, `terminal_stream_start`, `terminal_stream_stop`, `terminal_stream_focus`, `terminal_write`. The allowlist names the read models and the actions the contract index offers as data; `system.shutdown`, `terminal.open`, `provider.config.apply`, `memory.cleanup.apply`, `message.*`, and the employee-only task verbs are refused in Rust. The capability file lists exactly these commands plus the core window controls.

## 7. Verification

- `npm run check` (format, types, vitest with axe, production build) for the frontend.
- `cargo test -p lazoo-desktop` for the allowlist, frame shapes, response parsing, and the static configuration contract.
- `scripts/acceptance/desktop-shell.ps1` rewritten for this UI: the window title, the titlebar controls, the rail, the registered project in the space switcher, the launcher, the command surface keyboard flow, the Inbox and Settings pages, screenshots at 1180×760 and 860×620.
- A live run on the owner's machine with the built daemon: register a project, create an employee, start a conversation, watch the agent tab appear, open the native view, switch to Transcript, approve an item. Recorded in `docs/verification/2026-10-31-desktop-ui-pilot.md`.
