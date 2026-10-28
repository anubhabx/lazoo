# Provider Migration Preview Implementation Plan

**Goal:** Serve brief section 16.1 step 6, the migration preview, as a read-only derivation over the provider configuration discovery report: one candidate per distinct integration with a proposal (`manage`, `merge`, `choose`) and Lazoo's words for it.

**Architecture:** No schema change. `lazoo-providers` gains `preview.rs` with the types and the pure `migration_preview(&ProviderConfigurationReport)` beside the discovery it reads. `lazood` serves `provider.config.preview {}` by discovering then deriving; `lazooctl provider preview` maps to it.

**Tech Stack:** Rust 1.98, clap, Tokio, PowerShell acceptance.

**Spec:** `docs/superpowers/specs/2026-10-13-provider-migration-preview-design.md`.

## Global Constraints

- No migration; schema stays 15. No change under `apps/lazoo-desktop`; `desktop.snapshot` unchanged.
- Read-only: no provider file is written, moved, or backed up; no selection is stored; no secret value is read.
- No provider smoke or live model use in acceptance.

---

### Task 1: Types and rule

- [x] `crates/lazoo-providers/src/preview.rs`: `MigrationProposal`, `MigrationSource`, `MigrationCandidate`, `MigrationSummary`, `MigrationPreview`, `migration_preview`.
- [x] `crates/lazoo-providers/tests/preview.rs`: merge, manage, and choose over a fixture profile; the secret key named and its value absent; the empty profile.

### Task 2: Daemon and CLI

- [x] `apps/lazood/src/app.rs`: `provider.config.preview` (no parameters).
- [x] `apps/lazooctl/src/lib.rs`: `provider preview`.
- [x] `apps/lazood/tests/daemon.rs` and `apps/lazooctl/tests/cli.rs`: the daemon story over a fixture profile (proposals, sources, present and missing providers, no secret, refused parameters, files unchanged, no event, snapshot unchanged), the CLI mapping, and the acceptance-script contract.

### Task 3: Acceptance and continuity

- [x] `scripts/acceptance/provider-migration-preview.ps1` writes `artifacts/acceptance/provider-migration-preview-manifest.json`.
- [x] README, NEXT-SESSION, and `docs/verification/2026-10-13-provider-migration-preview.md`.
