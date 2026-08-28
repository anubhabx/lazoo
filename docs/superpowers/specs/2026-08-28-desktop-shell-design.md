# Lazoo Desktop Shell Design

- **Status:** Approved for implementation by the owner's 2026-08-28 “Go ahead with the desktop shell” direction
- **Slice:** Minimal Stage 2 Tauri shell
- **Platform:** Windows 11, current local user, Tauri 2, WebView2

## Purpose

This slice gives the existing Lazoo control plane its first native desktop surface. It proves that a Tauri webview can read the authenticated daemon contract, present operational state clearly, survive daemon unavailability, and remain inside a deliberately narrow privilege boundary.

The result is a useful control-room view, not a simulated finished product. It shows daemon health, registered projects, recent events, and recovered emergency-incident count. It does not add employees, provider execution, tasks, terminals, approvals, settings, mutation controls, tray behavior, installers, or automatic daemon startup.

## Owner-facing experience

Launching `lazoo-desktop.exe` opens one resizable Windows window titled **Lazoo — Local control room**. The initial view is a loading state while the Rust backend contacts `lazood.exe` through the existing installation-scoped authenticated named pipe.

When connected, the window shows:

- a clear healthy or degraded daemon state;
- last refresh time and a manual refresh action;
- project count, event waterline, pending outbox count, and recovered emergency-incident count;
- registered projects with checkout mode and canonical path;
- a recent activity ledger with sequence, type, time, and concise payload summary;
- protocol, database journal mode, daemon version, boot identity, uptime, and data-root details.

The view refreshes every ten seconds while the document is visible. A refresh never erases the last successful snapshot before a replacement succeeds. When the daemon is unavailable, a persistent disconnected banner explains that the keeper/daemon must be started and offers retry. Empty project and activity states teach the next useful CLI command instead of presenting blank panels.

## Visual direction

The shell is a calm Windows-native operations cockpit for one technical owner. It uses warm paper neutrals, dark ink, safety orange for attention, and green only for confirmed health. Bahnschrift provides a compact identity for the Lazoo wordmark and operational labels; Segoe UI Variable provides native, legible body text.

The layout is intentionally asymmetric: the project and activity ledger occupy the primary reading column, while a narrow system pulse rail carries detailed diagnostics. Thin rules and typographic hierarchy replace repeated floating cards. Motion is limited to a short initial reveal, refresh rotation, and state transitions, all disabled by `prefers-reduced-motion`.

The design remains usable at the minimum 860 by 620 window size. At narrower widths, the diagnostic rail moves below the main ledger without hiding information.

## Architecture

### Protocol snapshot

`lazoo-protocol` gains a `DesktopSnapshot` response containing:

- `captured_at` as an RFC 3339 UTC timestamp;
- `SystemStatus`;
- `Vec<ProjectRecord>`;
- `Vec<EventSummary>`.

The new `desktop.snapshot` JSON-RPC method accepts an empty object. The daemon reads its store status, project list, and up to the 100 most recent events, then returns one response. It derives the recent-event cursor from the current event high-water mark so the method stays within the existing store API.

This additive method remains protocol version 1. It introduces no mutation and no subscription. Existing CLI methods remain unchanged.

### Desktop Rust boundary

`apps/lazoo-desktop` is a new workspace member and native executable. Its Rust client:

1. resolves the existing Lazoo data root;
2. loads the installation identity and DPAPI-protected nonce;
3. connects through `PipeClient` as `lazoo-desktop`;
4. sends one `desktop.snapshot` request;
5. rejects malformed result/error combinations; and
6. returns a typed `DesktopSnapshot` or a redacted `DesktopFailure` DTO.

The Tauri command `load_desktop_snapshot` is the only webview command registered by the application manifest. The main-window capability contains `core:default` plus the generated `allow-load-desktop-snapshot` permission and no filesystem, process, HTTP, clipboard, or shell plugin permission. The webview cannot supply a daemon method name or arbitrary parameters.

### Static frontend

Tauri serves a checked-in static `ui/` directory directly through `frontendDist`; there is no JavaScript framework, package installation, development server, or remote asset. `window.__TAURI__.core.invoke` calls the one registered command.

Pure formatting and view-model functions live in an ES module and are tested with the installed Node test runner. DOM rendering uses element creation and `textContent`; daemon payloads never enter `innerHTML`. The CSP permits only local application assets and Tauri IPC.

## Error and lifecycle behavior

- A missing or busy pipe becomes `LZ-DESKTOP-DAEMON-OFFLINE` with a short actionable message.
- Authentication, protocol, runtime, daemon RPC, and malformed-response errors keep their stable Lazoo code where available and expose no nonce or secret value.
- Refresh requests are serialized. A second refresh request while one is active is ignored.
- The previous successful snapshot remains visible under a disconnected banner after a later refresh fails.
- Closing the desktop window exits only the desktop process. It does not send `system.shutdown` and does not stop the keeper or daemon.
- The desktop does not start the keeper automatically in this slice; that lifecycle belongs to the installer/tray slice.

## Testing and acceptance

The slice uses inline red-green-refactor cycles for every new Rust behavior and every pure JavaScript formatter.

Automated gates must prove:

- protocol camel-case serialization and strict decoding for `DesktopSnapshot`;
- daemon snapshot method success, recent-event ordering, and invalid-parameter rejection;
- desktop request construction, typed response decoding, RPC-error preservation, and offline-error redaction;
- JavaScript uptime, timestamp, event-label, event-detail, and health-state formatting;
- Tauri configuration uses the static frontend, one main window, a CSP, and global Tauri API;
- `lazoo-desktop.exe` builds on the pinned Windows toolchain;
- a real desktop process opens a nonzero Windows main-window handle against an isolated running keeper/daemon;
- the existing first-slice test and acceptance gates remain green.

Visual QA uses a temporary screenshot of the real Tauri window at its default size and one narrow resize. The screenshot is inspected for clipping, contrast, hierarchy, empty states, and disconnected-state clarity; it is not committed as a product asset.

## Explicit exclusions

- provider discovery or adapters;
- employee, conversation, task, message, or approval records;
- project registration from the desktop;
- terminal streaming, ConPTY, or Job Objects;
- worktree creation;
- desktop mutation commands;
- tray/minimize-to-tray lifecycle;
- notification-center UI beyond the existing recovered-incident count;
- installer, signing, updater, or custom icon;
- external fonts, analytics, or network access.

## Completion gate

The slice is complete only when the Rust workspace tests, JavaScript tests, formatting, Clippy with denied warnings, all-target compilation, locked release build, existing first-slice Windows acceptance, and new desktop-shell acceptance all pass. Any visual or environment-bound gate that cannot run is reported explicitly rather than inferred.
