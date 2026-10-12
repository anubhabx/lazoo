# Provider migration preview verification

Date completed: 2026-10-13 IST

Branch: `feat/provider-migration-preview`

Base: `fix/inbox-uncertain-descendants` (pull request #23, stacked on #22 to #11)

Product version: `0.1.0`

Database schema: 15 (no migration)

Scope: brief section 16.1 step 6, the migration preview, as a read-only derivation over the discovery report (`provider.config`, steps 1 to 5): the preview types and rule in `lazoo-providers`, the daemon's `provider.config.preview`, `lazooctl provider preview`, acceptance harness, and continuity documentation. Step 7 (selection) and section 16.2 (apply) are not part of this slice; no provider file is written and nothing is stored.

## Outcome

PASS.

`provider.config.preview {}` returns one `MigrationCandidate` per distinct integration identity, ordered by name: what discovery normalized (kind, transport, command, arguments, URL with its query already stripped), the secret key names as a union over the sources and never a value, the `proposal` (`manage` when one provider holds it, `merge` when several providers hold the same identity, `choose` when another integration shares its name with a different identity, which section 16.3 says needs an explicit choice or a provider-specific alias; a conflict wins over a duplicate), Lazoo's words for it, the sources, the providers with a known configuration that hold it and those that do not, and the same-name labels it conflicts with; the discovery's unsupported entries pass through and a summary counts every proposal. The preview is recomputed from the files on every call.

No live model turn was run.

## Design notes

| Decision | Reason |
|---|---|
| The rule lives in `lazoo-providers` beside the discovery it reads | The report's types are defined there and `lazoo-domain` does not depend on the providers crate; the preview is one pure function over the report. |
| A conflict wins over a duplicate | Two harnesses holding the same server under a name a third harness uses for a different server still need the user's choice before anything is merged. |
| `presentIn` and `missingFrom` cover only providers with a known format | Grok Build and Antigravity have no documented configuration; saying they lack an integration would claim knowledge Lazoo does not have. |
| No selection, no stored preview | Step 7 is state and needs a home (Settings has no contract yet); the preview is cheap to recompute. |

## Fresh verification matrix

All commands were run from `C:\workspace\Orchestrator` on branch `feat/provider-migration-preview` after the final change.

| Command | Exit | Evidence |
|---|---:|---|
| `cargo fmt --all -- --check` | 0 | No formatting differences. |
| `cargo clippy --workspace --all-targets --locked -- -D warnings` | 0 | Clean. |
| `cargo test --workspace --locked` | 0 | One invocation, 412 passed, 0 failed, 11 ignored (explicit live-provider tests), 70 test binaries. New: `lazoo-providers` 1 (the `preview` test binary), `lazooctl` 1, `lazood` 2 (the preview story and the acceptance-script contract). |
| `cargo build --workspace --release --locked` | 0 | Locked release build (up to date in 0.6 s: the task-graph acceptance contract test inside the workspace test run had just rebuilt the release binaries from the same sources). |
| `scripts\acceptance\provider-migration-preview.ps1` | 0 | PASS schema-15 manifest, `deterministic-fake-transport`, 5 commands, every evidence flag true. |
| every other acceptance script (deterministic ones; the real-binary ones with `-SkipBuild`; `resource-budget -SkipBuild`; `native-session-ownership -SkipBuild -Provider codex`) | 0 | All 30 PASS manifests at schema 15. |
| `git diff --check` | 0 | No whitespace errors. |

Not run, as in every prior backend slice: `provider-adapters.ps1 -Provider codex` (live model turn) and `desktop-shell.ps1` (unchanged, separately owned UI).

## Provider-migration-preview acceptance evidence

`artifacts/acceptance/provider-migration-preview-manifest.json` records literal `PASS`, schema 15, and `deterministic-fake-transport`. Its focused commands prove:

- over a fixture profile with Codex and Claude Code files: the identical `context7` server in both is one `merge` candidate with two sources, present in both and missing from OpenCode, its header key named once and its value nowhere; `obsidian` in one harness is `manage`; the two `paper` servers at different addresses are each `choose` naming the other, and the one whose URL carried a token lists `url-query` as its secret key with the query stripped; `codex/ghost` passes through as unsupported; the summary counts one, one, two, and the unsupported entries; the serialized preview contains no secret; and an empty profile is an empty preview;
- `lazooctl provider preview` maps to `provider.config.preview`;
- through the daemon, the preview over the same fixture names the same proposals, sources, present and missing providers, and secret key names, refuses parameters with `LZ-INVALID-PARAMS`, leaves both configuration files byte-for-byte unchanged, emits no event, and leaves the desktop snapshot unchanged; and
- schema is 15 and the desktop tree is unchanged.

## Boundary evidence

- `git diff --check` passed; both diffs under `apps/lazoo-desktop` are empty.
- No migration, event, collaboration tool, or desktop change; the daemon gains one read-only method and the CLI one command.
- No provider file is read for secret values, written, moved, or backed up; no selection exists.
- `UI BACKEND CONTRACT: GO` remains intentionally withheld.

## Retained follow-up boundaries

Step 7 and section 16.2 (selection and apply) and everything else NEXT-SESSION lists remain separate reviewed slices.
