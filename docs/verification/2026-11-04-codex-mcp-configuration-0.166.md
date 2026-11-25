# Codex 0.166.0-alpha.5.1 selected MCP configuration

The installed Codex auto-updated to `codex-cli 0.166.0-alpha.5.1`
(`...\Codex\bin\22cfb1d302613eae\codex.exe`, SHA-256 `3097c909...0e16`). The core-v1
gate then failed `configuration-lifecycle` with `LZ-CONFIG-VERSION-UNSUPPORTED`. The
exact-version guard worked as intended.

The [0.165 parser check](2026-10-29-codex-mcp-configuration.md) was repeated with
the same fixture `config.toml` in a fresh disposable `CODEX_HOME`. Native
`mcp list --json`, `mcp get fixture_stdio --json` and `mcp get fixture_http --json`
exited 0. Their output is byte-identical to the reviewed 0.165 evidence:
disabled stdio (`type: stdio`, command and args preserved) and disabled URL
(`type: streamable_http`). Evidence is in `artifacts/readiness/codex-config-0.166/`.
No MCP server was enabled, no model turn was sent, and no personal provider
configuration was read or written.

Only this exact version is added to the reviewed-version allowlist. The regression
test also asserts that the neighboring `0.166.0-alpha.5.2` is refused.
`configuration-lifecycle.ps1 -SkipBuild` then passed against fresh release binaries:
preview, apply, lost-reply replay, show and rollback all passed.
