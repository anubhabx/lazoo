# Provider Configuration Discovery Design

**Date:** 2026-10-13

**Status:** Implemented on `feat/provider-config-discovery` (stacked on `feat/conversation-rename`, pull request #15) with deterministic acceptance, for owner review in a pull request before merge.

**Scope:** Backend-only, no schema change, read-only. One daemon method, `provider.config`, and `lazooctl provider config`, implementing the first five steps of the brief's section 16.1 import (discover, parse known formats, redact, hash and normalize, name duplicates and conflicts). No preview, selection, apply, backup, or rollback (steps 6 and 7 and section 16.2); no desktop, Tauri, or collaboration-tool change; no live model use.

## 1. Outcome

Stage 5 begins with "read-only provider config discovery". Lazoo already discovered provider executables; it knew nothing about what each provider had configured. The user can now run `lazooctl provider config` and see, in one report:

- which configuration file Lazoo looked for per provider (`~/.codex/config.toml` or `CODEX_HOME/config.toml`; `~/.claude.json`; `~/.config/opencode/opencode.json`), whether it exists, whether it parsed, and a message when it did not;
- every MCP server each provider has configured, as Lazoo's one integration shape: transport (`stdio` or `http`), command and arguments or URL, whether it is enabled, the names of the environment variables and headers the provider will send, and a SHA-256 identity;
- `duplicates`: groups of `provider/name` whose identities are equal (the same server configured in several harnesses);
- `conflicts`: groups of `provider/name` that share a name but point at different servers;
- `unsupported`: providers with no known configuration format (Grok Build, Antigravity) and entries Lazoo could not classify.

## 2. Rule

- Only known formats are parsed: Codex's `[mcp_servers.<name>]` TOML (`command`/`args`/`env`, or `url`/`http_headers`/`env_http_headers`, `enabled`), Claude Code's `mcpServers` JSON (`command`/`args`/`env`, or `type: http|sse` with `url`/`headers`), and OpenCode's `mcp` JSON (`type: local` with a `command` array and `environment`, or `type: remote` with `url`/`headers`, `enabled`).
- Secrets never leave the file: environment and header values are dropped and only their key names are reported; a URL keeps no query string and `url-query` is named among the secret keys when one was stripped; a malformed file is reported as not valid without quoting it.
- Identity is the SHA-256 of the normalized transport, command, arguments, and URL (case-folded, path separators and a trailing slash normalized), so the same server is the same identity wherever and however it is configured.
- A missing file is a fact, not an error; one unreadable or malformed file does not hide the others.
- Nothing is written, moved, backed up, or re-probed. Provider-native files stay authoritative.

## 3. Rules kept

- `DiscoveryEnvironment` carries the config home (`USERPROFILE`) and `CODEX_HOME`, so tests point discovery at a temporary profile and never read the developer's own files.
- `desktop.snapshot` and the collaboration tools are unchanged; no tool can read a provider's configuration.
- The report is derived on every call and never stored.

## 4. Deterministic acceptance

`scripts/acceptance/provider-config-discovery.ps1` runs the providers, CLI, and daemon proofs over fixture profiles and writes `artifacts/acceptance/provider-config-discovery-manifest.json`. No provider smoke or live model turn.

## 5. Explicitly deferred work

- section 16.1 steps 6 and 7 (migration preview, user selection of what Lazoo manages) and section 16.2 apply, backup, validation, and rollback;
- plugins, skills, and connectors; project-scoped files (`.mcp.json`, `.codex/config.toml` in a repository); Claude Code `settings.json` enablement lists;
- Grok Build and Antigravity formats, once their configuration locations are documented;
- a desktop Settings surface (the separately owned UI).

## 6. Review gate

Merge authorizes only the read-only report above and its CLI command; it does not authorize any write to a provider's configuration.
