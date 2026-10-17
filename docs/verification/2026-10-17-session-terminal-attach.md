# Session terminal attach verification

Date completed: 2026-10-17 IST

Branch: `worktree-wf_0f2eb737-354-4` (slice worktree; the integrator merges it into the main checkout)

Base: `f98ab93` (`codex/core-v1-backend` tip, schema 18)

Product version: `0.1.0`

Database schema: 18 (no migration; the acceptance manifest's `schemaVersion = 18` literal is bumped by the integrator if the final schema differs)

Scope: `SessionTerminalAttachParams`/`SessionTerminalAttachResult`, `lazoo_providers::attach_args`, `Store::task_workspace_for_session`, `Store::record_session_terminal_attached`, the daemon's `session.terminal.attach` and its `LZ-SESSION-ATTACH-UNSUPPORTED` / `LZ-SESSION-ATTACHED` / `LZ-SESSION-ATTACH-FAILED` codes, `lazooctl session attach`, the real-binary acceptance script, and this record. The desktop app, Tauri surface, `desktop.snapshot`, collaboration tools, delivery, dispatch, and every provider adapter are unchanged.

## Outcome

PASS for the backend increment; the live native resume is NOT proven (see the owner gate below).

PROJECT-BRIEF section 27 wants the owner to take over an employee's native harness. The first step that is safe on the current architecture is a takeover of a *suspended* session: `session.terminal.attach { id, expectedRevision, columns, rows, idempotencyKey }` reads the session row, requires the cited revision and the `suspended` state, refuses memory-summary sessions, resolves the resume argv from the provider and the recorded `nativeSessionId` (`claude --resume <id>`, `codex resume <id>`), resolves the executable through the daemon's provider discovery, resolves the working directory from Lazoo's own records (the task dispatch's retained `workspacePath` for a worker session, otherwise `<dataRoot>\sessions\<sessionId>`), then opens an ordinary owner terminal through the existing `terminal.open` path (same ConPTY, kill-on-close job, 8-terminal limit, idempotency, byte I/O pipe). The result is `{ sessionId, terminal: TerminalRecord }`; `terminal read/write/resize/stop` work on it unchanged. One live terminal per session: a second attach is `LZ-SESSION-ATTACHED` with `data.terminalId`; the slot frees itself when the terminal exits or is stopped. Each attach writes one `session.terminal.attached` audit event with `sessionId` and `terminalId`; the session row itself is untouched (state, revision, native id). `lazooctl session attach <id> --expected-revision N [--columns --rows --idempotency-key]` maps to it; `LZ-SESSION-*` already exits 10 and `LZ-PROVIDER-NOT-DETECTED` exits 8, so no exit-code mapping changed.

No live model turn was run and no real Claude or Codex binary was launched.

## Design notes

| Decision | Reason |
|---|---|
| Only `suspended` sessions attach; `idle` is `LZ-SESSION-STATE` | Verified in the adapters: every provider keeps one long-lived protocol host for the whole time a session is idle (Claude `--print --input-format stream-json`, Codex `app-server`, OpenCode `acp`, Antigravity stream-json). A second interactive process on the same native id would put two writers on one transcript. Lazoo never resumes a suspended native session itself (delivery targets only `idle` sessions of this boot; dispatch always starts a fresh session and stops it), so no delivery or dispatch guard was needed. |
| Claude Code and Codex only; OpenCode, Grok Build, Antigravity, empty ids and `pending:` ids are `LZ-SESSION-ATTACH-UNSUPPORTED` | OpenCode and Grok never receive governed turns; Antigravity's session row keeps the `pending:<uuid>` placeholder (the real id lives only on turn records). Refusing by name beats guessing an argv. |
| No authority flags are passed to the TUI | The owner is at the keyboard, so the TUI runs under the owner's own native configuration. Owner decision; recorded as a boundary because a permissive user config can exceed the employee's ceiling. |
| Reuse `terminal_open`, `validate_open`, `release_exited`; add only an in-memory `attachments` map | The attach is an ordinary terminal with a resolved command. Exited or stopped terminals already leave `sessions`, so the per-session slot clears with no extra hook. |
| Replay of the same key returns the same terminal | The derived key `session-attach:<sessionId>:<key>` namespaces attaches from ad-hoc `terminal.open` keys and gives the CLI's replay the terminal it already opened instead of `LZ-SESSION-ATTACHED`. |
| Audit event through the existing `insert_event` helper, no schema change | Mirrors `memory.archived`: five lines, durable, visible in `events.list`. The in-memory terminal binding still dies with the daemon. |
| New activations of the employee are allowed while attached | Data safety does not need the refusal: a new session is a fresh native session in a different directory. Owner decision. |
| `cwd` for a worker session is the retained task worktree | It is the directory the native session's history refers to. Owner edits there touch a worktree that may be under review; recorded as a boundary. |

