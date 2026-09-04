# Conversation turn provider delivery verification

**Branch:** `codex/conversation-delivery`

**Scope:** Backend and communication layers only: schema-5 conversations, record-first turns, actor-aware collaboration envelopes, Codex App Server delivery, retry/cancel/recovery behavior, CLI controls, and isolated acceptance evidence.

**Verdict:** Deterministic release acceptance passes. Two pre-fix live Codex attempts failed and were not claimed. Their root cause is fixed and covered by real-shaped deterministic events, but post-fix live completion has not been rerun.

## Delivered behavior

- Schema 5 adds projectless or project-scoped conversations, immutable user- or employee-authored turns, append-only normalized provider events, and idempotent command records.
- A canonical message, queued delivery intent, turn, audit event, and outbox row commit before provider I/O.
- User-authored turns may explicitly activate the selected lead. Employee-authored turns cannot activate a dormant recipient.
- Daemon-managed delivery is enabled only for Codex in this slice.
- The canonical message is acknowledged only after a native turn ID or normalized progress is durably recorded.
- Definite pre-acceptance failures follow bounded retry policy. Accepted and uncertain work is not retried automatically.
- Native `completed`, `failed`, and `interrupted` terminal statuses map to `completed`, `terminal-failed`, and `cancelled` while preserving acknowledged-message truth after acceptance.
- Prior-boot submitting or accepted turns reconcile to `uncertain` with `LZ-TURN-OWNER-LOST`; no provider PID or native turn is adopted.
- Conversation and turn controls are available through daemon RPC and `lazooctl`; errors use CLI exit code 12.

## Test-driven regressions discovered by the live gate

The live attempts exposed two gaps that were first reproduced as failing tests:

1. The observer handled native `completed` but did not map native `failed` or `interrupted` terminal results. `native_terminal_statuses_close_accepted_turns_without_uncertainty` now proves both mappings while the canonical message remains acknowledged.
2. `Store::observe_turn` reapplied the acceptance-evidence check to every later event while the turn state was already `accepted`. Real Codex emits session-level events such as `thread/tokenUsage/updated` without a turn ID. `accepted_turn_allows_session_level_events_without_reproving_acceptance` reproduced the observer failure; the guard now runs only for the `submitting -> accepted` transition.

The disposable transport now emits a session-level token-usage event after native acceptance and an accepted `failed` terminal path. Both are exercised by release acceptance without a model or network call.

## Fresh deterministic gate matrix

| Command | Exit | Evidence |
|---|---:|---|
| `powershell -NoProfile -ExecutionPolicy Bypass -File scripts\verify-rust-toolchain.ps1` | 0 | `C:\Users\anubhab\.cargo\bin\cargo.exe`; Cargo 1.98.0 release 1.98.0 |
| `cargo test --workspace` | 0 | 196 passed, 0 failed, 11 ignored live-only tests |
| `cargo fmt --all -- --check` | 0 | No formatting or newline error |
| `cargo clippy --workspace --all-targets -- -D warnings` | 0 | No warnings |
| `cargo check --workspace --all-targets` | 0 | All targets checked |
| `cargo build --workspace --release --locked` | 0 | Locked optimized workspace build |

Focused post-fix evidence also passed: 7/7 store conversation tests and 41/41 daemon integration tests.

## Fresh release acceptance manifests

| Manifest | Status | Duration | Commands | Key evidence |
|---|---:|---:|---:|---|
| `first-slice-manifest.json` | PASS | 10,169 ms | 14 | Database schema 5 and original control-plane contract |
| `employee-roster-manifest.json` | PASS | 7,557 ms | 17 | Schema 5 roster durability and non-model refresh |
| `native-session-ownership-manifest.json` | PASS | 11,299 ms | 18 | Creation-time containment, stop, and owner-loss handling |
| `collaboration-bus-manifest.json` | PASS | 6,764 ms | 26 | Record-first messaging, retry/dead-letter, restart, audit/outbox |
| `conversation-delivery-manifest.json` | PASS | 24,523 ms | 32 | Schema 5; projectless/project conversations; both authors; record-first order; native acceptance; session-level events; terminal mapping; retry; cancellation; owner loss; transcript bounds; planned-shutdown provider-tree cleanup; audit/outbox; outer Codex context isolation; no model invocation |

