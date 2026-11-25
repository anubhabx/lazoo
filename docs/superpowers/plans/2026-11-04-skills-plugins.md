# Skills and plugins implementation plan

> Execute inline in the existing `feat/skills-plugins` worktree. Owner instructions prohibit staging, committing, pushing and additional target directories.

**Goal:** Deliver the backend contract in the companion design and PROJECT-BRIEF section 16, with exact UI RPC documentation and reproducible verification.

**Architecture:** provider filesystem/process helpers, schema-23 extension intent ledger, daemon orchestration, CLI request builders, desktop allowlist only.

1. Add failing `apps/lazood/tests/skills_plugins.rs` lifecycle tests through real App RPCs with temporary data and homes. Exercise preview before install, stale revisions and unowned collision refusal. Run `cargo test -p lazood --test skills_plugins` and record the preimplementation failure.
2. Add `crates/lazoo-providers/src/extensions/` modules for bounded sources, metadata/checksums, environment roots, exposure transactions and native plugins. Reuse OwnerSecurity, atomic_write, JobChild/JobCommand and redaction. Unit tests use controlled filesystem trees and fake native executables.
3. Add `migrations/0023_extensions.sql` and `crates/lazoo-store/src/extensions.rs`: revision-checked state/intent and audit-outbox writes. Register migration and bump every current schema assertion in store, daemon and acceptance scripts.
4. Add `apps/lazood/src/app/extensions.rs`: RPC dispatch, private preview storage, account profile expansion, serialized mutation, read-back and durable outcomes. Fail closed on interrupted intents. Add the exact request/result/errors/events contract to UI-BACKEND-HANDOFF before UI integration.
5. Add `apps/lazooctl/src/extensions.rs` command enums and request builders, register them in lib.rs, add focused CLI tests. Add only the new allowlist entries/count in desktop client.rs.
6. Add `scripts/acceptance/skills-plugins.ps1` using redirected homes and local fixtures, register it in gate.ps1. Manifest includes commands, schema, binaries and assertions. Leave existing user artifacts untouched.
7. Run `cargo fmt --all -- --check`, `cargo clippy --workspace --all-targets --locked -- -D warnings`, `cargo test --workspace --locked` and acceptance, with CARGO_BUILD_JOBS=2, CARGO_INCREMENTAL=0, CARGO_PROFILE_DEV_DEBUG=0, CARGO_PROFILE_TEST_DEBUG=0, RUST_TEST_THREADS=2. Fix actual failures and rerun affected checks.
8. Update README, PROJECT-BRIEF sections 16 and 22 Stage 5, NEXT-SESSION, UI-BACKEND-HANDOFF and the verification record. Record exact results, unsupported/unverified behavior and exact new/modified paths for Claude to stage. Inspect `git diff --check` and scope before handoff.
