# Lazoo desktop design system

Version 1.0 (2026-11-25), implemented in `apps/lazoo-desktop/ui`. This is the contributor spec: read it before adding UI. Every value below comes from `tokens.css`, `components.css` or `primitives.tsx`; if the code and this file disagree, the code wins and this file is a bug. `reference.html` shows every token and primitive rendered.

## Principles

1. **Hierarchy over options.** Each view has one primary action and a clear order of reading; secondary actions go in a menu or ghost button.
2. **Structure over clutter.** Hairlines, alignment and a shared row height separate things; boxes inside boxes and decoration do not.
3. **Breathing space over flash.** Use the spacing scale generously and let content sit at the top-left of a quiet page instead of filling it.
4. **Smooth over decorative.** Motion confirms an action in 60 to 200 ms; it never entertains, and nothing a keyboard triggers animates.
5. **Sharp and professional over friendly.** Small radii, flat surfaces, plain words and no exclamation marks or emoji.

## Brand

- The mark is an L and a cursor block on a 16-unit grid: the L is ink, the cursor is iris. Assets live in `brand/`; `brand/README.md` lists each file.
- `lazoo-mark.svg` is the tile version (icon, favicon). The `glyph` files are the mark without a tile, one per surface; the `lockup` files add the wordmark.
- The wordmark is IBM Plex Mono SemiBold, outlined to paths so no font is needed.
- Clear space is 2 grid units on every edge. Minimum size is 16 px.
- Colours: ink `#e8ecf1`, tile `#171c24`, iris `#8d9af4` (`#5a69d6` on light surfaces).
- Do not recolour, rotate, stretch or add effects.
- The in-app `LogoMark` (`components/Logo.tsx`) blinks its cursor only while an agent is working, in hard steps; it holds still under reduced motion.
- The app icon is drawn by `apps/lazoo-desktop/build.rs` from the same geometry, one nearest-pixel rendering per size.

## Colour

Components use semantic roles (`--lz-color-*`, `--lz-status-*`, `--lz-provider-*`), never the slate or iris primitives, except when defining a new role.

**Surface ladder**, darkest to lightest:

| Token | Value | Use |
| --- | --- | --- |
| `--lz-color-canvas` | slate-000 `#0f1218` | The stage behind a native terminal view, so a TUI sits edge to edge. |
| `--lz-color-window` | slate-050 `#13171e` | App chrome, the page background and input fill. |
| `--lz-color-background` | slate-100 `#171c24` | Editor and code areas, the brand tile. |
| `--lz-color-surface` | slate-150 `#1b2029` | Cards, tables, lists, dialogs: one step above the page. |
| `--lz-color-surface-raised`, `-overlay` | slate-200 `#20262f` | Floating layers: menus and toasts. |

Interaction washes sit on any surface: hover `--lz-color-surface-hover` (5%), active and selected `--lz-color-surface-active` and `-selected` (8.5%).

**Text roles:** `text-primary` for titles, values and control labels; `text-secondary` for body copy in rows; `text-muted` for descriptions, metadata and column heads; `text-disabled` for placeholders and disabled controls; `text-inverse` on the primary button; `text-accent` for links. Borders: `border-subtle` (7%) between rows, `border` (11%) around groups and controls, `border-strong` (18%) on hover.

**Iris is the only accent.** It may mark focus, the current selection, "needs you", progress and checked controls. It is never a button fill and never decoration. The primary action is quiet ink (`--lz-color-primary`) on slate.

**Status** roles (`--lz-status-running`, `healthy`, `completed`, `attention`, `waiting`, `needs-input`, `blocked`, `failed`, `paused`, `dormant`, `detected`, `unverified`, `degraded`, `configured`) are always paired with a word or an icon; colour is never the only signal. **Harnesses** keep one hue everywhere (`--lz-provider-claude-code` lilac, `codex` teal, `opencode` sky, `antigravity` mint, `github-copilot` peach, `grok-build` sand).

