# Lazoo Desktop Information Architecture and Native Harness Workspace Design

- **Status:** Owner-approved interaction design; written specification awaiting final owner review
- **Owner review of visual sections:** 2026-08-30
- **Platform boundary:** Windows-first, local-first, single-user desktop software
- **Scope:** Target desktop information architecture, navigation, screen inventory, native terminal workspace, and required states
- **Implementation state:** Not started; the current read-only desktop remains the technical baseline

## 1. Purpose

This specification replaces the current desktop shell's provisional information architecture with an owner-approved, work-focused structure.

Lazoo is a harness for native coding harnesses. Its desktop must make it easy to start work, resume a conversation, watch delegated employees, respond to attention items, and configure the local control plane without impersonating Codex, Claude Code, OpenCode, Grok Build, Antigravity, or any other provider.

The core UI decision is therefore:

> Lazoo owns the surrounding project, conversation, worker, policy, and reliability shell. Each employee's actual native harness TUI remains the work surface inside its own supervised terminal session.

The existing dark visual system, design tokens, custom Windows title bar, keyboard principles, and minimum 860 by 620 window remain valid. The existing `Control room`, `Projects`, `Activity`, and `System` destinations do not remain the target sitemap.

This document does not authorize implementation or backend expansion. A separately reviewed implementation plan must stage the required typed contracts, provider adapters, ConPTY streams, and durable records. Until those capabilities exist, the desktop must not simulate native terminals, employees, or work that the daemon cannot truthfully report.

## 2. Owner-approved decisions

| Area | Decision |
|---|---|
| Home | A new-work launcher, not an operational metrics dashboard |
| Primary navigation | Discord-style two-level navigation: a narrow space/project rail and a contextual conversation sidebar |
| Projectless work | A permanent **Home** space above project icons owns projectless new work and history |
| History unit | Each row is a user conversation/work thread, not an employee session or delegated task |
| Lead selection | The user chooses a lead the first time in each Home/project space; Lazoo remembers it and keeps the selector visible |
| Launcher controls | Prompt, lead, and authority mode are visible; other overrides live behind **Options** |
| Active work | The lead's actual native harness TUI fills the primary pane |
| Workers | One compact bottom-dock card per delegated worker; selecting one expands its actual TUI in a resizable bottom panel |
| Attention | One global **Inbox** icon with an unresolved-item badge |
| Settings | A pinned gear opens full-page settings; project settings deep-link to the selected project |
| Project requirement | Optional; projectless conversations are durable first-class conversations |

## 3. Product jobs and design principles

### 3.1 Primary jobs

The desktop must make these jobs obvious within the first minute:

1. Start new work with a chosen lead and an explicit authority mode.
2. Resume a previous conversation in Home or a registered project.
3. Interact with the lead through its real native harness TUI.
4. See which employees were delegated, what harness each uses, and whether each is running, waiting, blocked, failed, or complete.
5. Open any worker's native terminal without losing the lead's terminal.
6. Respond to approvals, permission requests, and incidents from one attention queue.
7. Configure harnesses, employees, projects, authority, storage, memory, notifications, and resource limits.

### 3.2 Design principles

- **Work before telemetry.** Home starts or resumes work. Metrics and diagnostics are secondary.
- **Native capability over imitation.** Provider interaction stays inside the provider's actual TUI.
- **Conversation before implementation detail.** History follows the user's work thread; tasks and native sessions remain inside it.
- **Truthful state.** An unavailable lead is unavailable. Lazoo does not silently route to another employee or infer progress.
- **Progressive disclosure.** Frequent controls remain visible; advanced launch and diagnostic controls stay behind Options, inspectors, or Settings.
- **One place for attention.** Approvals, permissions, failures, and incidents share Inbox while preserving their distinct types.
- **Stable spatial memory.** Home and project positions do not reorder automatically. Returning to a space restores its last selected conversation.
- **Keyboard and mouse parity.** Terminal focus, navigation, overlays, and actions must work without trapping either input method.

## 4. Terminology contract

