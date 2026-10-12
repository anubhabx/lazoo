# Provider Migration Preview Design

**Date:** 2026-10-13

**Status:** Implemented on `feat/provider-migration-preview` (pull request #24, stacked on #23) with deterministic acceptance, for owner review before merge.

**Scope:** Brief section 16.1 step 6, "present a migration preview", as a read-only derivation over the discovery report that pull request #16 already serves (`provider.config`, steps 1 to 5). It says, per integration, what Lazoo would do if the user asked it to manage their integrations, using section 16.3's duplicate and conflict rules. Step 7 (the user's selection, which is state) and section 16.2 (apply, with validation, diff, backup, atomic write, re-probe, rollback, and recorded desired versus actual state) stay outside it and still need the owner's design.

## 1. Why

Discovery names duplicates and conflicts as label groups, which is enough for a report but not for a decision: the user needs one row per integration with a proposal and a reason. Section 16.3 defines the comparison (normalized endpoint or command, arguments, environment-key names) that discovery already hashes into `identity`, and says what a same-name conflict requires ("an explicit choice or provider-specific alias"). Nothing else is needed to derive the preview.

## 2. Read model

```text
provider.config.preview {}   →   lazooctl provider preview
result: { version: 1, scannedAt, providers: ProviderKind[], candidates: MigrationCandidate[],
          unsupported: string[], summary: { manage, merge, choose, unsupported } }
```

One `MigrationCandidate` per distinct `identity`, ordered by name then identity:

| Field | Meaning |
|---|---|
| `identity`, `name`, `kind`, `transport`, `command`, `args`, `url` | the integration as discovery normalized it (URL query already stripped, secrets already redacted) |
| `secretKeys` | the union of the sources' secret key names; no value is ever read |
| `proposal` | `manage` (one provider holds it), `merge` (several providers hold the same identity: one managed entry for all), `choose` (another integration shares its name with a different identity) |
| `reason` | Lazoo's words for the proposal |
| `sources` | provider, name, file, enabled, per holding provider |
| `presentIn`, `missingFrom` | the providers with a known configuration format that hold it and that do not |
| `conflictsWith` | `provider/name` labels of the same-name integrations with another identity |

`providers` is the discovery's `files` order (Codex, OpenCode, Claude Code); Grok Build and Antigravity have no documented format and stay in `unsupported` with the entries discovery could not classify.

## 3. Rules

1. A conflict wins over a duplicate: an identity whose name collides with another identity is `choose`, even if two providers hold it identically.
2. Identity equality is section 16.3's comparison, as discovery already computes it; the preview adds no comparison of its own.
3. Nothing is selected, written, or stored; the preview is recomputed on every call from the files as they are.
4. No secret value is read into the preview; `secretKeys` are names.

## 4. Boundaries

No provider file is written, moved, or backed up. No selection is remembered. No tool can read the preview. The desktop and `desktop.snapshot` are unchanged.

## 5. Next

Step 7 and section 16.2 need the owner's design: where the selection lives (Settings has no contract yet), how a `choose` is answered (pick one, or alias per provider), the rendering of a managed entry into each provider's native format with its compatibility class, and the backup, atomic write, re-probe, and rollback sequence.
