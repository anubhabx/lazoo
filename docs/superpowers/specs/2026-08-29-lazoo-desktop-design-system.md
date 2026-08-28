# Lazoo desktop design system: Zed structure × Raycast interaction

- **Date:** 2026-08-29
- **Status:** Proposed; research and starter tokens complete; not applied to the product
- **Scope:** Dark-first desktop design system for the Lazoo Tauri shell and future orchestration surfaces

## Decision

Lazoo will use a deliberately hybrid design system:

- **Zed supplies the structural grammar:** density-aware spacing, compact typography, surface roles, component dimensions, restrained elevation, icon sizes, and short timing anchors.
- **Raycast supplies the interaction grammar:** search as a primary surface, contextual actions, shortcut teaching, immediate feedback, compact focus, and desktop-native behavioral polish.
- **Lazoo owns the product expression:** graphite neutrals, ember identity accent, truthful provider states, agent/task/approval components, Windows keyboard conventions, and orchestration-specific information architecture.

This is not a pixel clone. It does not copy logos, icons, illustrations, proprietary assets, or unverified private tokens. It takes public, observable rules and converts them into a coherent Lazoo-owned system.

The generated starter files are:

- `design-system/tokens.json`
- `design-system/tokens.css`
- `design-system/README.md`

They are intentionally not imported by the current UI.

## Evidence and confidence

| Evidence                                                                                        | What it proves                                                                                                                | Confidence                                    |
| ----------------------------------------------------------------------------------------------- | ----------------------------------------------------------------------------------------------------------------------------- | --------------------------------------------- |
| Zed source pinned at `956a49e4ca8aa4b7c2c293e1414c91f009824ae3`                                 | Exact public spacing math, type sizes, dimensions, radii, elevation, theme roles, and animation anchors                       | High                                          |
| Zed official appearance, theme-builder, and settings case studies                               | Why Zed separates surface roles, links related color roles, supports density, and treats focus/state as component concerns    | High                                          |
| Raycast official redesign, current product screenshots, manual, UI API, and technical deep dive | Search/action hierarchy, shortcut behavior, theme model, immediate rendering, and native-feeling webview requirements         | High                                          |
| Automated extraction of `zed.dev` and `raycast.com`                                             | Marketing-site font and spacing observations                                                                                  | Medium for those pages; low for in-product UI |
| Independent Raycast design-system analyses                                                      | Cross-checks marketing-site measurements and perceived visual patterns                                                        | Secondary only                                |
| Installed Zed application                                                                       | Not available: it was not discoverable through app inventory, command path, Start menu, AppX inventory, or uninstall registry | No evidence used                              |

The automated extraction was retained as a constraint rather than treated as authoritative:

- `zed.dev` reported a `plexSerif` marketing font and the observed spacing values `1, 2, 3.5, 4, 6, 8, 10, 12, 14, 16, 19, 24, 32, 36, 39, 40, 48, 84.4375px`, but it yielded no reliable palette, radius, or shadow scale.
- `raycast.com` reported Inter and the observed spacing values `1, 2, 2.5, 3, 4, 8, 10, 12, 14.5, 15.5, 16, 20, 24, 32, 40, 48, 50, 64, 96, 224px`, but its palette extraction collapsed to white and transparent.

Those values describe marketing pages, not the desktop products. They therefore do not override the product evidence below.

## What comes directly from Zed

### Density-aware spacing

