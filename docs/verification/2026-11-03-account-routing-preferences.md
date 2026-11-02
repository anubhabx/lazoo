# Account routing preferences and usage verification

Date: 2026-11-03. Worktree: C:\workspace\Orchestrator\.worktrees\copilot-account-routing. Branch: feat/account-routing-preferences. Base: 4fc9527.

## Scope and evidence

Implements migration 0022/schema 22, priority/order routing, revision-fenced reserves, structured exhaustion, public usage windows, a 90-day hourly ledger, owner RPCs and CLI commands, and account labels in existing session/worker/lead read models. The desktop change is limited to client.rs allowlisting (44 reads / 31 mutations) and protocol.ts types. There are no component edits.

Installed Codex 0.153.4 schema generation confirms account/rateLimits/read (null params), account/rateLimits/updated, primary/secondary window fields, public plan types and typed codexErrorInfo values. Compact definitions and SHA256 hashes are retained in 2026-11-03-account-routing-protocol-evidence.json. The full generated schema remains untracked under artifacts/account-preferences-codex-schema; sandbox policy rejected its cleanup. Exclude artifacts from staging. The compact evidence file is the review artifact.

Installed Claude Code 2.1.316 contains the documented rate_limits statusline and StopFailure/stream surfaces. Supported field names were checked against official documentation. Lazoo-owned native sessions install only session-scoped settings; no personal statusline settings are edited. The normalized authenticated callback responds immediately, retains one latest snapshot, and never handles permission decisions. Native context-window occupancy is not token consumption.

OpenAI/Anthropic rate-limit headers are normalized from ordinary upstream API proxy responses. Copilot's ACP documentation supplies no verified structured subscription quota/exhaustion contract. Its separate SDK account.getQuota declaration is acknowledged but not integrated. No private subscription HTTP endpoint, OAuth extraction, free-form screen parsing, or model quota probe was used.

## Commands and results

- PASS: cargo fmt --all; changed Rust sources also checked for CRLF.
- PASS: cargo clippy --workspace --all-targets -- -D warnings.
- PASS: focused routing, ledger, quota/header normalization, native callback, CLI, RPC and allowlist tests. Command: cargo test --workspace --lib --test provider_accounts --test codex --test opencode --test cli_deadlines_contract --no-fail-fast. Its only remaining failure was the old Codex event-position expectation; after updating that assertion to include the new quota event, cargo test --workspace --test codex passed (21 passed, 3 existing live tests ignored).
- PASS: final cargo test --workspace, exit 0. All 153 daemon tests passed, including the acceptance-script literal contracts and the task-graph execution. Existing live-provider tests remain ignored.
- PASS: task-graph acceptance with current debug fixtures and SkipBuild (schema 22, all evidence checks true). The workspace test now uses this same path.
- PASS: git diff --check and UI scope check (only protocol.ts changed under the UI tree).

All Cargo runs used CARGO_BUILD_JOBS=2 and the existing target. Final logs are artifacts/account-preferences-clippy-final.log, account-preferences-codex-final.log and account-preferences-workspace-complete.log. Intermediate failures remain in account-preferences-focused-tests.log, account-preferences-workspace-tests.log and account-preferences-workspace-final.log. An intermediate nested debug build exhausted disk; the owner freed space by deleting incremental caches, and the final suite passed with approximately 10 GB still free. The test harness no longer performs a redundant nested workspace build.

All Cargo commands use CARGO_BUILD_JOBS=2 and this worktree's existing target. The final workspace test path uses debug builds, and scripts/gate.ps1 was not run. An initial workspace run unexpectedly executed a nested release build through an existing task-graph test; it completed before that hidden behavior was identified. This is an explicit deviation from the requested skip, not a claimed release gate. The harness now selects debug while standalone acceptance retains its release default. Existing ignored authenticated/live-provider tests remain ignored. Temporary fixture repositories/worktrees created by existing workspace tests are test-owned temporary data, not additional development worktrees or target directories.

## Regression coverage