## Typography

Inter Variable carries the UI, with the features in `--lz-font-features`. IBM Plex Mono (`--lz-font-technical`) is for technical strings only: paths, ids, commands, key caps, counts in badges. Never use it for prose or headings. Weights are 400, 500 (labels, buttons) and 600 (titles).

| Token | Size / line | Use |
| --- | --- | --- |
| `--lz-text-xs` | 11 / 14 | Table heads (uppercase, `--lz-tracking-label`), group labels, mono metadata |
| `--lz-text-sm` | 12 / 16 | Descriptions, hints, field labels, small controls |
| `--lz-text-md` | 13 / 19 | Body text and default controls |
| `--lz-text-lg` | 15 / 22 | Dialog titles |
| `--lz-headline-sm` | 17 / 24 | Reserved for sub-headings in long documents |
| `--lz-headline-page` | 20 / 28 | Page titles (`PageHeader`), with `--lz-tracking-headline` |
| `--lz-headline-md` | 22 / 28 | Large in-view headings |
| `--lz-headline-lg` | 28 / 34 | Display text, rare |

## Space and layout

Everything sits on a 4 px grid. Spacing tokens: `--lz-space-04`, `06`, `08`, `10`, `12`, `16`, `20`, `24`, `32`, `40`, `48` (plus 00, 01, 02 and 03 for hairline adjustments). Rows and controls use the heights `--lz-control-h-xs/sm/md/lg` (20, 24, 28, 32) and `--lz-row-h-*` (28, 28, 32, 36); list rows are at least 52 px (`--row-min`) and table rows 44 px.

The page frame is the `Page` primitive. It pads by `--lz-page-pad` (32 px) and holds one column of at most `--lz-page-max` (920 px), left-aligned, so every page title sits at the same x and y. Running text inside it is capped at `--lz-prose-max` (68ch). A page may go full width (`<Page wide>`) only when its main content is a wide table or a canvas that needs the room; the header stays left-aligned either way. Shell geometry: title bar 40 px, navigation 248 px, inspector 320 px, command palette 640 px.

## Shape and depth

Radii are sharp: `--lz-radius-xs` 2 px (key caps, badges), `sm` 3 px (small controls, switch, tooltips), `md` 4 px (buttons, inputs, tiles), `xl` 6 px (cards, tables, menus, dialogs, toasts), `3xl` 8 px at most. Groups are separated by 1 px hairlines (`--lz-border-hairline`), not by fills or gaps. The page has no shadows: `--lz-shadow-raised` is `none`. Only floating layers cast one: `--lz-shadow-overlay` for menus and toasts, `--lz-shadow-modal` for dialogs, over `--lz-color-scrim`. The one decorative device is ordered (Bayer) dithering with a 2 px cell (`--lz-dither-cell`): agent avatars (`lib/dither.ts`), skeletons, meter reserve, the empty-state tile.

## Motion

| Token | Value | Use |
| --- | --- | --- |
| `--lz-duration-instant` | 60 ms | Press scale (`--lz-press-scale`, 0.98) |
| `--lz-duration-fast` | 120 ms | Hover, colour, border |
| `--lz-duration-moderate` | 160 ms | Panels and fades |
| `--lz-duration-slow` | 200 ms | Mode and dialog changes |

Easing is `--lz-ease-out` (`cubic-bezier(0.22, 1, 0.36, 1)`) by default; `--lz-ease-in-out`, `--lz-ease-standard` and `--lz-ease-in` exist for the rare other case. Moving surfaces use the spring in `primitives.tsx` (`SPRING`, no overshoot). Animate only colour, opacity and transform, by at most `--lz-motion-micro` (4 px) or `--lz-motion-panel` (8 px). Nothing triggered by the keyboard animates. Under `prefers-reduced-motion` the tokens collapse the durations and distances to 0 and the press scale to 1; ambient animation (spinner, skeleton, logo cursor) is switched off.

