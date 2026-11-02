# Desktop: native first, hierarchy, dithered identity

**Date:** 2026-11-03 · **Branch:** `feat/desktop-ui-native-first` (from `main` 4fc9527) · Supersedes the visual language of `2026-11-01-desktop-ui-refresh-design.md` (slate palette, soft radii, glows) and the pane model of `2026-10-31-desktop-ui-pilot-design.md` (Native / Transcript toggle). Reads, events, Tauri boundary and terminal budget are unchanged.

## 1. What the owner asked for

> I want hierarchy over extra options; cleanliness and structure over a cluttered, overcrowded UI; breathing space over extra flashiness; smoothness over deep design; sharpness and professionalism over being too friendly.

Plus: the conversation must be native first; the transcript UI is bad and its whole premise needs a feasibility call; multi-account routing needs management, usage, analytics and routing preferences; every agent gets a unique avatar with thinking and working animations, dithered, and dithering may carry through the app.

## 2. Who uses this

Lazoo's user runs several coding-agent harnesses at once (Claude Code, Codex, Copilot, OpenCode, Antigravity) on Windows. They already live in those TUIs and know them by heart; they adopted Lazoo to run a team of them, not to get a new chat app. What follows from that:

| They are | So the UI |
|---|---|
| Terminal natives, keyboard first | shows the genuine TUI as the work surface; every action has a key; nothing animates on a keyboard action |
| Watching several agents at once | answers "who is working, who needs me, what is queued" at a glance, in one row, without reading prose |
| Spending real money and quota | shows which account each agent burns and how close each account is to its limit |
| Running long sessions | uses low-glare surfaces, mono type for machine facts, no ambient motion |
| Experts who distrust magic | states facts plainly (what is observed, what is not), never greets them, never uses sparkles |

## 3. Principles, made concrete

| Principle | Rule in this design |
|---|---|
| Hierarchy over options | One primary action per surface. Everything else is in a menu, the details panel or the palette. The conversation has exactly one header row. |
| Structure over clutter | One 4 px grid, rows 28 / 32 px, one gutter (24 px), aligned left edges. A surface has a title, its content, and at most one footer. No cards inside cards. |
| Breathing space over flash | No glows, gradients, halos, sparkle icons, greetings or ambient drift. Space separates groups; hairlines separate regions. |
| Smoothness over deep design | Motion only where it explains a change: 120–160 ms, transform and opacity, CSS transitions. Route changes, tab switches, palette open and list updates are instant. |
| Sharp over friendly | Radii 2 px (controls) and 4 px (panels, dialogs); hairline borders; no drop shadows except on floating layers; sentence-case copy that states facts. |

## 4. The transcript, decided

Lazoo cannot faithfully re-render what a harness is doing, so it will not try. Evidence (code map, 2026-11-03):

- **Claude Code** governed turns run its interactive TUI over ConPTY with hooks. The structured feed is: prompt accepted, assistant text (MessageDisplay), tool name and status (Pre/PostToolUse), stop. There is no thinking, no command output, no exit code, no diff, no subagent or background-shell event. MessageDisplay batches must arrive in strict order or the turn ends uncertain.
- **Codex** (app-server) gives text, reasoning, command output deltas and item type/status; the command string, exit code, diffs, MCP arguments and results, and plans are dropped or reduced to generic progress.
- **Copilot (ACP)** gives text, thoughts, tool title/status and usage; arguments, diffs and plans are only in raw frames.
- **OpenCode** gives text only. **Antigravity** gives tool status and one text block at stop.
- No harness gets tool arguments or results into the persisted turn record.

Rendering a credible activity stream would mean, per harness, new typed events for commands, diffs, MCP calls, plans, subagents and background shells, a larger persisted payload, and then chasing every harness release that changes its own UI. That is five products, always behind the real ones. The genuine TUI already renders all of it, correctly, today.

**Decision.** The conversation is the native TUI, edge to edge. The Native / Transcript toggle and the "default pane" setting are removed. Lazoo renders only what it owns and knows for certain, the orchestration layer, as an **Activity** list in the details panel (Ctrl+I): your requests, each agent's final reply, delegations, worker results, approvals and failures. Tool calls are counted, not listed (the live run showed the old transcript was dominated by permission rows). If a harness later exposes a complete structured stream, Activity can grow per harness without changing the layout.

## 5. Conversation layout

```
titlebar   ← →  project › conversation title                                   – ▢ ×
agent bar  [▦ mira  lead ◦working] [▦ rune] [▦ kit]          codex · gpt-5.4 · ask · Work   ⏹ ⋯ ▯
           ─────────────────────────────────────────────────────────────────────────────────
           genuine TUI (no frame, no overlay)                                    read-only
           ─────────────────────────────────────────────────────────────────────────────────
attention  ● rune wants to run cargo test --workspace                     Approve once  Decline
composer   Message mira                                                        Ask ▾   ↵
```

- The agent tabs are the header: dithered avatar (animated by state), name, role mark for the lead. The right side carries the active agent's machine facts in mono (harness, model, authority, account) and its contextual actions (cancel turn while busy, the rest in a menu).
- "Read-only" is a quiet label at the terminal's corner, not a pill over output. Closing and reopening the view live in the menu.
- Empty, opening, suspended and error states sit in the terminal area as one dithered field with one line of text and at most one action.
- Workers have no composer; their footer is their state reason.

## 6. Dither as identity

Ordered (Bayer 4×4) dithering is the app's one decorative device, used where texture carries meaning:

- **Agent avatars**: a 10×10 symmetric glyph seeded from the agent's name (shape = the agent), filled in its harness hue (colour = the harness), shaded by an ordered-dither ramp. States animate the dither threshold, never the layout: *working*, a diagonal sweep; *thinking* (starting, waiting on itself), a slow shimmer; *needs you*, an accent blink; *failed*, rose and still; *dormant*, dim and still. One shared 12 fps ticker drives every visible avatar; hidden ones cost nothing; reduced motion freezes them.
- **Fields**: empty and loading states of the terminal area, the launcher's top edge, and usage meters use dithered ramps instead of gradients or glows.
- Pixels stay crisp: canvases render at device resolution with integer cell edges; CSS patterns use `image-rendering: pixelated`.

## 7. Accounts, usage, routing

Settings → Accounts becomes the place to answer "which account pays for this, and how close is it to its limit":

- Accounts grouped by harness, in routing order. Each harness has one routing control: existing login, balanced, use in order (drain the first until it is exhausted or hits its reserve), pinned, or API pool.
- Each account row: label, credential kind, state (ready, cooling down, exhausted until, disabled), its observed usage windows as dithered meters (with the reserve marked), reserve, and its actions in a menu.
- A usage view: per account, sessions, turns, requests, failures and tokens over the last 7 days (day or hour buckets). What a harness does not report is labelled "not observed", never zero.
- The backend for priority order, reserves, exhaustion, observed windows and the usage ledger is a separate slice (Codex, `feat/account-routing-preferences`); the UI binds to its RPCs.

## 8. Not in scope

A light theme; a re-rendered transcript (section 4); writable native views (the backend refuses view input by design: prompts and approvals stay in Lazoo's controls).