The deterministic conversation manifest records `nativeTerminalMapping = PASS`, `outerCodexContextIsolated = PASS`, `cleanProviderTreeStop = PASS`, `modelInvocationAbsent = PASS`, and `temporaryDataRetained = false`. `releaseBuild = SKIPPED_BY_CALLER` is expected because the locked release build ran immediately before the acceptance sequence.

## Explicit live Codex evidence

The live command was:

```powershell
powershell -NoProfile -ExecutionPolicy Bypass -File scripts\acceptance\conversation-delivery.ps1 -SkipBuild -Provider codex -LivePrompt "Reply with exactly LAZOO_TURN_OK. Do not use tools."
```

Attempt 1 ended in 6,560 ms with CLI exit 12 and `LZ-TURN-UNCERTAIN`. It revealed that the daemon inherited the outer Codex desktop task identity. The acceptance harness now removes `CODEX_THREAD_ID` and `CODEX_SESSION_ID` from the daemon environment; deterministic evidence records `outerCodexContextIsolated = PASS`.

Attempt 2 ran with that isolation, ended in 7,063 ms, and still returned CLI exit 12:

```text
RpcError { code: "LZ-TURN-UNCERTAIN", message: "provider delivery outcome is uncertain", data: Some(Object {"turnId": String("acbf2f2b-8941-48bb-9b80-549ea560d914")}) }
```

The failed manifest is retained as `conversation-delivery-live-manifest.json`; it is not PASS evidence. A read-only App Server `thread/read` of native thread `01a0694d-5ac1-72e2-bc1e-9fbe70862ef0` showed turn `01a0694d-60f2-7bd0-a072-6426489fda71` as `interrupted`, with no items and no provider error. The persisted thread used the actually selected Codex executable, CLI 0.153.0. Its generated App Server schema confirmed `completed`, `interrupted`, `failed`, and `inProgress` as the current native turn statuses.

The empty interrupted turn matched the acceptance-observer failure described above: Lazoo interrupted native work after a normal session-level event failed the overbroad acceptance-evidence guard. That root cause is fixed and covered. No third quota-consuming prompt was issued in this verification pass, so exact `LAZOO_TURN_OK` output after the fix remains unverified.

## Current non-model Codex readiness

A 2026-09-04 preflight refreshed only the non-model surroundings of the pending gate:

| Command | Exit | Evidence |
|---|---:|---|
| selected `codex.exe --version` | 0 | Desktop executable `codex-cli 0.153.1` |
| selected `codex.exe login status` | 0 | `Logged in using ChatGPT` |
| `cargo test -p lazoo-providers --test codex live_codex_app_server_handshake -- --ignored --exact --nocapture` | 0 | 1 passed; App Server v2 configured; native authentication ready; no account data exposed |
| `cargo test -p lazoo-providers --test codex live_codex_session_start_reports_contained_host -- --ignored --exact --nocapture` | 0 | 1 passed; native host present and cleanly stopped; no model turn |

Both live tests ran with the outer `CODEX_THREAD_ID` and `CODEX_SESSION_ID` removed from their subprocess environment. They show that the current 0.153.1 executable can authenticate, handshake, and create/stop the contained native session. They do not prove a model response or close the post-fix conversation-delivery gate.

## Scope and exclusions

- No file under `apps/lazoo-desktop` changed.
- No Tauri command, capability, or `desktop.snapshot` field changed.
- No ConPTY, terminal-I/O, task orchestration, worktree mutation, approval engine, managed memory, resource governor, or connector capability was added.
- Other providers retain discovery/probe/smoke/session behavior; conversation delivery is not claimed for them.
- Deterministic transport proof and live Codex proof are reported separately.

## Remaining gate

With fresh user authorization for the quota-consuming call, rerun exactly one live command above. Require a PASS live manifest, exact `LAZOO_TURN_OK`, durable native session/turn identity, acknowledged canonical message, completed local turn, and clean provider-tree stop. On failure, preserve the exact blocker and do not retry automatically.
