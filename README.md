# Lazoo

Lazoo is a Windows-first, local desktop control plane for a personal team of native coding-agent harnesses. It is intended to coordinate Codex, Claude Code, OpenCode, Grok Build, and Antigravity employees without replacing or flattening their native environments.

The name is a deliberately silly blend of **lazy** and **zoo**: wake the right employee when work arrives, let the team coordinate, and keep the menagerie under control.

## Status

The first Windows control-plane slice, polished read-only desktop shell, native-provider adapters, durable employee roster, native-session ownership, collaboration bus, conversation/turn delivery, and schema-6 durable task/delegation graph are implemented. Lazoo provides five native executables:

- `lazoo-desktop.exe` opens a dark, command-first Tauri 2 control room for daemon health, projects, recent events, and recovered-incident count.
- `lazoo-keeper.exe` supervises the daemon with a bounded restart policy and emergency spool.
- `lazood.exe` owns the SQLite WAL store, revisioned employee roster, native-session lifecycle, collaboration-message, conversation/turn, and task/evidence ledgers, and authenticated named-pipe API.
- `lazooctl.exe` exposes status, project registration/listing, event listing/following, native-provider diagnostics, employee/roster/session/message/conversation/turn/task control, and planned shutdown.
- `lazoo-session-host.exe` is the protocol-transparent launch gate that places each native provider tree in its daemon-owned Windows Job Object before provider startup.

The desktop remains deliberately read-only. Backend adapters connect Codex App Server, OpenCode ACP, Grok Build ACP, Antigravity stream JSON, and Claude Code stream JSON without flattening native configuration or authentication. Durable named employees can be created, revised, disabled, inspected, warmed into an idle native session, stopped, and reconciled after daemon loss. The collaboration bus records immutable correlated envelopes before attempts and preserves dead-letter and owner-loss evidence. Schema 6 adds a durable task tree to the conversation ledger: every new user turn creates one root task with the same ID, existing-task follow-ups reuse that root, and eight task operations cover bounded delegation, acceptance, lifecycle evidence, cancellation, uncertainty resolution, and reads. Ask-mode delegation creates no message or provider action until user acceptance; non-ask delegation never widens authority. Task-backed Codex turns apply a per-turn permission ceiling and become `running` only after native acceptance or progress. Accepted children are queued but are not automatically dispatched. Deterministic release acceptance is green. The earlier authorized live conversation gate completed with exact `LAZOO_TURN_OK`, a durable native turn ID, acknowledged delivery, and clean provider-tree shutdown; no live model gate was run for the task slice. Automatic task dispatch, managed memory, worktree automation, approvals, ConPTY, and other-provider conversation delivery remain later slices. The full product contract lives in [docs/PROJECT-BRIEF.md](docs/PROJECT-BRIEF.md).

The target desktop information architecture is owner-approved, but UI implementation is owned separately while backend work continues. The current read-only shell remains this branch's technical baseline. Continue from [docs/NEXT-SESSION.md](docs/NEXT-SESSION.md) with automatic accepted-task dispatch and project/worktree isolation; no launcher, terminal, worker, Inbox, Settings, or task field was added to the desktop contract.

## Build and run

Install and build the checked-in desktop frontend before the locked Rust release build:

```powershell
npm --prefix apps\lazoo-desktop\ui ci
npm --prefix apps\lazoo-desktop\ui run build
cargo build --workspace --release --locked
& .\target\release\lazoo-keeper.exe
```

Open the desktop from another terminal:

```powershell
& .\target\release\lazoo-desktop.exe
```

The shell reconnects every ten seconds and keeps the last successful snapshot visible if the daemon becomes unavailable. It does not start or stop the keeper. The companion CLI remains available for control operations:

```powershell
& .\target\release\lazooctl.exe status
& .\target\release\lazooctl.exe project register C:\path\to\git-repository
& .\target\release\lazooctl.exe project list
& .\target\release\lazooctl.exe events list
& .\target\release\lazooctl.exe provider list
& .\target\release\lazooctl.exe provider probe codex
& .\target\release\lazooctl.exe employee create Rhea --provider codex --role Lead
& .\target\release\lazooctl.exe employee list
& .\target\release\lazooctl.exe roster show
& .\target\release\lazooctl.exe roster refresh
$employeeId = '<employee-uuid>'
$sessionId = '<session-uuid>'
& .\target\release\lazooctl.exe session warm $employeeId --expected-employee-revision 1
& .\target\release\lazooctl.exe session list
& .\target\release\lazooctl.exe session stop $sessionId --expected-revision 2
$recipientEmployeeId = '<employee-uuid>'
$messageId = '<message-uuid>'
& .\target\release\lazooctl.exe message send $employeeId $recipientEmployeeId --kind peer --body '{"text":"status-request"}'
& .\target\release\lazooctl.exe message list
& .\target\release\lazooctl.exe message claim $messageId --expected-revision 1
$conversationId = (& .\target\release\lazooctl.exe --json conversation create $employeeId --title 'First task' | ConvertFrom-Json).conversation.id
& .\target\release\lazooctl.exe turn submit $conversationId --text 'Describe the repository.' --activate-if-needed
& .\target\release\lazooctl.exe turn list $conversationId
$rootTaskId = '<task-uuid>'
$workerEmployeeId = '<worker-employee-uuid>'
& .\target\release\lazooctl.exe task delegate $rootTaskId --actor-employee $employeeId --actor-session $sessionId --assigned-employee $workerEmployeeId --objective 'Review the change.' --requested-authority ask --roster-revision 1
& .\target\release\lazooctl.exe task list --root-task-id $rootTaskId
& .\target\release\lazooctl.exe task show $rootTaskId
& .\target\release\lazooctl.exe system shutdown
```

Add `--json` for a stable machine-readable result. Mutable state defaults to `%LOCALAPPDATA%\Lazoo`; set `LAZOO_DATA_ROOT` to isolate a test installation.

`provider list` performs discovery only and never launches a provider. `provider probe <provider>` launches a bounded, non-model native readiness check. A model-backed diagnostic is always explicit because it may consume provider quota:

```powershell
& .\target\release\lazooctl.exe provider smoke codex --prompt "Reply with exactly LAZOO_CODEX_OK. Do not use tools."
```

Provider names are `codex`, `opencode`, `grok-build`, `antigravity`, and `claude-code`. Provider failures use the `LZ-PROVIDER-*` RPC family and CLI exit code 8. Provider diagnostics are intentionally absent from `desktop.snapshot`; no desktop UI was added for this slice.

`employee create` performs one bounded non-model readiness probe, then stores the named employee and safe normalized provider evidence transactionally. `employee list`, `employee show`, and `roster show` are read-only and never launch a provider. `roster refresh` explicitly reruns one non-model probe per enabled provider; it never invokes a model smoke. Employee conflicts and stale revisions use the `LZ-EMPLOYEE-*` RPC family and CLI exit code 9. Employee and roster fields remain absent from `desktop.snapshot`.

`session warm` is the only session command that launches a provider. It requires the employee configuration revision, records `starting` before launch, and returns only after the provider-native session reports an ID and owned host PID; it does not send a turn. Warm and stop are idempotent. Stop is revision-checked and bounded, while prior-boot active sessions reconcile to `failed` with `LZ-SESSION-OWNER-LOST` instead of adopting a PID. Session failures use CLI exit code 10. Session and roster fields remain absent from `desktop.snapshot`.

`message send` records an immutable version-1 envelope and queued delivery state before any attempt. Claim, acknowledge, fail, retry, and cancel are idempotent and revision-checked; exhausted attempts create durable dead-letter evidence, and prior-boot delivering attempts reconcile with `LZ-MESSAGE-OWNER-LOST`. Message failures use CLI exit code 11. These controls do not activate a provider, send a model turn, or expand `desktop.snapshot`.

`conversation create` and `turn submit/retry/cancel/list/show` use the schema-6 conversation ledger and actor-aware version-2 envelopes. User-authored submission may explicitly activate the selected Codex lead with `--activate-if-needed`; employee-authored turns cannot activate dormant recipients. Native turn IDs or observed progress are required before message acknowledgement, accepted turns are never retried automatically after uncertainty, and provider terminal states remain distinct from canonical delivery truth. Conversation/turn failures use CLI exit code 12.

`task delegate/accept/start/report/cancel/resolve/list/show` operates on the durable schema-6 task graph. Root task identity is the creating user-turn identity; child depth is capped at four and nonterminal direct fan-out at eight. Authority is the most restrictive of parent, request, and assignee. Native evidence, revision checks, actor/session checks, descendant blocking, and owner-loss uncertainty fail closed. Task failures use CLI exit code 13. Accepted child tasks remain queued until an explicit task-backed turn is submitted; automatic dispatch is not implemented. Task controls are daemon/CLI-only and do not expand `desktop.snapshot`.

For the frontend development loop, run the app from its Tauri directory with Tauri CLI 2 installed. The Tauri configuration starts Vite on `127.0.0.1:1420` automatically:

```powershell
Push-Location apps\lazoo-desktop
cargo tauri dev
Pop-Location
```