| Term | UI meaning |
|---|---|
| **Home** | Permanent projectless space containing its launcher and projectless conversation history |
| **Project** | A registered local folder/repository with project-scoped policy, memory, worktrees, and conversations |
| **Conversation** | One user-started work thread containing a lead session, delegated tasks, worker sessions, messages, approvals, incidents, and results |
| **Employee** | A named configured AI identity with a harness, model, role, capabilities, and policy |
| **Lead** | The employee the user directly starts or resumes for a conversation |
| **Worker** | An employee delegated work inside the current conversation |
| **Harness** | The native provider runtime, such as Codex, Claude Code, OpenCode, Grok Build, or Antigravity |
| **Terminal** | The embedded, supervised terminal surface carrying one harness's real TUI |
| **Task** | A scoped unit of delegated work within a conversation; not a top-level navigation destination |
| **Inbox item** | A typed approval, permission request, incident, or failure that needs user attention |
| **Authority** | Ask, Self-decision, or Full autonomy within the configured policy envelope |

The UI must not label the contextual sidebar **Session history**. **Conversation history** is the owner-facing concept. **Session** is reserved for an internal native harness runtime and appears only where operational detail is useful.

## 5. Sitemap

```text
Lazoo
├── Home
│   ├── New work
│   └── Projectless conversations
├── Project: <name>                  one destination per registered project
│   ├── New work
│   └── Project conversations
├── Inbox
│   ├── All
│   ├── Approvals
│   ├── Permissions
│   ├── Incidents
│   └── Resolved
└── Settings
    ├── General
    ├── Harnesses
    ├── Employees
    ├── Authority
    ├── Projects
    ├── Worktrees & storage
    ├── Memory
    ├── Notifications
    ├── Resources
    └── Advanced
```

The project rail also contains **Add project**. Command search is globally available but is not a separate destination.

## 6. Persistent shell and navigation

### 6.1 Shell regions

```text
┌──────────────────────────────────────────────────────────────────────┐
│ Lazoo              Search or run a command              Connection  │
├──────┬──────────────────────┬────────────────────────────────────────┤
│ Home │ Contextual sidebar   │ Main destination/work surface          │
│ ───  │                      │                                        │
│ Proj │ Home/Project:        │ Launcher or native terminal workspace  │
│ Proj │ New work + history   │                                        │
│  +   │                      │ Inbox: filters + items                 │
│      │ Inbox: filters       │ Settings: selected settings page       │
│Inbox │ Settings: categories │                                        │
│ Set  │                      │                                        │
├──────┴──────────────────────┴────────────────────────────────────────┤
│ Local/project scope, terminal focus, worker count, connection truth │
└──────────────────────────────────────────────────────────────────────┘
```

The default window remains 1180 by 760 and the minimum remains 860 by 620. At constrained widths, the narrow rail remains visible, the contextual sidebar becomes user-collapsible, and only one expanded worker terminal is shown. Lazoo must not solve narrow widths by shrinking several live terminals into unreadable grids.

### 6.2 Space/project rail

The left rail contains, from top to bottom:

1. permanent Home;
2. a separator;
3. one icon per registered project;
4. Add project;
5. flexible space;
6. Inbox with unresolved count;
7. Settings.

Project icons use a stable saved order and never reorder themselves based on recent activity. Newly registered projects append after existing projects. A tooltip and accessible name always expose the full project name; duplicate initials receive deterministic differentiators.

When projects exceed the available height, only the project segment scrolls. Home remains fixed above it, and Add project, Inbox, and Settings remain reachable.

Selecting Home or a project restores that space's last selected conversation. Selecting the already active icon keeps the current conversation. Home and each project keep independent launcher drafts and conversation selection.

### 6.3 Contextual sidebar

For Home and projects, the sidebar contains:

- the space/project name and context menu;
- **New work** pinned first;
- conversation history grouped by Today, Previous 7 days, and Earlier;
- state markers only when a conversation is active, needs attention, failed, or has completed unread results.

Each history row represents the full conversation. Worker sessions and tasks never become sibling history rows. Titles are generated from the first request, are user-editable, and remain stable after editing.

Inbox uses the same region for attention filters. Settings uses it for settings categories.

### 6.4 Header, breadcrumbs, back behavior, and command search

- The main header shows `Home / New work`, `<Project> / New work`, or `<Project> / <Conversation>`.
- The active terminal header separately identifies employee, lead/worker role, harness, task, authority, and worktree scope.
- `Alt+Left` and `Alt+Right` traverse Lazoo destination/selection history. They never alter terminal command history.
- `Escape` closes the topmost overlay, then exits worker-terminal maximize, then collapses the expanded worker panel. It does not cancel native work.
- `Ctrl+K` opens command search. It finds conversations, switches spaces/projects, focuses a terminal, opens Inbox filters, and exposes safe context-aware commands.
- Closing the window leaves daemon-owned work running. Reopening restores each space, conversation, terminal selection, and unsent draft when the durable state still exists.

