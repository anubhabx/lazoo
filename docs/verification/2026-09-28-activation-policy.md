# Activation policy verification

Date completed: 2026-09-28 IST

Branch: `feat/activation-policy`

Base: `main` at `2c28a0e` (`feat: deliver queued notices to a recipient as one turn`), the first commit pushed to `github.com/anubhabx/lazoo`

Product version: `0.1.0`

Database schema: 10 (`0010_activation_policy.sql` adds `employees.activation_policy`, default `manual`)

Scope: the `ActivationPolicy` protocol enum and employee field, the domain rule, the schema-10 column through the store, one bounded daemon activation per recorded notice turn, the `--activation-policy` CLI flag, acceptance harness, and continuity documentation. Desktop product code and contract and the tool catalog are unchanged.

## Outcome

PASS.

The local user can now opt one employee at a time into bounded automatic activation with `lazooctl employee update <id> --expected-revision N --activation-policy notice-turns`. When the daemon's notice pass finds such an employee dormant with a recorded notice turn, it starts one session keyed `notice-activation:<turnId>`, delivers that turn on the employee's own task, and stops the session (`notice-activation:<turnId>:stop`), so the employee is `suspended` again before the pass moves on. A replay of that key means the turn already used its one activation and waits for the user. A refused start fails the session, raises an `LZ-SESSION-START-FAILED` incident, and is never retried; the user's `session.warm` still delivers the same recorded turn, and a session the user warmed is never stopped by a notice.

Every employee written before schema 10 reads `manual`, and under `manual` nothing changed: employee-authored traffic never starts a provider process, a coalesced turn for a dormant recipient stays `recorded` with its notices `queued` until the user resumes it, and employee-authored `turn.submit` with `activateIfNeeded` is still refused. A policy change is revisioned like every other employee setting and refused while the employee has an active session.

No live model turn was run.

## Design notes

Design and implementation landed together under the owner's standing instruction to continue the build; `docs/superpowers/specs/2026-09-28-activation-policy-design.md` and `docs/superpowers/plans/2026-09-28-activation-policy.md` record the contract for owner review in the pull request.

| Decision | Reason |
|---|---|
| The policy is a defaulted employee column, `manual` unless the user changes it | It is the user's per-employee decision, revisioned and refused during an active session like `authority_mode` and `checkout_policy`; a `DEFAULT` keeps every existing row and every raw insert in the upgrade tests on `manual`. |
| The daemon copies task dispatch's start, deliver, stop sequence instead of widening `turn.submit` activation | Dispatch already proves the shape; the notice path only had to consult the policy on the `LZ-TURN-SESSION-NOT-READY` branch it already recognised, and `turn.rs` is untouched apart from exposing `turn_failure` to the sibling module. |
| `begin_session` is keyed `notice-activation:<turnId>` | The existing `session.warm` command replay gives "one activation per recorded turn" durably: a second pass for the same turn gets the earlier session back with no event and waits for the user. |
| The activated session is stopped whether the turn succeeded or failed | The bound is "for this turn only"; a session left idle would be a provider process nobody asked for, which is what the `manual` rule protects against. |
| `SessionConflict` and `SessionProviderNotReady` from `begin_session` mean wait, not error | A session still starting or stopping, or a provider that lost readiness, is the same "not ready" case the pass already leaves for the next event. |
| `notice_turn_may_activate` lives in `lazoo-domain` | Activation is policy, not traffic; the one-line rule sits beside `approval_policy` and has its own proof in the acceptance script. |
| The activation call in the notice loop is `Box::pin`ned | Clippy 1.98 `large_futures` reported the notice future at 16664 bytes once the activation path joined it; boxing that one call keeps the loop's future small. |

## Fresh verification matrix

All commands were run from `C:\workspace\Orchestrator` on branch `feat/activation-policy` after the final fixes.

