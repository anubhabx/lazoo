# Claude Code full-autonomy delivery verification

Date completed: 2026-10-13 IST

Branch: `feat/claude-full-autonomy`

Base: `test/resource-budget-failure-storm` (pull request #32, stacked on #31 to #11)

Product version: `0.1.0`

Database schema: 15 (no migration)

Scope: one rule (`provider_enforces_authority`) behind the dispatch and turn delivery gates, so Claude Code employees receive turns and tasks under `full-autonomy` only; its deterministic story; acceptance harness; and continuity documentation. No adapter, protocol, store, CLI, or desktop change. No live gate has run, and no support is claimed.

## Outcome

PASS.

Before this slice every non-Codex provider was refused at the two gates. Now a Codex lead under full autonomy delegates to a Claude Code worker whose authority is full autonomy, the task is accepted, dispatched on a Claude Code session of its own, and completed; the same lead's delegation to a Claude Code worker whose authority is `ask` is proposed, accepted by the user, and refused at dispatch with the task failed (`LZ-TASK-AUTHORITY-UNSUPPORTED`) and an `LZ-CONFIG-INCOMPATIBLE` incident, before any provider I/O; a delegation to an Antigravity worker under full autonomy is refused the same way, because Antigravity has no prompt-free mode. A Claude Code lead refuses a user turn under `ask` before any I/O and completes one under full autonomy on a session it activates for it. Requesting an authority wider than the assignee's ceiling is still refused as authority expansion. The existing before-I/O refusal test is unchanged and still passes.

No live model turn was run.

## Design notes

| Decision | Reason |
|---|---|
| Only Claude Code, only `full-autonomy` | Its `--permission-mode auto` is the one mode in which no prompt can occur; under `ask` and `self-decision` its prompts stay inside its own stream where Lazoo cannot answer them (`capabilities.permission: false`). |
| Antigravity stays refused under every authority | Its adapter maps only `plan` and `accept-edits` explicitly and refuses `auto`; `full-autonomy` has no native mode to enforce it. |
| OpenCode and Grok Build stay refused | The ACP adapter cancels every `session/request_permission`; a worker would be denied every governed action under any authority. |
| A turn with no task fails closed | Every delivered turn belongs to a task (root, child, or recipient); an orphan has no authority to check. |
| No support claim | Brief section 21.3: the roster says Detected, Configured, Degraded, or Unverified until the provider's live gate passes; this slice adds a deterministic path, not a gate. |

## Fresh verification matrix

All commands were run from `C:\workspace\Orchestrator` on branch `feat/claude-full-autonomy` after the final change, through `scripts\gate.ps1`.

| Command | Exit | Evidence |
|---|---:|---|
| `cargo fmt --all -- --check` | 0 | No formatting differences. |
| `cargo clippy --workspace --all-targets --locked -- -D warnings` | 0 | Clean with the one story allowance. |
| `cargo test --workspace --locked` | 0 | 416 passed, 0 failed, 11 ignored (explicit live-provider tests), 70 test binaries. New: `lazood` 2 (the story and the acceptance-script contract). |
| `cargo build --workspace --release --locked` | 0 | Locked release build (up to date in 0.6 s: the task-graph acceptance contract test inside the workspace test run had just rebuilt the release binaries from the same sources). |
| `scripts\acceptance\claude-full-autonomy.ps1` | 0 | PASS schema-15 manifest, `deterministic-fake-transport`, 4 commands, every evidence flag true. |
| every other acceptance script | 0 | All 31 PASS manifests at schema 15; `gate-manifest.json` records all 36 steps PASS. |
| `git diff --check` | 0 | No whitespace errors. |

Not run, as in every prior backend slice: `provider-adapters.ps1 -Provider codex` (live model turn) and `desktop-shell.ps1` (unchanged, separately owned UI). Not run and still owed before any claim of support: a live Claude Code gate (session, user turn, dispatched task, cancellation, resume) through a configured endpoint.

## Boundary evidence

- `git diff --check` passed; both diffs under `apps/lazoo-desktop` are empty.
- No schema, protocol, store, CLI, adapter, or desktop change; the daemon changes one rule at two gates.
- Collaboration tools stay Codex-only; a Claude Code session receives none.
- `UI BACKEND CONTRACT: GO` remains intentionally withheld.