## 7. Home and New work

### 7.1 Home purpose

Home answers one question: **What should the team work on?**

It does not show daemon metrics, event counts, promotional content, generic recent-project cards, or a control-room dashboard. Projectless conversation history already appears in the sidebar, and attention already appears in Inbox.

### 7.2 Launcher fields

Always visible:

- prompt;
- lead employee;
- authority mode;
- project control only when launching from Home;
- Start action.

Behind **Options**, capability-dependent controls may include:

- model and reasoning effort;
- editing/worktree policy;
- allowed tools, skills, MCP, plugins, and connectors;
- memory mode and context controls;
- environment overrides;
- terminal/session resume options.

Unsupported controls are absent, not disabled promises.

### 7.3 Lead memory

The first launch in Home or a project requires the user to choose a lead. After a successful start, Lazoo remembers that lead for that space. Later launchers preselect it but keep the selector visible.

If the remembered lead is unavailable, degraded beyond policy, removed, or incompatible, Lazoo shows the exact reason and requires a new choice. It never silently substitutes another employee.

### 7.4 Projectless work

Home defaults to **Project: None**. The user may attach a registered project or choose a local folder during launch. Choosing an unregistered folder completes the short project-registration sheet before work starts.

A projectless conversation has durable history, native sessions, tasks, approvals, incidents, and results. Its harnesses run from a dedicated Lazoo-managed scratch directory for that conversation; it has no Git worktree or project-scoped memory until a project is attached.

Attaching a project later is an explicit, audited scope change. It does not silently relocate existing files or rewrite prior history.

### 7.5 Validation and keyboard behavior

Start is blocked when:

- the prompt is empty;
- no lead has been chosen;
- the lead is not currently startable;
- required harness setup or authentication is incomplete;
- the selected options contradict policy.

The launcher explains the first blocking reason beside the affected control. `Enter` starts work and `Shift+Enter` inserts a newline. A running start request cannot be submitted twice.

## 8. Active conversation and native harness workspace

### 8.1 Lead terminal

The lead's actual native harness TUI fills the primary work area. Lazoo hosts the terminal stream and owns framing, focus, lifecycle controls, identity, and recovery state. It does not re-render provider turns as generic chat bubbles and does not strip provider-native tools, permissions, plugins, skills, or terminal behavior.

The terminal header always shows:

- conversation title and project/Home scope;
- employee name and **Lead** role;
- harness and model when available;
- authority mode;
- worktree/projectless scope;
- truthful lifecycle state.

### 8.2 Worker dock

The bottom dock appears when the conversation has delegated workers. Each card shows:

- employee name;
- harness;
- concise task label;
- running, waiting, needs approval, blocked, failed, complete, interrupted, or unknown state.

Cards use a horizontal overflow treatment rather than wrapping into multiple tiny terminal rows. Completed workers remain visible, visually muted, until dismissed or the conversation is archived.

Selecting a card opens that worker's actual TUI in a resizable bottom panel while the lead remains visible above. Only one worker terminal is expanded at a time. Selecting the active card collapses it. The expanded panel may be temporarily maximized and then restored to its previous height.

### 8.3 Terminal focus and takeover

Only the visibly focused terminal receives keyboard input. Focus is indicated in the terminal frame and repeated in the status bar. Switching cards does not redirect pending keystrokes.

Interrupt, direct user input, takeover, resume, stop, and terminal maximize are explicit actions. Actions that change employee execution are audited. Closing or collapsing a panel never stops its worker.

Native terminal scrollback remains independently scrollable per employee. Lazoo preserves terminal dimensions and sends resize events only to the affected ConPTY session.

### 8.4 Approvals and permissions

Two approval layers remain distinct:

1. Provider-native permission prompts stay inside that harness's TUI.
2. Lazoo policy approvals appear in an external contextual strip and in Inbox.

An inline Lazoo approval strip may offer the exact allowed actions, such as **Approve once**, **Review scope**, or **Deny**. It must never look like provider terminal output. Approval state is durable and linked to the conversation, employee, task, requested action, authority policy, and action fingerprint.

When a provider permission can be answered through a typed adapter contract, Inbox may expose the response. Otherwise Inbox deep-links to and focuses the waiting terminal.

### 8.5 No synthetic collaboration transcript