| Command | Exit | Evidence |
|---|---:|---|
| `cargo fmt --all -- --check` | 0 | No formatting differences. |
| `cargo clippy --workspace --all-targets --locked -- -D warnings` | 0 | All workspace targets linted with warnings denied, after boxing the activation future. |
| `cargo test --workspace --locked` | 0 | Entire Rust workspace passed: 80 daemon integration tests (3 new: the policy activation scenario, the failed-activation scenario, and the acceptance-script contract; the tool harness now takes a policy and the fake backend can refuse a start), 7 store employee tests (1 new), 12 store delivery tests, 11 store tests, 9 domain delivery tests (1 new), 15 protocol contract tests (1 new), 20 CLI tests (1 extended), 16 Codex adapter tests, all other suites unchanged. Explicit live-provider tests remained ignored. |
| `cargo build --workspace --release --locked` | 0 | Locked release build. |
| `npm --prefix apps\lazoo-desktop\ui run check` | 0 | Prettier, TypeScript, Vitest, and the Vite production build passed on the unchanged frontend. |
| `scripts\acceptance\activation-policy.ps1` | 0 | PASS schema-10 manifest, `deterministic-fake-transport`, 10 commands, every evidence flag true. |
| `scripts\acceptance\coalesced-notices.ps1` | 0 | PASS schema-10 manifest; coalesced delivery and the `manual` dormant scenario unchanged. |
| `scripts\acceptance\peer-messaging.ps1` | 0 | PASS schema-10 manifest; a `manual` peer recipient is still never activated. |
| `scripts\acceptance\result-delivery.ps1` | 0 | PASS schema-10 manifest; a `manual` lead still waits for the user. |
| `scripts\acceptance\minimum-incidents.ps1` | 0 | PASS schema-10 manifest. |
| `scripts\acceptance\collaboration-tools.ps1` | 0 | PASS schema-10 manifest; the tool catalog is unchanged. |
| `scripts\acceptance\approval-gate.ps1` | 0 | PASS schema-10 manifest. |
| `scripts\acceptance\automatic-task-dispatch.ps1` | 0 | PASS schema-10 fake-transport manifest. |
| `scripts\acceptance\first-slice.ps1 -SkipBuild` | 0 | PASS manifest; database schema 10. |
| `scripts\acceptance\employee-roster.ps1 -SkipBuild` | 0 | PASS manifest; `employee.create` and `employee.update` accept and return `activationPolicy`. |
| `scripts\acceptance\collaboration-bus.ps1 -SkipBuild` | 0 | PASS manifest. |
| `scripts\acceptance\conversation-delivery.ps1 -SkipBuild` | 0 | PASS deterministic manifest; user-authored activation unchanged. |
| `scripts\acceptance\task-delegation-graph.ps1 -SkipBuild` | 0 | PASS manifest, run against the real daemon with the notice loop active. |
| `scripts\acceptance\native-session-ownership.ps1 -SkipBuild -Provider codex` | 0 | PASS manifest; real Codex CLI accepting the eight declared tools, non-model session contract only. |
| `git diff --check` | 0 | No whitespace errors. |

The fourteen acceptance scripts ran in one sequence; the session harness stopped that shell for host memory pressure while the last script, `native-session-ownership.ps1`, was starting, after the first thirteen had passed. That script was then run alone in the foreground and passed; no daemon process was left behind by the interrupted run.

Not run, as in every prior backend slice: `provider-adapters.ps1 -Provider codex` requires a live model turn (`LAZOO_CODEX_OK`) that the handoff does not authorize, and `desktop-shell.ps1` asserts against the unchanged, separately owned UI; its tracked manifest was left as committed.

## Activation-policy acceptance evidence

`artifacts/acceptance/activation-policy-manifest.json` records literal `PASS`, schema 10, and `deterministic-fake-transport`. Its focused commands prove:

- `ActivationPolicy` serialises as `manual` and `notice-turns`, defaults to `manual` on `employee.create`, and rejects any other value;
- the domain rule activates only under `notice-turns`;
- a policy change bumps the employee's configuration revision and the roster revision, an identical change bumps nothing and emits no event, and a row inserted without the column reads `manual`;
- `lazooctl employee create` defaults `activationPolicy` to `manual` and `employee update --activation-policy notice-turns` maps it;
- a dormant `notice-turns` lead with a queued child result is started exactly once by the notice pass, receives the result as one turn on its own task, completes the task, and is `suspended` again with exactly one more provider start and one more stop, and a further pass starts nothing;
- a refused start fails the session with `LZ-SESSION-START-FAILED`, raises one incident naming the lead and that session, leaves the turn `recorded` and the notice `queued`, is never retried by the next pass, and after the user's `session.warm` the same turn is delivered on the user's session, which is not stopped;
- a `manual` lead still waits for the user, and a `manual` recipient of a peer message is still never activated; and
- schema is 10 and the desktop tree is unchanged.

## Boundary evidence

- `git diff --check` passed.
- Both staged and unstaged diffs under `apps/lazoo-desktop` are empty.
- No Tauri command, capability, `desktop.snapshot` field, frontend source, or tool declaration changed; the only migration is the one defaulted column.
- No tool can read or set the activation policy; no activation resumes a Codex thread, widens authority, approves anything, changes task ownership, retries work, or deletes data.
- This slice is submitted as a pull request against `main` rather than a local fast-forward.
- `UI BACKEND CONTRACT: GO` remains intentionally withheld.

## Retained follow-up boundaries

Idle suspension of user-warmed sessions, activation triggers other than a recorded notice turn, activation budgets beyond one per turn, a message-arrival notification replacing the 250 ms polls, a live Codex tool-call, peer-message, coalesced-notice, and policy-activation gate, cross-tree and user-to-employee messaging through the tools, bounded lead remediation, user notification channels, typed desktop Inbox binding, ConPTY, per-capability policy, managed memory, scheduling, and resource governance remain separate reviewed slices.
