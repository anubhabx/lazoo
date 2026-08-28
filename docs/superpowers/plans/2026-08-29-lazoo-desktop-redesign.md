# Lazoo Desktop Redesign Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace the current paper-styled read-only Tauri shell with the approved dark Lazoo design system, a native-feeling custom window frame, accessible command-first interaction, and polished operational views while preserving the existing snapshot-only backend boundary.

**Architecture:** Keep `load_desktop_snapshot` as the only product-data command and move the webview to a React 19 + TypeScript application compiled by Vite into a checked-in `ui/dist` runtime payload. React Aria Components owns focus, dialog, listbox, button, and tooltip behavior; local CSS consumes the canonical Lazoo semantic tokens; Tauri's core window API owns only minimize, maximize/restore, drag, and close. A small dependency-injected bridge isolates Tauri calls from presentation code and makes browser-level tests deterministic.

**Tech Stack:** Tauri 2.11, React 19, TypeScript 6, Vite 8, React Aria Components 1.20, Lucide React, Vitest, Testing Library, axe-core, CSS custom properties, bundled Inter Variable and IBM Plex Mono fonts.

---

## Delivery boundary

- Preserve the Rust daemon, IPC, protocol, store, and `DesktopSnapshot` contract.
- Preserve the 10-second visible-window refresh and last-good-snapshot behavior.
- Add only Tauri core window permissions required by the custom titlebar; add no filesystem, shell, process, HTTP, clipboard, registration, or shutdown capability.
- Implement only truthful read-only navigation and actions: refresh, navigate among snapshot views, inspect a project or event, copy nothing, mutate nothing.
- Treat `design-system/tokens.css` as the canonical token source. Product CSS may add layout aliases but no literal product colors.
- Bundle all fonts, icons, JavaScript, and CSS locally. No runtime CDN or remote asset is allowed.
- Keep generated `ui/dist` committed because `cargo build` embeds the webview payload directly and must remain reproducible without an implicit Node build inside Cargo.

## Target frontend structure

```text
apps/lazoo-desktop/ui/
├── index.html
├── package.json
├── package-lock.json
├── tsconfig.json
├── tsconfig.app.json
├── tsconfig.node.json
├── vite.config.ts
├── dist/                         # checked-in packaged webview payload
└── src/
    ├── main.tsx
    ├── App.tsx
    ├── app.test.tsx
    ├── bridge.ts
    ├── bridge.test.ts
    ├── model.ts
    ├── model.test.ts
    ├── test/setup.ts
    ├── components/
    │   ├── CommandPalette.tsx
    │   ├── Inspector.tsx
    │   ├── Navigation.tsx
    │   ├── OperationalRows.tsx
    │   ├── ShellChrome.tsx
    │   └── primitives.tsx
    └── styles/
        ├── app.css
        └── contract.test.ts
```

### Task 1: Establish the tested frontend and Tauri packaging boundary

**Files:**

- Create: `apps/lazoo-desktop/ui/package.json`
- Create: `apps/lazoo-desktop/ui/package-lock.json`
- Create: `apps/lazoo-desktop/ui/tsconfig.json`
- Create: `apps/lazoo-desktop/ui/tsconfig.app.json`
- Create: `apps/lazoo-desktop/ui/tsconfig.node.json`
- Create: `apps/lazoo-desktop/ui/vite.config.ts`
- Create: `apps/lazoo-desktop/ui/src/main.tsx`
- Create: `apps/lazoo-desktop/ui/src/App.tsx`
- Create: `apps/lazoo-desktop/ui/src/test/setup.ts`
- Modify: `apps/lazoo-desktop/ui/index.html`
- Delete after replacement: `apps/lazoo-desktop/ui/app.js`
- Delete after replacement: `apps/lazoo-desktop/ui/styles.css`
- Delete after replacement: `apps/lazoo-desktop/ui/view-model.js`
- Delete after replacement: `apps/lazoo-desktop/ui/view-model.test.mjs`
- Modify: `apps/lazoo-desktop/tauri.conf.json`
- Modify: `apps/lazoo-desktop/capabilities/main.json`
- Modify: `apps/lazoo-desktop/tests/config.rs`
- Create: `apps/lazoo-desktop/ui/dist/`

