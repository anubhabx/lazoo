# Provider accounts and GitHub Copilot

Lazoo can choose an isolated account when starting a native provider session and can
route compatible API traffic through a local credential pool. Existing installations
keep their current provider configuration until the owner changes a routing policy.

## Setup

Open **Settings → Accounts**. Add a native profile or an API credential, then choose
the provider's routing policy. Native profiles get their own configuration directory
below Lazoo's data root. **Sign in** opens that provider's own login command in a
writable terminal. Complete sign-in there before enabling pooled native sessions.
Use **Verify routing** after choosing the policy. It sends one short model prompt,
consumes quota, and establishes launcher readiness for that route. Supply a model if
the account restricts models. OpenCode API accounts always require `backend/model`,
so a default/free provider cannot accidentally verify a different route. Verification is never
run automatically when adding an account or opening Settings. Credential and routing
changes invalidate earlier readiness; a personal login probe cannot substitute for it.

API secrets are submitted once, protected with Windows DPAPI for the current user,
and omitted from account lists and audit events. **Rotate key** replaces the secret
using the account's current revision. Password fields clear after submission;
plaintext credentials are not saved in browser storage or placed in process arguments.

| Policy | Behavior |
| --- | --- |
| Use existing behavior | Use the provider's preexisting environment and login. |
| Balance accounts | Select the least-selected eligible account for a new native session. |
| Use accounts in order (`priority`) | Drain the first eligible account in `order`, then move to the next for new sessions. |
| Pin an account | Select the specified account for new sessions. |
| Pool API credentials | Start a session-scoped loopback proxy and select an eligible API credential. |

Eligibility checks enabled/deleted state, model restrictions, cooldowns, structured
exhaustion and observed reserve thresholds. Explicit order breaks balanced selection
ties; unlisted accounts follow by creation timestamp, then UUID. Migrated accounts
recover creation from their earliest retained account audit event, falling back to
their last saved metadata timestamp if that event has expired.
A configured pool with no eligible member fails with `LZ-ACCOUNT`; it does not fall
back to a personal login. Native profile eligibility does not prove that its login or
subscription is ready: authentication remains the native CLI's responsibility.

Policy changes affect new sessions. Existing/resumed sessions retain their account,
profile directory, and native history. Disabling/removing their account prevents a
future resume. A running API proxy checks account availability before each request.
Already-running direct native processes retain their launch environment until stopped.

## Provider capabilities

| Provider | Native profile isolation | Direct key/token | HTTP API pool |
| --- | --- | --- | --- |
| Codex | `CODEX_HOME` | `OPENAI_API_KEY` | Responses, including compact |
| Claude Code | `CLAUDE_CONFIG_DIR` | `ANTHROPIC_API_KEY`; CLI also accepts `CLAUDE_CODE_OAUTH_TOKEN` | Anthropic API keys; Messages, beta Messages and token counts |
| GitHub Copilot | `COPILOT_HOME`, isolated `GH_CONFIG_DIR` | `COPILOT_GITHUB_TOKEN` | No; use native session balancing/pinning |
| OpenCode | Isolated home and XDG config/data/state/cache | OpenAI, Anthropic, Google and OpenRouter keys | `openai` Responses and `anthropic` Messages backends |
| Antigravity | Isolated home and Windows app-data directories | Native login | No; use native session balancing/pinning |

Grok Build remains subject to its existing adapter limitations; this feature does not
admit it into governed execution. Native subscriptions are routed as native sessions.
Lazoo does not extract subscription OAuth tokens, imitate private subscription HTTP
endpoints, or convert subscriptions into API-key quotas.

For OpenCode, use a `backend/model` employee selection matching the configured backend.
Model restrictions accept either the bare upstream model ID or that qualified name.
Managed API-key sessions enable only that backend, including the native takeover terminal;
native profiles keep their own provider choices. This prevents a default or auxiliary
model from escaping the selected credential route.
The native CLI and its own account store continue to own OAuth refresh. Lazoo does not
refresh copied OAuth credentials itself.

Copilot uses the CLI's stdio ACP interface, with Lazoo's permission observer and a
session-scoped authenticated MCP bridge. It supports model/effort selection, streaming,
cancel, and ACP session loading when advertised by the installed version. Version
and ACP probes do not send a model prompt and do not establish subscription readiness.
There is no claimed concurrent Copilot native TUI view. Suspended sessions can be
opened through the native resume command; existing providers keep their actual TUIs.

## API routing and safety

