# Resource budget verification

Date completed: 2026-10-13 IST

Branch: `feat/resource-budget`

Base: `docs/workspace-contract` at `cb48123` (pull request #13, stacked on #12 and #11)

Product version: `0.1.0`

Database schema: 15 (no migration)

Scope: one acceptance gate for the brief's section 20.1 targets, `scripts/acceptance/resource-budget.ps1`, its daemon contract test, and continuity documentation. No production Rust code changed; the measurement uses `system.resources` from pull request #12 and the process table.

## Outcome

PASS.

The brief's core v1 definition (section 24) requires that "Lazoo stays within the baseline performance budget", and section 20.1 sets the targets for "Lazoo itself": under 1 percent sustained aggregate CPU at idle, under 5 percent during normal routing, and under 350 MiB for the idle daemon plus desktop, measured over rolling 30-second windows rather than a single sample. The new gate starts the real keeper and daemon on an isolated data root, lets startup settle, and measures keeper plus daemon over two 30-second windows: idle, and normal routing (a steady stream of `status`, `project list`, `events list`, `inbox counts`, and `roster show` calls plus one project registration). CPU share is the two processes' CPU time over wall-clock time times the machine's logical processors (12 on the baseline laptop); memory is their working sets. No provider process is started, the desktop is not running, and nothing is enforced.

Measured on the baseline laptop, with a cargo build running concurrently in the background:

| Window | CPU share | Target | Working set (keeper + daemon) | Target |
|---|---:|---:|---:|---:|
| Idle, 30 s | 0.000 (0 ms of CPU) | < 0.01 | 18.3 MiB | < 350 MiB |
| Routing, 30 s, 1945 calls | 0.0028 (1016 ms of CPU) | < 0.05 | 45.3 MiB | < 350 MiB |

No live model turn was run.

## Design notes

| Decision | Reason |
|---|---|
| Measure keeper plus daemon, not the desktop or CLI | They are the always-on part of "Lazoo itself"; the desktop is separately owned and not started by backend acceptance, and the CLI runs only while a command is active. The gate says so in its manifest. |
| Share of total machine capacity, from process CPU time over wall-clock time | Section 20.1 defines percentages that way; process time is unaffected by other load on the laptop, so the gate is stable under a concurrent build. |
| Two windows, idle and routing | Section 20.2 lists six scenarios; failure storm, memory indexing, terminal streaming, and voice need features that do not exist yet, so the gate measures the two that do and names the rest as unmeasured. |
| A three-second settle before the idle window | Startup migration and reconciliation are not idle. |
| No enforcement | Section 20.2's CPU-rate control and throttling are a separate slice; this gate only proves the targets hold today. |

## Fresh verification matrix

All commands were run from `C:\workspace\Orchestrator` on branch `feat/resource-budget` after the final change.

| Command | Exit | Evidence |
|---|---:|---|
| `cargo fmt --all -- --check` | 0 | No formatting differences. |
| `cargo clippy --workspace --all-targets --locked -- -D warnings` | 0 | Clean. |
| `cargo test -p lazood --test daemon resource_budget_acceptance_contract_measures_lazoo_itself_without_a_provider --locked` | 0 | The only Rust change in this slice is this contract test; the full workspace suite was run on the base branch (381 passed, 0 failed, 11 ignored) and no production code changed. |
| `scripts\acceptance\resource-budget.ps1 -SkipBuild` | 0 | PASS manifest, `real-binaries-no-provider`, every gate PASS, the measurements above. |
| `git diff --check` | 0 | No whitespace errors. |

Not run: `provider-adapters.ps1 -Provider codex` (live model turn) and `desktop-shell.ps1` (separately owned UI). The other acceptance scripts were run on the base branch and no production code changed.

## Resource-budget acceptance evidence

`artifacts/acceptance/resource-budget-manifest.json` records literal `PASS`, schema 15, `real-binaries-no-provider`, the processor count, the targets, both windows' measurements (CPU share, CPU milliseconds per process, working sets, handle count, call count), and gates for schema, idle CPU, idle memory, routing CPU, routing memory, no provider process, and planned shutdown.

## Boundary evidence

- `git diff --check` passed; both diffs under `apps/lazoo-desktop` are empty.
- No daemon method, CLI command, migration, event, or tool changed; the gate reads `system.resources` and the process table.
- The gate starts no provider process (`liveSessions` is 0 at the end) and sets no limit on anything.
- `UI BACKEND CONTRACT: GO` remains intentionally withheld.

## Retained follow-up boundaries

The failure-storm, memory-indexing, terminal-streaming, and voice measurements, the desktop's own share once it runs beside the daemon in acceptance, section 20.2's enforcement, and everything else NEXT-SESSION lists remain separate reviewed slices.
