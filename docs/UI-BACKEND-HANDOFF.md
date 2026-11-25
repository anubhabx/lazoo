# Backend integration guide for the first UI

The backend supports Codex, OpenCode, Claude Code and Antigravity, including their
genuine native TUIs during orchestrated work. The current provider matrix and
acceptance evidence are in the [provider parity handoff](UI-PROVIDER-PARITY-HANDOFF.md).
The earlier [Codex verification record](verification/2026-10-29-ui-backend-readiness.md)
is retained as the reference checkpoint; its provider restrictions are superseded here.
The frontend is not implemented here. Full core v1 acceptance remains broader.

## Sources of truth

Use the typed structures in `crates/lazoo-protocol` and the daemon's RPCs. The
existing desktop snapshot is a shell/health read, not a replacement for the
workspace, Inbox, roster, memory, configuration, and terminal APIs. Keep the Rust
client and daemon from the same build while these pre-release contracts evolve.
Expose explicit Tauri commands to the frontend; do not expose arbitrary RPC or
process execution. No frontend binding is added here.

| UI surface | Backend reads |
| --- | --- |
| Health and registered project rail | `desktop.snapshot`, `system.status`, `project.list` |
| Home/project launch | `workspace.launcher` |
| Conversation sidebar | `workspace.conversations` |
| Active work, lead and workers | `workspace.conversation`, `turn.show` |
| Global attention queue | `inbox.list`, `inbox.counts` |
| Employee settings | `employee.list`, `employee.show`, `roster.get`, `provider.list` |
| Configuration discovery and selected migration | `provider.config`, `provider.config.preview`, `provider.config.select.preview`, `provider.config.operations.show` |
| Shared memory | `memory.policy.show`, `memory.search`, `memory.queue.status`, `memory.worker.show` |
| Resource display | `system.resources`, `session.resources` |
| Owner terminal | Terminal lifecycle RPCs and the separate authenticated byte channel |
| Native workspace trust | `native.trust.list`, `native.trust.show`, `native.trust.policy.show` |
| Skills and native plugins | `skill.*`, `plugin.*`; see **Skills and plugins** below |

The exact supported method names are dispatched in `apps/lazood/src/app.rs`;
avoid deriving additional methods from this table. The CLI provides executable
examples through `--help` and `--json`.

Selected Codex MCP configuration apply remains gated to exact reviewed versions:
`0.155.1`, `0.162.0-alpha.17.2`, `0.165.0-alpha.1.2`, and `0.166.0-alpha.5.1`. An installed provider is
not automatically eligible for configuration writes. Use preview results and
their refusal reasons; see the [current compatibility evidence](verification/2026-11-04-codex-mcp-configuration-0.166.md).

## Launch and capability handling

The top-level launcher `authorityModes` lists the vocabulary. The selected lead's
`authorityModes` is its usable subset. Require `startable`, show `blockingReason`
when false, and never offer a mode absent from that subset. `defaultAuthority`
must not be treated as an independent grant. An employee's configured authority
is a ceiling; choosing another value in the UI cannot widen it.

`collaborationToolsAvailable` reports whether Lazoo injects the roster, delegation,
wait, reporting, and peer-message tools. It is true for the four governed providers.
A startable conversational provider with that field false cannot be presented
as providing automatic team orchestration.

`nativeViewSupported` is a separate provider capability: it says whether Lazoo can
offer the controller-isolated genuine native TUI path at all. It is not a claim
that a particular lead is currently eligible to open one; use
`session.terminal.status.viewable` and its blocking fields for that live truth.

Current daemon policy, independently of whether a provider is installed or logged in:

| Provider | Conversation authority offered by Lazoo | Collaboration tools | Native live view |
| --- | --- | --- | --- |
| Codex | Modes up to the employee's configured ceiling | Yes | Yes |
| OpenCode | Modes up to the employee's configured ceiling | Yes | Yes |
| Claude Code | Modes up to the employee's configured ceiling | Yes | Yes |
| Antigravity | Modes up to the employee's configured ceiling | Yes | Yes |
| Grok Build | Excluded from core v1 and refused for governed delivery | No | No |