Zed's public spacing system defines base values at the default density and maps every value to compact and comfortable alternatives. Small values use explicit triplets; values from 24px upward use `base − 4`, `base`, and `base + 4`. The exact implementation is visible in Zed's [spacing definition](https://github.com/zed-industries/zed/blob/956a49e4ca8aa4b7c2c293e1414c91f009824ae3/crates/ui/src/styles/spacing.rs) and [spacing macro](https://github.com/zed-industries/zed/blob/956a49e4ca8aa4b7c2c293e1414c91f009824ae3/crates/ui_macros/src/dynamic_spacing.rs).

| Base token | Compact | Default | Comfortable |
| ---------: | ------: | ------: | ----------: |
|          0 |       0 |       0 |           0 |
|          1 |       1 |       1 |           2 |
|          2 |       1 |       2 |           4 |
|          3 |       2 |       3 |           4 |
|          4 |       2 |       4 |           6 |
|          6 |       3 |       6 |           8 |
|          8 |       4 |       8 |          10 |
|         12 |      10 |      12 |          14 |
|         16 |      14 |      16 |          18 |
|         20 |      18 |      20 |          22 |
|         24 |      20 |      24 |          28 |
|         32 |      28 |      32 |          36 |
|         40 |      36 |      40 |          44 |
|         48 |      44 |      48 |          52 |

Lazoo adopts this table exactly. Default is the only active product density for the first redesign; compact and comfortable are encoded now to prevent one-off spacing later.

### Type scale

Zed's public component typography uses 10, 12, 14, and 16px UI text and approximately 14, 16, 18, 20, and 23px headlines. Its default settings use a 16px UI font, 15px buffer font, IBM Plex Sans through `.ZedSans`, and Lilex through `.ZedMono`. See Zed's [typography source](https://github.com/zed-industries/zed/blob/956a49e4ca8aa4b7c2c293e1414c91f009824ae3/crates/ui/src/styles/typography.rs) and [default settings](https://github.com/zed-industries/zed/blob/956a49e4ca8aa4b7c2c293e1414c91f009824ae3/assets/settings/default.json).

Lazoo keeps the sizes but uses a product-specific font stack:

| Role            | Size / line height | Weight  | Use                                               |
| --------------- | ------------------ | ------- | ------------------------------------------------- |
| Micro           | 10 / 14            | 500     | exceptional counters and tiny annotations only    |
| Metadata        | 12 / 16            | 400–500 | timestamps, providers, shortcuts, secondary state |
| UI body         | 14 / 20            | 400     | rows, controls, messages, settings                |
| UI emphasis     | 16 / 22            | 500     | section titles, prominent values                  |
| Headline small  | 18 / 24            | 600     | panel or inspector title                          |
| Headline medium | 20 / 28            | 600     | workspace title                                   |
| Headline large  | 23 / 32            | 600     | rare empty-state or first-run title               |

The UI stack is `Inter Variable`, then Inter, Segoe UI Variable, and Segoe UI. Technical strings use IBM Plex Mono, then Cascadia Mono and Consolas. Font files are not bundled by this research slice; the fallbacks make the starter safe until the implementation plan includes an asset and license check.

### Component dimensions

Zed's public component code defines button heights of 18, 22, 28, and 32px; a 32px tab; and icon sizes of 10, 12, 14, 16, and 48px. See the [button sizing](https://github.com/zed-industries/zed/blob/956a49e4ca8aa4b7c2c293e1414c91f009824ae3/crates/ui/src/components/button/button_like.rs), [tab sizing](https://github.com/zed-industries/zed/blob/956a49e4ca8aa4b7c2c293e1414c91f009824ae3/crates/ui/src/components/tab.rs), and [icon sizing](https://github.com/zed-industries/zed/blob/956a49e4ca8aa4b7c2c293e1414c91f009824ae3/crates/ui/src/components/icon.rs).

Lazoo adopts those anchors and adds only product-level geometry:

- ordinary row: 32px;
- compact row: 28px;
- comfortable row: 36px;
- command result: 40px;
- global search: 44px;
- titlebar: 40px;
- toolbar and contextual action bar: 36px;
- status bar: 28px;
- navigation: 208px;
- inspector: 320px;
- command palette: 720px wide, 560px maximum height.

### Radius and elevation

Zed's GPUI radius scale is `0, 2, 4, 6, 8, 12, 16, 24, full`. Lazoo uses 4px for rows, 6px for controls, 8px for grounded panels, and 12–16px only for floating surfaces. This preserves Raycast's polish without turning every element into a pill.

Zed models five elevation roles: background, surface, editor surface, elevated surface, and modal surface. Normal surfaces have no shadow; only raised and modal surfaces do. Lazoo adopts the same hierarchy and its restrained multi-layer shadow construction from Zed's [elevation source](https://github.com/zed-industries/zed/blob/956a49e4ca8aa4b7c2c293e1414c91f009824ae3/crates/ui/src/styles/elevation.rs).

### Motion anchors

Zed defines 50ms, 150ms, and 300ms timings with an ease-out-quint entrance. See the [animation source](https://github.com/zed-industries/zed/blob/956a49e4ca8aa4b7c2c293e1414c91f009824ae3/crates/ui/src/styles/animation.rs). Lazoo uses those anchors and adds 220ms for panel transitions:

- 50ms: immediate acknowledgment;
- 150ms: hover, press, selection, tooltip;
- 220ms: inspector, command palette, disclosure;
- 300ms: rare mode or large-area transition.

Zed's example entrance can travel 40px. Lazoo does not adopt that distance: ordinary motion is 4px and panel motion is 8px, matching Raycast's quieter perceived movement.

## What comes from Raycast

Raycast describes its design principles as **fast, simple, and delightful**. Its redesign made search more prominent, consolidated navigation, toasts, contextual actions, and shortcuts into an action bar, standardized an outline icon family, and added a compact mode that deemphasizes nonessential UI. These are explicit product decisions in Raycast's [design write-up](https://www.raycast.com/blog/a-fresh-look-and-feel).

Raycast's extension UI formalizes four high-level surfaces—List, Grid, Detail, and Form—and attaches a context-aware Action Panel to them. It recommends rendering immediately and exposing a loading state instead of waiting for data before showing the view. See the [Raycast UI reference](https://developers.raycast.com/api-reference/user-interface) and [Action Panel reference](https://developers.raycast.com/api-reference/user-interface/action-panel).

Its current technical case study makes the desktop-quality bar explicit: wrong animations, clipped popovers, and flickering transitions make a webview feel like a website. It also distinguishes native behavior from web conventions and describes pre-rendering work used to prevent visual blank frames. See [A Technical Deep Dive Into the New Raycast](https://www.raycast.com/blog/a-technical-deep-dive-into-the-new-raycast).

Raycast themes expose background, primary, and support colors rather than asking users to manage hundreds of unrelated visual values. See the [Raycast theme manual](https://manual.raycast.com/themes). Lazoo uses that simplicity at the personalization layer while retaining Zed-like semantic roles internally.

### Color character adopted for Lazoo

Observable Raycast product surfaces use near-black foundations, small luminance steps, thin neutral borders, bright text, restrained selection fills, and localized accent color. Lazoo translates that character into a fixed initial dark palette:

| Role            | Value     | Rule                                           |
| --------------- | --------- | ---------------------------------------------- |
| Canvas          | `#08090B` | outside or deepest app layer                   |
| Window          | `#0B0D10` | primary window background                      |
| Background      | `#0F1115` | blank panes and workspace background           |
| Surface         | `#13151A` | navigation, panels, grounded groups            |
| Raised surface  | `#17191E` | inspector or elevated container                |
| Overlay         | `#1C1E24` | menus, command palette, dialogs                |
| Hover           | `#22252B` | pointer confirmation on Windows                |
| Active/selected | `#292C33` | pressed or selected row                        |
| Border          | `#323640` | ordinary one-pixel separation                  |
| Strong border   | `#454A55` | overlay and active control boundary            |
| Primary text    | `#F4F5F7` | titles and normal readable content             |
| Secondary text  | `#A9ADB7` | supporting content                             |
| Muted text      | `#7E838E` | metadata and inactive content                  |
| Lazoo ember     | `#FF6B45` | focus, current location, primary action, brand |

The near-black surface structure is Raycast-derived; the ember accent is Lazoo-owned. It preserves the useful warmth of the previous identity without carrying forward the previous paper UI.

Measured WCAG contrast against `#0B0D10` is 17.84:1 for primary text, 8.66:1 for secondary text, 5.12:1 for muted text, and 6.89:1 for ember. Disabled text intentionally sits at 2.90:1 because it is not actionable or required for comprehension.

## Lazoo design principles

### 1. Dense, not cramped

The default 32px row and 14px UI text make scanning efficient. Whitespace expresses grouping, not prestige. A screen should not use large empty areas to look polished.

### 2. Search and action are first-class

`Ctrl+K` opens one context-aware command surface from anywhere. With no actionable selection it is global; with an active object it ranks that object's actions first while keeping navigation searchable. `Enter` performs the primary action, `Ctrl+Enter` performs the secondary action, arrows change selection, and `Escape` closes the current transient layer. Visible shortcut hints teach these routes as the user works.

### 3. State is truthful

Agent activity and provider capability are separate concepts. An employee can be idle while its provider is configured, or active while a dependency is degraded. Lazoo never reduces these distinctions to one colored dot.

Provider capability labels remain `Detected`, `Configured`, `Degraded`, and `Unverified` until stronger product gates establish additional states. Activity labels use `Running`, `Waiting`, `Needs input`, `Paused`, `Completed`, and `Failed` as appropriate to the later domain contract.

### 4. Color is information

Ember identifies Lazoo, focus, current location, and the primary action. Blue means running, green means confirmed health, yellow means attention, red means blocked or failed, violet means paused, gray means unverified, and cyan means configured. Every semantic color is paired with text or iconography.

### 5. Desktop behavior wins over web habit

Buttons and list rows keep the arrow cursor; only links use a pointer. Windows receives a restrained hover fill because hover confirmation is a familiar Windows behavior, but focus and selection remain visually distinct. Transient layers never clip at the application edge. Layout changes do not flash or jump while data arrives.

### 6. Progressive disclosure beats dashboards

The Control Room prioritizes `Needs attention`, `Running now`, and recent outcomes. Details belong in an inspector, disclosure region, or dedicated workspace. Lazoo does not begin with a grid of large metric cards.

## Interaction state model

| State          | Background                   | Border                      | Content                             | Motion              |
| -------------- | ---------------------------- | --------------------------- | ----------------------------------- | ------------------- |
| Default        | semantic surface             | subtle or none              | primary/secondary                   | none                |
| Hover          | hover surface                | unchanged                   | primary                             | 150ms color         |
| Keyboard focus | current surface + focus wash | 2px ember                   | primary                             | 50ms acknowledgment |
| Pressed        | active surface               | strong                      | primary                             | 50ms                |
| Selected       | selected surface             | optional 2px leading marker | primary + semantic icon             | 150ms               |
| Disabled       | disabled surface             | subtle                      | disabled text/icon                  | none                |
| Loading        | stable layout                | unchanged                   | skeleton/progress in reserved space | opacity only        |

Focus uses `:focus-visible`; mouse activation does not leave a persistent focus halo. Selection and focus may coexist and must remain distinguishable.

## Core component contracts

### Application shell

- 40px integrated titlebar with Lazoo identity, current workspace, global command entry, compact health summary, and Windows controls.
- 208px navigation pane with 32px rows. It may collapse to icons but is not permanently icon-only.
- Central workspace owns the primary task.
- 320px inspector appears only when selection needs detail or action.
- 28px status bar shows provider/daemon truth and background activity without competing with the workspace.
- Surfaces meet through one-pixel borders; the shell is not a set of floating cards.

### Global command surface

- Opens with `Ctrl+K` and receives focus immediately. Its initial scope follows the current focus/selection, but global navigation remains searchable.
- 720px wide, maximum 560px high, with a 44px search field and 40px result rows.
- Groups results by intent: Navigate, Create, Act, Inspect, and Recent.
- Selected result uses neutral selection fill; icons or semantic metadata carry color.
- The bottom action bar shows the primary action and the shortcut to reveal all actions.
- Results render immediately; asynchronous sections use reserved loading rows rather than delaying the whole surface.

### Contextual action bar and action panel

- The action bar is 36px high and anchored to the bottom of the active surface.
- Left side: current location, breadcrumb, or transient message.
- Right side: primary action, shortcut, and `Actions  Ctrl+K`.
- The action panel groups actions semantically and searches within the current context.
- Destructive actions are separated and never become the default selected action.
- If no contextual actions exist, the right side collapses; it does not show disabled filler.

### Operational list and row

- Ordinary row is 32px; command rows are 40px.
- Anatomy: start slot, primary label, secondary metadata, flexible spacer, state, trailing action.
- Start slot is normally a 16px icon or 20–24px avatar.
- End actions may appear on hover or focus, but status and essential metadata never disappear.
- Dense rows use neutral selection; large colored row backgrounds are prohibited.
- Tree indentation advances 12px per level, matching Zed's public list-item default.

### Employee row

- Shows employee identity, current task, provider/model, activity state, elapsed time, and exceptional condition.
- Provider truth is available in metadata or inspector, not conflated with activity.
- Dormant or idle employees are visually quiet but remain readable.
- `Needs input`, approval waits, and failures rise above ordinary running work.

### Task and approval rows

- Task rows prioritize title, owner/employee, state, elapsed time, and project/worktree.
- Approval rows show requested capability, requester, scope, risk explanation, and age.
- Approve and deny actions are reachable from the keyboard and the action panel.
- Approval color communicates urgency or risk, not a recommendation to approve.

### Inspector

- Opens from the right without resizing the navigation pane.
- Uses 12–16px section padding and 8px group gaps.
- Provides the selected object's exact state, history, permissions, logs, and actions.
- Closing it returns focus to the item that opened it.

### Status indicators

- Small status dots are 6px visual marks inside a 10px minimum indicator box.
- A dot never appears without a nearby label, tooltip, or accessible name.
- Live activity may use a subtle opacity pulse only when it communicates actual ongoing work.
- Health green is reserved for confirmed success; presence alone is not health.

### Overlays and notifications

- Popovers use the overlay surface, 12px radius, strong one-pixel border, and raised shadow.
- Modals use a 16px radius and modal shadow; they are reserved for decisions that truly block the workflow.
- Toasts live inside the shell/action area where possible so they remain associated with context.
- Error messages state what failed, the affected object, and the next available action.

## Iconography

Use one outline icon family with consistent stroke weight and corner behavior. Default icon size is 16px, metadata icons are 12–14px, and 20px is reserved for prominent controls. Multicolor provider icons may retain their identity; application action icons remain monochrome until selected or semantic.

The specific icon package is an implementation decision, but mixing Lucide, Fluent, emoji, and custom glyphs in the same surface is prohibited.

## Motion rules

- Animate opacity and transform; do not animate layout properties when a transform can express the same relationship.
- Hover and selection: 150ms.
- Inspector and command surface: 220ms with 8px maximum travel.
- Tooltip: 150ms with 4px maximum travel.
- Progress changes may crossfade; they must not repeatedly slide the entire row.
- No spring overshoot for operational state.
- No glow, bouncing status dots, ambient gradients, or perpetual decorative movement.
- `prefers-reduced-motion` reduces nonessential durations and translation to zero.

## Content and density rules

- Sentence case everywhere except compact acronyms and provider names.
- Avoid all-caps section headings and excessive letter spacing.
- Use tabular numerals for elapsed times, counts, and timestamps.
- Truncate paths and identifiers in the middle when the start and end both matter.
- Do not hide a critical state behind a tooltip.
- Empty states explain why the area is empty and offer at most one primary next action.
- Loading preserves final layout dimensions.

## Accessibility and keyboard contract

- All interactive components have an accessible name, role, state, and predictable tab order.
- Composite lists use arrow-key selection inside one focus scope rather than placing every row action in the global tab order.
- Dialogs and popovers trap or restore focus as appropriate.
- Primary, secondary, muted, and accent text combinations meet WCAG AA for normal text; disabled content is excluded from required comprehension.
- Color never acts alone.
- The product remains fully usable with reduced motion and at 200% UI scaling.

## Explicit anti-patterns

The following do not belong in the Lazoo system:

- generic SaaS metric-card dashboards;
- default shadcn visual styling;
- purple gradients used merely to signal “AI”;
- paper textures, editorial hero headings, or presentation-like whitespace;
- full Fluent styling that makes the product resemble Microsoft Settings;
- glass on every surface;
- thick shadows around grounded panels;
- pill-shaped containers for ordinary rows and metadata;
- `transition: all`;
- pointer cursors on desktop buttons and selectable rows;
- status communicated only by colored dots;
- remote fonts, icons, or other CDN assets in the packaged application.

## Implementation boundary

This design-system slice creates research, decisions, and starter tokens only. It does not:

- import the token CSS;
- restyle or restructure the current shell;
- add React, React Aria, Radix, Motion, or another frontend dependency;
- expand Tauri capabilities;
- alter the daemon/backend contract;
- claim that later employee, provider, task, or approval workflows are implemented.

After owner approval, a separate implementation plan should migrate the shell to the selected frontend foundation, implement primitives and focus behavior, and redesign the Control Room against this specification. The first visual acceptance should compare the real Tauri window at default, minimum, and high-DPI sizes—not a browser-only mockup.

## Verification required for implementation

The eventual implementation is not complete until all of these are demonstrated:

1. Every component consumes semantic tokens rather than literal product colors.
2. Default, hover, focus, active, selected, disabled, loading, empty, disconnected, and error states are visibly distinct.
3. `Ctrl+K`, action-panel routing, arrow selection, primary/secondary actions, dismissal, and focus restoration work in the real Tauri window.
4. No critical state depends on color alone.
5. Motion is smooth at normal settings and absent where required under reduced motion.
6. No layout flashes, clipped overlays, missing focus rings, or remote runtime assets appear.
7. The shell remains legible at the minimum supported window size and at 200% UI scaling.
8. Provider and agent status wording remains truthful to the backend's verified state.

## Sources

Primary sources:

- [Zed UI spacing](https://github.com/zed-industries/zed/blob/956a49e4ca8aa4b7c2c293e1414c91f009824ae3/crates/ui/src/styles/spacing.rs)
- [Zed dynamic spacing macro](https://github.com/zed-industries/zed/blob/956a49e4ca8aa4b7c2c293e1414c91f009824ae3/crates/ui_macros/src/dynamic_spacing.rs)
- [Zed typography](https://github.com/zed-industries/zed/blob/956a49e4ca8aa4b7c2c293e1414c91f009824ae3/crates/ui/src/styles/typography.rs)
- [Zed elevation](https://github.com/zed-industries/zed/blob/956a49e4ca8aa4b7c2c293e1414c91f009824ae3/crates/ui/src/styles/elevation.rs)
- [Zed animation](https://github.com/zed-industries/zed/blob/956a49e4ca8aa4b7c2c293e1414c91f009824ae3/crates/ui/src/styles/animation.rs)
- [Zed theme color roles](https://github.com/zed-industries/zed/blob/956a49e4ca8aa4b7c2c293e1414c91f009824ae3/crates/theme/src/styles/colors.rs)
- [Zed default settings](https://github.com/zed-industries/zed/blob/956a49e4ca8aa4b7c2c293e1414c91f009824ae3/assets/settings/default.json)
- [Zed Theme Builder case study](https://zed.dev/blog/theme-builder)
- [Zed Settings Editor case study](https://zed.dev/blog/settings-ui)
- [Raycast design refresh](https://www.raycast.com/blog/a-fresh-look-and-feel)
- [Raycast current redesign](https://www.raycast.com/blog/the-new-raycast)
- [Raycast cross-platform technical case study](https://www.raycast.com/blog/a-technical-deep-dive-into-the-new-raycast)
- [Raycast themes](https://manual.raycast.com/themes)
- [Raycast UI reference](https://developers.raycast.com/api-reference/user-interface)
- [Raycast Action Panel](https://developers.raycast.com/api-reference/user-interface/action-panel)

Secondary cross-checks, not authoritative product specifications:

- [Independent Raycast marketing-system analysis](https://www.shadcn.io/design/raycast)
- [Community-maintained Raycast design notes](https://github.com/Khalidabdi1/design-ai/blob/main/design-md/raycast/DESIGN.md)
