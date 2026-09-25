# Lazoo

Lazoo is a Windows-first, local desktop control plane for a personal team of native coding-agent harnesses. It is intended to coordinate Codex, Claude Code, OpenCode, Grok Build, and Antigravity employees without replacing or flattening their native environments.

The name is a deliberately silly blend of **lazy** and **zoo**: wake the right employee when work arrives, let the team coordinate, and keep the menagerie under control.

## Status

The first Windows control-plane slice, polished read-only desktop shell, native-provider adapters, durable employee roster, native-session ownership, collaboration bus, conversation/turn delivery, schema-7 automatic task dispatch, the schema-8 approval gate, Codex collaboration tools, the schema-9 incident ledger, lead notice delivery, and peer messaging tools are implemented. Lazoo provides five native executables:

- `lazoo-desktop.exe` opens a dark, command-first Tauri 2 control room for daemon health, projects, recent events, and recovered-incident count.
- `lazoo-keeper.exe` supervises the daemon with a bounded restart policy and emergency spool.
- `lazood.exe` owns the SQLite WAL store, revisioned employee roster, native-session lifecycle, collaboration-message, conversation/turn, and task/evidence ledgers, and authenticated named-pipe API.
- `lazooctl.exe` exposes status, project registration/listing, event listing/following, native-provider diagnostics, employee/roster/session/message/conversation/turn/task control, and planned shutdown.
- `lazoo-session-host.exe` is the protocol-transparent launch gate that places each native provider tree in its daemon-owned Windows Job Object before provider startup.

