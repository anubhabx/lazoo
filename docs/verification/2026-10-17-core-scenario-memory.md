# Core scenario memory records verification

Date completed: 2026-10-17 IST

Branch: `worktree-wf_0f2eb737-354-3` (integrated by the owner's integrator)

Base: `f98ab93` (the `codex/core-v1-backend` checkpoint with the schema 16-18 memory pipeline)

Product version: `0.1.0`

Database schema: 18 (no migration)

Scope: brief section 21.2 step 12 ("Results include commits, verification, task links, messages, incidents, and concise memory records") inside the deterministic core scenario. One `#[doc(hidden)] pub` worker entry point, the story's memory-record chapter, the step 12 flip in `scripts/acceptance/core-scenario.ps1`, and continuity documentation. No RPC, CLI command, migration, event, tool, or desktop file changed.

## Outcome

PASS on the deterministic fake transport, with the boundary below.

`docs/verification/2026-10-13-core-scenario.md` recorded step 12 as PARTIAL: commits, verification, task links, messages, and incidents were proven, memory records needed Stage 5. The schema 16-18 pipeline (compressed canonical turn archives, the typed memory ledger, the durable summary queue, and the opt-in summary worker) now exists, so the story proves the last clause the same way it proves steps 1-11, 13, and 14: in one daemon lifetime on `FakeSessionBackend`, on the restarted boot, after step 14.

What the story now does and asserts, on the restarted `App`:

- Sage, a dedicated dormant Codex employee with model `test-model`, is created and never warmed or dispatched. `memory.worker.configure { projectId, employeeId: sage, model: "test-model", requestsPerMinute: 1, enabled: true }` answers `settings.enabled: true`, `provider: "codex"`, `model: "test-model"`, `projectPermission.state: "nativeEmployeeOnly"`.
- `memory.archive { sourceTurnId: <Mira's dispatch turn>, projectId }` answers `archive.sourceTurnId` and `job.sourceTurnId` equal to that turn, `archive.projectId` equal to the project, `archive.truncated: false`, `job.state: "pending"`, and the compressed archive named by `archive.archiveName` is a file under `<data root>/memory-archives`. `Store::read_memory_source` on that root, the same read the worker makes before it summarizes, returns the archive's decompressed bytes (it refuses a file whose size, zstd payload, or SHA-256 differs from the recorded row), and they parse as Mira's redacted dispatch turn: `id` is her turn and `taskId` her task.
- The shared fake `turn_events` are swapped so the next `TurnCompleted.output` is strict JSON with one `achievement` record; this is the story's last provider turn, because the swap is global to the fake backend.
- `App::summarize_next_memory_job_once()` returns `Ok(true)`: the store claimed the one pending job (first admission in the scope, `last_admitted_at` NULL, Sage dormant) and `process_memory_job` ran it to completion.
- The last captured `TurnRequest` has `read_only: true` and its prompt starts with `[LAZOO_INTERNAL_MEMORY_SUMMARY]`.
- `memory.queue.status { projectId, limit: 20 }` has `jobs[0].state: "completed"` and `archiveErrorCount: 0`.
- `session.list` shows exactly one session for Sage, in state `suspended`: the summary ran on a fresh session that was stopped after use and never left idle.
- `memory.search { projectId, query: "", taskId: <Mira's task>, limit: 10 }` returns a record with `sourceTurnId` equal to Mira's dispatch turn, `sourceHash` equal to `archive.sourceHash`, `recordType: "achievement"`, `active: true`, and the scripted text.
- `desktop.snapshot` is still roster-neutral afterwards.

The step 12 evidence text in `core-scenario.ps1` names this chain, `step12` joined the PASS loop, and the PARTIAL override is gone. The manifest keeps `mode: deterministic-fake-transport`, `schemaVersion: 18`, `schemaMigration: NONE`, and `providers: FAKE_TRANSPORT_FOR_EVERY_PROVIDER`.

Not proven: model summary quality. The summary turn's output is a scripted JSON string on the fake transport, exactly as every other provider turn in the story is scripted. What is proven is the worker pipeline around a summary: explicit opt-in on a dedicated dormant employee, archive integrity as the store defines it (the on-disk compressed file's size, zstd payload, and SHA-256 match the recorded row, read back through `read_memory_source`; the story does not recompute the hash itself), archive scope, durable queue admission, a fresh read-only reserved session, strict typed validation of the output, source citation (turn id and hash) on the record, scoped retrieval by task, and session teardown. No live model turn was run. No real daemon binary was launched; `core-scenario.ps1` has never launched one.