These are Lazoo implementation limits, not claims about every feature of the
vendor products. OpenCode governed starts also inspect configuration for native
permission overrides and may refuse at launch. A launcher read is advisory;
the mutation always rechecks its actual operating conditions.

Claude and Antigravity startup waits for explicit native workspace trust by default.
Show the pending notification and exact provider/path, then resolve it through
`native.trust.resolve` with its revision and idempotency key. Offer the optional
"Automatically accept next time" choice for that provider's future Lazoo-managed
workspaces. It never authorizes unmanaged paths. Keep cancellation distinct from
declining trust, and allow resetting the provider policy. See the parity handoff
for the complete lifecycle and CLI examples.

Antigravity needs a managed workspace where Lazoo can create its native hook/MCP
configuration; existing configuration collisions are refused. OpenCode needs a
verified selected model before launch. These conditions should be shown using
the returned readiness or launch refusal, rather than hidden behind a spinner.

The existing launch contract is two operations: `conversation.create`, then
`turn.submit` with `activateIfNeeded: true`. It is not an atomic transaction across
the provider. Persist a distinct idempotency key per logical operation until its
outcome is known. Reuse that same key and same payload to reconcile a lost reply;
do not generate another key and accidentally create another request. A created
conversation may remain empty if its first submission cannot be recorded.

## Execution, errors, and recovery

`Recorded` and a queued message mean durable intent only. Show execution only when
native acceptance/progress proves it. An approval means Lazoo granted that one
action, not that the provider executed it. Never automatically resend an
`Uncertain` turn or task.

After a refusal with `data.turnId`, read `turn.show` and refresh the workspace.
Preserve the returned failure code/message for a useful explanation. A transient
session readiness failure differs from a permanent provider/authority refusal;
do not map every error to either “running” or “try again”.

A direct user root refused with `LZ-TASK-AUTHORITY-UNSUPPORTED` is settled as
`terminal-failed` together with its canonical message and failed root task.
Its failed task evidence uses the user actor, no session ID, and
`detail.source: "daemon-pre-provider-rejection"`. No native acceptance is implied.
An identical submission key returns the existing failed record instead of
creating another attempt. A new request after changing configuration is a new
logical operation and needs its own key. Temporary dormant/activation/readiness
failures keep their previous explicit retry or cancellation behavior.

Send the server-provided Inbox/header/worker action with its revision, adding
only its required inputs and a fresh logical idempotency key. On a stale revision,
re-read before offering the action again. Do not change authority, resolve
uncertain work, or send a cancellation merely to clear a UI error state.

## Events and reconnect

Every workspace/Inbox model carries `eventHighWater`. Retain it and subscribe
with `events.subscribe { after: eventHighWater }`; the server replays durable
events and then streams. The mark is a lower bound across the response's reads,
so events after it may already exist when the read returns. Reprocessing an event
and refreshing is safe; advancing past an unseen event is not.

Replay uses bounded pages until the captured durable high-water is reached. Live
notifications, including broadcast lag, wake another durable drain; they are not
sent directly past unseen audit rows. Clients retain the last safely consumed
cursor if a slow connection is closed by the existing write deadline.

Debounce relevant-event refreshes to at most one read per 250 ms. See the
[contract index](superpowers/specs/2026-10-13-desktop-backend-contract-index.md)
for the event families per surface. On disconnect retain last-known information,
show connection uncertainty, and reconnect from the last safely consumed cursor.
Do not change durable lifecycle state because the desktop lost its connection.

## Skills and plugins

This contract is implemented, tested (`scripts/acceptance/skills-plugins.ps1`, in the
gate) and bound by Settings > Skills and Settings > Plugins. The desktop owns no
separate state: it calls the RPCs below,
keeps the returned `revision`, and refreshes after the durable events. All request
objects reject unknown fields. `ExtensionTarget` is `{ provider, accountId? }`,
where an omitted/null `accountId` means the personal harness and a non-null UUID
must identify an existing native-profile account. Provider names are the protocol
`ProviderKind` values. `mode` is `"link"` or `"copy"`.