- Priority order, chronological unlisted fallback, balanced ties, api-pool priority, order validation and fail-closed ineligibility.
- Revision fencing, reserve threshold including unknown/expired windows, immutable ownership across pinned/native resume, structured exhaustion and reset expiry.
- Hour/day aggregation, retention/range validation, nullable versus reported-zero tokens, cumulative deduplication, exclusion of historical Codex tokens on first observation, account/null filters and transaction rollback.
- Owner RPC shapes, desktop allowlist counts, public session labels, CLI wire filters/revisions, quota/header normalization and secret exclusion.
- Native Claude authenticated nonblocking statusline callback, OpenCode owned-message usage deduplication, Codex quota-read ordering with tool/approval responses, and existing continuation/account proxy regression suites.
- Store schema assertions and all acceptance-script schema literals are 22, including daemon literal-string contract tests and cli_deadlines_contract.rs.

## Failures investigated during validation

1. Desktop allowlist split still expected 43/30; corrected to 44/31.
2. tasklist returns Access denied even for the caller's own PID in this sandbox. Existing Claude/Antigravity cancellation helpers interpreted empty stdout as process exit. They now use local Get-Process enumeration, assert command success, and still check the real child before/after cancellation.
3. A Codex fixture waiting for a tool/approval answer consumed the added quota request. Quota reads now occur before turn/start, and the fixture implements the documented read method. This also avoids delaying an active permission request behind quota observation. The subsequent focused run caught quota events preceding acceptance evidence; admission now remains first (after retained session identity on resume).
4. OpenCode fixture sessions inherited an unreadable personal opencode.jsonc and correctly failed closed. Protocol and daemon ACP fixtures now use isolated session launch environments and their own repository boundaries. Production configuration refusal is unchanged; dedicated global-configuration tests still exercise the real refusal rules.
5. A separate CLI-deadline acceptance contract retained schemaVersion = 21; updated to 22 and included in the rerun.
6. The task-graph workspace test implicitly invoked a release build and rejected any pre-existing desktop diff. Its test invocation now reuses existing debug fixtures with SkipBuild (the profile and skipped release build are asserted in the manifest), and the script compares the desktop diff before/after instead of requiring a clean desktop checkout. It still checks that employee-only capabilities are absent.

## Remaining external checks and publication

- Not verified live: authenticated subscription quota capture, real provider exhaustion/reset behavior, actual multi-account model turns/resume, provider billing equivalence, or full production performance. Fixtures and installed schemas establish contracts, not live acceptance.
- Native Claude and Antigravity hooks do not expose spent-token directions. Legacy adapter token reports are conditional. Copilot/OpenCode/Antigravity native subscription exhaustion remains unavailable where no verified structured signal exists.
- Desktop component integration/UI build is owned by the concurrent UI work. Required account fields in existing session types must be handled by its fixtures/read models.
- Release build and full gate are the owner's pre-merge checks.
- Push and PR creation are intentionally left to the owner per the latest instruction.
- Local staging was attempted twice, including after the final passing suite, and denied: Git could not create C:/workspace/Orchestrator/.git/worktrees/copilot-account-routing/index.lock. The worktree's Git metadata lies outside the writable sandbox root. No local commit was created; no commit SHA is claimed. The owner must stage/commit from outside this sandbox. Suggested message: feat(accounts): add routing preferences and observed usage analytics.

The review-ready PR body is ../../PR-BODY.md, with a dated copy in 2026-11-03-account-routing-preferences-pr-body.md and includes exact RPC/TypeScript shapes and the per-provider observability table. No attribution lines are included.

## Local commit handoff

Run from the named worktree, outside the sandbox. These pathspecs include the source, migration, tests and documentation while excluding generated artifacts and target:

```powershell
git add -- Cargo.lock PR-BODY.md apps/lazoo-desktop/src/client.rs apps/lazoo-desktop/ui/src/lib/protocol.ts apps/lazooctl apps/lazood crates migrations docs scripts
git commit -m "feat(accounts): add routing preferences and observed usage analytics"
```

Suggested PR title: Account routing preferences and observed usage analytics. Use the saved PR body with base main.