- [x] Extend `apps/lazoo-desktop/tests/config.rs` first so it requires `frontendDist: ui/dist`, a local Vite hook, `decorations: false`, dark theme, the existing minimum size, `withGlobalTauri: false`, and exactly the snapshot plus approved core-window permissions.
- [x] Run `cargo test -p lazoo-desktop --test config tauri_configuration_is_static_local_and_single_windowed main_capability_allows_only_core_window_and_snapshot_commands` and confirm RED because the current config still points at `ui`, uses native decorations, exposes the global API, and lacks window permissions.
- [x] Add exact frontend dependencies and scripts. Use exact versions and commit the npm lockfile. Scripts must include `dev`, `build`, `typecheck`, `test`, `test:watch`, `format`, `format:check`, and `check`.
- [x] Configure Vite with `base: './'`, `build.outDir: 'dist'`, source maps disabled, a fixed localhost development port, `server.strictPort: true`, and a workspace allow-list that permits importing `design-system/tokens.css` without exposing paths above the repository root.
- [x] Configure Vitest for `jsdom`, the shared setup file, restored mocks, and CSS enabled. Add missing DOM shims only when a failing test demonstrates the need.
- [x] Replace `index.html` with a minimal dark boot document containing `#root`, a strict module entry, dark color scheme metadata, and no inline style/script or remote resource.
- [x] Add the smallest React boot surface, import local font packages and canonical `design-system/tokens.css`, and build `ui/dist`.
- [x] Update Tauri to package `ui/dist`, use Vite hooks for `tauri dev/build`, disable decorations, set dark theme, and disable `withGlobalTauri`.
- [x] Grant only `core:window:default`, close, minimize, toggle-maximize, start-dragging, is-maximized, and the generated snapshot command.
- [x] Run the focused Rust configuration tests and confirm GREEN.
- [x] Run `npm --prefix apps/lazoo-desktop/ui run check` and confirm typecheck, Vitest, formatting, and production build pass.
- [x] Run `cargo build -p lazoo-desktop` and confirm `target/debug/lazoo-desktop.exe` is emitted from the checked-in `dist` payload.
- [x] Commit: `build: add React desktop frontend pipeline`

### Task 2: Port and strengthen the presentation model

**Files:**

- Create: `apps/lazoo-desktop/ui/src/model.ts`
- Create: `apps/lazoo-desktop/ui/src/model.test.ts`
- Create: `apps/lazoo-desktop/ui/src/bridge.ts`
- Create: `apps/lazoo-desktop/ui/src/bridge.test.ts`

- [x] Write model tests first for the existing healthy, degraded, recovered-incident, empty, malformed-time, checkout-label, event-label, bounded-summary, and newest-event-first behavior.
- [x] Add new tests for explicit connection states (`loading`, `connected`, `degraded`, `offline`), last-good-snapshot retention, command filtering/grouping, selection descriptors, and path middle-truncation data.
- [x] Run `npm --prefix apps/lazoo-desktop/ui test -- src/model.test.ts` and confirm RED because the TypeScript model does not exist.
- [x] Implement strict TypeScript DTOs and pure transformations. Keep unknown provider/employee concepts out of the model because the backend does not return them.
- [x] Run the focused model suite and confirm GREEN.
- [x] Write bridge tests first for invoking only `load_desktop_snapshot`, rejecting a missing Tauri runtime with `LZ-DESKTOP-BRIDGE-MISSING`, and forwarding close/minimize/toggle-maximize/start-dragging/is-maximized through an injected window adapter.
- [x] Run `npm --prefix apps/lazoo-desktop/ui test -- src/bridge.test.ts` and confirm RED because the bridge adapter does not exist.
- [x] Implement the narrow bridge using `@tauri-apps/api/core` and `@tauri-apps/api/window`; export dependency-injection seams rather than test-only production methods.
- [x] Run both focused suites and confirm GREEN.
- [x] Commit: `feat: add typed desktop presentation model`

### Task 3: Build the token-driven shell and native window chrome

**Files:**

- Create: `apps/lazoo-desktop/ui/src/components/primitives.tsx`
- Create: `apps/lazoo-desktop/ui/src/components/ShellChrome.tsx`
- Create: `apps/lazoo-desktop/ui/src/components/Navigation.tsx`
- Create: `apps/lazoo-desktop/ui/src/styles/app.css`
- Create: `apps/lazoo-desktop/ui/src/styles/contract.test.ts`
- Modify: `apps/lazoo-desktop/ui/src/App.tsx`
- Modify: `apps/lazoo-desktop/ui/src/app.test.tsx`

