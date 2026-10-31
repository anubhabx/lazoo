# Copilot and account routing implementation plan

> For agentic workers: use superpowers:subagent-driven-development or superpowers:executing-plans task-by-task.

Goal: support Copilot and persistent account/key pools without changing existing native sessions.
Architecture: provider ACP adapter, protected account registry, explicit session identity,
and compatible streaming API routing; owner controls through daemon, CLI and desktop.
Tech stack: Rust/Tokio/SQLite/Windows DPAPI, existing ACP/MCP bridges, React/Tauri.

## 1. Copilot adapter
- [x] Add failing contract/discovery and fake ACP lifecycle tests.
- [x] Implement `crates/lazoo-providers/src/copilot.rs`, provider kind/registry/discovery/config support.
- [x] Verify ACP MCP permissions, model selection, cancellation, history and capability honesty.
- [x] Check installed 1.0.79 with no-model initialization, then bounded smoke if authenticated.
- [x] Review specification compliance and code quality; package the implementation commit.

## 2. Protected account registry and selection
- [x] Add protocol records and migration/store tests for redaction, revisions and durable affinity.
- [x] Expose existing DPAPI primitives through a small protected-secret API.
- [x] Implement account CRUD, eligibility, balanced/pinned selection and observations.
- [x] Add per-process credentials/config isolation to session launch and native takeover/view paths.
- [x] Integrate session start, resume, stop and failure handling without ambient fallback.

## 3. API streaming routing
- [x] Test real local HTTP upstreams for two-account distribution, sticky continuation, cooldowns,
  invalid proxy auth, streaming backpressure, cancellation and refusal of unsafe replay.
- [x] Implement loopback proxy and provider-specific endpoint/key injection.
- [x] Test Codex/Claude/OpenCode compatible protocols and document native-only modes.
- [x] Benchmark local routing overhead with fixture traffic, preserving measured units.

## 4. Owner controls
- [x] Add owner RPC dispatch and CLI account commands; secrets enter via stdin or a protected form.
- [x] Add Settings pool controls and provider/routing visibility, including Copilot launcher support.
- [x] Verify UI build and interactions; inspect the integrated desktop UI in the browser after integration with Claude.

## 5. Final verification and stacked PR
- [x] Review full implementation for account leaks, continuation mistakes, authority regression.
- [x] Run format, targeted/workspace tests, strict lint, build and relevant acceptance scripts.
- [x] Update source-backed provider matrix, benchmark and limitations in verification/handoff docs.
- [x] Fetch PR state; rebase on Claude's completed PR if first, otherwise stack on current pilot.
- [ ] Push feature branch, raise PR with exact evidence and remaining live-acceptance limitations.
