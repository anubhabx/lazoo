# Smoke-verified provider readiness verification

Date completed: 2026-10-17 IST

Branch: `feat/smoke-verifies-provider`

Base: `feat/core-v1-completion` at `e3cd3b5`

Product version: `0.1.0`

Database schema: 19 (no migration)

Scope: the daemon records a `verified` provider observation when `provider.smoke` completes a model turn; the store keeps a `verified` observation against later probes that merely cannot see authentication; a deterministic daemon story, a store story, and this record. No protocol, CLI, migration, or desktop change. No live provider run was made here and no support is claimed.

## Observed live failure

On 2026-10-17 `provider.smoke` completed a real model turn through the native path on OpenCode and returned `LAZOO_OPENCODE_OK`. Immediately afterwards `conversation create` with an OpenCode lead failed with `LZ-CONVERSATION-LEAD-NOT-READY`, and the roster entry read `unverified` with the reason "provider model readiness has not been verified".

Cause: the roster rule (`derive_availability`) makes an employee `Dormant` only when the observation's verification is `configured` or `verified` and a native or external authentication state is `ready`. The OpenCode adapter's probe (`configured_probe`) can only report `verification: unverified` with authentication `unknown`/`unknown`, because an ACP handshake cannot see a login and the free model needs none. The `verified` vocabulary existed in the rule, but nothing in the product produced it, so an OpenCode employee could never become dormant.

## Outcome

PASS (deterministic). A successful smoke now makes the provider's employees dormant, a later `roster.refresh` keeps them dormant, and a failed or unfinished smoke changes nothing.

## The rule

1. `provider.smoke` counts as answered when the turn returned a `turnCompleted` event whose status is not `failed`, `interrupted`, or `cancelled`, and no `error` event. The daemon then records one observation for that provider through `Store::refresh_provider_observations`, the same path a `roster.refresh` uses: `verification: verified`, authentication `native: ready` (a model answered through the native channel) and `external` as the base observation reported it, else `unknown`, no degradation reason. The roster revision advances and `roster.refreshed` is emitted exactly as a refresh would, and only when the effective observation changed (a second smoke over an unchanged verified row is a no-op).
2. A stored `verified` observation is sticky (`merge_provider_observation`, documented in code). When the incoming probe is `detected`, `unverified`, or `configured`, has the same version and protocol, and neither channel is `blocked` or `needs-login`, the row stays `verified` and keeps its `ready` native state; the rest of the probe (capabilities, external state, check time) is taken as reported. A `degraded` or `not-detected` probe, a `blocked`/`needs-login` state, an incoming `verified`, or a changed version or protocol replaces the row exactly as before, so a replaced binary must be verified again.
3. A smoke that errors, ends without a completion, or ends with a failed completion records nothing.

## Decisions

| Decision | Reason |
|---|---|
| Record only on a completed, non-failed turn | The doctrine is evidence of a model answer. ACP reports its stop reason (OpenCode: `end_turn`) as the completion status, so success cannot be tested as `completed`; only the unsuccessful statuses are named. Codex and Claude Code emit `failed` for a failed turn. |
| Metadata from a fresh probe taken in the same handler | The version, protocol, capabilities, and check time then describe the binary that just answered. If that probe is `degraded` or `not-detected` it cannot describe the provider that answered, so the stored observation is used instead; with neither, nothing is recorded and the smoke result is returned unchanged. |
| Stickiness in the store, in one choke point | `upsert_provider_observation` serves employee create, employee update, and (through the same merge in its loop) roster refresh. Putting the merge in the daemon's `refresh_roster` would have let an employee create or update, which also stores a fresh probe, downgrade `verified`. The refresh loop merges before comparing, so an unchanged verified row is not reported as a change. |
| Idempotency key `provider-smoke:<uuid>` | A client-chosen request id could repeat across smokes; the store replays a processed key without writing, which would drop the second verification. |
| Smoke starts its session through the daemon's session backend | `provider_smoke` previously called the provider registry directly, so the daemon story could not script a smoke turn. The production backend delegates to the same `ProviderRegistry::start`; the turn runs through `turn_observed` with an observer that declines every permission request, as the earlier `turn` did. |
| `verification` stays a free string | No new column or migration; schema 19 is unchanged. |
| Authentication `native: ready` | A model answered through the native channel. `external` is not claimed. |

## Commands

