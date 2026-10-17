# Session resume on wake verification

Date completed: 2026-10-17 IST

Branch: `feat/session-resume-on-wake` (a dedicated worktree branch; the integrator merges it into `feat/core-v1-completion`)

Base: `c2eb9e3` on `feat/core-v1-completion`

Product version: `0.1.0`

Database schema: 19 (this slice adds no migration and no column; it adds two audit event types and reads two existing tables)

Scope: when the daemon activates an employee and the employee's latest session at the same configuration revision is `suspended`, the new session starts with the provider's resume (`SessionRequest::resume(native_id)`) in the suspended session's working directory; every other activation starts fresh as before, and a suspended predecessor that was not continued is audited with the reason. The decision lives in `App::start_new_session`, so the user-turn, notice-turn and `session.warm` paths share it; task dispatch passes its own workspace and never resumes. New code is `apps/lazood/src/app/resume.rs`, a small call-site change in `app.rs`, one `terminal.rs` predicate, and four read/audit methods on `Store` (`previous_session_for_employee`, `session_has_completed_turn`, `resumed_working_directory`, `record_session_resume_outcome`). No protocol type, RPC method, error code, migration, or desktop change.

## Outcome

PASS for what this slice claims: with a deterministic fake backend, an idle Claude Code lead (full autonomy) that suspends after its idle timeout is woken by the next user turn on the same native session id, in the prior session's working directory, with one `session.resumed` event naming both sessions and the native id; a Codex lead in the same situation starts a fresh session with one `session.resume.skipped` event (`codex-dynamic-tools`) and no resume request reaches the provider.

No live resume was run. Whether `claude -p --resume <id> --input-format stream-json` continues a real conversation under Lazoo's flags is not proven here; the fake backend proves the daemon asks for it correctly, not that Claude Code honours it. No live model turn was run and no provider binary was launched. No release binary was built in this worktree.

Resume works today for Claude Code only, and only under full autonomy, because that is the one authority Lazoo delivers to Claude Code. Codex, OpenCode, Grok Build and Antigravity start fresh.

## Design notes

