# Backend readiness for the first UI

The backend is ready to begin UI implementation for the Codex workflow, including
the actual native TUI visible during orchestrated work. The final source checkpoint
is `feab4d556b3424d3b9b0b7ec9e155f62434b8a95` on
`codex/backend-ui-readiness`; subsequent changes only preserve verification evidence
and update documentation. The [consolidated manifest](../../artifacts/readiness/ui-backend-final-manifest.json)
links the checks and matching release binary hashes. Full core v1 remains broader.

The first full run completed 44 steps with one failure: configuration lifecycle
demanded the removed Codex `0.162.0-alpha.17.2` installation. All other steps,
including workspace tests and release build, passed. The current installed alpha
has since passed a separate native MCP parser review; see
[compatibility evidence](2026-10-29-codex-mcp-configuration.md). Its exact-version
allowlist test failed before the change and the three configuration tests passed
afterward. The repaired lifecycle check passed independently, then the final full
gate passed all 44 steps on the source checkpoint above. The failed attempt is
retained as `artifacts/readiness/gate-native-before-config-upgrade-manifest.json`.

The owner requested a comfortable backend starting point for UI work and explicitly
required the actual native TUI to remain visible during orchestrated work. The
implemented concurrent native path is Codex. Other providers retain explicit
capability limits; this is not full core v1 completion.

## Changes and evidence

| Requirement | Implementation and verification |
| --- | --- |
| Truthful launch controls | Per-lead authority choices respect the employee ceiling and provider policy; collaboration and concurrent native-view support are explicit. Codex supports both. |
| Durable refused work | Permanent authority refusals atomically settle the direct user turn, canonical message and root task. Temporary activation/recovery behavior is preserved. |
| History and reconnect | Exclusive backward turn cursor; current lead/workers remain current. Durable event subscriptions drain bounded pages beyond 1,024 rows and recover notification lag without skipping rows. |
| Terminal recovery | Typed status, view and existing attach operations; terminal byte transport remains authenticated and separate. Keys, revisions and boot identities fence replay/reconnect. |
| Same-session native display | One controller-owned Codex app-server, a restricted view bridge and genuine Codex TUI in ConPTY. Viewer mutations and server-request responses cannot reach provider control. |
| Tool and approval ownership | Actual installed Codex plus local Responses fixture completed `employees_list`, accepted a native command approval into Lazoo's ledger, propagated a decline and streamed the resulting response in the TUI. |
| Lifecycle and secrets | View close/reopen preserves a pending turn and approval; session stop closes the view. Private child environment, redacted Debug, scoped reads, bounded queues/messages/writes and deterministic close. |
| UI integration handoff | `docs/UI-BACKEND-HANDOFF.md` and terminal/contract specifications describe the available calls, events, paging, errors, provider limits and native-input restrictions. |

Fresh focused verification completed:

- Providers library: 26 tests passed, including broker ordering, cancellation,
  stale replies, overflow, EOF, write deadlines and view scope.
- Native-view WebSocket integration: 3 tests passed, exercising authentication,
  browser-Origin/duplicate-view refusal, bootstrap ordering, scoped reads,
  immutable authority, real history errors and nonreading-viewer close.
- Daemon native-view integration: 2 tests passed using the actual Codex adapter
  over the compiled protocol fixture. They cover replay/conflicts, close/reopen,
  session cleanup, stale revisions and unsupported sessions.
- Empty/nonempty Windows environment serialization: focused test passed. Earlier
  terminal environment integration also passed (9 tests).
- `cargo clippy --workspace --all-targets --locked -- -D warnings`: passed after
  correcting the new code's lint findings. Formatting and `git diff --check` passed.
- Independent transport/lifecycle review: material findings fixed and re-reviewed.

The production debug acceptance run
`artifacts/readiness/native-tui/production-e1bbbe92dcb24b64b0a906b6df4e2f76/result.json`
passed all 13 checks using installed Codex `0.165.0-alpha.1.2`. It used four local
model-fixture requests and no external model requests. Its evidence includes the
real TUI's output bytes, canonical turns, RPC results and the native rejected
command result. The repeatable driver is
`artifacts/readiness/native-tui/acceptance_production.py`.

The earlier feasibility proxy also passed on Codex `0.153.4` and
`0.165.0-alpha.1.2`; it is separate from the production-path evidence. The native
TUI uses its built-in “Viewing sub-agent” label even for a lead because direct
input is disabled. Prompts and approvals belong in Lazoo controls.

## Final gate

| Check | Result |
| --- | --- |
| Full `scripts/gate.ps1` on source checkpoint | PASS: 44/44 steps, including 39 acceptance scripts; `artifacts/readiness/gate-native-final-manifest.json` |
| Workspace tests within that gate | 607 passed, 0 failed, 14 intentionally ignored; excludes nested filtered test invocations from the count |
| Release production native-view acceptance | PASS: 13/13 checks, Codex `0.165.0-alpha.1.2`; `artifacts/readiness/native-tui/release-evidence/result.json` |
| 2,048 CLI calls, concurrency 16 | PASS: 2,048 exit-zero results, no failures, no stderr on success, no daemon stderr; `artifacts/readiness/acceptance/cli-stress-a186aaed80f44e7a921529eca178eacd.json` |
| Frontend changes | None |
| External model/provider smoke calls | Not run |
| Personal provider configuration writes | None; local simulator uses disposable state |

The release native run used four local fixture requests, no external model
requests, and the same daemon/CLI binaries as the stress test (SHA-256 verified).
Its native screen bytes/text, session, completed turns and preservation hashes
are retained in `artifacts/readiness/native-tui/release-evidence`. The repeatable
driver remains `artifacts/readiness/native-tui/acceptance_production.py release`.

The cold-worktree gate now builds normal-profile `lazoo-session-host` and
`lazoo-turn-transport-fixture` before tests. All 38 rewritten historical acceptance
manifests were copied with matching hashes, then restored to their previous
contents. The 39 fresh gate manifests and stress result are retained separately
under `artifacts/readiness/acceptance`; `acceptance-preservation.json` maps them.
The gate log is retained locally as `artifacts/readiness/gate-native-final.log`.

Builds and tests were serialized with `CARGO_BUILD_JOBS=1`,
`CARGO_PROFILE_DEV_DEBUG=0`, `CARGO_PROFILE_TEST_DEBUG=0`, `CARGO_INCREMENTAL=0`
and `RUST_TEST_THREADS=2`. The release native and stress checks ran after the gate,
without competing test workloads. No production source changed after these checks.

## Remaining product scope

Concurrent native views and collaboration tools for non-Codex providers, broader
governed authority support, remaining configuration federation formats and
whole-product performance windows still require work. Connectors, vectors and
voice are later phases. The desktop terminal renderer and UI flows are not built
here. Historical live-provider evidence remains dated; local simulation proves
native protocol/rendering and control ownership, not current external account or
model-service availability.
