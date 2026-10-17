# Core v1 completion integration verification

Date completed: 2026-10-17 IST

Branch: `feat/core-v1-completion`, stacked on the `codex/core-v1-backend` checkpoint (`610bdbb`, draft pull request #36) and opened as its own pull request against that branch

Product version: `0.1.0`

Database schema: 19 (migration `0019_memory_raw_frames.sql`, from the raw-frame archive slice; every acceptance manifest, store assertion, and daemon contract literal now reads 19)

Scope: integration of six backend slices built in isolated worktrees during the 2026-10-17 session, two integration fixes found by the full gate, the schema 19 literal bump across the later slices, the documentation that records the new state, a final full gate, and a release stress run. No desktop, Tauri, or `desktop.snapshot` change. No live model turn. No user provider configuration changed.

## Outcome

PASS. The final full gate on `eb91ee2` passed all 41 steps in 393,190 ms (format, clippy with warnings denied, workspace tests, locked release build, and 37 acceptance scripts): 543 workspace tests passed, 0 failed, 11 ignored (the explicit live-provider tests) across 85 test binaries, and every acceptance manifest records PASS at schema 19. The release that gate built passed `scripts/test-cli-stress.ps1 -SkipBuild -Invocations 2048 -Concurrency 16`: 2,048 of 2,048 calls exit 0, no failures, an empty `stderrOnSuccess` (so no governor lock-timeout phase occurred), and no timeout diagnostic; the manifest is `artifacts/acceptance/cli-stress-4c902a0fd9aa4ee2b1aefb7ffb1bca4f.json` with daemon SHA-256 `B5029069E6D8146073B1A72E620BA1270A536B9476E8BDC32E35D243C2E17EAD` and CLI SHA-256 `880C931E59B4BA149C13CF0CA39A3CA6C334A31CF7379A100A9BFA199A415CF6`.

Two earlier full gates the same day failed and led to the two integration fixes below. A third failed one real-binary script on its 10-second planned-shutdown timing check while a 10 GB directory deletion saturated the disk; that script passed alone immediately afterwards and in the clean rerun. After this gate the live section 21.3 runs were executed through the owner's installed CLIs and passed for all four providers, including a full Claude Code live lifecycle that found and fixed a cancellation defect; the Claude Code adapter changes that followed were re-gated (see the final gate row in Commands) and are recorded in `2026-10-17-live-lifecycle-gates.md`. Core v1 as defined by PROJECT-BRIEF section 24 is still not accepted in full: OpenCode governed delivery, the live-TUI model, the desktop-inclusive budget, and non-Codex configuration apply remain open, as listed under Remaining boundaries.

## Slices integrated

| Slice | Record | What it adds |
|---|---|---|
| Raw provider-frame retention | `2026-10-17-raw-frame-archive.md` | Schema 19 `source_kind` on the memory archive tables; redacted raw provider frames of every completed turn are captured in the turn observer and archived at turn end as a second archive kind inside the shared quota and cleanup preview. |
| Bounded CLI waits | `2026-10-17-cli-deadlines.md` | `lazooctl` request write 5 s, read-only response 20 s, every other response 600 s, `LZ-IPC` exit 3 naming the phase; the governor initialization lock gives up after 5 s and frees its security descriptor on every path; the stress harness keeps exit-0 stderr. |
| Core scenario step 12 | `2026-10-17-core-scenario-memory.md` | The story's completed task is archived, summarized by a dedicated dormant employee through the real memory worker pipeline on the fake transport, and read back through `memory.search`; step 12 is PASS. |
| Session terminal attach | `2026-10-17-session-terminal-attach.md` | `session.terminal.attach` and `lazooctl session attach <id>` open an owner terminal that resumes a suspended Claude Code or Codex session's native TUI; idle and failed sessions are refused. |
| Adapter cancellation | `2026-10-17-adapter-cancellation.md` | Deterministic proof that `cancel()` ends an active Claude Code or Antigravity turn with one `Cancelled` event, no completion, and a dead host; Codex and ACP coverage pre-existed. |
| Session resume on wake | `2026-10-17-session-resume-on-wake.md` | A suspended Claude Code session at the same configuration revision is resumed on its next activation in its origin directory and audited; Codex starts fresh with `codex-dynamic-tools`; attach, the workspace header, and the wake decision share one directory rule; a rejected resume stops the session so the next wake is fresh. |

Merge order was raw-frame archive, core-scenario memory, CLI deadlines, terminal attach (all from base `f98ab93`), then adapter cancellation (from `6b58c4e`) and session resume (from `c2eb9e3`, after its independent review and fix pass). Every merge was conflict-free; the three files touched by more than one slice (`apps/lazood/tests/daemon.rs`, `apps/lazooctl/src/lib.rs`, `scripts/acceptance/core-scenario.ps1`) merged automatically.

## Integration decisions

| Decision | Reason |
|---|---|
| Bump the later slices' `schemaVersion = 18` literals to 19 in the integrator commit (`6b58c4e`) rather than in each slice | The slices were built in parallel from a schema 18 base and said so in their records; one commit after the merge keeps the bump in one place. The contract tests pin the literals, so a missed one fails the gate. |
| Widen the Claude Code adapter's pre-prompt startup drain from 1 ms to 150 ms for a session's first prompt only (`c2eb9e3`) | The first full gate failed `claude_refuses_queued_startup_frames_before_writing_a_prompt`; three reruns of the test binary gave one pass, one single failure, and one run with five startup-frame tests failing while a parallel agent was building. The 1 ms readiness check missed `SessionStart` hook frames that trail `init` under load, in production as in the fixture; a trailing frame would then be attributed to the first turn. Later prompts keep the 1 ms check. Five reruns under the same load passed 17 of 17, and the adapter-cancellation slice's 19-test binary passed three times at default parallelism after the fix. |
| Publish the outbox after a raw-frame archive and after an archiver sweep that archived anything (`0c29478`) | The second gate failed `conversation-delivery.ps1` deterministically (`outbox drained expected '0' but found '1'`, two of two reruns). The turn observer publishes after each event, so `turn.completed` was published before the new raw-frame archive wrote `memory.archived` on the `turn.submit` path, and nothing published that row until the next unrelated write. The fix publishes at the write site; `completed_turn_archives_redacted_raw_provider_frames` now asserts `pendingOutbox == 0` after the turn and fails without the fix. The sixty-second archiver sweep had the same latent gap and gets the same publish. |
| Review the adapter-cancellation diff in-line and the resume slice through an independent reviewer | The cancellation slice changed tests and fixtures only. The resume slice changed the daemon's single session start point, so a Sonnet reviewer read it against twelve invariants and found one high (attach and the workspace header assumed `sessions/<id>` for a resumed session; attaching an older session that shares a native id with a live one), two medium (resume-chain orphaning when an intermediate session took no turn; an unrecoverable dead host at idle timeout 0), and three lower findings; all were fixed with four new tests before the merge. |
| Subagents ran on Sonnet | The owner asked for efficient delegation and corrected an Opus/Fable plan to Sonnet mid-session. The two read-only mapping and vendor-research agents had already started on Opus and were allowed to finish; the implementers, the reviewer, and the fix pass ran on Sonnet at high effort in isolated worktrees with their own `CARGO_TARGET_DIR`. |
| Remove the four prior-session worktrees after merging and the two new ones after their slices merged | The merged branches remain; the worktrees only held checkouts and, for the new ones, build caches of several gigabytes on a disk with 33 GB free. |

## Vendor facts established

Checked against the installed binaries and current documentation on 2026-10-17, without any model call: codex-cli 0.153.4 (`codex.cmd`) and 0.162.0-alpha.17.2 (the LocalAppData build Lazoo discovers) generate a `ThreadResumeParams` schema with `approvalPolicy`, `cwd`, `developerInstructions`, `sandbox`, and other fields but no `dynamicTools`; a live `thread/resume` against a disposable `CODEX_HOME` drops unknown fields silently and answers `no rollout found`, so Lazoo's fail-closed check stands. Claude Code 2.1.302 accepts `-p --resume <id> --input-format stream-json --output-format stream-json`, persists print-mode sessions unless `--no-session-persistence` (not passed by Lazoo), and offers `--fork-session`. OpenCode 1.19.0's ACP `initialize` advertises `loadSession: true` and `session/load` exists. Antigravity 1.3.6 resumes with `--conversation <id>` and documents no fork.

## Commands

| Command | Exit | Result |
|---|---|---|
| `pwsh -File scripts/gate.ps1 -OutputPath artifacts/acceptance/core-v1-gate-manifest.json` on `6b58c4e` (first run) | 1 | FAIL at `test`: `claude_refuses_queued_startup_frames_before_writing_a_prompt` (one of 17 Claude adapter tests); format and clippy passed. Led to `c2eb9e3`. |
| The same gate on `c2eb9e3` (second run) | 1 | Format, clippy, the workspace test step, and the release build passed; FAIL at `conversation-delivery.ps1` with `outbox drained expected '0' but found '1'`, reproduced two of two times alone. Led to `0c29478`. |
| The same gate on `eb91ee2` (third run) | 1 | 40 of 41 steps passed; `conversation-delivery.ps1` failed `planned-shutdown native tree process ... remained alive after 10 seconds` while a concurrent 10 GB worktree deletion ran; the script passed alone right after. |
| The same gate on `eb91ee2` (final run, machine otherwise idle) | 0 | PASS, 41 steps, 393,190 ms; 543 workspace tests passed, 0 failed, 11 ignored across 85 binaries; 37 acceptance manifests PASS at schema 19. Transcript `artifacts/core-v1-gate.log`, manifest `artifacts/acceptance/core-v1-gate-manifest.json`. |
| `pwsh -File scripts/test-cli-stress.ps1 -SkipBuild -Invocations 2048 -Concurrency 16` on that release | 0 | PASS: `exitCodes` `[{code 0, count 2048}]`, 0 failures, `stderrOnSuccess` empty, `timeoutDiagnostic` null. || The same gate after the Claude Code adapter changes that followed the live runs (observe `Cancelled` before returning it; name the frame in the pre-init refusal) | 0 | PASS, 41 steps, 483,454 ms; 543 workspace tests passed, 0 failed, 11 ignored across 85 binaries; 37 acceptance manifests PASS at schema 19. |
| `cargo test -p lazoo-providers --test claude --locked`, five runs under parallel build load after `c2eb9e3` | 0 | 17 passed each time (before the fix, three runs gave 17, 16, and 12 of 17). The 19-test binary after the cancellation merge passed three runs at default parallelism. |
| `cargo test -p lazood --test daemon completed_turn_archives_redacted_raw_provider_frames --locked -- --exact` with `0c29478`'s production change reverted, then applied | 101, then 0 | FAILED at the new `pendingOutbox == 0` assertion, then ok. |
| `pwsh -File scripts/acceptance/conversation-delivery.ps1 -SkipBuild -Provider codex` after `0c29478` and again after the third gate | 0 | PASS both times. |

## Boundary evidence

- Every acceptance script's desktop guard (`git diff --quiet -- apps/lazoo-desktop` and the cached form) passed; no Tauri command, capability, `desktop.snapshot` field, or frontend source changed.
- `provider-adapters.ps1` and `desktop-shell.ps1` stayed excluded from the gate; no live model turn ran and no real provider binary was launched by the gate.
- The vendor probes used disposable `CODEX_HOME`, `CLAUDE_CONFIG_DIR`, and XDG directories under the session scratchpad; no user provider configuration was read for writing or changed.
- `UI BACKEND CONTRACT: GO` remains intentionally withheld.

## Remaining boundaries

The remaining product gates are listed in `docs/NEXT-SESSION.md` and in the status record `2026-10-15-core-v1-backend-status.md` (section "Remaining core v1 acceptance boundaries"): the live section 21.3 runs passed on 2026-10-17 (`2026-10-17-live-lifecycle-gates.md`), leaving cancellation on Codex, OpenCode, and Antigravity fixture-proven only; OpenCode governed delivery and the live-TUI model are owner decisions; Codex resume waits on a vendor way to carry dynamic tools across `thread/resume`; the notice-turn session as a resume candidate is a policy to confirm; the desktop-inclusive budget and non-Codex configuration apply stay open; the historical CLI timeout and heap-corruption exit have bounds, not causes.
