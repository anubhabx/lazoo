# Lazoo Desktop-Shell Slice Verification

- **Date:** 2026-08-28
- **Result:** PASS
- **Branch:** `codex/desktop-shell`
- **Baseline:** Windows 11, Rust 1.98.0 MSVC, Node 24.16.0, Tauri 2.11.5, WebView2
- **Desktop acceptance manifest:** `artifacts/acceptance/desktop-shell-manifest.json`
- **Retained foundation manifest:** `artifacts/acceptance/first-slice-manifest.json`

## Delivered boundary

This slice delivers `lazoo-desktop.exe`, a read-only Tauri 2 desktop control room backed by the existing current-user-authenticated named pipe. The daemon exposes one additive `desktop.snapshot` JSON-RPC method returning a typed capture of daemon status, registered projects, and up to 100 recent events. The desktop displays healthy, degraded, loading, empty, and disconnected states; refreshes every ten seconds while visible; preserves the last successful snapshot after a later connection failure; and closes without stopping the keeper or daemon.

The webview can invoke only the generated `allow-load-desktop-snapshot` command plus Tauri core defaults. It has no filesystem, shell, process, HTTP, clipboard, project-registration, or daemon-shutdown permission. Provider adapters, employees, conversations, tasks, messages, approvals, terminals, worktrees, memory, tray lifecycle, installers, and mutation controls remain outside this slice.

## Fresh automated gates

| Gate | Result | Evidence |
|---|---|---|
| Toolchain | PASS | `scripts\verify-rust-toolchain.ps1` resolved plain Cargo to `C:\Users\anubhab\.cargo\bin\cargo.exe`, Rust 1.98.0 MSVC |
| JavaScript presentation tests | PASS | `node --test apps\lazoo-desktop\ui\view-model.test.mjs`: 7 passed, 0 failed |
| Rust workspace tests | PASS | `cargo test --workspace`: 61 passed, 0 failed, plus all doc tests passed |
| Formatting | PASS | `cargo fmt --all -- --check` exited 0 |
| Lint | PASS | `cargo clippy --workspace --all-targets -- -D warnings` exited 0 |
| All-target compilation | PASS | `cargo check --workspace --all-targets` exited 0 |
| Locked release build | PASS | `cargo build --workspace --release --locked` exited 0 and produced all four release executables |
| Foundation Windows acceptance | PASS | `scripts\acceptance\first-slice.ps1` exited 0; all eight retained gates report PASS |
| Desktop Windows acceptance | PASS | `scripts\acceptance\desktop-shell.ps1 -SkipBuild` exited 0 after the separate locked release build; all eight desktop gates report PASS |
| Static webview safety scan | PASS | No `innerHTML`, `outerHTML`, `insertAdjacentHTML`, remote HTTP resource, or inline-script-content match under `apps/lazoo-desktop/ui` |
| Whitespace | PASS | `git diff --check` exited 0 |

The Rust count increased from 48 to 61: one protocol contract test, two daemon snapshot tests, six desktop-client tests, and four Tauri configuration/security and acceptance-scope tests were added. Seven Node tests cover uptime and timestamp formatting, checkout/event labels, bounded event summaries, healthy state, degraded state, and recovered-incident state.

## TDD evidence

- The protocol test first failed because `DesktopSnapshot` did not exist, then passed after the strict camel-case DTO was added.
- Both daemon tests first returned `LZ-METHOD-NOT-FOUND`, then passed after `desktop.snapshot` was implemented with empty-parameter enforcement.
- Desktop-client tests first failed on missing request/parser/error types, then passed after the one-shot typed bridge was implemented.
- Review regression tests first failed because the bridge had no overall deadline and accepted mismatched JSON-RPC metadata. They pass after a five-second request deadline and strict version/request-ID correlation were added.
- Tauri configuration tests first failed because the configuration, capability, and build manifest files were absent, then passed after the restricted scaffold was added.
- The failed-run cleanup regression first proved the harness selected daemons by shared executable path. Later review passes exposed a restart race in a one-time descendant snapshot and check-to-stop races that could interrupt `finally`. All retained-process cleanup now uses one idempotent helper; keeper cleanup confirms the root stop, then repeatedly rescans and drains only its descendants. Live probes cover a concurrent natural root exit and force a child restart during cleanup while a same-binary unrelated daemon remains running until explicitly stopped.
- The Node suite first failed because `view-model.js` was absent. Its first implementation exposed an acronym-humanization failure; the focused fix produced 7/7 passing tests.

