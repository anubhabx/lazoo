# UI provider parity handoff

## Implemented contracts

Codex, OpenCode, Claude Code, and Antigravity are the governed native-provider set. Grok Build
remains excluded. For the governed set, the daemon attaches the same collaboration catalog to a
session, routes each turn through the authority observer, keeps the employee ceiling, and permits
the read-only memory-summary path. Native views are controller-isolated/read-only leases: closing
a view does not own or stop the canonical provider session.

The requested Codex-level parity checkpoint is ready for UI work. All four providers passed
release-daemon native acceptance. Installation, authentication and selected-model readiness
remain live conditions; use the launcher/status fields instead of assuming every installed
provider is currently startable. The [UI integration guide](UI-BACKEND-HANDOFF.md) is updated.

## Native workspace trust

Before Claude Code or Antigravity starts with native workspace access, Lazoo creates a durable,
per-provider, exact-canonical-path trust request. It defaults to `pending`; provider launch waits
without holding a store lock. A pending request is desktop-notified once, can be listed or shown,
and is revision-fenced on resolution. Dropping/cancelling the wait cancels its pending record;
startup reconciliation cancels pending records from an older daemon boot.

User RPCs are `native.trust.list`, `native.trust.show`, `native.trust.resolve`,
`native.trust.policy.show`, `native.trust.policy.set`, and `native.trust.policy.reset`. The CLI
equivalent is `lazooctl native-trust`; for example,
`lazooctl native-trust approve <id> --expected-revision <n> --auto-accept-next-time`.

An approval always applies to its one exact provider/path request. Exact approved paths may be
reused for a later session grant. `autoAcceptManagedNextTime` is provider-scoped and affects only
future Lazoo-owned managed session/task workspaces, never an unmanaged path. Policy set/reset are
revision-fenced; reset returns the policy to false.

## Verification matrix

| Provider | Verified version | Release evidence |
| --- | --- | --- |
| Codex | 0.165.0-alpha.1.2 | 13/13 checks, four local model-fixture requests, zero external model requests. |
| Claude Code | 2.1.312 | 13/13 checks, one bounded prompt in the final run. |
| Antigravity | 1.4.10 | 13/13 checks, one bounded prompt in the final successful run. Earlier 1.4.9 evidence is retained. |
| OpenCode | 1.19.0 | 10/10 checks, three local model-fixture requests, zero external model requests. |

The final native runs use the same release daemon, CLI and session-host binary hashes. They
exercise named employees, durable canonical turns, genuine native rendering, Lazoo collaboration
MCP, command approval/decline, close/reopen while approval is pending, retained history and stop.
Claude/Antigravity also prove default trust notification, explicit approval, future managed-path
opt-in/reset, refused view input, and native conversation identity after resume.

The evidence index is
[`provider-parity-final-manifest.json`](../artifacts/provider-parity/provider-parity-final-manifest.json).
The full workspace suite passed, including 153 daemon integration tests. All 44 gate steps have
passing evidence: the full run passed 43, then a corrected restart harness passed its targeted
rerun and a regression that kept another same-binary daemon alive. This was not one uninterrupted
44/44 run. The final Antigravity-only updater/probe-lifecycle change then passed the complete
provider suite, strict workspace/all-target clippy, release build and the four native runs.
The manifest separately records final CLI stress and preserves original acceptance files by hash.

## Integration details and limits

- Antigravity needs a managed workspace where Lazoo can create its native hook/MCP files;
  existing configuration collisions are refused. Its native self-updater changed 1.4.9 to 1.4.10
  during development. Lazoo now sets the documented `AGY_CLI_DISABLE_AUTO_UPDATE=true` on its
  owned processes and terminates timed-out probe children. Personal settings remain unchanged.
- Claude/Antigravity views borrow the canonical ConPTY. Every byte-input path is refused;
  resize and renderer scrollback remain available. Closing the view does not stop its employee.
- Codex/OpenCode use separate native viewers through restricted authenticated proxies. They
  cannot submit canonical work, answer approvals or mutate provider configuration.
- A stopped view has `live: false` in cached metadata. Output reads may report a missing handle
  after closure. Clients should reconcile through terminal/session status rather than requiring
  one final readable output frame.
- OpenCode needs verified model readiness. Rejecting a native permission may finish its
  generation immediately; the UI must not wait for an additional assistant message.
- Native terminal output uses cursor updates, so a reply can span redraws. The acceptance harness
  renders VT output before asserting visible text. Structured hooks/transcripts remain the
  canonical turn/permission evidence; screen text never authorizes or settles work.

No Grok live calls, frontend work, new connectors, vector search, voice, or wider configuration
federation were added. This is the requested provider-parity starting point, not full-product
acceptance of every deferred core-v1 feature or performance window.