- [ ] Write a CSS contract test first that rejects literal hex/rgb/hsl/oklch product colors, `transition: all`, remote URLs, mixed emoji icons, and pointer cursors on buttons or selectable rows in `src`.
- [ ] Run the contract test and confirm RED because `app.css` and the token import contract are absent.
- [ ] Write component tests first for the 40px titlebar landmarks, accessible window-control names, 208px navigation labels, selected navigation state, 28px status bar, loading layout, and minimum-shell structure.
- [ ] Run the focused component tests and confirm RED because the shell components do not exist.
- [ ] Implement reusable React Aria Button and Tooltip primitives with visible `:focus-visible`, disabled, pressed, hover, and loading states.
- [ ] Implement the custom draggable titlebar, brand mark, current view label, command trigger, compact connection state, and Windows minimize/maximize/close controls. Prevent drag initiation from controls.
- [ ] Implement Control room, Projects, Activity, and System navigation as read-only client views; use one Lucide outline family and label future functionality nowhere as available.
- [ ] Implement the status bar with daemon truth, refresh age, project/event counts, and local-pipe wording.
- [ ] Style the shell from canonical semantic tokens: graphite surface hierarchy, ember current-location/focus accent, 1px borders, Zed spacing/type dimensions, restrained hover, and no grounded-panel shadows.
- [ ] Collapse the navigation to labeled tooltips at the minimum-width breakpoint; retain accessible names and keyboard reachability.
- [ ] Run the contract and component tests and confirm GREEN.
- [ ] Run `npm --prefix apps/lazoo-desktop/ui run build`, inspect `dist/index.html`, and confirm all runtime resources are local.
- [ ] Commit: `feat: build Lazoo dark desktop shell`

### Task 4: Implement operational views, states, and inspector

**Files:**

- Create: `apps/lazoo-desktop/ui/src/components/OperationalRows.tsx`
- Create: `apps/lazoo-desktop/ui/src/components/Inspector.tsx`
- Modify: `apps/lazoo-desktop/ui/src/App.tsx`
- Modify: `apps/lazoo-desktop/ui/src/app.test.tsx`
- Modify: `apps/lazoo-desktop/ui/src/styles/app.css`

- [ ] Write integration tests first for the initial reserved loading rows, healthy snapshot, degraded snapshot, disconnected-without-data state, failure-after-success retention, manual refresh busy state, empty projects/events, view navigation, project selection, event selection, inspector dismissal, and focus return.
- [ ] Run the focused app suite and confirm RED on the missing operational surfaces.
- [ ] Implement a stable loading skeleton that reserves the final list geometry and never swaps the whole shell.
- [ ] Implement the Control Room as a prioritized operational summary: health line, compact metric strip, registered-project rows, and recent-event rows. Keep `Control plane steady`, the project name, and event label accessible for retained Windows acceptance.
- [ ] Implement dedicated Projects, Activity, and System views from the same snapshot model; no extra backend calls.
- [ ] Implement 32px React Aria selectable rows with arrow-key navigation, neutral selection, explicit text/icon status, and tabular times/counts.
- [ ] Implement the 320px inspector for selected projects/events/system facts. At narrow width it overlays the workspace without resizing navigation. Closing it restores focus to the originating row.
- [ ] Implement offline/error banners with error code, affected surface, retained-data wording, and one retry action. Preserve the last successful snapshot after later failures.
- [ ] Refresh on initial mount, every ten seconds only while visible, on visibility restoration, and on manual retry; prevent concurrent refreshes.
- [ ] Run the app suite and confirm GREEN.
- [ ] Run axe-core against loading, healthy, empty, degraded, and disconnected renders; assert zero critical or serious violations.
- [ ] Commit: `feat: redesign desktop operational views`

### Task 5: Implement the Raycast-style command and action flow

**Files:**

- Create: `apps/lazoo-desktop/ui/src/components/CommandPalette.tsx`
- Modify: `apps/lazoo-desktop/ui/src/App.tsx`
- Modify: `apps/lazoo-desktop/ui/src/app.test.tsx`
- Modify: `apps/lazoo-desktop/ui/src/styles/app.css`

- [ ] Write interaction tests first for `Ctrl+K`, immediate search focus, context-ranked groups, query filtering, arrow selection, `Enter` primary action, `Ctrl+Enter` secondary inspect action where available, `Escape` dismissal, outside dismissal, and focus restoration to the titlebar trigger or originating row.
- [ ] Write tests for the bottom contextual action bar changing with the active view/selection and collapsing when no secondary action exists.
- [ ] Run the focused interaction tests and confirm RED because the command surface is missing.
- [ ] Implement a controlled React Aria modal dialog with a 44px search field, 40px ListBox rows, 720px maximum width, 560px maximum height, intent groups, stable empty/loading results, and no clipped overflow.
- [ ] Rank contextual inspect/refresh actions first when a row is selected while keeping global navigation searchable.
- [ ] Implement only real actions: navigate, refresh, select/inspect, dismiss. Destructive or unavailable mutation actions must not appear.
- [ ] Implement the 36px contextual action bar with location on the left and truthful primary/secondary shortcuts on the right.
- [ ] Use opacity/transform transitions only: 150ms micro states, 220ms overlay, 8px maximum travel, no spring or overshoot.
- [ ] Confirm reduced-motion tokens remove nonessential duration and translation.
- [ ] Run the interaction suite and confirm GREEN.
- [ ] Commit: `feat: add desktop command and action flow`

### Task 6: Strengthen real-window acceptance and responsive visual evidence