## Fresh verification matrix

All commands were run from the slice worktree after the final change, with `CARGO_TARGET_DIR=C:\workspace\Orchestrator\target`. Three sibling slice worktrees share that cache and cargo hashes path crates relative to the workspace root, so a sibling's build of `lazoo-providers`/`lazoo-store` overwrote the shared rlibs once mid-run and made `lazood` fail to compile against them (`no attach_args in the root`). Every command below was therefore run after touching this worktree's sources, and the daemon test executable and release binaries were copied out of the shared directory before use. The integrator's full gate on the merged checkout is the authoritative run.

| Command | Exit | Evidence |
|---|---:|---|
| `cargo fmt --all -- --check` | 0 | No formatting differences. |
| `cargo clippy -p lazoo-protocol -p lazoo-providers -p lazoo-store -p lazood -p lazooctl --all-targets --locked -- -D warnings` | 0 | Clean after one `doc_markdown` fix in the `attach_args` doc comment. |
| `cargo test -p lazoo-protocol --locked` | 0 | 31 passed, 0 failed; new `tests/terminal.rs` (1 test: strict camelCase params, no authority field, result wraps a plain `TerminalRecord`). |
| `cargo test -p lazoo-providers --locked` | 0 | 76 passed, 0 failed, 11 ignored (explicit live-provider tests); new `attach_args_name_only_the_documented_resume_shapes_and_refuse_placeholders`. |
| `cargo test -p lazooctl --locked` | 0 | 33 passed, 0 failed; new `session_cli_maps_attach_with_default_terminal_size_and_session_exit_codes`. |
| `cargo test -p lazood --test daemon --no-run --locked`, then the built executable with `<name> --exact` for each of the six names below | 0 | 6 passed, 0 failed: `session_terminal_attach_resumes_a_suspended_codex_session_in_an_owner_terminal`, `session_terminal_attach_refuses_idle_sessions_stale_revisions_and_unknown_ids`, `session_terminal_attach_fails_closed_without_an_executable_or_a_resume_command`, `session_terminal_attach_leaves_the_employee_free_for_new_sessions`, `session_terminal_attach_acceptance_contract_is_real_binary_and_backend_only`, and the extended `accepted_projectless_child_dispatches_automatically_through_the_canonical_message` (worker-session cwd equals `dispatch.workspacePath`). |
| `cargo build -p lazood -p lazooctl -p lazoo-turn-transport-fixture --release --locked` | 0 | Release binaries copied into the worktree's `target\release` beside the unchanged `lazoo-session-host.exe` (source last changed 2026-10-13, copied from the shared release directory). |
| `scripts\acceptance\session-terminal-attach.ps1 -SkipBuild` | 0 | PASS manifest, `real-binaries-disposable-data-root-no-provider`, schema 18, 17 commands, every evidence flag true (below). |
| `git diff --check` | 0 | No whitespace errors. |

Not run: the whole `lazood` daemon test binary without a filter, `cargo test --workspace`, `scripts/gate.ps1`, the other acceptance scripts, `provider-adapters.ps1` (live model turn), `desktop-shell.ps1` (unchanged, separately owned UI), and `npm --prefix apps\lazoo-desktop\ui run check` (frontend unchanged). The integrator runs the full gate after merging.

## Session-terminal-attach acceptance evidence

`artifacts/acceptance/session-terminal-attach-manifest.json` records literal `PASS`, schema 18, and `real-binaries-disposable-data-root-no-provider`. The script runs the real `lazood.exe` and `lazooctl.exe` on a disposable `LAZOO_DATA_ROOT` with `lazoo-turn-transport-fixture.exe` copied onto the daemon's `PATH` as `codex.exe` (no real provider, no model call) and proves:

- `session warm` gives an idle session whose `nativeSessionId` is the fixture thread id; `session attach` on it exits 10 with `LZ-SESSION-STATE` (`idleAttachRefused`);
- after `session stop`, `session attach` returns `terminal.executable` ending in `\fixture-bin\codex.exe`, `args == ['resume', '<thread id>']`, `cwd == <dataRoot>\sessions\<sessionId>`, `live == true` (`resumeArgumentsFromRecords`, `cwdFromRecords`, `attachedTerminalLive`), and the fixture log gains one more `server-start` entry, so the attached process is the discovered executable (`attachedProcessIsTheDiscoveredExecutable`);
- a second attach exits 10 with `LZ-SESSION-ATTACHED` naming the live terminal id (`oneLiveTerminalPerSession`);
- `terminal write` of a JSON-RPC `initialize` line and `terminal read` round-trip bytes: the fixture's `lazoo-turn-transport-fixture` user agent appears in the ConPTY output (`terminalBytesRoundTrip`);
- `terminal stop` returns `live == false`, a re-attach opens a different live terminal, and it is stopped again (`reattachAfterStop`);
- `session show` still reports `suspended` at the same revision with the native id retained (`sessionRecordUnchanged`);
- attaching an unknown session id exits 10 with `LZ-SESSION-NOT-FOUND` (`unknownSessionRefused`);
- `events list` carries exactly two `session.terminal.attached` events whose payloads name the session and each terminal (`auditEventRecorded`);
- the daemon stops cleanly on `system shutdown`, and both diffs under `apps/lazoo-desktop` are empty (`uiChangesAbsent`).

The daemon tests cover the same assertions in-process with `cmd.exe` injected as the Codex known location (cmd.exe ignores `resume native-1` and stays interactive), plus: replay of the same idempotency key returns the same terminal; a stale revision is `LZ-SESSION-STALE` with `currentRevision`; `expectedRevision: 0`, a blank key, and an unknown field are `LZ-SESSION-INVALID` / `LZ-INVALID-PARAMS`; no executable is `LZ-PROVIDER-NOT-DETECTED`; `opencode` and `antigravity` employees are `LZ-SESSION-ATTACH-UNSUPPORTED` with `data.sessionId`; no terminal is opened on any refusal; a worker session dispatched for a task attaches in `dispatch.workspacePath` with that session's native id; and `session.warm` for the same employee still starts a second session while the first is attached.

## Boundary evidence

- `git diff --check` passed; both diffs under `apps/lazoo-desktop` are empty.
- No Tauri command, capability, `desktop.snapshot` field, frontend source, migration, collaboration tool, delivery rule, dispatch rule, or provider adapter changed. The daemon gains `session.terminal.attach`; `lazooctl session` gains `attach`; `TerminalRecord`, `terminal.list` and `terminal.show` are byte-identical.
- The resume argv is taken from the installed CLIs' help (Claude Code 2.1.302 `--resume <id>`, codex-cli 0.153.4 `resume <id>`) and is NOT proven against a live resume. Unproven: that Claude `--resume` opens a session created in print mode; that `codex resume` accepts an app-server thread started with experimental `dynamicTools`; that the Codex build Lazoo discovers (LocalAppData alpha vs the npm build on `PATH`) has the same `resume` syntax.
- The TUI runs under the owner's native configuration, not the employee's authority mode. A resumed Codex thread declared Lazoo's collaboration tools, which the interactive TUI has no handler for. Turns typed in the TUI never enter Lazoo's turn ledger or memory archive.
- The attach binding is in memory and dies with the daemon; the audit event is the only durable trace. Attaches share the eight-terminal limit and 1 MiB output ring with ad-hoc terminals.
- `failed` sessions (including prior-boot `LZ-SESSION-OWNER-LOST`) are refused with `LZ-SESSION-STATE` in this slice; only `suspended` is accepted.
- `UI BACKEND CONTRACT: GO` remains intentionally withheld.

## Retained follow-up boundaries

1. Live gate, owner-authorized, separate from the deterministic gate: with the real Claude Code and the real Codex build Lazoo discovers, run one short smoke turn each through Lazoo, `session stop`, `session attach`, and confirm from the terminal output that the native TUI shows the same conversation history without sending a prompt. Until then `attach_args` documents shapes, not proven resumes.
2. Section-27 finding: the current protocol-host architecture cannot show a live native TUI while Lazoo drives turns, because every adapter owns the one long-lived native process for the session and drives it over a machine protocol. The owner must choose between suspend-and-resume takeover (this slice), a fork of the live session (`claude --resume <id> --fork-session`, `codex fork`, `opencode --session <id> --fork`), or a rendered transcript view in the desktop. Embedded native terminals must not be claimed until that choice is made and its gate passes.
3. Attaching `failed`/owner-lost sessions, a guarded stop-then-attach for idle sessions, Antigravity (real id from turn records) and OpenCode (`--session`) attach, refusing employee activations while attached, and the desktop binding each remain separate reviewed slices.
