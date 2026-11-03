# Skills and plugins backend verification — 2026-11-04

Status: verified. Codex's final run was stopped by the host's low-memory reaper after strict Clippy passed and before its last acceptance rerun; Claude ran the remaining checks in the foreground on the same tree (rows marked "final").

Branch `feat/skills-plugins`; base/HEAD `718eb8ecb182d5366c085232aa85e23abb734adc`. Existing work from the interrupted run was retained and corrected. Nothing was staged, committed, pushed, or changed in another checkout. Older `artifacts/*` files were left alone. The only desktop change is `apps/lazoo-desktop/src/client.rs` (17 allowlist entries and count assertions).

## Delivered behavior

Schema 23 stores the shared extension revision, desired skill ownership, and durable mutation intents. The canonical skill library is owner-protected under the data root. Bounded previews support local directories, GitHub shorthand/tree URLs, skills.sh and HTTPS Git sources. Source files are rechecked before apply. Copy and junction exposures expand to the personal harness plus existing native profiles; shared discovery roots remain read-only. Inventory reports shared visibility and duplicates. Removal refuses edited copies, changed junction targets, and unmanaged destinations.

Native plugin previews describe exact commands and current observations. Apply serializes mutations, backs up control files, rechecks state/version, invokes the CLI without interactive stdin, re-probes, and records each target's actual outcome. Marketplace identities are checked by name and normalized source. Failed native changes retain backups; config bytes that may include concurrent edits are never blindly restored. Recovery-needed blocks subsequent mutations. All RPCs, nullable fields, errors, events, bounds, capability labels and CLI shapes are documented in [UI-BACKEND-HANDOFF.md](../UI-BACKEND-HANDOFF.md#skills-and-plugins).

Resumption fixes include manifest traversal rejection, source entry/memory limits, bounded native config reads, failed/expired staging cleanup, final-state transaction verification for updates, same-volume backups outside native skill roots, marketplace identity checks, correct Claude/Copilot marketplace parsing, and refusal of Copilot's unavailable toggle commands.

## Commands and evidence

Every Cargo command ran alone with `CARGO_BUILD_JOBS=2`, `CARGO_INCREMENTAL=0`, `CARGO_PROFILE_DEV_DEBUG=0`, `CARGO_PROFILE_TEST_DEBUG=0`, and `RUST_TEST_THREADS=2`, using the existing `target/`.

| Command | Result |
| --- | --- |
| `cargo test -p lazood --test skills_plugins --locked` | PASS: 8 tests after fixture corrections. |
| `cargo build -p lazoo-session-host -p lazoo-turn-transport-fixture --locked` | PASS; normal-profile fixture binaries prepared before workspace tests. |
| `cargo fmt --all -- --check` | PASS; final rerun after the inventory and Codex parser fixes also PASS. |
| `cargo clippy --workspace --all-targets --locked -- -D warnings` | PASS; final rerun after the last fixes also PASS. |
| `cargo test --workspace --locked` | PASS: 764 passed, 23 ignored, 0 failed. Not rerun after the final parser and inventory fixes; those are covered by the focused tests in the acceptance script, and the full workspace suite runs again in the combined final gate. Log `artifacts/acceptance/skills-plugins-workspace-tests.log`. |
| `pwsh -NoProfile -ExecutionPolicy Bypass -File scripts/acceptance/skills-plugins.ps1` | PASS: 21 tests (8 daemon, 11 provider, 1 CLI, 1 schema), 5 commands, 74.368 seconds. Final rerun PASS (same 21 tests); manifest `artifacts/acceptance/skills-plugins-manifest.json`. |
| `git diff --check` | PASS; final rerun PASS. |

The daemon tests exercise preview/install/list/remove, revision refusal, unmanaged preservation, copy/junction drift, update/staging tamper, native-profile expansion, interrupted intents, native plugin preview/apply/read-back, partial target outcomes, and failed mutation/concurrent-file preservation. Provider tests cover source normalization/traversal/shadowing, plugin identities and native result shapes, unsupported commands, JSONC, closed stdin, deadline/output caps, and withheld native error text. CLI and store tests verify request mapping and schema 23. Acceptance uses only temporary redirected homes and fake native CLIs; it never performs real-user mutations or model turns.

The resumed baseline's first observed focused run passed its single existing test. No preimplementation failing-test evidence from the killed run is claimed. Initial strict-Clippy warnings and fixture-format failures were fixed; they are not represented as passing runs.

## Native checks and limits

