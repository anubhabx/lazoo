# Copilot and account routing implementation plan

> For agentic workers: use superpowers:subagent-driven-development or superpowers:executing-plans task-by-task.

Goal: support Copilot and persistent account/key pools without changing existing native sessions.
Architecture: provider ACP adapter, protected account registry, explicit session identity,
and compatible streaming API routing; owner controls through daemon, CLI and desktop.
Tech stack: Rust/Tokio/SQLite/Windows DPAPI, existing ACP/MCP bridges, React/Tauri.

## 1. Copilot adapter
- [ ] Add failing contract/discovery and fake ACP lifecycle tests.
- [ ] Implement `crates/lazoo-providers/src/copilot.rs`, provider kind/registry/discovery/config support.
- [ ] Verify ACP MCP permissions, model selection, cancellation, history and capability honesty.
- [ ] Check installed 1.0.79 with no-model initialization, then bounded smoke if authenticated.
- [ ] Review specification compliance and code quality; commit the verified slice.

## 2. Protected account registry and selection
- [ ] Add protocol records and migration/store tests for redaction, revisions and durable affinity.
- [ ] Expose existing DPAPI primitives through a small protected-secret API.
- [ ] Implement account CRUD, eligibility, balanced/pinned selection and observations.
- [ ] Add per-process credentials/config isolation to session launch and native takeover/view paths.
- [ ] Integrate session start, resume, stop and failure handling without ambient fallback.

## 3. API streaming routing
- [ ] Test real local HTTP upstreams for two-account distribution, sticky continuation, cooldowns,
  invalid proxy auth, streaming backpressure, cancellation and refusal of unsafe replay.
- [ ] Implement loopback proxy and provider-specific endpoint/key injection.
- [ ] Test Codex/Claude/OpenCode compatible protocols and document native-only modes.
- [ ] Benchmark local routing overhead with fixture traffic, preserving measured units.

## 4. Owner controls
- [ ] Add owner RPC dispatch and CLI account commands; secrets enter via stdin or a protected form.
- [ ] Add Settings pool controls and provider/routing visibility, including Copilot launcher support.
- [ ] Verify UI build and interactions; inspect desktop visually after integration with Claude.

## 5. Final verification and stacked PR
- [ ] Review full implementation for account leaks, continuation mistakes, authority regression.
- [ ] Run format, targeted/workspace tests, strict lint, build and relevant acceptance scripts.
- [ ] Update source-backed provider matrix, benchmark and limitations in verification/handoff docs.
- [ ] Fetch PR state; rebase on Claude's completed PR if first, otherwise stack on current pilot.
- [ ] Push feature branch, raise PR with exact evidence and remaining live-acceptance limitations.
