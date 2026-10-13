# Claude Code Full-Autonomy Delivery Implementation Plan

**Goal:** Deliver conversation turns and dispatch tasks to Claude Code employees under `full-autonomy` only, fail-closed for every other authority and every other non-Codex provider, with no support claim until a live gate runs.

**Architecture:** One rule in `lazood` (`provider_enforces_authority`) used by the dispatch gate in `app/task.rs` and the delivery gate in `app/turn.rs`; the turn gate reads the authority of the turn's task. No schema, protocol, store, CLI, or adapter change.

**Tech Stack:** Rust 1.98, Tokio, PowerShell acceptance.

**Spec:** `docs/superpowers/specs/2026-10-13-claude-full-autonomy-design.md`.

## Global Constraints

- No migration; schema stays 15. No change under `apps/lazoo-desktop`.
- Fail closed: a turn with no task, any non-Codex provider under `ask` or `self-decision`, and Antigravity, OpenCode, and Grok Build under any authority are refused with the existing codes.
- No live model turn in acceptance; no support claim.

---

### Task 1: The rule and the two gates

- [x] `apps/lazood/src/app.rs`: `provider_enforces_authority`.
- [x] `apps/lazood/src/app/task.rs`: the dispatch gate reads the task's effective authority.
- [x] `apps/lazood/src/app/turn.rs`: the delivery gate reads the turn's task authority.

### Task 2: Proof

- [x] `apps/lazood/tests/daemon.rs`: `claude_code_runs_turns_and_tasks_only_under_full_autonomy` (a Codex lead's delegations to a Claude worker under full autonomy, a Claude worker under `ask`, and an Antigravity worker; a Claude lead's user turns under `ask` and full autonomy) and the acceptance-script contract; the existing before-I/O refusal test unchanged.

### Task 3: Acceptance and continuity

- [x] `scripts/acceptance/claude-full-autonomy.ps1` writes `artifacts/acceptance/claude-full-autonomy-manifest.json`.
- [x] README, NEXT-SESSION, and `docs/verification/2026-10-13-claude-full-autonomy.md`.