## Design notes

| Decision | Reason |
|---|---|
| `run_memory_worker_once` keeps its throttle check and delegates to a new `#[doc(hidden)] pub async fn summarize_next_memory_job_once` | Test `App`s have no resource governor, so `background_throttled()` is always true and the throttled path returns `Ok(false)` before claiming. The daemon loop still calls only the throttled path; the unthrottled entry point follows the `dispatch_pending_tasks_once` and `deliver_pending_notices_once` precedent for integration stories. |
| The memory chapter runs on the restarted boot, after step 14 | The worker must run on the live boot, and the fake backend's `turn_events` are one shared list: the swapped JSON output would become every later turn's output, so the summary must be the story's last provider turn. |
| A dedicated dormant employee, Sage, not Mira or Nico | The claim refuses an employee with any starting, idle, or stopping session, and the status document's rule is that ordinary active sessions are never repurposed for background summaries. Mira was warmed in step 14; Nico has a session history. |
| Only Mira's dispatch turn is archived | Test `App`s never spawn the 60-second archiver, so exactly one job exists; a second job in the same scope would wait on per-scope admission spacing. |
| The step 12 row flips to PASS with the fake-summarizer boundary stated in the evidence text | Steps 1-11, 13, and 14 are accepted on the same fake transport; the manifest and this document say what the fake proves and what it does not. The `PARTIAL` legend comment on the script's line 27 stays because the acceptance-contract test requires that literal. |
| The scenario's `schemaVersion` literal is untouched | Another slice bumps the schema; the integrator aligns literals after merging. |
| `README.md` is not edited here, although its core-scenario paragraph still says "step 12 is partial (...; memory records need Stage 5)" | The integrator owns `README.md`, `docs/PROJECT-BRIEF.md`, and `docs/NEXT-SESSION.md` across the parallel slices. On merge, that clause becomes: "step 12 is proven in the story too (the first worker reports completion through `task_report` with commit and verification evidence that is the task's durable evidence through `task.show`, and the result notice links the task; on the restarted daemon the completed task's dispatch turn is archived through `memory.archive`, a dedicated dormant Codex summary employee opted in through `memory.worker.configure` writes a typed memory record citing that turn and the archive's source hash, and `memory.search` reads it back by task; the summary output is scripted on the fake transport, so the worker pipeline is proven and model summary quality is not)". The step 12 row of `docs/verification/2026-10-13-core-scenario.md` names this worktree branch as its history; the integrator replaces it with the final branch and pull request number, as every other row does. |

## Fresh verification matrix

All commands were run from the worktree `C:\workspace\Orchestrator\.claude\worktrees\wf_0f2eb737-354-3` on branch `worktree-wf_0f2eb737-354-3` after the final change (the review-fix commit that adds the archive read-back), with `CARGO_TARGET_DIR=C:\workspace\Orchestrator\target`. Per the slice's build rules, only focused tests were run; the integrator runs the full gate after merging.