The proxy listens only on loopback with a random per-session access token. Upstream
keys are substituted inside the daemon, never forwarded from the caller's headers.
HTTPS is required for configured upstreams; HTTP is allowed only for loopback fixtures
or a deliberately configured local gateway. Redirects are disabled. Request bodies
are capped at 8 MiB; responses stream without full-response buffering.

New API sessions balance across eligible credentials when policy `order` is empty.
A nonempty `order` makes `api-pool` select in priority order, including early failover.
Before a session's first accepted
model request, an explicit 401, 403 or 429 can try one compatible replacement. Protocol,
backend, credential type and model eligibility must match. Transport failures are
not replayed because delivery is uncertain. Accepted sessions, resumed sessions,
previous response IDs, uploaded file references, and compact requests never rotate
silently to a different credential. Partial streams and client cancellation do not
trigger a replay. The native profile owner and the upstream API owner are persisted
separately so early failover does not lose native transcript history.

Authentication failures cool down an API credential for one hour. HTTP 429 sets both
cooldown and `exhaustedUntil`, using numeric or HTTP-date `Retry-After`, then an
exhausted response-header window's reset, bounded to 1–86,400 seconds (60 seconds
without either). Server errors have a short bounded cooldown. Replacing a secret
clears its authentication cooldown but does not manufacture a quota reset.

Use an API URL prefix such as `https://gateway.example/v1` for custom endpoints.
Anthropic also accepts a server root; Lazoo normalizes the `/v1` difference between
Claude Code's SDK and OpenCode's SDK. Arbitrary routes, WebSockets, uploads, and
arbitrary query strings are not forwarded. Beta Anthropic requests preserve
`?beta=true`. API pools cannot serve a provider feature that needs another route.

## CLI

List redacted records and policy revisions:

```powershell
lazooctl account list
```

Create a native profile (the response supplies its ID/revision):

```powershell
lazooctl account put --provider github-copilot --label Work
lazooctl account login <account-id> --expected-revision 1
lazooctl account policy github-copilot balanced --expected-revision 0
lazooctl provider smoke github-copilot --prompt "Reply OK without using tools"
```

`account login` returns a terminal record. The desktop displays it automatically;
CLI users can operate it through the existing terminal commands. It does not silently
log in or replace the owner's personal provider credentials.

For API credentials, supply the secret on standard input from a password manager or
other private input source. Do not place a literal secret in command history:

```text
<private secret source> | lazooctl account put --provider codex --label Work --credential-environment OPENAI_API_KEY --api-protocol responses --key-stdin
lazooctl account policy codex api-pool --expected-revision 0
```

For an OpenCode Anthropic pool member add `--backend-provider anthropic`, use
`--credential-environment ANTHROPIC_API_KEY` and `--api-protocol anthropic`.
For an OpenCode OpenAI member use `--backend-provider openai` with Responses.
Google/OpenRouter keys support direct session routing, without `--api-protocol`.

`account put --id <id> --expected-revision <revision>` updates an existing record.
Keep its immutable identity fields (provider, credential type, profile directory,
endpoint, API protocol and backend) unchanged. Label, model restrictions, enabled state
and secret can change. `account remove <id> --expected-revision <revision>` clears the
protected key and preserves an ownership tombstone. Native profile files remain on
disk so removing an account does not delete provider history.

## Reserves and observed usage

```powershell
lazooctl account policy codex priority --order <first-id>,<second-id> --expected-revision 1
lazooctl account preferences <second-id> --reserve-percent 25 --expected-revision 1
lazooctl account usage
lazooctl account usage --provider codex --account-id <second-id> --bucket hour
lazooctl account usage --personal --bucket day
```

Omit `--reserve-percent` on `account preferences` to clear the reserve. Its RPC field
is `reservePercent: null`. Reserve changes use the account revision, not the policy
revision. Identity fields remain immutable.

`reservePercent` accepts 0–90. A new session is excluded when any current observed
window reaches `usedPercent >= 100 - reservePercent`. Without a configured reserve,
an observed 100% window still excludes new routing. A usable observation needs a
percentage and a future reset. Missing, expired, or incomplete observations report
`reserveEnforceable: false` and `reserveReason`; Lazoo cannot guarantee a reserve
without provider evidence. Existing sessions are never interrupted to enforce a
reserve, and another application can consume the same quota between observations.

Codex's typed `usageLimitExceeded`/`rateLimitExceeded` errors and Claude's structured
rate-limit failures set `exhaustedUntil`. An exhausted window's reported future reset
is used, bounded to 30 days; absent reset evidence uses 15 minutes. Expiry is checked
at admission and read time, without a timer job or policy revision change. No
free-form terminal or error-message matching is used. Resumes keep their original
account; existing disabled/deleted/cooldown refusals remain in force. A reserve does
not move or exclude an existing native binding.