- Codex `0.166.0-alpha.5.1`, installed at `LocalAppData/OpenAI/Codex/bin/22cfb1d302613eae/codex.exe`: a disposable app-server `skills/list` found `lazoo-legacy-probe` under `CODEX_HOME/skills`. The PATH/NVM package is older, `0.153.4`; daemon discovery remains authoritative. Evidence: `artifacts/acceptance/skills-plugins-native-check/codex-skills.json`. Current [Codex skill-root source](https://github.com/openai/codex/blob/main/codex-rs/ext/skills/src/host_roots.rs) also retains this deprecated root.
- Copilot `1.0.79`: disposable seeded `plugin list` passed. Both advertised `plugins enable/disable <name> --plugin` commands exited 1 with “The plugins command is not available.” Toggles are therefore Unsupported. Disposable local `plugin marketplace add/list/remove` passed and confirmed the text format including built-in marketplaces. Evidence: `copilot-plugins.json` and `copilot-marketplaces.json` under the same native-check directory.
- Read-only installed Claude `plugin list --json` and `plugin marketplace list --json` succeeded (22 plugins and 8 marketplaces). The native marketplace shape has top-level `source` and `repo`; the adapter handles that shape. Codex's read-only lists also returned valid `{installed,available}` and `{marketplaces}` envelopes in the inherited environment. No native secrets or raw private configuration were retained in the report.
- Antigravity help exposes no read-only skill-list command. Its exposure root is inferred from documented configuration locations; actual skill loading remains unverified. Native plugin network installs, remote skill fetches, and live cross-provider installs were not exercised. Grok Build is Unsupported. Codex/Copilot plugin toggles, Antigravity marketplace management, and OpenCode removal/toggles/Claude marketplaces are explicitly unsupported.
- No release build or full historical acceptance-script suite is claimed. No desktop UI was built for this slice. Interrupted native mutations may require manual inspection of private journals; there is no automatic blanket restore over changed native configuration.

## Exact paths to stage

Stage exactly the following 77 paths. Exclude all `artifacts/` paths, including older artifacts and disposable native-check homes. The manifests/logs are local verification evidence, not product files.

Modified (59):

```text
Cargo.lock
README.md
apps/lazoo-desktop/src/client.rs
apps/lazooctl/src/lib.rs
apps/lazooctl/tests/cli.rs
apps/lazood/Cargo.toml
apps/lazood/src/app.rs
apps/lazood/tests/cli_deadlines_contract.rs
apps/lazood/tests/daemon.rs
crates/lazoo-providers/src/discovery.rs
crates/lazoo-providers/src/lib.rs
crates/lazoo-store/src/lib.rs
crates/lazoo-store/src/migration.rs
crates/lazoo-store/tests/incident.rs
crates/lazoo-store/tests/store.rs
docs/NEXT-SESSION.md
docs/PROJECT-BRIEF.md
docs/UI-BACKEND-HANDOFF.md
scripts/acceptance/activation-policy.ps1
scripts/acceptance/approval-gate.ps1
scripts/acceptance/automatic-task-dispatch.ps1
scripts/acceptance/claude-full-autonomy.ps1
scripts/acceptance/cli-deadlines.ps1
scripts/acceptance/coalesced-notices.ps1
scripts/acceptance/collaboration-bus.ps1
scripts/acceptance/collaboration-tools.ps1
scripts/acceptance/configurable-thresholds.ps1
scripts/acceptance/conversation-delivery.ps1
scripts/acceptance/conversation-rename.ps1
scripts/acceptance/core-scenario.ps1
scripts/acceptance/desktop-budget.ps1
scripts/acceptance/desktop-inbox.ps1
scripts/acceptance/employee-roster.ps1
scripts/acceptance/first-slice.ps1
scripts/acceptance/idle-suspension.ps1
scripts/acceptance/incident-thresholds.ps1
scripts/acceptance/lead-remediation.ps1
scripts/acceptance/message-arrival.ps1
scripts/acceptance/message-priority.ps1
scripts/acceptance/minimum-incidents.ps1
scripts/acceptance/native-session-ownership.ps1
scripts/acceptance/opencode-full-autonomy.ps1
scripts/acceptance/opencode-governed-delivery.ps1
scripts/acceptance/opencode-governed-live.ps1
scripts/acceptance/peer-messaging.ps1
scripts/acceptance/provider-config-discovery.ps1
scripts/acceptance/provider-live-lifecycle.ps1
scripts/acceptance/provider-migration-preview.ps1
scripts/acceptance/raw-frame-archive.ps1
scripts/acceptance/resource-accounting.ps1
scripts/acceptance/resource-budget.ps1
scripts/acceptance/result-delivery.ps1
scripts/acceptance/session-terminal-attach.ps1
scripts/acceptance/task-delegation-graph.ps1
scripts/acceptance/user-notifications.ps1
scripts/acceptance/workspace-actions.ps1
scripts/acceptance/workspace-conversation.ps1
scripts/acceptance/workspace-read-models.ps1
scripts/gate.ps1
```

New (18):

```text
apps/lazooctl/src/extensions.rs
apps/lazood/src/app/extensions.rs
apps/lazood/src/app/extensions/inventory.rs
apps/lazood/src/app/extensions/plugins.rs
apps/lazood/src/app/extensions/skills.rs
apps/lazood/tests/skills_plugins.rs
crates/lazoo-providers/src/extensions/files.rs
crates/lazoo-providers/src/extensions/homes.rs
crates/lazoo-providers/src/extensions/mod.rs
crates/lazoo-providers/src/extensions/plugins.rs
crates/lazoo-providers/src/extensions/process.rs
crates/lazoo-providers/src/extensions/source.rs
crates/lazoo-store/src/extensions.rs
docs/superpowers/plans/2026-11-04-skills-plugins.md
docs/superpowers/specs/2026-11-04-skills-plugins-design.md
docs/verification/2026-11-04-skills-plugins.md
migrations/0023_extensions.sql
scripts/acceptance/skills-plugins.ps1
```