One shared-cache fact, verified while rerunning: cargo hashes a workspace member by its path relative to the workspace root (the `path` field of `target/debug/.fingerprint/lazood-*/lib-lazood.json`), so every worktree of this repository that shares that target directory writes the same `liblazood-<hash>.rlib`, and a worktree whose source files are older than another worktree's dep-info reuses the other worktree's artifact as fresh. The first rerun here linked a `lazood` library without `summarize_next_memory_job_once` (compiled from a sibling worktree at `f98ab93`) and failed to compile; a second run linked a sibling's daemon test binary (127 tests filtered out instead of 123). The `mtime` of every workspace crate root (`crates/*/src/lib.rs`, `apps/lazood/src/{lib,main}.rs`, `apps/lazooctl/src/{lib,main}.rs`, `apps/lazood/tests/daemon.rs`) was then bumped so the rows below were rebuilt from this worktree; clippy's output names this worktree's paths. The integrator should expect the same effect in any worktree built while others share the cache.

| Command | Exit | Evidence |
|---|---:|---|
| `cargo fmt --all -- --check` | 0 | No formatting differences. |
| `cargo clippy -p lazood --all-targets --locked -- -D warnings` | 0 | Clean; `Checking lazood v0.1.0 (...\wf_0f2eb737-354-3\apps\lazood)`, 13.97 s. |
| `cargo test -p lazood --test daemon the_core_scenario_runs_end_to_end_on_the_deterministic_provider --locked -- --exact` | 0 | 1 passed, 0 failed, 123 filtered out; 2.57 s. |
| `cargo test -p lazood --test daemon core_scenario_acceptance_contract_maps_every_step_to_evidence --locked -- --exact` | 0 | 1 passed, 0 failed, 123 filtered out. |
| `cargo test -p lazood --lib memory_worker --locked` | 0 | 7 passed, 0 failed, 7 filtered out (worker completion, invalid JSON, deferral, isolation, restart, recall scope, strict parser); 0.82 s. |
| `scripts\acceptance\core-scenario.ps1` | 0 | PASS schema-18 manifest, `deterministic-fake-transport`, 5 commands (four focused cargo tests, each exit 0, and the desktop diff guard), `step12: PASS`, 20.2 s. |
| `git diff --check` | 0 | No whitespace errors. |

Not run in this slice, by the build rules: `cargo test --workspace`, the unfiltered `lazood` daemon test binary, `cargo build --workspace --release`, `scripts/gate.ps1`, every other acceptance script, `provider-adapters.ps1` (live model turn), and `desktop-shell.ps1` (unchanged, separately owned UI).

## Core-scenario acceptance evidence

`artifacts/acceptance/core-scenario-manifest.json` was regenerated by the run above. It records literal `PASS`, schema 18, `deterministic-fake-transport`, the four focused cargo commands plus the desktop diff guard, `step12.status: PASS` with the evidence text quoted above, `step15: PROVEN_ELSEWHERE`, every evidence flag true, and the boundaries `providerSmoke: NOT_RUN`, `externalModelUse: NOT_RUN`, `desktopUi: UNCHANGED`, `schemaMigration: NONE`, `providers: FAKE_TRANSPORT_FOR_EVERY_PROVIDER`, `liveScenario: NOT_RUN_REQUIRES_EXPLICIT_AUTHORIZATION`.

## Boundary evidence

- `git diff --check` passed; both diffs under `apps/lazoo-desktop` are empty (the script's guard).
- Production code gained one `#[doc(hidden)] pub` method and nothing else: no daemon method, CLI command, migration, event, tool, or protocol type changed. The daemon's worker loop still calls only the throttled `run_memory_worker_once`.
- No live model turn was run and no real provider binary was launched; every provider in the story is the fake transport.
- `UI BACKEND CONTRACT: GO` remains intentionally withheld.

## Retained follow-up boundaries

Model summary quality (a live or real-binary summarizer answering `[LAZOO_INTERNAL_MEMORY_SUMMARY]`), the live scenario across real providers, the real-binary turn-transport fixture's lack of a summary branch and of tool calls, and everything else the status document and NEXT-SESSION list remain separate reviewed slices.