All commands ran in `C:\workspace\Orchestrator\.claude\worktrees\s5-verified` with `CARGO_TARGET_DIR` set to that worktree's `target` directory. The workspace test, `scripts\gate.ps1`, and release build were not run (integrator owns the full gate).

| Command | Exit | Evidence |
|---|---:|---|
| `cargo test -p lazoo-store --locked --test employee` | 0 | 9 passed, including `a_verified_observation_survives_probes_that_cannot_see_authentication`. |
| `cargo test -p lazood --locked --test daemon a_successful_smoke_verifies` | 0 | 1 passed: `a_successful_smoke_verifies_an_opencode_employee_and_survives_a_refresh`. |
| `cargo test -p lazood --locked --test daemon -- --skip task_graph_acceptance_executes_backend_only_contract` | 0 | 141 passed, 0 failed, 1 filtered out (the skipped release-build test); includes `opencode_runs_turns_and_tasks_only_under_full_autonomy`, `claude_code_runs_turns_and_tasks_only_under_full_autonomy`, `employee_roster_acceptance_contract_is_backend_only`, `roster_refresh_is_non_model_and_does_not_advance_unchanged_state`, and `employee_create_update_disable_and_roster_are_durable_and_ui_neutral`. |
| `cargo test -p lazoo-store --locked` | 0 | All suites passed. |
| `cargo test -p lazoo-domain --locked` | 0 | All suites passed. |
| `cargo fmt --all -- --check` | 0 | No formatting differences. |
| `cargo clippy -p lazood -p lazoo-store -p lazoo-domain --all-targets --locked -- -D warnings` | 0 | Clean. |

The daemon story drives the real OpenCode adapter probe against a stand-in executable (`apps/lazood/tests/fixtures/opencode-probe.cmd`, which answers `--version` and the ACP `initialize` handshake only) and the fake session backend for the smoke turn. It shows, in order: the employee is `unverified` after `roster.refresh`; `conversation.create` fails with `LZ-CONVERSATION-LEAD-NOT-READY`; a smoke whose turn errors, one that ends without a completion, and one that ends with a failed completion each leave it `unverified` with the roster revision and `roster.refreshed` count unchanged; a smoke with an `end_turn` completion makes it `dormant` with `verified`/`native: ready`, advancing the revision by one and emitting one `roster.refreshed`; a second `roster.refresh` returns no event and keeps it `dormant`; and `conversation.create` then succeeds. The probe launches its child through `lazoo-session-host.exe`, so the story builds that debug binary once (`cargo build -p lazoo-session-host --locked`), as the provider crate's own fixtures do.

`scripts/acceptance/employee-roster.ps1` was not run. It is a real-binary script that needs a release build, which this slice does not make; it pins no literal this slice changes (it accepts `disabled`, `offline`, `unverified`, or `dormant`, never calls `provider.smoke`, and asserts `providerSmokeAbsent`), and its daemon contract test `employee_roster_acceptance_contract_is_backend_only` passed.

## Boundary evidence

- No migration and no new column: schema 19 and the `provider_observations` table are unchanged.
- No desktop change: `apps/lazoo-desktop` is untouched, the desktop snapshot stays roster-neutral (the story ends with `assert_desktop_is_roster_neutral`), and the UI vocabulary Detected/Configured/Degraded/Unverified/Verified is unchanged.
- No protocol or CLI change.
- No live run: no real provider binary was launched and no model prompt was sent. The probe ran against a stand-in executable and the smoke turn against the fake session backend. The live recheck (a real OpenCode smoke followed by `roster.refresh` and `conversation create`) is the integrator's to run after merging.
- `UI BACKEND CONTRACT: GO` remains intentionally withheld.

## Retained boundaries

- `verified` means model readiness for that binary version, recorded after one answered smoke turn. It is not a full brief section 21.3 lifecycle claim (session, user turn, dispatched task, cancellation, and resume are separate live gates).
- Verification does not expire except when the version or protocol changes, a probe degrades or reports blocked authentication, or a later answered smoke replaces it. A provider that loses its login without changing version is not detected by a probe that cannot see authentication.
- `provider.smoke` verifies the provider, not an employee's model, effort, or native settings.
- The stored observation is per provider, so one answered smoke makes every employee of that provider dormant.
- The roster rule, the availability reasons, and the UI vocabulary are unchanged.
