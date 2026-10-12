# Provider Configuration Discovery Implementation Plan

**Goal:** Give the user a read-only, secret-free view of what each installed provider has configured (MCP servers today), with duplicates and conflicts named, as the first step of the brief's configuration federation.

**Architecture:** No schema change. `lazoo-providers` gains `config.rs` (`discover_provider_configuration` over `DiscoveryEnvironment`, which now carries the config home and `CODEX_HOME`): known file per provider, TOML/JSON parsing of known shapes only, redaction, SHA-256 identity, duplicate and conflict groups. `lazood` serves `provider.config`; `lazooctl provider config` maps to it. `toml` becomes a direct dependency (already in the lock transitively).

**Tech Stack:** Rust 1.98, `toml` 0.9, `serde_json`, `sha2`, clap, Tokio, PowerShell acceptance.

**Spec:** `docs/superpowers/specs/2026-10-13-provider-config-discovery-design.md`

## Global Constraints

- Read-only: nothing under a provider's profile is written, moved, or backed up.
- Secrets are never reported: key names only, URL query strings stripped, malformed files not quoted.
- No change under `apps/lazoo-desktop`; `desktop.snapshot` unchanged; no tool access.
- No provider smoke or live model use in acceptance.

---

### Task 1: Providers

- [x] `crates/lazoo-providers/src/discovery.rs`: `config_home` and `codex_home` on `DiscoveryEnvironment` (`current()` from `USERPROFILE` and `CODEX_HOME`; `with_config_home`, `with_codex_home`).
- [x] `crates/lazoo-providers/src/config.rs`: report types, the three parsers, redaction, identity, duplicates, conflicts, unsupported.
- [x] `crates/lazoo-providers/tests/config.rs`: fixture profiles with secrets in every position; missing, malformed, and `CODEX_HOME` cases.

### Task 2: Daemon and CLI

- [x] `apps/lazood/src/app.rs`: `provider.config` (empty params only).
- [x] `apps/lazooctl/src/lib.rs`: `provider config`.
- [x] `apps/lazood/tests/daemon.rs` and `apps/lazooctl/tests/cli.rs`: the report through the daemon with secrets redacted and the snapshot unchanged, the parameter refusal, the CLI mapping, and the acceptance-script contract.

### Task 3: Acceptance and continuity

- [x] `scripts/acceptance/provider-config-discovery.ps1` writes `artifacts/acceptance/provider-config-discovery-manifest.json`.
- [x] README, PROJECT-BRIEF Stage 5 status, NEXT-SESSION, and `docs/verification/2026-10-13-provider-config-discovery.md`.
