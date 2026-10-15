# Backend handoff: core v1 implementation, updated 2026-10-16

## Owner scope and source

The owner asked to finish the core v1 backend and explicitly chose core v1 over connectors/vector-search/voice. They also required checking out and pulling main before starting. That was done once: main advanced to `dfa35f1`, then was merged at `ab00236` into `codex/core-v1-backend`, retaining the existing Claude full-autonomy work.

On 2026-10-16 the owner excluded Grok from core v1 and restored the Claude subscription path. The required provider smoke set is now Codex, OpenCode, Antigravity, and Claude. An external Claude endpoint is optional; record the authentication channel actually tested. Keep Grok's historical evidence, but do not count it as a required gate.

This instruction supersedes the previous handoff's exclusions for terminal I/O, managed memory, configuration apply, approval bridging, and resource governance. UI remains separately owned: the desktop frontend, Tauri surface, and `desktop.snapshot` have not changed. Do not emit `UI BACKEND CONTRACT: GO` from this backend work.

## Current implementation

Schema 18 adds memory archive/typed ledger/queue, selected native configuration operations, and explicit summary-worker settings/attempt reservations. Previous migrations 1-15 are unchanged.

Read `docs/verification/2026-10-15-core-v1-backend-status.md` for the capability matrix, final commands, evidence, and limitations. It is authoritative over older slice notes. The plan is `docs/superpowers/plans/2026-10-15-core-v1-backend.md`.

New backend surfaces include:

- ACP/Claude per-action approval observers, streaming evidence and drop/cancel/deadline refusal. Other-provider governed dispatch is still refused where native auto-allow policy can bypass the observer.
- Hardened bounded authenticated control IPC and independent terminal byte I/O. Generic ConPTY owner terminals support lifecycle, resize, bounded offsets, overflow and process-tree cleanup.
- Compressed canonical turn archives with quotas, source integrity and recoverable intents; typed FTS memory; scoped retrieval and bounded context. Project memory can be recalled across tasks; Home stays within its conversation.
- Opt-in native memory summaries: fixed source excerpt, exact employee/model, per-scope admission spacing, fresh reserved session, bounded execution, strict records, no automatic resend after durable linking, and teardown. Native MCP tools are not proven absent; the configured Codex sandbox and denied observed actions are narrower guarantees.
- Codex 0.155.1 and installed 0.162.0-alpha.17.2 selected MCP configuration preview/apply/recovery/rollback with private backups and redacted public metadata. Both supported configuration shapes were parsed by the installed alpha in a disposable CODEX_HOME. Other formats/versions fail closed.
- Shared Windows infrastructure Job Object for daemon, keeper and CLI with 20% cap, rolling CPU accounting and 15% background throttling. Provider trees and desktop are explicitly outside that job.

## Verification and continuation

The canonical current gate is `scripts/gate.ps1 -OutputPath artifacts/acceptance/core-v1-gate-manifest.json`, with transcript `artifacts/core-v1-gate.log`. It runs formatting, warnings-denied clippy, all workspace tests, locked release build, and deterministic/real-binary acceptance scripts. It excludes paid-model provider adapters and the separately owned desktop.

`core-services.ps1` exercises the new real-binary memory/terminal/budget controls in disposable state. `scripts/test-cli-stress.ps1 -SkipBuild -Invocations 2048 -Concurrency 16` checks the final release without touching user data. Preserve the distinction between stress non-reproduction and a causal fix for the earlier rare heap-corruption exit.

The full gate passed 37 steps; later profile fixes passed 508 workspace tests plus one nested governor child-process probe, followed by the final focused configuration and Claude regressions plus workspace clippy/release. The final release passed disposable configuration lifecycle, core-service controls, and 4,096 CLI calls at concurrency 16. An earlier stress run timed out after 850 successes; its cause is unresolved. Exact evidence and binary hashes are in the status report and `artifacts/acceptance/core-v1-final-verification.json`.

Live adapter handshakes can be run with `scripts/acceptance/provider-adapters.ps1 -Provider <provider> -HandshakeOnly -SkipBuild`. The four required short smoke tests now have PASS evidence. OpenCode 1.19.0 passed with Big Pickle; its earlier 1.18.23 stall is historical and unexplained. After the owner signed in, Claude 2.1.300 passed through the native subscription. Its first post-login adapter attempt timed out, followed by a successful native diagnostic and an 11.307-second successful adapter smoke, without any code or timeout change. Read `docs/verification/2026-10-16-provider-recheck.md` and the provider manifest before continuing. No full native tool/permission/cancel/resume acceptance claim follows from these short replies. Never automatically resend uncertain task work or infer provider support from fixture tests or executable detection alone.

## Remaining product gates

Core v1 as defined by PROJECT-BRIEF section 24 is not accepted merely because these backend tests and short live replies pass. Remaining boundaries are recorded in the status document: broader native lifecycle and policy coverage before widening dispatch; provider-session terminal takeover and desktop binding; full raw-history retention beyond the bounded canonical source; aggregate desktop-inclusive performance validation; and additional native plugin/skill/config formats only after exact schema review. The intermittent CLI and provider no-progress timeouts have no established causal fix.

The new background worker remains unconfigured by default. No actual user provider config is changed and no live inference is triggered by building or running the deterministic gate.

## Durable rules

Preserve record-first evidence, revision and attempt fencing, native acceptance before execution claims, project/Home retrieval scope, and one-action-only approvals. Do not automatically resend uncertain model work, widen provider authority, overwrite configuration drift, delete retained task worktrees, or treat a terminal byte stream as the control channel. Archive cleanup remains an explicit preview/apply action. Future changes to UI contracts or paid-model endpoints require the corresponding owner intent.