| Provider / transport | Quota windows | Token ledger | Exhaustion evidence |
| --- | --- | --- | --- |
| Codex app-server | `primary`, `secondary`; percent, minutes, reset, optional plan | Reported thread input/output totals, differenced; first report uses `last` to exclude prior history where supplied | `codexErrorInfo` on error or failed turn |
| Claude native TUI | Supported statusline `rate_limits.five_hour` / `seven_day` in Lazoo-owned sessions | Not reported by these native hooks; context occupancy is excluded | `StopFailure.error == "rate_limit"` |
| Claude stream adapter | Structured `rate_limit_event`, when emitted | `result.usage` input/output, when emitted | Typed `rejected` state, accounting for allowed overage |
| API pool (Codex, Claude, OpenCode) | OpenAI `x-ratelimit-*` or Anthropic `anthropic-ratelimit-*` response headers, per serving credential | Provider events above; proxy counts requests/failures, not response-body token estimates | HTTP 429 and retry/reset headers |
| OpenCode owned HTTP | No native subscription windows; API pool headers when configured | Owned assistant `message.updated` input/output counts, summed across messages and differenced within a turn | No verified native subscription exhaustion code |
| GitHub Copilot ACP | No verified structured subscription windows | Directional `turn_completed.usage` only when supplied; context `usage_update.used` is excluded | No verified quota-specific ACP error schema; generic errors never imply exhaustion |
| Antigravity | No verified subscription windows | Legacy result token fields if emitted; governed native hooks do not report them | Not observable |
| Grok Build | Not observable | Not observable | Existing adapter limitations apply |

`observableProviders` describes adapter capability, not a promise that a particular
account has supplied data. Read the reason and actual nullable totals/windows.
For Claude/Antigravity, `tokens: true` refers only to the legacy machine adapter;
the governed native path remains unreported. OpenCode's input/output fields retain
their provider definitions; separate cache/reasoning counts are not invented or
folded into those directions.

Account reads never launch a model or scrape a subscription endpoint. Codex reads
quota through the already-owned app-server when running a requested turn and consumes
its notifications. Claude's generated session-only settings install a statusline
callback; personal settings files stay untouched. The callback retains only numeric
quota windows in a bounded in-memory slot, replies immediately, and never acts as a
control/permission hook. Active hook processing, account reads and new account
selection flush pending local observations. No `context_window` percentage is quota.
Copilot documents an informational `/usage` command, but its text output is not a
verified structured quota schema; Lazoo does not invoke it automatically. The installed
Copilot package also declares `account.getQuota` on its separate SDK transport; that
transport is not Lazoo's ACP session and is not integrated here. No cross-transport
account attribution or exhaustion behavior is assumed.

## Owner RPC and desktop contract

All methods below are owner control-plane RPCs; they are not worker MCP tools.
Desktop reads use `Kind::Read`, preferences and policy use `Kind::Mutation`.
Optional desktop mutation `idempotencyKey` is accepted; durable revisions fence writes.

```typescript
// provider.accounts.policy (revision is the expected policy revision)
{ provider: string, mode: "inherited" | "balanced" | "priority" | "pinned" | "api-pool",
  accountId?: string | null, revision: number, order?: string[] }
// -> ProviderAccountPolicy { provider, mode, accountId, revision, order }

// provider.accounts.preferences
{ id: string, expectedRevision: number, reservePercent: number | null }
// -> ProviderAccountRecord

// provider.accounts.list {}
// -> { version: 1, accounts: ProviderAccountRecord[], policies: ProviderAccountPolicy[] }
// Record additions:
{ createdAt: string, reservePercent: number | null, exhaustedUntil: string | null,
  reserveEnforceable: boolean, reserveReason: string | null, windows: UsageWindow[] }

// provider.accounts.usage
{ from?: string, to?: string, bucket?: "hour" | "day", provider?: string,
  accountId?: string | null }
// ->
{ buckets: Array<{ start: string, provider: string, accountId: string | null,
    sessions: number, turnsCompleted: number, turnsFailed: number,
    requests: number, failures: number, inputTokens: number | null,
    outputTokens: number | null, tokensReported: number }>,
  windows: UsageWindow[],
  observableProviders: Record<string, { windows: boolean, tokens: boolean, reason: string | null }> }

type UsageWindow = {
  provider: string; accountId: string | null; kind: string;
  usedPercent: number | null; remaining: number | null; limit: number | null;
  windowMinutes: number | null; resetsAt: string | null; observedAt: string;
  source: string; planType: string | null;
};

// EmployeeSessionRecord, LeadSessionSummary and WorkerSessionSummary add:
{ account: { id: string, label: string } | null }
```