## Components

`primitives.tsx` holds the React primitives; `components.css` holds their look. Pages compose them and add layout only (`app.css`).

| Primitive | For | Rules |
| --- | --- | --- |
| `Button` | Any action | Variants `default`, `primary`, `ghost`, `danger`, `accent`; sizes `sm`, `md`, `lg`. `icon` needs `tip` or `aria-label`; `loading` disables it and shows a `Spinner`. |
| `Spinner` | Work in progress inside a control | 12 px by default; stops under reduced motion. |
| `Kbd` | A shortcut hint | One key per `kbd`; hidden from assistive tech. |
| `Dot`, `StatusWord` | Operational state | A square 6 px dot, never alone: `StatusWord` always carries the word. |
| `Badge` | A short technical tag | Tones `neutral`, `accent`, `success`, `warning`, `danger`; mono, 18 px high. |
| `Avatar` (`AgentAvatar`) | An agent | Dithered glyph seeded from the name, hue from its harness; state animates the dither only. |
| `ProjectGlyph`, `glyphHue` | A project | A tinted tile with the initial; the hue is stable per name. |
| `Field` | Label, control and hint | Wraps `.input`, `.select` or `.textarea`; label above, hint below. |
| `Segmented` | A small exclusive choice | Two to five options; the thumb moves as a shared-layout spring. |
| `Switch` | An immediate on/off | Takes effect at once; use a checkbox when a form is saved later. |
| `Skeleton`, `TableSkeleton` | Loading | Dithered, not shimmering; same card and row height as the real content. |
| `Empty` | Nothing here yet | Top-left; a 32 px dithered tile, a title, one muted line, at most one action. `compact` inside a section. |
| `Page`, `PageHeader`, `Section` | Page structure | One `Page` per route, one `PageHeader` with the h1 and its actions, then `Section`s 32 px apart. |
| `Dialog` | A modal task | Traps focus, closes on Escape and scrim, returns focus; footer holds the actions. |
| `Menu`, `MenuItem` | Secondary actions | Opens from an icon button; dangerous items are marked `danger`. |
| `EASE`, `SPRING`, `fade`, `fadeFast`, `rise` | Motion constants | Use these instead of new curves. |

Composition rules:

- One primary button per view, top right of the `PageHeader` or in the dialog footer. Everything else is `default`, `ghost` or in a `Menu`.
- Cards, tables, lists and account rows share one treatment: a 1 px `--lz-color-border`, `--lz-radius-xl` and `--lz-color-surface`. Rows divide with `--lz-color-border-subtle`, pad 12 px by 16 px and are at least 52 px high. Do not nest cards.
- Selected state is a wash (`--lz-color-surface-selected`), plus a 2 px iris marker on the left of a selectable row. Never fill a selection with iris.
- Empty states sit top-left in the page column, never centred.
- Focus is always visible: a 2 px iris outline with 1 px offset on every keyboard stop (`:focus-visible`); text fields use an iris border and `--lz-shadow-focus`. Do not remove it.
- Buttons and rows keep the arrow cursor; pointer cursors are for links.

## Files and workflow

- `tokens.css` is the source. `tokens.json` is generated: run `node design-system/build-tokens.mjs` after any change and commit both.
- `apps/lazoo-desktop/ui/src/styles/components.css` styles the primitives, `app.css` holds the shell and page layout, and `primitives.tsx` is the React side.
- To add a token: add it to `tokens.css` in the right group with a comment, regenerate `tokens.json`, then use it by role. Add a primitive-level value to the file that owns it, not to a page.
- To add a component: build it from existing tokens, put markup in `primitives.tsx` and styles in `components.css`, cover hover, press, focus and disabled, and add a specimen to `reference.html`.
- `reference.html` is a static page that links `tokens.css` and `components.css`; open it in a browser from disk to review changes. It needs no build step.