Every preview has `{ previewId: UUID, revision: u64, expiresAt: RFC-3339 UTC }`.
At most eight skill/plugin previews are active together; they expire after ten
minutes. A skill preview also has `source`, `skills`, `resolvedRevision?`,
`conflicts`, and (for update previews) `updateName?`; each skill is
`{ name, description, relativePath, fileCount, bytes, checksum, problems }`.
A plugin preview has that header plus `{ request, targets }`. Each planned plugin
target is `{ target, before?, commands, error? }`; `before` is the redacted,
normalized native observation and `commands` are the exact native argument arrays
to be run. A preview is consumed only after a finished attempted mutation;
preflight refusals retain it. Missing, consumed, or
expired previews return `LZ-EXTENSION-PREVIEW-EXPIRED`.

The desktop allowlist uses its existing long-deadline class for previews and native plugin reads as well as mutations; `skill.list` uses the read deadline. This timeout classification does not make plugin list operations apply changes.

### Skill RPCs

| RPC | Parameters | Result |
| --- | --- | --- |
| `skill.list` | `{}` | `{ revision, library, exposures, inventory, problems, pending, targets }`. `library` entries add `{ source, resolvedRevision?, revision }`; exposures are `{ name, target, path, mode, checksum, libraryPath }`. Inventory entries report provider/target/name/description/path/canonicalPath/kind/managed/checksum/fileCount/bytes/problems/enabled plus `duplicate` and `alsoVisibleTo`; `kind` is `link`, `copy`, or `real-dir`, and `enabled` is nullable boolean. If linked/unreadable resources prevent a complete checksum, metadata remains visible with null `checksum`, `fileCount`, and `bytes`, plus a `problems` entry. Targets report roots, primary exposure directory, `Native`/`Unsupported`, and limitations. |
| `skill.source.preview` | `SkillSource`: `{ source: string, sourceKind?: "auto"|"local"|"github"|"skills-sh"|"git", gitRef?: string|null }` | A skill preview. Sources may be local directories, GitHub shorthand/tree URLs, skills.sh pages, or HTTPS Git URLs; the result records the resolved source revision when available. |
| `skill.update.preview` | `{ name }` | A skill preview for the managed library entry's recorded source; refuses an unmanaged name. |
| `skill.install` | `{ previewId, names: string[1..32 unique], providers?: ProviderKind[], mode, replace?: boolean, expectedRevision }` | `{ revision, outcome, library, exposures }`. Selected names must occur exactly once and have no validation problems. |
| `skill.update` | Same as install | Same result. It only accepts its `skill.update.preview`, exactly that one name, and `replace: true`. |
| `skill.expose` | `{ name, providers: ProviderKind[], mode, expectedRevision }` | `{ revision, outcome, library, exposures }`; `providers` must identify a managed skill and create exposures for each existing personal/profile target of those providers. |
| `skill.unexpose` | `{ name, providers: ProviderKind[nonempty], expectedRevision }` | Same mutation result; removes matching desired exposures. |
| `skill.remove` | `{ name, expectedRevision }` | Same mutation result; removes the owned library copy and all desired exposures after ownership/content checks. |

Each conflict record is `{ name, scope: "library"|"harness", kind: "duplicate"|"conflict", replaceAllowed, target?, path?, managed? }`; the final three keys occur for harness conflicts. Source traversal accepts at most 4,096 entries including directories, 64 MiB across regular files, 8 MiB per file, and depth 24; skill discovery is depth 3. Git refs containing `/` are disambiguated by explicit `gitRef`.

The mutation `outcome` is `{ operationId, kind: "skill", method, status,
error?, backupDirectory, requiresProviderRestart: true }`, where `status` is
`applied`, `rolled-back`, or `recovery-needed`. Each target-volume skill backup
is an owner-protected sibling of its skill root named
`.lazoo-extension-backup-<uuid>`; the data-root operation journal records the
exact backup paths. A library replacement is never
implicit: differing same-name content needs `replace: true`; unmanaged destination
or shared-root collisions return `LZ-SKILL-UNMANAGED-CONFLICT`. The daemon rejects
unsafe source/path trees, reparse points, special files, traversal, validation
problems, modified staged/library/exposure bytes, and non-owned deletes. Shared
roots are observed only; Lazoo writes its dedicated first root. Newly created
native profiles need a later expose action.