Timestamps are RFC3339. `accountId` omitted queries all accounts; explicit null selects
unmanaged personal login. Defaults are the last seven days and UTC day buckets. The
range must be within the last 90 days, at most 90 days long. Hourly storage means
partial-hour endpoints include their intersecting hour. Results exceeding 10,000
buckets or windows fail with an input error instead of silently truncating. Windows
are the latest observed values independently of the selected analytics interval.
No historical usage is backfilled from old transcripts.

Sessions count successful start/resume activations. Completed/failed turns count
durable outcomes (failed submission attempts included), not repeated error messages.
The API owner before acceptance can change only through the existing explicit
rejection flow; the start count moves with that binding CAS. Proxy attempts stay
attributed to the credential actually attempted. Token counters only include provider
reports. `null` means no report for that direction; reported zero stays zero, and
`tokensReported` counts observed directional reports. Codex thread totals and other
per-turn snapshots use durable high-water marks to avoid summing cumulative usage.

Migration 0022 retains 90 days of hourly counters and token watermarks. Aggregation
runs inside existing session/turn/proxy transactions, with no additional ledger
fsync. Latest quota observations contain only allowlisted kinds/sources, numeric
values, timestamps and optional public plan enums. They never include upstream
account IDs, raw headers, credentials, error text or screen contents.

Evidence: installed Codex `0.153.4` schema generation; Claude Code `2.1.316` binary
surface check plus [statusline reference](https://code.claude.com/docs/en/statusline),
[StopFailure hook reference](https://code.claude.com/docs/en/hooks#stopfailure), and
[structured stream reference](https://code.claude.com/docs/en/agent-sdk/typescript#sdkratelimitevent).
Header formats follow [OpenAI](https://developers.openai.com/api/docs/guides/rate-limits)
and [Anthropic](https://platform.claude.com/docs/en/api/rate-limits).
See also [Copilot ACP](https://docs.github.com/en/copilot/reference/copilot-cli-reference/acp-server)
and [OpenCode token schema](https://github.com/anomalyco/opencode/blob/dev/packages/schema/src/token-usage.ts).

Current-slice checks and limits are recorded in the [routing preferences verification](verification/2026-11-03-account-routing-preferences.md).

## Performance and research

Native session selection adds a local database transaction and private environment
setup, without an additional network hop. The API pool adds a loopback HTTP hop,
credential decryption, and a small metrics transaction. Request bodies and streamed
response bytes are preserved. This is an availability/capacity feature; it cannot
increase an individual subscription's quota or make model generation faster.
Keys that share an upstream quota still share it after pooling.

Moving fresh sessions across accounts can reduce prompt-cache reuse, increasing
input usage and first-token latency. Keeping accepted sessions pinned avoids repeated
account churn within a conversation. Adding a remote gateway would also add its network
latency. Real model timing and billing depend on the upstream and cannot be inferred
from a loopback fixture benchmark.

The design was informed by [codex-lb](https://github.com/Soju06/codex-lb) at
`72630bbfeee9e3b0d9acfb95e2b1e706b8b0188e` (MIT), especially the distinction between
[soft cache affinity and hard continuation ownership](https://soju06.github.io/codex-lb/routing/).
No codex-lb source was vendored.

Provider configuration references:

- [Codex custom provider configuration](https://developers.openai.com/siwc/token-sharing-open-source/codex-app-server)
- [Anthropic SDK base URL](https://github.com/anthropics/anthropic-sdk-typescript/blob/main/src/client.ts)
- [Anthropic SDK Messages and beta paths](https://github.com/anthropics/anthropic-sdk-typescript/blob/main/api.md)
- [AI SDK Anthropic URL prefix](https://github.com/vercel/ai/blob/main/content/providers/01-ai-sdk-providers/05-anthropic.mdx)
- [OpenCode provider allowlist and model configuration](https://github.com/anomalyco/opencode/blob/v1.18.29/packages/web/src/content/docs/config.mdx)

See the [verification report](ACCOUNT-ROUTING-VERIFICATION.md) for evidence and measured
overhead. Fixture tests establish routing behavior; they do not prove every native login
or every upstream subscription has been accepted in a live multi-account deployment.