## Windows acceptance evidence

The final desktop manifest records:

| Gate | Evidence |
|---|---|
| `releaseBuild` | Release binaries existed from the immediately preceding locked workspace build |
| `cleanupIsolation` | The harness tolerated a probe root exiting concurrently, forced a daemon restart during keeper cleanup, drained the restarted descendant, and preserved an unrelated `lazood.exe` launched from the same release binary with a separate data root |
| `daemonReady` | Isolated keeper/daemon reported protocol 1 and SQLite WAL |
| `desktopWindow` | `lazoo-desktop.exe` exposed a nonzero Windows main-window handle with the exact title `Lazoo — Local control room` |
| `snapshotRpc` | Windows UI Automation observed `Control plane steady`, `Desktop acceptance`, and `Project registered` from the real WebView2 surface |
| `defaultScreenshot` | `PrintWindow` captured the 1180×760 application surface |
| `narrowScreenshot` | The same real window resized to 860×620 and retained `System pulse` and project content in the accessibility tree |
| `processIsolation` | Closing the desktop left the keeper running; planned daemon shutdown then ended the keeper with exit code 0 |

The acceptance harness initially used `CopyFromScreen`; visual inspection caught that an occluding Chrome window could produce a false screenshot pass. The harness was corrected to use Windows `PrintWindow` for the application surface, with screen copy only as a fallback. Review then caught that failed-run cleanup selected every daemon sharing the release executable path and later found child-restart and concurrent-root-exit races in the fixes. Cleanup is now idempotent, confirms the keeper has stopped, repeatedly drains its descendant tree, and verifies root-exit, forced-restart, and same-binary isolation at runtime. The final manifest was regenerated after these corrections.

## Visual QA

The final default and narrow captures referenced by `desktop-shell-manifest.json` were inspected at original resolution.

- **Default 1180×760:** PASS. Connected status, primary health statement, four metrics, disposable project row, recent event, and system-pulse rail are readable. No overlap, cutoff, or false empty state was observed. Long Windows paths wrap inside their assigned columns.
- **Minimum 860×620:** PASS. Metrics reflow to one column without horizontal clipping. The project ledger and system-pulse content remain present below the fold and reachable through vertical scrolling. The header and refresh action remain visible.
- **Visual hierarchy and contrast:** PASS. Warm paper/ink palette, safety-orange labels, green confirmed-health state, thin rules, and restrained typography remain distinct without relying on glow, gradients, or decorative charts.

The screenshots remain in the manifest's isolated temporary run directory for evidence review and are not committed as product assets.

## Retained manual limitations

The earlier foundation slice still has two explicitly manual/environment-bound checks: visual appearance of the native Windows toast and a live cross-user negative pipe connection. The automated WinRT notification call, effective owner/SYSTEM-only DACL assertion, invalid nonce test, and incompatible-protocol test remain green. This desktop slice adds no new blocked gate.

## Contract audit

The implementation was compared with `docs/superpowers/specs/2026-08-28-desktop-shell-design.md` and the delivery exclusions above. Every named automated gate ran. The desktop exposes no mutation command, does not start or stop the keeper, uses no JavaScript framework or remote asset, rejects mismatched JSON-RPC responses, bounds stalled daemon exchanges, and does not claim provider or employee support.