Lazoo stores and correlates user turns, delegated tasks, messages, results, and audit events, but it does not create a fake merged chat surface. Cross-employee messages and task evidence are available through contextual inspectors and durable records; the native lead and worker TUIs remain the conversational surfaces.

## 9. Global Inbox

Inbox is the only permanent attention destination. Its rail badge counts unresolved items requiring user action, not total events or unread informational activity.

Filters are:

- All;
- Approvals;
- Permissions;
- Incidents;
- Resolved.

Operational failures are typed incidents with a failure subtype, so they appear under **Incidents** rather than creating a fifth unresolved category.

Every item shows type/severity, concise required action, project/Home scope, conversation, employee, harness, task when present, age, and current state. Selecting an item opens its detail and offers a deep link to the exact conversation and terminal.

Resolving, denying, retrying, or acknowledging an item updates its durable state. Resolved items leave the badge and remain available in the Resolved archive. Unrelated healthy work remains usable when one Inbox item is degraded or failed.

## 10. Settings

Settings is a full-page destination, not a modal. Categories are:

| Category | Responsibility |
|---|---|
| General | Startup, close behavior, interface, default remembered-lead behavior |
| Harnesses | Discovery, executable/version truth, authentication readiness, adapter compatibility, native configuration handoff |
| Employees | Named identities, harness, model, role, capabilities, project access, enable/disable state |
| Authority | Global defaults and visible Ask/Self-decision/Full-autonomy envelopes |
| Projects | Registered folders, per-project defaults, conversation retention, contextual project settings |
| Worktrees & storage | Isolation defaults, disk attribution, retention, safe cleanup previews |
| Memory | Transcript retention, typed memory, external processing visibility, project overrides |
| Notifications | Native Windows notifications, priority rules, mute behavior |
| Resources | Lazoo budget, employee/provider visibility, concurrency and idle suspension |
| Advanced | Diagnostics, data root, protocol/runtime information, export, recovery tools |

Project header menus deep-link to **Settings / Projects / `<selected project>`**. Harness authentication stays in the harness's supported native flow. Lazoo does not collect provider passwords or replace provider credential stores.

Settings with immediate, reversible effects may save inline. Destructive or scope-expanding changes require a preview and explicit confirmation. Dirty worktrees, retained transcripts, or active sessions are never silently deleted or relocated.

## 11. Supporting surfaces

### 11.1 Add project

The rail's **Add project** action opens the native Windows folder picker followed by a short registration sheet. The sheet shows detected Git identity, canonical path, default editing isolation, remembered lead if selected, and any blocking validation. It does not expose the full project-settings form.

### 11.2 Command palette

The command palette handles fast navigation and safe context-aware actions. Commands are grouped by Go to, Conversations, Terminals, and Actions. Destructive operations never become one-keystroke commands without their normal confirmation.

### 11.3 Inspector and drawers

Inspectors are for secondary detail:

- task scope and dependencies;
- worktree path and starting revision;
- commits and verification evidence;
- delivery/message correlation;
- incident history;
- session/runtime metadata;
- audit history.

Inspectors never replace the launcher, native terminal, Inbox, or Settings as the primary surface for their jobs.

### 11.4 Status bar

The status bar stays concise and global. It shows connection truth, current Home/project scope, terminal focus identity, and active worker count. Diagnostics and metrics belong in Advanced settings or an inspector.

## 12. Global versus contextual visibility

| Always global | Visible in the active context | On demand |
|---|---|---|
| Home/project rail | Conversation history | Launch Options |
| Inbox badge | Lead identity and authority | Task/evidence inspector |
| Settings entry | Native lead terminal | Worktree/audit detail |
| Command search | Worker dock | Advanced diagnostics |
| Connection truth | Inline Lazoo approval strip | Destructive confirmations |
| Status bar | Focused terminal identity | Resolved Inbox archive |

There is no permanent Control room, Activity, System, global task board, or employee-roster destination. Their useful responsibilities are redistributed to Home, worker dock, Inbox, Settings, command search, inspectors, and the status bar.

## 13. Core journeys

### 13.1 First run

```text
Open Lazoo
  -> Home launcher
  -> no employee available
  -> detect/configure a native harness
  -> create the first named employee
  -> choose that lead and authority
  -> optionally attach a project
  -> start the native lead TUI
```

First run uses the ordinary Home structure. It does not introduce a separate full-screen wizard that users never see again.

### 13.2 Everyday project work

```text
Select project icon
  -> New work or remembered conversation
  -> enter prompt
  -> confirm remembered lead and authority
  -> start
  -> lead's native TUI becomes primary
```

