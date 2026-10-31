# Desktop UI refresh: slate design system, projects-first sidebar, shipped agents

**Date:** 2026-11-01

**Status:** Implemented on `feat/desktop-ui-refresh`, stacked on `feat/desktop-ui-pilot` (pull request #40). This document supersedes the pilot design (`2026-10-31-desktop-ui-pilot-design.md`) for colour, type, motion, the rail, the titlebar, and first-run; the pilot's reads, events, actions, terminal budget, and Tauri boundary (its sections 4 to 6) are unchanged.

## 1. What the owner asked for

After using the pilot, the owner kept the minimal layout and asked for:

- a design system that is not harsh: a slate palette that is easy on the eyes, no amber, a 2026 feel rather than 2024 flatness;
- smoothness: base components with real interactions, hierarchy, and reactions, built from one source of truth;
- an app that feels alive and is usable the moment it opens: default agents shipped with the app for whatever harness is detected, offered in a quick setup popup with editable defaults chosen from what the community reports works best;
- projects first in the sidebar, the way the ChatGPT and Claude apps do it, because isolating work per project is the core of a developer tool.

The pilot screenshots showed why: a near-black canvas (`#0b0d10`) with pure-white text, an ember accent on every primary control, flat surfaces with no depth, an empty launcher with one sentence and a radio list, and the space switcher hidden in a titlebar dropdown.

## 2. Direction

| Was | Now | Why |
|---|---|---|
| Graphite near-black, `#f4f5f7` text | Slate lifted off black (`#171c24` sidebar, `#1b2029` stage, `#20262f` cards), `#e8ecf1` text | The GitHub "dark dimmed" lesson: lower contrast between canvas and ink is what makes long sessions comfortable |
| Ember accent for primary, focus, selection, attention | One iris accent (`#8d9af4`) for focus, selection, "needs you" and progress; the primary button is quiet ink on slate | Iris sits in slate's own hue family, so the accent reads as part of the surface rather than a warning; amber is gone everywhere |
| Saturated status colours (`#43d17c`, `#ff5d64`, `#f2c94c`) | Soft hues tuned for slate: sky running, mint done, rose failed, lilac suspended, sand only for blocked | Status must stay legible without glowing |
| Flat surfaces, borders only | A 1 px top highlight plus a low, soft shadow on raised surfaces; overlays get a deeper shadow and backdrop blur | Depth is what separates a 2026 desktop app from a flat 2024 one, used sparingly |
| 4 to 6 px radii | 6 to 10 px for controls and cards, 14 to 18 px for the composer and dialogs | Softer without becoming pill-heavy |
| Static rows and tabs | Selections glide (shared-layout springs), controls press, dots breathe, work shimmers, lists stagger in | Section 6 lists every reaction |
| Inter 13 px, default alternates | Inter 13 px with `cv02 cv03 cv04 cv11` (open digits, single-storey a) and tighter headline tracking | The modern Inter cut, no new font download |

The owner's earlier calls stay: 13 px body, dark only, keyboard first, terminals untouched.

## 3. One source of truth

```
design-system/tokens.css      every value: primitives, then semantic roles (--lz-color-*, --lz-status-*, --lz-provider-*)
design-system/tokens.json     generated from tokens.css by design-system/build-tokens.mjs (never edited by hand)
ui/src/styles/components.css  how each base control looks in every state (hover, press, focus, disabled, loading)
ui/src/components/primitives.tsx  the base components: Button, Spinner, Kbd, Dot, StatusWord, Badge, Avatar,
                              ProjectGlyph, Field, Segmented, Switch, Skeleton, Empty, Dialog, plus motion presets
ui/src/styles/app.css         shell layout and pages only
```

Rules: components read semantic roles, never primitives; pages compose primitives and add layout only; motion presets (`EASE`, `SPRING`, `fade`, `rise`) live beside the components so every surface moves the same way. The xterm palette in `lib/terminals.ts` mirrors the tokens because xterm cannot read CSS variables; the comment there says so.

## 4. Information architecture

```
sidebar 256 px (full height)          │ titlebar 44 px: [⟵ ⟶] [▣ project › conversation title]        [– ▢ ×]
 ▣ lazoo                       [⊟]    │ ─────────────────────────────────────────────────────────────────────
 + New work            Ctrl N         │ stage: launcher (project home) · conversation · Inbox · Settings
 ⌕ Search              Ctrl K         │
 ▤ Inbox                    2         │
 Projects                     +       │
  ▣ lazoo                     ›       │
     fix flaky test        ●          │
     doc pass                         │
     Show all 12                      │
  ▣ website                   ›       │
 Recents                              │
  retry budget             ●          │
 ⚙ Settings            ● Connected    │
```

- **Projects first.** Every registered project is a row with a stable tinted glyph. Selecting it opens the project home (the launcher scoped to the project, with its recent conversations) and expands it in place; the chevron expands or collapses without navigating. An expanded project shows its five newest conversations and "Show all". Expansion is remembered. "+" opens a small dialog that registers a repository, worktree isolation on by default.
- **Recents** holds Home's projectless conversations, as ChatGPT's "Chats" does.
- **The selection glides** between rows across sections (one shared layout id), so moving between a project and a conversation reads as one motion.
- **Titlebar** is only location and history: back and forward (Alt+Left/Right), then a breadcrumb of the space and the conversation. The conversation title is the rename control. The space switcher dropdown is gone; projects are in the sidebar.
- **Status bar removed.** Its facts moved where they are used: connection to the sidebar foot, worker count to the conversation header, schema and version to Settings, Daemon.
- **Ctrl+B** collapses the sidebar (also from the titlebar and the palette); collapsed content is hidden from pointer, keyboard, and assistive technology.

## 5. Usable on first open: shipped agents

`ui/src/lib/presets.ts` ships six roles. Each role lists harnesses in order of fit; the first one `provider.list` reports as detected wins. Core roles appear when any of their harnesses is detected; optional roles appear only when their first-choice harness is detected.

| Role (name) | First choice | Then | Ceiling | Workspace | Activation |
|---|---|---|---|---|---|
| Lead (`atlas`) | Claude Code, `opus`, high | Codex `gpt-6-astra` high, Antigravity, OpenCode | self-decision | own worktree | notice-turns |
| Builder (`forge`) | Codex, `gpt-6.1-sol`, medium | Claude Code `sonnet` medium, OpenCode, Antigravity | self-decision | own worktree | manual |
| Frontend (`pixel`) | Claude Code, `sonnet`, medium | Codex `gpt-6.1-sol` medium, Antigravity, OpenCode | self-decision | own worktree | manual |
| Reviewer (`sentinel`) | Codex, `gpt-6-astra`, high | Claude Code `opus` high, OpenCode | ask | read-only | manual |
| Researcher (`scout`, optional) | Antigravity, harness default (Gemini) | Claude Code `sonnet` low | ask | read-only | manual |
| Quick fixes (`dash`, optional) | OpenCode, its configured model | Claude Code `haiku` low | self-decision | own worktree | manual |

Why these picks (community reports, October 2026; no single source settles it, and the dialog says each row's reason in one line):

- Claude Code with Opus is the most common recommendation for planning, architecture, and long agentic runs, and Claude is most often preferred for UI work ([MindStudio](https://www.mindstudio.ai/blog/claude-code-vs-openai-codex-comparison), [Firecrawl](https://www.firecrawl.dev/blog/claude-code-vs-codex), [BuildMVPFast](https://www.buildmvpfast.com/blog/best-ai-model-developer-task-frontend-backend-coding-2026)).
- Codex's recommended everyday model is GPT-6.1 Sol; GPT-6 Astra is suggested for multi-hour refactors and the hardest tasks; Sonnet is the everyday Claude model ([DDS Hub model guide](https://www.ddshub.cc/en/blog/claude-opus-5-5-vs-gpt-6-1-sol-codex-model-guide)).
- Codex is favoured for mechanical multi-file refactors and unattended work; a frequent pattern is one vendor writing and the other reviewing, and the reviewer's strength matters most ([Composio](https://composio.dev/content/claude-code-vs-openai-codex), [claude-code-cross-review](https://github.com/atompilot/claude-code-cross-review), [Morph](https://www.morphllm.com/comparisons/codex-vs-claude-code)).
- Gemini through Antigravity is cited for very large context and fast reading; its CLI defaults to Gemini 3.5 Flash; Gemini is a common pick for documentation ([sanj.dev](https://sanj.dev/post/antigravity-cli-deep-dive/), [BuildMVPFast](https://www.buildmvpfast.com/blog/best-ai-model-developer-task-frontend-backend-coding-2026)).
- OpenCode is model-agnostic and expects the user's own provider; Lazoo requires its model in `provider/model` form, so the preset leaves it empty and OpenCode uses what the user configured.

Safety defaults: no agent gets a full-autonomy ceiling; the reviewer and researcher are read-only; only the lead uses `notice-turns`, so a dormant lead wakes for its workers' results. Claude Code efforts are limited to what the adapter accepts (`low` to `max`); harnesses without an effort setting send none.

**The setup dialog** (`components/SetupDialog.tsx`) opens by itself once per install, the first time a harness is detected (`localStorage` key `lazoo.setup`), and any time from the launcher's Team section, Settings, Employees, or the palette ("Set up agents"). It shows which harnesses were found, one row per agent (avatar in the harness hue, role badge, harness, model, effort, the one-line reason) with a tick to leave it out and an expandable editor (name, harness, model with suggestions, effort, authority ceiling, workspace). Changing the harness brings that role's recommended model for it. Names an enabled employee already uses start unticked. Creating sends one `employee.create` per agent in order, through the store's idempotent `mutate` (key scope: name and harness), each row turning to a spinner and then a check; the lead becomes the launcher's selected lead. Nothing starts a model: creation runs the daemon's bounded readiness probe only.

## 6. Interactions and reactions

| Element | Reaction |
|---|---|
| Buttons, chips, suggestions, rail rows, team cards | Hover brightens; press scales to 0.97 (`--lz-press-scale`, 1 under reduced motion); loading shows a spinner and blocks double submit |
| Rail, Settings nav, agent tabs, segmented controls, Inbox filters, palette | The selection is a shared-layout pill that springs to the new item (stiffness 520, damping 42: settles without overshoot) |
| Icon buttons | CSS tooltip after 450 ms, with the shortcut |
| Status dots | Running and needs-you dots breathe (a ring expands and fades every 2.2 s); dormant and unknown are hollow |
| Busy agent | The state word shimmers ("working") in the conversation header |
| Launcher | Sections stagger in; the prompt gains an iris halo on focus; "Enter to start" appears once the request is startable; a slow ambient glow drifts behind the heading |
| Loading | Skeletons for the team, the conversation, and the Inbox instead of sentences |
| Toasts, dialogs, palette | Spring in with a slight scale; overlays blur what is behind them |
| Inputs | Focus ring is a soft iris glow, not an outline |

Reduced motion (the Windows "Show animations" setting) removes the breathing, shimmer, glow drift, spinners, and press scale; layout stays understandable without motion.

## 7. Store changes

- `spaceConversations` holds the sidebar lists: Home plus every expanded project, read in the same debounced `conversations` refresh as the current space (the current space is read last). A project's `conversations` read failing never blanks the current space.
- `expandedProjects` and `sidebarCollapsed` persist in `localStorage`.
- `employees` and `providers` are read models refreshed by `employee.*` and `roster.*` events.
- `openSetup`, `createTeam`, `setProjectExpanded`, `toggleSidebar`; `selectSpace` on the current space returns to its home.
- Found by the new tests and fixed: expanding a project before switching to it started a read for the old space that swallowed the new space's read; the dialog's focus effect re-ran on every render and stole focus while typing.

## 8. Verification

- `npm run check`: prettier, types, vitest (store, presets, transcript, app with axe on the sidebar and setup dialog), production build.
- `cargo test -p lazoo-desktop`: the acceptance-script contract test now requires the sidebar project and setup-dialog steps.
- `scripts/acceptance/desktop-shell.ps1`: dismisses the setup dialog when offered, restores a collapsed sidebar, and selects the project from the sidebar (gate `projectSidebar` replaces `spaceSwitch`).
- Visual review through the dev mock (`vite dev`, `?mock=1`, `&fresh=1` for first run) at 1280×800. Record: `docs/verification/2026-11-01-desktop-ui-refresh.md`.

## 9. Not done here

- A native folder picker for "Add a project" needs the Tauri dialog plugin (a new Rust dependency and capability); the dialog takes a pasted path for now.
- A light theme: the token roles make it a set of overrides, but the owner asked for dark slate.
- Live model lists per harness: suggestions are static; the field accepts any model the harness accepts.
