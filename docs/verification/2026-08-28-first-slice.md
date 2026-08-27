# First Lazoo Control-Plane Slice Verification

- **Date:** 2026-08-28
- **Result:** PASS
- **Branch:** `feature/first-control-plane-slice`
- **Acceptance manifest:** `artifacts/acceptance/first-slice-manifest.json`
- **Baseline:** Windows 11, Rust 1.92.0 MSVC

## Delivered boundary

The first runnable slice delivers `lazoo-keeper.exe`, `lazood.exe`, and `lazooctl.exe`. The keeper supervises the daemon; the CLI reaches it only through an installation-scoped, current-user-authenticated Windows named pipe. Project registration uses real Git identity, defaults to isolated checkout mode, and commits the project, immutable event, and outbox record transactionally in SQLite WAL. Restart-budget exhaustion seals an emergency NDJSON incident and attempts a native Windows toast before the keeper exits nonzero.

Provider adapters, employee execution, worktree creation, managed memory, policy/approval flows, ConPTY, Job Objects, and the desktop shell remain outside this slice.

## Automated gates

| Gate | Result | Evidence |
|---|---|---|
| Formatting | PASS | `cargo fmt --all -- --check` exited 0 |
| Lint | PASS | `cargo clippy --workspace --all-targets -- -D warnings` exited 0 |
| Tests | PASS | `cargo test --workspace` ran 48 tests; 48 passed, 0 failed |
| All-target compilation | PASS | `cargo check --workspace --all-targets` exited 0 |
| Release executables | PASS | `cargo build --workspace --release --locked` exited 0 from a clean worktree |
| Windows acceptance | PASS | `powershell -NoProfile -ExecutionPolicy Bypass -File scripts/acceptance/first-slice.ps1` exited 0 |
| Whitespace | PASS | `git diff --check` exited 0 |

The final acceptance run completed in 10.418 seconds and recorded 15 release/CLI commands. It verified release binary presence, protocol and schema version 1, WAL mode, default isolated registration, duplicate-idempotency collapse to one project and event, forced daemon restart with a new boot ID and PID, durable project/event/outbox state, crash-loop exhaustion, keeper exit code 20, a sealed `LZ-DAEMON-UNAVAILABLE` incident, Windows notification API acceptance, reconciliation, and exactly-once incident import across a second recovery restart.

## Acceptance-criterion audit

| Criterion | Result | Evidence |
|---|---|---|
| All three executables build on baseline Windows | PASS | Locked release build and binary assertions in the acceptance manifest |
| Unit, store, Windows IPC, keeper, and end-to-end suites pass | PASS | 48 Rust tests plus the PASS acceptance manifest |
| Registration survives a forced daemon restart | PASS | `restartDurability` gate; boot ID and PID changed while one project/event remained |
| Repeated idempotency key produces one mutation | PASS | `projectMutation` gate and daemon/store tests |
| Pipe rejects invalid proof and has owner/SYSTEM-only grants | PASS | `invalid_nonce_is_rejected_before_application_frames` and `pipe_dacl_contains_only_owner_and_system_grants` |
| Exhaustion stops, spools, and attempts notification | PASS | 1/2/5-second policy tests and `restartBudgetAndSpool`/`nativeNotificationApi` gates |
| Emergency recovery imports exactly once | PASS | `exactlyOnceRecovery` gate and store exact-once test |
| CLI has stable human/JSON and exit behavior | PASS | CLI parser/output/exit-family tests plus release CLI acceptance |
| Required format, lint, build, and security gates pass | PASS | Commands above; IPC security assertions are automated |

## Manual and environment-bound evidence

- **Native notification visual appearance:** BLOCKED for visual confirmation because the final run was unattended. The real WinRT toast call returned success and `keeper-state.json` recorded `notificationAttempted: true` with `notificationError: null`; spool durability does not depend on it.
- **Cross-user negative connection:** BLOCKED because the baseline run had no second Windows user token. Effective DACL inspection proves grants only for the installation owner and LocalSystem, while invalid nonce and incompatible protocol paths are exercised automatically.

No required automated gate is blocked. `rustfmt` and Clippy were available through the rustup-managed MSVC toolchain, so the implementation plan's earlier host-availability warning did not apply at closeout.
