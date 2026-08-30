# Native Provider Adapters Verification — 2026-08-30

## Outcome

The backend connector slice is complete for Codex, OpenCode, Grok Build, Antigravity, and Claude Code. Codex, OpenCode, Grok Build, and Antigravity passed fresh exact-output model smokes through the full `lazooctl -> named pipe -> lazood -> native adapter` path. Claude Code passed installation, version, authentication-status, and stream-protocol startup checks, but its model gate is blocked by the configured organization's access policy.

The generated evidence is [provider-adapters-manifest.json](../../artifacts/acceptance/provider-adapters-manifest.json). It stores executable filename and path SHA-256 metadata, not raw executable paths, credentials, identity fields, session identifiers, prompts, or raw provider payloads.

## Provider evidence

| Provider | Installed version | Native protocol | Non-model probe | Model-backed gate | Resume evidence |
|---|---|---|---|---|---|
| Codex | `codex-cli 0.150.0-alpha.12.2` | App Server v2 | PASS, `configured` | PASS, exact `LAZOO_CODEX_OK` | PASS, App Server thread resume |
| OpenCode | `1.18.23` | ACP v1 | PASS, handshake `unverified` until model auth is exercised | PASS, exact `LAZOO_OPENCODE_OK` | PASS, exact `LAZOO_OPENCODE_RESUME_OK` |
| Grok Build | `grok 1.0.13 (5e9a58528b76) [stable]` | ACP v1 | PASS, handshake `unverified` until model auth is exercised | PASS, exact `LAZOO_GROK_OK` | PASS, exact `LAZOO_GROK_RESUME_OK` |
| Antigravity | `1.1.22` | `agy` stream JSON | PASS, `configured` | PASS, exact `LAZOO_AGY_OK` | PASS, same-process continuation and exact `LAZOO_AGY_RESUME_OK` |
| Claude Code | `2.1.241 (Claude Code)` | Claude stream JSON | PASS, `configured`; native auth ready and external auth unknown | BLOCKED | BLOCKED before model execution |

Claude's exact live blocker is:

> Your organization has disabled Claude subscription access for Claude Code · Use an Anthropic API key instead, or ask your admin to enable access

The adapter normalized that terminal record as a provider error even though Claude's native result subtype was `success`; it was not upgraded to a completed turn. Native subscription evidence and external-endpoint credential evidence remain separate.

OpenCode and Grok intentionally report their non-model ACP handshakes as `unverified`, because ACP initialization alone does not prove model authentication. Their fresh model-backed exact-output gates provide the stronger live evidence recorded above.

## Repository gates

All commands ran from `C:\workspace\Orchestrator\.worktrees\provider-adapters` on branch `codex/provider-adapters`.

| Command | Exit | Evidence |
|---|---:|---|
| `scripts\verify-rust-toolchain.ps1` | 0 | Rust/Cargo 1.98.0 MSVC toolchain resolved through rustup |
| `cargo test --workspace` | 0 | 99 passed, 0 failed, 10 ignored live-only tests |
| `cargo check --workspace --all-targets` | 0 | PASS |
| `cargo clippy --workspace --all-targets -- -D warnings` | 0 | PASS, zero warnings |
| `cargo fmt --all -- --check` | 0 | PASS |
| `cargo build --workspace --release --locked` | 0 | PASS |
| `scripts\acceptance\first-slice.ps1` | 0 | PASS: release, control plane, project mutation, restart durability, restart budget/spool, notification API, and exactly-once recovery |
| `scripts\acceptance\provider-adapters.ps1 -Provider codex` | 0 | Provider status PASS |
| `scripts\acceptance\provider-adapters.ps1 -Provider opencode` | 0 | Provider status PASS |
| `scripts\acceptance\provider-adapters.ps1 -Provider grok-build` | 0 | Provider status PASS |
| `scripts\acceptance\provider-adapters.ps1 -Provider antigravity` | 0 | Provider status PASS |
| `scripts\acceptance\provider-adapters.ps1 -Provider claude-code` | 0 | Diagnostic completed; provider status BLOCKED with the exact account-policy reason above |

The ten ignored workspace cases are explicit live-only tests. On the final code, the model/resume live tests were rerun individually for Codex, OpenCode, Grok Build, and Antigravity and each exited 0. Claude's live model test reached the native terminal error and could not proceed to resume; fixture and contract tests cover its UUID/resume command construction and error normalization.

## Requirement coverage

- A versioned provider contract now covers discovery, probe truth, capabilities, authentication evidence, lifecycle, and normalized events.
- The registry lists all five providers in stable order without launching them. `provider.probe` performs a bounded non-model readiness check. `provider.smoke` is explicit and quota-consuming.
- Codex uses App Server v2; OpenCode and Grok use ACP v1 schema types; Antigravity and Claude use separate long-lived stream-JSON schemas over shared bounded NDJSON process plumbing.
- Direct children use structured arguments, explicit working directories, redirected streams, output bounds, progress/turn/stop deadlines, and `kill_on_drop`. No adapter constructs a shell command or passes a dangerous permission-bypass flag.
- Grok always receives an explicit safe permission baseline and never inherits the installed `always-approve` configuration silently.
- Provider RPC errors use stable `LZ-PROVIDER-*` codes; `lazooctl` maps that family to exit code 8.
- Provider smoke transcripts remove raw payloads, cap individual strings at 64 KiB, and cap the returned transcript at 512 KiB.
- `desktop.snapshot` is unchanged and contains no provider field. The branch changes zero files under `apps/lazoo-desktop`; the main worktree remained clean.

## Deliberate boundaries

This slice owns diagnostic single-turn sessions, not durable employee/session ownership. Windows Job Objects, ConPTY terminal takeover, task delegation, worktree automation, approval UI, and persistent team messaging remain later slices. No desktop UI or frontend dependency work was performed.

Claude remains implemented but model-blocked in the present environment. A future successful smoke requires either organization enablement for Claude Code or a separately configured external Anthropic endpoint credential; Lazoo does not read, copy, or migrate either credential channel.
