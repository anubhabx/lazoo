# Desktop UI refresh: verification record

**Date:** 2026-11-01

**Branch:** `feat/desktop-ui-refresh`, stacked on `feat/desktop-ui-pilot` (pull request #40) at `fc4ceed`. Design: `docs/superpowers/specs/2026-11-01-desktop-ui-refresh-design.md`.

## What landed

| Area | What | Where |
|---|---|---|
| Tokens | Slate primitives lifted off black, one iris accent, soft status and harness hues, depth and motion tokens; ember, graphite, and the unused density variants removed; `tokens.json` generated from the CSS | `design-system/tokens.css`, `design-system/build-tokens.mjs`, `design-system/tokens.json`, `design-system/README.md` |
| Base components | Button (variants, sizes, press, loading, tooltip), Spinner, Kbd, Dot, StatusWord, Badge, Avatar, ProjectGlyph, Field, Segmented, Switch, Skeleton, Empty, Dialog (focus kept inside, Escape and scrim close, focus restored), motion presets; every state in one stylesheet | `ui/src/components/primitives.tsx`, `ui/src/styles/components.css` |
| Shell | Full-height sidebar with New work, Search, Inbox, Projects (expandable, newest five conversations, Show all, add-project dialog), Recents, Settings and the connection; collapsible with `Ctrl+B`; titlebar with back, forward, and a breadcrumb whose title renames the conversation; status bar removed | `ui/src/App.tsx`, `components/Rail.tsx`, `components/Titlebar.tsx`, `styles/app.css` |
| Launcher | Greeting or project eyebrow, a larger prompt with lead and authority pills, starter suggestions, the team as selectable cards with skeletons while loading, an inviting empty team that opens setup, recent conversations in a project | `pages/Launcher.tsx` |
| First run | Six shipped presets resolved against `provider.list`; a setup dialog offered once, editable per agent, creating each through idempotent `employee.create` and selecting the new lead | `lib/presets.ts`, `components/SetupDialog.tsx`, `lib/store.ts` |
| Conversation | Avatars in the agent tabs with a sliding pill, an identity line with a "working" shimmer and worker count, skeleton loading, a rounded composer with the authority pill and a round send | `pages/Conversation.tsx` |
| Inbox, Settings, palette, toasts | Sliding filter and nav selections with counts, avatars in Inbox rows, skeletons, empty state with an icon, framed tables, switches for trust policy, a recommended-team callout, palette commands "Set up agents" and "Hide sidebar", spring toasts with icons | `pages/Inbox.tsx`, `pages/Settings.tsx`, `components/CommandPalette.tsx`, `components/Toasts.tsx` |
| Terminal palette | xterm colours mirror the slate tokens | `lib/terminals.ts` |
| Acceptance | `desktop-shell.ps1` dismisses the setup dialog when offered, restores a collapsed sidebar, and selects the project from the sidebar (`projectSidebar` replaces `spaceSwitch`); the Rust contract test follows | `scripts/acceptance/desktop-shell.ps1`, `apps/lazoo-desktop/tests/config.rs` |

No backend, protocol, or Rust source changed; `employee.create`, `employee.list`, `provider.list`, and `project.register` were already allowlisted.

## Automated verification

| Check | Result |
|---|---|
| `npm run check` in `apps/lazoo-desktop/ui` (prettier, `tsc -b`, vitest, production build) | PASS: 38 tests in 4 files; bundle 947 kB JS / 62 kB CSS, fonts bundled, no remote assets |
| New tests | `presets.test.ts` (6: nothing detected, the Claude Code and Codex split, single-harness fill, optional roles, taken names, create params); `app.test.tsx` adds the sidebar project flow with axe, `Ctrl+B`, setup offered once and creating four agents as data with axe, and editing and leaving out rows |
| Bugs the new tests found | Expanding a project before switching to it started a read for the old space that the store's in-flight dedupe then returned for the new space; the dialog's focus effect depended on the close callback's identity, re-ran on every render, and stole focus while typing. Both fixed |

## Visual review

The dev mock (`npx vite --port 1431`, `?mock=1`, and `&fresh=1` for a machine with no employees) was driven with agent-browser at 1280×800 in Chromium, the engine WebView2 uses. Reviewed: the setup dialog (detected harness chips, five rows with reasons, the expanded editor), "Your team is ready", the empty-team launcher, the populated launcher with the lead preselected, a project home with the project expanded in the sidebar and its unread and needs-you markers, a conversation in Native and Transcript modes with the attention strip, the command palette, Settings (Employees, Harnesses). One defect found and fixed during review: nested project rows overflowed their grid track and clipped their status dots (`grid-template-columns: minmax(0, 1fr)` on the sidebar sections).

## Rust checks and the real window

The host disk sat between 0 and 1.4 GB free while another worktree built. Headroom came from deleting the main checkout's rustc incremental cache (`target/debug/incremental`, 5.7 GB of compilation cache; compiled crates stay in `deps`). The worktree's `target` is a junction to the main checkout's, so only the four desktop crates recompiled. Afterwards the main checkout's `lazoo-desktop` debug binary and test binaries were rebuilt from its own sources, so the shared cache does not carry this branch's UI.

| Check | Result |
|---|---|
| `cargo test -p lazoo-desktop` | PASS: 23 unit and 6 configuration tests, including the acceptance-script contract with the sidebar project and setup-dialog steps |
| `cargo clippy -p lazoo-desktop --all-targets -- -D warnings`, `cargo fmt --all -- --check` | PASS |
| `cargo build -p lazoo-desktop` (debug, this branch's `ui/dist` embedded) | PASS |
| `scripts/acceptance/desktop-shell.ps1 -Configuration debug -SkipBuild` | **PASS** in 14.6 s, manifest `artifacts/acceptance/desktop-shell-manifest.json`. Gates runtimeAssets, releaseBuild (binary presence), cleanupIsolation, daemonReady, desktopWindow, customTitlebar, setupDialog, launcher, connection, projectSidebar, commandPaletteKeyboard, inboxPage, settingsProjects, defaultScreenshot (1180×760), narrowScreenshot (860×620), processIsolation all PASS; highDpiEvidence BLOCKED because Windows text scaling is 100 % (unchanged rule) |

Three runs failed before the pass; each was the script, not the app. The first run showed the real dialog being offered on a fresh WebView profile and dismissed through "Not now". Its absence check then waited on "Set up your team", which the launcher's empty-team button also carries; the check now waits for "Not now" to disappear. The second run took the dismiss branch with no dialog open for the same reason; the dialog is now recognised by "Not now" and the check waits for the connection first, because the store decides whether to offer the dialog before it reports the connection. The third run looked for the removed status bar's "schema 20"; it now reads the Daemon page's "Schema" fact. The passing run took the "already offered" branch because the first run had offered the dialog in this profile.

The real window's screenshots (Settings, Daemon at both sizes with "Desktop acceptance" expanded in the sidebar and "Start the first conversation" under it; the command palette with "Set up agents" and "Hide sidebar") match the browser review: slate surfaces, iris focus ring, the gliding selection.

## Smoothness

The acceptance run checks behaviour, not motion, so frame timing was measured separately. Method: the production bundle with React's production build and the scripted daemon (`vite build --mode mock`, served by `vite preview`, `?mock=1`), driven by agent-browser in Chromium at 1280×800 on the host GPU (ANGLE, RTX 3050, D3D11, 120 Hz). A page script records every `requestAnimationFrame` delta and every long animation frame (≥ 50 ms) with its top script, tagged by step. The pass covers a launcher suggestion, two project switches with sidebar expansion, opening a conversation (lead terminal), Transcript and Native, the palette with arrow keys, `Ctrl+B` twice, Settings (Employees, Harnesses), and back to New work.

| Pass | Frames | p50 | p95 | p99 | Worst | Over 50 ms | Long animation frames |
|---|---|---|---|---|---|---|---|
| Warm (same session, second pass, every step confirmed) | 2,940 | 8.3 ms | 8.5 ms | 8.6 ms | 33 ms | 0 | 0 |
| Cold first run (fresh profile, empty GPU shader cache, includes creating the team), after the fixes below | 3,121 | 8.3 ms | 8.5 ms | 8.7 ms | 192 ms | 5 | 4 |
| Cold first run, before the fixes | 3,122 | 8.3 ms | 8.4 ms | 8.6 ms | 192 ms | 7 | 8 |

What the cold stalls were, from long-animation-frame attribution and a DevTools trace:

- xterm's WebGL addon draws its glyph atlas in one idle task, 190 to 207 ms, the first time any terminal opens in a session. This is CPU work and repeats on every launch. Fixed: `TerminalManager.prewarm()` draws the atlas on a hidden terminal 1.5 s after launch, at idle; terminals with the same options share it, so opening a conversation no longer pays it.
- Creating each terminal's WebGL renderer ran inside the open itself (61 to 86 ms). Fixed: the DOM renderer paints first and WebGL attaches at the next idle period (`whenIdle`), which brought the open's own task to 26 to 30 ms.
- The remaining cold frames have little or no script. The trace puts them on Chrome's GPU thread (raster flushes up to 199 ms) and GPU workers (pipeline work up to 190 ms): first-use shader compilation, which WebView2 keeps in its disk cache between launches. The warm pass shows none of them. Two effects that cost GPU work on every frame were removed: the backdrop blur behind the terminal's status pill, and the gradient mask on the sidebar scroller (re-applied while the selection glides). Blur remains only on the palette and dialogs, which are open briefly.

The dev-mode React numbers taken first (slower by design) are not used for conclusions. A one-minute recording of the first-run setup and the pass above was shared with the owner (`lazoo-interactions.webm`, not committed).

After these changes the frontend check (38 tests), the debug desktop build, and the real-window `desktop-shell.ps1` were run again: PASS in 16.8 s with the same gates.

### In the real desktop

The same sampler ran inside the real Tauri window. WebView2 exposed DevTools through `WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS=--remote-debugging-port=9333` on a debug build of this branch, against a real keeper and daemon on a disposable data root with two registered projects (`orchestrator`, `website`), three employees created through `lazooctl` (atlas on Claude Code, forge and sentinel on Codex), and six conversations. A small Node script drove it over CDP: element clicks for the sidebar rows, panes, and Settings; real key events for `Ctrl+K`, the arrow keys, `Escape`, and `Ctrl+B`.

The pass: open the `orchestrator` project, toggle `website`, open a project conversation, Transcript and Native, the palette with arrows, collapse and restore the sidebar, open a Home conversation, Transcript and Native again, Settings (Employees, Harnesses), Inbox, New work, and the setup dialog opened and closed.

| Pass in the real window | Frames | p50 | p95 | p99 | Worst | Over 33 ms | Long animation frames |
|---|---|---|---|---|---|---|---|
| First pass in this launch | 2,179 | 8.3 ms | 8.5 ms | 8.6 ms | 16.8 ms | 0 | 0 |
| Second pass, every step confirmed | 4,249 | 8.3 ms | 8.5 ms | 8.6 ms | 16.8 ms | 0 | 0 |

These conversations had no live sessions, so no terminal was created; the terminal-open path is covered by the production-bundle measurements above. The stack was stopped and its data root removed afterwards, and the main checkout's debug desktop binary was rebuilt from its own sources.

## Owner sign-off

On 2026-11-01, after the screenshots and the one-minute recording, the owner approved the look and feel as is and kept the shipped agent defaults (Claude Code Opus lead, Codex GPT-6.1 Sol builder, Claude Code Sonnet frontend, Codex GPT-6 Astra read-only reviewer, Antigravity researcher and OpenCode quick fixes when installed).
