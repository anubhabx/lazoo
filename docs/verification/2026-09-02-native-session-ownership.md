# Native Session Ownership Verification

**Date:** 2026-09-02

**Platform:** Windows 11 (`Microsoft Windows NT 10.0.26200.0`)

**Branch:** `codex/backend-foundation`

**Scope:** Backend-only durable employee-session ownership and creation-time Windows Job Object containment

## Outcome

PASS. Database schema 3 stores durable native-session history. `lazood` and `lazooctl` expose idempotent, revision-checked warm/list/show/stop operations. Each provider process tree is placed in a dedicated kill-on-close Job Object before provider launch through the protocol-transparent `lazoo-session-host.exe`. A forced daemon loss removes the owned native tree; the next daemon boot records `LZ-SESSION-OWNER-LOST` and never adopts the prior PID.

The live acceptance warmed a configured Codex App Server session and stopped it without sending a prompt or starting a model turn. No `provider smoke` command ran. The closeout changed no file under `apps/lazoo-desktop` and did not expand `desktop.snapshot`, Tauri commands, or UI capabilities.

This evidence does not claim ConPTY, terminal attachment or I/O, provider turns, conversations, task delegation, peer-message delivery, worktree automation, approvals, resource governance, or managed memory.

## Repository gates

| Gate | Result | Evidence |
|---|---:|---|
| Rust toolchain provenance | PASS | Rust/Cargo 1.98.0 MSVC resolved through `C:\Users\anubhab\.cargo\bin\cargo.exe`; `scripts\verify-rust-toolchain.ps1` exited 0 |
| `cargo test --workspace` | PASS | 138 passed, 0 failed, 11 ignored explicit live-provider-only tests |
| `cargo fmt --all -- --check` | PASS | Exit 0; Rust working-tree files use the repository's Windows newline style |
| `cargo clippy --workspace --all-targets -- -D warnings` | PASS | Exit 0, zero warnings |
| `cargo check --workspace --all-targets` | PASS | Exit 0 |
| `cargo build --workspace --release --locked` | PASS | Exit 0; optimized release binaries built from the lockfile |
| `git diff --check` | PASS | Exit 0, no whitespace errors |
| `scripts\acceptance\first-slice.ps1 -SkipBuild` | PASS | 8 control-plane/restart/spool/IPC gates, database schema 3, 10.972 seconds |
| `scripts\acceptance\employee-roster.ps1 -SkipBuild` | PASS | 8 roster gates, database schema 3, 7.731 seconds |
| `scripts\acceptance\native-session-ownership.ps1 -SkipBuild -Provider codex` | PASS | 14 session/containment gates, database schema 3, 27.259 seconds |

The 11 ignored workspace tests are installed-provider handshakes, contained-session starts, or model-backed turns. The native-session acceptance separately exercised the installed Codex non-model session-start path with real release binaries. It did not run any quota-consuming model smoke.

## Native-session acceptance evidence

The isolated acceptance recorded:

- `creationTimeContainment=PASS`: the release probe assigned the gated host, root fixture, and descendant fixture to one Job Object before release of the launch gate;
- `completeTreeTerminated=PASS`: terminating the Job Object removed the host, root, and descendant; the probe exited 0 in 367 ms;
- `providerReadyWithoutModelTurn=PASS`: employee creation performed the existing bounded non-model readiness probe and the roster reported route-ready session capability;
- `idempotentWarm=PASS`: the first Codex warm completed in 4,730 ms and returned an idle native session plus host PID; replay with the same key returned in 34 ms with the same durable session and no second process tree;
- `nativeHostPid=PASS`: the reported PID resolved to the release `lazoo-session-host.exe`, and the protected per-session working directory existed below the isolated data root;
- `staleStopRejected=PASS`: expected revision 1 was rejected with CLI exit code 10, `LZ-SESSION-STALE`, and current-revision evidence;
- `revisionCheckedBoundedStop=PASS`: expected revision 2 stopped the session in 2,154 ms; replay returned in 31 ms, the durable state was `suspended`, and the host PID was cleared;
- `daemonLossTerminatesTree=PASS` and `liveProviderTreeTerminated=PASS`: the acceptance snapshotted the live Codex host-plus-provider descendant tree before loss, then force-stopping the owning daemon removed every recorded process within the deadline;
- `ownerLossReconciled=PASS`: restart used a new boot ID, retained both historical session records, and changed the prior active record to `failed` with `LZ-SESSION-OWNER-LOST` and no native PID;
- `providerSmokeAbsent=PASS`: no final acceptance manifest contains a `provider smoke` command.

Manifest: `artifacts/acceptance/native-session-ownership-manifest.json`

The original control-plane and employee-roster manifests were regenerated against schema 3 after the session migration:

- `artifacts/acceptance/first-slice-manifest.json`
- `artifacts/acceptance/employee-roster-manifest.json`

## Security and scope checks

- The launch gate name is random and short-lived; it transports no credential or provider payload.
- Provider credentials remain in native stores and inherited provider environments. The session host does not inspect stdin, stdout, stderr, or protocol frames.
- Session directories use the existing owner/SYSTEM-protected data-root ACL and contain no project worktree claim.
- Native host PIDs are transient diagnostic ownership evidence and are cleared after stop or reconciliation.
- Warm requires route-ready provider evidence, an enabled employee, and the current employee configuration revision.
- Active sessions lock employee configuration and enforce one active session per employee.
- Stop is bounded; Job Object termination remains the cleanup guarantee if graceful provider shutdown fails.
- Planned shutdown attempts graceful suspension before daemon exit; forced loss is reconciled truthfully on the next boot.