| Decision | Reason |
|---|---|
| Decide in `start_new_session` when the caller passed no working directory | It is the single place a `SessionRequest` is built. The user-turn, notice-turn and `session.warm` callers pass `None`; task dispatch passes its task workspace and so never resumes. |
| Candidate is the employee's newest other session at the current configuration revision, and it must be `suspended` | `previous_session_for_employee` reuses the existing `latest_session_for_employee` query (now with an exclusion for the session being started). Scoping to the current revision is the PROJECT-BRIEF section 9.4 rule that a changed configuration never resumes. A `failed` newest session is a candidate for nothing and is not audited. |
| "Resumable" is `attach_args(provider, native_id).is_some()` | Reuses the existing predicate: Claude Code and Codex with a real id; never OpenCode, Grok Build, Antigravity, an empty id, or a `pending:` id. Reason `unsupported-provider`. A missing native id is folded into the same reason. |
| Codex always starts fresh, reason `codex-dynamic-tools` | `thread_resume_params` fails closed when tools are declared, because `thread/resume` has no `dynamicTools` field in codex-cli 0.153.4 or 0.162.0-alpha.17.2 (checked against both binaries' generated JSON schema; unknown fields are silently dropped). Lazoo declares its collaboration tools to every Codex session, so a resumed thread would silently lose them. The daemon decides up front instead of letting the adapter fail the start. |
| Task-dispatch sessions are never resumed by a conversation, reason `task-session` | `task_workspace_for_session(prior)` is `Some`; that session ran in a task worktree under a task's authority ceiling. A real dispatched session exercises this in `claude_code_runs_turns_and_tasks_only_under_full_autonomy`. |
| Memory-summary sessions stay fresh, reason `memory-reserved` | Checked for both the session being started (reserved by `reserve_memory_summary_session` before the start) and the candidate, so summary source data never shares native context with normal work. |
| A candidate that never completed a conversation turn is not resumed, reason `no-completed-turn` | Added during implementation; see the next section. Uses `conversation_turns.recipient_session_id` with `state = 'completed'`. |
| A candidate with a live owner terminal attachment is not resumed, reason `owner-terminal-attached` | `session_has_live_attachment` applies the same test as `reserve_attachment` (after `release_exited`): a `None` slot or a still-live terminal counts. The owner is driving that conversation; a second native process on it would fork it. |
| Resume runs in the prior session's directory, reason `working-directory-missing` if it is gone | Claude Code files its transcript under the directory it ran in. A resumed session ran in its predecessor's directory, not its own, so the directory is recorded in the `session.resumed` payload (`workingDirectory`) and `resumed_working_directory` reads it back, otherwise a second resume would look for `sessions/<second id>` and wrongly skip. Found by the test's third wake. |
| `session.resumed` carries `workingDirectory` beyond `sessionId`, `priorSessionId` and `nativeSessionId` | The only place the origin directory can be kept without a column. It is also the audit of where the conversation lives. |
| Audit after `mark_session_ready` succeeds, best effort | A start that fails never leaves a `session.resumed` behind. The session is live by then, so losing the audit row must not fail the wake (`let _ =`). The cost is that a skipped decision on a start that then fails is not recorded. |
| A store error while deciding falls back to a fresh start | Resume is an optimisation; it must never stop an employee waking. |
| No retry of a rejected resume | Out of scope for this slice. See the next section for what bounds the damage instead. |

## Late-failure self-healing: checked against the code

The slice plan expected a bad Claude resume to fail on the first turn, after which `fail_session` marks the session `failed` and the next activation finds no suspended candidate. Reading the code does not support that. `fail_session` (`crates/lazoo-store/src/session.rs`) is reached only through `record_session_failure`, and the callers of that are the start path, the stop path, shutdown, and memory-worker cleanup. A failed or uncertain conversation turn goes through `fail_provider_delivery` / `mark_turn_uncertain`, which change the turn and raise an incident but never touch the session. So a session whose resume the provider rejected stays `idle`; idle suspension would later stop it cleanly (it has no unfinished turn) and it would end `suspended` carrying the same native id. Without a guard, every later wake would resume the same rejected id again.

The slice therefore adds one guard the plan did not list: a candidate must have completed at least one conversation turn (`no-completed-turn`). A session that took no turn has no transcript to resume, and a rejected resume leaves exactly such a session. Its suspension is skipped on the next wake, a fresh session starts, and that session's completed turns make it a valid candidate afterwards, so the loop ends after one failed wake without a retry. This is also why a warmed-but-unused Claude session is not resumed: Claude Code writes its transcript with the first message. The guard is proven by `a_suspended_claude_code_session_that_never_completed_a_turn_is_not_resumed`; the claim that a real Claude rejection takes this path is an inference from the code, not an observation.

## Fresh verification matrix

All commands were run from the worktree `C:\workspace\Orchestrator\.claude\worktrees\s1-resume` with `CARGO_TARGET_DIR=C:\workspace\Orchestrator\.claude\worktrees\s1-resume\target` after the final change.

| Command | Exit | Evidence |
|---|---:|---|
| `cargo fmt --all -- --check` | 0 | No formatting differences. |
| `cargo clippy -p lazood -p lazoo-store --all-targets --locked -- -D warnings` | 0 | Clean. |
| `cargo test -p lazood --test daemon an_idle_codex_session_suspends_after_its_employee_timeout_and_the_next_user_turn_starts_fresh_with_a_recorded_reason --locked -- --exact` | 0 | 1 passed (was `an_idle_session_suspends_after_its_employee_timeout_and_the_next_user_turn_starts_fresh`): starts == 2, `resume_ids` empty, exactly one `session.resume.skipped` with `codex-dynamic-tools` naming both sessions. |
| `cargo test -p lazood --test daemon an_idle_claude_code_session_suspends_and_the_next_user_turn_resumes_its_native_session --locked -- --exact` | 0 | 1 passed (new): first wake has no resume event; second wake resumes `native-1` in the first session's directory with one `session.resumed`; a third wake resumes the same id in the same origin directory. |
| `cargo test -p lazood --test daemon a_suspended_claude_code_session_that_never_completed_a_turn_is_not_resumed --locked -- --exact` | 0 | 1 passed (new): a warmed, suspended session is skipped with `no-completed-turn`. |
| `cargo test -p lazood --test daemon claude_code_runs_turns_and_tasks_only_under_full_autonomy --locked -- --exact` | 0 | 1 passed, extended: the task dispatch requested no resume, and the Claude lead's later conversation start records `session.resume.skipped` (`task-session`) naming the dispatch session. |
| `cargo test -p lazood --test daemon the_core_scenario_runs_end_to_end_on_the_deterministic_provider --locked -- --exact` | 0 | 1 passed (step 14 suspends a Codex employee; the extra skipped event does not disturb it). |
| `cargo test -p lazood --test daemon core_scenario_acceptance_contract_maps_every_step_to_evidence --locked -- --exact` | 0 | 1 passed. |
| `cargo test -p lazood --test daemon idle_suspension_acceptance_contract_is_deterministic_and_backend_only --locked -- --exact` | 0 | 1 passed: pins the renamed and two new test names, `claudeCodeResumedOnWake`, `codexStartsFreshWithReason`, and `schemaVersion = 19`. |
| `cargo test -p lazood --test daemon <name> --locked -- --exact` for each of `idle_suspension_waits_for_an_unfinished_turn_and_never_touches_a_zero_timeout`, `a_turn_recorded_on_a_since_stopped_session_is_delivered_on_the_current_session`, `a_notice_turns_lead_suspended_for_idleness_is_woken_once_by_its_childs_result` | 0 | 3 runs, 1 passed each (the remaining idle test is the Codex flip above). |
| `cargo test -p lazood --test daemon --locked -- --skip task_graph_acceptance_executes_backend_only_contract` | 0 | 134 passed, 0 failed, 1 filtered out. The skipped test executes `task-delegation-graph.ps1`, which needs a release build and was deliberately not run; it is the only test in the file that spawns a script. |
| `cargo test -p lazood --lib --locked` | 0 | 14 passed (including the memory-worker session tests). |
| `cargo test -p lazoo-store --test store --locked` | 0 | 14 passed. |
| `pwsh -NoProfile -File scripts/acceptance/idle-suspension.ps1` | 0 | PASS manifest at `artifacts/acceptance/idle-suspension-manifest.json`: schema 19, `deterministic-fake-transport`, 13 commands (4 library, 1 CLI, 6 daemon, 1 store, 1 desktop diff guard), every evidence flag true. |
| `git diff --quiet -- apps/lazoo-desktop` | 0 | Empty. |

Not run, deliberately: `cargo test --workspace`, `scripts/gate.ps1`, `task_graph_acceptance_executes_backend_only_contract` and every other real-binary script (they need a release build owned by the main checkout's gate), `provider-adapters.ps1` (live model turn), and the other store test files, which the changed crate surface (`pub(crate) latest_session_for_employee` gaining a parameter, four new methods) does not reach beyond what `--test store` and the acceptance script's `session` test cover.

## Boundary evidence

- `git diff --quiet -- apps/lazoo-desktop` is empty. No Tauri command, `desktop.snapshot` field, frontend source, migration, protocol type, RPC method, or error code changed.
- Schema stays 19. The two new events (`session.resumed`, `session.resume.skipped`) are `audit_events` rows written like `session.terminal.attached`; no session row changes. `lazoo-store` gains one public enum (`SessionResumeOutcome`) and four public methods.
- No live resume was run, no provider binary was launched, and no prompt was sent to a model. `request.session_id` reaches only the deterministic fake.
- Providers that can resume today: Claude Code, under full autonomy only. Codex cannot (dynamic tools); OpenCode, Grok Build and Antigravity have no governed delivery or no real conversation id.
- The real-binary turn-transport fixture needs no change: it treats `thread/start` and `thread/resume` alike and no daemon path now sends a Codex resume.

## Retained follow-up boundaries

- A live Claude resume proof is owner-authorized work: start a Claude Code lead under full autonomy, let it suspend, wake it, and confirm the second turn sees the first turn's context, and that a deleted transcript fails as described above. Until then the Claude path is deterministic-fake evidence only.
- Codex resume waits on a vendor way to carry dynamic tools across `thread/resume` (a `dynamicTools` field or a session-level declaration). When one exists, drop the `codex-dynamic-tools` branch and let the adapter's resume carry the tools.
- OpenCode and Antigravity resume waits on governed delivery for those providers (and, for Antigravity, a real conversation id instead of the `pending:` placeholder).
- A resume the provider rejects still costs one failed first turn; the `no-completed-turn` guard stops it repeating but nothing retries the turn on a fresh session. A retry is a separate decision.
- A resumed session's origin directory is kept only in the `session.resumed` event. If audit events are ever pruned, a resumed lineage degrades to `working-directory-missing` and the next wake starts fresh.
- A notice-turn session (the lead woken by a child's result and stopped after that one turn) is a valid candidate, so the next user turn resumes it. That shares the notice's context with the user conversation, which is the point of resuming the lead, but it is a policy the owner has not been asked about.
