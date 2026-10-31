# Backend handoff: four-provider UI starting point, 2026-10-29

## Desktop UI pilot, 2026-10-31

The desktop is no longer the read-only shell. `feat/desktop-ui-pilot` (cut from `origin/main` at `87b63e8`) rebuilds `apps/lazoo-desktop` on the readiness contract: `daemon_call` behind a Rust allowlist, an events channel, a terminal byte-stream poller, and a React front end with a launcher, a conversation surface (agent tabs, Native and Transcript panes, attention strip, composer, details), the Inbox, Settings, and a command palette. Design: `docs/superpowers/specs/2026-10-31-desktop-ui-pilot-design.md`. Evidence and the live pilot on the owner's machine: `docs/verification/2026-10-31-desktop-ui-pilot.md`.

Run it from a build: `npm --prefix apps/lazoo-desktop/ui ci`, `npm --prefix apps/lazoo-desktop/ui run build`, `cargo build -p lazoo-desktop -p lazood -p lazoo-keeper -p lazooctl -p lazoo-session-host`, start `lazoo-keeper.exe`, then `lazoo-desktop.exe` with the same `LAZOO_DATA_ROOT`. `scripts/acceptance/desktop-shell.ps1` accepts `-Configuration debug`.

Backend fix carried in this branch (`crates/lazoo-providers/src/codex_view.rs`): the view bridge's 32 KiB WebSocket write cap dropped viewers on replies such as `skills/list`, and a forwarded `plugin/list` coincided with the controller app-server's stdout closing; the cap is now four frames and `plugin/list` is refused like the other sidebar reads. With that, the genuine Codex TUI resumes through the view and streams live turns on the owner's machine (record section "Native view bootstrap"). Terminal open and close emit no audit event, so the desktop polls status every 5 s while a pane has no stream. Proven live on 2026-10-31: a Codex lead delegating to a Codex worker through `task_delegate`, the proposal accepted from the attention strip, the worker tab appearing and completing, the lead's native view streaming, transcripts for both. Also found: Claude Code 2.1.314 employee sessions fail with `provider timed out during native interactive startup` (the ConPTY trust-dialog startup in `interactive_session.rs`) while the print-mode probe and smoke pass, so the interactive startup detector needs checking against this build; and the trust event family is spelled `native-trust.*`. The full gate on this branch's final tree passes all 44 steps in one uninterrupted run (`artifacts/acceptance/core-v1-gate-manifest.json`); an earlier run had `resource-budget` hang on a `lazooctl message fail` call, recorded as a CLI finding. Also carried: `session.terminal.attach` appends `--no-daemon` to `codex resume` when the installed CLI advertises it, and `task-delegation-graph.ps1` checks the desktop the way every other slice does (working tree) plus an allowlist assertion instead of a committed diff against `main`. `Alt+Left/Right` traverse Lazoo's own route history (never a focused terminal's).

## Current checkpoint

The requested Codex-level parity is implemented for OpenCode, Claude Code and
Antigravity, including their genuine native TUIs during orchestration. All four
providers passed release-daemon acceptance. Start the UI from this worktree and
branch, `codex/backend-ui-readiness`; the original checkout remains separate.

Read [the provider parity handoff](UI-PROVIDER-PARITY-HANDOFF.md) and
[the UI integration guide](UI-BACKEND-HANDOFF.md). The current evidence index is
`artifacts/provider-parity/provider-parity-final-manifest.json`. It records the
full test/acceptance coverage, targeted corrections, native release results and
CLI stress without hiding failed intermediate attempts.

Workspace trust requires confirmation and notification by default. The optional
"Automatically accept next time" policy applies only to future Lazoo-managed
workspaces for the selected provider. Grok was not live-tested and stays excluded.
The owner explicitly capped scope at Codex parity: connectors, vector search,
voice, wider configuration federation and full-product performance remain later
work. No frontend implementation is included.

## Historical Codex-only checkpoint (superseded by the current section)

The following dated reference explains the original baseline. Its restrictions
on other providers are superseded by the parity handoff above.

The owner asked for a backend foundation on which UI development can comfortably
begin and explicitly required the actual native TUI to remain visible during
orchestrated work. That starting point is verified for Codex. Full core v1 and
concurrent native views for other providers remain unfinished.

Work is on `codex/backend-ui-readiness` in
`C:\Users\anubhab\.codex\worktrees\backend-ui-readiness\Orchestrator`, based on
`feat/core-v1-completion` at `9c7fa9d` (including `origin/main` at `dfa35f1` when
fetched at the start). Production source is checkpointed at
`feab4d556b3424d3b9b0b7ec9e155f62434b8a95`; later changes only add evidence/docs.
The original checkout remains separate. No PR was created or branch pushed here.

Start with [the UI integration guide](UI-BACKEND-HANDOFF.md),
[the terminal client contract](superpowers/specs/2026-10-28-terminal-client-contract.md)
and [the final verification](verification/2026-10-29-ui-backend-readiness.md).
Both readiness plans are complete. No frontend changes were made.

Implemented and independently reviewed:

- Launcher authority choices obey the employee ceiling and daemon provider policy.
  Collaboration tools and concurrent native-view support are explicit capabilities.
- Permanent unsupported-authority refusals atomically settle the user turn, message
  and root task, preserving idempotency, revision fences and temporary recovery.
- Exclusive backward history paging retains current session/worker state. Durable
  event replay crosses 1,024 rows and recovers notification lag without skipping.
- Genuine Codex TUI runs in ConPTY against the same controller-owned app-server
  thread. Its authenticated restricted view cannot submit native turns/config
  changes or answer controller requests. Lazoo owns tools and approvals.
- `session.terminal.view` and status fields provide revision/idempotency fences,
  reconnect and cleanup. Closing a view preserves a pending turn and approval;
  stopping the session closes its view. Suspended native attach stays separate.
- The installed Codex `0.165.0-alpha.1.2` passed an actual native MCP parser review
  and configuration preview/apply/replay/rollback. Only exact reviewed versions may
  write configuration; adjacent unreviewed versions remain refused.

Final verification on that source:

- Full `scripts/gate.ps1`: **44/44 steps passed**, including 39 acceptance scripts,
  strict clippy, format, release build and **607 workspace tests passed** (14
  intentionally ignored). Count excludes nested filtered test invocations.
- Release production native acceptance: **13/13 checks passed**, actual installed
  Codex `0.165.0-alpha.1.2`, four local model-fixture requests, zero external model
  requests. It proves native rendering, collaboration routing, approval decline,
  history reconnect and close/reopen while an approval remains pending.
- CLI stress: **2,048/2,048 passed at concurrency 16**, no stderr on success or
  daemon stderr. Its daemon/CLI hashes match the release native run.
- Evidence: `artifacts/readiness/ui-backend-final-manifest.json`. Fresh manifests
  and native screen/turn evidence are preserved under `artifacts/readiness`;
  historical tracked acceptance manifests were restored after hash-verified copies.

The Codex TUI uses its built-in “Viewing sub-agent — direct input is disabled”
label even for the lead. Prompts and approval decisions belong in Lazoo controls.
The first UI must respect the capability table; other providers cannot currently
be offered equivalent orchestration/native-view behavior. Remaining work includes
non-Codex collaboration/views, broader governed authority, further configuration
formats/plugins/skills, and unmeasured whole-product performance windows.
Connectors, vectors and voice remain later phases. Fresh verification used local
fixtures/disposable configuration; external account/model availability was not
retested and personal provider configuration was not changed.

Use `CARGO_BUILD_JOBS=1`, `CARGO_PROFILE_DEV_DEBUG=0`,
`CARGO_PROFILE_TEST_DEBUG=0`, `CARGO_INCREMENTAL=0`, and `RUST_TEST_THREADS=2` for
this host/cache. Keep Cargo jobs serialized. The next product work can implement
the desktop bindings and UI using the guide; no unresolved prerequisite remains
for that Codex UI starting point.

## Historical checkpoints (superseded where the current section differs)

The remainder records earlier work and dated limitations. In particular, the old
pending native-TUI decision and uninterrupted-gate requirement below are resolved
for the current Codex checkpoint above; they are retained as history.

## Owner checkpoint

On 2026-10-17 the owner asked for a larger leap toward finishing the core v1 backend. The work landed on `feat/core-v1-completion`, stacked on the `codex/core-v1-backend` checkpoint (draft pull request #36) and opened as its own pull request. `docs/verification/2026-10-17-core-v1-completion.md` records the integration, the final gate, and the stress run; it is authoritative over the per-slice records it indexes. The 2026-10-16 provider scope stands: Codex, OpenCode, Antigravity, and Claude Code (native subscription) are the required providers, Grok Build is excluded, and UI remains separately owned. Do not emit `UI BACKEND CONTRACT: GO` from this backend work.

## Current implementation

Schema 19 adds `source_kind` to the memory archive tables (migration `0019_memory_raw_frames.sql`), so the redacted raw provider frames of every completed turn are retained as a second archive kind inside the same quota and cleanup preview. Schema 18 and earlier are unchanged.

Landed on 2026-10-17, each with its own record under `docs/verification/2026-10-17-*.md`:

- Raw provider-frame retention (`raw-frame-archive`): per-turn capture in the turn observer, archived when the turn completes, redacted, quota-bounded; wire frames that map to no event are not captured; `memory.archived` is published with the turn.
- Bounded CLI waits (`cli-deadlines`): 5 s request write, 20 s read-only response, 600 s every other response, `LZ-IPC` exit 3 naming the phase; the governor initialization lock gives up after 5 s and the call runs un-enrolled; the stress harness keeps exit-0 stderr so that phase is recorded inside a PASS.
- Core scenario step 12 (`core-scenario-memory`): the story's completed task is archived, summarized by a dedicated dormant employee through the real worker pipeline on the fake transport, and read back through `memory.search`.
- Session terminal attach (`session-terminal-attach`): `session.terminal.attach` / `lazooctl session attach <id>` opens an owner terminal that resumes a suspended Claude Code or Codex session's native TUI; idle and failed sessions are refused.
- Adapter cancellation (`adapter-cancellation`): deterministic proof that `cancel()` ends an active Claude Code or Antigravity turn with one `Cancelled` event and a dead host, beside the existing Codex and ACP coverage.
- Session resume on wake (`session-resume-on-wake`): a suspended Claude Code session at the same configuration revision is resumed on its next user turn, notice turn, or `session.warm` in its prior working directory and audited as `session.resumed`; Codex starts fresh and audits `session.resume.skipped` with `codex-dynamic-tools`, because `thread/resume` carries no `dynamicTools` field in codex-cli 0.153.4 or 0.162.0-alpha.17.2; task dispatch, memory-summary sessions, attached sessions, and sessions without a completed turn never resume.

Two integration fixes were needed and are recorded in the completion record: the Claude Code adapter waits 150 ms (first prompt only) for trailing `SessionStart` hook frames, and the raw-frame archive and archiver sweep publish the outbox after writing `memory.archived`.

## Verification and continuation

The canonical gate is unchanged: `pwsh -File scripts/gate.ps1 -OutputPath artifacts/acceptance/core-v1-gate-manifest.json` with transcript `artifacts/core-v1-gate.log` (format, clippy with warnings denied, workspace tests, locked release build, every deterministic and real-binary acceptance script; live adapters and the desktop excluded). The last full gate run on the final tree was stopped by the host for memory pressure after the global-scan merge (`289b492`); its format and clippy steps had passed, and the remaining content was then run as foreground steps on the same tree: 566 workspace tests passed and all 39 acceptance scripts passed (`core-v1-gate-acceptance-final-manifest.json`), so the branch is fully verified; a single uninterrupted `scripts/gate.ps1` run is still the tidiest artifact for the merge record. Run `scripts/test-cli-stress.ps1 -SkipBuild -Invocations 2048 -Concurrency 16` on the release afterwards; a PASS with a non-empty `stderrOnSuccess` is a reproduction of the governor lock phase, not a clean run.

Vendor facts checked on 2026-10-17 (installed binaries and current documentation): Codex `thread/resume` accepts `developerInstructions`, `sandbox`, `approvalPolicy`, and `cwd` but has no `dynamicTools`, and unknown fields are dropped silently, so Lazoo's fail-closed check is correct; Claude Code 2.1.302 accepts `-p --resume <id> --input-format stream-json --output-format stream-json`, persists print-mode sessions unless `--no-session-persistence` (which Lazoo does not pass), and offers `--fork-session`; OpenCode 1.19.0's ACP server advertises `loadSession` and `session/load` exists; Antigravity 1.3.6 resumes with `--conversation <id>` and documents no fork. Whether Codex restores dynamic tools from a thread's rollout on resume is unknown.

## Remaining product gates

1. The live section 21.3 runs were executed on 2026-10-17 (`docs/verification/2026-10-17-live-lifecycle-gates.md`): all four providers passed readiness, a model-backed turn, resume where supported, and the daemon smoke, and Claude Code passed `scripts/acceptance/provider-live-lifecycle.ps1 -Provider claude-code` (messaging, live cancellation, resume on wake, planned shutdown) on the native subscription, after a real defect (the adapter returned a `Cancelled` event without observing it, so live cancels ended `uncertain`) was fixed. Later the same day `provider-live-lifecycle.ps1 -Provider codex` passed (messaging, native-interrupt cancellation, stop, fresh wake with the recorded `codex-dynamic-tools` reason, planned shutdown) and the ignored adapter tests proved live cancellation on OpenCode (mid-stream) and Antigravity (prompt kill before output). Still unproven live: the rendered history of an attached native TUI, and permissions under `ask` (refused for Claude Code by design). Rerun with `provider-adapters.ps1 -Provider <provider> -SkipBuild`, the ignored `live_*` tests, and `provider-live-lifecycle.ps1 -Provider claude-code`; each sends a few short model turns.
2. OpenCode now receives turns and tasks under `full-autonomy` only (launched with `OPENCODE_PERMISSION={"*":"allow"}`, `docs/verification/2026-10-17-opencode-full-autonomy.md`), the same shape as Claude Code, and a completed `provider.smoke` now records the provider `verified` so such an employee can become dormant and lead (`docs/verification/2026-10-17-smoke-verifies-provider.md`). With that and the ACP stop-reason mapping, `provider-live-lifecycle.ps1 -Provider opencode` passed live (messaging, cancellation, stop, fresh wake, planned shutdown). Governed delivery under `ask` and `self-decision` landed the same day (`docs/verification/2026-10-17-opencode-governed-delivery.md`): a forced `{"*":"ask"}` block with read-only native allows, command execution and file edit/delete/move bridged into the approval ledger, network, connector, and unknown kinds declined one-shot, and, because a live probe showed the block does not beat repository-scoped agent permissions, a governed session refuses to start when its working-directory chain carries an OpenCode config or `.opencode` directory (a global agent-scoped allow bypasses it too, a global top-level `permission` does not, so the scan also refuses a governed start when the owner's global OpenCode config carries agent-scoped keys or agent markdown). Proven live the same day (`scripts/acceptance/opencode-governed-live.ps1`): a shell command under `ask` ran only after an Inbox approval, a decline left it unexecuted, `self-decision` waited for the user. What remains yours: whether a third approval kind for network and connector requests may enter the desktop Inbox contract, and the `self-decision` policy for ACP requests (the bridge marks every one a scope expansion, so today they wait for you). Claude Code `ask` and `self-decision` stay refused by design (only the managed-settings tier can make the harness the sole permission source).
3. Live native TUI while Lazoo drives turns needs the section 27 choice between suspend-then-attach (implemented), a fork of the live session (`claude --resume <id> --fork-session`, `codex fork`, `opencode --session <id> --fork`), and a rendered transcript view; desktop binding stays with the UI owner.
4. Codex resume waits on a vendor way to carry dynamic tools across `thread/resume`; OpenCode and Antigravity resume wait on governed delivery for those providers.
5. A notice-turn session is a valid resume candidate, so the next user turn shares the notice's context with the lead; confirm or change that policy.
6. The desktop-inclusive budget was measured on 2026-10-17 (`scripts/acceptance/desktop-budget.ps1`, excluded from the gate): CPU idle 0.074 percent and routing 0.274 percent PASS; idle memory 127 MiB and routing memory 128 MiB by private working set (the Task Manager basis) PASS against 350 and 750 MiB, with the summed working set recorded as the upper bound (417 MiB idle, shared WebView2 pages counted repeatedly; daemon about 13 MiB). Failure-storm, indexing, terminal-streaming, and voice windows are unmeasured. Beyond that, configuration apply beyond the verified Codex formats, and a causal explanation of the historical CLI timeout and heap-corruption exit remain open; the new bounds record a recurrence with a phase instead of killing it.

Connectors, vector retrieval, and voice remain outside this request. The background summary worker remains unconfigured by default, no user provider configuration is changed, and no live inference is triggered by the deterministic gate.

## Durable rules

Preserve record-first evidence, revision and attempt fencing, native acceptance before execution claims, project/Home retrieval scope, and one-action-only approvals. Do not automatically resend uncertain model work, widen provider authority, overwrite configuration drift, delete retained task worktrees, resume a session across a configuration change, or treat a terminal byte stream as the control channel. Archive cleanup remains an explicit preview/apply action. Future changes to UI contracts or paid-model endpoints require the corresponding owner intent.
