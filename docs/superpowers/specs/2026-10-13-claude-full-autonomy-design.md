# Claude Code Full-Autonomy Delivery Design

**Date:** 2026-10-13

**Status:** Implemented on `feat/claude-full-autonomy` (pull request #33, stacked on #32) with deterministic acceptance, for owner review before merge. No live gate has run: Lazoo claims no support for Claude Code beyond this deterministic path, and the roster keeps saying Detected, Configured, Degraded, or Unverified (brief section 21.3).

**Scope:** The smallest honest unlock of other-provider task delivery that `docs/NEXT-SESSION.md` proposed: conversation turns and task dispatch to Claude Code employees only under `full-autonomy`, fail-closed for `ask` and `self-decision`, and every other provider unchanged. One rule, two call sites, no schema change.

## 1. Why only this

Lazoo delivers a turn to a provider only when that provider can enforce the task's authority ceiling natively, because Lazoo's approval ledger (schema 8) is bridged only to Codex today.

| Provider | Native enforcement | Result |
|---|---|---|
| Codex | approval requests reach the schema-8 ledger under every authority | delivered under every authority (unchanged) |
| Claude Code | `full-autonomy` maps to `--permission-mode auto`, the one mode in which no prompt can occur; under `ask` and `self-decision` its prompts stay inside its own stream (`capabilities.permission: false`), where Lazoo cannot answer them | delivered under `full-autonomy` only |
| Antigravity | explicit modes are `plan` and `accept-edits`; there is no prompt-free mode, and the adapter refuses `auto` | refused (unchanged) |
| OpenCode, Grok Build (ACP) | the adapter answers every `session/request_permission` with `Cancelled` | refused (unchanged) |

## 2. The rule

`lazood::app::provider_enforces_authority(provider, authority)` is true for Codex, true for Claude Code when the authority is `full-autonomy`, and false otherwise. The dispatch path reads the task's effective authority; the turn delivery path reads the authority of the task the turn belongs to (a user turn's root task, a dispatch turn's child, a notice turn's recipient task) and fails closed when a turn has none. The refusal codes are unchanged: `LZ-TASK-AUTHORITY-UNSUPPORTED` for a task turn or dispatch, `LZ-TURN-PROVIDER-UNSUPPORTED` for a turn without a task; a refused dispatch fails the task and raises `LZ-CONFIG-INCOMPATIBLE` as before.

Effective authority is still the most restrictive of the parent's, the request's, and the assignee's, so a Claude Code worker whose own authority is `ask` can never be dispatched, and a request wider than the assignee's ceiling is refused as authority expansion. A Claude Code session receives no Lazoo collaboration tools (those are Codex dynamic tools), so a Claude Code lead works alone and a Claude Code worker completes through native completion evidence.

## 3. What this does not do

No approval bridging, no ACP permission handling, no Antigravity unlock, no change to how the roster labels a provider, no live gate. The desktop and `desktop.snapshot` are unchanged.

## 4. Review gate

The owner decides whether delivery to Claude Code under full autonomy may ship ahead of its live gate, and when that gate runs: one real Claude Code session, one user turn, one dispatched task, cancellation, and resume, through a configured endpoint, before any claim of support.