### 13.3 Delegation and worker inspection

```text
Lead proposes or performs delegation
  -> Ask mode may create a Lazoo approval
  -> approved task starts a separate native harness/terminal/worktree
  -> worker card appears in the bottom dock
  -> select card to expand worker TUI
  -> collapse/maximize without stopping work
  -> result remains linked to the conversation
```

### 13.4 Attention flow

```text
Approval, provider permission, or incident occurs
  -> affected terminal/dock state updates
  -> contextual strip appears when relevant
  -> Inbox badge increments
  -> selecting item deep-links to exact conversation/terminal
  -> explicit action resolves or archives the item
```

## 14. Required states and error behavior

| State | Required behavior |
|---|---|
| First run | Home remains recognizable; launcher explains the minimum harness/employee setup and keeps projects optional |
| Loading | Shell appears immediately; retain last valid content; show local progress without fabricated terminal/session data |
| Empty Home | Show the projectless New work launcher and no sample history |
| Empty project | Show that project's New work launcher and empty conversation sidebar; no metrics dashboard |
| Empty Inbox | Show one calm all-clear state and the active filter; do not insert informational events to make the list look populated |
| No startable lead | Lead control explains detected/configured/authenticated/compatible state and links to Harnesses or Employees |
| Daemon disconnected | Persistent banner and status bar; preserve last output; mark running state unknown; freeze new input/actions; offer Retry and diagnostics |
| Harness degraded | Mark only affected employee/terminal; create Inbox item when actionable; keep unrelated work usable |
| Terminal stream interrupted | Preserve scrollback, show reconnection/resume state, and never report the employee stopped without evidence |
| Worker stalled | Worker card and Inbox show missing-progress state, retry budget, and available remediation |
| Permission waiting | Native prompt remains inside TUI; dock/Inbox identify the waiting worker and route focus correctly |
| Lazoo approval waiting | External strip plus Inbox; bind decision to one action fingerprint and expiry |
| Project folder unavailable | Keep history; disable new project-scoped work; show prior canonical path and recovery choices |
| Many workers | Horizontally scroll/overflow worker cards; expand one terminal at a time; never create an unreadable mini-terminal grid |
| Full disk or retention limit | Block unsafe new worktree/archive writes, preserve existing work, show attributable usage and cleanup preview |
| Settings read/write failure | Keep the last confirmed value, mark the affected control unsaved, show the stable error code, and offer retry without implying success |

If daemon connectivity is lost, the UI must say **Unknown** rather than changing a running employee to **Stopped**. Reconnection reconciles durable state and emits one recovery summary instead of duplicate attention items.

## 15. Component boundaries

The frontend should retain small, testable responsibilities:

| Unit | Responsibility | Depends on |
|---|---|---|
| Shell chrome | Title bar, rail, global status, window controls | route state, connection state |
| Space navigation | Home/project icons and stable selection | project registry |
| Context sidebar | New work, conversations, Inbox filters, Settings categories | active destination |
| Launcher | prompt, lead, authority, project, Options, validation | live roster, policy, capability truth |
| Terminal host | attach/render/focus/resize one native stream | typed terminal stream bridge |
| Worker dock | worker cards, selection, overflow, expanded panel | durable task/session state |
| Approval strip | exact Lazoo policy decision | approval contract |
| Inbox | filters, item list/detail, deep links, actions | approval/permission/incident ledger |
| Settings | category routing and typed forms | configuration contracts |
| Inspector | evidence and operational metadata | selected durable record |
| Command palette | search and safe actions | route index, command registry |

No component accepts an arbitrary daemon method, shell command, or provider payload from the webview. Privileged operations remain typed Rust/Tauri commands with narrow permissions.

## 16. Data and event flow

```text
Keyboard/mouse
  -> Lazoo route or focused terminal host
  -> typed Tauri command / framed terminal input
  -> authenticated local daemon connection
  -> policy + durable record
  -> provider adapter
  -> employee ConPTY/native harness

Native harness output
  -> ConPTY/provider adapter
  -> separate framed terminal stream
  -> selected terminal host + retained scrollback

Task/session/approval/incident changes
  -> SQLite transaction + outbox/event stream
  -> shell snapshot/subscription
  -> conversation state, worker dock, Inbox, status bar
```

High-volume terminal traffic remains separate from control traffic so output cannot starve cancellation, approvals, heartbeats, or incidents. The worker dock derives state from durable task/session records and adapter progress evidence, not from terminal-text scraping.

