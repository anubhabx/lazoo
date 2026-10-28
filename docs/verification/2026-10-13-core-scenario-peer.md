# Core scenario peer verification (step 7)

Date completed: 2026-10-13 IST

Branch: `test/core-scenario-peer`

Base: `docs/desktop-backend-contract-index` (pull request #30, stacked on #29 to #11)

Product version: `0.1.0`

Database schema: 15 (no migration)

Scope: the core scenario story (`the_core_scenario_runs_end_to_end_on_the_deterministic_provider`) now proves the brief's step 7 itself: an employee asks a peer a direct clarification without routing through Rhea. The acceptance script's step map and the continuity documentation follow. No production code changed.

## Outcome

PASS; step 7 moves from PROVEN ELSEWHERE to PASS inside the story.

The story's cast gains one Codex peer, Nico, beside Mira and the three other-provider peers (the four peers across different providers the brief names are unchanged; the other-provider peers cannot be dispatched until other-provider delivery exists, so a sibling exchange needs a second Codex worker). Rhea's plan delegates the tests to Nico and the fix to Mira. Nico's dispatch turn waits on Mira's task through `task_wait`, a sibling in the same tree; inside her own dispatch turn Mira asks Nico through `message_send` and waits for his reply; his wait returns early with her question, he answers through `message_reply`, her wait returns his answer, and both turns finish. The ledger holds the question on Nico's task and the reply on Mira's, and no peer turn ever reaches Rhea. The two dispatches run concurrently without timing tricks: the first pass takes Nico's task and stays inside his waiting turn, and a second pass sees only Mira's task because his already has a dispatch row; the first pass then finds her task dispatched and reports that, which the story accepts.

The rest of the story is unchanged in substance: sessions still count one per task, each task still runs in its own worktree under the dirty checkout, Mira's commit and verification evidence is still reported (step 12), the stall, the remediation, the restart, and the idle suspension run as before.

No live model turn was run.

## Fresh verification matrix

All commands were run from `C:\workspace\Orchestrator` on branch `test/core-scenario-peer` after the final change, through `scripts\gate.ps1`.

| Command | Exit | Evidence |
|---|---:|---|
| `cargo fmt --all -- --check` | 0 | No formatting differences. |
| `cargo clippy --workspace --all-targets --locked -- -D warnings` | 0 | Clean. |
| `cargo test --workspace --locked` | 0 | 414 passed, 0 failed, 11 ignored (explicit live-provider tests), 70 test binaries; no new test, the story grew. |
| `cargo build --workspace --release --locked` | 0 | Locked release build (up to date in 0.6 s: the task-graph acceptance contract test inside the workspace test run had just rebuilt the release binaries from the same sources). |
| `scripts\acceptance\core-scenario.ps1` | 0 | PASS schema-15 manifest; `steps.step7.status` is `PASS` after the story. |
| every other acceptance script (deterministic ones; the real-binary ones with `-SkipBuild`; `resource-budget -SkipBuild`; `native-session-ownership -SkipBuild -Provider codex`) | 0 | All 30 PASS manifests at schema 15; `gate-manifest.json` records all 35 steps PASS. |
| `git diff --check` | 0 | No whitespace errors. |

Not run, as in every prior backend slice: `provider-adapters.ps1 -Provider codex` (live model turn) and `desktop-shell.ps1` (unchanged, separately owned UI).

## Boundary evidence

- No production code changed: the diff is the daemon story, the acceptance script's step map, and documentation.
- `git diff --check` passed; both diffs under `apps/lazoo-desktop` are empty.
- `UI BACKEND CONTRACT: GO` remains intentionally withheld.

## Retained follow-up boundaries

If the owner prefers the brief's cast exactly, the second Codex peer can be removed and step 7 returns to the lead-recipient proof in `peer-messaging`. Step 12's memory records need Stage 5; step 15 stays a real-binary measurement.
