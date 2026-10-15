# Provider configuration discovery verification

Date completed: 2026-10-13 IST

Branch: `feat/provider-config-discovery`

Base: `feat/conversation-rename` (pull request #15, stacked on #14, #13, #12, and #11)

Product version: `0.1.0`

Database schema: 15 (no migration)

Scope: `DiscoveryEnvironment`'s config and Codex homes, the `lazoo-providers` configuration module (parsers, redaction, identity, duplicates, conflicts, unsupported), the daemon's `provider.config`, `lazooctl provider config`, the `toml` dependency, acceptance harness, and continuity documentation. The desktop app, Tauri surface, `desktop.snapshot`, and collaboration tools are unchanged; nothing writes to a provider's files.

## Outcome

PASS.

Stage 5 of the brief begins with read-only provider configuration discovery, and section 16.1 defines import as discover, parse known formats, redact, hash and normalize, and identify duplicates, equivalents, conflicts, and unsupported entries. `provider.config {}` now does the first five: for Codex, Claude Code, and OpenCode it reads the known file under the user's profile (or `CODEX_HOME`), parses only the MCP-server shapes Lazoo knows, reports each integration with its transport, command or URL, enabled flag, the names of the environment variables and headers the provider sends, and a SHA-256 identity over the normalized transport, command, arguments, and URL, and names duplicate groups (equal identities), conflict groups (same name, different identity), and unsupported providers and entries. Secret values never leave the file: a URL keeps no query string, and a malformed file is reported as not valid without being quoted. Nothing is written, moved, or stored.

No live model turn was run.

## Design notes

| Decision | Reason |
|---|---|
| One report over all providers, no per-provider parameter | The brief's import compares across harnesses (duplicates and conflicts); one call gives the whole picture and keeps the method parameterless like `provider.list`. |
| `DiscoveryEnvironment` carries the config home instead of reading `USERPROFILE` at call time | Tests point discovery at a fixture profile and never touch the developer's own configuration; the daemon's `current()` still reads the real environment once. |
| Key names only, URL query stripped, parse errors not quoted | Section 16.1 step 3 is "redact secrets"; TOML and JSON parser messages quote the offending line, which is where a token would be. |
| SHA-256 identity over normalized facts | Section 16.3 compares integrations by normalized endpoint/command, arguments, and content; a hash lets the UI and CLI say "same server" without re-deriving the rule. |
| Unsupported is a list of plain strings | Grok Build and Antigravity have no documented configuration location yet, and an entry with neither command nor URL is not an integration; naming them keeps the report honest without inventing a schema for what Lazoo does not understand. |
| `toml` 0.9 as a direct dependency | Codex's configuration is TOML; the version was already in the lock transitively, so the lock gained only the new edge. |

## Fresh verification matrix

All commands were run from `C:\workspace\Orchestrator` on branch `feat/provider-config-discovery` after the final change.

| Command | Exit | Evidence |
|---|---:|---|
| `cargo fmt --all -- --check` | 0 | No formatting differences. |
| `cargo clippy --workspace --all-targets --locked -- -D warnings` | 0 | Clean with the one fixture-test allowance. |
| `cargo test --workspace --locked` | 0 | One invocation, 391 passed, 0 failed, 11 ignored (explicit live-provider tests), 68 test binaries. New: `lazoo-providers` 2 (the `config` test binary), `lazooctl` 1, `lazood` 2 (the daemon report and the acceptance-script contract; 107 daemon tests). |
| `cargo build --workspace --release --locked` | 0 | Locked release build with the updated lock. |
| `scripts\acceptance\provider-config-discovery.ps1` | 0 | PASS schema-15 manifest, `deterministic-fake-transport`, 6 commands, every evidence flag true. |
| `conversation-rename`, `resource-accounting`, `desktop-inbox`, `configurable-thresholds`, `message-priority`, `message-arrival`, `user-notifications`, `incident-thresholds`, `lead-remediation`, `idle-suspension`, `activation-policy`, `coalesced-notices`, `peer-messaging`, `result-delivery`, `minimum-incidents`, `collaboration-tools`, `approval-gate`, `automatic-task-dispatch` | 0 | PASS schema-15 manifests. |
| `first-slice`, `employee-roster`, `collaboration-bus`, `conversation-delivery`, `task-delegation-graph`, `resource-budget`, each with `-SkipBuild` | 0 | PASS manifests against the real release binaries. |
| `scripts\acceptance\native-session-ownership.ps1 -SkipBuild -Provider codex` | 0 | PASS manifest on the first run, straight after `resource-budget`; real Codex CLI, non-model session contract only. |
| `git diff --check` | 0 | No whitespace errors. |

Not run, as in every prior backend slice: `provider-adapters.ps1 -Provider codex` (live model turn) and `desktop-shell.ps1` (unchanged, separately owned UI). `npm --prefix apps\lazoo-desktop\ui run check` was not run because the frontend is unchanged.

## Provider-config-discovery acceptance evidence

`artifacts/acceptance/provider-config-discovery-manifest.json` records literal `PASS`, schema 15, and `deterministic-fake-transport`. Its focused commands prove:

- over a fixture profile holding Codex TOML, Claude Code JSON, and OpenCode JSON with a secret in every position (environment, headers, URL query), all three files are found and parsed, eight integrations are reported in one shape, no secret value survives anywhere in the serialized report, only key names are listed (`url-query` for a stripped query string), the same server configured three ways (case and trailing slash aside) is one duplicate group, the same name at two servers is one conflict group, entries with neither command nor URL and non-object entries are named as unsupported beside Grok Build and Antigravity, and every identity is a 64-character hash;
- an empty environment reports no files and five unsupported providers; a malformed Codex file is reported as existing, unparsed, and not valid without quoting it while the OpenCode file still parses; an explicit `CODEX_HOME` wins over the profile;
- `lazooctl provider config` maps to `provider.config`;
- through the daemon, a fixture profile's two configured copies of one server are reported as one duplicate group with secrets redacted, parameters are refused with `LZ-INVALID-PARAMS`, the desktop snapshot carries no configuration, and the provider file is untouched; and
- schema is 15 and the desktop tree is unchanged.

## Boundary evidence

- `git diff --check` passed; both diffs under `apps/lazoo-desktop` are empty.
- No Tauri command, capability, `desktop.snapshot` field, frontend source, migration, event, or collaboration tool changed; the daemon gains `provider.config` and `lazooctl provider` gains `config`.
- No provider file is written, moved, backed up, or re-probed; no secret value is reported.
- `UI BACKEND CONTRACT: GO` remains intentionally withheld.

## Retained follow-up boundaries

Section 16.1 steps 6 and 7 and section 16.2 (preview, selection, apply, backup, rollback), plugins, skills, connectors, project-scoped files, the Grok Build and Antigravity formats, and everything else NEXT-SESSION lists remain separate reviewed slices.