### Plugin RPCs

`PluginAction` is one of `"install"`, `"uninstall"`, `"enable"`, `"disable"`,
`"marketplace-add"`, or `"marketplace-remove"`. `marketplace` is optional
`{ name, source }`; a cross-harness install with it requires `name` to be
`plugin@marketplace-name`.

| RPC | Parameters | Result |
| --- | --- | --- |
| `plugin.list` | `{ target?: ExtensionTarget }` | `{ revision, targets, capabilities, pending }`. Each target is `{ target, observed?, error? }`; observed is `{ plugins, marketplaces, providerVersion, compatibility, limitations }`, and a plugin record is `{ name, enabled?, version?, marketplace?, path? }`. |
| `plugin.marketplace.list` | `{ target?: ExtensionTarget }` | Same envelope; successful `observed` is `{ marketplaces: [{ name, source }] }`. |
| `plugin.preview` | `{ action, targets: ExtensionTarget[1..16 distinct], name, marketplace?: { name, source } }` | A plugin preview described above. Per-target planning errors remain in the preview rather than failing all targets. |
| `plugin.install`, `plugin.uninstall`, `plugin.enable`, `plugin.disable`, `plugin.marketplace.add`, `plugin.marketplace.remove` | `{ previewId, expectedRevision }` | `{ revision, outcome }`. `outcome` is `{ operationId, kind: "plugin", method, targets, sessionEffect: "Requires provider restart" }`; each target has `{ target, status, error?, observed?, rollbackStatus, backupDirectory?, limitations? }`. Status is `applied`, `rolled-back`, `recovery-needed`, `refused`, or `unsupported-or-unavailable`. |

The daemon checks the preview's action, executable, observed native state, and
revision again before a command. It backs up known control files and a recoverable
uninstall package, then observes afterward. It compensates an install only when
the newly installed identity is observed. If native control-file bytes changed,
it does not blindly restore them because it cannot distinguish concurrent external
edits; the result is `recovery-needed` and requires manual inspection. Existing
marketplaces with the requested name but a different or unknown source are
refused; a cross-harness install is read back by both plugin name and marketplace
source. Marketplace add requires `name == marketplace.name`; equivalent
owner/repo and `https://github.com/owner/repo.git` identities compare equal.
The daemon does not claim cross-provider atomicity, overwrite concurrent
native changes, or delete unknown package caches. Native command output and
credentials do not escape the API.

### Revision, errors, and events

`expectedRevision` fences every mutation. The extension ledger revision increases
once for every finished operation, including a rolled-back or recovery-needed one.
Before any mutation it rejects a changed revision with `LZ-EXTENSION-STALE`
(`data.currentRevision`) and an interrupted journal with
`LZ-EXTENSION-RECOVERY-REQUIRED` (`data.revision`, `data.pending`). A skill
preview made at another revision is drift (`LZ-EXTENSION-DRIFT`); a plugin
preview revision/action mismatch is `LZ-INVALID-PARAMS`. Refresh and preview again.

Extension-specific errors are `LZ-INVALID-PARAMS`, `LZ-EXTENSION-UNSAFE`,
`LZ-EXTENSION-LIMIT`, `LZ-EXTENSION-DRIFT`, `LZ-EXTENSION-UNSUPPORTED`,
`LZ-EXTENSION-NOT-DETECTED`, `LZ-EXTENSION-NATIVE-FAILED`,
`LZ-EXTENSION-TIMEOUT`, `LZ-EXTENSION-FORMAT`, `LZ-EXTENSION-IO`,
`LZ-EXTENSION-PREVIEW-EXPIRED`, `LZ-SKILL-CONFLICT`, and
`LZ-SKILL-UNMANAGED-CONFLICT`; unknown extension methods return
`LZ-METHOD-NOT-FOUND`. `plugin.preview` can instead return a successful preview
whose individual target carries its RPC error. Generic parameter parsing uses
`LZ-INVALID-PARAMS` and does not expose native command output.

