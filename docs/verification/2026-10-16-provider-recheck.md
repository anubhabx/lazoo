# Provider scope and live recheck, 2026-10-16

The owner excluded Grok from core v1, confirmed OpenCode's terminal CLI works with its free models including Big Pickle, and restored Claude's subscription. After the installed Claude CLI initially reported logged out, the owner completed native sign-in.

## Current smoke results

| Provider | Result | Evidence |
| --- | --- | --- |
| Codex | PASS retained from 2026-10-15 | Exact-response adapter smoke; installed 0.162.0-alpha.17.2 |
| Antigravity | PASS retained from 2026-10-15 | Exact-response adapter smoke; installed agy 1.3.6 |
| OpenCode | PASS on 2026-10-16 | Installed 1.19.0, Big Pickle, streamed progress/thought/text/usage and exact completion token; model smoke 14.173 seconds |
| Claude | PASS on 2026-10-16 | Installed 2.1.300, native subscription ready, streamed progress/text/usage and exact completion token; model smoke 11.307 seconds |
| Grok | EXCLUDED | Owner decision; prior authentication failure remains historical evidence |

The machine-readable gate is `artifacts/acceptance/provider-adapters-manifest.json`. It retains all historical provider entries while evaluating only its explicit four-provider `requiredProviders` set. A missing required provider remains PARTIAL even if an excluded provider has evidence; an excluded failure cannot block the required set. Six local aggregation cases and PowerShell parsing passed. No Rust code or user provider configuration changed in this recheck.

The OpenCode and Claude tests used daemon SHA-256 `C146E61EE80652CCC3006BCC31574DE1F0E3821CD8FFF9990594081A941B587B` and CLI SHA-256 `02470E82ED3D59928314EE7FC85FD6BC267F4A72815CD12626A2151CC1C7291D`. These match the prior final release stress/configuration checks. Codex and Antigravity retain their earlier per-entry binary hashes; their tests were not repeated here.

## Timeout evidence and limits

OpenCode's earlier stall occurred on 1.18.23; the successful recheck used 1.19.0. The version changed between observations, but the evidence does not establish why the earlier request stalled. Lazoo's adapter and 30-second progress threshold were unchanged.

Claude's first post-login Lazoo request passed authentication but hit the 30-second no-progress threshold. Its native transcript recorded an API request and no assistant response before Lazoo stopped it. A separate, fresh diagnostic with the same streaming/permission flags and a 90-second observation ceiling completed successfully in 13.844 seconds; `artifacts/acceptance/2026-10-16-claude-native-diagnostic.json` records only frame metadata and result checks. The following fresh Lazoo smoke passed with the original 30-second threshold. No causal timeout fix is claimed. Local raw debug logs are ignored by Git and are not part of the acceptance artifact.

Logs: `artifacts/2026-10-16-opencode-recheck.log`, `artifacts/2026-10-16-claude-recheck.log`, and `artifacts/2026-10-16-claude-adapter-confirmation.log`. Structured follow-up evidence is in `artifacts/acceptance/core-v1-provider-diagnostics.json` and `core-v1-final-verification.json`.

These are short prompt/stream/completion tests. They clear the current smoke blockers, not all lifecycle, tool-authority, cancellation, resume/recovery, terminal-integration, archive, or aggregate performance requirements. Full core v1 acceptance remains open as recorded in `2026-10-15-core-v1-backend-status.md`.
