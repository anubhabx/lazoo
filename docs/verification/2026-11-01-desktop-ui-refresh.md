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

## Not run in this session

The host disk sat between 0 and 1.4 GB free while another worktree built, so the Tauri desktop was not rebuilt and the real-window acceptance and `cargo test -p lazoo-desktop` were not run. A failed link at zero free space would also risk the shared build cache. With room on the disk, run:

```powershell
npm --prefix apps/lazoo-desktop/ui run build
cargo test -p lazoo-desktop
cargo build -p lazoo-desktop
pwsh -File scripts/acceptance/desktop-shell.ps1 -Configuration debug -SkipBuild
```