Each native command permits at most 1 MiB of stdout and 1 MiB of stderr and has a
45-second deadline. Git source preparation has a 60-second deadline while
monitoring a 128 MiB staging directory with at most 8,192 entries. These limits
are independent of the eight active previews and their ten-minute expiry.

The durable event sequence is `extension.operation.started` with the recorded
intent (operation ID, kind, method, targets/journal and exact backup locations), then
`extension.operation.finished` with `{ revision, outcome, recoveryNeeded }`, then
`skill.changed` or `plugin.changed` with `{ revision, operationId }`. A client
should refresh `skill.list` or the relevant plugin list on either changed event;
the operation-start event indicates an in-progress exclusive mutation, not a
completed native change.

### Native capability matrix and limits

Every plugin capability entry includes `compatibility`, `actions`,
`limitations`, `sessionEffect: "Requires provider restart"`, and
`authentication: "Requires user OAuth or native setup"`.

| Provider | Label and actions | Exact limitation |
| --- | --- | --- |
| Claude Code | Native: list, marketplace list/add/remove, install/uninstall, enable/disable | User scope only. |
| Codex | Native: list, marketplace list/add/remove, install/uninstall | Enable/disable is unsupported: there is no native CLI command; the reviewed TOML writer is MCP-only. Current Codex documentation describes `CODEX_HOME/skills` as legacy, but the disposable 0.166.0-alpha.5.1 app-server probe successfully found `lazoo-legacy-probe` there. |
| GitHub Copilot | Native: list, marketplace list/add/remove, install/uninstall | Installed state comes from comment-bearing native config/settings. Enable/disable is unsupported: a disposable Copilot 1.0.79 plugin list succeeded, but both `plugins disable|enable <name> --plugin` returned exit 1, `The plugins command is not available.` |
| Antigravity | Native: list, install/uninstall, enable/disable | Marketplace management and cross-harness Claude-marketplace conversion are unsupported. |
| OpenCode | Translated with documented limitations: list, install | npm modules only; local `.js`/`.ts` modules are inventoried; removal/toggles and Claude-format marketplaces are unsupported. |
| Grok Build | Unsupported | Excluded from core v1. |

Skill exposure has no plugin-specific compatibility matrix: all current harnesses
except Grok Build expose through their configured native skills root, while
Antigravity also reports that its global root is inferred and its installed CLI
has no read-only skills inventory command; its exposure remains unverified. A missing native executable is
`LZ-EXTENSION-NOT-DETECTED`; unsupported roots or native actions are explicit
refusals, never config-file emulation.

Profile environments deliberately match account-session isolation. Codex, Claude
Code, and Copilot profiles isolate their provider config variable but retain the
personal `HOME` and its shared skill roots. OpenCode and Antigravity profiles
isolate `HOME`. The current installed Codex candidate used for this slice is
`0.166.0-alpha.5.1` under
`LocalAppData/OpenAI/Codex/bin/22cfb1d302613eae`; the NVM shim resolves the older
`0.153.4`, so executable discovery must choose the candidate returned by the
daemon rather than a shell shim. The Codex skills evidence is
`artifacts/acceptance/skills-plugins-native-check/codex-skills.json`; it proves
only disposable app-server `skills/list` visibility of the legacy skill. Copilot
plugin-list/toggle evidence is
`artifacts/acceptance/skills-plugins-native-check/copilot-plugins.json`; it proves the listed
toggle commands are unavailable. Copilot's disposable local marketplace add/list/remove
evidence is `artifacts/acceptance/skills-plugins-native-check/copilot-marketplaces.json`;
the native text list includes defaults. No artifact live-tests a network install,
an Antigravity skill exposure, or an extension-operation pass.

## Older conversation history

`workspace.conversation` accepts optional `beforeTurnSequence`. Omit it, or send
null, for the latest page. For an older page, pass the preceding response's
`nextBeforeTurnSequence` and the same conversation ID. The cursor is exclusive;
each page is ordered oldest to newest. `turnLimit` is 1 through 500 (default 50),
and a non-null cursor must be positive and fit a signed 64-bit sequence. Stop when
`nextBeforeTurnSequence` is null. `turnsTruncated` describes older rows beyond the
requested page, not whether newer rows exist above it.

