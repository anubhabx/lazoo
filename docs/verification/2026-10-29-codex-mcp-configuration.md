# Codex 0.165.0-alpha.1.2 selected MCP configuration

The installed executable used by Lazoo reports `codex-cli 0.165.0-alpha.1.2`.
Current official [MCP configuration types](https://github.com/openai/codex/blob/main/codex-rs/config/src/mcp_types.rs)
and [CLI serialization](https://github.com/openai/codex/blob/main/codex-rs/cli/src/mcp_cmd.rs)
were checked through Context7. Main-branch documentation alone does not establish
compatibility with this installed alpha; the following local parser check does.

With a fresh disposable `CODEX_HOME`, native `mcp list --json` and
`mcp get <name> --json` accepted both supported writer shapes:

- Disabled stdio: `command = "fixture-mcp"`, arguments `--stdio`, `--safe`.
  Native output preserved the values and reported `type: stdio`, `enabled: false`.
- Disabled URL: `url = "https://example.invalid/mcp"`. Native output preserved it
  and reported `type: streamable_http`, `enabled: false`.

Evidence is in `artifacts/readiness/codex-config-0.165/`: `config.toml`, `list.json`,
`stdio.json`, and `http.json`. No MCP server was enabled, no model turn was sent,
and no personal provider configuration was read or written by the parser check.

Only this exact version is added to the existing reviewed-version allowlist. The
neighboring unreviewed `0.165.0-alpha.1.3` remains refused by the regression test.
The new positive assertion failed before the allowlist change. No new writer
fields, transports, providers, plugins or skills are admitted.

The lifecycle acceptance script now probes the executable the daemon actually
discovers and records its path, version and SHA-256. It no longer demands an old
installation that may have been removed by an update. The backend still applies
its exact-version guard. Preview, apply, lost-reply replay, show and byte-exact
rollback passed in a separate disposable profile against fresh release binaries.
The final complete gate and binary identity are recorded in the UI-readiness
verification record.