The desktop remains deliberately read-only. Backend adapters connect Codex App Server, OpenCode ACP, Grok Build ACP, Antigravity stream JSON, and Claude Code stream JSON without flattening native configuration or authentication. Schema 7 adds durable dispatch ownership and workspace evidence to the task tree. Accepted Codex child tasks wake automatically when their assigned worker is free, reuse the one canonical delegation message, and start only after native acceptance or durable progress. Projectless tasks use owner-protected private directories; project tasks use deterministic retained Git worktrees and never mutate dirty canonical checkout files. Configuration drift and pre-provider restart fail atomically, possible in-flight restart becomes `uncertain`, and task cancellation interrupts an accepted native turn. Schema 8 adds the durable approval ledger: Codex command-execution and file-change approval requests are recorded first, answered by policy under `self-decision` and `full-autonomy`, and held for the local user under `ask`. Pending approvals expire, cancel with their turn, and fail closed on restart; the provider is always answered `decline` when no approval exists. Every daemon-managed Codex session also receives four Lazoo collaboration tools (`employees_list`, `task_delegate`, `task_wait`, `task_report`; the brief's dotted names, spelled with underscores because the Codex Responses API rejects dots) through the app-server dynamic-tool surface, so a lead can read the live roster, delegate child tasks, wait for their evidence, and report structured outcomes from inside its native session; each call acts only as the turn's own employee and session and goes through the same fail-closed task guards as the CLI. Schema 9 adds the human-readable incident ledger: session start failures, provider stalls and rejections, dead letters, dispatch failures, restart owner loss, and keeper emergency records fold into one incident per code, employee, and project, three equivalent failures inside five minutes promote it to `priority`, only the local user acknowledges or resolves it, and employees read and report incidents through `incident_list` and `incident_report`. Lead notice delivery closes the loop back to the lead without a schema change: a child task that reaches a terminal state and an incident promoted to `priority` become durable `result` and `status` notices addressed to the lead of the task tree, delivered as one provider turn each on the lead's idle session, consumed directly when the lead is already inside `task_wait`, and held while the lead is dormant until the local user resumes it; a notice never starts a provider process. Peer messaging rides the same path without a schema change: `message_send` and `message_reply` let an employee ask another employee in its task tree a question and answer one, each message links both tasks and threads under the message it answers, a recipient reads it inside its own `task_wait` or `message_send` wait or as one turn on its idle session, a peer turn never completes the recipient's task, and a dormant recipient is never activated. The earlier authorized live conversation gate completed with exact `LAZOO_TURN_OK`; the automatic-dispatch, approval-gate, collaboration-tools, minimum-incidents, result-delivery, and peer-messaging acceptances are deterministic and did not invoke a live model. Managed memory, ConPTY, session-wide grants, bounded remediation and automatic lead activation, cross-tree messaging, other-provider task delivery, approval bridging, and tool surfaces, and UI binding remain later slices. The full product contract lives in [docs/PROJECT-BRIEF.md](docs/PROJECT-BRIEF.md).

The target desktop information architecture is owner-approved, but UI implementation is owned separately while backend work continues. The current read-only shell remains this branch's technical baseline. Continue from [docs/NEXT-SESSION.md](docs/NEXT-SESSION.md); no launcher, terminal, worker, Inbox, Settings, or task field was added to the desktop contract.

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
$approvalId = '<approval-uuid>'
& .\target\release\lazooctl.exe approval list --state pending
& .\target\release\lazooctl.exe approval show $approvalId
& .\target\release\lazooctl.exe approval approve $approvalId --expected-revision 1
& .\target\release\lazooctl.exe approval decline $approvalId --expected-revision 1
$incidentId = '<incident-uuid>'
& .\target\release\lazooctl.exe incident list --state open --priority priority
& .\target\release\lazooctl.exe incident show $incidentId
& .\target\release\lazooctl.exe incident acknowledge $incidentId --expected-revision 1
& .\target\release\lazooctl.exe incident resolve $incidentId --expected-revision 2
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

`conversation create` and `turn submit/retry/cancel/list/show` use the schema-7 conversation ledger and actor-aware version-2 envelopes. User-authored submission may explicitly activate the selected Codex lead with `--activate-if-needed`; employee-authored turns cannot activate dormant recipients. Native turn IDs or observed progress are required before message acknowledgement, accepted turns are never retried automatically after uncertainty, and provider terminal states remain distinct from canonical delivery truth. Conversation/turn failures use CLI exit code 12.

`task delegate/accept/start/report/cancel/resolve/list/show` operates on the durable schema-7 task graph. Root task identity is the creating user-turn identity; child depth is capped at four and nonterminal direct fan-out at eight. Authority is the most restrictive of parent, request, and assignee. Accepted non-ask children and user-accepted proposals are claimed once by the daemon, dispatched when their worker is free, and expose dispatch/workspace evidence through `task show`. Native evidence, revision checks, actor/session checks, cancellation, descendant blocking, and owner-loss uncertainty fail closed. Git worktrees are retained for audit; cleanup is intentionally not automatic. Task failures use CLI exit code 13. Task controls are daemon/CLI-only and do not expand `desktop.snapshot`.

`approval list/show/approve/decline` operates on the schema-8 approval ledger. When a daemon-managed Codex turn asks to run a command or change files, Lazoo records the request first and applies the linked task's effective authority: `ask` holds the provider until the local user decides, `self-decision` approves in-scope work by policy and holds scope expansions, and `full-autonomy` approves by policy. A decision is bound to one approval and revision; there is no session-wide grant. Pending approvals expire after fifteen minutes, close when their turn is cancelled, and close with `LZ-APPROVAL-OWNER-LOST` on restart, and every closure answers the provider `decline`. Approval failures use CLI exit code 14. Approval controls are daemon/CLI-only and do not expand `desktop.snapshot`.

Codex collaboration tools need no CLI command: the daemon declares `employees_list`, `task_delegate`, `task_wait`, and `task_report` to every Codex session it starts, and the model calls them natively. `task_delegate` must cite the `rosterRevision` returned by `employees_list`, never widens authority, and creates a `proposed` task under `ask` that waits for `lazooctl task accept`; otherwise the child is accepted and dispatched automatically. `task_wait` blocks for at most ten minutes and returns each child's state, failure, and native output. `task_report` records progress, completion, or failure for the caller's own task. Refused calls answer the model a stable `LZ-TOOL-*` or `LZ-TASK-*` code without ending the turn, and a session without the daemon attached answers `LZ-TOOL-UNATTACHED`. Tools are declared only for Codex and only on `thread/start`.

`incident list/show/acknowledge/resolve` operates on the schema-9 incident ledger. The daemon raises an incident right after it commits a session start failure, a provider rejection, stall, timeout, exit, or missing completion evidence, a dead letter, a dispatch failure, or restart owner loss, and folds keeper emergency records in at startup. Equivalent failures share one row whose occurrence counters advance; three inside five minutes promote it to `priority`. Each incident carries a plain-language title, explanation, and next action, the linked employee, project, task, session, turn, and message ids, and bounded detail with the raw failure code. Acknowledging and resolving are revision-checked and idempotent; a new occurrence reopens an acknowledged incident, and a resolved one never reopens. Employees see their project's open incidents through `incident_list` and record their own through `incident_report`. Incident failures use CLI exit code 15. Incidents never retry work, change authority, or expand `desktop.snapshot`.

Lead notices need no CLI command either. When a child task reaches `completed`, `failed`, `cancelled`, or `uncertain` by any path, the daemon queues one `result` message from the worker to the employee that owns the parent task, threaded under the delegation, carrying the state, failure, native output bounded to 16 KiB, and the count of unfinished siblings. When an incident about a child task is promoted to `priority`, it queues one `status` message to the lead of the tree. A notice is delivered as a single provider turn on the lead's idle session under the lead task's effective authority, only while no other turn on that task is unfinished; a lead already inside `task_wait` receives the outcome there and the notice is acknowledged with that turn as evidence. A dormant lead is never activated by a notice: it stays queued with its turn recorded until `lazooctl session warm` or a new user turn resumes the lead. Notices about the lead itself, about the root task, or on a terminal tree are never sent, and `lazooctl message list` shows every notice and its delivery state.

Peer messages need no CLI command either. Inside a Codex turn, `message_send` sends up to 16 KiB of text to another employee that has an accepted or running task in the caller's task tree (the lead or a sibling), and `message_reply` answers a message the caller received by citing its id; the daemon fixes the sender to the turn's own employee and session, links the sender's and recipient's tasks, and threads a reply under the message it answers. A recipient reads peer messages inside its own `task_wait` (which returns early with `messages` so a lead can answer and wait again) or inside its own `message_send` wait (`waitForReplySeconds`, at most ten minutes, returns the `reply` inline), or, when it is idle with a running task, as one provider turn on its existing session. A peer turn only progresses the recipient's task, never completes it, so a question delivered just before a child's result cannot lose that result. A dormant recipient is never activated: the message stays `queued` until the local user resumes it, and a message whose recipient task ends first is closed as `cancelled`, which a waiting sender sees at once. Refused sends and replies (to oneself, outside the tree, empty or oversized text, or a message not addressed to the caller) answer `LZ-TOOL-INVALID` or `LZ-TOOL-SCOPE` without ending the turn, and a peer message is never an approval and never changes task ownership, authority, or permissions.

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
powershell -NoProfile -ExecutionPolicy Bypass -File scripts\acceptance\automatic-task-dispatch.ps1
powershell -NoProfile -ExecutionPolicy Bypass -File scripts\acceptance\approval-gate.ps1
powershell -NoProfile -ExecutionPolicy Bypass -File scripts\acceptance\collaboration-tools.ps1
powershell -NoProfile -ExecutionPolicy Bypass -File scripts\acceptance\minimum-incidents.ps1
powershell -NoProfile -ExecutionPolicy Bypass -File scripts\acceptance\result-delivery.ps1
powershell -NoProfile -ExecutionPolicy Bypass -File scripts\acceptance\peer-messaging.ps1
powershell -NoProfile -ExecutionPolicy Bypass -File scripts\acceptance\desktop-shell.ps1
powershell -NoProfile -ExecutionPolicy Bypass -File scripts\acceptance\provider-adapters.ps1 -Provider codex
```

The acceptance script uses a unique temporary data root, builds release binaries, forces a real daemon restart, exhausts the production keeper budget with a crashing fixture, and writes `artifacts\acceptance\first-slice-manifest.json`.

The employee-roster acceptance uses an isolated data root and real release binaries to verify idempotent creation, schema-7 restart durability, stale-revision rejection, non-model refresh, disable semantics, and identity preservation across a second restart. It writes `artifacts\acceptance\employee-roster-manifest.json`.

The native-session acceptance runs an independent release containment fixture, warms Codex without a prompt, proves warm/stop idempotency and revision checks, forces daemon loss, verifies complete-tree termination, and checks restart reconciliation to `LZ-SESSION-OWNER-LOST`. It writes `artifacts\acceptance\native-session-ownership-manifest.json`.

The collaboration-bus acceptance uses an isolated data root and real release binaries to verify record-first/idempotent sends, causal ordering, acknowledgement correlation, retry deadlines and exhaustion, dead-letter evidence, cancellation, forced-restart reconciliation to `LZ-MESSAGE-OWNER-LOST`, and audit/outbox publication without invoking a provider model. It writes `artifacts\acceptance\collaboration-bus-manifest.json`.

The conversation-delivery acceptance uses an isolated data root, real release binaries, and a disposable Codex App Server protocol fixture. It verifies schema 7, projectless/project conversations, actor-aware task-linked envelopes, record-first order, explicit activation, native acceptance, session-level progress, terminal mapping, bounded transcripts, retry/cancel behavior, owner-loss uncertainty, planned-shutdown provider-tree cleanup, audit/outbox publication, and zero desktop expansion without invoking a model. It writes `artifacts\acceptance\conversation-delivery-manifest.json`. The separate quota-consuming historical `-LivePrompt` gate passed the earlier conversation slice with exact `LAZOO_TURN_OK`; its evidence remains reported separately from deterministic task-graph proof.

The task-delegation-graph acceptance uses isolated schema-7 state, real release binaries, and the disposable Codex protocol fixture. It exercises root identity/replay/follow-up, ask proposal and user acceptance, authority narrowing, stale/unavailable/self/depth/fan-out guards, native start/progress/outcomes, descendant blocking, restart owner loss, idempotent uncertainty resolution, and audit/outbox correlation. It also proves provider smoke, live model use, worktree mutation, ConPTY, desktop expansion, and UI changes are absent. It writes `artifacts\acceptance\task-delegation-graph-manifest.json`.

The automatic-task-dispatch acceptance uses the in-process fake provider transport and disposable Git repositories. It verifies background wakeup after a busy worker stops, canonical-message reuse, projectless and Git-worktree execution, dirty-checkout preservation, configuration-drift isolation, sibling progress after one failure, cancellation before and after native acceptance, session cleanup, and both restart boundaries. It invokes no provider smoke or external model, retains task worktrees for audit, and writes `artifacts\acceptance\automatic-task-dispatch-manifest.json`.

The approval-gate acceptance uses the in-process fake session backend, the disposable Codex app-server fixture, and the store and CLI test suites. It verifies that `ask` holds the provider until the user decides and forwards `accept`, that decline, expiry, and turn cancellation answer `decline` with matching durable evidence, that `self-decision` approves in-scope work by policy and waits on scope expansion, that decisions are revisioned and idempotent, that prior-boot pending approvals expire with owner-loss evidence, that the Codex adapter answers the native request id, and that CLI mapping and exit code 14 hold. It invokes no provider smoke or external model and writes `artifacts\acceptance\approval-gate-manifest.json`.

The collaboration-tools acceptance uses the in-process fake session backend, the disposable Codex app-server fixture, and the contract and store test suites. It verifies that an unattached observer fails closed, that the Codex adapter opts into the experimental API only with declared tools, declares them on `thread/start`, answers `item/tool/call` with the Lazoo result, redacts retained arguments, and refuses tools on resume, that every daemon-managed Codex session declares the four tools, that a `self-decision` lead lists the roster, delegates, and waits until the automatically dispatched child completes with the worker's output, that an `ask` lead's delegation stays `proposed` while unknown, malformed, and widening calls are refused without ending the turn, that a worker's `task_report failed` is authoritative over its later native completion, and that schema stays 8. It invokes no provider smoke or external model and writes `artifacts\acceptance\collaboration-tools-manifest.json`.

The minimum-incidents acceptance uses the domain, store, daemon fake-backend, and CLI test suites. It verifies the plain-language taxonomy and next actions, the three-in-five-minutes threshold window, fold/escalate/replay semantics, revisioned and idempotent acknowledge and resolve, keeper spool records folding exactly once, session start failures, provider failures escalating and settling through RPC, dead-letter and dispatch-failure scope, restart owner loss folding once per employee, employee tools scoped to the caller, CLI mapping and exit code 15, and schema 9. It invokes no provider smoke or external model and writes `artifacts\acceptance\minimum-incidents-manifest.json`.

The result-delivery acceptance uses the domain, store, and daemon fake-backend test suites. It verifies bounded truthful notice text, one `result` per terminal child threaded under the delegation, notice turns that reuse the notice message and wait while the lead is busy, `task_wait` consuming results with evidence, priority incidents notifying the lead once, an idle lead receiving a result turn after its own turn ended, a dormant lead never being activated until the user resumes it, and schema 9.

The peer-messaging acceptance uses the domain, store, and daemon fake-backend test suites. It verifies bounded truthful peer text, messages that link both tasks and queue once, replies that thread under the question and return to the sender's task, in-turn reads with evidence that close a stale notice turn, peer turns that never complete the recipient's task, a worker's question reaching an idle lead as a turn with the reply returning inline to the waiting worker, `task_wait` returning early so a lead answers inside its own turn, a dormant recipient never being activated, refusals that never end the turn, and schema 9. It invokes no provider smoke or live model turn.

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