Deduplicate/merge by turn ID when refreshing. A new turn arriving after the first
page cannot move the exclusive boundary of an older page. Only the returned turn
history is paged: the lead header, root task, workers and attention still describe
current state. Fetch an individual turn's text/events with `turn.show`; summary
rows are not complete transcripts. CLI example:

```powershell
lazooctl --json workspace show <conversation-id> --turn-limit 50 --before-turn-sequence <cursor>
```

## Native terminal boundary

Follow the [terminal client contract](superpowers/specs/2026-10-28-terminal-client-contract.md)
for authenticated framing, byte offsets, output gaps, boot/revision fencing, and
input acknowledgement uncertainty. All four governed providers have a genuine concurrent native TUI
view of the same session Lazoo controls. `session.terminal.view` opens it without
stopping the employee or granting another path to submit turns or approvals.
The separate `session.terminal.attach` operation remains a takeover of a suspended
session for these providers. It must not compete with an active controller.

Given `lead.session.id` or a worker card's session ID, call
`session.terminal.status { id }` (CLI: `session terminal-status <id>`). Its fields
are `bootId`, `sessionId`, `sessionRevision`, `opening`, nullable `terminal`,
`attachable`, nullable `blockingCode`, and nullable `blockingReason`. A live
attachment returns the terminal record needed for reconnect and byte I/O; an
opening attachment must be allowed to settle before sending another request.
Known incompatibility is data, while missing sessions and storage faults remain
RPC errors. Additional fields describe the concurrent view: `viewable`, nullable
`viewBlockingCode`/`viewBlockingReason`, `viewOpening`, and nullable `viewTerminal`.
Prefer reconnecting the returned live view over opening another one. Status reads
neither reserve a slot nor activate an employee. An exited terminal may retain
metadata until the bounded cache evicts it; metadata does not retain screen bytes.

Use `sessionRevision` for a new explicit attach request and the terminal's own
revision/boot ID for resize/stop. Eligibility is advisory and is checked again
when attaching or viewing. Terminal
exit is observed through bounded status/output polling, so session audit events
alone are not an exit notification stream.

For an eligible active session, send `session.terminal.view` with `id`,
`expectedRevision`, `columns`, `rows`, and `idempotencyKey` (CLI: `session view`).
Retain the key and exact payload to reconcile a lost reply. A changed payload for
that key is refused; opening a new view after closing the old one needs a new key.
The current contract allows one view per session and eight total live terminals.
Terminal keys and view metadata are bounded and expire with daemon restart.

Send prompts and approval decisions through Lazoo's controls. Codex uses its own
“Viewing sub-agent” label even for a lead; Codex/OpenCode native viewers can navigate
while their restricted bridges refuse controller mutations. Claude/Antigravity
borrow the canonical terminal's output and refuse all keyboard input; implement
scrollback in the terminal renderer and keep resize available. The UI should label
every view as read-only because each native TUI presents that boundary differently.
Closing a view preserves its canonical turn and pending approval; stopping the
employee closes the view. `terminal.show` retains closed metadata until cache
eviction, while output reads can fail once the live handle is removed.
Private loopback capabilities never enter UI data or terminal arguments. The UI
needs only Lazoo's authenticated terminal byte transport.

Production-path acceptance with the installed Codex 0.165.0-alpha.1.2 and a local
Responses simulator proved streaming, collaboration-tool routing, history after
reopen, view close/reopen during a pending approval, decline propagation, status
recovery, and session-stop cleanup. This tests real native rendering and backend
ownership without consuming an external model. The release acceptance and final
full gate both passed; their evidence and limits are in the
[readiness verification](verification/2026-10-29-ui-backend-readiness.md).

## Remaining full-product work

Additional configuration federation formats/plugins/skills and the remaining
whole-product performance windows are not completed by this guide.
Connectors, vectors, and voice remain later phases. The summary worker remains
opt-in and unconfigured by default. Historical live-provider results must be
dated separately from fresh deterministic verification.