**Files:**

- Modify: `scripts/acceptance/desktop-shell.ps1`
- Modify: `apps/lazoo-desktop/tests/config.rs`
- Modify: `apps/lazoo-desktop/ui/src/app.test.tsx`
- Modify: `apps/lazoo-desktop/ui/src/styles/app.css`
- Regenerate: `artifacts/acceptance/desktop-shell-manifest.json`

- [ ] Extend acceptance-contract tests first to require a frontend build gate, local-runtime-asset scan, custom-titlebar UI Automation names, command-palette keyboard exercise, inspector focus round-trip, default 1180×760 capture, minimum 860×620 capture, and a high-DPI capture at Windows 200% text scaling or an explicit environment-bound blocker.
- [ ] Run the focused Rust acceptance-contract test and confirm RED because the script lacks the new gates.
- [ ] Update the PowerShell harness without weakening its scoped process cleanup. Run `npm ci` and the frontend build before the locked Rust release build unless `-SkipBuild` is used.
- [ ] Add keyboard input through Windows UI Automation for `Ctrl+K`, verify command text is exposed, dismiss with Escape, open a project inspector, close it, and verify focus returns.
- [ ] Retain exact title, nonzero HWND, snapshot text, close isolation, and screenshot checks from the original harness.
- [ ] Add static checks against remote runtime assets, inline code, and missing packaged files.
- [ ] Run `cargo test -p lazoo-desktop --test config` and confirm GREEN.
- [ ] Run the updated desktop acceptance once with a fresh locked build and retain its exact manifest and screenshot paths.
- [ ] Inspect default, minimum, and high-DPI evidence at original resolution. Fix clipping, weak hierarchy, inconsistent spacing, overlay cutoff, focus visibility, and scroll failures through failing regression tests before CSS changes.
- [ ] Commit: `test: verify redesigned desktop interactions`

### Task 7: Full verification, documentation synchronization, and audit

**Files:**

- Modify: `README.md`
- Modify: `docs/PROJECT-BRIEF.md`
- Modify: `docs/superpowers/specs/2026-08-29-lazoo-desktop-design-system.md`
- Modify: `docs/superpowers/plans/2026-08-29-lazoo-desktop-redesign.md`
- Create: `docs/verification/2026-08-29-lazoo-desktop-redesign.md`

- [ ] Update user/developer commands for `npm ci`, frontend tests/build, `tauri dev`, Cargo, and desktop acceptance.
- [ ] Mark the design spec Implemented only after every non-environment-bound gate passes. Keep the backend boundary and later-slice exclusions explicit.
- [ ] Record exact dependency/license choices, test counts, command exit codes, acceptance manifest gates, screenshot sizes/paths, visual observations, and any genuine blocked high-DPI/manual gate.
- [ ] Run `powershell -NoProfile -ExecutionPolicy Bypass -File scripts/verify-rust-toolchain.ps1`.
- [ ] Run `npm --prefix apps/lazoo-desktop/ui ci` followed by `npm --prefix apps/lazoo-desktop/ui run check`.
- [ ] Run `cargo test --workspace`.
- [ ] Run `cargo fmt --all -- --check`.
- [ ] Run `cargo clippy --workspace --all-targets -- -D warnings`.
- [ ] Run `cargo check --workspace --all-targets`.
- [ ] Run `cargo build --workspace --release --locked` after the fresh frontend build.
- [ ] Run `powershell -NoProfile -ExecutionPolicy Bypass -File scripts/acceptance/first-slice.ps1 -SkipBuild`.
- [ ] Run `powershell -NoProfile -ExecutionPolicy Bypass -File scripts/acceptance/desktop-shell.ps1 -SkipBuild`.
- [ ] Run `git diff --check` and a source/runtime scan for inline scripts, remote assets, unsafe HTML injection, mixed icon systems, forbidden capability expansion, literal component colors, and `transition: all`.
- [ ] Compare the implementation line by line with `docs/superpowers/specs/2026-08-29-lazoo-desktop-design-system.md`; confirm every required state and interaction has evidence and every exclusion remains excluded.
- [ ] Scan the plan, implementation, and verification notes for unfinished markers and unqualified future claims.
- [ ] Update every task checkbox and the execution status in this plan to match reality.
- [ ] Commit: `docs: verify Lazoo desktop redesign`

## Completion criteria

The redesign is complete only when the checked-in `ui/dist` opens in the real Tauri window; the default, minimum, and available high-DPI layouts have inspected evidence; every applicable frontend/Rust/acceptance gate passes; `Ctrl+K` and inspector focus behavior work through the real window; no remote runtime asset or unauthorized Tauri capability is present; and the documentation reports exact evidence without implying unimplemented provider, employee, task, approval, terminal, or mutation support.