## Verification

```powershell
& .\scripts\verify-rust-toolchain.ps1
npm --prefix apps\lazoo-desktop\ui ci
npm --prefix apps\lazoo-desktop\ui run check
cargo test --workspace
cargo check --workspace --all-targets
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --all -- --check
cargo build --workspace --release --locked
powershell -NoProfile -ExecutionPolicy Bypass -File scripts\acceptance\first-slice.ps1
powershell -NoProfile -ExecutionPolicy Bypass -File scripts\acceptance\employee-roster.ps1
powershell -NoProfile -ExecutionPolicy Bypass -File scripts\acceptance\native-session-ownership.ps1 -Provider codex
powershell -NoProfile -ExecutionPolicy Bypass -File scripts\acceptance\collaboration-bus.ps1
powershell -NoProfile -ExecutionPolicy Bypass -File scripts\acceptance\conversation-delivery.ps1
powershell -NoProfile -ExecutionPolicy Bypass -File scripts\acceptance\task-delegation-graph.ps1
powershell -NoProfile -ExecutionPolicy Bypass -File scripts\acceptance\desktop-shell.ps1
powershell -NoProfile -ExecutionPolicy Bypass -File scripts\acceptance\provider-adapters.ps1 -Provider codex
```

The acceptance script uses a unique temporary data root, builds release binaries, forces a real daemon restart, exhausts the production keeper budget with a crashing fixture, and writes `artifacts\acceptance\first-slice-manifest.json`.

The employee-roster acceptance uses an isolated data root and real release binaries to verify idempotent creation, schema-6 restart durability, stale-revision rejection, non-model refresh, disable semantics, and identity preservation across a second restart. It writes `artifacts\acceptance\employee-roster-manifest.json`.

The native-session acceptance runs an independent release containment fixture, warms Codex without a prompt, proves warm/stop idempotency and revision checks, forces daemon loss, verifies complete-tree termination, and checks restart reconciliation to `LZ-SESSION-OWNER-LOST`. It writes `artifacts\acceptance\native-session-ownership-manifest.json`.

The collaboration-bus acceptance uses an isolated data root and real release binaries to verify record-first/idempotent sends, causal ordering, acknowledgement correlation, retry deadlines and exhaustion, dead-letter evidence, cancellation, forced-restart reconciliation to `LZ-MESSAGE-OWNER-LOST`, and audit/outbox publication without invoking a provider model. It writes `artifacts\acceptance\collaboration-bus-manifest.json`.

The conversation-delivery acceptance uses an isolated data root, real release binaries, and a disposable Codex App Server protocol fixture. It verifies schema 6, projectless/project conversations, actor-aware task-linked envelopes, record-first order, explicit activation, native acceptance, session-level progress, terminal mapping, bounded transcripts, retry/cancel behavior, owner-loss uncertainty, planned-shutdown provider-tree cleanup, audit/outbox publication, and zero desktop expansion without invoking a model. It writes `artifacts\acceptance\conversation-delivery-manifest.json`. The separate quota-consuming historical `-LivePrompt` gate passed the earlier conversation slice with exact `LAZOO_TURN_OK`; its evidence remains reported separately from deterministic task-graph proof.

The task-delegation-graph acceptance uses isolated schema-6 state, real release binaries, and the disposable Codex protocol fixture. It exercises root identity/replay/follow-up, ask proposal and user acceptance, authority narrowing, stale/unavailable/self/depth/fan-out guards, native start/progress/outcomes, descendant blocking, restart owner loss, idempotent uncertainty resolution, and audit/outbox correlation. It also proves provider smoke, live model use, worktree mutation, ConPTY, desktop expansion, and UI changes are absent. It writes `artifacts\acceptance\task-delegation-graph-manifest.json`.

The desktop-shell acceptance starts an isolated keeper and daemon, registers a disposable Git repository, opens the real WebView2 window, exercises `Ctrl+K`, arrow/Enter navigation, `Ctrl+Enter` inspector focus, Escape dismissal, and inspector focus return through Windows UI Automation, checks default and minimum-size captures, and writes `artifacts\acceptance\desktop-shell-manifest.json`. A 200% text-scale capture runs only when Windows is configured at 200%; otherwise the manifest records an explicit environment blocker.

## Operating boundary

- Windows first
- One local user
- Native provider sessions and terminals
- Git worktrees by default for editing assignments
- Configurable authority, with `ask` as the default
- Local control plane with no network listener by default
- Read-only desktop snapshot; control mutations remain in the CLI until separately designed

Each later slice still requires a reviewed design, a written implementation plan, and acceptance evidence.