The current `load_desktop_snapshot` command and read-only snapshot boundary remain unchanged until an approved implementation slice defines additive typed contracts. The webview never receives provider credentials, DPAPI material, or an arbitrary IPC escape hatch.

## 17. Accessibility and interaction requirements

- Every icon-only rail, dock, and terminal-control action has an accessible name and tooltip.
- Rail, contextual sidebar, worker dock, Inbox, Settings navigation, and command results use roving keyboard focus where appropriate.
- Terminal focus is never conveyed by color alone.
- Opening and closing a worker panel, Inbox detail, command palette, or inspector restores focus to the initiating control.
- Alerts use text, iconography, and semantic live regions; terminal output is not redundantly announced as a live region.
- Motion respects `prefers-reduced-motion` and never delays terminal input.
- The full structure remains operable at 860 by 620 and with 200 percent text scaling, with collapsible context rather than hidden actions.

## 18. Testing and acceptance

### 18.1 Pure model and component tests

Tests must cover:

- Home/project route restoration and stable project ordering;
- conversation grouping and editable stable titles;
- first-choice lead memory per space and unavailable-lead blocking;
- launcher validation, authority display, Options capability filtering, and duplicate-start prevention;
- worker-card ordering, state mapping, overflow, selection, collapse, maximize, and focus restoration;
- Inbox unresolved counts, filtering, resolution, and deep-link routing;
- disconnected and last-good-state transitions without false stopped states;
- Settings routing and contextual project deep links;
- keyboard navigation and accessible names.

### 18.2 Contract and integration tests

Typed Rust/daemon tests must prove:

- no arbitrary daemon method or terminal target can be supplied by the webview;
- terminal input routes only to the focused authorized session;
- resize, interrupt, takeover, stop, and resume target the correct employee session and are audited;
- terminal streams cannot starve control-plane messages;
- worker state comes from durable task/session/progress records;
- approval responses bind to the exact request and cannot cross conversations or expire silently;
- reconnect reconciliation does not duplicate conversations, workers, or Inbox items.

### 18.3 Real-window and failure acceptance

Acceptance must include:

1. Start projectless work after choosing a lead and Ask authority.
2. Reopen Home and confirm the lead is remembered and still visible.
3. Start project work and verify the lead's actual harness TUI is embedded.
4. Delegate to at least two different native harnesses and verify separate terminals/process identities.
5. Expand one worker, switch to another, collapse, maximize, and return without losing terminal state or misrouting input.
6. Exercise a provider-native permission and a Lazoo policy approval and verify their distinct surfaces.
7. Disconnect the daemon during live output and verify frozen last output plus Unknown state, then reconcile cleanly.
8. Stall and crash one worker while unrelated work continues.
9. Verify Home, project, Inbox, and Settings at 1180 by 760, 860 by 620, keyboard-only, and the available Windows text-scaling gate.

No acceptance may claim a native harness TUI works until that harness passes its own live provider conformance gate. Mock terminal fixtures prove UI behavior only.

## 19. Implementation staging constraints

The design is a target architecture, not a request for a monolithic UI rewrite. The implementation plan should use narrow end-to-end slices:

1. provider truth, first employee setup, and truthful launcher prerequisites;
2. one live lead harness with embedded ConPTY TUI and recovery;
3. conversation persistence and Home/project navigation;
4. durable delegation plus the worker dock and expanded worker terminal;
5. approvals/permissions/incidents through Inbox;
6. full Settings categories as their underlying configuration contracts become real.

A slice must not expose a visible control before its end-to-end contract exists. Static mock terminals, fake workers, inert settings, and optimistic provider status are prohibited outside tests and design artifacts.

## 20. Explicit exclusions

- generic Lazoo-rendered chat bubbles or a merged multi-agent transcript;
- a global task-board-first home screen;
- permanent Control room, Activity, or System destinations;
- external terminal windows as the default interaction model;
- multi-user, hosted, web, or mobile navigation;
- voice UI;
- alternate visual themes or further aesthetic redesign;
- exact provider-specific Settings forms before adapter contracts are designed;
- implementation code or backend protocol changes in this design step.

## 21. Completion gate for this design

The design step is complete when:

1. the owner approves this written specification;
2. the specification contains no unfinished markers, contradictory routes, or ambiguous primary surfaces;
3. continuity documents no longer describe the sitemap as undecided; and
4. a separate implementation plan is written and reviewed before UI code changes resume.
