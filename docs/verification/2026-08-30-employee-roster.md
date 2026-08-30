# Durable Employee Roster Verification

**Date:** 2026-08-30
**Platform:** Windows 11 (`Microsoft Windows NT 10.0.26200.0`)
**Branch:** `codex/backend-foundation`
**Scope:** Backend-only employee configuration and revisioned provider-truth roster

## Outcome

PASS. Database schema 2 now stores durable employee identities, safe provider observations, and a monotonic roster revision. Daemon and CLI paths support create, list, show, update, disable, roster show, and explicit non-model refresh. No file under `apps/lazoo-desktop` changed in this slice, and `desktop.snapshot` exposes no employee, roster, or provider fields.

This evidence does not claim native employee sessions, Job Object or ConPTY ownership, terminals, conversations, task delegation, worktree automation, approvals, peer messaging, resource governance, or resumed UI implementation.

## Automated gates

| Gate | Result | Evidence |
|---|---:|---|
| Rust toolchain provenance | PASS | Rust/Cargo 1.98.0 MSVC resolved through `C:\Users\anubhab\.cargo\bin\cargo.exe` |
| `cargo test --workspace` | PASS | 119 passed, 0 failed, 10 ignored live-provider tests |
| `cargo fmt --all -- --check` | PASS | Exit 0 |
| `cargo clippy --workspace --all-targets -- -D warnings` | PASS | Exit 0, no warnings |
| `cargo check --workspace --all-targets` | PASS | Exit 0 |
| `cargo build --workspace --release --locked` | PASS | Exit 0; optimized release binaries built from the lockfile |
| `scripts/acceptance/first-slice.ps1 -SkipBuild` | PASS | Protocol 1, database schema 2, original control-plane/restart/spool gates retained |
| `scripts/acceptance/employee-roster.ps1 -SkipBuild` | PASS | All eight roster gates passed against real release binaries |
| `git diff --check` | PASS | No whitespace errors |

The 10 ignored tests are explicit live provider handshakes/model turns requiring installed authenticated native harnesses. Their omission from the ordinary workspace suite is intentional; this roster acceptance invokes only bounded non-model provider probes.

## Employee and roster evidence

The isolated roster acceptance recorded:

- protocol version 1 and database schema version 2;
- one durable employee from an idempotent create/replay pair;
- configuration revision 1 at creation, revision 2 after update, and revision 3 after disable;
- monotonic roster revisions for effective changes;
- stale update rejection with CLI exit code 9, `LZ-EMPLOYEE-STALE`, and only `currentRevision` as revision evidence;
- one explicit `roster refresh` with no provider smoke command;
- employee survival across the first daemon restart;
- disabled state, updated role, and stable identity surviving the second daemon restart;
- removal of the isolated temporary data root after the manifest was written.

Manifest: `artifacts/acceptance/employee-roster-manifest.json`

The original control-plane acceptance was rerun after migration 2. Project registration replay, restart durability, keeper restart-budget exhaustion, emergency spool recovery, notification API, exactly-once recovery, and IPC security coverage remained green.

Manifest: `artifacts/acceptance/first-slice-manifest.json`

## Regression found during acceptance

The first roster run exposed that Clap's inferred parser represented the default `--native-settings {}` as a JSON string rather than an object. A CLI regression assertion reproduced the boundary error. Both create and update flags now use an explicit JSON parser; the focused regression test and full workspace suite pass.

## Security and truthfulness checks

- Secret-like keys are rejected recursively in provider-native settings.
- Persisted observations exclude executable paths, raw provider evidence, credentials, and environment values.
- List/show operations never launch a provider; refresh is explicit, bounded, deduplicated by enabled provider, and non-model.
- Disabled employees remain durable rather than being physically deleted.
- Availability is derived deterministically as `disabled`, `offline`, `unverified`, or `dormant`; no running state is implied.
- UI capabilities and Tauri permissions are unchanged.
